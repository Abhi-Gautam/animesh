# typed: false
# frozen_string_literal: true

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
# The cask can come later on macOS once there is a Developer ID.
#
# Builds from the tagged source archive rather than shipping a prebuilt binary.
# On macOS the daemon has to live in an app bundle that is signed on the machine
# it runs on, and a formula that compiles is a shorter path to that than one
# that downloads, re-signs and hopes. The release also publishes prebuilt
# tarballs for people who would rather not compile.
class Animesh < Formula
  desc "Personal release radar for anime and other scheduled media"
  homepage "https://animesh.syntropicsystems.dev/"
  url "https://github.com/Abhi-Gautam/animesh/archive/refs/tags/v0.7.1.tar.gz"
  sha256 "9a96463a90c2023ed9db5e097a7262a67f1ad3d804821bd3f7fd84ad0a74902c"
  license "MIT"
  head "https://github.com/Abhi-Gautam/animesh.git", branch: "master"

  depends_on "rust" => :build
  depends_on "node" => :build

  on_linux do
    depends_on "pkgconf" => :build
    depends_on "desktop-file-utils" => :build
    depends_on "webkitgtk"
    depends_on "gtk+3"
  end

  resource "typescript" do
    url "https://registry.npmjs.org/typescript/-/typescript-5.9.3.tgz"
    sha256 "10e108c9cf7d5f2879053dff18515fb405abf2ccef63eaaf017d9c571687a1d3"
  end

  def install
    # A pinned Homebrew resource keeps the frontend build offline. TypeScript
    # is the only npm dependency; its compiler runs through Node directly.
    (buildpath/"desktop/ui/node_modules/typescript").install resource("typescript")
    (buildpath/"desktop/ui/node_modules/.bin").mkpath
    ln_s "../typescript/bin/tsc", buildpath/"desktop/ui/node_modules/.bin/tsc"
    system "cargo", "xtask", "bundle", "--release"
    system "cargo", "xtask", "verify", "--path", OS.mac? ? "target/bundle/Animesh.app" : "target/bundle/animesh"
    if OS.mac?
      # The daemon has to run from a bundle. UNUserNotificationCenter traps
      # without a bundle identifier, so a bare binary in bin/ could never post
      # a notification — xtask assembles and ad-hoc signs the app, and the CLI
      # is linked out of it.
      prefix.install "target/bundle/Animesh.app"
      bin.install_symlink prefix/"Animesh.app/Contents/Helpers/animesh"
    else
      bin.install Dir["target/bundle/animesh/bin/*"]
      share.install Dir["target/bundle/animesh/share/*"]
    end
  end

  def caveats
    notes = <<~TEXT
      Run Animesh in the background and start it at login:

        animesh service start

      Then find something to follow:

        Open Animesh from the menu bar (macOS) or applications menu (Linux).

      Or use the CLI:

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
    # Every assertion here has to hold whether or not the daemon is running,
    # because `brew test` is run on machines where it is. Asserting "not
    # running" passes in CI and fails for anyone who followed the caveats.

    # The CLI must answer without a daemon, a database, or a desktop session.
    assert_match "animesh", shell_output("#{bin}/animesh --version")

    # Validation happens before the socket is touched, so this exercises the
    # JSON envelope and the exit code without needing to know what is running.
    document = JSON.parse(shell_output("#{bin}/animesh --json next -n 0", 1))
    refute document.fetch("ok")
    assert_equal "invalid_argument", document.dig("error", "code")

    # The agent skill is written from the binary, so this proves the install
    # carries it rather than depending on a file that was never packaged.
    assert_match "skill", shell_output("#{bin}/animesh --json skill status")

    # The CLI must be findable through the symlink Homebrew installs it behind:
    # resolving from the link alone finds an empty bin and reports a correct
    # install as missing its daemon.
    assert_predicate bin/"animesh", :symlink? if OS.mac?

    if OS.mac?
      assert_predicate prefix/"Animesh.app/Contents/Resources/AppIcon.icns", :exist?
      assert_predicate prefix/"Animesh.app/Contents/Helpers/Animesh Desktop.app/Contents/MacOS/animesh-desktop", :exist?
    else
      assert_predicate share/"icons/hicolor/256x256/apps/animesh.png", :exist?
      assert_match "animesh-desktop #{version}", shell_output("#{bin}/animesh-desktop --version")
      assert_match "Terminal=false", (share/"applications/animesh.desktop").read
    end
  end
end
