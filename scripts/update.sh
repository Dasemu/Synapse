#!/usr/bin/env bash
# Idempotent update of a Synapse server install: pull, build, install the binary atomically.
# Run as the service user (by hand or via POST /admin/update). Exit codes:
#   0  updated; the caller (the server) exits so systemd restarts the new binary
#   10 already up to date, nothing to restart
#   other: failed; the running binary is left untouched
set -euo pipefail

REPO_DIR="${SYNAPSE_REPO_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
BIN_DIR="${SYNAPSE_BIN_DIR:-/opt/synapse/bin}"
BRANCH="${SYNAPSE_UPDATE_BRANCH:-main}"
export PATH="$HOME/.cargo/bin:$PATH"

log() { printf '%s %s\n' "$(date -u +%FT%TZ)" "$*"; }
trap 'log "update FAILED (line $LINENO)"' ERR

cd "$REPO_DIR"
log "fetching $BRANCH"
git fetch --quiet origin "$BRANCH"
local_rev=$(git rev-parse HEAD)
remote_rev=$(git rev-parse "origin/$BRANCH")
if [ "$local_rev" = "$remote_rev" ] && [ -x "$BIN_DIR/synapse-server" ]; then
  log "already up to date ($local_rev)"
  exit 10
fi

log "updating $local_rev -> $remote_rev"
git merge --ff-only "origin/$BRANCH"
log "building (release)"
cargo build --release --locked -p synapse-server

# Schema migrations run in synapse-core when each graph is opened; nothing to do here.
mkdir -p "$BIN_DIR"
install -m 0755 target/release/synapse-server "$BIN_DIR/synapse-server.new"
mv -f "$BIN_DIR/synapse-server.new" "$BIN_DIR/synapse-server"   # atomic replace
log "installed $remote_rev"
