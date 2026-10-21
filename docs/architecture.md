# Architecture

```
app/ (Flutter, Dart)  ──FFI──▶  synapse-ffi  ──▶  synapse-core  ◀── synapse-search
                                     │                 ▲   ▲        synapse-import
                                     │                 │   └─────── synapse-history
                                     ├──▶ synapse-ai ──┘ (Embedder) synapse-caldav
                                     └──▶ synapse-sync ──HTTP──▶ synapse-server (axum)
```

## Graph on disk
`<graph>/synapse.db` (SQLite, WAL) + `<graph>/blobs/` (content-addressed, blake3). Sibling
crates may add their own tables to the same database (prefix them: `search_*`, `history_*`,
`caldav_*`, `sync_*`) or their own files under `<graph>/` (e.g. `<graph>/index/` for tantivy).

## Data model (`synapse-core`)
- **Page**: title (unique, case-insensitive), optional `journal_date`, `origin_journal`
  (the day it was born), properties. Journal ids are UUIDv5 of the date, so the same day has
  the same id on every replica; other ids are UUIDv7.
- **Block**: outliner node (`parent_id`, fractional `position`), Markdown `content`,
  properties, collapsed flag.
- **Refs**: extracted on every write from `[[Page]]`, `#tag`, `#[[tag]]`, `key:: [[Page]]`,
  `((block))`, `{{embed ((block))}}`. Page refs are stored by normalised title, so they
  never dangle and survive sync/renames (rename rewrites the text).
- **Aliases**: alternative titles resolving to a page.
- **page_activity**: (page, day, edits) — links pages to the journals of days they were edited.
- **Synapses**: local weighted associations between pages (Hebbian, 30-day half-life).
- **Assets**: metadata rows; bytes in the blob store.

## Temporal model
- `journal_neighbors(date)` gives the previous/next existing journal (episodic chain).
- Pages created from a journal block are born there; other new pages are born in today's
  journal. `timeline(from, to)` returns, per day: journal, pages born, pages edited.

## Change feed and sync
Every mutation appends an `Op` to the `ops` table with a hybrid logical clock timestamp.
`ops_since(seq)` is the single feed for indexers, backups and sync. `apply_remote(hlc, op)`
merges another replica's ops with last-writer-wins per entity and tombstones for deletes;
applied ops are re-logged locally so they propagate further.

## Threading
`Graph` owns a `rusqlite::Connection` and is `Send` but not `Sync`. The FFI layer keeps it
behind a mutex (or one graph per worker thread opening the same directory; WAL allows it).

## Brain-like queries
`related(page)` runs spreading activation (3 hops, fan-out normalised) over refs in both
directions, synapses, the journal chain and origin/activity anchors.
