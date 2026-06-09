//! Per-canonical LLM context blob. Plain `serde_json::Value` so the
//! shape is observable in tests and reviewable by a human.

use anyhow::Result;
use serde_json::json;

use crate::library::Library;
use crate::tui::model::Show;

/// Build a JSON object summarising a canonical for paste into an LLM.
/// Pulls every source_ref + the last 10 engagements so the agent sees
/// the substrate, not just the row.
pub fn build(lib: &Library, show: &Show) -> Result<serde_json::Value> {
    let refs = lib.source_refs_for(show.canonical_id())?;
    let engagements = lib.engagement_for(show.canonical_id())?;
    let recent: Vec<_> = engagements
        .iter()
        .rev()
        .take(10)
        .map(|e| {
            json!({
                "event": e.event.as_str(),
                "at": e.occurred_at,
                "meta": e.meta.as_ref().map(|m| m.to_json_value()),
            })
        })
        .collect();
    Ok(json!({
        "canonical_id": show.canonical_id().as_str(),
        "kind": show.canonical_id().kind().as_str(),
        "title": show.display_title(),
        "romaji": show.romaji(),
        "score": show.score(),
        "status": show.status(),
        "format": show.format(),
        "description": show.description(),
        "refs": refs.iter().map(|r| json!({
            "source": r.source,
            "source_id": r.source_id,
        })).collect::<Vec<_>>(),
        "streaming_links": show.streaming.iter().map(|l| json!({
            "site": l.site,
            "url": l.url,
        })).collect::<Vec<_>>(),
        "verified": show.last_verified.as_ref().map(|e| json!({
            "streamer": show.verified_streamer(),
            "url": show.verified_url(),
            "at": e.occurred_at,
        })),
        "last_completed": show.last_completed.as_ref().map(|e| json!({
            "at": e.occurred_at,
            "meta": e.meta.as_ref().map(|m| m.to_json_value()),
        })),
        "recent_engagement": recent,
        "subscribed_match": show.subscribed_match,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::store::{CanonicalRelease, SourceRef};
    use crate::time::FixedClock;
    use crate::tui::model::Show;

    fn dummy_show() -> Show {
        let id = CanonicalId::new(ReleaseKind::Anime, "frieren").unwrap();
        Show {
            canonical: CanonicalRelease {
                id: id.clone(),
                kind: ReleaseKind::Anime,
                display_title: "Frieren".into(),
                cover_ascii: None,
                cover_color: None,
                followed_at: Some(1),
                dropped_at: None,
                user_note: None,
                created_at: 1,
            },
            primary_source: SourceRef {
                canonical_id: id,
                source: "anilist".into(),
                source_id: "154587".into(),
                raw_title: Some("Frieren".into()),
                confidence: 1.0,
            },
            cache: None,
            last_completed: None,
            last_verified: None,
            subscribed_match: false,
            pane: None,
            streaming: vec![],
        }
    }

    #[test]
    fn build_includes_kind_and_refs() {
        let lib = Library::open_in_memory(Arc::new(FixedClock(1))).unwrap();
        let v = build(&lib, &dummy_show()).unwrap();
        assert_eq!(v["title"], "Frieren");
        assert_eq!(v["kind"], "anime");
        assert!(v["refs"].is_array());
    }
}
