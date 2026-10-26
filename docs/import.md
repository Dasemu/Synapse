# Logseq import

Crate `synapse-import`: `import_logseq(src, &graph, &ImportOptions, progress) -> Report`.
CLI: `cargo run --release -p synapse-import --example import -- <logseq_dir> <graph_dir> [--skip-existing] [--keep-legacy]`.

## Pipeline
1. **Scan** `pages/`, `journals/` (`.md`, `.org`) and `assets/` (recursive). Settings are read from
   `logseq/config.edn`: `:journal/file-name-format`, `:journal/page-title-format`,
   `:file/name-format` (`:triple-lowbar` → `___` is the namespace separator, otherwise `.`).
2. **Assets**: every file in `assets/` is streamed into the blob store (`put_asset_file`, constant
   memory) keeping its original file name.
3. **Parse** (rayon): outline → block tree. Fractional positions via `position::between`.
4. **Pages**, then **aliases**, then **blocks**, written on one thread.

## Mapping
- File name → title: `a___b.md` → `a/b`, `%2F`/url-encoding decoded. A `title::` page property
  overrides it. Journals `2024_03_15.md` → journal page `2024-03-15`.
- `[[Mar 15th, 2024]]` (configured title format) is rewritten to `[[2024-03-15]]`.
- Page properties = lines before the first bullet, YAML front matter, or a first block that holds
  only properties. `alias::` also registers aliases. Block `key:: value` lines go to block
  properties (not the content); `id::` becomes the block id (so `((uuid))` refs keep working);
  `collapsed:: true` sets the flag.
- Assets: `../assets/x%20y.png` → `asset://<blake3>` in place (`![alt](asset://…)`; `?`/`#`
  suffixes kept). Missing files are left as is and reported.
- Logseq's organisational files `contents`, `card`, `favorites` and `hls__*` (PDF highlights)
  are skipped (`ImportOptions::skip_legacy_pages`, default on; `--keep-legacy` in the CLI); other
  built-in names (`TODO`, `NOW`, ...) import normally. Skipped paths go to
  `Report::skipped_legacy`, and empty stubs with those titles that refs created *in this import*
  are removed (pages that existed before are never touched). Purging other legacy pages is a
  manual action in the app (`legacy_candidates` / `purge_pages`).
- Hubs (Synapse-only pages without blocks, see `architecture.md`) are never written by a
  re-import: the file is skipped with a warning.
- `.org` files: one raw root block per top-level heading, no parsing; listed in `Report::org_files`.
- Timestamps: page `created_at` = file birth time (≤ mtime), `updated_at` = mtime; blocks inherit
  them. `origin_journal` = earliest journal referencing the page (aliases resolved), else the
  file's creation day. Journal pages keep core's midnight timestamps.

## Re-import and native content
The importer records what it wrote in two local tables (`import_pages`: source file → page id,
`import_blocks`: block id → state hash). Re-imports use them so Synapse content is never lost:
- Pages are found by source file first, so a page renamed in Synapse is updated, not duplicated.
- Block ids: Logseq `id::` when present, otherwise UUIDv5 of (source file, parent id, content,
  duplicate index). Inserting or moving a block elsewhere in a file does not change other ids.
- Only blocks the importer wrote are ever overwritten or deleted, and only while unedited in
  Synapse. Edited blocks keep the Synapse version (warning if the file changed too); a removed
  block that was edited or has native children is kept (warning). Native blocks are never touched.
- First import into an existing page (e.g. today's journal) appends the imported blocks after the
  native ones; page properties are merged without overriding existing keys.
- A block deleted in Synapse stays deleted unless the file changed it.
- Each document is written in its own savepoint; a failure rolls the document back and becomes a
  warning. The whole write phase is one transaction.
- `ExistingPages::Skip` leaves any page that already has blocks alone.

## Report / progress
`Progress { phase, done, total }` (phases: Scanning, Assets, Parsing, Pages, Blocks) and
`Report { pages, journals, blocks, assets, bytes, updated, unchanged, org_files, skipped_legacy, warnings }`.

## Known limits
- Not parsed: org-mode syntax, whiteboards, `draws/`, namespace parent stubs, Logseq queries.
- Speed is bound by core's per-block savepoint inside the import transaction: 208k blocks
  (10k pages) ≈ 60 s; re-importing an unchanged graph ≈ 1.3 s.
