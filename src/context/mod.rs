//! LLM-readable export of the user's taste graph.
//!
//! Builds a single document — JSON or Markdown — that an external
//! agent ("what should I watch tonight?") can consume without any
//! priors. The document is intended to be self-explanatory: every
//! field's name is its semantics. Schemas drift, so we lock both
//! output shapes via insta snapshots.

use std::sync::Arc;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::library::Library;
use crate::store::EngagementEvent;

/// Which serialization the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Markdown,
}

/// Build a contextual snapshot from the library.
pub struct ContextExporter {
    library: Arc<Library>,
}

impl ContextExporter {
    pub fn new(library: Arc<Library>) -> Self {
        Self { library }
    }

    /// Render an export. `engagement_since` is unix seconds — events
    /// older than this are dropped. `engagement_limit` caps the
    /// number of recent events; 0 means no cap.
    pub fn export(
        &self,
        format: ExportFormat,
        engagement_since: i64,
        engagement_limit: u32,
    ) -> Result<String> {
        let snapshot = self.build_snapshot(engagement_since, engagement_limit)?;
        match format {
            ExportFormat::Json => serde_json::to_string_pretty(&snapshot)
                .context("serialize context snapshot to JSON"),
            ExportFormat::Markdown => Ok(render_markdown(&snapshot)),
        }
    }

    fn build_snapshot(&self, since: i64, limit: u32) -> Result<Snapshot> {
        let followed = self
            .library
            .followed()
            .context("load followed canonicals")?
            .into_iter()
            .map(|cr| FollowedEntry {
                canonical_id: cr.id.to_string(),
                kind: cr.kind.to_string(),
                display_title: cr.display_title,
                followed_at: cr.followed_at,
                user_note: cr.user_note,
            })
            .collect();

        let events = self
            .library
            .recent_engagement(since, limit)
            .context("load recent engagement")?
            .into_iter()
            .map(|e| EngagementEntry {
                canonical_id: e.canonical_id.to_string(),
                event: event_str(e.event).to_string(),
                occurred_at: e.occurred_at,
                meta: e.meta,
            })
            .collect();

        Ok(Snapshot {
            schema_version: 1,
            followed,
            recent_engagement: events,
        })
    }
}

fn event_str(e: EngagementEvent) -> &'static str {
    e.as_str()
}

// ---------------------------------------------------------------------------
// Wire shapes. Kept private to the module — the public surface is
// strings (JSON / Markdown). Changing these shapes is a snapshot
// review.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct Snapshot {
    schema_version: u32,
    followed: Vec<FollowedEntry>,
    recent_engagement: Vec<EngagementEntry>,
}

#[derive(Debug, Serialize)]
struct FollowedEntry {
    canonical_id: String,
    kind: String,
    display_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    followed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_note: Option<String>,
}

#[derive(Debug, Serialize)]
struct EngagementEntry {
    canonical_id: String,
    event: String,
    occurred_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    meta: Option<String>,
}

fn render_markdown(s: &Snapshot) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("# animesh — taste snapshot\n\n");
    out.push_str(&format!("_schema version: {}_\n\n", s.schema_version));

    out.push_str("## Followed\n\n");
    if s.followed.is_empty() {
        out.push_str("_(no followed canonicals)_\n\n");
    } else {
        out.push_str("| canonical_id | kind | title | followed_at | note |\n");
        out.push_str("| --- | --- | --- | --- | --- |\n");
        for f in &s.followed {
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} |\n",
                f.canonical_id,
                f.kind,
                escape_md_pipe(&f.display_title),
                f.followed_at
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "-".into()),
                f.user_note
                    .as_deref()
                    .map(escape_md_pipe)
                    .unwrap_or_default()
            ));
        }
        out.push('\n');
    }

    out.push_str("## Recent engagement\n\n");
    if s.recent_engagement.is_empty() {
        out.push_str("_(no recent events)_\n");
    } else {
        out.push_str("| occurred_at | event | canonical_id | meta |\n");
        out.push_str("| --- | --- | --- | --- |\n");
        for e in &s.recent_engagement {
            out.push_str(&format!(
                "| {} | {} | `{}` | {} |\n",
                e.occurred_at,
                e.event,
                e.canonical_id,
                e.meta
                    .as_deref()
                    .map(escape_md_pipe)
                    .unwrap_or_default()
            ));
        }
    }

    out
}

fn escape_md_pipe(s: &str) -> String {
    s.replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::time::AdvanceableClock;

    fn build_lib() -> Arc<Library> {
        let clock = AdvanceableClock::new(1_000_000);
        let lib = Arc::new(Library::open_in_memory(Arc::new(clock.clone())).unwrap());

        let severance = CanonicalId::new(ReleaseKind::Tv, "severance").unwrap();
        let onepiece = CanonicalId::new(ReleaseKind::Anime, "one-piece").unwrap();

        clock.set(1_000_000);
        lib.follow_with_source(
            &severance,
            ReleaseKind::Tv,
            "Severance",
            "tmdb",
            "95396",
            Some("Severance"),
            1.0,
        )
        .unwrap();

        clock.set(1_000_100);
        lib.follow_with_source(
            &onepiece,
            ReleaseKind::Anime,
            "One Piece",
            "anilist",
            "21",
            Some("ONE PIECE"),
            1.0,
        )
        .unwrap();

        clock.set(1_000_200);
        lib.engage(&severance, EngagementEvent::Opened, None).unwrap();
        clock.set(1_000_300);
        lib.engage(
            &onepiece,
            EngagementEvent::Completed,
            Some(r#"{"seen":1100}"#),
        )
        .unwrap();
        clock.set(1_000_400);
        lib.engage(&severance, EngagementEvent::Verified, None).unwrap();

        lib
    }

    #[test]
    fn json_snapshot_locks_schema_shape() {
        let exporter = ContextExporter::new(build_lib());
        let json = exporter.export(ExportFormat::Json, 0, 0).unwrap();
        insta::assert_snapshot!("context_json", json);
    }

    #[test]
    fn markdown_snapshot_locks_human_format() {
        let exporter = ContextExporter::new(build_lib());
        let md = exporter.export(ExportFormat::Markdown, 0, 0).unwrap();
        insta::assert_snapshot!("context_markdown", md);
    }

    #[test]
    fn engagement_since_threshold_filters_old_events() {
        let exporter = ContextExporter::new(build_lib());
        // since=1_000_350 → only the verified event at 1_000_400.
        let snap = exporter
            .build_snapshot(1_000_350, 0)
            .expect("snapshot ok");
        assert_eq!(snap.recent_engagement.len(), 1);
        assert_eq!(snap.recent_engagement[0].event, "verified");
    }

    #[test]
    fn engagement_limit_caps_event_count() {
        let exporter = ContextExporter::new(build_lib());
        let snap = exporter.build_snapshot(0, 2).expect("snapshot ok");
        assert_eq!(snap.recent_engagement.len(), 2);
    }

    #[test]
    fn empty_library_produces_self_explanatory_markdown() {
        let lib = Arc::new(
            Library::open_in_memory(Arc::new(crate::time::FixedClock(1_000_000))).unwrap(),
        );
        let exporter = ContextExporter::new(lib);
        let md = exporter.export(ExportFormat::Markdown, 0, 0).unwrap();
        assert!(md.contains("no followed"));
        assert!(md.contains("no recent events"));
    }

    #[test]
    fn empty_library_produces_well_formed_json() {
        let lib = Arc::new(
            Library::open_in_memory(Arc::new(crate::time::FixedClock(1_000_000))).unwrap(),
        );
        let exporter = ContextExporter::new(lib);
        let json = exporter.export(ExportFormat::Json, 0, 0).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert!(v["followed"].is_array());
        assert!(v["recent_engagement"].is_array());
        assert_eq!(v["followed"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn escape_md_pipe_avoids_breaking_table_rows() {
        assert_eq!(escape_md_pipe("a|b"), "a\\|b");
        assert_eq!(escape_md_pipe("normal"), "normal");
    }
}
