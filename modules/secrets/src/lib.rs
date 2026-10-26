//! Secrets: an encrypted vault for passwords and other secrets.
//!
//! Storage (the module's synced key-value store, so the server only ever sees ciphertext):
//! - `vault`: header `{v, kdf: {memory_kib, iterations, lanes}, salt, key}`. `key` is a random
//!   data key wrapped (XChaCha20-Poly1305) by a key derived from the master password
//!   (Argon2id). Changing the password re-wraps the data key; entries stay as they are.
//! - `entry/<id>`: one entry (name, username, password, url, notes) sealed with the data key,
//!   with `entry/<id>` bound as associated data so entries cannot be swapped.
//!
//! Keys live in the host (`crypto` host functions); this module only holds handles, and
//! decrypted entries while unlocked. The vault locks after [`AUTO_LOCK_MS`] without use, when
//! the app is suspended, or on request. A revealed password hides again after [`REVEAL_MS`].

use serde::{Deserialize, Serialize};
use synapse_module_sdk::{Value, crypto, json, kv, ui};

synapse_module_sdk::manifest!("../manifest.json");
synapse_module_sdk::module!(handle);

pub const AUTO_LOCK_MS: u64 = 5 * 60 * 1000;
pub const REVEAL_MS: u64 = 20 * 1000;
pub const CLIPBOARD_CLEAR_MS: u64 = 30 * 1000;
pub const MIN_PASSWORD: usize = 8;
const VAULT_KEY: &str = "vault";
const ENTRY_PREFIX: &str = "entry/";
const AAD_VAULT: &str = "synapse.secrets/vault/v1";
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_ITERATIONS: u32 = 3;
const KDF_LANES: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Kdf {
    memory_kib: u32,
    iterations: u32,
    lanes: u32,
}

#[derive(Serialize, Deserialize)]
struct Header {
    v: u32,
    kdf: Kdf,
    salt: String,
    key: String,
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
    dek: Option<crypto::Key>,
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

fn header() -> Result<Option<Header>, String> {
    match kv::get(VAULT_KEY)? {
        None => Ok(None),
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|_| "the vault header is damaged".to_string()),
    }
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

fn new_header(password: &str, dek: crypto::Key) -> Result<Header, String> {
    let salt = synapse_module_sdk::random(16)?;
    let mut h = Header {
        v: 1,
        kdf: Kdf {
            memory_kib: KDF_MEMORY_KIB,
            iterations: KDF_ITERATIONS,
            lanes: KDF_LANES,
        },
        salt: synapse_module_sdk::b64_encode(&salt),
        key: String::new(),
    };
    let kek = derive(password, &h)?;
    let wrapped = crypto::wrap(kek, dek, AAD_VAULT);
    crypto::forget(kek);
    h.key = wrapped?;
    Ok(h)
}

fn save_header(h: &Header) -> Result<(), String> {
    kv::set(
        VAULT_KEY,
        &serde_json::to_string(h).map_err(|e| e.to_string())?,
    )
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

fn load_entries(s: &mut State) -> Result<(), String> {
    let Some(dek) = s.dek else { return Ok(()) };
    let mut entries = Vec::new();
    let mut unreadable = 0;
    for (key, value) in kv::list(ENTRY_PREFIX)? {
        let id = key[ENTRY_PREFIX.len()..].to_string();
        match crypto::open(dek, &value, &entry_aad(&id))
            .ok()
            .and_then(|plain| serde_json::from_str::<Entry>(&plain).ok())
        {
            Some(mut e) => {
                e.id = id;
                entries.push(e);
            }
            None => unreadable += 1,
        }
    }
    entries.sort_by_key(|e| e.name.to_lowercase());
    s.entries = entries;
    s.unreadable = unreadable;
    Ok(())
}

fn save_entry(s: &State, e: &Entry) -> Result<(), String> {
    let dek = s.dek.ok_or("the vault is locked")?;
    let plain = serde_json::to_string(e).map_err(|e| e.to_string())?;
    let sealed = crypto::seal(dek, &plain, &entry_aad(&e.id))?;
    kv::set(&format!("{ENTRY_PREFIX}{}", e.id), &sealed)
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
        let now = synapse_module_sdk::now();
        let op = req.get("op").and_then(Value::as_str).unwrap_or("");
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
            "tick" => Ok(json!({ "changed": tick(&mut s, now) })),
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
            if header()?.is_some() {
                s.message = Some(("A vault already exists: unlock it.".into(), true));
            } else if pw.chars().count() < MIN_PASSWORD {
                s.message = Some((format!("Use at least {MIN_PASSWORD} characters."), true));
            } else if pw != confirm {
                s.message = Some(("The passwords do not match.".into(), true));
            } else {
                let dek = crypto::generate()?;
                save_header(&new_header(pw, dek)?)?;
                s.dek = Some(dek);
                s.entries.clear();
                effects.push(json!({ "refresh_macros": true }));
            }
        }
        "unlock" => {
            let h = header()?.ok_or("there is no vault yet")?;
            let kek = derive(value(req, "password"), &h)?;
            let dek = crypto::unwrap(kek, &h.key, AAD_VAULT);
            crypto::forget(kek);
            match dek {
                Ok(dek) => {
                    s.dek = Some(dek);
                    s.screen = Screen::List;
                    effects.push(json!({ "refresh_macros": true }));
                }
                Err(_) => s.message = Some(("Wrong password.".into(), true)),
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
            } else {
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
            if let Screen::ConfirmDelete(id) = &s.screen {
                kv::delete(&format!("{ENTRY_PREFIX}{id}"))?;
                if s.selected.as_ref() == Some(id) {
                    s.selected = None;
                }
            }
            s.screen = Screen::List;
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
            let h = header()?.ok_or("there is no vault")?;
            let (cur, new, confirm) = (
                value(req, "current"),
                value(req, "new"),
                value(req, "confirm"),
            );
            let kek = derive(cur, &h)?;
            let check = crypto::unwrap(kek, &h.key, AAD_VAULT);
            crypto::forget(kek);
            match check {
                Err(_) => s.message = Some(("The current password is wrong.".into(), true)),
                Ok(k) if new.chars().count() < MIN_PASSWORD => {
                    crypto::forget(k);
                    s.message = Some((format!("Use at least {MIN_PASSWORD} characters."), true));
                }
                Ok(k) if new != confirm => {
                    crypto::forget(k);
                    s.message = Some(("The new passwords do not match.".into(), true));
                }
                Ok(k) => {
                    crypto::forget(k);
                    let dek = s.dek.ok_or("the vault is locked")?;
                    save_header(&new_header(new, dek)?)?;
                    s.screen = Screen::List;
                    s.message = Some(("Master password changed.".into(), false));
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

fn render(s: &mut State) -> Result<Value, String> {
    let now = synapse_module_sdk::now();
    if s.dek.is_some() && now.saturating_sub(s.last_activity) > AUTO_LOCK_MS {
        lock(s);
    }
    let Some(_) = s.dek else {
        let exists = header()?.is_some();
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
                ui::text("It is deleted on every synced device. This cannot be undone."),
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
