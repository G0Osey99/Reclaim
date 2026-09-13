# homebrew-reclaim

Homebrew tap for [Reclaim](https://github.com/G0Osey99/reclaim) — read-only disk,
file, and photo recovery for macOS.

**The contents of this directory become the `G0Osey99/homebrew-reclaim` repo.**
Push `Formula/` and `Casks/` to the root of that repo (Homebrew taps expect those
directory names at the repo root).

## Install

```bash
# CLI (builds from source)
brew install G0Osey99/reclaim/reclaim

# App (notarized DMG)
brew install --cask G0Osey99/reclaim/reclaim
```

## Releasing a new version

1. Publish the GitHub release (tag `vX.Y.Z`, upload `Reclaim-X.Y.Z.dmg`).
2. `packaging/homebrew-reclaim/update.sh X.Y.Z` — fills the version + SHA256 for
   both the formula (source tarball) and the cask (DMG).
3. Commit and push `Formula/reclaim.rb` and `Casks/reclaim.rb` to the tap repo.
4. Verify on a clean machine:
   ```bash
   brew install --build-from-source G0Osey99/reclaim/reclaim
   brew install --cask G0Osey99/reclaim/reclaim
   ```
