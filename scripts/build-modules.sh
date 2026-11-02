#!/usr/bin/env bash
# Builds the bundled Synapse modules (modules/*) to WebAssembly and copies them to
# app/assets/modules/<module id>.wasm. Needs: rustup target add wasm32-unknown-unknown
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root/modules"
cargo build --release --target wasm32-unknown-unknown
out="$root/app/assets/modules"
mkdir -p "$out"
# crate name -> module id (the id in its manifest.json)
cp target/wasm32-unknown-unknown/release/synapse_secrets.wasm "$out/synapse.secrets.wasm"
ls -l "$out"
