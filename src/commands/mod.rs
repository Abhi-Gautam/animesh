use anyhow::Result;
use async_trait::async_trait;

pub mod doctor;
pub mod drop;
pub mod follow;
pub mod list;
pub mod schedule;
pub mod sync;
pub mod unfollow;

pub use doctor::DoctorCommand;
pub use drop::DropCommand;
pub use follow::FollowCommand;
pub use list::ListCommand;
pub use schedule::ScheduleCommand;
pub use sync::SyncCommand;
pub use unfollow::UnfollowCommand;

/// Base trait for all commands.
///
/// The futures are intentionally not `Send`: `rusqlite::Connection`
/// is `!Send` because of its internal statement cache, and commands
/// regularly hold the Db across `.await` points. `?Send` reflects
/// reality and costs nothing — the binary is single-threaded.
#[async_trait(?Send)]
pub trait Command {
    /// Execute the command
    async fn execute(&self) -> Result<()>;
}
