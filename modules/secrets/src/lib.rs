//! Secrets: an encrypted vault for passwords and other secrets.
//!
//! Storage (the module's synced key-value store, so the server only ever sees ciphertext):
//! - `vault/<key id>`: a header `{v, key_id, kdf: {memory_kib, iterations, lanes}, salt, key}`.
//!   `key` is a random data key wrapped by a key derived from the master password (Argon2id).
//!   Each data key has its own header, so a header written by another device never replaces
//!   one this device still uses.
//! - `vault`: `{current: <key id>}`, the data key new entries use.
//! - `entry/<id>`: `{k: <key id>, c: <ciphertext>}`, one entry (name, username, password,
//!   url, notes) sealed (XChaCha20-Poly1305) with data key `k`, with `entry/<id>` bound as
//!   associated data so entries cannot be swapped.
//!
//! Unlocking tries the password on every header; entries under another unlocked data key
//! (two devices that set up a vault before syncing) are re-sealed under the current one and
//! the extra headers dropped. Changing the password rotates the data key: every entry is
//! re-sealed and the old header deleted, so an old password no longer opens current data
//! (older ciphertext stays in the op log history; see docs/modules.md).
//!
//! Keys live in the host (`crypto` host functions); this module only holds handles, and
//! decrypted entries while unlocked. The vault locks after [`AUTO_LOCK_MS`] without use (checked
//! on every request), when the app is suspended or hidden, or on request. A revealed password
//! hides again after [`REVEAL_MS`].

use serde::{Deserialize, Serialize};
use synapse_module_sdk::{Value, crypto, json, kv, ui};

synapse_module_sdk::manifest!("../manifest.json");
synapse_module_sdk::module!(handle);

pub const AUTO_LOCK_MS: u64 = 5 * 60 * 1000;
pub const REVEAL_MS: u64 = 20 * 1000;
pub const CLIPBOARD_CLEAR_MS: u64 = 30 * 1000;
pub const MIN_PASSWORD: usize = 8;
const POINTER_KEY: &str = "vault";
const HEADER_PREFIX: &str = "vault/";
const ENTRY_PREFIX: &str = "entry/";
const AAD_VAULT: &str = "synapse.secrets/vault/v1/";
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_LANES: u32 = 1;
/// Headers tried when unlocking (each costs one Argon2 run).
const MAX_HEADERS: usize = 3;

#[derive(Serialize, Deserialize)]
struct Kdf {
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
}

impl Kdf {
    /// Synced headers are untrusted: refuse parameters that are too weak to protect the
    /// password or too heavy for a phone.
    fn acceptable(&self) -> bool {
        (32 * 1024..=128 * 1024).contains(&self.memory_kib)
            && (2..=6).contains(&self.iterations)
            && (1..=4).contains(&self.lanes)
    }
}

#[derive(Serialize, Deserialize)]
struct Header {
    v: u32,
    key_id: String,
    kdf: Kdf,
    salt: String,
    key: String,
}

#[derive(Serialize, Deserialize)]
struct Pointer {
    current: String,
}

#[derive(Serialize, Deserialize)]
struct Sealed {
    k: String,
    c: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Entry {
    #[serde(skip)]
    id: String,
    name: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    updated: u64,
}

#[derive(Clone, PartialEq)]
enum Screen {
    List,
    Edit(Option<String>),
    ConfirmDelete(String),
    ChangePassword,
}

struct State {
    /// The current data key: its id and handle.
    dek: Option<(String, crypto::Key)>,
    entries: Vec<Entry>,
    unreadable: usize,
    last_activity: u64,
    screen: Screen,
    filter: String,
    selected: Option<String>,
    /// When the selected entry's password was revealed.
    revealed_at: Option<u64>,
    /// Entry names (lowercase) revealed inline in blocks, with when.
    inline_revealed: Vec<(String, u64)>,
    /// Form values kept across renders (edit form after "Generate").
    form: serde_json::Map<String, Value>,
    message: Option<(String, bool)>,
}

impl State {
    fn new() -> Self {
        Self {
            dek: None,
            entries: Vec::new(),
            unreadable: 0,
            last_activity: 0,
            screen: Screen::List,
            filter: String::new(),
            selected: None,
            revealed_at: None,
            inline_revealed: Vec::new(),
            form: serde_json::Map::new(),
            message: None,
        }
    }
}

thread_local! {
    static STATE: std::cell::RefCell<State> = std::cell::RefCell::new(State::new());
}

fn entry_aad(id: &str) -> String {
    format!("synapse.secrets/{ENTRY_PREFIX}{id}")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A random password from letters, digits and symbols (rejection sampling, no bias).
pub fn generate_password(len: usize) -> Result<String, String> {
    const ALPHABET: &[u8] =
        b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!#$%&*+-=?@^_~";
    let limit = 256 - (256 % ALPHABET.len());
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in synapse_module_sdk::random(len * 2)? {
            if (b as usize) < limit && out.len() < len {
                out.push(ALPHABET[b as usize % ALPHABET.len()] as char);
            }
        }
    }
    Ok(out)
}

fn pointer() -> Result<Option<String>, String> {
    Ok(kv::get(POINTER_KEY)?
        .and_then(|s| serde_json::from_str::<Pointer>(&s).ok())
        .map(|p| p.current))
}

/// Valid headers, the current one first.
fn headers() -> Result<Vec<Header>, String> {
    let current = pointer()?;
    let mut hs: Vec<Header> = kv::list(HEADER_PREFIX)?
        .into_iter()
        .filter_map(|(k, v)| {
            let h: Header = serde_json::from_str(&v).ok()?;
            (k[HEADER_PREFIX.len()..] == h.key_id && h.kdf.acceptable()).then_some(h)
        })
        .collect();
    hs.sort_by_key(|h| Some(&h.key_id) != current.as_ref());
    Ok(hs)
}

fn derive(password: &str, h: &Header) -> Result<crypto::Key, String> {
    let salt = synapse_module_sdk::b64_decode(&h.salt)?;
    crypto::derive(
        password,
        &salt,
        h.kdf.memory_kib,
        h.kdf.iterations,
        h.kdf.lanes,
    )
}

/// The data key of `h` if `password` opens it.
fn open_header(password: &str, h: &Header) -> Result<Option<crypto::Key>, String> {
    let kek = derive(password, h)?;
    let dek = crypto::unwrap(kek, &h.key, &format!("{AAD_VAULT}{}", h.key_id));
    crypto::forget(kek);
    Ok(dek.ok())
}

/// Writes the header of a data key and makes it current.
fn save_header(password: &str, key_id: &str, dek: crypto::Key) -> Result<(), String> {
    let salt = synapse_module_sdk::random(16)?;
    let mut h = Header {
        v: 1,
        key_id: key_id.to_string(),
        kdf: Kdf {
            memory_kib: KDF_MEMORY_KIB,
            iterations: KDF_ITERATIONS,
            lanes: KDF_LANES,
        },
        salt: synapse_module_sdk::b64_encode(&salt),
        key: String::new(),
    };
    let kek = derive(password, &h)?;
    let wrapped = crypto::wrap(kek, dek, &format!("{AAD_VAULT}{key_id}"));
    crypto::forget(kek);
    h.key = wrapped?;
    kv::set(&format!("{HEADER_PREFIX}{key_id}"), &to_json(&h)?)?;
    set_pointer(key_id)
}

fn set_pointer(key_id: &str) -> Result<(), String> {
    kv::set(
        POINTER_KEY,
        &to_json(&Pointer {
            current: key_id.to_string(),
        })?,
    )
}

fn to_json<T: Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|e| e.to_string())
}

fn lock(s: &mut State) {
    crypto::forget_all();
    s.dek = None;
    s.entries.clear();
    s.unreadable = 0;
    s.screen = Screen::List;
    s.selected = None;
    s.revealed_at = None;
    s.inline_revealed.clear();
    s.form.clear();
}

fn sealed_entries() -> Result<Vec<(String, Sealed)>, String> {
    Ok(kv::list(ENTRY_PREFIX)?
        .into_iter()
        .filter_map(|(k, v)| {
            let sealed = serde_json::from_str::<Sealed>(&v).ok()?;
            Some((k[ENTRY_PREFIX.len()..].to_string(), sealed))
        })
        .collect())
}

fn seal_entry(key_id: &str, dek: crypto::Key, e: &Entry) -> Result<(), String> {
    let plain = serde_json::to_string(e).map_err(|e| e.to_string())?;
    let c = crypto::seal(dek, &plain, &entry_aad(&e.id))?;
    let sealed = Sealed {
        k: key_id.to_string(),
        c,
    };
    kv::set(&format!("{ENTRY_PREFIX}{}", e.id), &to_json(&sealed)?)
}

fn open_entry(id: &str, dek: crypto::Key, sealed: &Sealed) -> Option<Entry> {
    let plain = crypto::open(dek, &sealed.c, &entry_aad(id)).ok()?;
    let mut e: Entry = serde_json::from_str(&plain).ok()?;
    e.id = id.to_string();
    Some(e)
}

/// Re-seals every entry under `from` keys with `to` and returns how many moved.
fn reseal(from: &[(String, crypto::Key)], to: &(String, crypto::Key)) -> Result<usize, String> {
    let mut moved = 0;
    for (id, sealed) in sealed_entries()? {
        let Some((_, k)) = from.iter().find(|(kid, _)| *kid == sealed.k) else {
            continue;
        };
        if let Some(e) = open_entry(&id, *k, &sealed) {
            seal_entry(&to.0, to.1, &e)?;
            moved += 1;
        }
    }
    Ok(moved)
}

/// Tries `password` on the vault headers. On success the current data key is set, and
/// entries under other data keys it opened are moved to it.
fn unlock(s: &mut State, password: &str) -> Result<bool, String> {
    let hs = headers()?;
    if hs.is_empty() {
        return Err("there is no vault yet".into());
    }
    let mut keys = Vec::new();
    for h in hs.iter().take(MAX_HEADERS) {
        if let Some(k) = open_header(password, h)? {
            keys.push((h.key_id.clone(), k));
        }
    }
    if keys.is_empty() {
        return Ok(false);
    }
    let current = keys.remove(0);
    if !keys.is_empty() {
        reseal(&keys, &current)?;
        for (id, k) in &keys {
            kv::delete(&format!("{HEADER_PREFIX}{id}"))?;
            crypto::forget(*k);
        }
    }
    if pointer()?.as_ref() != Some(&current.0) {
        set_pointer(&current.0)?;
    }
    let skipped = hs.len() - keys.len() - 1;
    if skipped > 0 {
        s.message = Some((
            format!(
                "{skipped} other vault key(s) use a different password; their entries stay \
                 unreadable until unlocked with it."
            ),
            true,
        ));
    }
    s.dek = Some(current);
    Ok(true)
}

/// False (and the vault locks) when another device replaced the current data key: writing
/// with ours would create entries nobody else can read.
fn still_current(s: &mut State) -> Result<bool, String> {
    let Some((id, _)) = &s.dek else {
        return Ok(false);
    };
    let ok = pointer()?.as_ref() == Some(id) && kv::get(&format!("{HEADER_PREFIX}{id}"))?.is_some();
    if !ok {
        lock(s);
        s.message = Some((
            "The vault changed on another device. Unlock it again.".into(),
            true,
        ));
    }
    Ok(ok)
}

fn load_entries(s: &mut State) -> Result<(), String> {
    let Some((key_id, dek)) = s.dek.clone() else {
        return Ok(());
    };
    let mut entries = Vec::new();
    let mut unreadable = 0;
    for (key, value) in kv::list(ENTRY_PREFIX)? {
        let id = &key[ENTRY_PREFIX.len()..];
        match serde_json::from_str::<Sealed>(&value)
            .ok()
            .filter(|sealed| sealed.k == key_id)
            .and_then(|sealed| open_entry(id, dek, &sealed))
        {
            Some(e) => entries.push(e),
            None => unreadable += 1,
        }
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    s.entries = entries;
    s.unreadable = unreadable;
    Ok(())
}

fn save_entry(s: &State, e: &Entry) -> Result<(), String> {
    let (key_id, dek) = s.dek.as_ref().ok_or("the vault is locked")?;
    seal_entry(key_id, *dek, e)
}

fn find<'a>(s: &'a State, name: &str) -> Option<&'a Entry> {
    let key = name.trim().to_lowercase();
    s.entries
        .iter()
        .find(|e| e.name.trim().to_lowercase() == key)
}

fn value<'a>(req: &'a Value, id: &str) -> &'a str {
    req.get("values")
        .and_then(|v| v.get(id))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn copy(text: &str) -> Value {
    json!({ "copy": { "text": text, "clear_after_ms": CLIPBOARD_CLEAR_MS } })
}

fn toast(text: &str) -> Value {
    json!({ "toast": text })
}

pub fn handle(req: Value) -> Value {
    STATE.with(|cell| {
        let mut s = cell.borrow_mut();
        let was_unlocked = s.dek.is_some();
        let now = synapse_module_sdk::now();
        let op = req.get("op").and_then(Value::as_str).unwrap_or("");
        // Every request checks the idle timeout before counting as activity, so a late tick
        // cannot leave the vault usable.
        if s.dek.is_some() && now.saturating_sub(s.last_activity) > AUTO_LOCK_MS {
            lock(&mut s);
            s.message = Some(("Locked after inactivity.".into(), false));
        }
        let result = match op {
            "render" => render(&mut s).map(|ui| json!({ "ui": ui })),
            "event" => {
                s.last_activity = now;
                event(&mut s, &req, now)
            }
            "macro" => Ok(json!({ "inline": inline(&s, args(&req), now) })),
            "macro_action" => {
                s.last_activity = now;
                macro_action(&mut s, &req, now)
            }
            "tick" => {
                Ok(json!({ "changed": tick(&mut s, now) || was_unlocked && s.dek.is_none() }))
            }
            "suspend" => {
                let was = s.dek.is_some();
                lock(&mut s);
                Ok(json!({ "changed": was }))
            }
            _ => Err(format!("unknown request {op:?}")),
        };
        result.unwrap_or_else(|e| json!({ "error": e }))
    })
}

fn args(req: &Value) -> &str {
    req.get("args").and_then(Value::as_str).unwrap_or("").trim()
}

fn tick(s: &mut State, now: u64) -> bool {
    let mut changed = false;
    if s.dek.is_some() && now.saturating_sub(s.last_activity) > AUTO_LOCK_MS {
        lock(s);
        s.message = Some(("Locked after inactivity.".into(), false));
        return true;
    }
    if s.revealed_at
        .is_some_and(|t| now.saturating_sub(t) > REVEAL_MS)
    {
        s.revealed_at = None;
        changed = true;
    }
    let before = s.inline_revealed.len();
    s.inline_revealed
        .retain(|(_, t)| now.saturating_sub(*t) <= REVEAL_MS);
    changed || before != s.inline_revealed.len()
}

fn event(s: &mut State, req: &Value, now: u64) -> Result<Value, String> {
    let action = req.get("action").and_then(Value::as_str).unwrap_or("");
    let arg = req
        .get("arg")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut effects: Vec<Value> = Vec::new();
    s.message = None;
    match action {
        "setup" => {
            let (pw, confirm) = (value(req, "password"), value(req, "confirm"));
            if !headers()?.is_empty() {
                s.message = Some(("A vault already exists: unlock it.".into(), true));
            } else if pw.chars().count() < MIN_PASSWORD {
                s.message = Some((format!("Use at least {MIN_PASSWORD} characters."), true));
            } else if pw != confirm {
                s.message = Some(("The passwords do not match.".into(), true));
            } else {
                let dek = crypto::generate()?;
                let key_id = hex(&synapse_module_sdk::random(8)?);
                save_header(pw, &key_id, dek)?;
                s.dek = Some((key_id, dek));
                s.entries.clear();
                effects.push(json!({ "refresh_macros": true }));
            }
        }
        "unlock" => {
            if unlock(s, value(req, "password"))? {
                s.screen = Screen::List;
                effects.push(json!({ "refresh_macros": true }));
            } else {
                s.message = Some(("Wrong password.".into(), true));
            }
        }
        "lock" => {
            lock(s);
            effects.push(json!({ "refresh_macros": true }));
        }
        _ if s.dek.is_none() => {
            s.message = Some(("The vault is locked.".into(), true));
        }
        "filter" => s.filter = value(req, "filter").to_string(),
        "select" => {
            s.selected = Some(arg);
            s.revealed_at = None;
            s.screen = Screen::List;
        }
        "new" => {
            s.form.clear();
            s.screen = Screen::Edit(None);
        }
        "edit" => {
            s.form.clear();
            s.screen = Screen::Edit(Some(arg));
        }
        "generate" => {
            if let Some(v) = req.get("values").and_then(Value::as_object) {
                s.form = v.clone();
            }
            s.form
                .insert("password".into(), Value::String(generate_password(20)?));
        }
        "save" => {
            let name = value(req, "name").trim().to_string();
            let id = match &s.screen {
                Screen::Edit(id) => id.clone(),
                _ => None,
            };
            if name.is_empty() {
                s.form = req
                    .get("values")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                s.message = Some(("The name is required.".into(), true));
            } else if s.entries.iter().any(|e| {
                e.name.trim().to_lowercase() == name.to_lowercase() && Some(&e.id) != id.as_ref()
            }) {
                s.form = req
                    .get("values")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                s.message = Some((format!("There is already an entry called {name}."), true));
            } else if still_current(s)? {
                let id = match id {
                    Some(id) => id,
                    None => hex(&synapse_module_sdk::random(12)?),
                };
                let e = Entry {
                    id: id.clone(),
                    name,
                    username: value(req, "username").to_string(),
                    password: value(req, "password").to_string(),
                    url: value(req, "url").trim().to_string(),
                    notes: value(req, "notes").to_string(),
                    updated: now,
                };
                save_entry(s, &e)?;
                s.form.clear();
                s.selected = Some(id);
                s.screen = Screen::List;
                effects.push(json!({ "refresh_macros": true }));
            }
        }
        "cancel" => {
            s.form.clear();
            s.screen = Screen::List;
        }
        "delete" => s.screen = Screen::ConfirmDelete(arg),
        "confirm_delete" => {
            if let Screen::ConfirmDelete(id) = s.screen.clone()
                && still_current(s)?
            {
                kv::delete(&format!("{ENTRY_PREFIX}{id}"))?;
                if s.selected.as_ref() == Some(&id) {
                    s.selected = None;
                }
                s.screen = Screen::List;
            }
            effects.push(json!({ "refresh_macros": true }));
        }
        "reveal" => {
            s.revealed_at = match s.revealed_at {
                Some(_) => None,
                None => Some(now),
            }
        }
        "copy_password" | "copy_username" => {
            load_entries(s)?;
            if let Some(e) = s.entries.iter().find(|e| e.id == arg) {
                let (text, what) = if action == "copy_password" {
                    (&e.password, "Password")
                } else {
                    (&e.username, "Username")
                };
                effects.push(copy(text));
                effects.push(toast(&format!(
                    "{what} copied; the clipboard clears in 30 s."
                )));
            }
        }
        "change_password" => s.screen = Screen::ChangePassword,
        "save_password" => {
            let (cur, new, confirm) = (
                value(req, "current"),
                value(req, "new"),
                value(req, "confirm"),
            );
            if new.chars().count() < MIN_PASSWORD {
                s.message = Some((format!("Use at least {MIN_PASSWORD} characters."), true));
            } else if new != confirm {
                s.message = Some(("The new passwords do not match.".into(), true));
            } else if still_current(s)? {
                let (old_id, old_dek) = s.dek.clone().ok_or("the vault is locked")?;
                let hs = headers()?;
                let h = hs
                    .iter()
                    .find(|h| h.key_id == old_id)
                    .ok_or("the vault header is missing")?;
                match open_header(cur, h)? {
                    None => s.message = Some(("The current password is wrong.".into(), true)),
                    Some(k) => {
                        crypto::forget(k);
                        // Rotate the data key: an old password (and old headers kept by sync
                        // history) must not open what is stored from now on.
                        let new_dek = crypto::generate()?;
                        let new_id = hex(&synapse_module_sdk::random(8)?);
                        let next = (new_id.clone(), new_dek);
                        reseal(&[(old_id.clone(), old_dek)], &next)?;
                        save_header(new, &new_id, new_dek)?;
                        for h in &hs {
                            if h.key_id != new_id {
                                kv::delete(&format!("{HEADER_PREFIX}{}", h.key_id))?;
                            }
                        }
                        crypto::forget(old_dek);
                        s.dek = Some(next);
                        s.screen = Screen::List;
                        s.message = Some(("Master password changed.".into(), false));
                    }
                }
            }
        }
        other => return Err(format!("unknown action {other:?}")),
    }
    Ok(json!({ "ui": render(s)?, "effects": effects }))
}

fn inline(s: &State, name: &str, now: u64) -> Value {
    if name.is_empty() {
        return json!({ "text": "secret", "icon": "🔑", "error": "Write {{secret Name}}" });
    }
    if s.dek.is_none() {
        return json!({
            "text": name, "icon": "🔒", "masked": "locked",
            "actions": [{ "id": "unlock", "label": "Unlock the vault", "icon": "lock" }],
        });
    }
    let Some(e) = find(s, name) else {
        return json!({ "text": name, "icon": "🔑", "error": "No such secret" });
    };
    let key = e.name.trim().to_lowercase();
    let revealed = s
        .inline_revealed
        .iter()
        .any(|(n, t)| *n == key && now.saturating_sub(*t) <= REVEAL_MS);
    let mut actions = vec![
        json!({ "id": "reveal", "label": if revealed { "Hide" } else { "Show" }, "icon": if revealed { "eye_off" } else { "eye" } }),
        json!({ "id": "copy", "label": "Copy password", "icon": "copy" }),
    ];
    if !e.username.is_empty() {
        actions.push(json!({ "id": "copy_username", "label": "Copy username", "icon": "person" }));
    }
    json!({
        "text": e.name, "icon": "🔑",
        "masked": if revealed { e.password.clone() } else { "••••••••".to_string() },
        "revealed": revealed,
        "actions": actions,
    })
}

fn macro_action(s: &mut State, req: &Value, now: u64) -> Result<Value, String> {
    let name = args(req).to_string();
    let action = req.get("action").and_then(Value::as_str).unwrap_or("");
    let mut effects = Vec::new();
    if action == "unlock" || s.dek.is_none() {
        effects.push(json!({ "open_view": "vault" }));
        return Ok(json!({ "inline": inline(s, &name, now), "effects": effects }));
    }
    load_entries(s)?;
    if let Some(e) = find(s, &name).cloned() {
        let key = e.name.trim().to_lowercase();
        match action {
            "reveal" => {
                let before = s.inline_revealed.len();
                s.inline_revealed.retain(|(n, _)| *n != key);
                if before == s.inline_revealed.len() {
                    s.inline_revealed.push((key, now));
                }
            }
            "copy" => {
                effects.push(copy(&e.password));
                effects.push(toast("Password copied; the clipboard clears in 30 s."));
            }
            "copy_username" => {
                effects.push(copy(&e.username));
                effects.push(toast("Username copied."));
            }
            _ => {}
        }
    }
    Ok(json!({ "inline": inline(s, &name, now), "effects": effects }))
}

fn message(s: &State) -> Vec<Value> {
    s.message
        .iter()
        .map(|(m, err)| ui::styled(m, if *err { "error" } else { "muted" }))
        .collect()
}

fn secret_field(id: &str, label: &str, value: &str) -> Value {
    ui::with(ui::field(id, label, value), json!({ "secret": true }))
}

/// The current screen's tree. Its `key` names the screen: the app starts with fresh fields
/// when it changes, so text typed on one screen never shows up on another.
fn render(s: &mut State) -> Result<Value, String> {
    let tree = render_screen(s)?;
    let key = match (&s.dek, &s.screen) {
        (None, _) => "locked".to_string(),
        (_, Screen::List) => "list".to_string(),
        (_, Screen::Edit(id)) => format!("edit:{}", id.as_deref().unwrap_or("")),
        (_, Screen::ConfirmDelete(_)) => "delete".to_string(),
        (_, Screen::ChangePassword) => "password".to_string(),
    };
    Ok(ui::with(tree, json!({ "key": key })))
}

fn render_screen(s: &mut State) -> Result<Value, String> {
    let now = synapse_module_sdk::now();
    if s.dek.is_some() && now.saturating_sub(s.last_activity) > AUTO_LOCK_MS {
        lock(s);
    }
    let Some(_) = s.dek else {
        let exists = !headers()?.is_empty();
        let mut c = vec![ui::heading("Secrets")];
        if exists {
            c.push(ui::styled("The vault is locked.", "muted"));
            c.push(ui::with(
                secret_field("password", "Master password", ""),
                json!({ "autofocus": true, "submit": "unlock" }),
            ));
            c.extend(message(s));
            c.push(ui::button("Unlock", "unlock", "primary"));
        } else {
            c.push(ui::text(
                "Create a vault protected by a master password. It encrypts every secret on \
                 this device before it is stored or synced; without the password nobody, \
                 including you, can read them again.",
            ));
            c.push(ui::with(
                secret_field("password", "Master password", ""),
                json!({ "autofocus": true }),
            ));
            c.push(ui::with(
                secret_field("confirm", "Repeat the password", ""),
                json!({ "submit": "setup" }),
            ));
            c.extend(message(s));
            c.push(ui::button("Create vault", "setup", "primary"));
        }
        return Ok(ui::column(c));
    };
    load_entries(s)?;
    Ok(match s.screen.clone() {
        Screen::Edit(id) => edit_form(s, id.as_deref()),
        Screen::ConfirmDelete(id) => {
            let name = s
                .entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.name.clone())
                .unwrap_or_default();
            ui::column(vec![
                ui::heading(&format!("Delete {name}?")),
                ui::text("It is deleted on every synced device."),
                ui::row(vec![
                    ui::button("Delete", "confirm_delete", "danger"),
                    ui::button("Cancel", "cancel", "plain"),
                ]),
            ])
        }
        Screen::ChangePassword => {
            let mut c = vec![
                ui::heading("Change master password"),
                secret_field("current", "Current password", ""),
                secret_field("new", "New password", ""),
                ui::with(
                    secret_field("confirm", "Repeat the new password", ""),
                    json!({ "submit": "save_password" }),
                ),
            ];
            c.extend(message(s));
            c.push(ui::row(vec![
                ui::button("Change", "save_password", "primary"),
                ui::button("Cancel", "cancel", "plain"),
            ]));
            ui::column(c)
        }
        Screen::List => list_screen(s, now),
    })
}

fn list_screen(s: &State, now: u64) -> Value {
    let filter = s.filter.trim().to_lowercase();
    let items: Vec<Value> = s
        .entries
        .iter()
        .filter(|e| {
            filter.is_empty()
                || e.name.to_lowercase().contains(&filter)
                || e.username.to_lowercase().contains(&filter)
                || e.url.to_lowercase().contains(&filter)
        })
        .map(|e| {
            ui::with(
                ui::item(&e.name, &e.username, "select", &e.id),
                json!({ "selected": s.selected.as_ref() == Some(&e.id) }),
            )
        })
        .collect();
    let mut left = vec![
        ui::row(vec![
            ui::with(
                ui::button("New", "new", "primary"),
                json!({ "icon": "add" }),
            ),
            ui::with(
                ui::button("Lock", "lock", "plain"),
                json!({ "icon": "lock" }),
            ),
            ui::with(
                ui::button("Change password", "change_password", "plain"),
                json!({ "icon": "key" }),
            ),
        ]),
        ui::with(
            ui::field("filter", "Search", &s.filter),
            json!({ "change": "filter", "icon": "search" }),
        ),
    ];
    left.extend(message(s));
    if s.unreadable > 0 {
        left.push(ui::styled(
            &format!(
                "{} entries could not be decrypted (another vault key?).",
                s.unreadable
            ),
            "error",
        ));
    }
    left.push(ui::list(items, "No secrets yet."));
    let right = match s
        .selected
        .as_ref()
        .and_then(|id| s.entries.iter().find(|e| &e.id == id))
    {
        None => ui::styled(
            "Select a secret. In any block, {{secret Name}} shows it masked with copy buttons.",
            "muted",
        ),
        Some(e) => {
            let revealed = s
                .revealed_at
                .is_some_and(|t| now.saturating_sub(t) <= REVEAL_MS);
            let mut c = vec![ui::heading(&e.name)];
            if !e.username.is_empty() {
                c.push(ui::row(vec![
                    ui::pair("Username", &e.username, false),
                    ui::with(
                        ui::button("Copy", "copy_username", "plain"),
                        json!({ "arg": e.id, "icon": "copy" }),
                    ),
                ]));
            }
            c.push(ui::row(vec![
                ui::pair(
                    "Password",
                    if revealed {
                        &e.password
                    } else {
                        "••••••••"
                    },
                    true,
                ),
                ui::with(
                    ui::button(if revealed { "Hide" } else { "Show" }, "reveal", "plain"),
                    json!({ "icon": if revealed { "eye_off" } else { "eye" } }),
                ),
                ui::with(
                    ui::button("Copy", "copy_password", "plain"),
                    json!({ "arg": e.id, "icon": "copy" }),
                ),
            ]));
            if !e.url.is_empty() {
                c.push(ui::with(
                    ui::pair("URL", &e.url, false),
                    json!({ "link": true }),
                ));
            }
            if !e.notes.is_empty() {
                c.push(ui::pair("Notes", &e.notes, false));
            }
            c.push(ui::styled(
                &format!("Use it in a block: {{{{secret {}}}}}", e.name),
                "muted",
            ));
            c.push(ui::row(vec![
                ui::with(
                    ui::button("Edit", "edit", "plain"),
                    json!({ "arg": e.id, "icon": "edit" }),
                ),
                ui::with(
                    ui::button("Delete", "delete", "danger"),
                    json!({ "arg": e.id, "icon": "delete" }),
                ),
            ]));
            ui::column(c)
        }
    };
    ui::split(ui::column(left), right)
}

fn edit_form(s: &State, id: Option<&str>) -> Value {
    let base = id
        .and_then(|id| s.entries.iter().find(|e| e.id == id))
        .cloned()
        .unwrap_or_default();
    let get = |k: &str, d: &str| -> String {
        s.form
            .get(k)
            .and_then(Value::as_str)
            .map_or_else(|| d.to_string(), str::to_string)
    };
    let mut c = vec![
        ui::heading(if id.is_some() {
            "Edit secret"
        } else {
            "New secret"
        }),
        ui::with(
            ui::field("name", "Name", &get("name", &base.name)),
            json!({ "autofocus": id.is_none() }),
        ),
        ui::field("username", "Username", &get("username", &base.username)),
        ui::row(vec![
            secret_field("password", "Password", &get("password", &base.password)),
            ui::with(
                ui::button("Generate", "generate", "plain"),
                json!({ "icon": "refresh" }),
            ),
        ]),
        ui::field("url", "URL", &get("url", &base.url)),
        ui::with(
            ui::field("notes", "Notes", &get("notes", &base.notes)),
            json!({ "multiline": true }),
        ),
    ];
    c.extend(message(s));
    c.push(ui::row(vec![
        ui::with(
            ui::button("Save", "save", "primary"),
            json!({ "icon": "save" }),
        ),
        ui::button("Cancel", "cancel", "plain"),
    ]));
    ui::column(c)
}
