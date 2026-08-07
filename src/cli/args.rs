//! The `animesh` command surface, frozen by plan section 16.

use clap::{Parser, Subcommand};

use crate::domain::ids::{AniListId, MediaId};

#[derive(Debug, Parser)]
#[command(
    name = "animesh",
    version,
    about = "Personal release radar for anime",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Search AniList for a title.
    Search {
        /// Free-text query. Quote it if it contains spaces.
        query: Vec<String>,
    },

    /// Follow a title by its AniList id.
    Follow {
        #[arg(value_name = "ANILIST_ID")]
        id: AniListId,
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
            vec!["search", "follow", "next", "list", "drop", "refresh", "status"]
        );
    }

    #[test]
    fn an_out_of_range_id_is_rejected_by_the_parser() {
        // The newtype's FromStr is what stops a bad id reaching the socket.
        assert!(Cli::try_parse_from(["animesh", "follow", "0"]).is_err());
        assert!(Cli::try_parse_from(["animesh", "follow", "banana"]).is_err());
        assert!(Cli::try_parse_from(["animesh", "follow", "21"]).is_ok());
    }

    #[test]
    fn search_accepts_unquoted_multiword_queries() {
        let cli = Cli::try_parse_from(["animesh", "search", "one", "piece"]).expect("parse");
        match cli.command {
            Command::Search { query } => assert_eq!(query, vec!["one", "piece"]),
            other => panic!("expected search, got {other:?}"),
        }
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
