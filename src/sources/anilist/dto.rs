//! Wire shapes exactly as AniList sends them.
//!
//! Nothing here validates or normalizes; that is [`super::parser`]'s job. These
//! types exist to get bytes into Rust without losing information, so almost
//! every field is `Option` even where the schema nominally guarantees a value.
//!
//! That permissiveness is deliberate. GraphQL nulls any field it could not
//! resolve and reports the reason in `errors`, so a strict struct would fail to
//! deserialize the whole page and take every healthy sibling item down with the
//! one that failed. The opposite is what is needed: accept the valid items and
//! classify the invalid ones individually.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQlEnvelope<T> {
    pub data: Option<T>,
    #[serde(default)]
    pub errors: Vec<GraphQlError>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphQlError {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub status: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailData {
    #[serde(rename = "Media")]
    pub media: Option<Media>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PageData {
    #[serde(rename = "Page")]
    pub page: Option<MediaPage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaPage {
    #[serde(default)]
    pub media: Vec<Option<Media>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Media {
    pub id: Option<i64>,
    pub title: Option<MediaTitle>,
    pub status: Option<String>,
    pub format: Option<String>,
    pub episodes: Option<i64>,
    #[serde(rename = "seasonYear")]
    pub season_year: Option<i64>,
    #[serde(rename = "nextAiringEpisode", default)]
    pub next_airing_episode: Option<NextAiringEpisode>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MediaTitle {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct NextAiringEpisode {
    pub episode: Option<i64>,
    #[serde(rename = "airingAt")]
    pub airing_at: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_envelope_parses() {
        let body = r#"{"data":{"Media":{"id":21,"title":{"romaji":"ONE PIECE","english":"One Piece","native":"ワンピース"},"status":"RELEASING","episodes":null,"format":"TV","seasonYear":1999,"nextAiringEpisode":{"episode":1169,"airingAt":1783865760}}}}"#;
        let envelope: GraphQlEnvelope<DetailData> =
            serde_json::from_str(body).expect("parse detail");
        let media = envelope.data.and_then(|d| d.media).expect("media present");
        assert_eq!(media.id, Some(21));
        assert_eq!(
            media.next_airing_episode.and_then(|n| n.episode),
            Some(1169)
        );
    }

    #[test]
    fn explicit_null_media_parses_as_absent() {
        // AniList's not-found shape: HTTP 404, but a well-formed envelope.
        let body = r#"{"errors":[{"message":"Not Found.","status":404}],"data":{"Media":null}}"#;
        let envelope: GraphQlEnvelope<DetailData> = serde_json::from_str(body).expect("parse");
        assert!(envelope.data.expect("data present").media.is_none());
        assert_eq!(envelope.errors.len(), 1);
        assert_eq!(envelope.errors[0].status, Some(404));
    }

    #[test]
    fn a_null_item_does_not_fail_the_whole_page() {
        // The point of the Option-heavy shapes: one unresolvable item must not
        // cost us the healthy ones beside it.
        let body = r#"{"data":{"Page":{"media":[{"id":21,"title":{"romaji":"ONE PIECE","english":null,"native":null},"status":"RELEASING","episodes":null,"format":"TV","seasonYear":1999,"nextAiringEpisode":null},null]}},"errors":[{"message":"Internal error"}]}"#;
        let envelope: GraphQlEnvelope<PageData> = serde_json::from_str(body).expect("parse page");
        let media = envelope
            .data
            .and_then(|d| d.page)
            .expect("page present")
            .media;
        assert_eq!(media.len(), 2);
        assert!(media[0].is_some());
        assert!(media[1].is_none());
    }

    #[test]
    fn an_item_with_a_nulled_id_still_parses() {
        // A strict `id: i64` would reject the entire response here.
        let body = r#"{"data":{"Page":{"media":[{"id":null,"title":null,"status":null,"episodes":null,"format":null,"seasonYear":null,"nextAiringEpisode":null}]}}}"#;
        let envelope: GraphQlEnvelope<PageData> = serde_json::from_str(body).expect("parse");
        let media = envelope.data.and_then(|d| d.page).expect("page").media;
        assert_eq!(media[0].as_ref().expect("item present").id, None);
    }

    #[test]
    fn data_null_with_errors_parses() {
        let body = r#"{"data":null,"errors":[{"message":"Validation error"}]}"#;
        let envelope: GraphQlEnvelope<PageData> = serde_json::from_str(body).expect("parse");
        assert!(envelope.data.is_none());
        assert_eq!(envelope.errors[0].message, "Validation error");
    }

    #[test]
    fn missing_errors_key_defaults_to_empty() {
        let body = r#"{"data":{"Media":null}}"#;
        let envelope: GraphQlEnvelope<DetailData> = serde_json::from_str(body).expect("parse");
        assert!(envelope.errors.is_empty());
    }

    #[test]
    fn a_missing_next_airing_key_is_absent_not_an_error() {
        let body = r#"{"data":{"Media":{"id":1,"title":null,"status":"FINISHED","episodes":12,"format":"TV","seasonYear":2020}}}"#;
        let envelope: GraphQlEnvelope<DetailData> = serde_json::from_str(body).expect("parse");
        let media = envelope.data.and_then(|d| d.media).expect("media");
        assert!(media.next_airing_episode.is_none());
    }
}
