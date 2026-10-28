//! Module manifest: identity, permissions and contributions, embedded in the `.wasm` file as a
//! custom section named [`MANIFEST_SECTION`] holding JSON.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// Name of the custom section that holds the manifest JSON.
pub const MANIFEST_SECTION: &str = "synapse.manifest";
/// Module API version this host speaks.
pub const API_VERSION: u32 = 1;

/// What a module may do. Host functions behind a permission fail for modules without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Permission {
    /// Its own key-value store, synced with the graph.
    #[serde(rename = "storage")]
    Storage,
    /// Key derivation (Argon2id) and authenticated encryption (XChaCha20-Poly1305) done by
    /// the host; keys stay in the host.
    #[serde(rename = "crypto")]
    Crypto,
    /// Put text on the clipboard (cleared after a delay).
    #[serde(rename = "clipboard")]
    Clipboard,
    /// Read pages.
    #[serde(rename = "pages:read")]
    PagesRead,
    /// Create pages and add blocks to them.
    #[serde(rename = "pages:write")]
    PagesWrite,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::Storage => "storage",
            Permission::Crypto => "crypto",
            Permission::Clipboard => "clipboard",
            Permission::PagesRead => "pages:read",
            Permission::PagesWrite => "pages:write",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }
}

/// A view the module adds to the sidebar, rendered from the UI tree it returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewDecl {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub api: u32,
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub permissions: Vec<Permission>,
    #[serde(default)]
    pub views: Vec<ViewDecl>,
    /// Names of the `{{name args}}` block macros the module renders.
    #[serde(default)]
    pub macros: Vec<String>,
    /// When set, the app sends a `tick` request this often (seconds, >= 5) while it runs.
    #[serde(default)]
    pub tick_seconds: Option<u32>,
}

fn simple_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

impl Manifest {
    pub fn parse(json: &[u8]) -> Result<Self> {
        let mut m: Manifest = serde_json::from_slice(json)
            .map_err(|e| Error::Invalid(format!("bad manifest: {e}")))?;
        if m.api != API_VERSION {
            return Err(Error::Invalid(format!(
                "module API {} is not supported (this app speaks {API_VERSION})",
                m.api
            )));
        }
        if !synapse_core::is_valid_module_id(&m.id) {
            return Err(Error::Invalid(format!("invalid module id {:?}", m.id)));
        }
        if m.name.trim().is_empty() || m.name.chars().count() > 64 {
            return Err(Error::Invalid("module name must be 1-64 characters".into()));
        }
        if m.version.trim().is_empty() || m.version.len() > 32 {
            return Err(Error::Invalid("invalid module version".into()));
        }
        if m.views.len() > 8 || m.macros.len() > 16 {
            return Err(Error::Invalid("too many views or macros".into()));
        }
        for v in &m.views {
            if !simple_name(&v.id) || v.title.trim().is_empty() || v.title.chars().count() > 64 {
                return Err(Error::Invalid(format!("invalid view {:?}", v.id)));
            }
            if v.icon.as_ref().is_some_and(|i| i.chars().count() > 16) {
                return Err(Error::Invalid(format!("invalid icon of view {:?}", v.id)));
            }
        }
        if let Some(m) = m.macros.iter().find(|n| !simple_name(n)) {
            return Err(Error::Invalid(format!("invalid macro name {m:?}")));
        }
        if m.tick_seconds.is_some_and(|t| t < 5) {
            return Err(Error::Invalid("tick_seconds must be at least 5".into()));
        }
        m.description
            .truncate(m.description.floor_char_boundary(1000));
        m.author.truncate(m.author.floor_char_boundary(100));
        m.permissions.sort();
        m.permissions.dedup();
        Ok(m)
    }

    /// Reads the manifest from the custom section of a WebAssembly binary, without compiling it.
    pub fn from_wasm(wasm: &[u8]) -> Result<Self> {
        let section = custom_section(wasm, MANIFEST_SECTION)?
            .ok_or_else(|| Error::Invalid(format!("no {MANIFEST_SECTION} section")))?;
        Self::parse(section)
    }
}

fn leb_u32(b: &[u8], pos: &mut usize) -> Result<u32> {
    let mut out: u32 = 0;
    for shift in (0..35).step_by(7) {
        let byte = *b
            .get(*pos)
            .ok_or_else(|| Error::Invalid("truncated module".into()))?;
        *pos += 1;
        out |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(out);
        }
    }
    Err(Error::Invalid("bad LEB128 in module".into()))
}

/// The payload of the first custom section called `name`, if any.
fn custom_section<'a>(wasm: &'a [u8], name: &str) -> Result<Option<&'a [u8]>> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        return Err(Error::Invalid("not a WebAssembly module".into()));
    }
    let mut pos = 8;
    while pos < wasm.len() {
        let id = wasm[pos];
        pos += 1;
        let size = leb_u32(wasm, &mut pos)? as usize;
        let end = pos
            .checked_add(size)
            .filter(|&e| e <= wasm.len())
            .ok_or_else(|| Error::Invalid("truncated module".into()))?;
        if id == 0 {
            let mut p = pos;
            let len = leb_u32(wasm, &mut p)? as usize;
            let name_end = p
                .checked_add(len)
                .filter(|&e| e <= end)
                .ok_or_else(|| Error::Invalid("bad custom section".into()))?;
            if &wasm[p..name_end] == name.as_bytes() {
                return Ok(Some(&wasm[name_end..end]));
            }
        }
        pos = end;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wasm_with(section: &str, payload: &[u8]) -> Vec<u8> {
        let mut w = b"\0asm\x01\0\0\0".to_vec();
        let mut body = vec![section.len() as u8];
        body.extend_from_slice(section.as_bytes());
        body.extend_from_slice(payload);
        w.push(0);
        w.push(body.len() as u8); // small payloads only
        w.extend(body);
        w
    }

    #[test]
    fn reads_the_manifest_section() {
        let json = br#"{"api":1,"id":"x.y","name":"X","version":"1","permissions":["crypto","storage","crypto"]}"#;
        let m = Manifest::from_wasm(&wasm_with(MANIFEST_SECTION, json)).unwrap();
        assert_eq!(m.id, "x.y");
        assert_eq!(m.permissions, vec![Permission::Storage, Permission::Crypto]);
    }

    #[test]
    fn refuses_bad_manifests() {
        for json in [
            r#"{"api":2,"id":"x","name":"X","version":"1"}"#,
            r#"{"api":1,"id":"X Y","name":"X","version":"1"}"#,
            r#"{"api":1,"id":"x","name":"X","version":"1","permissions":["network"]}"#,
            r#"{"api":1,"id":"x","name":"X","version":"1","macros":["Bad Name"]}"#,
            r#"{"api":1,"id":"x","name":"X","version":"1","tick_seconds":1}"#,
        ] {
            assert!(
                Manifest::from_wasm(&wasm_with(MANIFEST_SECTION, json.as_bytes())).is_err(),
                "{json}"
            );
        }
        assert!(Manifest::from_wasm(&wasm_with("other", b"{}")).is_err());
        assert!(Manifest::from_wasm(b"nope").is_err());
    }
}
