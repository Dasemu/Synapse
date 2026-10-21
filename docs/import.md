# Logseq import

Crate `synapse-import`: `import_logseq(src, &graph, &ImportOptions, progress) -> Report`.
CLI: `cargo run --release -p synapse-import --example import -- <logseq_dir> <graph_dir> [--skip-existing]`.

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
- `.org` files: one raw root block per top-level heading, no parsing; listed in `Report::org_files`.
- Timestamps: page `created_at` = file birth time (≤ mtime), `updated_at` = mtime; blocks inherit
  them. `origin_journal` = earliest journal referencing the page (aliases resolved), else the
  file's creation day. Journal pages keep core's midnight timestamps.

## Re-import
Idempotent. Blocks without `id::` get UUIDv5 ids from `(page title, ordinal)`, so a re-import
upserts the same rows; identical pages are counted in `Report::unchanged` and not touched. Pages
that changed are rewritten and stale blocks deleted (`ExistingPages::Update`, default);
`ExistingPages::Skip` leaves any page that already has blocks alone (keeps edits made in Synapse).

## Report / progress
`Progress { phase, done, total }` (phases: Scanning, Assets, Parsing, Pages, Blocks) and
`Report { pages, journals, blocks, assets, bytes, updated, unchanged, org_files, warnings }`.

## Known limits
- Not parsed: org-mode syntax, whiteboards, `draws/`, namespace parent stubs, Logseq queries.
- Sequential commits (core has no nestable transaction): ~7k blocks/s; 10k pages × 20 blocks ≈ 30 s,
  typical graphs a few seconds.
