# Homebrew cask for the Reclaim app (tap: G0Osey99/homebrew-reclaim).
# Installs the notarized Reclaim.app from the GitHub release DMG. SHA256 is filled
# in by packaging/update.sh once the release DMG is uploaded.
cask "reclaim" do
  version "1.0.0"
  sha256 "REPLACE_WITH_DMG_SHA256"

  url "https://github.com/G0Osey99/reclaim/releases/download/v#{version}/Reclaim-#{version}.dmg",
      verified: "github.com/G0Osey99/reclaim/"
  name "Reclaim"
  desc "Read-only disk, file, and photo recovery"
  homepage "https://github.com/G0Osey99/reclaim"

  depends_on macos: ">= :ventura" # macOS 13+

  app "Reclaim.app"

  # The privileged read-only helper (com.reclaim.helper) is registered by the app
  # on first run via SMAppService; the user removes it from Login Items. There is
  # no separate pkg to uninstall here.
  uninstall quit: "com.reclaim.app"

  zap trash: [
    "~/Library/Application Support/Reclaim",
    "~/Library/Preferences/com.reclaim.app.plist",
    "~/Library/Caches/com.reclaim.app",
  ]

  caveats <<~EOS
    Reclaim installs a read-only privileged helper on first launch (approve it in
    System Settings → General → Login Items & Extensions) and needs Full Disk
    Access to read raw devices. It never writes to a scanned disk.
  EOS
end
