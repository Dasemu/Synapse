#!/usr/bin/env bash
# PostToolUse hook: lint/format the file that was just edited or written.
# Reads the hook JSON payload from stdin and dispatches by extension.
# Best-effort: missing tooling is reported but never blocks the edit.
set -u

payload="$(cat)"
file_path="$(printf '%s' "$payload" | sed -n 's/.*"file_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"

[ -z "$file_path" ] && exit 0
[ -f "$file_path" ] || exit 0

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo_root" || exit 0

case "$file_path" in
  *.clj|*.cljs|*.cljc)
    if command -v clojure >/dev/null 2>&1; then
      clojure -M:clj-kondo --lint "$file_path" --cache false || true
    fi
    ;;
  libs/*.ts|libs/*.tsx|libs/*.js)
    if [ -f "libs/node_modules/.bin/prettier" ]; then
      (cd libs && node_modules/.bin/prettier --write "${file_path#libs/}") || true
    fi
    ;;
esac

exit 0
