#!/usr/bin/env bash
# build-app.sh — build, assemble and sign Reclaim.app (build guide Phase-5 C,
# Part 2.3 trap 4; docs/plan/06 §9).
#
# 1. Build a universal reclaim-ffi static lib (arm64 + x86_64, lipo).
# 2. Regenerate the Swift bindings from it.
# 3. Build a universal Reclaim + ReclaimHelper via SwiftPM.
# 4. Assemble dist/Reclaim.app with the embedded helper, LaunchDaemons plist,
#    Info.plist and hardened-runtime entitlements.
# 5. If DEVELOPMENT_TEAM + a Developer ID Application cert exist: codesign
#    (hardened runtime), build a DMG, `notarytool submit --wait`, staple.
#    Otherwise: ad-hoc sign and print the exact Phase-6 commands.
#
# Never blocks on the Apple account (trap 4).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
export MACOSX_DEPLOYMENT_TARGET=13.0

ROOT="$(pwd)"
PKG="$ROOT/apps/Reclaim"
DIST="$ROOT/dist"
APP="$DIST/Reclaim.app"
VERSION="0.5.0-beta"
BUNDLE_ID="com.reclaim.app"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }

# --- 1. universal FFI static lib -------------------------------------------
step "cargo build -p reclaim-ffi (release, arm64 + x86_64)"
cargo build --release -p reclaim-ffi --target aarch64-apple-darwin
cargo build --release -p reclaim-ffi --target x86_64-apple-darwin
mkdir -p "$PKG/Frameworks"
lipo -create \
    "target/aarch64-apple-darwin/release/libreclaim_ffi.a" \
    "target/x86_64-apple-darwin/release/libreclaim_ffi.a" \
    -output "$PKG/Frameworks/libreclaim_ffi.a"
lipo -info "$PKG/Frameworks/libreclaim_ffi.a"

# --- 2. Swift bindings ------------------------------------------------------
step "regenerate Swift bindings"
GEN="$(mktemp -d)"
cargo run --features cli --bin uniffi-bindgen -- \
    generate --library "target/aarch64-apple-darwin/release/libreclaim_ffi.dylib" \
    --language swift --out-dir "$GEN"
cp "$GEN/reclaim_ffi.swift" "$PKG/Sources/ReclaimCore/reclaim_ffi.swift"
cp "$GEN/reclaim_ffiFFI.h" "$PKG/Sources/ReclaimFFI/include/reclaim_ffiFFI.h"
cat > "$PKG/Sources/ReclaimFFI/include/module.modulemap" <<'MAP'
module reclaim_ffiFFI {
    header "reclaim_ffiFFI.h"
    export *
}
MAP
rm -rf "$GEN"

# --- 3. per-arch SwiftPM build + lipo ---------------------------------------
# The universal `swift build --arch … --arch …` path needs XCBuild (full Xcode);
# under Command Line Tools we build each triple with the native build system and
# lipo the executables (the FFI static lib is already fat).
build_triple() {
    local triple="$1"
    ( cd "$PKG" && swift build -c release --triple "$triple" \
        --product Reclaim --product ReclaimHelper 2>&1 \
        | grep -vE "ld: warning|was built for newer" || true )
}
step "swift build (release, arm64 + x86_64 via --triple)"
build_triple "arm64-apple-macosx13.0"
build_triple "x86_64-apple-macosx13.0"
ARM_DIR="$PKG/.build/arm64-apple-macosx/release"
X86_DIR="$PKG/.build/x86_64-apple-macosx/release"
STAGE="$(mktemp -d)"
lipo -create "$ARM_DIR/Reclaim" "$X86_DIR/Reclaim" -output "$STAGE/Reclaim"
lipo -create "$ARM_DIR/ReclaimHelper" "$X86_DIR/ReclaimHelper" -output "$STAGE/ReclaimHelper"
BIN_DIR="$STAGE"
echo "universal binaries in: $BIN_DIR"

# --- 4. assemble the bundle -------------------------------------------------
step "assemble $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS"
mkdir -p "$APP/Contents/Library/LaunchDaemons"
mkdir -p "$APP/Contents/Resources"
cp "$BIN_DIR/Reclaim" "$APP/Contents/MacOS/Reclaim"
cp "$BIN_DIR/ReclaimHelper" "$APP/Contents/MacOS/ReclaimHelper"
cp "$PKG/packaging/Info.plist" "$APP/Contents/Info.plist"
cp "$PKG/packaging/com.reclaim.helper.plist" \
    "$APP/Contents/Library/LaunchDaemons/com.reclaim.helper.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $VERSION" \
    "$APP/Contents/Info.plist" 2>/dev/null || true
lipo -info "$APP/Contents/MacOS/Reclaim" || true

# --- 5. sign / package ------------------------------------------------------
ENTITLEMENTS="$PKG/packaging/Reclaim.entitlements"
DEVID_CERT="$(security find-identity -v -p codesigning 2>/dev/null \
    | grep -o 'Developer ID Application[^"]*' | head -1 || true)"

if [ -n "${DEVELOPMENT_TEAM:-}" ] && [ -n "$DEVID_CERT" ]; then
    step "codesign with Developer ID ($DEVID_CERT), hardened runtime"
    SIGN=(codesign --force --timestamp --options runtime \
        --sign "Developer ID Application")
    # Inside-out: helper first, then the app.
    "${SIGN[@]}" --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/ReclaimHelper"
    "${SIGN[@]}" --entitlements "$ENTITLEMENTS" "$APP"
    codesign --verify --deep --strict --verbose=2 "$APP"

    step "create DMG"
    DMG="$DIST/Reclaim-$VERSION.dmg"
    rm -f "$DMG"
    hdiutil create -volname "Reclaim" -srcfolder "$APP" -ov -format UDZO "$DMG"

    step "notarize + staple"
    if xcrun notarytool submit "$DMG" --keychain-profile "reclaim-notary" --wait; then
        xcrun stapler staple "$DMG"
        xcrun stapler staple "$APP"
        echo "notarized + stapled: $DMG"
    else
        echo "notarytool failed — check 'xcrun notarytool store-credentials reclaim-notary'."
    fi
else
    step "ad-hoc sign (no Developer ID / DEVELOPMENT_TEAM — Part 2.3 trap 4)"
    codesign --force --options runtime --sign - \
        --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/ReclaimHelper"
    codesign --force --options runtime --sign - \
        --entitlements "$ENTITLEMENTS" "$APP"
    codesign --verify --verbose=2 "$APP" || true
    cat <<EOF

------------------------------------------------------------------------
Ad-hoc signed: $APP
This launches locally but is NOT notarized and the SMAppService daemon
will not install cleanly without a stable (Developer ID) signature.

Phase 6, once the Apple Developer account exists (Part 5.3), run:

  export DEVELOPMENT_TEAM=<TEAMID>
  # one-time notary credential:
  xcrun notarytool store-credentials reclaim-notary \\
      --apple-id <APPLE_ID> --team-id <TEAMID> --password <APP_SPECIFIC_PW>

  # then re-run this script (it will Developer ID sign + notarize), or manually:
  codesign --force --timestamp --options runtime \\
      --sign "Developer ID Application: <NAME> (<TEAMID>)" \\
      --entitlements "$ENTITLEMENTS" "$APP/Contents/MacOS/ReclaimHelper"
  codesign --force --timestamp --options runtime \\
      --sign "Developer ID Application: <NAME> (<TEAMID>)" \\
      --entitlements "$ENTITLEMENTS" "$APP"
  hdiutil create -volname Reclaim -srcfolder "$APP" -ov -format UDZO \\
      "$DIST/Reclaim-$VERSION.dmg"
  xcrun notarytool submit "$DIST/Reclaim-$VERSION.dmg" \\
      --keychain-profile reclaim-notary --wait
  xcrun stapler staple "$DIST/Reclaim-$VERSION.dmg"
  xcrun stapler staple "$APP"
------------------------------------------------------------------------
EOF
fi

step "done"
echo "artifact: $APP"
