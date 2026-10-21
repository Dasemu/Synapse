# Synapse

## Overview
Native "second brain" notes app (Logseq-like outliner with linked pages and journals) for
Android, Linux and Windows. Rust core shared by a Flutter client (via flutter_rust_bridge)
and an optional self-hosted sync server. Local-only (standalone graph) or client-server
(online graph). No webviews.

## Commands
- Format: `cargo fmt --all` (Rust), `dart format app/` (Flutter)
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`, `cd app && flutter analyze`
- Test: `cargo test --workspace`, `cd app && flutter test`
- Run server: `cargo run -p synapse-server`
- Run app: `cd app && flutter run -d linux` (or `-d windows`, an Android device)
- Regenerate FFI bindings: `cd app && flutter_rust_bridge_codegen generate`

## Architecture
Cargo workspace under `crates/`, Flutter app under `app/`. Details in `docs/architecture.md`,
roadmap in `docs/plan.md`.
- `synapse-core`: domain model (blocks, pages, journals, typed links), SQLite store, blob store.
- `synapse-search`: full-text (tantivy) + vector index; auto-link suggestions.
- `synapse-import`: Logseq graph importer (pages, journals, assets).
- `synapse-ai`: provider-agnostic LLM/embeddings clients (user-supplied API keys).
- `synapse-history`: automatic git-like snapshots of the graph.
- `synapse-caldav`: CalDAV calendar + VTODO tasks.
- `synapse-sync` / `synapse-server`: client-server protocol and axum server.
- `synapse-ffi`: the only API surface exposed to Flutter.

## Conventions
- The Flutter app never touches storage directly; everything goes through `synapse-ffi`.
- Core crates are sync-free of UI concerns and must compile for Android (no desktop-only deps
  in `synapse-core`).
- IDs are UUIDv7 (time-ordered). Journals are keyed by local date `YYYY-MM-DD`.
- Heavy files go to the content-addressed blob store, never into SQLite rows.
- Version lives in the root `VERSION` file (bumped by CI only).

## Workflow quirks
- Hooks: `.claude/hooks/format.sh` formats after edits; `.claude/hooks/stop-check.sh` runs
  clippy + tests before a turn ends.

## Deployment
- Server: see `docs/server.md` (LXC/VPS + systemd, per the `web-deploy` skill).

## Gotchas
- Logseq graphs mix `.md` and `.org`; the importer only parses Markdown fully and keeps org
  files as raw blocks.
