//! `animesh unfollow` — hard delete from the library.
//!
//! This is the rare path. SP-2 will introduce watched-progress that
//! can be lost by an accidental unfollow; until then, the command
//! itself stays simple. A `--force` gate plus a preview of what would
//! be lost lands when SP-2 ships.

use anyhow::{anyhow, Result};
use async_trait::async_trait;

use crate::{
    commands::Command,
    store::{resolve_db_path, Db},
};

pub struct UnfollowCommand {
    source_id: String,
}

impl UnfollowCommand {
    pub fn new_anilist(id: i64) -> Self {
        Self {
            source_id: id.to_string(),
        }
    }
}

#[async_trait(?Send)]
impl Command for UnfollowCommand {
    async fn execute(&self) -> Result<()> {
        let path = resolve_db_path()?;
        let db = Db::open(&path)?;
        // Capture the title before the row is gone, so the printed
        // confirmation is meaningful.
        let title = db
            .find_by_source("anilist", &self.source_id)?
            .map(|i| i.display_title);
        let removed = db.unfollow("anilist", &self.source_id)?;
        if !removed {
            return Err(anyhow!(
                "no followed show with anilist id {} — nothing to unfollow",
                self.source_id
            ));
        }
        match title {
            Some(t) => println!("Unfollowed: {t} (id {})", self.source_id),
            None => println!("Unfollowed id {}", self.source_id),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    #[test]
    fn unfollow_hard_deletes_row() {
        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100)
            .unwrap();
        let removed = db.unfollow("anilist", "21").unwrap();
        assert!(removed);
        assert!(db.find_by_source("anilist", "21").unwrap().is_none());
    }
}
