#!/usr/bin/env bash
# Build the library, generate the Python bindings, and run the plugin-in-Python smoke test.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -p shelfscan
case "$(uname -s)" in Darwin) LIB=libshelfscan.dylib ;; *) LIB=libshelfscan.so ;; esac
OUT=bindings/python/generated
rm -rf "$OUT" && mkdir -p "$OUT"
cargo run -q -p uniffi-bindgen -- generate --library "target/debug/$LIB" --language python --out-dir "$OUT"
cp "target/debug/$LIB" "$OUT/"
python3 bindings/python/smoke_test.py
