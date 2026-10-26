//! Helpers for writing Synapse modules (module API v1). See `docs/modules.md`.
//!
//! ```ignore
//! synapse_module_sdk::manifest!("../manifest.json");
//! synapse_module_sdk::module!(handle);
//!
//! fn handle(req: Value) -> Value {
//!     json!({ "ui": ui::text("Hello") })
//! }
//! ```

pub use serde_json::{self, Value, json};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;

#[cfg(target_arch = "wasm32")]
mod sys {
    #[link(wasm_import_module = "synapse")]
    unsafe extern "C" {
        pub fn call(ptr: *const u8, len: i32) -> i32;
        pub fn take(ptr: *mut u8);
        pub fn respond(ptr: *const u8, len: i32);
    }
}

/// Embeds the manifest JSON file in the `synapse.manifest` custom section.
#[macro_export]
macro_rules! manifest {
    ($path:literal) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(link_section = "synapse.manifest")]
        #[used]
        static SYNAPSE_MANIFEST: [u8; include_bytes!($path).len()] = *include_bytes!($path);
    };
}

/// Exports the ABI entry points, routing every request to `$handler: fn(Value) -> Value`.
#[macro_export]
macro_rules! module {
    ($handler:path) => {
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn syn_alloc(len: i32) -> i32 {
            $crate::__alloc(len)
        }

        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn syn_handle(ptr: i32, len: i32) {
            $crate::__handle(ptr, len, $handler)
        }
    };
}

#[doc(hidden)]
pub fn __alloc(len: i32) -> i32 {
    let mut v = Vec::<u8>::with_capacity(len.max(0) as usize);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p as usize as i32
}

#[doc(hidden)]
pub fn __handle(ptr: i32, len: i32, handler: fn(Value) -> Value) {
    let len = len.max(0) as usize;
    // SAFETY: the host filled a buffer obtained from `__alloc(len)`.
    let bytes = unsafe { Vec::from_raw_parts(ptr as usize as *mut u8, len, len) };
    let req = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    drop(bytes);
    let resp = handler(req);
    let out = serde_json::to_vec(&resp).unwrap_or_else(|_| b"{}".to_vec());
    respond(&out);
}

#[cfg(target_arch = "wasm32")]
fn respond(out: &[u8]) {
    // SAFETY: plain FFI with a valid buffer.
    unsafe { sys::respond(out.as_ptr(), out.len() as i32) }
}

#[cfg(not(target_arch = "wasm32"))]
fn respond(_out: &[u8]) {}

/// Calls a host function: `args` is an object, `name` goes in its `fn` field.
pub fn host(name: &str, args: Value) -> Result<Value, String> {
    let mut args = match args {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    args.insert("fn".into(), Value::String(name.into()));
    let req = serde_json::to_vec(&Value::Object(args)).map_err(|e| e.to_string())?;
    let reply = raw_call(&req)?;
    let mut v: Value = serde_json::from_slice(&reply).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    Ok(v.get_mut("ok").map(Value::take).unwrap_or(Value::Null))
}

#[cfg(target_arch = "wasm32")]
fn raw_call(req: &[u8]) -> Result<Vec<u8>, String> {
    // SAFETY: plain FFI; `take` writes exactly the announced length into our buffer.
    unsafe {
        let n = sys::call(req.as_ptr(), req.len() as i32);
        let mut buf = vec![0u8; n.max(0) as usize];
        sys::take(buf.as_mut_ptr());
        Ok(buf)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn raw_call(_req: &[u8]) -> Result<Vec<u8>, String> {
    Err("host functions are only available inside Synapse".into())
}

pub fn log(msg: &str) {
    let _ = host("log", json!({ "msg": msg }));
}

/// Milliseconds since the Unix epoch.
pub fn now() -> u64 {
    host("now", json!({}))
        .ok()
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
}

pub fn random(len: usize) -> Result<Vec<u8>, String> {
    let v = host("random", json!({ "len": len }))?;
    b64_decode(v.as_str().unwrap_or_default())
}

pub fn b64_encode(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s).map_err(|e| e.to_string())
}

/// The module's synced key-value store (`storage` permission).
pub mod kv {
    use super::*;

    pub fn get(key: &str) -> Result<Option<String>, String> {
        Ok(host("kv.get", json!({ "key": key }))?
            .as_str()
            .map(str::to_string))
    }

    pub fn set(key: &str, value: &str) -> Result<(), String> {
        host("kv.set", json!({ "key": key, "value": value })).map(|_| ())
    }

    pub fn delete(key: &str) -> Result<(), String> {
        host("kv.set", json!({ "key": key, "value": null })).map(|_| ())
    }

    pub fn list(prefix: &str) -> Result<Vec<(String, String)>, String> {
        let v = host("kv.list", json!({ "prefix": prefix }))?;
        Ok(v.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| {
                        Some((
                            e.get("key")?.as_str()?.to_string(),
                            e.get("value")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }
}

/// Host cryptography (`crypto` permission). Keys are opaque handles held by the host.
pub mod crypto {
    use super::*;

    pub type Key = u32;

    fn key(v: Value) -> Result<Key, String> {
        v.as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| "bad key handle".to_string())
    }

    /// Argon2id: memory in KiB, iterations, lanes.
    pub fn derive(
        password: &str,
        salt: &[u8],
        memory_kib: u32,
        iterations: u32,
        lanes: u32,
    ) -> Result<Key, String> {
        key(host(
            "crypto.derive",
            json!({
                "password": password, "salt": b64_encode(salt),
                "memory_kib": memory_kib, "iterations": iterations, "lanes": lanes,
            }),
        )?)
    }

    pub fn generate() -> Result<Key, String> {
        key(host("crypto.generate", json!({}))?)
    }

    /// XChaCha20-Poly1305 with a random nonce; returns base64 of `nonce || ciphertext`.
    pub fn seal(key: Key, plaintext: &str, aad: &str) -> Result<String, String> {
        Ok(host(
            "crypto.seal",
            json!({ "key": key, "plaintext": plaintext, "aad": aad }),
        )?
        .as_str()
        .unwrap_or_default()
        .to_string())
    }

    pub fn open(key: Key, data: &str, aad: &str) -> Result<String, String> {
        Ok(host(
            "crypto.open",
            json!({ "key": key, "data": data, "aad": aad }),
        )?
        .as_str()
        .unwrap_or_default()
        .to_string())
    }

    pub fn wrap(kek: Key, target: Key, aad: &str) -> Result<String, String> {
        Ok(host(
            "crypto.wrap",
            json!({ "key": kek, "target": target, "aad": aad }),
        )?
        .as_str()
        .unwrap_or_default()
        .to_string())
    }

    pub fn unwrap(kek: Key, data: &str, aad: &str) -> Result<Key, String> {
        key(host(
            "crypto.unwrap",
            json!({ "key": kek, "data": data, "aad": aad }),
        )?)
    }

    pub fn forget(key: Key) {
        let _ = host("crypto.forget", json!({ "key": key }));
    }

    pub fn forget_all() {
        let _ = host("crypto.forget_all", json!({}));
    }
}

/// Builders for the declarative UI the app renders (see `docs/modules.md`).
pub mod ui {
    use super::*;

    pub fn column(children: Vec<Value>) -> Value {
        json!({ "type": "column", "children": children })
    }

    pub fn row(children: Vec<Value>) -> Value {
        json!({ "type": "row", "children": children })
    }

    /// Two panes side by side on wide screens, stacked on narrow ones.
    pub fn split(left: Value, right: Value) -> Value {
        json!({ "type": "split", "left": left, "right": right })
    }

    pub fn heading(text: &str) -> Value {
        json!({ "type": "heading", "text": text })
    }

    /// `style`: "body", "muted", "error" or "mono".
    pub fn text(text: &str) -> Value {
        json!({ "type": "text", "text": text })
    }

    pub fn styled(text: &str, style: &str) -> Value {
        json!({ "type": "text", "text": text, "style": style })
    }

    pub fn divider() -> Value {
        json!({ "type": "divider" })
    }

    /// A labelled value; `mono` for passwords and codes.
    pub fn pair(label: &str, value: &str, mono: bool) -> Value {
        json!({ "type": "pair", "label": label, "value": value, "mono": mono })
    }

    /// A text field; its current text is sent as `values[id]` with every event.
    pub fn field(id: &str, label: &str, value: &str) -> Value {
        json!({ "type": "field", "id": id, "label": label, "value": value })
    }

    /// `style`: "primary", "danger" or "plain".
    pub fn button(label: &str, action: &str, style: &str) -> Value {
        json!({ "type": "button", "label": label, "action": action, "style": style })
    }

    pub fn list(items: Vec<Value>, empty: &str) -> Value {
        json!({ "type": "list", "items": items, "empty": empty })
    }

    pub fn item(title: &str, subtitle: &str, action: &str, arg: &str) -> Value {
        json!({ "type": "item", "title": title, "subtitle": subtitle, "action": action, "arg": arg })
    }

    /// Sets extra properties on a widget (`with(field(..), json!({"secret": true}))`).
    pub fn with(mut w: Value, extra: Value) -> Value {
        if let (Some(o), Value::Object(e)) = (w.as_object_mut(), extra) {
            o.extend(e);
        }
        w
    }
}
