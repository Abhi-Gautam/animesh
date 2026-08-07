//! Build tooling. Never shipped as product code.
//!
//! Assembles `Animesh.app` from `assets/` and the two cargo binaries, signs it,
//! installs it, and verifies the result. Everything is done with explicit file
//! operations rather than a shell script inside the bundle, because a signed
//! bundle's contents are immutable and a script that rewrites them at runtime
//! would invalidate the signature.

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};

const APP_NAME: &str = "Animesh";
const BUNDLE_ID: &str = "dev.animesh.app";
const AGENT_LABEL: &str = "dev.animesh.agent";
const AGENT_PLIST: &str = "dev.animesh.agent.plist";
const CLI_NAME: &str = "animesh";
const APP_BIN: &str = "animesh-app";
const MIN_MACOS: &str = "13.0";
const INSTALL_DIR: &str = "/Applications";

/// Set to a Developer ID or Apple Development identity for dogfooding.
///
/// Ad-hoc signatures are erased by any move and do not persist TCC consent, so
/// a notification permission granted under one would be re-prompted on every
/// reinstall. CI smoke is the only place ad-hoc is acceptable.
const IDENTITY_ENV: &str = "ANIMESH_CODESIGN_IDENTITY";

#[derive(Parser)]
#[command(name = "xtask", about = "Build, sign, install, and verify Animesh.app")]
struct Cli {
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Assemble and sign the bundle under target/bundle.
    Bundle {
        #[arg(long)]
        release: bool,
    },
    /// Bundle, then replace the installed app and link the CLI.
    Install {
        #[arg(long)]
        release: bool,
    },
    /// Check a bundle's structure, signature, and versions.
    Verify {
        /// Defaults to the installed app. Point it at target/bundle to gate a
        /// build before it replaces anything.
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Remove the app and the CLI link. The database is preserved by default.
    Uninstall {
        /// Also delete the database and logs. Requires typing the app name.
        #[arg(long)]
        purge: bool,
    },
}

type Fallible = Result<(), String>;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Task::Bundle { release } => bundle(release).map(|_| ()),
        Task::Install { release } => install(release),
        Task::Verify { path } => verify(&path.unwrap_or_else(installed_app)),
        Task::Uninstall { purge } => uninstall(purge),
    };

    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            std::process::ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------------------
// paths
// ---------------------------------------------------------------------------

fn workspace_root() -> PathBuf {
    // The manifest directory is xtask/, so the workspace is its parent. This
    // holds no matter which directory cargo was invoked from.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf()
}

fn installed_app() -> PathBuf {
    Path::new(INSTALL_DIR).join(format!("{APP_NAME}.app"))
}

fn staging_app() -> PathBuf {
    workspace_root()
        .join("target/bundle")
        .join(format!("{APP_NAME}.app"))
}

/// The version in the root manifest, which every bundled plist must match.
fn crate_version() -> Result<String, String> {
    let manifest = std::fs::read_to_string(workspace_root().join("Cargo.toml"))
        .map_err(|e| format!("cannot read the root Cargo.toml: {e}"))?;

    manifest
        .lines()
        .skip_while(|line| line.trim() != "[package]")
        .find_map(|line| line.strip_prefix("version = "))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .ok_or_else(|| "no version in the [package] section".to_owned())
}

// ---------------------------------------------------------------------------
// process helpers
// ---------------------------------------------------------------------------

fn run(program: &str, args: &[&str]) -> Fallible {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() {
        return Ok(());
    }
    Err(format!("{program} {} failed", args.join(" ")))
}

fn output(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// bundle
// ---------------------------------------------------------------------------

fn bundle(release: bool) -> Result<PathBuf, String> {
    let root = workspace_root();
    let version = crate_version()?;
    let profile = if release { "release" } else { "debug" };

    let mut build = vec!["build", "--bin", CLI_NAME, "--bin", APP_BIN];
    if release {
        build.push("--release");
    }
    // --locked so a bundle is reproducible from the committed lockfile rather
    // than from whatever the resolver picks today.
    build.push("--locked");
    run("cargo", &build)?;

    let app = staging_app();
    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| format!("cannot clear staging: {e}"))?;
    }

    let contents = app.join("Contents");
    for dir in ["MacOS", "Helpers", "Library/LaunchAgents", "Resources"] {
        std::fs::create_dir_all(contents.join(dir))
            .map_err(|e| format!("cannot create {dir}: {e}"))?;
    }

    let built = root.join("target").join(profile);
    copy(&built.join(APP_BIN), &contents.join("MacOS").join(APP_NAME))?;
    copy(
        &built.join(CLI_NAME),
        &contents.join("Helpers").join(CLI_NAME),
    )?;

    let info = std::fs::read_to_string(root.join("assets/Info.plist"))
        .map_err(|e| format!("cannot read Info.plist: {e}"))?
        .replace("__VERSION__", &version);
    std::fs::write(contents.join("Info.plist"), info)
        .map_err(|e| format!("cannot write Info.plist: {e}"))?;

    copy(
        &root.join("assets").join(AGENT_PLIST),
        &contents.join("Library/LaunchAgents").join(AGENT_PLIST),
    )?;

    let icon = root.join("assets/AppIcon.icns");
    if icon.exists() {
        copy(&icon, &contents.join("Resources/AppIcon.icns"))?;
    } else {
        eprintln!("xtask: no assets/AppIcon.icns; the bundle will use the generic icon");
    }

    sign(&app)?;
    println!("bundled {} {version} at {}", APP_NAME, app.display());
    Ok(app)
}

fn copy(from: &Path, to: &Path) -> Fallible {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))
}

/// Signs helpers before the app.
///
/// The outer signature covers the nested ones, so signing the app first would
/// be invalidated the moment a helper was signed afterwards.
fn sign(app: &Path) -> Fallible {
    let identity = std::env::var(IDENTITY_ENV).unwrap_or_else(|_| {
        eprintln!(
            "xtask: {IDENTITY_ENV} is unset; signing ad-hoc. Notification consent \
             will be re-prompted on every reinstall."
        );
        "-".to_owned()
    });

    let helper = app.join("Contents/Helpers").join(CLI_NAME);
    let helper = helper.to_string_lossy().into_owned();
    let app_path = app.to_string_lossy().into_owned();

    run(
        "codesign",
        &["--force", "--sign", &identity, "--timestamp=none", &helper],
    )?;
    run(
        "codesign",
        &[
            "--force",
            "--sign",
            &identity,
            "--timestamp=none",
            "--options=runtime",
            &app_path,
        ],
    )
}

// ---------------------------------------------------------------------------
// install
// ---------------------------------------------------------------------------

fn install(release: bool) -> Fallible {
    let staged = bundle(release)?;
    let target = installed_app();

    stop_installed_app(&target);

    if target.exists() {
        std::fs::remove_dir_all(&target)
            .map_err(|e| format!("cannot remove the installed app: {e}"))?;
    }
    run(
        "cp",
        &["-R", &staged.to_string_lossy(), &target.to_string_lossy()],
    )?;

    link_cli(&target)?;
    verify(&target)?;

    println!("installed {}", target.display());
    println!("launch it once so it can request notification permission:");
    println!("  open {}", target.display());
    Ok(())
}

/// Asks a running instance to exit before its bundle is replaced.
///
/// Best effort: replacing the bundle under a live process leaves it running
/// from an unlinked inode, which is confusing rather than dangerous.
///
/// Agent unregistration is deliberately not attempted here. `SMAppService`
/// calls must come from inside the signed bundle, so it is the installed app's
/// own job — and it has no such code until V4 links the Apple frameworks.
fn stop_installed_app(target: &Path) {
    if !target.exists() {
        return;
    }
    let executable = target.join("Contents/MacOS").join(APP_NAME);
    let _ = Command::new("pkill")
        .args(["-f", &executable.to_string_lossy()])
        .status();
}

fn link_cli(app: &Path) -> Fallible {
    let Some(home) = std::env::var_os("HOME") else {
        return Err("HOME is unset; cannot link the CLI".to_owned());
    };
    let bin_dir = Path::new(&home).join(".local/bin");
    std::fs::create_dir_all(&bin_dir).map_err(|e| format!("cannot create {bin_dir:?}: {e}"))?;

    let link = bin_dir.join(CLI_NAME);
    if link.exists() || link.symlink_metadata().is_ok() {
        std::fs::remove_file(&link).map_err(|e| format!("cannot replace the CLI link: {e}"))?;
    }
    std::os::unix::fs::symlink(app.join("Contents/Helpers").join(CLI_NAME), &link)
        .map_err(|e| format!("cannot link the CLI: {e}"))?;

    println!("linked {}", link.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

fn verify(app: &Path) -> Fallible {
    if !app.exists() {
        return Err(format!("{} is not installed", app.display()));
    }
    let contents = app.join("Contents");

    for required in [
        PathBuf::from("Info.plist"),
        PathBuf::from("MacOS").join(APP_NAME),
        PathBuf::from("Helpers").join(CLI_NAME),
        PathBuf::from("Library/LaunchAgents").join(AGENT_PLIST),
    ] {
        if !contents.join(&required).exists() {
            return Err(format!("missing {}", required.display()));
        }
    }

    let expected = crate_version()?;
    for key in ["CFBundleShortVersionString", "CFBundleVersion"] {
        let found = plist_value(&contents.join("Info.plist"), key)?;
        if found != expected {
            return Err(format!("{key} is {found}, expected {expected}"));
        }
    }

    let id = plist_value(&contents.join("Info.plist"), "CFBundleIdentifier")?;
    if id != BUNDLE_ID {
        return Err(format!("bundle id is {id}, expected {BUNDLE_ID}"));
    }
    let ui_element = plist_value(&contents.join("Info.plist"), "LSUIElement")?;
    if ui_element != "true" && ui_element != "1" {
        return Err("LSUIElement is not set; the app would take a Dock tile".to_owned());
    }
    let minimum = plist_value(&contents.join("Info.plist"), "LSMinimumSystemVersion")?;
    if minimum != MIN_MACOS {
        return Err(format!(
            "LSMinimumSystemVersion is {minimum}, expected {MIN_MACOS}"
        ));
    }

    let label = plist_value(
        &contents.join("Library/LaunchAgents").join(AGENT_PLIST),
        "Label",
    )?;
    if label != AGENT_LABEL {
        return Err(format!("agent label is {label}, expected {AGENT_LABEL}"));
    }

    // --deep so the nested helper signature is checked too; a valid outer
    // signature over a broken inner one still fails to launch.
    run(
        "codesign",
        &["--verify", "--deep", "--strict", &app.to_string_lossy()],
    )?;

    verify_deployment_target(&contents.join("MacOS").join(APP_NAME))?;

    println!("verified {}", app.display());
    Ok(())
}

fn plist_value(plist: &Path, key: &str) -> Result<String, String> {
    output(
        "/usr/libexec/PlistBuddy",
        &["-c", &format!("Print :{key}"), &plist.to_string_lossy()],
    )
    .map(|value| value.trim().to_owned())
    .map_err(|e| format!("{key} is missing from {}: {e}", plist.display()))
}

/// Confirms the binary will actually run on the minimum OS it claims.
///
/// A mismatch here is silent until someone on an older Mac tries to launch it,
/// which for a personal daily driver means finding out on the machine that
/// matters least often.
fn verify_deployment_target(binary: &Path) -> Fallible {
    let load_commands = output("otool", &["-l", &binary.to_string_lossy()])?;
    let minos = load_commands
        .lines()
        .find_map(|line| line.trim().strip_prefix("minos "))
        .map(str::trim)
        .ok_or_else(|| "no LC_BUILD_VERSION minos in the app binary".to_owned())?;

    if minos != MIN_MACOS {
        return Err(format!(
            "binary minos is {minos} but Info.plist claims {MIN_MACOS}; \
             MACOSX_DEPLOYMENT_TARGET={MIN_MACOS} is set in .cargo/config.toml, \
             so a mismatch here means the build did not pick it up"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// uninstall
// ---------------------------------------------------------------------------

fn uninstall(purge: bool) -> Fallible {
    let app = installed_app();
    stop_installed_app(&app);

    if app.exists() {
        std::fs::remove_dir_all(&app).map_err(|e| format!("cannot remove the app: {e}"))?;
        println!("removed {}", app.display());
    }

    if let Some(home) = std::env::var_os("HOME") {
        let link = Path::new(&home).join(".local/bin").join(CLI_NAME);
        if link.symlink_metadata().is_ok() {
            std::fs::remove_file(&link).map_err(|e| format!("cannot remove the CLI link: {e}"))?;
            println!("removed {}", link.display());
        }
    }

    if !purge {
        println!("the database was preserved; pass --purge to delete it");
        return Ok(());
    }
    purge_data()
}

/// Deletes the database and logs, behind a typed confirmation.
///
/// This is the only irreversible thing xtask does: the follow graph is the
/// user's own data and there is no backup.
fn purge_data() -> Fallible {
    let Some(home) = std::env::var_os("HOME") else {
        return Err("HOME is unset; cannot locate the data directory".to_owned());
    };
    // These must track src/paths.rs. The database deliberately lives outside
    // the bundle, so uninstalling the app never touches it by accident.
    let data = Path::new(&home)
        .join("Library/Application Support")
        .join(APP_NAME);
    let logs = Path::new(&home).join("Library/Logs").join(APP_NAME);

    if !data.exists() && !logs.exists() {
        println!("no data directory at {}", data.display());
        return Ok(());
    }

    println!("This deletes {} and every follow in it.", data.display());
    println!("Type {APP_NAME} to confirm:");
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|e| format!("cannot read the confirmation: {e}"))?;

    if answer.trim() != APP_NAME {
        return Err("not confirmed; nothing was deleted".to_owned());
    }
    for path in [&data, &logs] {
        if path.exists() {
            std::fs::remove_dir_all(path)
                .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
            println!("purged {}", path.display());
        }
    }
    Ok(())
}
