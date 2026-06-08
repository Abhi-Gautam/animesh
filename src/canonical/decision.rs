//! Alias-match fast path for the canonicalization service.
//!
//! Compares a [`SourceRecord`] against the existing followed graph
//! using normalized string equality. Catches the common case where
//! the same show appears across sources with near-identical titles
//! (e.g. "Severance" on TMDB and "SEVERANCE" on AniList) so we don't
//! need to spend an LLM call.
//!
//! v0.5 keeps this deliberately dumb: lowercase + strip non-ASCII +
//! collapse whitespace. The LLM handles anything fuzzier. We can
//! upgrade to embedding-distance later (a similarity index over
//! display_title + aliases) without changing the public shape.

use crate::ids::CanonicalId;
use crate::sources::SourceRecord;
use crate::store::CanonicalRelease;

/// One alias-match hit.
#[derive(Debug, Clone, PartialEq)]
pub struct AliasMatch {
    pub canonical_id: CanonicalId,
    /// 0.0–1.0. 0.95 for exact normalized title match; 0.85 for an
    /// alias match. The caller's threshold decides whether to commit
    /// without going to the LLM.
    pub confidence: f64,
}

/// Try to alias-match a candidate against an existing canonical.
///
/// Returns the best (highest-confidence) hit, or None if nothing in
/// the neighborhood normalizes to the same shape.
pub fn try_alias_match(
    candidate: &SourceRecord,
    neighborhood: &[CanonicalRelease],
) -> Option<AliasMatch> {
    let cand_keys: Vec<String> = std::iter::once(candidate.display_title.as_str())
        .chain(std::iter::once(candidate.raw_title.as_str()))
        .chain(candidate.aliases.iter().map(String::as_str))
        .filter_map(normalize)
        .collect();
    if cand_keys.is_empty() {
        return None;
    }

    let mut best: Option<AliasMatch> = None;
    for cr in neighborhood {
        // Only match within the same kind.
        if cr.kind != candidate.kind {
            continue;
        }
        // Exact normalized display_title match — strongest signal.
        if let Some(target) = normalize(&cr.display_title) {
            if cand_keys.iter().any(|k| k == &target) {
                update_best(
                    &mut best,
                    AliasMatch {
                        canonical_id: cr.id.clone(),
                        confidence: 0.95,
                    },
                );
            }
        }
        // Slug match — weaker (slugs can collide across kinds).
        // Normalize the slug through the same pipeline so
        // dashes/underscores align: "better-call-saul" → "better call saul".
        if let Some(slug_normalized) = normalize(cr.id.slug()) {
            if cand_keys.iter().any(|k| k == &slug_normalized) {
                update_best(
                    &mut best,
                    AliasMatch {
                        canonical_id: cr.id.clone(),
                        confidence: 0.88,
                    },
                );
            }
        }
    }
    best
}

fn update_best(best: &mut Option<AliasMatch>, candidate: AliasMatch) {
    match best {
        Some(existing) if existing.confidence >= candidate.confidence => {}
        _ => *best = Some(candidate),
    }
}

/// Normalize a title for comparison: ASCII lowercase, drop punctuation
/// + symbols, treat dashes and underscores as space-equivalents,
/// collapse runs of whitespace to a single space. Returns None for
/// empty strings.
///
/// Dash-as-space lets us match "better-call-saul" (a slug form) against
/// "Better Call Saul" (a display title) — both normalize to
/// "better call saul".
fn normalize(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = true; // suppress leading whitespace
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_was_space = false;
        } else if c.is_whitespace() || c == '-' || c == '_' {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        }
        // Other punctuation/symbols: drop.
    }
    while out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReleaseKind;

    fn cr(slug: &str, title: &str, kind: ReleaseKind) -> CanonicalRelease {
        CanonicalRelease {
            id: CanonicalId::new(kind, slug).unwrap(),
            kind,
            display_title: title.to_string(),
            cover_ascii: None,
            cover_color: None,
            followed_at: Some(1),
            dropped_at: None,
            user_note: None,
            created_at: 1,
        }
    }

    fn rec(title: &str, kind: ReleaseKind) -> SourceRecord {
        SourceRecord {
            source: "tmdb",
            source_id: "1".into(),
            kind,
            display_title: title.to_string(),
            raw_title: title.to_string(),
            aliases: vec![],
            status: None,
            cover_url: None,
            description: None,
            streaming_links: vec![],
            next_episode_at: None,
        }
    }

    #[test]
    fn empty_neighborhood_returns_none() {
        assert!(try_alias_match(&rec("Severance", ReleaseKind::Tv), &[]).is_none());
    }

    #[test]
    fn exact_title_match_returns_high_confidence() {
        let nb = vec![cr("severance", "Severance", ReleaseKind::Tv)];
        let hit = try_alias_match(&rec("Severance", ReleaseKind::Tv), &nb).unwrap();
        assert_eq!(hit.canonical_id.slug(), "severance");
        assert!(hit.confidence >= 0.95);
    }

    #[test]
    fn case_and_punctuation_differences_still_match() {
        let nb = vec![cr("severance", "Severance", ReleaseKind::Tv)];
        let hit = try_alias_match(&rec("SEVERANCE!", ReleaseKind::Tv), &nb);
        assert!(hit.is_some());
        let hit = try_alias_match(&rec("  severance ", ReleaseKind::Tv), &nb);
        assert!(hit.is_some());
    }

    #[test]
    fn cross_kind_does_not_match() {
        let nb = vec![cr("severance", "Severance", ReleaseKind::Anime)];
        let hit = try_alias_match(&rec("Severance", ReleaseKind::Tv), &nb);
        assert!(hit.is_none(), "kinds must match");
    }

    #[test]
    fn slug_match_works_for_dashed_titles() {
        let nb = vec![cr("better-call-saul", "Better Call Saul", ReleaseKind::Tv)];
        let hit = try_alias_match(&rec("better-call-saul", ReleaseKind::Tv), &nb).unwrap();
        assert_eq!(hit.canonical_id.slug(), "better-call-saul");
        assert!(hit.confidence >= 0.85);
    }

    #[test]
    fn alias_field_contributes_to_match() {
        let nb = vec![cr("attack-on-titan", "Attack on Titan", ReleaseKind::Anime)];
        let mut record = rec("Shingeki no Kyojin", ReleaseKind::Anime);
        record.aliases = vec!["Attack on Titan".to_string()];
        let hit = try_alias_match(&record, &nb);
        assert!(hit.is_some(), "alias should hit display_title match");
    }

    #[test]
    fn picks_highest_confidence_when_multiple_hits() {
        let nb = vec![
            cr("severance", "Severance", ReleaseKind::Tv),
            cr("severance-x", "Severance X", ReleaseKind::Tv),
        ];
        let hit = try_alias_match(&rec("Severance", ReleaseKind::Tv), &nb).unwrap();
        // Exact wins over near.
        assert_eq!(hit.canonical_id.slug(), "severance");
    }

    #[test]
    fn no_match_for_unrelated_titles() {
        let nb = vec![cr("severance", "Severance", ReleaseKind::Tv)];
        let hit = try_alias_match(&rec("Better Call Saul", ReleaseKind::Tv), &nb);
        assert!(hit.is_none());
    }

    #[test]
    fn normalize_drops_punctuation_and_collapses_whitespace() {
        assert_eq!(normalize("Severance"), Some("severance".into()));
        assert_eq!(normalize("  Severance  "), Some("severance".into()));
        assert_eq!(normalize("S.e.v.e.r.a.n.c.e"), Some("severance".into()));
        assert_eq!(normalize("Severance!!!"), Some("severance".into()));
        assert_eq!(normalize("severance  "), Some("severance".into()));
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("!!!"), None);
        assert_eq!(normalize("Better Call Saul"), Some("better call saul".into()));
    }
}
