# Synapse server

Self-hosted sync server (`synapse-server`, axum). Protocol: `docs/sync.md`. Target: an LXC
container or VPS running Debian/Ubuntu (one of the last 3 releases), managed by systemd. No
Docker for deployment.

## Configuration (environment)
See `.env.example`.

| Variable | Required | Meaning |
|---|---|---|
| `SYNAPSE_DATA_DIR` | yes | Server graphs live in `<dir>/<graph_id>/` (`synapse.db` + `blobs/`) |
| `SYNAPSE_AUTH_TOKEN` | yes | Bearer token(s), comma separated, ≥ 16 chars. Compared in constant time |
| `SYNAPSE_BIND` | no | Listen address, default `127.0.0.1:7878` |
| `SYNAPSE_MAX_CLOCK_SKEW_MINUTES` | no | Reject pushed ops whose HLC is further ahead of the server clock (default 10) |
| `SYNAPSE_UPDATE_TOKEN` | no | Enables `/admin/update`; separate from the sync tokens |
| `SYNAPSE_UPDATE_SCRIPT` | no | Default `/opt/synapse/scripts/update.sh` |
| `SYNAPSE_LOG_FORMAT` | no | `text` (default) or `json` |
| `RUST_LOG` | no | Log filter, default `info` |

Graphs are created on first use by an authenticated client (`PUT /v1/graphs/{id}`). Each is a
normal `synapse_core::Graph`, so a backup is a copy of its directory (use `sqlite3 .backup`
or stop the service for a consistent copy of `synapse.db`).

## Install (Debian/Ubuntu, as root)
```bash
apt-get update && apt-get install -y git build-essential pkg-config curl caddy
useradd --system --create-home --home-dir /opt/synapse --shell /usr/sbin/nologin synapse
install -d -o synapse -g synapse /var/lib/synapse /etc/synapse
# Rust toolchain for the service user (the update script builds on the host)
su -s /bin/bash synapse -c 'curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal'
su -s /bin/bash synapse -c 'git clone <repo-url> /opt/synapse/src && /opt/synapse/src/scripts/update.sh' || true
```
`update.sh` installs the binary in `/opt/synapse/bin` (override with `SYNAPSE_BIN_DIR`); the
first run exits 0 once built. Point the unit's script path at the checkout:
```bash
install -d -o synapse -g synapse /opt/synapse/scripts
ln -sf /opt/synapse/src/scripts/update.sh /opt/synapse/scripts/update.sh
cp /opt/synapse/src/crates/synapse-server/deploy/synapse-server.service /etc/systemd/system/
```
Create `/etc/synapse/server.env` (mode `0600`, owner `synapse`) with the variables above, e.g.
`SYNAPSE_DATA_DIR=/var/lib/synapse`, `SYNAPSE_BIND=127.0.0.1:7878`, and tokens generated with
`openssl rand -hex 32`. Never commit this file. Then:
```bash
systemctl daemon-reload && systemctl enable --now synapse-server
curl -s http://127.0.0.1:7878/healthz
```

### TLS
The server speaks plain HTTP; put a reverse proxy in front for HTTPS. With Caddy
(`/etc/caddy/Caddyfile`):
```
notes.example.com {
    reverse_proxy 127.0.0.1:7878 {
        flush_interval -1   # keep the SSE stream (/events) unbuffered
    }
}
```
Large blobs are streamed; do not set request-body limits or buffering in the proxy.

## Updating
`scripts/update.sh` is idempotent: `git fetch` + fast-forward, `cargo build --release --locked`,
atomic install of the binary; output goes to stdout (the endpoint logs it to
`$SYNAPSE_DATA_DIR/update.log`). It fails safely: on any error the running binary is untouched.
Exit codes: `0` updated, `10` already up to date, other = failed. SQLite schema migrations run
in `synapse-core` when a graph is opened, so a restart applies them.

By hand: `sudo -u synapse /opt/synapse/scripts/update.sh && systemctl restart synapse-server`.

### Authenticated update endpoint
Enabled only when `SYNAPSE_UPDATE_TOKEN` is set (otherwise 404).
```bash
curl -X POST -H "Authorization: Bearer $SYNAPSE_UPDATE_TOKEN" https://notes.example.com/admin/update   # 202
curl          -H "Authorization: Bearer $SYNAPSE_UPDATE_TOKEN" https://notes.example.com/admin/update   # status
```
- Asynchronous, one update at a time (`409` while running), at least 60 s between runs (`429`),
  and 20 admin requests/minute overall (`429`).
- Status: `{state: idle|running|up_to_date|succeeded|failed, started_at_ms, finished_at_ms,
  exit_code, log_tail}`.
- After a successful update (script exit 0) the server exits about 2 s later and systemd
  (`Restart=always`) starts the new binary, so no root privileges are needed.

## Operations
- Logs: `journalctl -u synapse-server -f`; set `SYNAPSE_LOG_FORMAT=json` for structured logs.
- Health: `GET /healthz` (public) → `{"status":"ok","version":...,"protocol":1}`.
