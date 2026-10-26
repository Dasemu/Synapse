//! Module data: a small key-value store per installed module (`module_data` table, schema v9).
//!
//! Each entry is keyed by (module id, key) and synced with its own op
//! ([`Op::PutModuleData`], last writer wins per entry). A deleted entry is kept as a tombstone
//! (`value` NULL) so the delete wins over older writes from other replicas. The core never
//! interprets values: modules store their own encoding (the secrets module stores ciphertext).

use crate::error::{Error, Result};
use crate::model::Op;
use crate::store::Graph;
use rusqlite::{OptionalExtension, params};

/// Longest module id, in bytes.
pub const MAX_MODULE_ID: usize = 64;
/// Longest key, in bytes.
pub const MAX_MODULE_KEY: usize = 256;
/// Largest value, in bytes.
pub const MAX_MODULE_VALUE: usize = 1 << 20;

/// Module ids are lowercase ASCII letters, digits, `.`, `-` and `_` (e.g. `synapse.secrets`).
pub fn is_valid_module_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_MODULE_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        && !id.starts_with('.')
}

fn check(module: &str, key: &str, value: Option<&str>) -> Result<()> {
    if !is_valid_module_id(module) {
        return Err(Error::Invalid(format!("invalid module id {module:?}")));
    }
    if key.is_empty() || key.len() > MAX_MODULE_KEY || key.chars().any(char::is_control) {
        return Err(Error::Invalid(format!("invalid module key {key:?}")));
    }
    if value.is_some_and(|v| v.len() > MAX_MODULE_VALUE) {
        return Err(Error::Invalid(format!("value of {key:?} is too large")));
    }
    Ok(())
}

impl Graph {
    /// The value of `key` for `module`, `None` when unset or deleted.
    pub fn module_data(&self, module: &str, key: &str) -> Result<Option<String>> {
        Ok(self
            .row(
                "SELECT value FROM module_data WHERE module=?1 AND key=?2",
                params![module, key],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten())
    }

    /// Live entries of `module` whose key starts with `prefix`, sorted by key.
    pub fn module_data_list(&self, module: &str, prefix: &str) -> Result<Vec<(String, String)>> {
        let mut st = self.conn().prepare_cached(
            "SELECT key, value FROM module_data
             WHERE module=?1 AND value IS NOT NULL AND substr(key, 1, length(?2))=?2
             ORDER BY key",
        )?;
        let rows = st
            .query_map(params![module, prefix], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Sets (`Some`) or deletes (`None`) an entry, logged as [`Op::PutModuleData`]. Deleting an
    /// unset key is a no-op.
    pub fn set_module_data(&self, module: &str, key: &str, value: Option<&str>) -> Result<()> {
        check(module, key, value)?;
        let tx = self.tx()?;
        if value.is_none() && self.module_data(module, key)?.is_none() {
            tx.commit()?;
            return Ok(());
        }
        let hlc = self.tick();
        self.write_module_data_row(module, key, value, &hlc)?;
        self.log(
            &hlc,
            &Op::PutModuleData {
                module: module.to_string(),
                key: key.to_string(),
                value: value.map(str::to_string),
            },
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes every live entry of `module` (one op each). Returns how many were deleted.
    pub fn clear_module_data(&self, module: &str) -> Result<usize> {
        let tx = self.tx()?;
        let keys: Vec<String> = self
            .module_data_list(module, "")?
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        for k in &keys {
            self.set_module_data(module, k, None)?;
        }
        tx.commit()?;
        Ok(keys.len())
    }

    pub(crate) fn module_data_hlc(&self, module: &str, key: &str) -> Result<Option<String>> {
        Ok(self
            .row(
                "SELECT hlc FROM module_data WHERE module=?1 AND key=?2",
                params![module, key],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub(crate) fn write_module_data_row(
        &self,
        module: &str,
        key: &str,
        value: Option<&str>,
        hlc: &str,
    ) -> Result<()> {
        self.exec(
            "INSERT OR REPLACE INTO module_data(module, key, value, hlc) VALUES (?1, ?2, ?3, ?4)",
            params![module, key, value, hlc],
        )?;
        Ok(())
    }
}

/// Validates a remote [`Op::PutModuleData`] (same rules as local writes).
pub(crate) fn check_remote(module: &str, key: &str, value: Option<&str>) -> bool {
    check(module, key, value).is_ok()
}
