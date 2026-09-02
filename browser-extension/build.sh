#!/bin/sh
set -eu

extension_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
dist_dir="$extension_dir/dist"
target_dir="$extension_dir/target"

wasm_bindgen_bin=${WASM_BINDGEN:-wasm-bindgen}
if ! command -v "$wasm_bindgen_bin" >/dev/null 2>&1; then
  echo "wasm-bindgen is required. Install wasm-bindgen-cli before this build." >&2
  echo "You can also set WASM_BINDGEN to its absolute path." >&2
  exit 1
fi

rm -rf "$dist_dir"
mkdir -p "$dist_dir/wasm"
cargo build \
  --manifest-path "$extension_dir/Cargo.toml" \
  --target wasm32-unknown-unknown \
  --target-dir "$target_dir" \
  --release

"$wasm_bindgen_bin" \
  "$target_dir/wasm32-unknown-unknown/release/csilctl_browser_decoder.wasm" \
  --target web \
  --out-dir "$dist_dir/wasm" \
  --out-name csil_decoder \
  --no-typescript

cp "$extension_dir"/static/* "$dist_dir"/
echo "Built unpacked extension: $dist_dir"
