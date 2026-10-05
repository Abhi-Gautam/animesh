//! Bounded discovery normalization; no persistence and no follow policy.

use std::collections::BTreeSet;

use crate::domain::command_center::{FeedKey, DISCOVERY_LIMIT};
use crate::domain::ids::{EpisodeNumber, UnixTimestamp};
use crate::domain::media::{MediaObservation, NextAiring};

#[derive(Debug)]
pub struct CatalogPage {
    pub items: Vec<MediaObservation>,
    pub complete: bool,
}

pub fn anilist(body: &str, feed: FeedKey, now: UnixTimestamp) -> Result<CatalogPage, String> {
    let envelope: super::anilist::dto::GraphQlEnvelope<super::anilist::dto::PageData> =
        serde_json::from_str(body)
            .map_err(|e| format!("AniList sent unreadable discovery data: {e}"))?;
    let page = envelope
        .data
        .and_then(|d| d.page)
        .ok_or("AniList did not return a discovery page")?;
    let mut complete = envelope.errors.is_empty();
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for media in page.media {
        let Some(media) = media else {
            complete = false;
            continue;
        };
        match super::anilist::parser::parse_media(&media) {
            Ok(item) if !seen.insert(item.source_key) => {
                complete = false;
            }
            Ok(item) if feed.accepts(&item, now) => items.push(item),
            Ok(_) => {}
            Err(_) => complete = false,
        }
    }
    items.truncate(DISCOVERY_LIMIT);
    Ok(CatalogPage { items, complete })
}

pub fn tvmaze(body: &str) -> Result<CatalogPage, String> {
    let rows: Vec<super::tvmaze::dto::ScheduleRow> = serde_json::from_str(body)
        .map_err(|e| format!("TVmaze sent unreadable discovery data: {e}"))?;
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    let mut complete = true;
    for row in rows {
        let Some(show) = row.show() else {
            complete = false;
            continue;
        };
        if show.language.as_deref() != Some("English")
            || show.status.as_deref() != Some("Running")
            || !matches!(show.show_type.as_deref(), Some("Scripted" | "Animation"))
        {
            continue;
        }
        let Ok(mut item) = super::tvmaze::parser::parse_show(show) else {
            complete = false;
            continue;
        };
        if !seen.insert(item.source_key) {
            continue;
        }
        if let (Some(number), Some(stamp)) = (row.number, row.airstamp.as_deref()) {
            match (
                EpisodeNumber::new(number),
                chrono::DateTime::parse_from_rfc3339(stamp)
                    .ok()
                    .and_then(|d| UnixTimestamp::new(d.timestamp()).ok()),
            ) {
                (Ok(episode), Some(airing_at)) => {
                    item.next_airing = Some(NextAiring {
                        episode,
                        airing_at,
                        season: row.season.filter(|s| *s > 0),
                    })
                }
                _ => {
                    complete = false;
                    continue;
                }
            }
        }
        items.push(item);
    }
    items.truncate(DISCOVERY_LIMIT);
    Ok(CatalogPage { items, complete })
}
