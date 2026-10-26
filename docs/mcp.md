# MCP server

`synapse-mcp` lets AI assistants (Claude Code, Claude Desktop, any MCP client) read a graph
and curate its pages through high-level tools: no SQL, no file access. Two transports share
the same tools (`crates/synapse-mcp`, built on the official Rust SDK `rmcp`):

- **stdio**: the `synapse-mcp` binary on a local graph directory, next to the running app.
- **Streamable HTTP**: mounted in `synapse-server` at `/v1/graphs/{id}/mcp`.

## Intended workflow
The journal is the input; pages are the output. The assistant reads recent journals
(`get_journal`, `journal_links`, `timeline`), then creates pages for things mentioned there
(`create_page`, with `source_journal` to record the day they come from), types, tags and files
them in hubs reusing the existing vocabulary (`list_types`, `list_tags`, `hubs`), and fills
untyped/untagged pages (`list_pages` with `untyped` / `untagged`). **Journals are read-only
through MCP**: every write tool refuses journal pages and blocks.

## Setup
### stdio (local graph)
```bash
cargo build --release -p synapse-mcp   # target/release/synapse-mcp
synapse-mcp --graph ~/synapse          # read-only
synapse-mcp --graph ~/synapse --allow-write
```
Without `--graph` it uses `$SYNAPSE_GRAPH`, else the app's directory
(`~/.local/share/dev.dasemu.synapse/graph` on Linux, `%APPDATA%\dasemu\Synapse\graph` on
Windows). It refuses a directory without `synapse.db` (it never creates a graph).

Claude Code:
```bash
claude mcp add synapse -- /path/to/synapse-mcp --graph /path/to/graph
```
Claude Desktop (`claude_desktop_config.json`):
```json
{ "mcpServers": { "synapse": {
    "command": "/path/to/synapse-mcp", "args": ["--graph", "/path/to/graph"] } } }
```

### HTTP (sync server)
Set `SYNAPSE_MCP_TOKEN` (own tokens, comma separated, ≥ 16 chars, never the sync token) and,
behind a proxy, `SYNAPSE_MCP_ALLOWED_HOSTS=notes.example.com`. Add
`SYNAPSE_MCP_ALLOW_WRITE=1` for the write tools. See `docs/server.md`. The graph must already
exist on the server (created by a syncing client).

Claude Code:
```bash
claude mcp add --transport http synapse https://notes.example.com/v1/graphs/<graph-id>/mcp \
  --header "Authorization: Bearer $SYNAPSE_MCP_TOKEN"
```
Claude Desktop: use a remote connector with that URL and header, or bridge it with
`npx mcp-remote <url> --header "Authorization: Bearer <token>"`.

## Tools
Outputs are compact Markdown: `[[Page]]` titles, `((block-id))` after each block (its own id,
needed by the block tools), cursors (`next cursor: N`) for long lists. Pages are addressed by
title, alias, id or journal date (`YYYY-MM-DD`, `today`, `yesterday`). Errors come back as
tool errors with a hint (e.g. "did you mean [[Dune]]?").

| Tool | What |
|---|---|
| `search` | Titles/aliases + block text; all words must match, case/accent-insensitive, `"phrases"`. Filters: `scope`, `journals`, `tag`, `type`, `hub`, `page`, `from`/`to` |
| `get_page` | Header (type, tags, hub, aliases, properties, dates, born-in journal) + outline; `block` subtree, `max_depth`, `ids`, cursor. Hubs list members |
| `get_journal` | One day or a `from`/`to` range (≤ 31 days). Never creates a journal |
| `list_pages` | Filters `type`, `tag`, `hub`, `untyped`, `untagged`, `updated_since`, `created_since`, `journals`; sort `updated`/`created`/`title` |
| `backlinks` | Linked references grouped by page, parent block as context |
| `related` | Spreading activation (`Graph::related`) |
| `list_tags` / `list_types` | Vocabulary with counts (types: icons, unused defined/suggested) |
| `hubs` | Hub tree with note counts, or one hub's members |
| `timeline` | Per day (≤ 92 days): journal, pages born, pages edited |
| `journal_links` | Per journal day: linked pages with status (type, tags, untyped, untagged, empty) and existing pages mentioned without a link |

Write tools (only with `--allow-write` / `SYNAPSE_MCP_ALLOW_WRITE=1`; otherwise not listed):

| Tool | What |
|---|---|
| `create_page` | Title, `type`, `tags`, `hub`, Markdown outline `content`, `source_journal` (existing day; the page is "born" there) |
| `append_blocks` | Markdown outline at the end of a page or under a `parent` block |
| `update_block` / `delete_block` | Replace a block's text / delete it with its children |
| `set_page_type` | Set or clear (null) the single type |
| `set_page_tags` | `tags` replaces; `add` / `remove` edit |
| `set_page_hub` | Move into a hub, or out with null |

Markdown outlines use `- ` bullets with 2-space indents; `key:: value` lines are block
properties (leading ones are page properties); plain paragraphs become top-level blocks.
Resources: `synapse://page/{title}` and `synapse://journal/{date}` (outline without ids).

## Design decisions
- **SDK**: `rmcp` (official) covers both transports, tool schemas from the parameter structs
  (`schemars`) and protocol negotiation; hand-rolling JSON-RPC would duplicate all of that.
- **Concurrency (stdio)**: the graph is opened per call (`LocalHost`) next to the app (SQLite
  WAL). Reads run in a deferred transaction that is always rolled back (a consistent snapshot
  that can never write); writes run in `BEGIN IMMEDIATE`, held only for the call, retried a few
  times while SQLite is busy (plus the 5 s busy timeout). Opening per call also re-reads the
  op log's last clock, so this process never stamps ops behind the app's.
- **Server**: tools run on the shared graph handle (`GraphHandle::run`), so writes are normal
  ops: logged, pushed to SSE subscribers, pulled by every client. Stateless JSON responses (no
  MCP sessions to keep).
- **Search** is a scan of the core `pages`/`blocks`/`aliases` tables, not the tantivy index of
  `synapse-search`: the app does not maintain that index yet, and a second process owning its
  writer lock would block the app once it does. Fine for personal graphs (tens of thousands
  of blocks); switch to a read-only tantivy reader when the app indexes.
- **Writes** use only the `Graph` API (`create_page`, `insert_block`, `set_page_type`, ...).
  Side effects inherited from the core: a page born today, or edited today, ensures today's
  (empty) journal page exists, and `[[New Page]]` links create those pages, as in the app.
  The journal content itself is never modified. `rename_page` is not exposed: it rewrites the
  links in every referencing block, journals included.

## Security
- The assistant sees everything the tools return, including any secret written in plain text
  in a note. Only connect assistants you trust with the whole graph.
- Only core page/block data is exposed: no module data, no settings, no raw tables.
- HTTP: own tokens (hashed, constant-time compare), separate from the sync tokens; the graph
  must exist; `Host` allowlist against DNS rebinding; always behind TLS (see `server.md`).
- Read-only by default on both transports; write tools are not even listed without the flag.
