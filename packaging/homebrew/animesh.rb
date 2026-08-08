# Homebrew formula for animesh.
#
# Lives here so it is versioned with the code that it installs; the tap
# (Abhi-Gattam/homebrew-animesh) carries a copy. A formula rather than a cask,
# for two reasons. Casks download an artifact and Gatekeeper quarantines it, so
# an unnotarised app would refuse to launch on every machine but the author's —
# a formula's files are not quarantined, so v1 ships without notarisation.
# Second, the product on Linux is a CLI and a daemon, which is what a formula is
# for; the cask can come later on macOS once there is a Developer ID.
#
# head-only until the first tag. `url` and `sha256` land with v1; nothing here
# can be pinned to an archive that does not exist yet.
class Animesh < Formula
  desc "Personal release radar for anime and other scheduled media"
  homepage "https://github.com/Abhi-Gautam/animesh"
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
  end
end
