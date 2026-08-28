//! The `animesh` command surface.

use std::str::FromStr;

use clap::{Parser, Subcommand};

use crate::domain::ids::{AniListId, IdError, MediaId, SourceNumericId, TvMazeId};

/// What `follow` accepts: a bare number (AniList unless `--tv`), or `anilist:N` / `tvmaze:N`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowTarget {
    Bare(SourceNumericId),
    AniList(AniListId),
    TvMaze(TvMazeId),
}

impl FromStr for FollowTarget {
    type Err = IdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix("tvmaze:") {
            return Ok(Self::TvMaze(rest.parse()?));
        }
        if let Some(rest) = s.strip_prefix("anilist:") {
            return Ok(Self::AniList(rest.parse()?));
        }
        Ok(Self::Bare(s.parse()?))
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "animesh",
    version,
    about = "Personal release radar for anime and TV",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Emit one JSON document on stdout instead of text.
    ///
    /// Global rather than per-command so an agent can set it once and parse
    /// every answer the same way. The envelope is `{"ok":true,"kind":..,
    /// "data":..}` or `{"ok":false,"error":..}`; exit codes are unchanged.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search for a title. AniList by default; `--tv` uses TVmaze.
    Search {
        /// Free-text query. Quote it if it contains spaces.
        ///
        /// With `--tv` and no query, lists currently airing English-language
        /// US broadcasts and streams.
        query: Vec<String>,
        /// Search TVmaze instead of AniList.
        #[arg(long)]
        tv: bool,
    },

    /// Follow a title. AniList by default; ids overlap across sources.
    ///
    /// A bare number is AniList. Paste the `tvmaze:82` search prints, or pass
    /// `--tv 82`. `anilist:21` is the same as `21`.
    Follow {
        #[arg(value_name = "ID")]
        id: FollowTarget,
        /// Follow a bare number as a TVmaze show id.
        #[arg(long)]
        tv: bool,
    },

    /// Show upcoming episodes. Local-only; never touches the network.
    Next {
        #[arg(long, short = 'n')]
        limit: Option<u32>,
    },

    /// List everything you follow.
    List,

    /// Stop following a title.
    Drop {
        #[arg(value_name = "MEDIA_ID")]
        media_id: MediaId,
    },

    /// Ask the app to refresh schedules now.
    Refresh,

    /// Show app health.
    Status,

    /// Run Animesh in the background, or stop it.
    ///
    /// Installing already does this. These are repair tools.
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// Install the Agent Skill, so any AI agent can drive Animesh.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillAction {
    /// Write the skill where agents look for it.
    Install {
        /// Overwrite a skill file that has been edited since it was installed.
        #[arg(long)]
        force: bool,
    },
    /// Remove the skill from every location it was installed to.
    Uninstall,
    /// Where the skill is installed, and whether it is current.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum ServiceAction {
    /// Register the daemon and start it at login.
    Start,
    /// Stop the daemon and unregister it. The library is kept.
    Stop,
    Restart,
    /// Whether the system is managing the daemon.
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_command_surface_matches_the_plan() {
        Cli::command().debug_assert();

        let names: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        assert_eq!(
            names,
            vec![
                "search", "follow", "next", "list", "drop", "refresh", "status", "service", "skill"
            ]
        );
    }

    #[test]
    fn an_out_of_range_id_is_rejected_by_the_parser() {
        // The newtype's FromStr is what stops a bad id reaching the socket.
        assert!(Cli::try_parse_from(["animesh", "follow", "0"]).is_err());
        assert!(Cli::try_parse_from(["animesh", "follow", "banana"]).is_err());
        assert!(Cli::try_parse_from(["animesh", "follow", "21"]).is_ok());
        assert!(Cli::try_parse_from(["animesh", "follow", "--tv", "0"]).is_err());
        assert!(Cli::try_parse_from(["animesh", "follow", "tvmaze:0"]).is_err());
        assert!(Cli::try_parse_from(["animesh", "follow", "anilist:21"]).is_ok());
    }

    #[test]
    fn tv_flags_select_tvmaze() {
        let search = Cli::try_parse_from(["animesh", "search", "--tv"]).expect("parse");
        match search.command {
            Command::Search { query, tv } => {
                assert!(query.is_empty());
                assert!(tv);
            }
            other => panic!("expected search, got {other:?}"),
        }
        let follow = Cli::try_parse_from(["animesh", "follow", "--tv", "82"]).expect("parse");
        match follow.command {
            Command::Follow { id, tv } => {
                assert_eq!(
                    id,
                    FollowTarget::Bare(SourceNumericId::new(82).expect("id"))
                );
                assert!(tv);
            }
            other => panic!("expected follow, got {other:?}"),
        }
        let prefixed = Cli::try_parse_from(["animesh", "follow", "tvmaze:82"]).expect("parse");
        match prefixed.command {
            Command::Follow { id, tv } => {
                assert_eq!(id, FollowTarget::TvMaze(TvMazeId::new(82).expect("id")));
                assert!(!tv);
            }
            other => panic!("expected follow, got {other:?}"),
        }
    }

    #[test]
    fn search_accepts_unquoted_multiword_queries() {
        let cli = Cli::try_parse_from(["animesh", "search", "one", "piece"]).expect("parse");
        match cli.command {
            Command::Search { query, tv } => {
                assert_eq!(query, vec!["one", "piece"]);
                assert!(!tv);
            }
            other => panic!("expected search, got {other:?}"),
        }
    }

    #[test]
    fn json_is_accepted_before_or_after_the_subcommand() {
        // A global flag, so an agent can put it wherever it builds argv.
        assert!(
            Cli::try_parse_from(["animesh", "--json", "next"])
                .expect("parse")
                .json
        );
        assert!(
            Cli::try_parse_from(["animesh", "next", "--json"])
                .expect("parse")
                .json
        );
        assert!(
            !Cli::try_parse_from(["animesh", "next"])
                .expect("parse")
                .json
        );
    }

    #[test]
    fn next_accepts_a_limit() {
        let cli = Cli::try_parse_from(["animesh", "next", "-n", "5"]).expect("parse");
        match cli.command {
            Command::Next { limit } => assert_eq!(limit, Some(5)),
            other => panic!("expected next, got {other:?}"),
        }
    }
}
