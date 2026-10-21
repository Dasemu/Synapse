#!/usr/bin/env bash
# Stop hook: lint + tests must pass before the turn ends (exit 2 blocks).
cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" || exit 0
fail() { echo "$1 failed:" >&2; echo "$2" | tail -40 >&2; exit 2; }
if [ -f Cargo.toml ]; then
  out=$(cargo clippy --workspace --all-targets -q -- -D warnings 2>&1) || fail clippy "$out"
  out=$(cargo test --workspace -q 2>&1) || fail "cargo test" "$out"
fi
if [ -f app/pubspec.yaml ] && command -v flutter >/dev/null; then
  out=$(cd app && flutter analyze 2>&1) || fail "flutter analyze" "$out"
fi
exit 0
