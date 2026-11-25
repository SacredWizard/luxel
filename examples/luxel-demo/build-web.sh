#!/usr/bin/env bash
# Build the browser demo into examples/luxel-demo/web/ (needs wasm-bindgen-cli
# matching the wasm-bindgen version in Cargo.lock), then serve that folder:
#   python3 -m http.server -d examples/luxel-demo/web 8080
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -p luxel-demo --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir examples/luxel-demo/web \
  target/wasm32-unknown-unknown/release/luxel-demo.wasm
echo "built: examples/luxel-demo/web"
