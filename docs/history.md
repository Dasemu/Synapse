# History (`synapse-history`)

Automatic git-like backup of a graph. A bare git repository at `<graph>/history/` holds a
deterministic, Logseq-compatible Markdown export; every snapshot is a commit. Plain `git`
works on it (`git --git-dir=<graph>/history log`, `clone`, ...).

## Export format
- `pages/<title>.md`: title encoded Logseq-style (`/` → `___`, `% < > : " \ | ? *` and control
  chars percent-encoded). `journals/YYYY_MM_DD.md` for journals.
- File = page properties (`key:: value`, aliases as `alias:: a, b`), then one `- ` bullet per
  block, nested with tabs; extra content lines and block properties are indented under the
  bullet. Blocks targeted by `((uuid))` get an `id::` property. `collapsed:: true` is kept.
- Property values: raw text, or JSON when the value is not a plain string (so numbers/bools
  survive a restore). Keys are sorted, so output is byte-stable.
- Hubs: `synapse-kind:: hub` and `synapse-description::` page properties (hubs have no
  bullets). A page's tags (tag page titles) are listed in `synapse-tags:: ["a","b"]` and its types in `synapse-types:: [...]`; tag pages carry `synapse-kind:: tag` (all restored with the page). A page in hubs lists their titles in `synapse-hubs:: ["A","B"]`, so renaming a hub
  re-exports its members. Restore sets kinds first, then memberships (missing hubs or
  memberships that would now form a cycle are skipped).
- `assets.json`: manifest `hash → name/size/mime`, sorted. Asset bytes are **not** committed
  (the blob store is already content-addressed) unless `HistoryOptions::include_blobs`, which
  adds `blobs/<hash>`.

## Snapshots
`History::snapshot(&graph, now_ms)` is incremental: it reads `ops_since(last_seq)` plus a scan
of pages whose `updated_at` moved (block deletes/moves do not log their page), rewrites only
those files and removes files of deleted/renamed pages. State (`last_seq`, referenced block
ids) lives in `history/synapse-history.json`; if it is missing or ahead of the graph a full
export is done (unchanged files still hash to the same blobs, so the commit stays small).
Commit messages look like `3 pages edited, 1 created: A, B, C` and end with `Synapse-Seq: n`.

`maybe_snapshot(&graph, now_ms)` is for the app's periodic timer, driven by `SnapshotPolicy`:
`min_interval` (floor between snapshots), `max_interval` (ceiling when changes are pending),
`on_idle` (snapshot after the graph has been quiet that long).

## Queries and restore
`snapshots(limit)`, `page_history(title)` (snapshots that changed the page), `page_at`, `file`,
`diff_page(title, from, to)` (unified diff). Revisions are commit ids, abbreviations or `HEAD`.

`restore_page` / `restore_graph` re-import the Markdown through the `Graph` API and so append
new ops; git history is never rewritten. They take a safety snapshot first. Blocks keep their
ids (`id::`, or matched by identical content) so references survive; pages absent from the
snapshot are deleted (journals are emptied).

## Push
`History::push(url, branch, Some(&PushAuth{..}), force)` pushes to any `http(s)` Git remote
with a pure-Rust smart-HTTP implementation (gix cannot push yet). Credentials are used for the
request only and never stored. SSH remotes are not supported.

## Why gix
gix is pure Rust: no C toolchain or OpenSSL/libssh needed for Android and Windows builds.
Fallback if gix ever blocks us: libgit2 via `git2` with vendored libgit2.

## Known limits
- Block content lines that look like `key:: value` at the end of a block read back as block
  properties on restore.
- Alias changes trigger a full re-export (the previous owner of an alias is not logged).
