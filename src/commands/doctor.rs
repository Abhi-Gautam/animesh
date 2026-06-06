//! `animesh doctor` — read-only health surface.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;

use crate::{
    commands::Command,
    observer::{format_report, report},
    store::{resolve_db_path, Db},
};

pub struct DoctorCommand;

impl DoctorCommand {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DoctorCommand {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait(?Send)]
impl Command for DoctorCommand {
    async fn execute(&self) -> Result<()> {
        let path = resolve_db_path()?;
        let db = Db::open(&path)?;
        let now = Utc::now().timestamp();
        let r = report(&db, &path, now)?;
        print!("{}", format_report(&r));
        Ok(())
    }
}
