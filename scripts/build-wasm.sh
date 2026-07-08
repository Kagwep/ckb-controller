#!/usr/bin/env bash
#
# Build the controller WASM client and generate the JS/TS package into wasm/pkg/.
#
# The crate compiles to wasm32 on the workspace's rustc 1.85.1, but the matching
# `wasm-bindgen-cli` (it must equal the `wasm-bindgen` lib version in
# wasm/Cargo.toml) needs a newer rustc to *build*. Install it once with a newer
# toolchain — the project itself stays on 1.85.1:
#
#   cargo +stable install wasm-bindgen-cli --version <wasm-bindgen version>
#
# Output: wasm/pkg/{controller.js, controller.d.ts, controller_bg.wasm}, an ES
# module a browser/TS app imports directly:
#
#   import init, { controller_address, ChannelSession } from "./pkg/controller.js";
#   await init();
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=wasm32-unknown-unknown
ARTIFACT="target/$TARGET/release/controller_wasm.wasm"

rustup target add "$TARGET" >/dev/null 2>&1 || true

cargo build -p controller-wasm --release --target "$TARGET"

wasm-bindgen --target web --out-dir wasm/pkg --out-name controller "$ARTIFACT"

# Optional: shrink with wasm-opt if available.
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz -o wasm/pkg/controller_bg.wasm wasm/pkg/controller_bg.wasm
  echo "wasm-opt: optimized controller_bg.wasm"
fi

echo "built wasm/pkg/ ->"
ls -la wasm/pkg/
