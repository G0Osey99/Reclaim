#!/usr/bin/env bash
# update.sh — fill the formula + cask version and SHA256 from a PUBLISHED release.
# Run after `gh release create` has uploaded Reclaim-<ver>.dmg and the tag exists.
#   packaging/homebrew-reclaim/update.sh 1.0.0
set -euo pipefail
V="${1:?usage: update.sh <version, e.g. 1.0.0>}"
REPO="G0Osey99/reclaim"
here="$(cd "$(dirname "$0")" && pwd)"
src_url="https://github.com/$REPO/archive/refs/tags/v$V.tar.gz"
dmg_url="https://github.com/$REPO/releases/download/v$V/Reclaim-$V.dmg"

echo "hashing $src_url"
src_sha="$(curl -fsSL "$src_url" | shasum -a 256 | awk '{print $1}')"
echo "hashing $dmg_url"
dmg_sha="$(curl -fsSL "$dmg_url" | shasum -a 256 | awk '{print $1}')"

# Formula (source): bump the tag in the url and set the sha.
sed -i '' -E \
  -e "s|/tags/v[0-9][0-9.]*\.tar\.gz|/tags/v$V.tar.gz|" \
  -e "s|^(  sha256 )\"[A-Za-z0-9_]*\"|\1\"$src_sha\"|" \
  "$here/Formula/reclaim.rb"

# Cask (DMG): bump version and set the sha.
sed -i '' -E \
  -e "s|^(  version )\"[0-9.]*\"|\1\"$V\"|" \
  -e "s|^(  sha256 )\"[A-Za-z0-9_]*\"|\1\"$dmg_sha\"|" \
  "$here/Casks/reclaim.rb"

echo "updated:"
echo "  Formula/reclaim.rb  src sha256 = $src_sha"
echo "  Casks/reclaim.rb    dmg sha256 = $dmg_sha"
echo "commit + push these to github.com/G0Osey99/homebrew-reclaim"
