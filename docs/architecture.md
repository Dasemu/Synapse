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

## Tags and types (`synapse-core::{tags, types}`)
- **Tags are plain names**, never pages. A page carries `Page.tags: Vec<String>` (`pages.tags`,
  synced with the page, LWW per page), stored sorted case-insensitively by `title_key`. The
  derived `page_tags(page_id, tag_key)` index covers the page's tags plus its type and is
  rewritten on every page write. Journals and templates cannot be tagged.
  API: `set_page_tags`, `add_page_tag`, `remove_page_tag`, `pages_with_tag`, `tag_catalog`,
  `rename_tag` (tags and types, merging duplicates) and `remove_tag` (explicit tags only).
- **A page has at most one type** (`Page.types`, 0 or 1 entries; `Page::page_type()`), a name
  saying what the page is. It implies a tag of the same name. API: `set_page_type(page,
  Option<&str>)`, `type_catalog` (types in use by count, then the seeded `SUGGESTED_TYPES`).
- Schema v6 turned former tag pages (kind `tag`) into plain notes and trimmed multiple types to
  the first; `PageKind` has no `Tag` any more and "tag" parses as a note.
- In the graph (`synapse-ffi`) every tag in use is a virtual node (`GraphNode.is_tag`, id
  `tag:<title_key>`, no page), with a `TAGGED` edge from each tagged page (type included).
- Known limitations: replicas still on the old schema may overwrite tags with stale values (LWW per page), and `rename_tag` is a bulk page edit, so concurrent edits on another replica can undo part of it.
- Exports use `synapse-tags::` and `synapse-type::` (see `history.md`).

## Hubs and namespaces (`synapse-core::structure`)
Two orthogonal ways to organise pages.
- **Namespace** (kinship by title, derived): `A/B` is a child of `A` (`namespace_parent`,
  `Graph::namespace_children`). Parents may not exist as pages (virtual levels).
- **Hub** (folder, explicit): a page with `kind = hub` (`PageKind`, schema v3 column
  `pages.kind`). A hub has a `description` and **no blocks** (`insert_block`/`put_block`/
  `move_block` into a hub fail with `Error::Invalid`). Created with `create_hub`, or by
  `convert_to_hub` (manual, from an existing note) and undone with `convert_to_note`.
  - Membership lives on the **member**: `Page.hubs: Vec<PageId>` (`pages.hubs`, JSON), synced
    with the page (`UpsertPage`, LWW per page), so no new op exists. A derived local index
    `page_hubs(page_id, hub_id)` is rewritten on every page write (local or remote) and rebuilt
    by the v2 → v3 migration.
  - **One hub per page**: the list holds at most one id (local writes with more are refused;
    `add_to_hub` moves the page). The wire format stays a list, and readers (index, `structure`)
    use only its first entry, so data from older replicas converges.
  - **Template**: every hub owns exactly one page with `kind = template` (member of its hub,
    possibly empty, an ordinary outline). `create_hub`/`convert_to_hub` create it,
    `hub_template` returns it (creating one for older hubs). Templates are hidden from
    `hub_members`, `list_pages` and `structure`, cannot be deleted alone and go with their hub
    (`convert_to_note` deletes it). `create_page_from_template(hub, title)` makes a note in the
    hub with a copy of the template's blocks.
  - API: `set_page_hubs`, `add_to_hub`, `remove_from_hub`, `hub_members` (sub-hubs first, then
    by title), `hubs_of`. Rules: the target must be a hub, no self-membership, no journals, no
    cycles (checked locally); duplicates dropped. Readers ignore ids that dangle or point at a
    page that is no longer a hub. Deleting a hub or converting it to a note removes its id from
    every member (one `UpsertPage` per member).
  - `hub_conversion_preview` suggests a title (last namespace segment), a description (first
    prose block, plain text, <= 280 chars) and members (outline refs, then property and tag
    refs). `convert_to_hub` applies a `HubConversion` in one transaction: rename (link
    rewrite), optionally move the outline to a new note (`keep_outline_as`), delete the
    remaining blocks, set kind/description and add the members.
  - `Graph::structure()` returns every hub with members and nesting depth, and each page's
    *home hub* (deepest containing hub; ties: fewer members, then title; a hub is its own
    home). Cycles created by concurrent edits on two replicas are broken deterministically
    (edges ordered by the member page's HLC). The graph view adds `CONTAINS` edges hub → member.
  - Sync limits: `apply_remote` does not enforce "no blocks" (rejecting would break
    convergence), so a block added on one replica while another converts the page can land in
    a hub; `convert_to_note` keeps such blocks. Clients older than schema v3 drop `kind`,
    `description` and `hubs` when they upsert a page, so every client must be on v3.
- Logseq built-ins (`contents`, `TODO`, `hls__*`, ...) are recognised by `is_legacy_page`; the
  app offers to purge them (`legacy_candidates` / `purge_pages` in `synapse-ffi`). The importer
  only skips a narrower set (see `import.md`).

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
