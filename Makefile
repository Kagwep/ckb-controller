# Build / test for the controller session lock.
# Requires: rustup riscv target, clang 16+ (auto-detected, incl. Android NDK),
# and `make`. On a machine without `make`, use ./build.sh instead.

TOP := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
TARGET := riscv64imac-unknown-none-elf
MODE := release
CONTRACT := controller-session-lock
CONTRACTS := controller-session-lock controller-game-cell

# CKB-VM2 ISA + disable atomics (matches fiber-scripts).
FULL_RUSTFLAGS := -C target-feature=+zba,+zbb,+zbc,+zbs,-a -C debug-assertions
CLANG := $(shell $(TOP)scripts/find_clang)
AR := $(subst clang,llvm-ar,$(CLANG))

default: build test

build:
	mkdir -p build/$(MODE)
	for c in $(CONTRACTS); do \
		RUSTFLAGS="$(FULL_RUSTFLAGS)" TARGET_CC="$(CLANG)" TARGET_AR="$(AR)" \
			cargo build -p $$c --$(MODE) --target=$(TARGET); \
		cp target/$(TARGET)/$(MODE)/$$c build/$(MODE)/$$c; \
	done

test:
	cargo test -p tests -- --nocapture

# Host-only unit tests in the contract crate (merkle / params logic).
unit:
	cargo test -p $(CONTRACT)

prepare:
	rustup target add $(TARGET)

clean:
	cargo clean
	rm -rf build

.PHONY: default build test unit prepare clean
