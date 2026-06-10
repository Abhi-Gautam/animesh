use anyhow::{Context, Result};

use crate::ids::{CanonicalId, ReleaseKind};
use crate::store::{
    CacheEntry, CanonicalRelease, Engagement, EngagementEvent, EngagementMeta, EngagementSource,
    SourceRef,
};

use super::Db;

/// One fully-resolved canonical the TUI can render directly. Produced by
/// `Library::load_resolved` in a single store query — no per-row joins from
/// callers above the store boundary.
#[derive(Debug, Clone)]
pub struct ResolvedRelease {
    pub canonical: CanonicalRelease,
    /// Highest-confidence attached source_ref. Invariant: a followed canonical
    /// has at least one attached ref, enforced by Library follow primitives.
    pub primary_source: SourceRef,
    pub cache: Option<CacheEntry>,
    pub last_completed: Option<Engagement>,
    pub last_verified: Option<Engagement>,
}

/// One prepared-and-cached statement runs every per-frame resolved load. The
/// window-function picks the highest-confidence source_ref per canonical and
/// the newest `Completed`/`Verified` event per canonical — all in a single
/// round trip.
const LOAD_RESOLVED_SQL: &str = "\
WITH primary_sr AS (\
    SELECT canonical_id, source, source_id, raw_title, confidence \
    FROM (\
        SELECT canonical_id, source, source_id, raw_title, confidence, \
               ROW_NUMBER() OVER ( \
                   PARTITION BY canonical_id \
                   ORDER BY confidence DESC, source ASC \
               ) AS rn \
        FROM source_ref\
    ) WHERE rn = 1\
), \
last_engagement_by_event AS (\
    SELECT canonical_id, event, id, occurred_at, meta \
    FROM (\
        SELECT canonical_id, event, id, occurred_at, meta, \
               ROW_NUMBER() OVER ( \
                   PARTITION BY canonical_id, event \
                   ORDER BY occurred_at DESC, id DESC \
               ) AS rn \
        FROM engagement \
        WHERE event IN ('completed', 'verified')\
    ) WHERE rn = 1\
) \
SELECT \
    cr.id, cr.kind, cr.display_title, cr.cover_ascii, cr.cover_color, \
    cr.followed_at, cr.dropped_at, cr.user_note, cr.created_at, \
    psr.source, psr.source_id, psr.raw_title, psr.confidence, \
    mc.display_title, mc.title_english, mc.title_native, mc.status, \
    mc.total_episodes, mc.format, mc.next_episode_number, mc.next_episode_airs_at, \
    mc.fetched_at, mc.expires_at, mc.cover_image_url, mc.description, \
    mc.score, mc.studios, mc.streaming_links_json, \
    lc.id, lc.occurred_at, lc.meta, \
    lv.id, lv.occurred_at, lv.meta \
FROM canonical_release cr \
JOIN primary_sr psr ON psr.canonical_id = cr.id \
LEFT JOIN metadata_cache mc \
    ON mc.source = psr.source AND mc.source_id = psr.source_id \
LEFT JOIN last_engagement_by_event lc \
    ON lc.canonical_id = cr.id AND lc.event = 'completed' \
LEFT JOIN last_engagement_by_event lv \
    ON lv.canonical_id = cr.id AND lv.event = 'verified' \
WHERE cr.followed_at IS NOT NULL AND cr.dropped_at IS NULL \
ORDER BY cr.followed_at DESC";

impl Db {
    pub fn load_resolved(&self) -> Result<Vec<ResolvedRelease>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare_cached(LOAD_RESOLVED_SQL)
            .context("prepare load_resolved")?;
        let rows = stmt
            .query_map([], map_resolved_row)
            .context("query load_resolved")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect load_resolved rows")
    }
}

fn map_resolved_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResolvedRelease> {
    let cr_id: CanonicalId = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let kind = kind_str.parse::<ReleaseKind>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        )
    })?;
    let canonical = CanonicalRelease {
        id: cr_id.clone(),
        kind,
        display_title: row.get(2)?,
        cover_ascii: row.get(3)?,
        cover_color: row.get(4)?,
        followed_at: row.get(5)?,
        dropped_at: row.get(6)?,
        user_note: row.get(7)?,
        created_at: row.get(8)?,
    };

    let primary_source = SourceRef {
        canonical_id: cr_id.clone(),
        source: row.get(9)?,
        source_id: row.get(10)?,
        raw_title: row.get(11)?,
        confidence: row.get(12)?,
    };

    let mc_fetched_at: Option<i64> = row.get(21)?;
    let cache = if let Some(fetched_at) = mc_fetched_at {
        Some(CacheEntry {
            source: primary_source.source.clone(),
            source_id: primary_source.source_id.clone(),
            display_title: row.get(13)?,
            title_english: row.get(14)?,
            title_native: row.get(15)?,
            status: row.get(16)?,
            total_episodes: row.get(17)?,
            format: row.get(18)?,
            next_episode_number: row.get(19)?,
            next_episode_airs_at: row.get(20)?,
            fetched_at,
            expires_at: row.get(22)?,
            cover_image_url: row.get(23)?,
            description: row.get(24)?,
            score: row.get(25)?,
            studios: row.get(26)?,
            streaming_links_json: row.get(27)?,
        })
    } else {
        None
    };

    let last_completed = engagement_from_join(row, 28, EngagementEvent::Completed, &cr_id)?;
    let last_verified = engagement_from_join(row, 31, EngagementEvent::Verified, &cr_id)?;

    Ok(ResolvedRelease {
        canonical,
        primary_source,
        cache,
        last_completed,
        last_verified,
    })
}

fn engagement_from_join(
    row: &rusqlite::Row<'_>,
    base: usize,
    event: EngagementEvent,
    canonical_id: &CanonicalId,
) -> rusqlite::Result<Option<Engagement>> {
    let id: Option<i64> = row.get(base)?;
    let Some(id) = id else { return Ok(None) };
    let occurred_at: i64 = row.get(base + 1)?;
    let raw_meta: Option<String> = row.get(base + 2)?;
    Ok(Some(Engagement {
        source: EngagementSource::Persisted(id),
        canonical_id: canonical_id.clone(),
        event,
        occurred_at,
        meta: EngagementMeta::decode(event, raw_meta.as_deref()),
    }))
}
