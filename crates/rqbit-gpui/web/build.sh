#!/usr/bin/env bash
# Builds the browser version of the rqbit GPUI client into ./dist.
# Requires: rustup (toolchain from rust-toolchain.toml, auto-installed) and
# wasm-bindgen-cli matching the wasm-bindgen crate version:
#   cargo install wasm-bindgen-cli --version 0.2.120
# rqbit serves these under /gpui/ from $RQBIT_GPUI_WEB_DIR, default
# $XDG_DATA_HOME/rqbit/gpui-web (~/.local/share/rqbit/gpui-web): copy dist/* there.
set -euo pipefail
cd "$(dirname "$0")"
cargo build --release --target wasm32-unknown-unknown
rm -rf dist
wasm-bindgen --target web --no-typescript --out-dir dist \
  target/wasm32-unknown-unknown/release/rqbit_gpui_web.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -Os --enable-bulk-memory --enable-nontrapping-float-to-int \
    dist/rqbit_gpui_web_bg.wasm -o dist/rqbit_gpui_web_bg.wasm || true
fi
cp index.html dist/
# Precompressed copies; rqbit serves name.gz when the browser accepts gzip.
gzip -9 -k -f dist/*.wasm dist/*.js
ls -la dist
