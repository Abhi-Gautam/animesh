//! MusicBrainz parser for artist identity observations.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::ids::ReleaseKind;
use crate::ingest::{
    AliasObservation, ExternalIdObservation, LinkObservation, RawSourcePayload, SourceObservation,
    SourceParser,
};

pub struct MusicBrainzParser;

impl SourceParser for MusicBrainzParser {
    fn source(&self) -> &'static str {
        "musicbrainz"
    }

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>> {
        let resp: ArtistSearchResponse = serde_json::from_str(&payload.response_json)
            .context("parse MusicBrainz artist search response")?;
        resp.artists
            .into_iter()
            .map(|artist| artist_to_observation(artist, payload))
            .collect()
    }

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>> {
        let artist: Artist = serde_json::from_str(&payload.response_json)
            .context("parse MusicBrainz artist response")?;
        Ok(Some(artist_to_observation(artist, payload)?))
    }
}

#[derive(Debug, Deserialize)]
struct ArtistSearchResponse {
    #[serde(default)]
    artists: Vec<Artist>,
}

#[derive(Debug, Deserialize)]
struct Artist {
    id: String,
    name: String,
    #[serde(rename = "sort-name", default)]
    sort_name: Option<String>,
    #[serde(default)]
    disambiguation: Option<String>,
    #[serde(rename = "type", default)]
    artist_type: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    aliases: Vec<ArtistAlias>,
    #[serde(default)]
    tags: Vec<Tag>,
    #[serde(default)]
    relations: Vec<Relation>,
    #[serde(rename = "life-span", default)]
    life_span: Option<LifeSpan>,
}

#[derive(Debug, Deserialize)]
struct ArtistAlias {
    name: String,
    #[serde(default)]
    locale: Option<String>,
    #[serde(rename = "type", default)]
    alias_type: Option<String>,
    #[serde(default)]
    primary: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct Tag {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Relation {
    #[serde(rename = "type", default)]
    relation_type: Option<String>,
    #[serde(default)]
    url: Option<RelationUrl>,
}

#[derive(Debug, Deserialize)]
struct RelationUrl {
    resource: String,
}

#[derive(Debug, Deserialize)]
struct LifeSpan {
    #[serde(default)]
    ended: Option<bool>,
}

fn artist_to_observation(artist: Artist, payload: &RawSourcePayload) -> Result<SourceObservation> {
    let mut aliases = Vec::new();
    aliases.push(AliasObservation {
        alias: artist.name.clone(),
        locale: artist.country.clone(),
        alias_kind: Some("primary".into()),
        confidence: 1.0,
    });
    if let Some(sort_name) = artist.sort_name.as_ref().filter(|s| *s != &artist.name) {
        aliases.push(AliasObservation {
            alias: sort_name.clone(),
            locale: None,
            alias_kind: Some("sort_name".into()),
            confidence: 0.85,
        });
    }
    for alias in &artist.aliases {
        aliases.push(AliasObservation {
            alias: alias.name.clone(),
            locale: alias.locale.clone(),
            alias_kind: alias.alias_type.clone().or_else(|| Some("alias".into())),
            confidence: if alias.primary.unwrap_or(false) {
                0.95
            } else {
                0.75
            },
        });
    }
    for tag in &artist.tags {
        aliases.push(AliasObservation {
            alias: tag.name.clone(),
            locale: None,
            alias_kind: Some("tag".into()),
            confidence: 0.45,
        });
    }

    let links = artist
        .relations
        .iter()
        .filter_map(|relation| {
            relation.url.as_ref().map(|url| LinkObservation {
                site: relation
                    .relation_type
                    .clone()
                    .unwrap_or_else(|| "external".into()),
                url: url.resource.clone(),
                link_kind: relation.relation_type.clone(),
            })
        })
        .collect();

    let mut description_parts = Vec::new();
    if let Some(disambiguation) = &artist.disambiguation {
        if !disambiguation.trim().is_empty() {
            description_parts.push(disambiguation.clone());
        }
    }
    if let Some(artist_type) = &artist.artist_type {
        description_parts.push(artist_type.clone());
    }
    if let Some(country) = &artist.country {
        description_parts.push(country.clone());
    }

    let status = artist
        .life_span
        .as_ref()
        .and_then(|life| life.ended)
        .map(|ended| if ended { "ended" } else { "active" }.to_string());

    Ok(SourceObservation {
        source: "musicbrainz".into(),
        source_id: artist.id.clone(),
        raw_payload_id: payload.id.clone(),
        kind: ReleaseKind::MusicArtist,
        display_title: artist.name,
        raw_title: artist.sort_name,
        description: if description_parts.is_empty() {
            None
        } else {
            Some(description_parts.join(" · "))
        },
        status,
        observed_at: payload.fetched_at,
        source_updated_at: None,
        aliases,
        external_ids: vec![ExternalIdObservation {
            id_kind: "musicbrainz_artist".into(),
            id_value: artist.id,
            confidence: 1.0,
        }],
        release_events: vec![],
        links,
        images: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::HttpMethod;

    fn raw(body: &str) -> RawSourcePayload {
        RawSourcePayload {
            id: "raw:mb:1".into(),
            source: "musicbrainz".into(),
            endpoint: "artist_search".into(),
            method: HttpMethod::Get,
            request_key: "musicbrainz:artist:radiohead".into(),
            request_hash: "req".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp".into(),
            response_json: body.into(),
            fetched_at: 1_000,
            expires_at: None,
            created_at: 1_000,
        }
    }

    #[test]
    fn parses_artist_search_identity_aliases_tags_and_links() {
        let body = r#"{
            "artists": [{
                "id": "a74b1b7f-71a5-4011-9441-d0b5e4122711",
                "name": "Radiohead",
                "sort-name": "Radiohead",
                "type": "Group",
                "country": "GB",
                "disambiguation": "English alternative rock band",
                "life-span": {"ended": false},
                "aliases": [{"name": "On A Friday", "locale": "en", "type": "Artist name", "primary": false}],
                "tags": [{"name": "alternative rock"}],
                "relations": [{"type": "official homepage", "url": {"resource": "https://www.radiohead.com/"}}]
            }]
        }"#;

        let out = MusicBrainzParser.parse_search(&raw(body)).unwrap();
        assert_eq!(out.len(), 1);
        let artist = &out[0];
        assert_eq!(artist.kind, ReleaseKind::MusicArtist);
        assert_eq!(artist.display_title, "Radiohead");
        assert_eq!(artist.status.as_deref(), Some("active"));
        assert!(artist.aliases.iter().any(|a| a.alias == "On A Friday"));
        assert!(artist.aliases.iter().any(|a| a.alias == "alternative rock"));
        assert!(artist
            .links
            .iter()
            .any(|l| l.url == "https://www.radiohead.com/"));
        assert!(artist
            .external_ids
            .iter()
            .any(|e| e.id_kind == "musicbrainz_artist"));
    }

    #[test]
    fn parses_single_artist_fetch() {
        let body = r#"{
            "id": "1f9df192-a621-4f54-8850-2c5373b7eac9",
            "name": "Taylor Swift",
            "sort-name": "Swift, Taylor",
            "type": "Person",
            "country": "US"
        }"#;

        let obs = MusicBrainzParser.parse_fetch(&raw(body)).unwrap().unwrap();
        assert_eq!(obs.source_id, "1f9df192-a621-4f54-8850-2c5373b7eac9");
        assert_eq!(obs.display_title, "Taylor Swift");
        assert_eq!(obs.raw_title.as_deref(), Some("Swift, Taylor"));
    }
}
