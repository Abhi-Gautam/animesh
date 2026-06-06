use anyhow::Result;
use async_trait::async_trait;

pub mod drop;
pub mod follow;
pub mod list;
pub mod schedule;
pub mod sync;
pub mod unfollow;

pub use drop::DropCommand;
pub use follow::FollowCommand;
pub use list::ListCommand;
pub use schedule::ScheduleCommand;
pub use sync::SyncCommand;
pub use unfollow::UnfollowCommand;

/// Base trait for all commands
#[async_trait]
pub trait Command {
    /// Execute the command
    async fn execute(&self) -> Result<()>;
}
