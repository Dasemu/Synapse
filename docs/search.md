# Search (`synapse-search`)

Search and auto-link suggestions over a `synapse_core::Graph`. All entry points are on
`Search` (one per open graph, not `Sync`), taking `&Graph` per call.

## Full-text
- tantivy index in `<graph_dir>/index/`: one doc per block and one per page (title + aliases).
  Page titles shown in results are resolved from SQLite, so renames need no block reindex.
- Analyzer: simple tokenizer → lowercase → ASCII folding (`canción`≈`cancion`, `ñ`≈`n`) →
  optional stemmer (`Config::stemming`: none/English/Spanish; changing it rebuilds the index).
- Query: every term must match (relaxed to any term if nothing matches); the last term is a
  prefix; terms ≥ 5 chars tolerate one typo. `quick_switch` searches page titles/aliases only.
- Incremental: `Search::sync(&graph)` replays `Graph::ops_since(last_seq)`; the last indexed
  seq lives in table `search_state` of the graph DB. Documents are re-read from the DB, so
  replaying is idempotent. `rebuild` re-indexes from scratch; a missing index dir recovers
  automatically on open.

## Vectors
- Table `search_vectors(id, model_id, kind, page_id, stamp, dims, scale, vec)`: L2-normalised
  vectors quantised to int8 with a per-vector scale (4× smaller than f32, cosine error ≈ 1e-2).
- Search is an exact scan over the in-memory int8 matrix (loaded lazily per model), not HNSW:
  pure Rust, no C++, O(1) updates/deletes, ~6 ms at 50k and ~12 ms at 100k vectors (384 dims).
  Revisit with an ANN structure beyond ~500k vectors.
- The caller controls batching and network: `pending_embeddings(graph, model, limit)` returns
  chunks (`page title\nblock text`; page vectors embed `title; aliases`), `store_embeddings`
  saves `(pending, vector)` pairs, `embed_pending` is the loop for a given `&dyn Embedder`.
  Blocks are stale when `updated_at` changes; deleted blocks/pages lose vectors on `sync`.

## Hybrid
`hybrid_search` / `hybrid_with_vector`: reciprocal rank fusion (k = 60) of full-text and vector
rankings. `Filter`: scope (all/pages/blocks), journals (any/only/exclude), date range (journal
date, else last-edit day), single page. Journal/date filters on vectors are applied after the
scan (over-fetched ×5).

## Auto-link suggestions
- `LinkMatcher` (Aho-Corasick over `all_titles()`): word-bounded, longest match, case-insensitive,
  skips code spans/fences, `[[links]]`, `((refs))`, `#tags`, `[md](links)`, URLs, `{{macros}}` and
  property lines; titles under 3 chars are ignored. `Search::unlinked_references(page)` finds
  candidate blocks through a tantivy phrase query, then verifies with the matcher.
- `Search::semantic_suggestions(block)`: nearest page-title vectors not already linked.
- `Suggestion { block, span, matched, target_page, score, reason }`; `apply_suggestion` rewrites
  the span to `[[Title]]` via `Graph::update_block` (semantic ones append ` [[Title]]`) and
  returns `Error::Stale` if the block changed.

## Benchmark
`cargo run --release -p synapse-search --example bench -- 100000`
