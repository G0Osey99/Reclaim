#!/usr/bin/env bash
# release.sh — build every 1.0 release artifact (build guide Part 3.7 / Phase 6 E).
#
# Produces, under dist/release/<version>/:
#   reclaim-<ver>-aarch64-apple-darwin.tar.gz     CLI (Apple Silicon)
#   reclaim-<ver>-x86_64-apple-darwin.tar.gz      CLI (Intel)
#   reclaim-<ver>-universal-apple-darwin.tar.gz   CLI (fat)
#   reclaim-<ver>-x86_64-linux.tar.gz             CLI (Linux; needs a cross toolchain or CI)
#   Reclaim-<ver>.dmg                             app (built + notarized by build-app.sh)
#   appcast.xml                                   Sparkle feed (served from GitHub Releases)
#   SHA256SUMS
#
# Signing/notarization runs automatically WHEN the inputs exist and is cleanly
# SKIPPED with an explanation when they don't (build guide Part 5.3 — Ryker):
#   * DEVELOPMENT_TEAM + a "Developer ID Application" cert   -> codesign + notarize
#   * a Sparkle EdDSA key (Sparkle keychain item, or RECLAIM_SPARKLE_KEY) +
#     Sparkle's `sign_update`                                -> signed appcast
#
# Nothing here publishes: it only builds locally. `gh release create` and the tap
# push are separate, deliberate steps (see docs/build-log/phase-6.md).
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"

VERSION="${1:-$(awk -F'"' '/^version =/{print $2; exit}' Cargo.toml)}"
REPO="G0Osey99/reclaim"
OUT="dist/release/$VERSION"
STAGE="$OUT/.stage"
rm -rf "$OUT"; mkdir -p "$OUT" "$STAGE"
have() { command -v "$1" >/dev/null 2>&1; }
step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }

devid_identity() {
    security find-identity -v -p codesigning 2>/dev/null \
        | grep -o 'Developer ID Application[^"]*' | head -1
}

# Developer-ID sign (and, with a notary profile, notarize) a standalone CLI
# binary; ad-hoc sign otherwise. A bare Mach-O cannot be stapled — notarization
# is served online — so the tarball ships a signed, notarized-checked binary.
codesign_cli() {  # <binary>
    local bin="$1" id
    id="$(devid_identity)"
    if [ -n "${DEVELOPMENT_TEAM:-}" ] && [ -n "$id" ]; then
        codesign --force --timestamp --options runtime \
            --sign "Developer ID Application" "$bin" && echo "  codesigned (Developer ID): $bin"
        if xcrun notarytool history --keychain-profile reclaim-notary >/dev/null 2>&1; then
            local z="${bin}.zip"; /usr/bin/ditto -c -k "$bin" "$z"
            xcrun notarytool submit "$z" --keychain-profile reclaim-notary --wait >/dev/null 2>&1 \
                && echo "  notarized (online ticket; a bare CLI binary is not staplable)"
            rm -f "$z"
        fi
    else
        codesign --force --sign - "$bin" 2>/dev/null || true
        echo "  ad-hoc signed — set DEVELOPMENT_TEAM + a Developer ID cert for a notarized CLI (Part 5.3)"
    fi
}

pack_cli() {  # <triple-label> <binary-path>
    local label="$1" bin="$2"
    local d="$STAGE/reclaim-$VERSION-$label"
    rm -rf "$d"; mkdir -p "$d/man" "$d/completions"
    cp "$bin" "$d/reclaim"; chmod +x "$d/reclaim"
    cp LICENSE README.md CHANGELOG.md THIRD_PARTY.md docs/cli.md "$d/" 2>/dev/null || true
    cp "$STAGE/clidocs/man/"*.1 "$d/man/" 2>/dev/null || true
    cp "$STAGE/clidocs/completions/"* "$d/completions/" 2>/dev/null || true
    ( cd "$STAGE" && tar czf "../reclaim-$VERSION-$label.tar.gz" "reclaim-$VERSION-$label" )
    echo "packed reclaim-$VERSION-$label.tar.gz"
}

# Sparkle appcast. Signs with `sign_update` if the tool + EdDSA key are present,
# otherwise writes an unsigned feed and prints exactly what Ryker must run.
gen_appcast() {  # <dmg-path> <out-xml>
    local dmg="$1" xml="$2"
    local url="https://github.com/$REPO/releases/download/v$VERSION/Reclaim-$VERSION.dmg"
    local len=0 sig="" signtool=""
    [ -f "$dmg" ] && len="$(stat -f%z "$dmg" 2>/dev/null || echo 0)"
    have sign_update && signtool="sign_update"
    if [ -f "$dmg" ] && [ -n "$signtool" ]; then
        if [ -n "${RECLAIM_SPARKLE_KEY:-}" ]; then
            sig="$("$signtool" "$dmg" -f "$RECLAIM_SPARKLE_KEY" 2>/dev/null | sed -n 's/.*edSignature="\([^"]*\)".*/\1/p')"
        else
            sig="$("$signtool" "$dmg" 2>/dev/null | sed -n 's/.*edSignature="\([^"]*\)".*/\1/p')"
        fi
    fi
    local sigattr=""; [ -n "$sig" ] && sigattr=" sparkle:edSignature=\"$sig\""
    {
        echo '<?xml version="1.0" standalone="yes"?>'
        echo '<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" xmlns:dc="http://purl.org/dc/elements/1.1/">'
        echo '  <channel>'
        echo '    <title>Reclaim</title>'
        echo "    <link>https://github.com/$REPO</link>"
        echo '    <description>Reclaim application updates</description>'
        echo '    <language>en</language>'
        echo '    <item>'
        echo "      <title>Version $VERSION</title>"
        echo "      <pubDate>$(date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>"
        echo "      <sparkle:version>$VERSION</sparkle:version>"
        echo "      <sparkle:shortVersionString>$VERSION</sparkle:shortVersionString>"
        echo '      <sparkle:minimumSystemVersion>13.0</sparkle:minimumSystemVersion>'
        echo "      <description><![CDATA[See the changelog: https://github.com/$REPO/blob/main/CHANGELOG.md]]></description>"
        echo "      <enclosure url=\"$url\" sparkle:version=\"$VERSION\" sparkle:shortVersionString=\"$VERSION\" length=\"$len\" type=\"application/x-apple-diskimage\"$sigattr />"
        echo '    </item>'
        echo '  </channel>'
        echo '</rss>'
    } > "$xml"
    echo "wrote $xml"
    [ -z "$sig" ] && echo "  NOTE: appcast is UNSIGNED — Ryker generates the EdDSA key (Sparkle generate_keys) and re-runs with sign_update (Part 5.3). See docs/sparkle.md."
}

# --- shared doc payload -----------------------------------------------------
step "generate CLI docs (man pages + completions)"
scripts/gen-cli-docs.sh "$STAGE/clidocs" >/dev/null

# --- macOS CLI: per-arch + universal ---------------------------------------
DARWIN_BINS=""
for t in aarch64-apple-darwin x86_64-apple-darwin; do
    step "build CLI $t"
    rustup target add "$t" >/dev/null 2>&1 || true
    if cargo build --release -p reclaim-cli --target "$t"; then
        DARWIN_BINS="$DARWIN_BINS target/$t/release/reclaim"
        codesign_cli "target/$t/release/reclaim"
        pack_cli "$t" "target/$t/release/reclaim"
    else
        echo "SKIP $t: build failed"
    fi
done
if [ -n "$DARWIN_BINS" ]; then
    step "lipo universal CLI"
    # shellcheck disable=SC2086
    if lipo -create $DARWIN_BINS -output "$STAGE/reclaim-universal"; then
        codesign_cli "$STAGE/reclaim-universal"
        pack_cli "universal-apple-darwin" "$STAGE/reclaim-universal"
    fi
fi

# --- Linux CLI --------------------------------------------------------------
step "build CLI x86_64-linux"
LT=x86_64-unknown-linux-gnu
if cargo zigbuild --version >/dev/null 2>&1; then
    rustup target add "$LT" >/dev/null 2>&1 || true
    cargo zigbuild --release -p reclaim-cli --target "$LT" \
        && pack_cli "x86_64-linux" "target/$LT/release/reclaim" \
        || echo "SKIP linux: zigbuild failed"
elif rustup target list --installed 2>/dev/null | grep -q "$LT" \
        && cargo build --release -p reclaim-cli --target "$LT" 2>/dev/null; then
    pack_cli "x86_64-linux" "target/$LT/release/reclaim"
else
    echo "SKIP linux CLI: no cross toolchain on this host."
    echo "  build it on the Linux CI runner, or: cargo install cargo-zigbuild && rustup target add $LT"
fi

# --- app + DMG (build-app.sh signs/notarizes in devid mode when creds exist) -
step "build universal app + DMG"
RECLAIM_VERSION="$VERSION" scripts/build-app.sh || echo "app build reported issues (see above)"
[ -f "dist/Reclaim-$VERSION.dmg" ] && cp "dist/Reclaim-$VERSION.dmg" "$OUT/" && echo "copied Reclaim-$VERSION.dmg"

# --- Sparkle appcast + checksums -------------------------------------------
step "generate appcast.xml"
gen_appcast "$OUT/Reclaim-$VERSION.dmg" "$OUT/appcast.xml"

step "SHA256SUMS"
( cd "$OUT" && shasum -a 256 ./*.tar.gz ./*.dmg 2>/dev/null | sed 's# \./# #' > SHA256SUMS && cat SHA256SUMS )

step "done — nothing published"
echo "artifacts in $OUT/"
ls -la "$OUT"
rm -rf "$STAGE"
