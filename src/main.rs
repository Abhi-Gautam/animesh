// src/main.rs
mod anilist;
mod commands;
mod observer;
mod picker;
mod renderer;
mod store;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    Command, DoctorCommand, DropCommand, FollowCommand, ListCommand, ScheduleCommand,
    SyncCommand, UnfollowCommand,
};
use store::ListFilter;

/// A powerful CLI tool for anime fans to track their favorite shows
#[derive(Parser)]
#[command(name = "animesh", author = "Abhishek Gautam", version, about = "Track anime schedules and discover new shows", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show anime airing schedule (default: your followed shows; `--all`
    /// browses the global AniList schedule).
    Schedule {
        /// Number of days to show schedule for
        #[arg(short, long, default_value = "1")]
        interval: u32,

        /// Timezone for schedule display (e.g., "UTC", "IST")
        #[arg(short, long)]
        timezone: Option<String>,

        /// Show past episodes instead of upcoming ones (implies --all
        /// in v0.3; followed-only past views require historical episode
        /// data shipped in SP-3).
        #[arg(short, long)]
        past: bool,

        /// Show the global AniList schedule instead of just your
        /// followed shows. Side-effects results into the picker cache.
        #[arg(short = 'A', long)]
        all: bool,
    },
    /// List shows in your local library
    List {
        /// Include dropped shows alongside active ones
        #[arg(long)]
        all: bool,
        /// Show only dropped shows
        #[arg(long, conflicts_with = "all")]
        dropped: bool,
        /// Disable ANSI colors (useful in scripts and CI)
        #[arg(long)]
        no_color: bool,
    },
    /// Add a show to your library
    Follow {
        /// AniList numeric ID of the show.
        /// The interactive `animesh follow <query>` picker lands in v0.4.
        #[arg(long)]
        id: i64,
    },
    /// Soft-delete a show from your library. Hidden from default
    /// views; `animesh follow --id N` restores it preserving the
    /// original follow date.
    Drop {
        #[arg(long)]
        id: i64,
    },
    /// Hard-delete a show from your library. The rare path; prefer
    /// `drop` unless you really mean it.
    Unfollow {
        #[arg(long)]
        id: i64,
    },
    /// Refresh cached metadata for every active follow from AniList.
    /// Explicit; the only command that intentionally writes to the
    /// cache from the user's side.
    Sync,
    /// Show the EXPLAIN of animesh — DB path, schema version,
    /// counts, cache health, last sync, rate-limit headroom.
    Doctor,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Schedule {
            interval,
            timezone,
            past,
            all,
        } => {
            let command = ScheduleCommand::new(interval, timezone, past, all);
            command.execute().await?;
        }
        Commands::List {
            all,
            dropped,
            no_color,
        } => {
            let filter = match (all, dropped) {
                (_, true) => ListFilter::Dropped,
                (true, false) => ListFilter::All,
                (false, false) => ListFilter::Active,
            };
            let command = if no_color {
                ListCommand::plain(filter)
            } else {
                ListCommand::new(filter)
            };
            command.execute().await?;
        }
        Commands::Follow { id } => {
            FollowCommand::new(id).execute().await?;
        }
        Commands::Drop { id } => {
            DropCommand::new_anilist(id).execute().await?;
        }
        Commands::Unfollow { id } => {
            UnfollowCommand::new_anilist(id).execute().await?;
        }
        Commands::Sync => {
            SyncCommand::new().execute().await?;
        }
        Commands::Doctor => {
            DoctorCommand::new().execute().await?;
        }
    }

    Ok(())
}
