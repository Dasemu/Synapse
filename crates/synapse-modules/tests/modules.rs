use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use synapse_core::Graph;
use synapse_modules::{Error, MANIFEST_SECTION, Permission, Registry};

const SECRETS: &str = "synapse.secrets";

fn secrets_wasm() -> Vec<u8> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../app/assets/modules/synapse.secrets.wasm");
    std::fs::read(&p)
        .unwrap_or_else(|e| panic!("{}: {e} (run scripts/build-modules.sh)", p.display()))
}

fn setup() -> (tempfile::TempDir, Arc<Mutex<Graph>>, Registry) {
    let dir = tempfile::tempdir().unwrap();
    let graph = Arc::new(Mutex::new(Graph::open(dir.path()).unwrap()));
    let reg = Registry::open(dir.path(), graph.clone());
    (dir, graph, reg)
}

fn all_perms() -> Vec<Permission> {
    vec![
        Permission::Storage,
        Permission::Crypto,
        Permission::Clipboard,
        Permission::PagesRead,
        Permission::PagesWrite,
    ]
}

/// A module from WAT, with a manifest section appended.
fn wat_module(wat: &str, manifest: Value) -> Vec<u8> {
    let mut wasm = wat::parse_str(wat).unwrap();
    let json = serde_json::to_vec(&manifest).unwrap();
    let mut body = Vec::new();
    leb(&mut body, MANIFEST_SECTION.len());
    body.extend_from_slice(MANIFEST_SECTION.as_bytes());
    body.extend_from_slice(&json);
    wasm.push(0);
    leb(&mut wasm, body.len());
    wasm.extend(body);
    wasm
}

fn leb(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let b = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn texts(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Array(a) => a.iter().for_each(|x| texts(x, out)),
        Value::Object(o) => o.values().for_each(|x| texts(x, out)),
        _ => {}
    }
}

fn ui_text(resp: &Value) -> String {
    let mut s = String::new();
    texts(&resp["ui"], &mut s);
    s
}

fn event(reg: &Registry, action: &str, values: Value, arg: &str) -> Value {
    let r = reg
        .call(
            SECRETS,
            &json!({ "op": "event", "view": "vault", "action": action, "values": values, "arg": arg }),
        )
        .unwrap();
    assert!(r.get("error").is_none(), "{action}: {r}");
    r
}

#[test]
fn secrets_vault_end_to_end() {
    let (_dir, graph, reg) = setup();
    let wasm = secrets_wasm();
    let m = reg.inspect(&wasm).unwrap();
    assert_eq!(m.id, SECRETS);
    assert_eq!(
        m.permissions,
        vec![
            Permission::Storage,
            Permission::Crypto,
            Permission::Clipboard
        ]
    );
    // Installing needs every requested permission granted.
    assert!(matches!(
        reg.install(&wasm, &[Permission::Storage]),
        Err(Error::Denied(_))
    ));
    reg.install(&wasm, &m.permissions).unwrap();
    assert_eq!(reg.list().len(), 1);

    let r = reg
        .call(SECRETS, &json!({ "op": "render", "view": "vault" }))
        .unwrap();
    assert!(ui_text(&r).contains("Create vault"), "{r}");

    let r = event(
        &reg,
        "setup",
        json!({ "password": "short", "confirm": "short" }),
        "",
    );
    assert!(ui_text(&r).contains("at least 8"));
    let r = event(
        &reg,
        "setup",
        json!({ "password": "correct horse", "confirm": "correct horse" }),
        "",
    );
    assert!(ui_text(&r).contains("No secrets yet"), "{r}");

    event(&reg, "new", json!({}), "");
    let r = event(
        &reg,
        "save",
        json!({ "name": "GitHub", "username": "dario", "password": "hunter2-XYZ", "url": "https://github.com", "notes": "" }),
        "",
    );
    assert!(ui_text(&r).contains("GitHub"));
    assert!(!ui_text(&r).contains("hunter2-XYZ"), "masked by default");

    // Stored data is ciphertext only.
    {
        let g = graph.lock().unwrap();
        let rows = g.module_data_list(SECRETS, "").unwrap();
        assert_eq!(rows.len(), 2, "{rows:?}");
        for (_, v) in &rows {
            assert!(!v.contains("hunter2") && !v.contains("GitHub") && !v.contains("dario"));
        }
    }

    // Macro: masked, copy gives a clipboard effect.
    let r = reg
        .call(
            SECRETS,
            &json!({ "op": "macro", "name": "secret", "args": "github" }),
        )
        .unwrap();
    assert_eq!(r["inline"]["text"], "GitHub");
    assert_ne!(r["inline"]["masked"], "hunter2-XYZ");
    let r = reg
        .call(
            SECRETS,
            &json!({ "op": "macro_action", "name": "secret", "args": "GitHub", "action": "copy" }),
        )
        .unwrap();
    assert_eq!(r["effects"][0]["copy"]["text"], "hunter2-XYZ");
    let r = reg
        .call(
            SECRETS,
            &json!({ "op": "macro_action", "name": "secret", "args": "GitHub", "action": "reveal" }),
        )
        .unwrap();
    assert_eq!(r["inline"]["masked"], "hunter2-XYZ");
    let r = reg
        .call(
            SECRETS,
            &json!({ "op": "macro", "name": "secret", "args": "Nope" }),
        )
        .unwrap();
    assert!(r["inline"]["error"].is_string());

    // Lock: the macro no longer reveals anything.
    event(&reg, "lock", json!({}), "");
    let r = reg
        .call(
            SECRETS,
            &json!({ "op": "macro_action", "name": "secret", "args": "GitHub", "action": "copy" }),
        )
        .unwrap();
    assert_eq!(r["effects"][0]["open_view"], "vault");
    assert!(!r.to_string().contains("hunter2"));

    let r = event(&reg, "unlock", json!({ "password": "wrong horse" }), "");
    assert!(ui_text(&r).contains("Wrong password"));
    let r = event(&reg, "unlock", json!({ "password": "correct horse" }), "");
    assert!(ui_text(&r).contains("GitHub"));

    // Change the password: the old one stops working, entries survive.
    event(&reg, "change_password", json!({}), "");
    event(
        &reg,
        "save_password",
        json!({ "current": "correct horse", "new": "battery staple", "confirm": "battery staple" }),
        "",
    );
    // A restart (or crash) drops the instance: locked again.
    reg.stop(SECRETS);
    let r = event(&reg, "unlock", json!({ "password": "correct horse" }), "");
    assert!(ui_text(&r).contains("Wrong password"));
    let r = event(&reg, "unlock", json!({ "password": "battery staple" }), "");
    assert!(ui_text(&r).contains("GitHub"));

    // Suspend locks.
    let r = reg.call(SECRETS, &json!({ "op": "suspend" })).unwrap();
    assert_eq!(r["changed"], true);

    // Uninstall keeps synced data unless asked.
    reg.uninstall(SECRETS, false).unwrap();
    assert_eq!(
        graph
            .lock()
            .unwrap()
            .module_data_list(SECRETS, "")
            .unwrap()
            .len(),
        2
    );
    reg.install(&wasm, &m.permissions).unwrap();
    reg.uninstall(SECRETS, true).unwrap();
    assert!(
        graph
            .lock()
            .unwrap()
            .module_data_list(SECRETS, "")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn vault_data_syncs_as_ciphertext_and_unlocks_on_another_replica() {
    let (_a_dir, a_graph, a) = setup();
    let (_b_dir, b_graph, b) = setup();
    let wasm = secrets_wasm();
    let perms = a.inspect(&wasm).unwrap().permissions;
    a.install(&wasm, &perms).unwrap();
    b.install(&wasm, &perms).unwrap();
    event(
        &a,
        "setup",
        json!({ "password": "correct horse", "confirm": "correct horse" }),
        "",
    );
    event(&a, "new", json!({}), "");
    event(
        &a,
        "save",
        json!({ "name": "Bank", "password": "s3cret!" }),
        "",
    );
    let ops = a_graph.lock().unwrap().ops_since(0, 10_000).unwrap();
    {
        let g = b_graph.lock().unwrap();
        for op in &ops {
            if matches!(op.op, synapse_core::Op::PutModuleData { .. }) {
                assert!(!serde_json::to_string(&op.op).unwrap().contains("s3cret"));
                g.apply_remote(&op.hlc, &op.op).unwrap();
            }
        }
    }
    let r = b
        .call(
            SECRETS,
            &json!({ "op": "event", "action": "unlock", "values": { "password": "correct horse" } }),
        )
        .unwrap();
    assert!(ui_text(&r).contains("Bank"), "{r}");
    let r = b
        .call(
            SECRETS,
            &json!({ "op": "macro_action", "name": "secret", "args": "bank", "action": "copy" }),
        )
        .unwrap();
    assert_eq!(r["effects"][0]["copy"]["text"], "s3cret!");
}

const ECHO: &str = r#"
(module
  (import "synapse" "call" (func $call (param i32 i32) (result i32)))
  (import "synapse" "take" (func $take (param i32)))
  (import "synapse" "respond" (func $respond (param i32 i32)))
  (memory (export "memory") 2)
  (global $next (mut i32) (i32.const 1024))
  (func (export "syn_alloc") (param $n i32) (result i32)
    (local $p i32)
    global.get $next
    local.set $p
    global.get $next
    local.get $n
    i32.add
    global.set $next
    local.get $p)
  ;; Forwards the request to the host as a host call and returns the host's reply.
  (func (export "syn_handle") (param $p i32) (param $n i32)
    (local $len i32)
    local.get $p
    local.get $n
    call $call
    local.set $len
    i32.const 32768
    call $take
    i32.const 32768
    local.get $len
    call $respond))
"#;

fn manifest(id: &str, perms: &[&str]) -> Value {
    json!({ "api": 1, "id": id, "name": id, "version": "1", "permissions": perms })
}

#[test]
fn host_functions_need_their_permission() {
    let (_dir, graph, reg) = setup();
    let wasm = wat_module(ECHO, manifest("test.echo", &[]));
    reg.install(&wasm, &[]).unwrap();
    let r = reg
        .call(
            "test.echo",
            &json!({ "fn": "kv.set", "key": "k", "value": "v" }),
        )
        .unwrap();
    assert!(
        r["error"].as_str().unwrap().contains("permission denied"),
        "{r}"
    );
    let r = reg
        .call("test.echo", &json!({ "fn": "crypto.generate" }))
        .unwrap();
    assert!(r["error"].is_string());
    let r = reg
        .call("test.echo", &json!({ "fn": "pages.get", "title": "x" }))
        .unwrap();
    assert!(r["error"].is_string());
    let r = reg.call("test.echo", &json!({ "fn": "now" })).unwrap();
    assert!(r["ok"].as_u64().unwrap() > 0);

    let wasm = wat_module(
        ECHO,
        manifest("test.store", &["storage", "pages:write", "pages:read"]),
    );
    reg.install(&wasm, &all_perms()).unwrap();
    let r = reg
        .call(
            "test.store",
            &json!({ "fn": "kv.set", "key": "k", "value": "v" }),
        )
        .unwrap();
    assert!(r.get("error").is_none(), "{r}");
    let r = reg
        .call(
            "test.store",
            &json!({ "fn": "pages.append", "title": "Inbox", "text": "from a module" }),
        )
        .unwrap();
    assert_eq!(r["ok"]["page"], "Inbox");
    let r = reg
        .call(
            "test.store",
            &json!({ "fn": "pages.get", "title": "inbox" }),
        )
        .unwrap();
    assert_eq!(r["ok"]["markdown"], "- from a module\n");
    let g = graph.lock().unwrap();
    // Each module only sees its own namespace.
    assert_eq!(
        g.module_data("test.store", "k").unwrap().as_deref(),
        Some("v")
    );
    assert_eq!(g.module_data("test.echo", "k").unwrap(), None);
}

#[test]
fn copy_effects_need_the_clipboard_permission() {
    let (_dir, _graph, reg) = setup();
    // Echo answers with the host reply; make the module answer an arbitrary JSON instead by
    // sending a request the host rejects... simpler: a module that responds with its request.
    let wat = r#"
(module
  (import "synapse" "respond" (func $respond (param i32 i32)))
  (memory (export "memory") 2)
  (func (export "syn_alloc") (param i32) (result i32) i32.const 1024)
  (func (export "syn_handle") (param $p i32) (param $n i32)
    local.get $p
    local.get $n
    call $respond))
"#;
    let wasm = wat_module(wat, manifest("test.mirror", &[]));
    reg.install(&wasm, &[]).unwrap();
    let req = json!({ "effects": [{ "copy": { "text": "x" } }, { "toast": "hi" }] });
    let r = reg.call("test.mirror", &req).unwrap();
    assert_eq!(r["effects"], json!([{ "toast": "hi" }]));
}

#[test]
fn a_runaway_module_is_stopped_and_restarted() {
    let (_dir, _graph, reg) = setup();
    let wat = r#"
(module
  (memory (export "memory") 1)
  (func (export "syn_alloc") (param i32) (result i32) i32.const 1024)
  (func (export "syn_handle") (param i32 i32)
    (loop $l br $l)))
"#;
    let wasm = wat_module(wat, manifest("test.loop", &[]));
    reg.install(&wasm, &[]).unwrap();
    assert!(matches!(
        reg.call("test.loop", &json!({})),
        Err(Error::Module(_))
    ));
    // Still callable (a fresh instance), still stopped by the fuel limit.
    assert!(matches!(
        reg.call("test.loop", &json!({})),
        Err(Error::Module(_))
    ));
}

#[test]
fn memory_is_capped() {
    let (_dir, _graph, reg) = setup();
    let wat = r#"
(module
  (memory (export "memory") 1)
  (func (export "syn_alloc") (param i32) (result i32) i32.const 1024)
  (func (export "syn_handle") (param i32 i32)
    ;; 2000 pages = 125 MiB, over the cap: memory.grow returns -1, then trap.
    i32.const 2000
    memory.grow
    i32.const -1
    i32.eq
    if unreachable end))
"#;
    let wasm = wat_module(wat, manifest("test.mem", &[]));
    reg.install(&wasm, &[]).unwrap();
    assert!(matches!(
        reg.call("test.mem", &json!({})),
        Err(Error::Module(_))
    ));
}

#[test]
fn disabled_and_unknown_modules_refuse_calls() {
    let (dir, graph, reg) = setup();
    let wasm = wat_module(ECHO, manifest("test.echo", &[]));
    reg.install(&wasm, &[]).unwrap();
    reg.set_enabled("test.echo", false).unwrap();
    assert!(reg.call("test.echo", &json!({ "fn": "now" })).is_err());
    assert!(reg.call("nope", &json!({})).is_err());
    // State survives reopening.
    let reg2 = Registry::open(dir.path(), graph);
    assert!(!reg2.list()[0].enabled);
}

#[test]
fn a_swapped_file_cannot_gain_permissions() {
    let (dir, _graph, reg) = setup();
    reg.install(&wat_module(ECHO, manifest("test.echo", &[])), &[])
        .unwrap();
    let evil = wat_module(ECHO, manifest("test.echo", &["storage"]));
    std::fs::write(dir.path().join("modules/test.echo.wasm"), evil).unwrap();
    assert!(matches!(
        reg.call("test.echo", &json!({ "fn": "now" })),
        Err(Error::Denied(_))
    ));
}
