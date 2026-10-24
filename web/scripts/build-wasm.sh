#!/bin/sh
# Build crates/xeibe-wasm into web/src/wasm (generated, not in git): the
# module, wasm-bindgen's JavaScript glue, then wasm-opt.
#
# Needs the wasm32-unknown-unknown target (rust-toolchain.toml has rustup add
# it), clang (zstd's C code) and wasm-bindgen-cli at the version Cargo.toml
# pins wasm-bindgen to.
set -eu
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

pinned=$(sed -n 's/^wasm-bindgen = "=\(.*\)"/\1/p' Cargo.toml)
installed=$(wasm-bindgen --version | cut -d' ' -f2)
if [ "$pinned" != "$installed" ]; then
    echo "wasm-bindgen-cli is $installed, Cargo.toml pins $pinned:" >&2
    echo "  cargo install wasm-bindgen-cli --version $pinned --locked" >&2
    exit 1
fi

cargo build --profile dist --target wasm32-unknown-unknown -p xeibe-wasm
out=web/src/wasm
rm -rf "$out"
wasm-bindgen --target web --out-dir "$out" \
    target/wasm32-unknown-unknown/dist/xeibe_wasm.wasm
# The features Rust enables by default for wasm32-unknown-unknown.
web/node_modules/.bin/wasm-opt -O3 \
    --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
    --enable-mutable-globals --enable-reference-types --enable-multivalue \
    "$out/xeibe_wasm_bg.wasm" -o "$out/xeibe_wasm_bg.wasm"
ls -l "$out/xeibe_wasm_bg.wasm"
