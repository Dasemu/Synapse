//! Everything under `api` is scanned by `flutter_rust_bridge_codegen` and exposed to Dart.
//!
//! - `graph`: the opaque [`graph::SynapseGraph`] handle and read-side queries.
//! - `outline`: block editing (insert, update, move, indent/outdent, merge, delete).
//! - `search`: title/block lookup (stopgap until `synapse-search` is wired).
//! - `graph_view`: the whole page-level graph for the graph views (cached by op seq).
//! - `structure`: hubs (folder-like pages), namespaces and the legacy page purge.
//! - `tags`: free-form tags on pages (names, not pages) and their vocabulary.
//! - `layout`: Barnes-Hut force-directed layout, stepped from Dart off the UI thread.
//!
//! Extension points for Phase 2 (one module each, methods on `SynapseGraph` or their own
//! opaque handles): `ai` (synapse-ai), `history` (synapse-history), `caldav`
//! (synapse-caldav) and `sync` (synapse-sync client). Add `pub mod <name>;` here, implement it
//! on top of the locked `Graph`, and regenerate the bindings.

pub mod dto;
pub mod graph;
pub mod graph_view;
pub mod layout;
pub mod outline;
pub mod search;
pub mod structure;
pub mod tags;
