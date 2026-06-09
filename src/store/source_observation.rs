use anyhow::{Context, Result};
use rusqlite::params;

use crate::ingest::SourceObservation;
use crate::store::source_candidate::SourceCandidate;

use super::Db;

impl Db {
    pub fn upsert_source_observation(&mut self, obs: &SourceObservation) -> Result<()> {
        let tx = self
            .conn_mut()
            .transaction()
            .context("source observation tx")?;
        tx.execute(
            "INSERT INTO source_observation (
                source, source_id, raw_payload_id, kind, display_title, raw_title,
                description, status, observed_at, source_updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(source, source_id) DO UPDATE SET
                raw_payload_id = excluded.raw_payload_id,
                kind = excluded.kind,
                display_title = excluded.display_title,
                raw_title = excluded.raw_title,
                description = excluded.description,
                status = excluded.status,
                observed_at = excluded.observed_at,
                source_updated_at = excluded.source_updated_at",
            params![
                obs.source,
                obs.source_id,
                obs.raw_payload_id,
                obs.kind.as_str(),
                obs.display_title,
                obs.raw_title,
                obs.description,
                obs.status,
                obs.observed_at,
                obs.source_updated_at,
            ],
        )?;

        for table in [
            "source_alias_observation",
            "external_id_observation",
            "release_event_observation",
            "link_observation",
            "image_observation",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE source = ?1 AND source_id = ?2"),
                params![obs.source, obs.source_id],
            )?;
        }

        for a in &obs.aliases {
            tx.execute(
                "INSERT INTO source_alias_observation
                    (source, source_id, alias, locale, alias_kind, confidence)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    obs.source,
                    obs.source_id,
                    a.alias,
                    a.locale,
                    a.alias_kind,
                    a.confidence
                ],
            )?;
        }
        for e in &obs.external_ids {
            tx.execute(
                "INSERT INTO external_id_observation
                    (source, source_id, id_kind, id_value, confidence)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    obs.source,
                    obs.source_id,
                    e.id_kind,
                    e.id_value,
                    e.confidence
                ],
            )?;
        }
        for ev in &obs.release_events {
            tx.execute(
                "INSERT INTO release_event_observation
                    (id, source, source_id, event_kind, title, season, episode,
                     local_date, local_time, source_timezone, scheduled_at, precision,
                     confidence, observed_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    ev.id,
                    obs.source,
                    obs.source_id,
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
                ],
            )?;
        }
        for l in &obs.links {
            tx.execute(
                "INSERT INTO link_observation (source, source_id, site, url, link_kind)
                 VALUES (?1,?2,?3,?4,?5)",
                params![obs.source, obs.source_id, l.site, l.url, l.link_kind],
            )?;
        }
        for i in &obs.images {
            tx.execute(
                "INSERT INTO image_observation (source, source_id, image_kind, url, width, height)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    obs.source,
                    obs.source_id,
                    i.image_kind,
                    i.url,
                    i.width,
                    i.height
                ],
            )?;
        }

        let search_text = build_search_text(obs);
        let candidate = SourceCandidate {
            source: obs.source.clone(),
            source_id: obs.source_id.clone(),
            kind: obs.kind,
            display_title: obs.display_title.clone(),
            search_text,
            first_seen_at: obs.observed_at,
            last_seen_at: obs.observed_at,
            expires_at: None,
            score_hint: None,
        };
        tx.execute(
            "INSERT INTO source_candidate (
                source, source_id, kind, display_title, search_text,
                first_seen_at, last_seen_at, expires_at, score_hint
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(source, source_id) DO UPDATE SET
                kind = excluded.kind,
                display_title = excluded.display_title,
                search_text = excluded.search_text,
                last_seen_at = excluded.last_seen_at",
            params![
                candidate.source,
                candidate.source_id,
                candidate.kind.as_str(),
                candidate.display_title,
                candidate.search_text,
                candidate.first_seen_at,
                candidate.last_seen_at,
                candidate.expires_at,
                candidate.score_hint,
            ],
        )?;

        tx.commit().context("commit source observation tx")?;
        Ok(())
    }
}

fn build_search_text(obs: &SourceObservation) -> String {
    let mut parts = vec![obs.display_title.clone()];
    if let Some(raw) = &obs.raw_title {
        parts.push(raw.clone());
    }
    parts.extend(obs.aliases.iter().map(|a| a.alias.clone()));
    parts.extend(
        obs.external_ids
            .iter()
            .map(|e| format!("{}:{}", e.id_kind, e.id_value)),
    );
    parts.join(" ")
}
