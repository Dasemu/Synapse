# Modules

Modules extend Synapse without rebuilding the app. A module is one WebAssembly file that runs
sandboxed in the app (`crates/synapse-modules`, wasmi interpreter: no JIT, so it runs on
Linux, Windows and Android alike) and reaches the app only through host functions allowed by
the permissions the user accepted when installing it. Its UI is data: views and block macros
are JSON trees the app renders.

## Installing
Settings → Modules: modules bundled with the app (`app/assets/modules/`, offered with an
Install button) or **Install from file…** (`.wasm`). The app reads the manifest and shows a
consent dialog with the permissions; installing an update with new permissions asks again.
Ids starting with `synapse.` are reserved for bundled modules (a file cannot pose as one and
inherit its data), and the dialog warns when the graph already holds data under the id
(synced from another device), since the module will be able to read it. A block macro name
belongs to one installed module: installing another module that claims it is refused.
Installed modules live in `<graph>/modules/<id>.wasm` with `modules.json` (enabled flag and
granted permissions), so installation is per device. A file swapped on disk cannot gain
permissions: they are checked against `modules.json` on every start. Removing a module keeps
its data unless "also delete its data" is ticked (that deletion syncs).

## Writing a module
Use the SDK in `modules/sdk` (Rust, `wasm32-unknown-unknown`); `modules/secrets` is a complete
example. Build every bundled module with `scripts/build-modules.sh` (needs
`rustup target add wasm32-unknown-unknown`; it copies `<id>.wasm` into `app/assets/modules/`,
which is committed so the app and the host tests use the same file).

```rust
synapse_module_sdk::manifest!("../manifest.json");
synapse_module_sdk::module!(handle);

fn handle(req: Value) -> Value { json!({ "ui": ui::text("Hello") }) }
```

### Manifest (custom section `synapse.manifest`, JSON)
| Field | Meaning |
|---|---|
| `api` | Module API version, `1` |
| `id` | Lowercase letters, digits, `.`, `-`, `_` (≤ 64), e.g. `synapse.secrets` |
| `name`, `version`, `description`, `author` | Shown in Settings and the consent dialog |
| `permissions` | Any of `storage`, `crypto`, `clipboard`, `pages:read`, `pages:write` |
| `views` | `[{id, title, icon?}]`: sidebar entries (icon is an emoji) |
| `macros` | Names of `{{name args}}` block macros it renders |
| `tick_seconds` | Optional (≥ 5): the app sends `{"op":"tick"}` this often |

### ABI (API v1)
The module exports `memory`, `syn_alloc(len) -> ptr` and `syn_handle(ptr, len)`. The host
writes a JSON request into a buffer from `syn_alloc` and calls `syn_handle`, which answers by
calling the import `synapse.respond(ptr, len)`. Host calls: `synapse.call(ptr, len) -> n`
with `{"fn": name, ...}`, then `synapse.take(ptr)` copies the `n`-byte reply
(`{"ok": value}` or `{"error": message}`). The SDK wraps all of this.

Limits per request: 10⁹ fuel units (about a second of work; a module that runs out is
stopped), where every host call also costs 200,000 units plus 20 per byte in and out; at most
5,000 host calls, 16 MiB written to storage and 3 key derivations. Per instance: 64 MiB of
linear memory, 10,000 table elements, 4 MiB per message. Per module: 64 MiB and 50,000 entries
of storage. A module that traps loses its instance
(and in-memory state); the next request starts a fresh one.

### Requests from the app
| Request | Expected answer |
|---|---|
| `{"op":"render","view":id}` | `{"ui": tree}` |
| `{"op":"event","view":id,"action":a,"arg":s?,"values":{field id: text}}` | `{"ui": tree, "effects": [...]}` |
| `{"op":"macro","name":n,"args":s}` | `{"inline": chip}` |
| `{"op":"macro_action","name":n,"args":s,"action":a}` | `{"inline": chip, "effects": [...]}` |
| `{"op":"tick"}` | `{"changed": bool}` (true redraws views and macros) |
| `{"op":"suspend"}` | `{"changed": bool}`; sent when a phone app is paused |

Any answer may be `{"error": message}`.

**UI tree** nodes (`type`): `column{children}`, `row{children}` (fields and pairs stretch),
`split{left,right}` (side by side when wide, stacked when narrow), `heading{text}`,
`text{text, style: muted|error|mono}`, `divider`, `pair{label, value, mono?, link?}` (links
only for http/https), `field{id, label, value, secret?, multiline?, hint?, autofocus?,
submit?: action, change?: action, icon?}`, `button{label, action, arg?, style:
primary|danger|plain, icon?}`, `list{items, empty}`, `item{title, subtitle, action, arg,
selected?}`. A field keeps what the user typed until the module sends a different `value`.
Icon names: add, lock, unlock, copy, eye, eye_off, edit, delete, key, refresh, save, close,
search, link, person.

**Chip** (macro): `{text, icon?, masked?, revealed?, error?, actions: [{id, label, icon}]}`.

**Effects**: `{"copy": {"text", "clear_after_ms"?}}` (needs `clipboard`; the app clears the
clipboard later only if it still holds that text), `{"toast": text}`,
`{"open_view": view id}`, `{"refresh_macros": true}`.

### Host functions
| `fn` | Permission | Arguments → result |
|---|---|---|
| `log` | — | `msg` |
| `now` | — | → ms since epoch |
| `random` | — | `len` (≤ 4096) → base64 |
| `kv.get` / `kv.set` / `kv.list` | `storage` | `key`, `value` (string or null to delete), `prefix` → `[{key, value}]` |
| `crypto.derive` | `crypto` | `password, salt (b64), memory_kib ≤ 128 MiB, iterations ≤ 10, lanes ≤ 4` → key handle (Argon2id) |
| `crypto.generate` | `crypto` | → random key handle |
| `crypto.seal` / `crypto.open` | `crypto` | `key, plaintext / data (b64), aad` (XChaCha20-Poly1305, random nonce, `nonce‖ct`) |
| `crypto.wrap` / `crypto.unwrap` | `crypto` | `key (kek), target / data, aad` → wrapped key / handle |
| `crypto.forget` / `crypto.forget_all` | `crypto` | drop key handles (keys are zeroed) |
| `pages.get` | `pages:read` | `title` → `{title, markdown}` or null |
| `pages.append` | `pages:write` | `title, text` → adds a block (creates the page) |

Keys never enter the module: it holds handles; the host keeps the keys (zeroed on drop) and
does the cryptography natively (Argon2 inside the interpreter would take seconds). Wrapping
uses its own reserved associated data, so a wrapped key cannot be opened as data and data the
module sealed cannot be unwrapped as a key.

`storage` is the module's own key-value store in the graph (`module_data` table, schema v9),
synced with the op `put_module_data` (last writer wins per key, deletes are tombstones;
keys ≤ 256 bytes, values ≤ 1 MiB). Modules never see each other's data.

## Secrets (`synapse.secrets`)
An encrypted vault: Settings → Modules → Secrets → Install, then the **Secrets** sidebar
view. Create the vault with a master password (≥ 8 characters), add entries (name, username,
password with a generator, URL, notes), and reference one in any block with
`{{secret Name}}`: it shows masked, with buttons to show it (hidden again after 20 s), copy
the password or the username (the clipboard clears after 30 s, and when the app is hidden).
- Crypto: a random 256-bit data key encrypts every entry (XChaCha20-Poly1305, the entry key
  as associated data). Each data key has a header `vault/<key id>` wrapping it with a key
  derived from the master password (Argon2id, 64 MiB, 3 iterations, random salt); `vault`
  points at the current one. Headers synced with weaker or heavier parameters are ignored.
- Changing the password rotates the data key: every entry is re-sealed and the old header
  deleted. Older ciphertext and headers remain in the op log (and the server's) history, so
  someone with an old password and that history can read the entries as they were then, not
  later changes; the same goes for deleted entries.
- Only ciphertext is stored and synced; the server and the op log never see names or values.
  Secrets are not part of history snapshots.
- Locks after 5 minutes without use (checked on every request), on request, when the app is
  hidden or paused, or when the module stops. Forgetting the master password means the
  secrets are lost.
- Two devices that set up a vault before syncing each get a data key; unlocking with the
  same password moves the other key's entries to the current one. A device whose key was
  replaced elsewhere (a password change) locks before writing and asks for the password again.
- Android: the clipboard is not marked as sensitive (keyboards may show it in their clipboard
  history); it is cleared when the app is hidden.
