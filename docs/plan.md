# Synapse plan

## Goals
An outliner for linked notes and journals that fixes what Logseq/Obsidian lack: native performance on Android,
Linux and Windows, a real database instead of re-parsing Markdown on every start, heavy assets
handled properly, built-in sync (self-hosted) and backup, semantic search and AI, and a
"brain-like" model where connections have strength, memory is episodic (journals) and
knowledge resurfaces on its own.

## Gaps we fix (vs Logseq / Obsidian / Notion)
| Pain | Synapse answer |
|---|---|
| Electron apps: slow start, RAM hungry, poor mobile | Rust core + Flutter AOT native UI |
| Logseq re-indexes huge graphs, chokes on big assets | SQLite store + content-addressed blob store, incremental index |
| Sync is paid or fragile (iCloud/Syncthing conflicts) | Own op-log sync with HLC clocks, self-hosted server |
| Only keyword search | Full-text (tantivy) + vector search, hybrid ranking |
| Links are manual | Auto-link suggestions (title/alias matching + embeddings) |
| Journals are isolated files | Journals chained in time; pages anchored to their journals |
| No history beyond file history | Automatic git-like snapshots, restore any version |
| Calendar/tasks live elsewhere | CalDAV events + VTODO tasks inside journals |

## Brain-like model
- **Episodic memory**: every journal links to the previous and next journal that exists.
- **Origin**: a page created inside a journal block remembers that journal (`origin_journal`).
- **Activity trail**: each edit of a page records the day; the page links to those journals.
- **Synapses**: every link is an edge with a *weight*. Co-visiting or co-editing two notes
  strengthens their edge (Hebbian); weights decay over time unless reinforced.
- **Spreading activation**: "related" panel = activate the current note, propagate through
  weighted edges with decay, rank by activation (plus vector similarity).
- **Resurfacing**: spaced-repetition-style review of notes/blocks flagged as cards, and
  "on this day" + "forgotten but strongly connected" suggestions in the daily journal.
- **Consolidation** (AI): nightly/weekly summaries of journals into evergreen notes, proposed
  links and tags; the user accepts or rejects (rejections lower the edge weight).

## Features (scope)
1. Logseq importer: pages, journals, properties, block refs, embeds, tags, aliases, assets
   (large files streamed into the blob store), `logseq/config.edn` basics.
2. Efficient storage: SQLite (WAL) + blob store, op log for sync and undo.
3. Native GUI with rich text (block outliner, inline formatting, images, code, tables).
4. Auto-link suggestions.
5. Vector search (embeddings via AI provider or local model), hybrid with full-text.
6. Standalone (local graph) and client-server (online graph) modes, same API.
7. Semi-integrated AI: Anthropic, OpenAI, Gemini, Ollama/OpenAI-compatible; user API keys
   stored in the OS keystore. Ask-your-graph (RAG), summarize, rewrite, extract tasks.
8. Modules: WASM plugins (sandboxed, capability-based API) for commands, block renderers,
   importers. Phase 3.
9. Performance: lazy loading, virtualized lists, incremental indexing, background workers.
10. Temporal journals and page anchoring (see brain-like model).
11. Automatic backup: periodic git snapshots of a Markdown export of the graph.
12. Rich text.
13. CalDAV calendars and task lists (VTODO), shown in journals, two-way sync.
14. Graph views: force graph, **timeline** (journals on a line, pages hanging from their
    journals), local neighborhood, heat map of activity.
15. Extras: queries (Datalog-lite filters), templates, flashcards (SRS), quick capture
    (Android share target, global hotkey on desktop), web clipper via share, encryption at
    rest for the server, PDF/image annotation as blocks (later).

## Phases
- **Phase 0 – Foundation** (this branch): workspace, `synapse-core` (model, SQLite store,
  link parser, temporal links, synapses, blob store, op log), docs.
- **Phase 1 – Parallel build** (one session per piece, each in its own worktree/branch):
  1. `synapse-import` – Logseq importer.
  2. `synapse-search` – tantivy full-text, vector index, hybrid search, auto-link suggestions.
  3. `synapse-ai` – providers, chat + embeddings, `Embedder` impls, RAG helpers.
  4. `synapse-history` – automatic git snapshots of a Markdown export, restore.
  5. `synapse-caldav` – CalDAV events and VTODO tasks client + mapping to blocks.
  6. `synapse-sync` + `synapse-server` – op-log sync protocol, axum server, auth.
  7. `synapse-ffi` + `app/` – Flutter app: outliner editor, journals, search, timeline view.
- **Phase 2 – Integration**: wire search/AI/history/caldav/sync into FFI and the app;
  spreading-activation "related" panel; graph and timeline views polished; Android build.
- **Phase 3 – Modules & extras**: WASM plugin host, flashcards, queries, templates,
  consolidation, encryption.

## Dependencies (core)
- `rusqlite` (bundled SQLite: portable to Android/Windows, no system lib).
- `uuid` v7 (time-ordered ids, good index locality), `chrono` (dates/journals).
- `serde`/`serde_json` (properties, op payloads), `thiserror` (errors),
  `blake3` (fast content hashing for the blob store).
