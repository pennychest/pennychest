#!/bin/sh
# Rebuilds crates/core/tests/fixtures/plugins/qif/qif.wasm, the QIF example plugin built for
# WASI, which the plugin host's tests run. Needs the wasm32-wasip1 target:
#   rustup target add wasm32-wasip1
set -eu
cd "$(dirname "$0")/../.."
CARGO_PROFILE_RELEASE_OPT_LEVEL=z \
CARGO_PROFILE_RELEASE_STRIP=true \
CARGO_PROFILE_RELEASE_LTO=true \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
CARGO_PROFILE_RELEASE_PANIC=abort \
  cargo build --release --example qif -p pennychest-plugin --target wasm32-wasip1
cp target/wasm32-wasip1/release/examples/qif.wasm crates/core/tests/fixtures/plugins/qif/
