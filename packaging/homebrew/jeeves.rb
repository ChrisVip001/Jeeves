# The Homebrew formula for Jeeves.
#
# Built from source rather than installed as a binary: Jeeves is a cargo workspace, so any
# install path other than the release tarball has to compile anyway — and `cargo install`
# is the standard move in the cargo world.
#
# Usage:
#
#   brew tap <your tap> && brew install jeeves
#
# To check it locally:
#
#   brew install --build-from-source ./packaging/homebrew/jeeves.rb
#   brew test jeeves
#
# How to get the real checksum after tagging a release:
#
#   brew fetch --build-from-source ./packaging/homebrew/jeeves.rb
class Jeeves < Formula
  desc "Personal assistant for complex, long-running tasks"
  homepage "https://github.com/ChrisVip001/Jeeves"
  url "https://github.com/ChrisVip001/Jeeves/archive/refs/tags/v0.17.0.tar.gz"
  # The sha256 is rewritten by `just release-hash <tag>` from that same
  # /archive/refs/tags/ URL (see packaging/RELEASING.md). Do not compute it from the
  # GitHub API's /tarball/ endpoint: that one is a legacy archive and the bytes differ.
  sha256 "f49e3d5e3eb0021bf54cdde731a3e5acbc0b32b25623f57f691c6f6967b780bd"
  license any_of: ["MIT", "Apache-2.0"]
  head "https://github.com/ChrisVip001/Jeeves.git", branch: "main"

  depends_on "rust" => :build

  def install
    # The CLI crate is a workspace member: install from its directory, and dependency
    # resolution still goes through the root Cargo.toml.
    system "cargo", "install", *std_cargo_args(path: "jeeves-rs/jeeves-cli")
  end

  service do
    # What stays resident is the gateway/cockpit side, not the REPL. JEEVES_HOME stays in the
    # user's home directory.
    run [opt_bin/"jeeves", "dashboard"]
    keep_alive true
    environment_variables JEEVES_HOME: "${HOME}/.jeeves"
  end

  test do
    assert_match "jeeves #{version}", shell_output("#{bin}/jeeves --version")
    # An unknown subcommand prints usage and exits non-zero — that path needs no API key and
    # touches no network, which makes it a good post-install smoke test.
    assert_match "Jeeves", shell_output("#{bin}/jeeves definitely-not-a-command 2>&1", 1)
  end
end
