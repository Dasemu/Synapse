//! Installed modules of a graph: `<graph>/modules/<id>.wasm` plus `<graph>/modules/modules.json`
//! (enabled flag and granted permissions). Installation is local to each device; module data
//! (`module_data`) syncs with the graph.

use crate::host::{self, Running};
use crate::manifest::{Manifest, Permission};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use synapse_core::Graph;
use wasmi::{Engine, Module};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    enabled: bool,
    granted: Vec<Permission>,
}

/// An installed module as the app sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub manifest: Manifest,
    pub enabled: bool,
    /// Size of the `.wasm` file in bytes.
    pub size: u64,
}

struct Loaded {
    manifest: Manifest,
    module: Module,
    running: Option<Running>,
}

pub struct Registry {
    dir: PathBuf,
    graph: Arc<Mutex<Graph>>,
    engine: Engine,
    /// Serialises installs and the state file.
    state: Mutex<BTreeMap<String, Entry>>,
    /// One lock per module so a slow module does not block the others.
    loaded: Mutex<HashMap<String, Arc<Mutex<Option<Loaded>>>>>,
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join("modules.json")
}

impl Registry {
    /// Opens the registry of the graph directory `graph_dir` (creating `modules/` lazily).
    pub fn open(graph_dir: &Path, graph: Arc<Mutex<Graph>>) -> Self {
        let dir = graph_dir.join("modules");
        let state = std::fs::read(state_path(&dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            dir,
            graph,
            engine: host::engine(),
            state: Mutex::new(state),
            loaded: Mutex::new(HashMap::new()),
        }
    }

    fn wasm_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.wasm"))
    }

    fn save(&self, state: &BTreeMap<String, Entry>) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("modules.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
        std::fs::rename(tmp, state_path(&self.dir))?;
        Ok(())
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Entry>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn slot(&self, id: &str) -> Arc<Mutex<Option<Loaded>>> {
        self.loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// Validates a module file without installing it: manifest and WebAssembly.
    pub fn inspect(&self, wasm: &[u8]) -> Result<Manifest> {
        let manifest = Manifest::from_wasm(wasm)?;
        host::compile(&self.engine, wasm)?;
        Ok(manifest)
    }

    /// Installs or updates a module. `granted` must contain every permission the manifest
    /// asks for (the user saw and accepted them). An update keeps the enabled flag.
    pub fn install(&self, wasm: &[u8], granted: &[Permission]) -> Result<ModuleInfo> {
        let manifest = self.inspect(wasm)?;
        if let Some(p) = manifest.permissions.iter().find(|p| !granted.contains(p)) {
            return Err(Error::Denied(format!(
                "permission {} was not granted",
                p.as_str()
            )));
        }
        let mut state = self.lock_state();
        std::fs::create_dir_all(&self.dir)?;
        let path = self.wasm_path(&manifest.id);
        let tmp = path.with_extension("wasm.tmp");
        std::fs::write(&tmp, wasm)?;
        std::fs::rename(&tmp, &path)?;
        let enabled = state.get(&manifest.id).is_none_or(|e| e.enabled);
        state.insert(
            manifest.id.clone(),
            Entry {
                enabled,
                granted: manifest.permissions.clone(),
            },
        );
        self.save(&state)?;
        // Drop a running older version.
        *self
            .slot(&manifest.id)
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = None;
        Ok(ModuleInfo {
            manifest,
            enabled,
            size: wasm.len() as u64,
        })
    }

    /// Installed modules, by name. A module whose file is missing or broken is skipped.
    pub fn list(&self) -> Vec<ModuleInfo> {
        let state = self.lock_state().clone();
        let mut out: Vec<ModuleInfo> = state
            .into_iter()
            .filter_map(|(id, e)| {
                let bytes = std::fs::read(self.wasm_path(&id)).ok()?;
                let manifest = Manifest::from_wasm(&bytes).ok()?;
                (manifest.id == id).then_some(ModuleInfo {
                    manifest,
                    enabled: e.enabled,
                    size: bytes.len() as u64,
                })
            })
            .collect();
        out.sort_by_key(|m| m.manifest.name.to_lowercase());
        out
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        let mut state = self.lock_state();
        let e = state
            .get_mut(id)
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        e.enabled = enabled;
        self.save(&state)?;
        if !enabled {
            self.stop(id);
        }
        Ok(())
    }

    /// Removes the module file. Its synced data is deleted too only when `delete_data` (that
    /// deletion syncs to every device).
    pub fn uninstall(&self, id: &str, delete_data: bool) -> Result<()> {
        let mut state = self.lock_state();
        if state.remove(id).is_none() {
            return Err(Error::NotFound(id.to_string()));
        }
        self.save(&state)?;
        self.stop(id);
        let _ = std::fs::remove_file(self.wasm_path(id));
        if delete_data {
            self.graph
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clear_module_data(id)?;
        }
        Ok(())
    }

    /// Drops the running instance (its in-memory state, such as unlocked keys, is lost).
    pub fn stop(&self, id: &str) {
        if let Some(slot) = self
            .loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
        {
            *slot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    /// Drops every running instance.
    pub fn stop_all(&self) {
        let slots: Vec<_> = self
            .loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        for s in slots {
            *s.lock().unwrap_or_else(|p| p.into_inner()) = None;
        }
    }

    /// Sends a request to an enabled module, starting it if needed. If the module traps, its
    /// instance is discarded and the error returned.
    pub fn call(&self, id: &str, request: &Value) -> Result<Value> {
        let entry = self
            .lock_state()
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        if !entry.enabled {
            return Err(Error::Invalid(format!("module {id} is disabled")));
        }
        let slot = self.slot(id);
        let mut guard = slot.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            let bytes = std::fs::read(self.wasm_path(id))?;
            let manifest = Manifest::from_wasm(&bytes)?;
            if manifest.id != id {
                return Err(Error::Invalid("module file does not match its id".into()));
            }
            // Permissions are the ones granted at install time (a file swapped behind our back
            // cannot gain more).
            if manifest
                .permissions
                .iter()
                .any(|p| !entry.granted.contains(p))
            {
                return Err(Error::Denied(
                    "module asks for permissions not granted".into(),
                ));
            }
            let module = host::compile(&self.engine, &bytes)?;
            *guard = Some(Loaded {
                manifest,
                module,
                running: None,
            });
        }
        let loaded = guard.as_mut().expect("loaded");
        if loaded.running.is_none() {
            loaded.running = Some(Running::start(
                &self.engine,
                &loaded.module,
                loaded.manifest.clone(),
                self.graph.clone(),
            )?);
        }
        let result = loaded.running.as_mut().expect("running").request(request);
        if matches!(result, Err(Error::Module(_))) {
            loaded.running = None;
        }
        result
    }
}
