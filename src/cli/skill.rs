//! Installing the Agent Skill.
//!
//! The skill is how an agent learns that Animesh exists and how to drive it.
//! It is deliberately thin: the CLI's `--json` surface is the contract, so the
//! skill only has to teach an agent which commands exist and what the envelope
//! looks like. That keeps it correct across upgrades without a versioning
//! scheme of its own.
//!
//! `SKILL.md` is compiled into the binary rather than installed alongside it.
//! Every install path — the macOS bundle, a Homebrew Cellar, `cargo run` from a
//! checkout — then behaves identically, and there is no packaging step that can
//! be forgotten and no path to resolve at runtime.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{AppError, ErrorCode};

/// The skill's directory name, which the spec requires to match the `name`
/// field in its frontmatter.
const SKILL_NAME: &str = "animesh";

/// The skill itself.
const SKILL_MD: &str = include_str!("../../skills/animesh/SKILL.md");

/// One place a skill can be installed.
struct Target {
    root: PathBuf,
    /// Written unconditionally, or only when the directory already exists.
    ///
    /// The vendor-neutral location is created if missing, because that is what
    /// installing a skill means. A tool-specific one is not: creating
    /// `~/.claude` for someone who does not run Claude Code would be writing
    /// configuration for software that is not installed.
    always: bool,
    label: &'static str,
}

impl Target {
    fn skill_dir(&self) -> PathBuf {
        self.root.join(SKILL_NAME)
    }

    fn file(&self) -> PathBuf {
        self.skill_dir().join("SKILL.md")
    }

    fn applies(&self) -> bool {
        // The parent is the tool's own directory: ~/.agents or ~/.claude.
        self.always || self.root.parent().is_some_and(|p| p.exists())
    }
}

/// Every location the skill is installed to, in order of precedence.
fn targets(home: &Path) -> Vec<Target> {
    vec![
        // Shared discovery directory for hosts that support ~/.agents/skills.
        Target {
            root: home.join(".agents").join("skills"),
            always: true,
            label: "agents",
        },
        // The same standard skill in Claude Code's personal discovery directory.
        Target {
            root: home.join(".claude").join("skills"),
            always: false,
            label: "claude",
        },
    ]
}

fn home() -> Result<PathBuf, AppError> {
    #[cfg(feature = "test-harness")]
    if let Some(root) = std::env::var_os("ANIMESH_SKILL_HOME") {
        return Ok(PathBuf::from(root));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::internal("HOME is unset, so there is no place to install a skill"))
}

fn io(action: &str, path: &Path, error: std::io::Error) -> AppError {
    AppError::internal(format!("{action} {}: {error}", path.display()))
}

/// What is on disk at one target right now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Installed {
    Absent,
    Current,
    /// Present, but not what this build would write.
    Edited,
}

/// A standard skill file and its actual state on disk.
#[derive(Debug, Serialize)]
pub struct SkillLocation {
    pub path: PathBuf,
    pub state: Installed,
}

/// Applicable discovery locations, independent of daemon availability.
#[derive(Debug, Serialize)]
pub struct SkillStatus {
    pub locations: Vec<SkillLocation>,
}

pub fn installation_status() -> Result<SkillStatus, AppError> {
    installation_status_in(&home()?)
}

fn installation_status_in(home: &Path) -> Result<SkillStatus, AppError> {
    let mut locations = Vec::new();
    for target in targets(home) {
        let state = inspect(&target)?;
        if target.applies() || !matches!(state, Installed::Absent) {
            locations.push(SkillLocation {
                path: target.file(),
                state,
            });
        }
    }
    Ok(SkillStatus { locations })
}

fn inspect(target: &Target) -> Result<Installed, AppError> {
    let file = target.file();
    match std::fs::read_to_string(&file) {
        Ok(contents) if contents == SKILL_MD => Ok(Installed::Current),
        Ok(_) => Ok(Installed::Edited),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Installed::Absent),
        Err(e) => Err(io("reading", &file, e)),
    }
}

pub fn install(force: bool) -> Result<String, AppError> {
    install_into(&home()?, force)
}

fn install_into(home: &Path, force: bool) -> Result<String, AppError> {
    let targets: Vec<Target> = targets(home).into_iter().filter(Target::applies).collect();

    // Refuse before writing anything, so a conflict never leaves half the
    // locations upgraded and half not.
    if !force {
        let edited: Vec<PathBuf> = targets
            .iter()
            .filter(|t| matches!(inspect(t), Ok(Installed::Edited)))
            .map(Target::file)
            .collect();
        if let Some(first) = edited.first() {
            return Err(AppError::new(
                ErrorCode::InvalidArgument,
                format!(
                    "{} has been edited since it was installed; \
                     re-run with --force to replace it",
                    first.display()
                ),
            ));
        }
    }

    let mut written = Vec::new();
    for target in &targets {
        let dir = target.skill_dir();
        std::fs::create_dir_all(&dir).map_err(|e| io("creating", &dir, e))?;
        let file = target.file();
        std::fs::write(&file, SKILL_MD).map_err(|e| io("writing", &file, e))?;
        written.push(file);
    }

    let mut message = format!("Installed the {SKILL_NAME} skill:\n");
    for file in &written {
        message.push_str(&format!("  {}\n", file.display()));
    }
    message.push_str("\nYour agent can now run Animesh for you. Ask it what is airing this week.");
    Ok(message)
}

pub fn uninstall() -> Result<String, AppError> {
    uninstall_from(&home()?)
}

fn uninstall_from(home: &Path) -> Result<String, AppError> {
    let mut removed = Vec::new();
    for target in targets(home) {
        let dir = target.skill_dir();
        if !dir.exists() {
            continue;
        }
        std::fs::remove_dir_all(&dir).map_err(|e| io("removing", &dir, e))?;
        removed.push(dir);
    }

    if removed.is_empty() {
        return Ok(format!("The {SKILL_NAME} skill is not installed."));
    }
    let mut message = String::from("Removed:\n");
    for dir in &removed {
        message.push_str(&format!("  {}\n", dir.display()));
    }
    Ok(message.trim_end().to_owned())
}

pub fn status() -> Result<String, AppError> {
    status_of(&home()?)
}

fn status_of(home: &Path) -> Result<String, AppError> {
    let mut lines = Vec::new();
    let mut any = false;
    for target in targets(home) {
        let state = match inspect(&target)? {
            Installed::Current => {
                any = true;
                "installed"
            }
            Installed::Edited => {
                any = true;
                "installed, edited since"
            }
            Installed::Absent if target.applies() => "not installed",
            // The tool itself is absent, so this is not a gap to report as one.
            Installed::Absent => continue,
        };
        lines.push(format!(
            "  {:<8} {}  {state}",
            target.label,
            target.file().display()
        ));
    }

    let mut message = if any {
        String::from("Agent skill:\n")
    } else {
        String::from("Agent skill is not installed. Run 'animesh skill install'.\n")
    };
    message.push_str(&lines.join("\n"));
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(dirs: &[&str]) -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        for dir in dirs {
            std::fs::create_dir_all(temp.path().join(dir)).expect("create");
        }
        temp
    }

    #[test]
    fn the_embedded_skill_declares_the_name_of_its_directory() {
        // The spec requires `name` to match the parent directory, and the
        // parent directory is what `install` creates.
        assert!(
            SKILL_MD.contains(&format!("name: {SKILL_NAME}")),
            "SKILL.md does not declare name: {SKILL_NAME}"
        );
    }

    #[test]
    fn the_embedded_skill_starts_with_frontmatter() {
        assert!(SKILL_MD.starts_with("---\n"), "SKILL.md has no frontmatter");
        assert!(
            SKILL_MD.contains("\ndescription:"),
            "SKILL.md has no description"
        );
    }

    #[test]
    fn installing_writes_the_vendor_neutral_location() {
        let home = home_with(&[]);
        install_into(home.path(), false).expect("install");
        let file = home.path().join(".agents/skills/animesh/SKILL.md");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), SKILL_MD);
    }

    #[test]
    fn structured_status_tracks_disk_changes_and_new_discovery_locations() {
        let home = home_with(&[]);
        let missing = installation_status_in(home.path()).expect("status");
        assert_eq!(missing.locations.len(), 1);
        assert!(matches!(missing.locations[0].state, Installed::Absent));

        install_into(home.path(), false).expect("install");
        let installed = installation_status_in(home.path()).expect("status");
        assert!(matches!(installed.locations[0].state, Installed::Current));
        assert_eq!(
            serde_json::to_value(&installed).expect("serialize")["locations"][0]["state"],
            "current"
        );

        std::fs::create_dir(home.path().join(".claude")).expect("new assistant");
        let partial = installation_status_in(home.path()).expect("status");
        assert_eq!(partial.locations.len(), 2);
        assert!(matches!(partial.locations[1].state, Installed::Absent));

        install_into(home.path(), false).expect("complete installation");
        std::fs::write(&installed.locations[0].path, "user customization").expect("edit");
        let edited = installation_status_in(home.path()).expect("status");
        assert!(matches!(edited.locations[0].state, Installed::Edited));
        assert!(matches!(edited.locations[1].state, Installed::Current));

        uninstall_from(home.path()).expect("uninstall");
        assert!(installation_status_in(home.path())
            .expect("status")
            .locations
            .iter()
            .all(|location| matches!(location.state, Installed::Absent)));
    }

    #[test]
    fn structured_status_reports_unreadable_files_instead_of_missing_installation() {
        let home = home_with(&[]);
        std::fs::create_dir_all(home.path().join(".agents/skills/animesh/SKILL.md"))
            .expect("directory at file path");
        assert!(installation_status_in(home.path()).is_err());
    }

    #[test]
    fn claude_is_written_only_when_claude_code_is_installed() {
        // Writing into ~/.claude for someone who does not run Claude Code
        // would be creating configuration for absent software.
        let bare = home_with(&[]);
        install_into(bare.path(), false).expect("install");
        assert!(!bare.path().join(".claude").exists());

        let with_claude = home_with(&[".claude"]);
        install_into(with_claude.path(), false).expect("install");
        assert!(with_claude
            .path()
            .join(".claude/skills/animesh/SKILL.md")
            .exists());
    }

    #[test]
    fn reinstalling_over_an_identical_file_is_not_a_conflict() {
        let home = home_with(&[]);
        install_into(home.path(), false).expect("first install");
        install_into(home.path(), false).expect("second install");
    }

    #[test]
    fn an_edited_skill_is_not_silently_replaced() {
        let home = home_with(&[]);
        install_into(home.path(), false).expect("install");
        let file = home.path().join(".agents/skills/animesh/SKILL.md");
        std::fs::write(&file, "---\nname: animesh\n---\nmine\n").expect("edit");

        let error = install_into(home.path(), false).expect_err("should refuse");
        assert_eq!(error.code, ErrorCode::InvalidArgument);
        assert!(error.message.contains("--force"), "{}", error.message);
        // And it really did not write.
        assert!(std::fs::read_to_string(&file)
            .expect("read")
            .contains("mine"));

        install_into(home.path(), true).expect("forced install");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), SKILL_MD);
    }

    #[test]
    fn a_conflict_in_one_location_writes_none_of_them() {
        // Half-upgraded agents would answer differently from each other.
        let home = home_with(&[".claude"]);
        install_into(home.path(), false).expect("install");
        let claude = home.path().join(".claude/skills/animesh/SKILL.md");
        std::fs::write(&claude, "edited").expect("edit");
        std::fs::remove_file(home.path().join(".agents/skills/animesh/SKILL.md")).expect("remove");

        install_into(home.path(), false).expect_err("should refuse");
        assert!(!home.path().join(".agents/skills/animesh/SKILL.md").exists());
    }

    #[test]
    fn uninstalling_removes_every_location() {
        let home = home_with(&[".claude"]);
        install_into(home.path(), false).expect("install");
        uninstall_from(home.path()).expect("uninstall");
        assert!(!home.path().join(".agents/skills/animesh").exists());
        assert!(!home.path().join(".claude/skills/animesh").exists());
    }

    #[test]
    fn uninstalling_what_was_never_installed_is_not_an_error() {
        let home = home_with(&[]);
        let message = uninstall_from(home.path()).expect("uninstall");
        assert!(message.contains("not installed"), "{message}");
    }

    #[test]
    fn status_reports_each_state() {
        let home = home_with(&[".claude"]);
        assert!(status_of(home.path())
            .expect("status")
            .contains("not installed"));

        install_into(home.path(), false).expect("install");
        let installed = status_of(home.path()).expect("status");
        assert!(installed.contains("installed"), "{installed}");

        std::fs::write(
            home.path().join(".agents/skills/animesh/SKILL.md"),
            "edited",
        )
        .expect("edit");
        assert!(status_of(home.path())
            .expect("status")
            .contains("edited since"));
    }

    #[test]
    fn uninstalling_leaves_other_skills_alone() {
        let home = home_with(&[]);
        install_into(home.path(), false).expect("install");
        let neighbour = home.path().join(".agents/skills/something-else");
        std::fs::create_dir_all(&neighbour).expect("create");
        std::fs::write(neighbour.join("SKILL.md"), "theirs").expect("write");

        uninstall_from(home.path()).expect("uninstall");
        assert!(neighbour.join("SKILL.md").exists());
    }
}
