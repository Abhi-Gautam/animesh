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
use commands::{Command, ScheduleCommand};

/// A powerful CLI tool for anime fans to track their favorite shows
#[derive(Parser)]
#[command(name = "animesh", author = "Abhishek Gautam", version = "0.1.0", about = "Track anime schedules and discover new shows", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show anime airing schedule
    Schedule {
        /// Number of days to show schedule for
        #[arg(short, long, default_value = "1")]
        interval: u32,

        /// Timezone for schedule display (e.g., "UTC", "IST")
        #[arg(short, long)]
        timezone: Option<String>,

        /// Show past episodes instead of upcoming ones
        #[arg(short, long)]
        past: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Schedule {
            interval,
            timezone,
            past,
        } => {
            let command = ScheduleCommand::new(interval, timezone, past);
            command.execute().await?;
        }
    }

    Ok(())
}
