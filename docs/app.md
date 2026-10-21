# App (Flutter) and FFI

## Layout
- `crates/synapse-ffi`: the only API the app sees. `src/api/` is scanned by
  `flutter_rust_bridge` (v2.13): `graph.rs` (opaque `SynapseGraph` handle, reads), `outline.rs`
  (block editing), `search.rs` (title/block lookup until `synapse-search` is wired), `dto.rs`.
  Extension points for search/AI/history/CalDAV/sync are documented in `src/api/mod.rs`.
- `app/`: Flutter client. `lib/src/rust/` is generated; `rust_builder/` is the cargokit glue
  that builds `synapse-ffi` as part of `flutter build`/`run` (paths point at
  `../crates/synapse-ffi`).
- `app/lib/src/`: `backend/` (`GraphBackend` interface + FFI implementation), `state/`
  (app state, write queue, outline controller and algorithms), `editor/` (block row, editor,
  Markdown renderer, autocomplete), `views/` (journals, page, timeline, all pages, quick
  switcher, shell), `theme/`.

## Toolchain
- Flutter stable (3.47+) on `PATH`, Rust stable via rustup, `cargo install
  flutter_rust_bridge_codegen --version 2.13.0 --locked`.
- rustup's `~/.cargo/bin` must come **before** `/usr/bin` in `PATH`. Fedora's packaged `cargo`
  (rust 1.99) cannot reuse artifacts built by rustup's toolchain (1.97) and cargokit picks up
  whichever `cargo` is first: `export PATH="$HOME/.cargo/bin:$PATH"` (check with
  `cargo --version`), or remove the distro `cargo`/`rust` packages.
- Linux desktop: `clang cmake ninja-build gtk3-devel` (Fedora) / `clang cmake ninja-build
  libgtk-3-dev` (Debian/Ubuntu).
- Android: Android SDK + NDK (cargokit installs the Rust targets with rustup).
- Windows: Visual Studio 2022 with "Desktop development with C++", Rust MSVC toolchain.

## Build and run
```bash
cd app
flutter run -d linux            # debug, builds the Rust crate via cargokit
flutter build linux --release   # build/linux/x64/release/bundle/
flutter run -d <android-device> # or: flutter build apk --release
flutter build windows --release # on Windows
```
The graph lives in the platform app-support dir (`~/.local/share/es.zenvairo.synapse/graph`
on Linux). Override it with `SYNAPSE_GRAPH=/path/to/graph`.

## After changing the FFI API
```bash
cd app && flutter_rust_bridge_codegen generate
```
Commit the regenerated `crates/synapse-ffi/src/frb_generated.rs` and `app/lib/src/rust/`.
`frb_generated.rs` is excluded from rustfmt.

## Checks
- `cargo test -p synapse-ffi` (outline semantics against the real core).
- `cd app && dart analyze && flutter test` (editor key handling runs against an in-memory
  backend that reuses the Dart outline algorithms).
- `flutter analyze` crashes when the project path contains non-ASCII characters (its LSP
  channel miscounts bytes); `dart analyze` runs the same analysis.

## Editor
- Blocks render Markdown when idle and become raw-Markdown text fields when focused.
- Keys: Enter split (at start: new block above), Shift+Enter newline, Tab/Shift+Tab
  indent/outdent (Logseq's direct outdenting), Backspace at start merges into the previous
  block, Delete at end pulls the next one in, ↑/↓/←/→ cross block edges, Alt+Shift+↑/↓ move
  among siblings, Ctrl+↑/↓ collapse/expand, Ctrl+Enter cycles TODO/DONE, Esc leaves editing.
- `[[`, `((` auto-pair; `[[`, `#` and `((` open autocomplete (↑/↓, Enter/Tab, Esc).
- Drag a bullet onto a row: upper half = before, lower half = after, lower half to the right
  = first child.
- Edits apply locally at once and go to the core through one serial write queue (text saves
  are debounced 350 ms); after structural edits the outline is re-read and the core's
  structure wins if they ever disagree.
- App shortcuts: Ctrl+K quick switcher, Ctrl+Shift+J journals, Ctrl+Shift+T timeline,
  Ctrl+\ sidebar, Alt+←/→ back/forward.
