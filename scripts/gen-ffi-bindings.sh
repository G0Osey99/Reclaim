#!/usr/bin/env bash
# gen-ffi-bindings.sh — build the reclaim-ffi library and regenerate the Swift
# bindings + header into apps/Reclaim (build guide Phase-5 prompt A).
#
#   scripts/gen-ffi-bindings.sh [debug|release]
#
# Debug (default) builds a host-arch static lib for a fast `swift build` /
# selftest loop; `release` is for scripts/build-app.sh (which lipo's a universal
# lib itself). The generated sources are committed so the package builds without
# a Cargo step, but this script is the source of truth for them.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
# Match the app's minimum OS (macOS 13) so the C objects (zstd/sqlite) don't
# trip the linker's "built for newer macOS" warning against the SwiftPM target.
export MACOSX_DEPLOYMENT_TARGET=13.0

PROFILE="${1:-debug}"
PKG="apps/Reclaim"
FFI_INCLUDE="$PKG/Sources/ReclaimFFI/include"
CORE_SRC="$PKG/Sources/ReclaimCore"

flag=""
[ "$PROFILE" = "release" ] && flag="--release"

echo "==> cargo build -p reclaim-ffi $flag"
cargo build -p reclaim-ffi $flag

OUT="target/$PROFILE"
DYLIB="$OUT/libreclaim_ffi.dylib"
STATIC="$OUT/libreclaim_ffi.a"

echo "==> generate Swift bindings (library mode) from $DYLIB"
GEN="$(mktemp -d)"
cargo run --features cli --bin uniffi-bindgen -- \
    generate --library "$DYLIB" --language swift --out-dir "$GEN"

echo "==> install generated sources into $PKG"
cp "$GEN/reclaim_ffi.swift" "$CORE_SRC/reclaim_ffi.swift"
cp "$GEN/reclaim_ffiFFI.h" "$FFI_INCLUDE/reclaim_ffiFFI.h"
# SwiftPM expects a `module.modulemap`; write a minimal one (the uniffi-emitted
# `use "Darwin"` directives are dropped — the header pulls in what it needs).
cat > "$FFI_INCLUDE/module.modulemap" <<'MAP'
module reclaim_ffiFFI {
    header "reclaim_ffiFFI.h"
    export *
}
MAP

echo "==> install $PROFILE static lib into $PKG/Frameworks"
cp "$STATIC" "$PKG/Frameworks/libreclaim_ffi.a"

rm -rf "$GEN"
echo "gen-ffi-bindings: done ($PROFILE)."
