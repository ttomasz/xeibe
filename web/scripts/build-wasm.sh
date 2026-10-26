#!/bin/sh
# Build the pages' wasm modules into web/src/wasm (generated, not in git):
# each module, wasm-bindgen's JavaScript glue, then wasm-opt.
#
#   crates/xeibe-wasm            → xeibe_wasm.js, xeibe_wasm_bg.wasm (GML page)
#   crates/xeibe-wasm-reproject  → xeibe_wasm_reproject.js, … (reproject page)
#
# `build-wasm.sh gml` or `build-wasm.sh reproject` builds one; no argument
# builds both.
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

out=web/src/wasm
mkdir -p "$out"

build() {
    crate=$1
    name=$(echo "$crate" | tr - _)
    cargo build --profile dist --target wasm32-unknown-unknown -p "$crate"
    rm -f "$out/$name.js" "$out/$name.d.ts" "$out/${name}_bg.wasm" "$out/${name}_bg.wasm.d.ts"
    wasm-bindgen --target web --out-dir "$out" \
        "target/wasm32-unknown-unknown/dist/$name.wasm"
    # The features Rust enables by default for wasm32-unknown-unknown.
    web/node_modules/.bin/wasm-opt -O3 \
        --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext \
        --enable-mutable-globals --enable-reference-types --enable-multivalue \
        "$out/${name}_bg.wasm" -o "$out/${name}_bg.wasm"
    ls -l "$out/${name}_bg.wasm"
}

[ $# -gt 0 ] || set -- gml reproject
for page in "$@"; do
    case $page in
        gml) build xeibe-wasm ;;
        reproject) build xeibe-wasm-reproject ;;
        *) echo "unknown page $page: gml or reproject" >&2; exit 1 ;;
    esac
done
