#!/usr/bin/env bash
# Stop hook: lint + tests must pass before the turn ends (exit 2 blocks).
# Prefer the rustup toolchain over any distro-packaged cargo.
[ -d "$HOME/.cargo/bin" ] && PATH="$HOME/.cargo/bin:$PATH"
cd "$(git rev-parse --show-toplevel 2>/dev/null || pwd)" || exit 0
fail() { echo "$1 failed:" >&2; echo "$2" | tail -40 >&2; exit 2; }
if [ -f Cargo.toml ]; then
  out=$(cargo clippy --workspace --all-targets -q -- -D warnings 2>&1) || fail clippy "$out"
  out=$(cargo test --workspace -q 2>&1) || fail "cargo test" "$out"
fi
if [ -f app/pubspec.yaml ] && command -v dart >/dev/null; then
  # dart analyze: flutter analyze crashes on non-ASCII project paths.
  out=$(cd app && dart analyze 2>&1) || fail "dart analyze" "$out"
fi
exit 0
