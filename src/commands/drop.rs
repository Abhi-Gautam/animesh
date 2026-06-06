//! `animesh drop` — soft-delete. Hides the show from default `list`
//! and `schedule` views, preserves the row for future `follow` to
//! restore.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::Utc;

use crate::{
    commands::Command,
    errors::user_error,
    store::{resolve_db_path, Db},
};

pub struct DropCommand {
    source_id: String,
}

impl DropCommand {
    pub fn new_anilist(id: i64) -> Self {
        Self {
            source_id: id.to_string(),
        }
    }
}

#[async_trait(?Send)]
impl Command for DropCommand {
    async fn execute(&self) -> Result<()> {
        let path = resolve_db_path()?;
        let db = Db::open(&path)?;
        let now = Utc::now().timestamp();
        let touched = db.drop_follow("anilist", &self.source_id, now)?;
        if !touched {
            return Err(user_error(anyhow!(
                "no followed show with anilist id {} — nothing to drop",
                self.source_id
            )));
        }
        let row = db.find_by_source("anilist", &self.source_id)?;
        if let Some(item) = row {
            println!("Dropped: {} (id {})", item.display_title, item.source_id);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    #[test]
    fn drop_marks_existing_row_as_dropped() {
        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100)
            .unwrap();
        let touched = db.drop_follow("anilist", "21", 200).unwrap();
        assert!(touched);
        let row = db.find_by_source("anilist", "21").unwrap().unwrap();
        assert_eq!(row.dropped_at, Some(200));
    }

    #[test]
    fn drop_unknown_id_returns_false_so_command_can_error_user() {
        let db = Db::open_in_memory().unwrap();
        assert!(!db.drop_follow("anilist", "99999", 1).unwrap());
    }
}
