//! LLM prompt builder + response parser for canonicalization.
//!
//! Temperature-0, JSON-output. The prompt is deliberately small and
//! mechanical so it's deterministic across model versions: we list the
//! candidate, we list the neighborhood, we tell the model to pick or
//! create. No few-shot examples — too much room for drift.
//!
//! Response shape (enforced by parser):
//! ```json
//! {"decision":"match","canonical_id":"release:tv:severance","confidence":0.95}
//! {"decision":"new","suggested_slug":"new-show","confidence":0.7}
//! ```

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

use crate::canonical::CanonicalDecision;
use crate::ids::{CanonicalId, ReleaseKind};
use crate::llm::models;
use crate::sources::SourceRecord;
use crate::store::CanonicalRelease;

const SYSTEM: &str = "You are a canonicalization service for a personal media tracker. \
You match incoming source rows (a TV show, anime, film, or music artist from one API) \
against an existing graph of canonical releases. \
Reply with EXACTLY one JSON object, no prose. \
If the candidate matches an existing canonical, reply: \
{\"decision\":\"match\",\"canonical_id\":\"<existing id>\",\"confidence\":<0-1>}. \
If the candidate is a new release not present in the neighborhood, reply: \
{\"decision\":\"new\",\"suggested_slug\":\"<kebab-case-slug>\",\"confidence\":<0-1>}. \
Slugs are ASCII, lowercase, dashes for spaces, no special characters. \
Confidence is your honest assessment, not a fixed value.";

/// Build (system, user) prompt strings for one canonicalization
/// decision. The system prompt is constant — Anthropic can cache it
/// across requests for free.
pub fn build(candidate: &SourceRecord, neighborhood: &[CanonicalRelease]) -> (String, String) {
    let mut user = String::with_capacity(512 + neighborhood.len() * 64);
    user.push_str("CANDIDATE:\n");
    user.push_str(&format!("  source: {}\n", candidate.source));
    user.push_str(&format!("  source_id: {}\n", candidate.source_id));
    user.push_str(&format!("  kind: {}\n", candidate.kind));
    user.push_str(&format!("  title: {}\n", candidate.display_title));
    if candidate.raw_title != candidate.display_title {
        user.push_str(&format!("  raw_title: {}\n", candidate.raw_title));
    }
    if !candidate.aliases.is_empty() {
        user.push_str("  aliases:\n");
        for a in &candidate.aliases {
            user.push_str(&format!("    - {a}\n"));
        }
    }
    user.push('\n');
    user.push_str("NEIGHBORHOOD (existing canonicals to match against):\n");
    let same_kind: Vec<&CanonicalRelease> = neighborhood
        .iter()
        .filter(|cr| cr.kind == candidate.kind)
        .collect();
    if same_kind.is_empty() {
        user.push_str("  (empty — first follow of this kind)\n");
    } else {
        for cr in same_kind {
            user.push_str(&format!("  - {}: {}\n", cr.id, cr.display_title));
        }
    }
    (SYSTEM.to_string(), user)
}

#[derive(Debug, Deserialize)]
struct LlmReply {
    decision: String,
    #[serde(default)]
    canonical_id: Option<String>,
    #[serde(default)]
    suggested_slug: Option<String>,
    confidence: f64,
}

/// Parse the LLM's reply into a typed [`CanonicalDecision`].
/// Tolerant of leading/trailing whitespace and stray newlines; strict
/// about the JSON shape (missing fields → error, not silent default).
pub fn parse_response(text: &str, kind: ReleaseKind) -> Result<CanonicalDecision> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("LLM returned empty text");
    }
    // The model occasionally wraps JSON in ```json fences when it
    // forgets the instructions. Strip them defensively.
    let inner = strip_code_fence(trimmed);
    let reply: LlmReply =
        serde_json::from_str(inner).with_context(|| format!("parse LLM JSON: {inner}"))?;
    let decided_by = format!("llm:{}", models::CANONICALIZE_DEFAULT);
    if !(0.0..=1.0).contains(&reply.confidence) {
        bail!(
            "LLM confidence {} out of [0,1]",
            reply.confidence
        );
    }
    match reply.decision.as_str() {
        "match" => {
            let canonical_id = reply
                .canonical_id
                .ok_or_else(|| anyhow!("LLM decision=match missing canonical_id"))?;
            let canonical_id = CanonicalId::parse(&canonical_id)
                .with_context(|| format!("LLM returned malformed canonical_id: {canonical_id}"))?;
            Ok(CanonicalDecision::Match {
                canonical_id,
                confidence: reply.confidence,
                decided_by,
            })
        }
        "new" => {
            let slug = reply
                .suggested_slug
                .ok_or_else(|| anyhow!("LLM decision=new missing suggested_slug"))?;
            if slug.is_empty() {
                bail!("LLM suggested empty slug");
            }
            Ok(CanonicalDecision::NewCanonical {
                kind,
                suggested_slug: slug,
                confidence: reply.confidence,
                decided_by,
            })
        }
        other => bail!("unknown LLM decision {other:?}"),
    }
}

/// Strip a wrapping ```json … ``` (or plain ``` … ```) fence if
/// present. Returns the inner content trimmed.
fn strip_code_fence(s: &str) -> &str {
    let trimmed = s.trim();
    let after_open = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed);
    let inner = after_open.strip_suffix("```").unwrap_or(after_open);
    inner.trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_emits_candidate_and_neighborhood_sections() {
        let cand = SourceRecord {
            source: "tmdb",
            source_id: "95396".into(),
            kind: ReleaseKind::Tv,
            display_title: "Severance".into(),
            raw_title: "Severance".into(),
            aliases: vec!["切断".into()],
            status: None,
            cover_url: None,
            description: None,
            streaming_links: vec![],
            next_episode_at: None,
        };
        let nb = vec![CanonicalRelease {
            id: CanonicalId::new(ReleaseKind::Tv, "severance").unwrap(),
            kind: ReleaseKind::Tv,
            display_title: "Severance".into(),
            cover_ascii: None,
            cover_color: None,
            followed_at: Some(1),
            dropped_at: None,
            user_note: None,
            created_at: 1,
        }];
        let (system, user) = build(&cand, &nb);
        assert!(system.contains("decision"));
        assert!(user.contains("CANDIDATE"));
        assert!(user.contains("source: tmdb"));
        assert!(user.contains("title: Severance"));
        assert!(user.contains("切断"));
        assert!(user.contains("NEIGHBORHOOD"));
        assert!(user.contains("release:tv:severance"));
    }

    #[test]
    fn build_filters_neighborhood_by_kind() {
        let cand = SourceRecord {
            source: "tmdb",
            source_id: "1".into(),
            kind: ReleaseKind::Tv,
            display_title: "X".into(),
            raw_title: "X".into(),
            aliases: vec![],
            status: None,
            cover_url: None,
            description: None,
            streaming_links: vec![],
            next_episode_at: None,
        };
        let nb = vec![CanonicalRelease {
            id: CanonicalId::new(ReleaseKind::Anime, "y").unwrap(),
            kind: ReleaseKind::Anime,
            display_title: "Y".into(),
            cover_ascii: None,
            cover_color: None,
            followed_at: Some(1),
            dropped_at: None,
            user_note: None,
            created_at: 1,
        }];
        let (_, user) = build(&cand, &nb);
        assert!(!user.contains("release:anime:y"), "cross-kind row leaked: {user}");
        assert!(user.contains("(empty"), "expected the empty marker when no same-kind rows: {user}");
    }

    #[test]
    fn parse_match_response() {
        let text = r#"{"decision":"match","canonical_id":"release:tv:severance","confidence":0.93}"#;
        let d = parse_response(text, ReleaseKind::Tv).unwrap();
        match d {
            CanonicalDecision::Match {
                canonical_id,
                confidence,
                decided_by,
            } => {
                assert_eq!(canonical_id.slug(), "severance");
                assert!((confidence - 0.93).abs() < 1e-6);
                assert!(decided_by.starts_with("llm:"));
            }
            _ => panic!("expected Match"),
        }
    }

    #[test]
    fn parse_new_response() {
        let text = r#"{"decision":"new","suggested_slug":"new-show","confidence":0.7}"#;
        let d = parse_response(text, ReleaseKind::Tv).unwrap();
        match d {
            CanonicalDecision::NewCanonical {
                suggested_slug,
                confidence,
                kind,
                ..
            } => {
                assert_eq!(suggested_slug, "new-show");
                assert!((confidence - 0.7).abs() < 1e-6);
                assert_eq!(kind, ReleaseKind::Tv);
            }
            _ => panic!("expected NewCanonical"),
        }
    }

    #[test]
    fn parse_strips_code_fence() {
        let text = "```json\n{\"decision\":\"new\",\"suggested_slug\":\"x\",\"confidence\":0.5}\n```";
        assert!(parse_response(text, ReleaseKind::Tv).is_ok());
    }

    #[test]
    fn parse_rejects_empty_string() {
        let err = parse_response("", ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err}").contains("empty"));
    }

    #[test]
    fn parse_rejects_non_json() {
        let err = parse_response("nope", ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err:#}").contains("parse"));
    }

    #[test]
    fn parse_rejects_missing_canonical_id_in_match() {
        let text = r#"{"decision":"match","confidence":0.9}"#;
        let err = parse_response(text, ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err}").contains("canonical_id"));
    }

    #[test]
    fn parse_rejects_missing_slug_in_new() {
        let text = r#"{"decision":"new","confidence":0.9}"#;
        let err = parse_response(text, ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err}").contains("suggested_slug"));
    }

    #[test]
    fn parse_rejects_unknown_decision() {
        let text = r#"{"decision":"maybe","confidence":0.5}"#;
        let err = parse_response(text, ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err}").contains("unknown LLM decision"));
    }

    #[test]
    fn parse_rejects_confidence_out_of_bounds() {
        let text = r#"{"decision":"new","suggested_slug":"x","confidence":1.5}"#;
        let err = parse_response(text, ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err}").contains("confidence"));
    }

    #[test]
    fn parse_rejects_malformed_canonical_id() {
        let text = r#"{"decision":"match","canonical_id":"not-a-canonical-id","confidence":0.9}"#;
        let err = parse_response(text, ReleaseKind::Tv).unwrap_err();
        assert!(format!("{err:#}").contains("malformed"));
    }
}
