#!/usr/bin/env bash
# Build the browser demo into examples/photic-demo/web/ (needs wasm-bindgen-cli
# matching the wasm-bindgen version in Cargo.lock), then serve that folder:
#   python3 -m http.server -d examples/photic-demo/web 8080
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo build -p photic-demo --release --target wasm32-unknown-unknown
wasm-bindgen --target web --no-typescript --out-dir examples/photic-demo/web \
  target/wasm32-unknown-unknown/release/photic-demo.wasm
echo "built: examples/photic-demo/web"
