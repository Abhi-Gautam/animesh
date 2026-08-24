# Homebrew formula for animesh.
#
# This directory is what makes the repository its own tap: Homebrew looks for
# formulae in `Formula/`, then `HomebrewFormula/`, then the root, so there is no
# separate `homebrew-animesh` repository to keep in sync with this one. The cost
# is that installing takes a `brew tap <name> <url>` first, because the
# `user/tap/formula` shorthand resolves only to a repo named `homebrew-*`.
#
# A formula rather than a cask,
# for two reasons. Casks download an artifact and Gatekeeper quarantines it, so
# an unnotarised app would refuse to launch on every machine but the author's —
# a formula's files are not quarantined, so v1 ships without notarisation.
# Second, the product on Linux is a CLI and a daemon, which is what a formula is
# for; the cask can come later on macOS once there is a Developer ID.
#
# Builds from the tagged source archive rather than shipping a prebuilt binary.
# On macOS the daemon has to live in an app bundle that is signed on the machine
# it runs on, and a formula that compiles is a shorter path to that than one
# that downloads, re-signs and hopes. The release also publishes prebuilt
# tarballs for people who would rather not compile.
class Animesh < Formula
  desc "Personal release radar for anime and other scheduled media"
  homepage "https://github.com/Abhi-Gautam/animesh"
  url "https://github.com/Abhi-Gautam/animesh/archive/refs/tags/v0.6.0.tar.gz"
  sha256 "3f0266d52c744341f49f22ad9ec0a3a97be9455458db86707e9268f850759958"
  license "MIT"
  head "https://github.com/Abhi-Gautam/animesh.git", branch: "master"

  depends_on "rust" => :build

  def install
    if OS.mac?
      # The daemon has to run from a bundle. UNUserNotificationCenter traps
      # without a bundle identifier, so a bare binary in bin/ could never post
      # a notification — xtask assembles and ad-hoc signs the app, and the CLI
      # is linked out of it.
      system "cargo", "xtask", "bundle", "--release"
      prefix.install "target/bundle/Animesh.app"
      bin.install_symlink prefix/"Animesh.app/Contents/Helpers/animesh"
    else
      # Both binaries: `animesh` is the CLI and `animesh-app` is the daemon the
      # systemd unit launches. Installing only the CLI would leave
      # `animesh service start` with nothing to register.
      system "cargo", "install", *std_cargo_args(path: ".")

      # What makes a notification server show Animesh by name in its per-app
      # settings, instead of an unnamed sender.
      (share/"applications").install "assets/animesh.desktop"
    end
  end

  def caveats
    notes = <<~TEXT
      Run Animesh in the background and start it at login:

        animesh service start

      Then find something to follow:

        animesh search "one piece"
        animesh follow 21
        animesh next

      To let any AI agent read and edit your library, install the agent skill:

        animesh skill install

    TEXT

    notes += if OS.mac?
      <<~TEXT
        macOS will ask for notification permission the first time the daemon
        starts. Declining is fine — the CLI does not depend on it.

        This build is ad-hoc signed, so macOS may ask again after an upgrade.
      TEXT
    else
      <<~TEXT
        Notifications go through your desktop's notification server, and Animesh
        appears in its per-app notification settings once it has sent one.

        User services stop at logout, so if `animesh service start` could not
        enable lingering, run:

          sudo loginctl enable-linger $USER
      TEXT
    end

    notes
  end

  test do
    # The CLI must answer without a daemon, a database, or a desktop session.
    assert_match "animesh", shell_output("#{bin}/animesh --version")

    # And it must fail honestly when the daemon is not running rather than
    # hanging or reporting success: exit 3 is the retryable category.
    output = shell_output("#{bin}/animesh next 2>&1", 3)
    assert_match "not running", output

    # The machine-readable mode is the agent contract, and it has to hold in the
    # failure case too — one JSON document on stdout, with a stable code.
    document = JSON.parse(shell_output("#{bin}/animesh --json next", 3))
    refute document.fetch("ok")
    assert_equal "unavailable", document.dig("error", "code")

    # The CLI must find the daemon through the symlink Homebrew installs it
    # behind. Resolving from the link alone finds an empty bin and reports a
    # correct install as broken.
    assert_predicate bin/"animesh", :symlink? if OS.mac?
  end
end
