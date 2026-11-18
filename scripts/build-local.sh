#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
# Native dependencies use the ohos-rs OXC and tsrs forks pinned in Cargo.lock.
cargo +1.100.0 build --locked --release -p arkts-lsp --target-dir target/tsrs
mkdir -p target/release
# Replace the destination inode instead of overwriting it: macOS caches the code
# signature of a running executable, and truncating that same inode makes the
# next launch fail with "Killed: 9" until Zed's server processes restart.
rm -f target/release/arkts-lsp
cp target/tsrs/release/arkts-lsp target/release/arkts-lsp
# Match Zed's default workspace build: only the root extension is a default member.
cargo +1.100.0 build --locked --release --target wasm32-wasip2
rm -f extension.wasm
cp target/wasm32-wasip2/release/oxk_zed.wasm extension.wasm
printf 'Native server: %s/target/release/arkts-lsp\nZed extension: %s/extension.wasm\n' "$root" "$root"
