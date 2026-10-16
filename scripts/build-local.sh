#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# Native dependencies use the local OXC and tsrs checkouts declared in Cargo.toml.
cargo +1.99.0 build --locked --release -p arkts-lsp --target-dir target/tsrs
mkdir -p target/release
cp target/tsrs/release/arkts-lsp target/release/arkts-lsp
# Match Zed's default workspace build: only the root extension is a default member.
cargo +1.99.0 build --locked --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/oxk_zed.wasm extension.wasm
printf 'Native server: %s/target/release/arkts-lsp\nZed extension: %s/extension.wasm\n' "$root" "$root"
