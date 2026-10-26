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
The graph lives in the platform app-support dir (`~/.local/share/dev.dasemu.synapse/graph`
on Linux). Override it with `SYNAPSE_GRAPH=/path/to/graph`.

## Icons
The logo is `logo.png` (repo root). App icons are cut from its emblem: Android
`android/app/src/main/res/mipmap-*/ic_launcher.png`, Windows `windows/runner/resources/app_icon.ico`,
Linux `linux/runner/resources/app_icon.png` (installed to the bundle's `data/` and set as the
window icon in `my_application.cc`).

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
  indent/outdent (direct outdenting), Backspace at start merges into the previous
  block, Delete at end pulls the next one in, ↑/↓/←/→ cross block edges, Alt+Shift+↑/↓ move
  among siblings, Ctrl+↑/↓ collapse/expand, Ctrl+Enter cycles TODO/DONE, Esc leaves editing.
- `[[`, `((` auto-pair; `[[`, `#` and `((` open autocomplete (↑/↓, Enter/Tab, Esc).
- `/` at the start of a word opens the command menu: `/link` (page link, then the page picker),
  `/ref`, `/url`, `/image`, `/bold`, `/italic`, `/strike`, `/highlight`, `/code`, `/codeblock`,
  `/h1`–`/h3`, `/todo`, `/quote`, `/divider`, `/today`, `/tag` (`SlashCommand` in
  `editor/completion.dart`).
- Rich text is Markdown: `**bold**`, `*italic*`, `~~strike~~`, `==highlight==`, `` `code` ``,
  `[label](url)`, `![img](url)`. A backslash escapes punctuation (`\*` shows a literal `*`,
  `\[[` is not a link); the backslash is hidden when the block is not being edited.
- Ctrl+B / I / E / L and Ctrl+Shift+S / H wrap the selection (or the caret) in bold, italic,
  code, page link, strike and highlight; pressing again unwraps.
- Drag a bullet onto a row: upper half = before, lower half = after, lower half to the right
  = first child.
- Edits apply locally at once and go to the core through one serial write queue (text saves
  are debounced 350 ms); after structural edits the outline is re-read and the core's
  structure wins if they ever disagree.
- App shortcuts (defaults, all rebindable in Settings): Ctrl+K quick switcher, Ctrl+Shift+J
  journals, Ctrl+Shift+G graph, Ctrl+Shift+T timeline, Ctrl+\ sidebar, Ctrl+Shift+\ graph
  sidebar, Ctrl+, settings, Alt+←/→ back/forward.

## Settings, themes and key bindings
- Sidebar "Settings" (Ctrl+,): theme cards and the shortcut list. Click a shortcut, press the new
  combination (Esc cancels); a combo already in use is refused and names its owner; each row
  and "Reset all" restore defaults.
- Themes (`appThemes` in `theme/theme.dart`): System (Paper light/dark by OS), Paper, Paper dark,
  Sweet (KDE-style indigo and magenta), Sweet light, Nord. A theme is a `SynapseColors` palette.
- Preferences live in `app-settings.json` inside the graph directory (`FileSettings`);
  `Keymap` (`state/keymap.dart`) stores only the overrides, encoded as `ctrl+shift+<keyId>`.

## Types and tags
- **Type** (what a note is): plain names, no page (persona, lugar, empresa, cliente, herramienta,
  concepto, idea, ...). "Type" under the header opens a picker with the types in use (with count)
  and seeded suggestions, or type a new name. Stored in `Page.types`.
- **Tag** (a topic): a real page of kind `tag`. "Tag" opens a picker over the existing tag pages
  or creates one by typing a new name (a note used as a tag becomes a tag page; hubs, journals
  and templates cannot be tags). Tag chips open the tag page, which lists its tagged notes.
  An inline `#tema` or `[[tema]]` in the text also links to the page. Renaming or deleting a tag
  page updates its members. Tag pages cannot be tagged themselves.
- In the graph a tagged note points at its tag pages with a separate edge kind (TAGGED), apart
  from page links and inline references.

## Hubs, namespaces and cleanup
- **Hub** (folder icon): a page kind, not a note: no outline, a title, a short description
  and members (notes and sub-hubs). Membership is stored on the member (its parent hubs), so
  a page belongs to at most one hub (adding it to another hub moves it); cycles are rejected. **Namespace** (tree icon): `A/B/C`
  titles; `A/B` is the parent of `A/B/C` even when it does not exist yet.
- Hubs never appear in the sidebar's "RECENT" list (they are folders, not pages).
- Sidebar "HUBS" (always shown, "+" creates a hub): tree of root hubs, collapsed by default;
  unfolding a hub lists only its sub-hubs (notes live on the hub screen), above "RECENT";
  reloads after edits settle.
- Hub screen ends with the local graph panel (same as the page sidebar on notes).
- Every hub has a template (a hidden note, possibly empty): "Template" on the hub screen opens
  it for editing, and notes created from "Add" start as a copy of it.
- Hub screen: editable title and description (saved on blur), members listed sub-hubs first
  then by title, "Add" (pick a page, or create a note / sub-hub), × on hover or long-press
  to remove, `⋯` ▸ "Convert to note" (description and members become blocks and links).
- Notes: namespace breadcrumb, "In: ..." chips with a × to leave a hub, and a `⋯` menu
  with "Convert to hub…" (dialog: shortened title, description, member checklist, optional
  note that keeps the original outline; title clashes are shown inline) and "Add to hub…".
- All pages ▸ "Clean up": dialog listing legacy Logseq pages (`contents`, `hls__...`, pages
  with `type:: system`, ...) with checkboxes; "Delete (n)" removes only the checked ones
  and their blocks.

## Page sidebar and All pages
- The sidebar icon next to a note's `⋯` menu toggles a right sidebar (collapsed by default,
  state in `AppState.pageSidebar`) with the local graph (neighbourhood at depth 1–3) and the
  related pages; on narrow windows they appear below the content.
- All pages lists notes only (hubs are folders, not pages) and can be sorted by edited,
  created or title, ascending or descending.

## Graph view
One view (sidebar "Graph", Ctrl+Shift+G) with a mode selector:
- **Network**: force-directed graph of pages (journals shown by default). Edges: links, tags,
  property refs, block refs/embeds (aggregated per page) and dashed mentions (unlinked
  references: a page title appearing as whole words in another page's text). Node size by
  degree + backlinks, colour by cluster, primary tag or home hub. Hubs (a page kind: folders of
  notes) are drawn larger with a ring in every mode including Synapses (one batched draw per
  size), connected to their members by thin solid "contains" edges (CONTAINS, structural, on by
  default), both marked in the legend. Filters: title, tag, created date
  range, min links, orphans, journals, mentions. Drag nodes, hover highlights neighbours,
  wheel/pinch zoom, tap opens the page.
- **Synapses**: `related()` spreading activation from a focus page; brightness/size =
  activation, edge thickness = Hebbian synapse weight, a pulse travels outward by hops.
- **Clusters**: soft hulls grouped by "Hubs" (home hub, default when the graph has hubs; hull
  labelled with the hub title once it is wide enough on screen; layout-only springs pull
  members to their hub), by "Tags" (each page goes to its most widely used tag page, ties
  alphabetical; hull labelled with the tag name; springs pull pages to the best connected page
  of the tag; journals and untagged pages stay out) or by "Communities" (Louvain: groups found
  automatically from link density, no names). Colours follow group size.
- **Tree** (no "Hubs" node: root hubs and the notes that sit in no hub, orphans included,
  hang straight off the centre): the centre is labelled with the store name (graph directory name); every arm and
  sub-branch has its own colour (arms evenly spread over the hue circle, children split their
  parent's sector evenly); drag a node to move it with its branch, drag empty space to pan.
  Radial tree: hubs (root hubs, then members recursively, cycle-safe,
  each hub expanded once, folder glyph; membership = CONTAINS edges from the graph data),
  namespaces (`a/b/c` titles, attached straight to the centre, no "Namespaces" node)
  and Tags (with their pages).
- **Timeline**: the journal timeline.

Data and layout live in Rust: `SynapseGraph::graph_data()` returns the whole page-level
graph as parallel arrays (cached until the op seq or the synapse table changes);
`ForceLayout` (Barnes-Hut, O(n log n)) is stepped from Dart in small batches on a Rust
worker thread. The painter batches draws (one call per edge kind and node colour/size),
culls to the viewport and limits labels with an occupancy grid.

```bash
# Stats and timings for a graph (release build)
cargo run --release -p synapse-ffi --example graph_stats -- /path/to/graph
# Real-app smoke test of every graph mode with frame timings (opens a window; use a copy
# of the graph, opening it may write today's journal). Profile mode for real numbers:
cd app && flutter drive --profile -d linux --driver=test_driver/integration_test.dart \
  --target=integration_test/graph_smoke_test.dart \
  --dart-define=GRAPH=/path/to/copy [--dart-define=SHOTS=/dir/for/pngs]
```
Measured (profile, Linux, Impeller GL): 5 000 pages / 19 000 links settle in ~7 s and pan at
build p50 6 ms / raster p50 4 ms per frame; a 360-page graph settles in ~1 s at ~2 ms/frame.
Edges are drawn as one triangle mesh (`drawVertices`): stroking 10k+ lines made Impeller
tessellate every frame (raster ~40 ms).
