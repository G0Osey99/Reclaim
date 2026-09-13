# Homebrew formula for the Reclaim CLI (tap: G0Osey99/homebrew-reclaim).
# Builds from source so `brew install --build-from-source G0Osey99/reclaim/reclaim`
# works on a clean machine. SHA256 is filled in by packaging/update.sh once the
# vX.Y.Z tag exists on GitHub.
class Reclaim < Formula
  desc "Read-only disk, file, and photo recovery (CLI)"
  homepage "https://github.com/G0Osey99/reclaim"
  url "https://github.com/G0Osey99/reclaim/archive/refs/tags/v1.0.0.tar.gz"
  sha256 "REPLACE_WITH_SOURCE_TARBALL_SHA256"
  license "Apache-2.0"
  head "https://github.com/G0Osey99/reclaim.git", branch: "main"

  depends_on "rust" => :build
  depends_on :macos # raw-device enumeration and the health probe are macOS-only

  def install
    system "cargo", "build", "--release", "--locked", "--package", "reclaim-cli"
    bin.install "target/release/reclaim"

    # Man page + shell completions from the clap definition (feature `docgen`).
    ENV["RECLAIM_GEN_DOCS"] = buildpath/"clidocs"
    system "cargo", "run", "--release", "--locked", "--package", "reclaim-cli",
           "--features", "docgen"
    man1.install Dir[buildpath/"clidocs/man/*.1"]
    bash_completion.install "#{buildpath}/clidocs/completions/reclaim.bash" => "reclaim"
    zsh_completion.install "#{buildpath}/clidocs/completions/_reclaim"
    fish_completion.install "#{buildpath}/clidocs/completions/reclaim.fish"
  end

  test do
    assert_match "reclaim #{version}", shell_output("#{bin}/reclaim --version")
    # `list` runs without root and must exit cleanly.
    system bin/"reclaim", "list"
  end
end
