#!/usr/bin/env bash
# PostToolUse (Edit/Write): format the changed file. Never blocks.
f=$(jq -r '.tool_input.file_path // empty' 2>/dev/null)
[ -n "$f" ] && [ -f "$f" ] || exit 0
case "$f" in
  *.rs) command -v rustfmt >/dev/null && rustfmt --edition 2024 -q "$f" 2>/dev/null ;;
  *.dart) command -v dart >/dev/null && dart format -o write "$f" >/dev/null 2>&1 ;;
esac
exit 0
