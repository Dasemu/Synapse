# CalDAV (`synapse-caldav`)

Two-way sync of CalDAV calendars (VEVENT) and task lists (VTODO) with a Synapse graph.

## Usage
```rust
let client = DavClient::new("https://cloud.example.com", Credentials { username, password })?;
let cfg = Config::new(chrono_tz::Europe::Madrid);   // viewer zone; optional default task list
let report = synapse_caldav::sync(&graph, &client, &cfg).await?;   // pull, then push
let events = events_for_day(&graph, date, cfg.tz)?;                // from the local cache
materialize_day(&graph, date, cfg.tz)?;                            // optional: events as journal blocks
```
Credentials (use an app password on Nextcloud) are passed by the caller and never stored.
The futures hold `&Graph` across awaits and are therefore not `Send`: run them with `block_on`
on a dedicated thread.

## Protocol
Discovery: `/.well-known/caldav` → `current-user-principal` → `calendar-home-set`. Changes come
from `sync-collection` (RFC 6578); servers without it (or rejecting the token) fall back to
ctag + etag listing. Changed objects are fetched with `calendar-multiget`; writes are `PUT`
with `If-Match` (or `If-None-Match: *` for creation) and `DELETE` with `If-Match`. TLS is
rustls with the bundled webpki roots (self-signed servers are not supported yet).

## Mapping
| Synapse block | VTODO |
|---|---|
| `TODO`/`LATER` | `STATUS:NEEDS-ACTION` |
| `DOING`/`NOW` | `IN-PROCESS` |
| `DONE` | `COMPLETED` + `PERCENT-COMPLETE:100` + `COMPLETED` |
| `CANCELED` | `CANCELLED` |
| `[#A]`/`[#B]`/`[#C]` | `PRIORITY` 1/5/9 (read back: 1–4 A, 5 B, 6–9 C) |
| `SCHEDULED: <2026-10-21 Wed [14:00]>` | `DTSTART` (date, or zoned time in `Config::tz`) |
| `DEADLINE: <…>` | `DUE` |
| other lines | `DESCRIPTION` |

Block properties: `caldav-uid::`, `caldav-calendar::` (list href). Unknown VTODO properties are
preserved on write-back (the last server copy is cached and edited in place). Remote tasks with
no block yet are created on a page named after the task list.

New tasks: a block in a **journal** that starts with a marker (not `DONE`/`CANCELED`), has no
`caldav-uid`, and was edited after the first sync is created in the default task list
(`Config::default_task_calendar`, else the first list supporting VTODO). Older journals are
never scanned, so imported history is not uploaded.

## Events
Events live only in the cache (`caldav_object.ics`). `events_for_day` / `events_between` expand
`RRULE`/`EXDATE`/`RECURRENCE-ID` overrides, convert zones (`TZID`, UTC, floating, all-day) to the
viewer zone and skip `CANCELLED`. `materialize_day` writes `10:00-10:30 [[Standup]]` blocks in
that day's journal (property `caldav-event`); recurring meetings link to a page named after
the summary. It is idempotent, refreshes unedited blocks and removes blocks of vanished events.
Event blocks are not pushed back.

## Conflicts
If a task changed on both sides since the last sync, the newer one wins (remote
`LAST-MODIFIED`/`DTSTAMP` vs block `updated_at`); the loser is kept as a child block
(`CalDAV conflict: … version (older, kept as note)`). A 412 on push triggers a re-fetch, the same
resolution and one retry. Delete vs edit: the edit wins (the block is detached and re-pushed).

## Tables (graph DB)
`caldav_meta`, `caldav_calendar` (ctag, sync token), `caldav_object` (href, uid, etag, last
server ICS, block id, last synced content), `caldav_event_block`.

## Not done yet
Pushing event edits, RRULE-aware completion of recurring tasks (the whole VTODO is completed),
custom CA certificates, multiple accounts, VALARM, attendees.
