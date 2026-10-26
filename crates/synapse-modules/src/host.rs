//! A running module: a wasmi instance plus the host functions it may call.
//!
//! ABI (module API v1), all strings UTF-8 JSON:
//! - The module exports `memory`, `syn_alloc(len: i32) -> i32` (a buffer the host may fill) and
//!   `syn_handle(ptr: i32, len: i32)` (handle one request from the app).
//! - Inside `syn_handle` the module answers by calling the host import
//!   `synapse.respond(ptr, len)` once.
//! - To use the host it calls `synapse.call(ptr, len) -> i32` with `{"fn": name, ...args}`; the
//!   host keeps the JSON reply (`{"ok": value}` or `{"error": message}`) and returns its length,
//!   then the module copies it into its own memory with `synapse.take(ptr)`.
//!
//! Every request runs with a fuel budget and the instance's memory is capped, so a broken or
//! hostile module cannot hang or exhaust the app. A trap discards the instance (its state,
//! such as unlocked keys, is lost); the next request starts a fresh one.

use crate::crypto::{Keyring, random_bytes};
use crate::manifest::{Manifest, Permission};
use crate::{Error, Result};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use synapse_core::{Graph, Placement};
use wasmi::{
    Caller, Config, Engine, Instance as WasmInstance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};

/// Fuel for one request (roughly one unit per instruction).
pub const FUEL_PER_REQUEST: u64 = 1_000_000_000;
/// Linear memory cap per instance.
pub const MAX_MEMORY: usize = 64 << 20;
/// Largest request, response or host-call payload.
pub const MAX_MESSAGE: usize = 4 << 20;

pub(crate) struct HostState {
    manifest: Manifest,
    graph: Arc<Mutex<Graph>>,
    keys: Keyring,
    pending: Vec<u8>,
    response: Option<Vec<u8>>,
    limits: StoreLimits,
    log: Vec<String>,
}

pub(crate) fn engine() -> Engine {
    let mut config = Config::default();
    config.consume_fuel(true);
    Engine::new(&config)
}

/// Compiles a module (validation only; nothing runs).
pub(crate) fn compile(engine: &Engine, wasm: &[u8]) -> Result<Module> {
    Module::new(engine, wasm).map_err(|e| Error::Invalid(format!("invalid WebAssembly: {e}")))
}

pub struct Running {
    store: Store<HostState>,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    handle: TypedFunc<(i32, i32), ()>,
}

fn memory_of(caller: &Caller<'_, HostState>) -> Result<Memory, wasmi::Error> {
    caller
        .get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(|| wasmi::Error::new("module exports no memory"))
}

fn read_bytes(caller: &Caller<'_, HostState>, ptr: i32, len: i32) -> Result<Vec<u8>, wasmi::Error> {
    let len = usize::try_from(len).map_err(|_| wasmi::Error::new("negative length"))?;
    if len > MAX_MESSAGE {
        return Err(wasmi::Error::new("message too large"));
    }
    let mut buf = vec![0u8; len];
    memory_of(caller)?
        .read(caller, ptr as u32 as usize, &mut buf)
        .map_err(|_| wasmi::Error::new("out of bounds read"))?;
    Ok(buf)
}

impl Running {
    pub(crate) fn start(
        engine: &Engine,
        module: &Module,
        manifest: Manifest,
        graph: Arc<Mutex<Graph>>,
    ) -> Result<Self> {
        let state = HostState {
            manifest,
            graph,
            keys: Keyring::default(),
            pending: Vec::new(),
            response: None,
            limits: StoreLimitsBuilder::new()
                .memory_size(MAX_MEMORY)
                .instances(1)
                .memories(1)
                .tables(4)
                .build(),
            log: Vec::new(),
        };
        let mut store = Store::new(engine, state);
        store.limiter(|s| &mut s.limits);
        let mut linker = Linker::<HostState>::new(engine);
        linker
            .func_wrap(
                "synapse",
                "call",
                |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<i32, wasmi::Error> {
                    let req = read_bytes(&caller, ptr, len)?;
                    let reply = match dispatch(caller.data_mut(), &req) {
                        Ok(v) => json!({ "ok": v }),
                        Err(e) => json!({ "error": e.to_string() }),
                    };
                    let bytes = serde_json::to_vec(&reply).map_err(|e| wasmi::Error::new(e.to_string()))?;
                    if bytes.len() > MAX_MESSAGE {
                        return Err(wasmi::Error::new("host reply too large"));
                    }
                    let n = bytes.len() as i32;
                    caller.data_mut().pending = bytes;
                    Ok(n)
                },
            )
            .and_then(|l| {
                l.func_wrap(
                    "synapse",
                    "take",
                    |mut caller: Caller<'_, HostState>, ptr: i32| -> Result<(), wasmi::Error> {
                        let bytes = std::mem::take(&mut caller.data_mut().pending);
                        memory_of(&caller)?
                            .write(&mut caller, ptr as u32 as usize, &bytes)
                            .map_err(|_| wasmi::Error::new("out of bounds write"))
                    },
                )
            })
            .and_then(|l| {
                l.func_wrap(
                    "synapse",
                    "respond",
                    |mut caller: Caller<'_, HostState>, ptr: i32, len: i32| -> Result<(), wasmi::Error> {
                        let bytes = read_bytes(&caller, ptr, len)?;
                        caller.data_mut().response = Some(bytes);
                        Ok(())
                    },
                )
            })
            .map_err(|e| Error::Host(e.to_string()))?;
        store
            .set_fuel(FUEL_PER_REQUEST)
            .map_err(|e| Error::Host(e.to_string()))?;
        let instance: WasmInstance = linker
            .instantiate_and_start(&mut store, module)
            .map_err(|e| Error::Module(format!("cannot start: {e}")))?;
        let memory = instance
            .get_memory(&store, "memory")
            .ok_or_else(|| Error::Module("exports no memory".into()))?;
        let alloc = instance
            .get_typed_func::<i32, i32>(&store, "syn_alloc")
            .map_err(|e| Error::Module(format!("syn_alloc: {e}")))?;
        let handle = instance
            .get_typed_func::<(i32, i32), ()>(&store, "syn_handle")
            .map_err(|e| Error::Module(format!("syn_handle: {e}")))?;
        Ok(Self {
            store,
            memory,
            alloc,
            handle,
        })
    }

    /// Sends one request and returns the module's JSON response, with effects the module may
    /// not cause (by permission) removed. An error means the instance must be discarded.
    pub fn request(&mut self, request: &Value) -> Result<Value> {
        let bytes = serde_json::to_vec(request)?;
        if bytes.len() > MAX_MESSAGE {
            return Err(Error::Invalid("request too large".into()));
        }
        self.store
            .set_fuel(FUEL_PER_REQUEST)
            .map_err(|e| Error::Host(e.to_string()))?;
        self.store.data_mut().response = None;
        let ptr = self
            .alloc
            .call(&mut self.store, bytes.len() as i32)
            .map_err(|e| Error::Module(format!("syn_alloc failed: {e}")))?;
        self.memory
            .write(&mut self.store, ptr as u32 as usize, &bytes)
            .map_err(|_| Error::Module("syn_alloc returned a bad pointer".into()))?;
        self.handle
            .call(&mut self.store, (ptr, bytes.len() as i32))
            .map_err(|e| Error::Module(format!("crashed: {e}")))?;
        let out = self
            .store
            .data_mut()
            .response
            .take()
            .ok_or_else(|| Error::Module("gave no response".into()))?;
        let mut v: Value = serde_json::from_slice(&out)
            .map_err(|e| Error::Module(format!("bad response JSON: {e}")))?;
        filter_effects(&self.store.data().manifest, &mut v);
        Ok(v)
    }

    /// Log lines written by the module since the last call (for diagnostics).
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.store.data_mut().log)
    }

    pub fn keys_held(&self) -> usize {
        self.store.data().keys.len()
    }
}

/// Removes `copy` effects from modules without the clipboard permission.
fn filter_effects(manifest: &Manifest, v: &mut Value) {
    if manifest.permissions.contains(&Permission::Clipboard) {
        return;
    }
    if let Some(effects) = v.get_mut("effects").and_then(Value::as_array_mut) {
        effects.retain(|e| e.get("copy").is_none());
    }
}

fn need(state: &HostState, p: Permission) -> Result<()> {
    if state.manifest.permissions.contains(&p) {
        Ok(())
    } else {
        Err(Error::Denied(p.as_str().into()))
    }
}

fn arg_str<'a>(args: &'a Value, name: &str) -> Result<&'a str> {
    args.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid(format!("missing string argument {name:?}")))
}

fn arg_u32(args: &Value, name: &str) -> Result<u32> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Error::Invalid(format!("missing integer argument {name:?}")))
}

fn arg_b64(args: &Value, name: &str) -> Result<Vec<u8>> {
    B64.decode(arg_str(args, name)?)
        .map_err(|_| Error::Invalid(format!("{name:?} is not base64")))
}

fn graph(state: &HostState) -> std::sync::MutexGuard<'_, Graph> {
    state.graph.lock().unwrap_or_else(|p| p.into_inner())
}

/// Runs one host function for the module.
fn dispatch(state: &mut HostState, req: &[u8]) -> Result<Value> {
    let args: Value =
        serde_json::from_slice(req).map_err(|e| Error::Invalid(format!("bad call: {e}")))?;
    let name = arg_str(&args, "fn")?.to_string();
    let id = state.manifest.id.clone();
    match name.as_str() {
        "log" => {
            let msg = arg_str(&args, "msg")?;
            if state.log.len() < 200 {
                state.log.push(msg.chars().take(500).collect());
            }
            Ok(Value::Null)
        }
        "now" => Ok(json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        )),
        "random" => {
            let n = arg_u32(&args, "len")? as usize;
            if n > 4096 {
                return Err(Error::Invalid("at most 4096 random bytes".into()));
            }
            Ok(json!(B64.encode(random_bytes(n)?)))
        }
        "kv.get" => {
            need(state, Permission::Storage)?;
            Ok(json!(
                graph(state).module_data(&id, arg_str(&args, "key")?)?
            ))
        }
        "kv.set" => {
            need(state, Permission::Storage)?;
            let key = arg_str(&args, "key")?;
            let value = match args.get("value") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.as_str()),
                Some(_) => return Err(Error::Invalid("value must be a string or null".into())),
            };
            graph(state).set_module_data(&id, key, value)?;
            Ok(Value::Null)
        }
        "kv.list" => {
            need(state, Permission::Storage)?;
            let prefix = args.get("prefix").and_then(Value::as_str).unwrap_or("");
            let rows = graph(state).module_data_list(&id, prefix)?;
            Ok(Value::Array(
                rows.into_iter()
                    .map(|(k, v)| json!({ "key": k, "value": v }))
                    .collect(),
            ))
        }
        "crypto.derive" => {
            need(state, Permission::Crypto)?;
            let h = state.keys.derive(
                arg_str(&args, "password")?,
                &arg_b64(&args, "salt")?,
                arg_u32(&args, "memory_kib")?,
                arg_u32(&args, "iterations")?,
                arg_u32(&args, "lanes")?,
            )?;
            Ok(json!(h))
        }
        "crypto.generate" => {
            need(state, Permission::Crypto)?;
            Ok(json!(state.keys.generate()?))
        }
        "crypto.seal" => {
            need(state, Permission::Crypto)?;
            let ct = state.keys.seal(
                arg_u32(&args, "key")?,
                arg_str(&args, "plaintext")?.as_bytes(),
                arg_str(&args, "aad")?.as_bytes(),
            )?;
            Ok(json!(B64.encode(ct)))
        }
        "crypto.open" => {
            need(state, Permission::Crypto)?;
            let plain = state.keys.open(
                arg_u32(&args, "key")?,
                &arg_b64(&args, "data")?,
                arg_str(&args, "aad")?.as_bytes(),
            )?;
            let text = std::str::from_utf8(&plain).map_err(|_| Error::Decrypt)?;
            Ok(json!(text))
        }
        "crypto.wrap" => {
            need(state, Permission::Crypto)?;
            let w = state.keys.wrap(
                arg_u32(&args, "key")?,
                arg_u32(&args, "target")?,
                arg_str(&args, "aad")?.as_bytes(),
            )?;
            Ok(json!(B64.encode(w)))
        }
        "crypto.unwrap" => {
            need(state, Permission::Crypto)?;
            let h = state.keys.unwrap_key(
                arg_u32(&args, "key")?,
                &arg_b64(&args, "data")?,
                arg_str(&args, "aad")?.as_bytes(),
            )?;
            Ok(json!(h))
        }
        "crypto.forget" => {
            need(state, Permission::Crypto)?;
            state.keys.forget(arg_u32(&args, "key")?);
            Ok(Value::Null)
        }
        "crypto.forget_all" => {
            need(state, Permission::Crypto)?;
            state.keys.clear();
            Ok(Value::Null)
        }
        "pages.get" => {
            need(state, Permission::PagesRead)?;
            let g = graph(state);
            let Some(page) = g.page_by_title(arg_str(&args, "title")?)? else {
                return Ok(Value::Null);
            };
            let mut md = String::new();
            for (depth, b) in g.page_tree(page.id)? {
                for (i, line) in b.content.lines().enumerate() {
                    md.push_str(&"  ".repeat(depth));
                    md.push_str(if i == 0 { "- " } else { "  " });
                    md.push_str(line);
                    md.push('\n');
                }
            }
            Ok(json!({ "title": page.title, "markdown": md }))
        }
        "pages.append" => {
            need(state, Permission::PagesWrite)?;
            let title = arg_str(&args, "title")?;
            let text = arg_str(&args, "text")?;
            let g = graph(state);
            let page = g.ensure_page(title, None)?;
            let b = g.insert_block(page.id, Placement::LastChildOf(None), text)?;
            Ok(json!({ "page": page.title, "block": b.id.to_string() }))
        }
        other => Err(Error::Invalid(format!("unknown host function {other:?}"))),
    }
}
