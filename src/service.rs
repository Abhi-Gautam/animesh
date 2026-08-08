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

/// What one action did, phrased for a person reading a terminal.
pub type Outcome = Result<String, AppError>;

fn failed(message: impl Into<String>) -> AppError {
    AppError::new(ErrorCode::Internal, message)
}

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
    let cli = std::env::current_exe()
        .map_err(|e| failed(format!("cannot locate the running executable: {e}")))?;
    let dir = cli
        .parent()
        .ok_or_else(|| failed("the running executable has no directory"))?;

    let bundled = dir
        .parent()
        .map(|contents| contents.join("MacOS").join("Animesh"));
    if dir.ends_with("Helpers") {
        if let Some(path) = bundled.filter(|p| p.exists()) {
            return Ok(path);
        }
    }

    let sibling = dir.join("animesh-app");
    if sibling.exists() {
        return Ok(sibling);
    }
    Err(failed(format!(
        "cannot find the animesh daemon next to {}",
        cli.display()
    )))
}

fn write_file(path: &Path, contents: &str) -> Result<(), AppError> {
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
    Ok(home()?.join(".config/systemd/user").join(UNIT))
}

#[cfg(not(target_os = "macos"))]
fn unit_contents(daemon: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=Animesh release radar\n\
         Documentation=https://github.com/Abhi-Gautam/animesh\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        daemon.display()
    )
}

/// Installs the desktop entry alongside the unit.
///
/// It carries no launcher value — it exists so GNOME and KDE can match a
/// notification to an application and list animesh in their per-app
/// notification settings. Without it the owner cannot mute animesh short of
/// stopping the daemon, so it is installed by the same action that starts it.
#[cfg(not(target_os = "macos"))]
fn install_desktop_entry() -> Result<(), AppError> {
    let path = home()?
        .join(".local/share/applications")
        .join("animesh.desktop");
    write_file(&path, include_str!("../assets/animesh.desktop"))
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
    install_desktop_entry()?;
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
    let daemon = daemon_path()?;
    let path = unit_path()?;
    write_file(&path, &unit_contents(&daemon))?;
    register(&path)?;

    let mut lines =
        vec!["Animesh is running in the background and will start at login.".to_owned()];

    #[cfg(not(target_os = "macos"))]
    if !enable_linger() {
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
