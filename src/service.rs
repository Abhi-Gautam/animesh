//! Registering the daemon with the system's service manager.
//!
//! Installation starts the daemon, so these are repair tools rather than steps
//! anyone is told about: `status` when something looks wrong, `restart` after an
//! upgrade, `stop` to turn animesh off without uninstalling it. Nothing here
//! touches the database or the socket — that is the daemon's job, and this only
//! decides whether the daemon exists.
//!
//! Both platforms write a unit file describing how to launch the binary that is
//! running right now, rather than one baked at build time. A `brew upgrade`
//! moves the prefix; a unit pinned to yesterday's path silently stops working.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{AppError, ErrorCode};

/// launchd's label and systemd's unit name. Both are the handle the manager
/// knows the service by, so changing either orphans an installed unit.
#[cfg(target_os = "macos")]
pub const LABEL: &str = "dev.animesh.agent";

#[cfg(not(target_os = "macos"))]
pub const UNIT: &str = "animesh.service";

/// The daemon's filename inside the bundle. Must match what xtask writes to
/// `Contents/MacOS`, or an installed app cannot start its own daemon.
const APP_EXECUTABLE: &str = "Animesh";

/// What one action did, phrased for a person reading a terminal.
pub type Outcome = Result<String, AppError>;

fn failed(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Internal, message)
}

#[cfg(any(target_os = "macos", test))]
fn home() -> Result<PathBuf, AppError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| failed("HOME is unset, so there is no user service directory"))
}

/// The daemon executable, resolved from the CLI that is running.
///
/// Never a compiled-in path. Inside a macOS bundle the CLI sits in
/// `Contents/Helpers` and the daemon in `Contents/MacOS`; everywhere else they
/// are siblings, which covers a Homebrew prefix and `target/debug` alike.
fn daemon_path() -> Result<PathBuf, AppError> {
    let launched = std::env::current_exe()
        .map_err(|e| failed(format!("cannot locate the running executable: {e}")))?;
    resolve_daemon(&launched).ok_or_else(|| {
        failed(format!(
            "cannot find the animesh daemon next to {}",
            launched.display()
        ))
    })
}

/// Locates the daemon relative to the CLI that is running.
///
/// `launched` is the path the process was started from, which is not
/// necessarily the binary: every install path puts a symlink in front of the
/// CLI — `~/.local/bin/animesh` into the app bundle, and Homebrew's `bin` into
/// its Cellar — and macOS reports the link, not its target. Resolving it first
/// is what makes the daemon findable at all; searching next to the link finds
/// an empty `bin` directory and reports the daemon missing on a correct
/// install.
fn resolve_daemon(launched: &Path) -> Option<PathBuf> {
    let cli = std::fs::canonicalize(launched).unwrap_or_else(|_| launched.to_path_buf());
    let dir = cli.parent()?;

    if dir.ends_with("Helpers") {
        let bundled = dir
            .parent()
            .map(|contents| contents.join("MacOS").join(APP_EXECUTABLE));
        if let Some(path) = bundled.filter(|p| p.exists()) {
            return Some(path);
        }
    }

    // The desktop is nested in the daemon's macOS bundle. Service recovery
    // from that window must register the parent engine, never the webview.
    if dir.ends_with("Animesh Desktop.app/Contents/MacOS") {
        let helpers = dir.ancestors().nth(3)?;
        let bundled = helpers.parent()?.join("MacOS").join(APP_EXECUTABLE);
        if bundled.exists() {
            return Some(bundled);
        }
    }

    let sibling = dir.join("animesh-app");
    sibling.exists().then_some(sibling)
}

fn write_file(path: &Path, contents: &str) -> Result<(), AppError> {
    write_bytes(path, contents.as_bytes())
}

fn write_bytes(path: &Path, contents: &[u8]) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| failed(format!("creating {}: {e}", parent.display())))?;
    }
    std::fs::write(path, contents).map_err(|e| failed(format!("writing {}: {e}", path.display())))
}

/// Runs a service-manager command, returning its stdout.
fn run(program: &str, args: &[&str]) -> Result<String, AppError> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| failed(format!("cannot run {program}: {e}")))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned());
    }
    Err(failed(format!(
        "{program} {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// Whether the manager already knows about the service.
fn is_registered() -> bool {
    #[cfg(target_os = "macos")]
    {
        run("launchctl", &["print", &format!("gui/{}/{LABEL}", uid())]).is_ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        run("systemctl", &["--user", "is-enabled", UNIT])
            .map(|state| state == "enabled")
            .unwrap_or(false)
    }
}

#[cfg(target_os = "macos")]
fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

// ---------------------------------------------------------------------------
// macOS — launchd
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn unit_path() -> Result<PathBuf, AppError> {
    Ok(home()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

/// `KeepAlive` restarts on crash as well as at login, which is the single
/// registration that makes a separate supervisor unnecessary.
#[cfg(target_os = "macos")]
fn unit_contents(daemon: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Background</string>
</dict>
</plist>
"#,
        daemon.display()
    )
}

#[cfg(target_os = "macos")]
fn register(path: &Path) -> Result<(), AppError> {
    // Bootstrapping over a live service fails, so the old one is evicted first.
    // A service that was not loaded makes bootout fail, which is the normal
    // case on a first install and is not worth reporting.
    let _ = run("launchctl", &["bootout", &format!("gui/{}/{LABEL}", uid())]);
    run(
        "launchctl",
        &[
            "bootstrap",
            &format!("gui/{}", uid()),
            &path.to_string_lossy(),
        ],
    )
    .map(|_| ())
}

#[cfg(target_os = "macos")]
fn unregister() -> Result<(), AppError> {
    run("launchctl", &["bootout", &format!("gui/{}/{LABEL}", uid())]).map(|_| ())
}

// ---------------------------------------------------------------------------
// Linux — systemd user units
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
fn unit_path() -> Result<PathBuf, AppError> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| failed("cannot locate the user service directory"))?;
    Ok(base.config_dir().join("systemd/user").join(UNIT))
}

#[cfg(not(target_os = "macos"))]
fn unit_contents(daemon: &Path) -> String {
    let executable = daemon
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!(
        "[Unit]\n\
         Description=Animesh release radar\n\
         Documentation=https://github.com/Abhi-Gautam/animesh\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart=\"{}\"\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        executable
    )
}

/// Installs the desktop entry alongside the unit.
///
/// It opens the desktop when installed, and the CLI otherwise. Its basename
/// also lets GNOME and KDE match notifications to their per-app settings.
#[cfg(not(target_os = "macos"))]
fn install_desktop_entry(daemon: &Path) -> Result<(), AppError> {
    let base = directories::BaseDirs::new()
        .ok_or_else(|| failed("cannot locate the user application directory"))?;
    let share = base.data_dir();
    write_file(
        &share.join("applications/animesh.desktop"),
        &desktop_entry(daemon),
    )?;
    install_hicolor(share)
}

#[cfg(not(target_os = "macos"))]
fn desktop_entry(daemon: &Path) -> String {
    let desktop = daemon.with_file_name("animesh-desktop");
    let (template, executable, command) = if desktop.is_file() {
        (
            include_str!("../assets/animesh.desktop"),
            desktop,
            "animesh-desktop",
        )
    } else {
        (
            include_str!("../assets/animesh-cli.desktop"),
            daemon.with_file_name("animesh"),
            "animesh",
        )
    };
    // Exec is not a shell command. Escape reserved characters inside the
    // quoted executable, including literal percent signs (field codes).
    let executable = executable
        .to_string_lossy()
        .replace('\\', "\\\\\\\\")
        .replace('"', "\\\\\"")
        .replace('$', "\\\\$")
        .replace('`', "\\\\`")
        .replace('%', "%%");
    template.replace(
        &format!("Exec={command}"),
        &format!("Exec=\"{executable}\""),
    )
}

/// hicolor PNGs that `Icon=animesh` and the Notify `app_icon` both name.
#[cfg(not(target_os = "macos"))]
const HICOLOR_ICONS: &[(&str, &[u8])] = &[
    (
        "16x16",
        include_bytes!("../assets/icons/hicolor/16x16/apps/animesh.png"),
    ),
    (
        "24x24",
        include_bytes!("../assets/icons/hicolor/24x24/apps/animesh.png"),
    ),
    (
        "32x32",
        include_bytes!("../assets/icons/hicolor/32x32/apps/animesh.png"),
    ),
    (
        "48x48",
        include_bytes!("../assets/icons/hicolor/48x48/apps/animesh.png"),
    ),
    (
        "256x256",
        include_bytes!("../assets/icons/hicolor/256x256/apps/animesh.png"),
    ),
    (
        "512x512",
        include_bytes!("../assets/icons/hicolor/512x512/apps/animesh.png"),
    ),
];

#[cfg(not(target_os = "macos"))]
fn install_hicolor(data_home: &Path) -> Result<(), AppError> {
    for (size, bytes) in HICOLOR_ICONS {
        write_bytes(
            &data_home.join(format!("icons/hicolor/{size}/apps/animesh.png")),
            bytes,
        )?;
    }
    Ok(())
}

/// Whether the user's services keep running when they are not logged in.
///
/// Without lingering a systemd user unit dies at logout and does not come back
/// until the next graphical login — so a machine that reboots overnight would
/// miss every airtime until someone sat down at it. Tailscale sidesteps this by
/// running a system daemon; animesh holds one person's library and has no
/// business running as root, so it asks for lingering instead.
#[cfg(not(target_os = "macos"))]
fn enable_linger() -> bool {
    let Ok(user) = std::env::var("USER") else {
        return false;
    };
    run("loginctl", &["enable-linger", &user]).is_ok()
}

#[cfg(not(target_os = "macos"))]
fn register(_path: &Path) -> Result<(), AppError> {
    install_desktop_entry(&daemon_path()?)?;
    run("systemctl", &["--user", "daemon-reload"])?;
    run("systemctl", &["--user", "enable", "--now", UNIT])?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn unregister() -> Result<(), AppError> {
    run("systemctl", &["--user", "disable", "--now", UNIT]).map(|_| ())
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Registers the daemon and starts it. Idempotent.
pub fn start() -> Outcome {
    start_inner(true)
}

/// Desktop startup needs the user's login service, without asking for the
/// administrative policy that keeps a CLI user's jobs alive after logout.
pub fn start_from_desktop(restart: bool) -> Outcome {
    if restart {
        let _ = unregister();
    }
    start_inner(false)
}

fn start_inner(linger: bool) -> Outcome {
    let daemon = daemon_path()?;
    let path = unit_path()?;
    write_file(&path, &unit_contents(&daemon))?;
    register(&path)?;

    let mut lines =
        vec!["Animesh is running in the background and will start at login.".to_owned()];

    #[cfg(not(target_os = "macos"))]
    if linger && !enable_linger() {
        // Not fatal, and not something to fail the install over — but the owner
        // has to know, because the symptom is silent: no banners after a reboot
        // until they happen to log in graphically.
        lines.push(String::new());
        lines.push(
            "Note: your user services stop when you log out, so Animesh will not \n\
             run after a reboot until you log in. To keep it running:\n\
             \n\
             \tsudo loginctl enable-linger $USER"
                .to_owned(),
        );
    }

    #[cfg(target_os = "macos")]
    let _ = linger;

    lines.push(String::new());
    lines.push(format!("Service file: {}", path.display()));
    lines.push("Next: animesh search \"<title>\" — then animesh follow <id>".to_owned());
    Ok(lines.join("\n"))
}

/// Stops the daemon and unregisters it, leaving the database untouched.
pub fn stop() -> Outcome {
    if !is_registered() {
        return Ok("Animesh is not registered as a service.".to_owned());
    }
    unregister()?;
    Ok(
        "Stopped Animesh. Your library is untouched; 'animesh service start' brings it back."
            .to_owned(),
    )
}

pub fn restart() -> Outcome {
    let _ = unregister();
    start().map(|_| "Restarted Animesh.".to_owned())
}

/// Reports registration and reachability separately.
///
/// They fail independently and have different fixes: a service that is
/// registered but unreachable is a daemon that crashed on startup, which
/// `animesh status` explains and `service start` will not.
pub fn status() -> Outcome {
    let registered = is_registered();
    let where_from = unit_path()?;

    Ok(if registered {
        format!(
            "Registered and managed by the system.\nService file: {}\nRun 'animesh status' for what it is doing.",
            where_from.display()
        )
    } else {
        "Not registered. Run 'animesh service start' to run Animesh in the background.".to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a file at `path`, creating its parents.
    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        std::fs::write(path, b"").expect("write");
    }

    #[cfg(unix)]
    fn link(target: &Path, at: &Path) {
        std::fs::create_dir_all(at.parent().expect("parent")).expect("create dirs");
        std::os::unix::fs::symlink(target, at).expect("symlink");
    }

    #[cfg(unix)]
    #[test]
    fn the_daemon_is_found_through_the_bundle_symlink() {
        // `cargo xtask install` links ~/.local/bin/animesh into the bundle, and
        // macOS reports the link as the running executable. Resolving from the
        // link alone finds an empty bin directory, which is the failure this
        // guards: a correct install reporting the daemon missing.
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let contents = root.join("Animesh.app/Contents");
        let cli = contents.join("Helpers/animesh");
        let daemon = contents.join("MacOS").join(APP_EXECUTABLE);
        touch(&cli);
        touch(&daemon);

        let linked = root.join("bin/animesh");
        link(&cli, &linked);

        assert_eq!(
            resolve_daemon(&linked).map(|p| std::fs::canonicalize(p).expect("canonicalize")),
            Some(std::fs::canonicalize(&daemon).expect("canonicalize"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_daemon_is_found_through_the_homebrew_symlink() {
        // Homebrew's bin is symlinks into the Cellar, so every brew install
        // reaches `service start` through one.
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let cellar = root.join("Cellar/animesh/0.6.0/bin");
        let cli = cellar.join("animesh");
        let daemon = cellar.join("animesh-app");
        touch(&cli);
        touch(&daemon);

        let linked = root.join("bin/animesh");
        link(&cli, &linked);

        assert_eq!(
            resolve_daemon(&linked).map(|p| std::fs::canonicalize(p).expect("canonicalize")),
            Some(std::fs::canonicalize(&daemon).expect("canonicalize"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_daemon_is_found_through_two_hops_on_a_homebrew_mac() {
        // The shape that actually ships on macOS: the formula installs the
        // bundle into the Cellar and symlinks the CLI out of it, then Homebrew
        // symlinks *that* into its bin. Resolving one hop lands in the Cellar's
        // bin, which holds no daemon.
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        let keg = root.join("Cellar/animesh/0.6.0");
        let contents = keg.join("Animesh.app/Contents");
        let cli = contents.join("Helpers/animesh");
        let daemon = contents.join("MacOS").join(APP_EXECUTABLE);
        touch(&cli);
        touch(&daemon);

        let keg_link = keg.join("bin/animesh");
        link(&cli, &keg_link);
        let prefix_link = root.join("bin/animesh");
        link(&keg_link, &prefix_link);

        assert_eq!(
            resolve_daemon(&prefix_link).map(|p| std::fs::canonicalize(p).expect("canonicalize")),
            Some(std::fs::canonicalize(&daemon).expect("canonicalize"))
        );
    }

    #[test]
    fn the_daemon_is_found_beside_an_unlinked_cli() {
        // `target/release/animesh` from a checkout, with no link involved.
        let temp = tempfile::tempdir().expect("tempdir");
        let cli = temp.path().join("animesh");
        let daemon = temp.path().join("animesh-app");
        touch(&cli);
        touch(&daemon);

        assert_eq!(
            resolve_daemon(&cli).map(|p| std::fs::canonicalize(p).expect("canonicalize")),
            Some(std::fs::canonicalize(&daemon).expect("canonicalize"))
        );
    }

    #[test]
    fn a_cli_with_no_daemon_beside_it_resolves_to_nothing() {
        // A partial install must be reported, never guessed at.
        let temp = tempfile::tempdir().expect("tempdir");
        let cli = temp.path().join("animesh");
        touch(&cli);
        assert_eq!(resolve_daemon(&cli), None);
    }

    #[test]
    fn desktop_recovery_finds_the_parent_mac_engine() {
        let temp = tempfile::tempdir().expect("tempdir");
        let contents = temp.path().join("Animesh.app/Contents");
        let desktop = contents.join("Helpers/Animesh Desktop.app/Contents/MacOS/animesh-desktop");
        let daemon = contents.join("MacOS/Animesh");
        touch(&desktop);
        touch(&daemon);
        assert_eq!(
            resolve_daemon(&desktop),
            Some(std::fs::canonicalize(daemon).expect("canonicalize"))
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn desktop_launcher_uses_the_installed_gui_even_outside_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let daemon = temp.path().join("My apps/100%/animesh-app");
        let desktop = daemon.with_file_name("animesh-desktop");
        touch(&desktop);
        let entry = desktop_entry(&daemon);
        assert!(entry.contains("Terminal=false"));
        assert!(entry.contains(&format!(
            "Exec=\"{}\"",
            desktop.to_string_lossy().replace('%', "%%")
        )));
        assert!(!entry.contains("animesh next"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn cli_only_installs_keep_a_working_terminal_launcher() {
        let temp = tempfile::tempdir().expect("tempdir");
        let daemon = temp.path().join("animesh-app");
        let entry = desktop_entry(&daemon);
        assert!(entry.contains("Terminal=true"));
        assert!(entry.contains(&format!(
            "Exec=\"{}\" next",
            temp.path().join("animesh").display()
        )));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn systemd_exec_handles_spaces_and_literal_specifiers() {
        let unit = unit_contents(Path::new("/home/person/My apps/100%/animesh-app"));
        assert!(unit.contains("ExecStart=\"/home/person/My apps/100%%/animesh-app\""));
    }

    #[test]
    fn the_unit_launches_the_binary_that_wrote_it() {
        // A path baked at build time breaks the moment a brew upgrade moves the
        // prefix, and does so silently.
        let unit = unit_contents(Path::new("/opt/homebrew/bin/animesh-app"));
        assert!(
            unit.contains("/opt/homebrew/bin/animesh-app"),
            "unit does not name the daemon:\n{unit}"
        );
    }

    #[test]
    fn the_unit_restarts_the_daemon_rather_than_giving_up() {
        // The whole point of registering: a crash must not need a human.
        let unit = unit_contents(Path::new("/usr/local/bin/animesh-app"));
        let restarts = unit.contains("KeepAlive") || unit.contains("Restart=on-failure");
        assert!(restarts, "unit does not restart on failure:\n{unit}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn hicolor_icons_are_written_under_data_home() {
        let temp = tempfile::tempdir().expect("tempdir");
        install_hicolor(temp.path()).expect("install");
        for size in ["16x16", "24x24", "32x32", "48x48", "256x256", "512x512"] {
            let path = temp
                .path()
                .join(format!("icons/hicolor/{size}/apps/animesh.png"));
            assert!(path.exists(), "missing {size}");
            let bytes = std::fs::read(&path).expect("read");
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{size} is not a PNG");
        }
    }

    #[test]
    fn the_unit_lands_under_the_users_own_directory() {
        // A user agent, never a system daemon: animesh holds one person's
        // library and has no business running as root.
        let path = unit_path().expect("unit path");
        let home = home().expect("home");
        assert!(
            path.starts_with(&home),
            "unit path {} is outside {}",
            path.display(),
            home.display()
        );
    }
}
