//! `animesh sub` — manage streaming/audio subscriptions.
//!
//! Reads + writes `~/.config/animesh/config.toml`. Subscriptions are
//! the gating signal for the verify-then-notify flow: a streaming link
//! has to be for a subscribed streamer to fire a [`VerifiedRelease`].
//!
//! Subcommands:
//!
//!   * `animesh sub list [video|audio|all]`   — print configured
//!   * `animesh sub add <video|audio> <name>` — add and save
//!   * `animesh sub remove <video|audio> <name>` — remove and save

use anyhow::{anyhow, Result};

use crate::config::{Config, Subscriptions};
use crate::errors::user_error;

/// CLI entry point. `args` is the argv after `sub`.
pub async fn run(args: &[String]) -> Result<()> {
    let path = Config::default_path()?;
    let mut config = Config::load_or_default(&path)?;
    let cmd = args.first().map(String::as_str).unwrap_or("list");
    match cmd {
        "list" => {
            let kind = args.get(1).map(String::as_str).unwrap_or("all");
            print!("{}", render_list(&config.subscriptions, kind)?);
        }
        "add" => {
            let kind = args.get(1).ok_or_else(|| {
                user_error(anyhow!("Usage: animesh sub add <video|audio> <name>"))
            })?;
            let name = args.get(2).ok_or_else(|| {
                user_error(anyhow!("Usage: animesh sub add <video|audio> <name>"))
            })?;
            mutate(&mut config.subscriptions, kind, name, Op::Add)?;
            config.save(&path)?;
            println!("added {kind} subscription: {name}");
        }
        "remove" | "rm" => {
            let kind = args.get(1).ok_or_else(|| {
                user_error(anyhow!("Usage: animesh sub remove <video|audio> <name>"))
            })?;
            let name = args.get(2).ok_or_else(|| {
                user_error(anyhow!("Usage: animesh sub remove <video|audio> <name>"))
            })?;
            mutate(&mut config.subscriptions, kind, name, Op::Remove)?;
            config.save(&path)?;
            println!("removed {kind} subscription: {name}");
        }
        "--help" | "-h" => {
            eprintln!(
                "Usage:\n  animesh sub list [video|audio|all]\n  \
                 animesh sub add <video|audio> <name>\n  \
                 animesh sub remove <video|audio> <name>"
            );
        }
        other => {
            return Err(user_error(anyhow!(
                "unknown sub subcommand {other:?}; try --help"
            )));
        }
    }
    Ok(())
}

/// What `mutate` is being asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Add,
    Remove,
}

/// Mutate the in-memory subscriptions. Case-insensitive dedupe for
/// add (so `add Netflix` and `add netflix` don't both land), and
/// case-insensitive match for remove.
fn mutate(subs: &mut Subscriptions, kind: &str, name: &str, op: Op) -> Result<()> {
    let target = match kind {
        "video" => &mut subs.video,
        "audio" => &mut subs.audio,
        other => {
            return Err(user_error(anyhow!(
                "unknown kind {other:?}; want video or audio"
            )))
        }
    };
    let canonical = name.trim();
    if canonical.is_empty() {
        return Err(user_error(anyhow!("subscription name must not be empty")));
    }
    match op {
        Op::Add => {
            if !target.iter().any(|s| s.eq_ignore_ascii_case(canonical)) {
                target.push(canonical.to_string());
            }
        }
        Op::Remove => {
            let before = target.len();
            target.retain(|s| !s.eq_ignore_ascii_case(canonical));
            if target.len() == before {
                return Err(user_error(anyhow!(
                    "no {kind} subscription named {canonical:?}"
                )));
            }
        }
    }
    Ok(())
}

/// Render a `sub list` output. Public for testing.
fn render_list(subs: &Subscriptions, kind: &str) -> Result<String> {
    let mut out = String::new();
    match kind {
        "all" => {
            out.push_str(&render_section("video", &subs.video));
            out.push_str(&render_section("audio", &subs.audio));
        }
        "video" => out.push_str(&render_section("video", &subs.video)),
        "audio" => out.push_str(&render_section("audio", &subs.audio)),
        other => {
            return Err(user_error(anyhow!(
                "unknown kind {other:?}; want video, audio, or all"
            )))
        }
    }
    Ok(out)
}

fn render_section(kind: &str, items: &[String]) -> String {
    if items.is_empty() {
        return format!("{kind}: (none configured)\n");
    }
    let mut s = format!("{kind}:\n");
    for it in items {
        s.push_str(&format!("  - {it}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Subscriptions {
        Subscriptions::default()
    }

    #[test]
    fn add_video_subscription_persists_in_memory() {
        let mut s = fresh();
        mutate(&mut s, "video", "Netflix", Op::Add).unwrap();
        assert_eq!(s.video, vec!["Netflix"]);
    }

    #[test]
    fn add_dedupes_case_insensitively() {
        let mut s = fresh();
        mutate(&mut s, "video", "Netflix", Op::Add).unwrap();
        mutate(&mut s, "video", "netflix", Op::Add).unwrap();
        assert_eq!(s.video, vec!["Netflix"], "second add must not duplicate");
    }

    #[test]
    fn remove_matches_case_insensitively() {
        let mut s = fresh();
        mutate(&mut s, "video", "Netflix", Op::Add).unwrap();
        mutate(&mut s, "video", "NETFLIX", Op::Remove).unwrap();
        assert!(s.video.is_empty());
    }

    #[test]
    fn remove_nonexistent_returns_error() {
        let mut s = fresh();
        let err = mutate(&mut s, "video", "Netflix", Op::Remove).unwrap_err();
        assert!(format!("{err}").contains("Netflix"));
    }

    #[test]
    fn unknown_kind_returns_error() {
        let mut s = fresh();
        let err = mutate(&mut s, "podcasts", "Spotify", Op::Add).unwrap_err();
        assert!(format!("{err}").contains("video or audio"));
    }

    #[test]
    fn empty_name_is_rejected() {
        let mut s = fresh();
        let err = mutate(&mut s, "video", "   ", Op::Add).unwrap_err();
        assert!(format!("{err}").contains("empty"));
    }

    #[test]
    fn render_list_all_shows_both_kinds() {
        let s = Subscriptions {
            video: vec!["Netflix".into()],
            audio: vec!["Spotify".into()],
        };
        let out = render_list(&s, "all").unwrap();
        assert!(out.contains("video:\n  - Netflix"));
        assert!(out.contains("audio:\n  - Spotify"));
    }

    #[test]
    fn render_list_video_only_omits_audio() {
        let s = Subscriptions {
            video: vec!["Netflix".into()],
            audio: vec!["Spotify".into()],
        };
        let out = render_list(&s, "video").unwrap();
        assert!(out.contains("Netflix"));
        assert!(!out.contains("Spotify"));
    }

    #[test]
    fn render_list_empty_says_so() {
        let s = fresh();
        let out = render_list(&s, "all").unwrap();
        assert!(out.contains("none configured"));
    }
}
