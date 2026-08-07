//! The exact GraphQL documents Animesh sends.
//!
//! These are part of the frozen contract: the fields requested here are
//! precisely the fields `V0001` stores, no more. Asking for data we discard
//! costs the source's rate limit and invites a schema that drifts toward
//! whatever AniList happens to expose.

/// Maximum search results requested in one call.
pub const SEARCH_PER_PAGE: u32 = 10;

/// Maximum AniList IDs in one batch refresh.
pub const MAX_BATCH_IDS: usize = 50;

/// Fields shared by every media selection.
///
/// `timeUntilAiring` is deliberately absent. Presentation derives the relative
/// duration from `airingAt` and the injected clock; storing the source's own
/// countdown would only create a second, drifting truth.
const MEDIA_FIELDS: &str = "
    id
    title { romaji english native }
    status
    episodes
    format
    seasonYear
";

/// One media item by AniList ID.
pub fn detail() -> String {
    format!(
        "query ($id: Int) {{
  Media(id: $id, type: ANIME) {{{MEDIA_FIELDS}
    nextAiringEpisode {{ episode airingAt }}
  }}
}}"
    )
}

/// Many media items by AniList ID.
///
/// `perPage` is bound to the requested ID count by the caller so a partial page
/// is never silently truncated into an apparent omission.
pub fn batch() -> String {
    format!(
        "query ($ids: [Int], $perPage: Int) {{
  Page(page: 1, perPage: $perPage) {{
    media(id_in: $ids, type: ANIME) {{{MEDIA_FIELDS}
      nextAiringEpisode {{ episode airingAt }}
    }}
  }}
}}"
    )
}

/// Free-text search.
///
/// No `nextAiringEpisode`: search results are transient and never become
/// evidence, so the schedule fields would be fetched and thrown away.
pub fn search() -> String {
    format!(
        "query ($search: String, $perPage: Int) {{
  Page(perPage: $perPage) {{
    media(search: $search, type: ANIME, sort: SEARCH_MATCH) {{{MEDIA_FIELDS}
    }}
  }}
}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_requests_the_schedule() {
        let query = detail();
        assert!(query.contains("nextAiringEpisode"));
        assert!(query.contains("airingAt"));
        assert!(query.contains("type: ANIME"));
    }

    #[test]
    fn batch_pins_page_one_and_uses_id_in() {
        let query = batch();
        assert!(query.contains("page: 1"));
        assert!(query.contains("id_in: $ids"));
        assert!(query.contains("nextAiringEpisode"));
    }

    #[test]
    fn search_omits_schedule_fields() {
        // Search never becomes evidence, so fetching schedule data for it would
        // spend rate limit on values that are immediately discarded.
        assert!(!search().contains("nextAiringEpisode"));
        assert!(search().contains("SEARCH_MATCH"));
    }

    #[test]
    fn no_query_requests_time_until_airing() {
        // Never persisted, so never fetched.
        for query in [detail(), batch(), search()] {
            assert!(
                !query.contains("timeUntilAiring"),
                "query still requests timeUntilAiring:\n{query}"
            );
        }
    }

    #[test]
    fn every_query_requests_all_stored_fields() {
        for query in [detail(), batch(), search()] {
            for field in [
                "id",
                "romaji",
                "english",
                "native",
                "status",
                "episodes",
                "format",
                "seasonYear",
            ] {
                assert!(query.contains(field), "query is missing {field}:\n{query}");
            }
        }
    }
}
