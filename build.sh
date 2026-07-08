#!/usr/bin/env bash
#
# Build the controller session lock to the CKB riscv64 target, then stage the
# binary at build/release/. Works without `make` (only needs bash + rustup +
# a clang 16+, which find_clang locates — including an Android NDK clang).
set -euo pipefail
cd "$(dirname "$0")"

TARGET=riscv64imac-unknown-none-elf
CONTRACTS="controller-session-lock controller-game-cell"

CLANG="${CLANG:-$(bash scripts/find_clang)}"
AR="${AR:-$(printf '%s' "$CLANG" | sed 's/clang/llvm-ar/')}"
echo "Using CLANG=$CLANG"

rustup target add "$TARGET" >/dev/null 2>&1 || true

mkdir -p build/release
for CONTRACT in $CONTRACTS; do
  RUSTFLAGS="-C target-feature=+zba,+zbb,+zbc,+zbs,-a -C debug-assertions" \
  TARGET_CC="$CLANG" TARGET_AR="$AR" \
    cargo build -p "$CONTRACT" --release --target="$TARGET"
  cp "target/$TARGET/release/$CONTRACT" "build/release/$CONTRACT"
  echo "Built build/release/$CONTRACT"
done
