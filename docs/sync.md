# Sync protocol (v1)

Replication of a graph between devices through a self-hosted server. The unit of sync is the
**op log** of `synapse-core` (`Graph::ops_since` / `Graph::apply_remote`): each op carries a
hybrid logical clock (HLC) `{millis}-{counter}-{actor}`; replicas merge with last-writer-wins
per entity plus tombstones, so pushes and pulls are idempotent and order-insensitive.

Crates: `synapse-sync` (wire types in `protocol`, HTTP transport `Remote`, replicating
`SyncClient`) and `synapse-server` (axum). Clients send `X-Synapse-Protocol: 1`; `PUT
/v1/graphs/{id}` returns the server's `protocol` and the client refuses a mismatch.

## Auth
Every `/v1` request needs `Authorization: Bearer <token>`. `/healthz` is public. TLS is
terminated by a reverse proxy (see `docs/server.md`); the client uses rustls (no OpenSSL).

## Endpoints (`/v1/graphs/{id}`)
`{id}` is `[A-Za-z0-9_-]{1,64}` (a directory name on the server).

| Method | Path | Purpose |
|---|---|---|
| PUT | `/` | Create the graph if missing (idempotent). Returns `Info {protocol, server_id, last_seq}` |
| GET | `/` | Same, 404 if the graph does not exist |
| POST | `/ops` | Push `{ops:[{hlc, op}]}` (≤ 5000). Returns `{applied, last_seq}` |
| GET | `/ops?since=&limit=&exclude_actor=` | Pull ops with `seq > since` (limit ≤ 2000). Returns `{server_id, ops:[{seq,hlc,op}], next_cursor, last_seq, has_more}` |
| GET | `/events` | Server-Sent Events: `event: seq`, `data: {"last_seq":N}` on connect and after every change |
| HEAD | `/blobs/{hash}` | 200 if stored. Else 404 with `Upload-Offset` = bytes of a partial upload |
| GET | `/blobs/{hash}` | Streams the blob; supports `Range: bytes=N-[M]` (206) |
| PUT | `/blobs/{hash}` | Streamed upload, see below |

`{hash}` is the blake3 hex digest (64 lowercase hex chars) of the content.

### Read API (thin / online-mode clients, no replica needed)
`GET /pages?journals=&limit=&offset=`, `/pages/by-title?title=` (title or alias,
case-insensitive), `/pages/{uuid}` (page + aliases + block tree), `/pages/{uuid}/backlinks`,
`/journals/{YYYY-MM-DD}` (404 if the day has no journal; reads never create pages),
`/journals?from=&to=`. In Rust: `synapse_sync::Remote`.

## Push / pull and cursors
The client persists, in table `sync_state` of the local graph database: `pushed_seq` (highest
local seq already handled), `pull_cursor` (highest server seq already pulled), plus
`remote_key` (`url|graph_id`) and `server_id`. If either differs from the current remote, both
cursors reset to 0 (re-pushing is harmless: LWW ignores what the server already has).

A cycle (`SyncClient::sync_once`): 1) `PUT` graph; 2) push batches of local ops after
`pushed_seq`, uploading the blobs of `PutAsset` ops *first*; 3) pull batches after
`pull_cursor` and `apply_remote` each; 4) download blobs of assets that lack one (policy
`Eager`, or `OnDemand` via `ensure_blob`). Cursors are saved after each batch; a crash re-does
at most one batch.

### Avoiding echo
`apply_remote` re-logs applied ops locally (so they propagate to further replicas), which would
make them look like local edits. They are told apart by the HLC **actor**: local ops carry the
graph's own actor, applied ops keep the originating actor. The client pushes only ops whose
actor is its own, and the pull sends `exclude_actor=<own actor>` (and the client also skips them
when applying). The server's op log contains every actor's ops, so any other device receives
them through its pull.

## Blobs
Never fully in memory on either side. Upload is resumable:
1. `HEAD /blobs/{hash}` → 200 (done) or 404 + `Upload-Offset: N`.
2. `PUT` with `Upload-Length: <total>` (required) and `Upload-Offset: N` (omit/0 to start).
   The body (rest of the file) is appended to `<graph>/uploads/<hash>.part`.
   - offset ≠ stored length → `409` + the real `Upload-Offset`;
   - body ended before `Upload-Length` → `202` + `Upload-Offset` (partial kept, resume later);
   - complete → the content is hashed; match → moved into the blob store (`201`), mismatch →
     `422` and the partial file is dropped.
Download resumes from a partial `blobs/tmp/sync-<hash>.part` with `Range`, verifies the hash
and renames atomically. A `PutAsset` op is only pushed after its blob is on the server.

## Realtime
After each change the server publishes the new `last_seq`; connected clients (SSE) trigger a
pull immediately. The stream is only a hint: the client also polls local pending ops
(`poll_interval`, default 5 s) and does a full cycle every `full_sync_interval` (default 120 s).

## Client state and the background loop
`SyncClient::state()` / `subscribe()` give `SyncState {status: idle|syncing|offline|error,
message, pending_ops, last_sync_ms, last_pushed, last_pulled}`. `start()` spawns the loop:
sync on SSE hint, when ops are pending or when due; on failure exponential backoff (1 s → 5 min,
with jitter); "offline" (connect/timeout errors) never loses edits, they stay pending. Call
`trigger()` after local edits to sync without waiting for the poll.

### Graph handle (FFI integration)
`SyncClient::new(graph: Arc<Mutex<Graph>>, cfg)` — the expected handle type is
`Arc<Mutex<synapse_core::Graph>>` (std `Mutex`). `synapse-ffi` must hold its graph as that
same `Arc` (today it holds a bare `Mutex<Graph>`) and pass a clone to the client, so UI edits
and sync share one HLC clock and one connection. The client locks it only for short database
steps (never across network calls) and runs them on `spawn_blocking`; it needs a tokio
runtime (`start()` spawns onto the current one). After local edits call `trigger()`.

### Robustness
- **Invalid ops**: the server validates every pushed op (`protocol::validate_op`: well-formed
  HLC, `PutAsset.hash` = 64 lowercase hex) and rejects ops whose HLC is more than
  `SYNAPSE_MAX_CLOCK_SKEW_MINUTES` (default 10) ahead of the server clock. Rejected ops are
  not stored; they are listed in `PushResponse.rejected` (`{index, reason}`) and the rest of the
  batch is still applied. Ops that fail to apply with a non-transient error (`Invalid`, `Json`,
  `Conflict`, `NotFound`) are rejected the same way; storage errors fail the request (retry).
- **Quarantine (client)**: a pulled op that can never apply is logged, stored in table
  `sync_quarantine` (`server_seq, hlc, payload, reason`) and skipped, so the cursor advances.
  `SyncState.quarantined_ops` counts these plus ops the server rejected (per session).
- **Missing local blob**: the asset's `PutAsset` op is not pushed (warning,
  `SyncState.skipped_assets`) and later ops still flow.
- **Failed blob downloads** (e.g. 404): logged, counted in `SyncState.failed_blobs`, retried
  next cycle; other blobs and the op sync are unaffected. Connectivity/auth errors still abort
  the cycle.
- **Uploads** only complete when the server reports the blob stored (`HEAD` 200); `202`/`409`
  responses make the client resume until done.

## Limits / not in v1
- Per-user tokens and per-graph ACLs: `auth::Principal` already carries a graph allow-list;
  only the token source (env list) is implemented.
- No blob quota, no garbage collection of unreferenced blobs, no op-log compaction.
- A client with a wrong clock can win LWW; HLCs are not validated against the server time.
