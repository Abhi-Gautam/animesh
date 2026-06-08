//! Canonicalization service.
//!
//! Given a [`SourceRecord`] from any adapter, decide whether it maps
//! to an existing [`CanonicalRelease`] or needs a new canonical id.
//! Three-tier decision flow:
//!
//!   1. **Cache** ([`Library::cached_canonical_for`]) — instant hit for
//!      anything previously decided. Idempotent re-runs are free.
//!   2. **Alias-match** ([`decision::try_alias_match`]) — local string
//!      comparison against the existing followed graph. Catches the
//!      common case where the same show appears across sources with
//!      near-identical titles.
//!   3. **LLM** — last resort. Temperature-0, prompt-cached. Builds a
//!      JSON prompt with the candidate + neighborhood, parses the
//!      typed reply. The LLM decides match-or-new.
//!
//! The service NEVER writes follow state. It produces a
//! [`CanonicalDecision`]; the caller (CLI command, TUI handler, sync
//! loop) writes via [`Library::follow_with_source`].

pub mod decision;
mod prompt;

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::ids::{CanonicalId, ReleaseKind};
use crate::library::Library;
use crate::llm::{models, LlmClient, LlmRequest};
use crate::sources::SourceRecord;
use crate::store::CanonicalRelease;

pub use decision::try_alias_match;

/// The canonical service's verdict on a source record.
#[derive(Debug, Clone, PartialEq)]
pub enum CanonicalDecision {
    /// Source row resolves to an existing canonical.
    Match {
        canonical_id: CanonicalId,
        confidence: f64,
        decided_by: String,
    },
    /// Source row is new. The service proposes a slug; the caller
    /// builds the CanonicalId and creates the canonical_release row.
    NewCanonical {
        kind: ReleaseKind,
        suggested_slug: String,
        confidence: f64,
        decided_by: String,
    },
}

impl CanonicalDecision {
    /// True if the LLM (or alias-match) said "this matches".
    pub fn is_match(&self) -> bool {
        matches!(self, Self::Match { .. })
    }

    pub fn confidence(&self) -> f64 {
        match self {
            Self::Match { confidence, .. } => *confidence,
            Self::NewCanonical { confidence, .. } => *confidence,
        }
    }

    pub fn decided_by(&self) -> &str {
        match self {
            Self::Match { decided_by, .. } => decided_by,
            Self::NewCanonical { decided_by, .. } => decided_by,
        }
    }
}

/// Orchestrates the three-tier decision flow.
///
/// Holds an `Arc<Library>` and an `Arc<dyn LlmClient>` so it can be
/// constructed once and shared across CLI / TUI / sync.
pub struct CanonicalizationService {
    library: Arc<Library>,
    llm: Arc<dyn LlmClient>,
    /// Confidence threshold above which the cache+alias path commits
    /// without consulting the LLM. Default 0.85.
    pub alias_match_threshold: f64,
}

impl CanonicalizationService {
    pub fn new(library: Arc<Library>, llm: Arc<dyn LlmClient>) -> Self {
        Self {
            library,
            llm,
            alias_match_threshold: 0.85,
        }
    }

    /// Decide what canonical this source record maps to.
    ///
    /// `neighborhood` is the set of canonicals the LLM can choose from
    /// when alias-match misses. Typically: every followed canonical of
    /// the same kind. For very large libraries, callers should
    /// pre-filter by substring/embedding similarity.
    pub async fn canonicalize(
        &self,
        candidate: &SourceRecord,
        neighborhood: &[CanonicalRelease],
    ) -> Result<CanonicalDecision> {
        // Tier 1: cache.
        if let Some(cached) = self
            .library
            .cached_canonical_for(candidate.source, &candidate.source_id)
            .context("canonicalize cache lookup")?
        {
            return Ok(CanonicalDecision::Match {
                canonical_id: cached,
                confidence: 1.0,
                decided_by: "cache".to_string(),
            });
        }

        // Tier 2: alias-match.
        if let Some(alias) = try_alias_match(candidate, neighborhood) {
            if alias.confidence >= self.alias_match_threshold {
                self.library
                    .cache_canonicalization(
                        candidate.source,
                        &candidate.source_id,
                        &alias.canonical_id,
                        "alias-match",
                    )
                    .ok(); // best-effort cache write
                return Ok(CanonicalDecision::Match {
                    canonical_id: alias.canonical_id,
                    confidence: alias.confidence,
                    decided_by: "alias-match".to_string(),
                });
            }
        }

        // Tier 3: LLM.
        self.llm_decide(candidate, neighborhood).await
    }

    async fn llm_decide(
        &self,
        candidate: &SourceRecord,
        neighborhood: &[CanonicalRelease],
    ) -> Result<CanonicalDecision> {
        let (system, user) = prompt::build(candidate, neighborhood);
        let req = LlmRequest {
            model: models::CANONICALIZE_DEFAULT.to_string(),
            system: Some(system),
            user,
            max_tokens: 256,
            temperature: 0.0,
        };
        let resp = self.llm.complete(req).await.context("LLM canonicalize")?;
        let parsed = prompt::parse_response(&resp.text, candidate.kind)
            .context("parse LLM canonicalize response")?;
        // Cache the decision so we don't re-ask next time.
        if let CanonicalDecision::Match { canonical_id, .. } = &parsed {
            self.library
                .cache_canonicalization(
                    candidate.source,
                    &candidate.source_id,
                    canonical_id,
                    parsed.decided_by(),
                )
                .ok();
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SourceRecord;
    use crate::store::EngagementEvent;
    use crate::time::FixedClock;
    use async_trait::async_trait;
    use std::sync::Mutex;

    // --------------------------------------------------------------
    // Stub LlmClient — returns canned responses keyed on the prompt.
    // Insta-style: the test sets the expected reply, then asserts the
    // service's decision.
    // --------------------------------------------------------------

    struct StubLlm {
        canned: Mutex<String>,
        calls: Mutex<u32>,
    }

    impl StubLlm {
        fn new(reply: &str) -> Self {
            Self {
                canned: Mutex::new(reply.to_string()),
                calls: Mutex::new(0),
            }
        }

        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl LlmClient for StubLlm {
        async fn complete(&self, _req: LlmRequest) -> Result<crate::llm::LlmResponse> {
            *self.calls.lock().unwrap() += 1;
            Ok(crate::llm::LlmResponse {
                text: self.canned.lock().unwrap().clone(),
            })
        }
    }

    fn lib() -> Arc<Library> {
        Arc::new(Library::open_in_memory(Arc::new(FixedClock(1_000))).unwrap())
    }

    fn candidate(title: &str, source: &'static str, source_id: &str) -> SourceRecord {
        SourceRecord {
            source,
            source_id: source_id.to_string(),
            kind: ReleaseKind::Tv,
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

    fn follow_existing(lib: &Arc<Library>, slug: &str, title: &str) -> CanonicalId {
        let cid = CanonicalId::new(ReleaseKind::Tv, slug).unwrap();
        lib.follow_with_source(
            &cid,
            ReleaseKind::Tv,
            title,
            "seed",
            slug,
            Some(title),
            1.0,
        )
        .unwrap();
        cid
    }

    #[tokio::test]
    async fn cache_short_circuits_alias_and_llm() {
        let lib = lib();
        let existing = follow_existing(&lib, "severance", "Severance");
        // Plant a cache entry pointing tmdb:95396 → existing.
        lib.cache_canonicalization("tmdb", "95396", &existing, "manual")
            .unwrap();
        let stub = Arc::new(StubLlm::new("UNUSED"));
        let svc = CanonicalizationService::new(Arc::clone(&lib), stub.clone());

        let decision = svc
            .canonicalize(&candidate("Severance", "tmdb", "95396"), &lib.followed().unwrap())
            .await
            .unwrap();
        match decision {
            CanonicalDecision::Match {
                canonical_id,
                decided_by,
                ..
            } => {
                assert_eq!(canonical_id, existing);
                assert_eq!(decided_by, "cache");
            }
            d => panic!("expected Match from cache, got {d:?}"),
        }
        assert_eq!(stub.call_count(), 0, "LLM must not be called on cache hit");
    }

    #[tokio::test]
    async fn alias_match_short_circuits_llm_when_confidence_high() {
        let lib = lib();
        let existing = follow_existing(&lib, "severance", "Severance");
        let stub = Arc::new(StubLlm::new("UNUSED"));
        let svc = CanonicalizationService::new(Arc::clone(&lib), stub.clone());

        // Same title (modulo case) → alias-match confidence above 0.85.
        let decision = svc
            .canonicalize(&candidate("severance", "tmdb", "95396"), &lib.followed().unwrap())
            .await
            .unwrap();
        match decision {
            CanonicalDecision::Match {
                canonical_id,
                decided_by,
                ..
            } => {
                assert_eq!(canonical_id, existing);
                assert_eq!(decided_by, "alias-match");
            }
            d => panic!("expected Match from alias, got {d:?}"),
        }
        assert_eq!(stub.call_count(), 0, "LLM must not be called on alias hit");
        // Cache should have been written so the next call is even faster.
        let cached = lib.cached_canonical_for("tmdb", "95396").unwrap().unwrap();
        assert_eq!(cached, existing);
    }

    #[tokio::test]
    async fn llm_match_is_used_when_alias_misses() {
        let lib = lib();
        let existing = follow_existing(&lib, "severance", "Severance");
        let reply = format!(
            r#"{{"decision":"match","canonical_id":"{existing}","confidence":0.92}}"#
        );
        let stub = Arc::new(StubLlm::new(&reply));
        let svc = CanonicalizationService::new(Arc::clone(&lib), stub.clone());

        // "Severance: Cold Harbor" doesn't alias-match "Severance" exactly
        // → falls through to LLM.
        let decision = svc
            .canonicalize(
                &candidate("Severance: Cold Harbor", "tmdb", "999"),
                &lib.followed().unwrap(),
            )
            .await
            .unwrap();
        match decision {
            CanonicalDecision::Match {
                canonical_id,
                confidence,
                decided_by,
            } => {
                assert_eq!(canonical_id, existing);
                assert!((confidence - 0.92).abs() < 1e-6);
                assert!(decided_by.starts_with("llm:"), "got: {decided_by}");
            }
            d => panic!("expected Match from LLM, got {d:?}"),
        }
        assert_eq!(stub.call_count(), 1);
        // Decision was cached.
        let cached = lib.cached_canonical_for("tmdb", "999").unwrap().unwrap();
        assert_eq!(cached, existing);
    }

    #[tokio::test]
    async fn llm_new_canonical_is_returned_verbatim() {
        let lib = lib();
        let reply = r#"{"decision":"new","suggested_slug":"new-show","confidence":0.7}"#;
        let stub = Arc::new(StubLlm::new(reply));
        let svc = CanonicalizationService::new(Arc::clone(&lib), stub.clone());

        let decision = svc
            .canonicalize(&candidate("New Show", "tmdb", "1"), &[])
            .await
            .unwrap();
        match decision {
            CanonicalDecision::NewCanonical {
                kind,
                suggested_slug,
                confidence,
                decided_by,
            } => {
                assert_eq!(kind, ReleaseKind::Tv);
                assert_eq!(suggested_slug, "new-show");
                assert!((confidence - 0.7).abs() < 1e-6);
                assert!(decided_by.starts_with("llm:"));
            }
            d => panic!("expected NewCanonical, got {d:?}"),
        }
        // NewCanonical does NOT cache — the caller must create the
        // row first, after which the next call goes through the cache.
        assert!(lib.cached_canonical_for("tmdb", "1").unwrap().is_none());
    }

    #[tokio::test]
    async fn llm_garbage_response_surfaces_error() {
        let lib = lib();
        let stub = Arc::new(StubLlm::new("this is not json"));
        let svc = CanonicalizationService::new(Arc::clone(&lib), stub);
        let err = svc
            .canonicalize(&candidate("X", "tmdb", "1"), &[])
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("parse"));
    }

    #[tokio::test]
    async fn cache_short_circuit_survives_engagement_writes() {
        let lib = lib();
        let existing = follow_existing(&lib, "severance", "Severance");
        lib.engage(&existing, EngagementEvent::Opened, None).unwrap();
        lib.cache_canonicalization("tmdb", "1", &existing, "alias-match")
            .unwrap();
        let stub = Arc::new(StubLlm::new("UNUSED"));
        let svc = CanonicalizationService::new(Arc::clone(&lib), stub.clone());
        let decision = svc
            .canonicalize(&candidate("Severance", "tmdb", "1"), &lib.followed().unwrap())
            .await
            .unwrap();
        assert!(decision.is_match());
        assert_eq!(stub.call_count(), 0);
    }
}
