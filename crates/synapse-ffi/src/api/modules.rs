//! Installed modules: inspect, install, enable, uninstall, and JSON requests to them.
//!
//! Requests and responses are JSON strings (module API v1, see `docs/modules.md`); the app
//! renders the UI trees and applies the effects. None of these calls hold the graph lock
//! while a module runs: host functions lock it per call.

use super::dto::ModuleDto;
use super::graph::SynapseGraph;
use anyhow::{Result, anyhow};
use synapse_modules::Permission;

impl SynapseGraph {
    /// Reads and validates a module file without installing it (for the consent dialog).
    pub fn inspect_module(&self, wasm: Vec<u8>) -> Result<ModuleDto> {
        let m = self.modules.inspect(&wasm)?;
        Ok(ModuleDto::new(&m, false, wasm.len() as u64))
    }

    /// Installs or updates a module; `granted` must list every permission it asks for.
    pub fn install_module(&self, wasm: Vec<u8>, granted: Vec<String>) -> Result<ModuleDto> {
        let granted = granted
            .iter()
            .map(|p| Permission::parse(p).ok_or_else(|| anyhow!("unknown permission {p:?}")))
            .collect::<Result<Vec<_>>>()?;
        let info = self.modules.install(&wasm, &granted)?;
        Ok(ModuleDto::new(&info.manifest, info.enabled, info.size))
    }

    /// Installed modules, by name.
    pub fn modules(&self) -> Vec<ModuleDto> {
        self.modules
            .list()
            .iter()
            .map(|i| ModuleDto::new(&i.manifest, i.enabled, i.size))
            .collect()
    }

    pub fn set_module_enabled(&self, id: String, enabled: bool) -> Result<()> {
        Ok(self.modules.set_enabled(&id, enabled)?)
    }

    /// Removes a module from this device; `delete_data` also deletes its synced data on every
    /// device.
    pub fn uninstall_module(&self, id: String, delete_data: bool) -> Result<()> {
        Ok(self.modules.uninstall(&id, delete_data)?)
    }

    /// Sends a JSON request to a module and returns its JSON response.
    pub fn module_call(&self, id: String, request: String) -> Result<String> {
        let req: serde_json::Value = serde_json::from_str(&request)?;
        Ok(self.modules.call(&id, &req)?.to_string())
    }

    /// Stops a module's running instance (dropping its in-memory state).
    pub fn stop_module(&self, id: String) {
        self.modules.stop(&id);
    }
}
