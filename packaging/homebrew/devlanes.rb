# Template for Formula/devlanes.rb in devlanes-run/homebrew-tap.
# packaging/homebrew/render.sh replaces the @...@ placeholders from a release's SHA256SUMS.
class Devlanes < Formula
  desc "Development environments in Git worktree lanes"
  homepage "https://devlanes.run"
  license "Apache-2.0"

  livecheck do
    url :stable
    strategy :github_latest
  end

  # devlanes-gui comes from the release's GUI archive, next to the main one.
  on_macos do
    on_arm do
      url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA256_AARCH64_APPLE_DARWIN@"

      resource "gui" do
        url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-aarch64-apple-darwin-gui.tar.gz"
        sha256 "@SHA256_AARCH64_APPLE_DARWIN_GUI@"
      end
    end
    on_intel do
      url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA256_X86_64_APPLE_DARWIN@"

      resource "gui" do
        url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-x86_64-apple-darwin-gui.tar.gz"
        sha256 "@SHA256_X86_64_APPLE_DARWIN_GUI@"
      end
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_AARCH64_UNKNOWN_LINUX_GNU@"

      resource "gui" do
        url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-aarch64-unknown-linux-gnu-gui.tar.gz"
        sha256 "@SHA256_AARCH64_UNKNOWN_LINUX_GNU_GUI@"
      end
    end
    on_intel do
      url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_X86_64_UNKNOWN_LINUX_GNU@"

      resource "gui" do
        url "https://github.com/devlanes-run/devlanes/releases/download/v@VERSION@/devlanes-@VERSION@-x86_64-unknown-linux-gnu-gui.tar.gz"
        sha256 "@SHA256_X86_64_UNKNOWN_LINUX_GNU_GUI@"
      end
    end
  end

  def install
    # devlanes looks for devlanes-hosts next to itself when `devlanes setup`
    # copies the helper to /usr/local/libexec, and for devlanes-gui there too,
    # so all three go into the same bin.
    bin.install "devlanes", "devlanes-hosts"
    resource("gui").stage { bin.install "devlanes-gui" }
  end

  test do
    assert_match "devlanes #{version}", shell_output("#{bin}/devlanes --version")
    assert_match "build=release", shell_output("#{bin}/devlanes-hosts version")
    assert_match "devlanes-gui #{version} build=release", shell_output("#{bin}/devlanes-gui version")
  end
end
