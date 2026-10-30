#!/usr/bin/env bash
# Builds the Rust core for macOS and generates the Swift bindings into apple/Generated.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/apple/Generated"
target="aarch64-apple-darwin"
export MACOSX_DEPLOYMENT_TARGET="14.0"

cargo build --manifest-path "$root/core/Cargo.toml" --release --target "$target"

# Bindings are read from an unstripped debug build: the release profile strips
# the metadata symbols the generator looks for.
cargo build --manifest-path "$root/core/Cargo.toml"
rm -rf "$out"
mkdir -p "$out/lib"
# The generator runs `cargo metadata` in the current directory.
(cd "$root/core" && cargo run --quiet --features cli --bin uniffi-bindgen -- \
    generate --library target/debug/libnotes_core.dylib --language swift --out-dir "$out")

# Swift finds a C module only under this exact file name.
mv "$out/notes_coreFFI.modulemap" "$out/module.modulemap"
cp "$root/core/target/$target/release/libnotes_core.a" "$out/lib/"
echo "apple: core and bindings are in $out"
