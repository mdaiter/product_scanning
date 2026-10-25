#!/usr/bin/env bash
# Build apple/build/shelfscanFFI.xcframework (iOS device, iOS simulator, macOS) and generate the Swift bindings
# into apple/Sources/ShelfScan.
#
# Needs a Mac with Xcode (xcodebuild) and rustup. NOT RUN on a Mac yet: treat the first run as a test.
# What was verified on Linux: `uniffi-bindgen-swift` produces shelfscan.swift, shelfscanFFI.h and a modulemap
# from libshelfscan.a. Cross-compiling to Apple targets and xcodebuild have not been tried.
set -euo pipefail
cd "$(dirname "$0")/.."

[ "$(uname -s)" = Darwin ] || { echo "run this on a Mac (it needs xcodebuild)"; exit 1; }

export IPHONEOS_DEPLOYMENT_TARGET="${IPHONEOS_DEPLOYMENT_TARGET:-17.0}"
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)   # add x86_64-apple-ios for Intel simulators
OUT=apple/build
LIB=libshelfscan.a

rustup target add "${TARGETS[@]}"
for t in "${TARGETS[@]}"; do
  cargo build -p shelfscan --release --target "$t"
done

rm -rf "$OUT" && mkdir -p "$OUT/include"
HOST_LIB="target/aarch64-apple-darwin/release/$LIB"

# Swift sources (public API), C header, and a plain (non-framework) modulemap named shelfscanFFI,
# which is the module the generated Swift file imports.
cargo run -q -p uniffi-bindgen-swift -- "$HOST_LIB" apple/Sources/ShelfScan --swift-sources
cargo run -q -p uniffi-bindgen-swift -- "$HOST_LIB" "$OUT/include" --headers
cargo run -q -p uniffi-bindgen-swift -- "$HOST_LIB" "$OUT/include" --modulemap --module-name shelfscanFFI --modulemap-filename module.modulemap

ARGS=()
for t in "${TARGETS[@]}"; do ARGS+=(-library "target/$t/release/$LIB" -headers "$OUT/include"); done
xcodebuild -create-xcframework "${ARGS[@]}" -output "$OUT/shelfscanFFI.xcframework"

echo "done: $OUT/shelfscanFFI.xcframework and apple/Sources/ShelfScan/shelfscan.swift"
echo "next: cd apple && swift build"
