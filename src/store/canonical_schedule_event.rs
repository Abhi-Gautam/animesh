use anyhow::{Context, Result};
use rusqlite::params;

use crate::ids::CanonicalId;
use crate::ingest::{ReleaseEventObservation, TimePrecision};

use super::Db;

#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalScheduleEvent {
    pub id: String,
    pub canonical_id: CanonicalId,
    pub source: String,
    pub source_event_id: String,
    pub event_kind: String,
    pub title: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub local_date: Option<String>,
    pub local_time: Option<String>,
    pub source_timezone: Option<String>,
    pub scheduled_at: Option<i64>,
    pub precision: TimePrecision,
    pub confidence: f64,
    pub observed_at: i64,
    pub superseded_at: Option<i64>,
}

impl Db {
    pub fn upsert_canonical_schedule_events(
        &self,
        canonical_id: &CanonicalId,
        source: &str,
        events: &[ReleaseEventObservation],
    ) -> Result<()> {
        for ev in events {
            let id = canonical_schedule_event_id(canonical_id, &ev.id);
            self.conn()
                .execute(
                    "INSERT INTO canonical_schedule_event (
                        id, canonical_id, source, source_event_id, event_kind, title,
                        season, episode, local_date, local_time, source_timezone,
                        scheduled_at, precision, confidence, observed_at, superseded_at
                     ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)
                     ON CONFLICT(canonical_id, source, source_event_id) DO UPDATE SET
                        event_kind = excluded.event_kind,
                        title = excluded.title,
                        season = excluded.season,
                        episode = excluded.episode,
                        local_date = excluded.local_date,
                        local_time = excluded.local_time,
                        source_timezone = excluded.source_timezone,
                        scheduled_at = excluded.scheduled_at,
                        precision = excluded.precision,
                        confidence = excluded.confidence,
                        observed_at = excluded.observed_at,
                        superseded_at = NULL",
                    params![
                        id,
                        canonical_id,
                        source,
                        ev.id,
                        ev.event_kind,
                        ev.title,
                        ev.season,
                        ev.episode,
                        ev.local_date,
                        ev.local_time,
                        ev.source_timezone,
                        ev.scheduled_at,
                        ev.precision.as_str(),
                        ev.confidence,
                        ev.observed_at,
                        Option::<i64>::None,
                    ],
                )
                .with_context(|| format!("upsert canonical_schedule_event {}", ev.id))?;
        }
        Ok(())
    }

    pub fn schedule_events_for_canonical(
        &self,
        canonical_id: &CanonicalId,
    ) -> Result<Vec<CanonicalScheduleEvent>> {
        let mut stmt = self
            .conn()
            .prepare(
                "SELECT id, canonical_id, source, source_event_id, event_kind, title,
                        season, episode, local_date, local_time, source_timezone,
                        scheduled_at, precision, confidence, observed_at, superseded_at
                 FROM canonical_schedule_event
                 WHERE canonical_id = ?1 AND superseded_at IS NULL
                 ORDER BY scheduled_at IS NULL, scheduled_at, source_event_id",
            )
            .context("prepare schedule_events_for_canonical")?;
        let rows = stmt
            .query_map(params![canonical_id], |row| {
                let precision_s: String = row.get(12)?;
                let precision = parse_precision(&precision_s).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        12,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e)),
                    )
                })?;
                Ok(CanonicalScheduleEvent {
                    id: row.get(0)?,
                    canonical_id: row.get(1)?,
                    source: row.get(2)?,
                    source_event_id: row.get(3)?,
                    event_kind: row.get(4)?,
                    title: row.get(5)?,
                    season: row.get(6)?,
                    episode: row.get(7)?,
                    local_date: row.get(8)?,
                    local_time: row.get(9)?,
                    source_timezone: row.get(10)?,
                    scheduled_at: row.get(11)?,
                    precision,
                    confidence: row.get(13)?,
                    observed_at: row.get(14)?,
                    superseded_at: row.get(15)?,
                })
            })
            .context("query schedule_events_for_canonical")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect schedule_events_for_canonical")
    }
}

pub fn canonical_schedule_event_id(canonical_id: &CanonicalId, source_event_id: &str) -> String {
    format!(
        "canonical_schedule:{}:{}",
        canonical_id.as_str(),
        source_event_id
    )
}

fn parse_precision(s: &str) -> std::result::Result<TimePrecision, String> {
    match s {
        "instant" => Ok(TimePrecision::Instant),
        "date" => Ok(TimePrecision::Date),
        "month" => Ok(TimePrecision::Month),
        "year" => Ok(TimePrecision::Year),
        "unknown" => Ok(TimePrecision::Unknown),
        other => Err(format!("unknown time precision {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReleaseKind;
    use crate::ingest::{HttpMethod, RawSourcePayload, SourceObservation};

    #[test]
    fn schedule_events_project_from_source_events() {
        let mut db = Db::open_in_memory().unwrap();
        let canonical_id = CanonicalId::new(ReleaseKind::Anime, "frieren").unwrap();
        db.upsert_canonical(&canonical_id, ReleaseKind::Anime, "Frieren", 1_000)
            .unwrap();
        db.attach_source_ref(&canonical_id, "tvmaze", "123", Some("Frieren"), 1.0)
            .unwrap();
        let raw = RawSourcePayload {
            id: "raw:tvmaze:123".into(),
            source: "tvmaze".into(),
            endpoint: "show".into(),
            method: HttpMethod::Get,
            request_key: "tvmaze:show:123".into(),
            request_hash: "req".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp".into(),
            response_json: "{}".into(),
            fetched_at: 1_000,
            expires_at: None,
            created_at: 1_000,
        };
        db.upsert_raw_source_payload(&raw).unwrap();
        let event = ReleaseEventObservation {
            id: "tvmaze:episode:1".into(),
            event_kind: "episode".into(),
            title: Some("Journey's End".into()),
            season: Some(1),
            episode: Some(1),
            local_date: Some("2023-09-29".into()),
            local_time: Some("23:00".into()),
            source_timezone: Some("+09:00".into()),
            scheduled_at: Some(1_696_000_000),
            precision: TimePrecision::Instant,
            confidence: 0.95,
            observed_at: 1_000,
        };
        db.upsert_source_observation(&SourceObservation {
            source: "tvmaze".into(),
            source_id: "123".into(),
            raw_payload_id: raw.id.clone(),
            kind: ReleaseKind::Anime,
            display_title: "Frieren".into(),
            raw_title: None,
            description: None,
            status: Some("Running".into()),
            observed_at: 1_000,
            source_updated_at: None,
            aliases: vec![],
            external_ids: vec![],
            release_events: vec![event.clone()],
            links: vec![],
            images: vec![],
        })
        .unwrap();

        db.upsert_canonical_schedule_events(&canonical_id, "tvmaze", &[event])
            .unwrap();
        let projected = db.schedule_events_for_canonical(&canonical_id).unwrap();
        assert_eq!(projected.len(), 1);
        assert_eq!(projected[0].source_event_id, "tvmaze:episode:1");
        assert_eq!(projected[0].episode, Some(1));
        assert_eq!(projected[0].precision, TimePrecision::Instant);
    }
}
