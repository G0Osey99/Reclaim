# Sparkle auto-update

Reclaim.app updates itself with [Sparkle 2](https://sparkle-project.org). The
appcast is served from GitHub Releases and every update is signed with an EdDSA
(ed25519) key so the app only installs updates that came from us. This is the
integration and release procedure.

## One-time: generate the signing key (Ryker — build guide Part 5.3)

```bash
# From the Sparkle release's bin/ (or `brew install --cask sparkle` tools):
./generate_keys
```

`generate_keys` stores the **private** key in the login keychain (keep it out of
the repo, never commit it) and prints the **public** key (base64). Put the public
key in the app's Info.plist:

- `apps/Reclaim/packaging/Info.plist` → `SUPublicEDKey` = *(the base64 public key)*
  (a placeholder is in place now).

The feed URL is already set:

- `SUFeedURL` = `https://github.com/G0Osey99/reclaim/releases/latest/download/appcast.xml`

## App wiring (Xcode / SwiftPM)

Sparkle ships as a binary `.xcframework` consumed via SwiftPM. Add it to
`apps/Reclaim/Package.swift`:

```swift
// in dependencies:
.package(url: "https://github.com/sparkle-project/Sparkle", from: "2.6.0"),
// in the `Reclaim` executable target's dependencies:
.product(name: "Sparkle", package: "Sparkle"),
```

Add a standard updater controller and a **Check for Updates…** menu item:

```swift
import Sparkle

final class Updater: ObservableObject {
    let controller = SPUStandardUpdaterController(startingUpdater: true,
                                                  updaterDelegate: nil,
                                                  userDriverDelegate: nil)
}

// In the App scene:
.commands {
    CommandGroup(after: .appInfo) {
        Button("Check for Updates…") { updater.controller.checkForUpdates(nil) }
    }
}
```

> **Note:** the SwiftPM binary target requires full Xcode to resolve; this repo's
> Command-Line-Tools build path (`scripts/build-app.sh`) does not add Sparkle. Do
> the SwiftPM addition and the `xcodebuild` build in Xcode. Everything else
> (appcast, feed URL, key, release flow) is ready.

## Release flow (each version)

`scripts/release.sh` already generates `dist/release/<ver>/appcast.xml` pointing at
the release DMG. It signs the enclosure automatically when Sparkle's `sign_update`
tool and the key are available:

```bash
# sign_update is in the Sparkle distribution's bin/ (put it on PATH).
scripts/release.sh 1.0.0          # emits appcast.xml (signed if the key is present)
# or explicitly with a key file:
RECLAIM_SPARKLE_KEY=~/keys/reclaim_ed_private scripts/release.sh 1.0.0
```

Then upload `appcast.xml` and `Reclaim-<ver>.dmg` to the GitHub release (the
`latest/download/appcast.xml` URL always resolves to the newest release's
appcast). The app checks that URL, verifies the EdDSA signature against
`SUPublicEDKey`, and offers the update.

## Security properties

- Updates are **EdDSA-signed**; an unsigned or wrong-key appcast is rejected by
  the app. The private key never leaves Ryker's keychain.
- The DMG is also Developer-ID-signed, notarized and stapled (Gatekeeper), so
  there are two independent trust checks.
