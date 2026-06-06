//! `animesh list` — pure local read of the library.
//!
//! No network, no tokio runtime work, no schema migration triggered
//! by reading. Reads against any compatible schema version per spec
//! §8.

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    commands::Command,
    renderer::render_tracked_items,
    store::{resolve_db_path, Db, ListFilter},
    utils::get_user_timezone,
};

pub struct ListCommand {
    filter: ListFilter,
    colored: bool,
}

impl ListCommand {
    pub fn new(filter: ListFilter) -> Self {
        Self {
            filter,
            colored: true,
        }
    }

    /// Build a command that renders without ANSI colors (used by
    /// snapshot tests and `--no-color` scripts).
    pub fn plain(filter: ListFilter) -> Self {
        Self {
            filter,
            colored: false,
        }
    }
}

#[async_trait]
impl Command for ListCommand {
    async fn execute(&self) -> Result<()> {
        let path = resolve_db_path()?;
        let db = Db::open(&path)?;
        let items = db.list_follows(self.filter)?;
        let tz = get_user_timezone();
        let out = render_tracked_items(&items, tz, self.colored);
        print!("{out}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;
    use tempfile::TempDir;

    // Integration-ish smoke test: build a DB, populate via the store
    // API, then verify list_follows returns what we expect. Renderer
    // is tested independently; here we just verify the command's
    // logical path doesn't blow up when wired end-to-end.

    #[test]
    fn list_follows_active_returns_only_active() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("library.db");
        let mut db = Db::open(&path).unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100).unwrap();
        db.add_follow("anilist", "1", "anime", "Cowboy Bebop", 200).unwrap();
        db.drop_follow("anilist", "1", 300).unwrap();
        let active = db.list_follows(ListFilter::Active).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].source_id, "21");
        let dropped = db.list_follows(ListFilter::Dropped).unwrap();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].source_id, "1");
    }
}
