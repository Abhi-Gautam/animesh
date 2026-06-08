//! Source-link delta verification.
//!
//! Pure function. Given (previous_links, current_links, subscriptions),
//! emit a [`VerifiedRelease`] for every streaming link that is:
//!
//!   1. Present in `current_links` but NOT in `previous_links`.
//!   2. For a `site` that matches one of the user's `subscriptions`.
//!
//! Matching is case-insensitive ("netflix" matches "Netflix"). The
//! caller (the sync loop) is responsible for persisting these events
//! via [`crate::library::Library::engage`] with
//! [`crate::store::EngagementEvent::Verified`].
//!
//! ## Why this is the verify signal
//!
//! Source APIs (AniList, TMDB) only attach a streaming URL to a
//! title once the content is genuinely available on that streamer in
//! a given region. The transition "URL absent → URL present" is the
//! cheapest possible "verify-then-notify" signal: it requires only a
//! diff between two cache rows, and the URL itself is the deep link
//! we ship to the notifier.

use crate::ids::CanonicalId;
use crate::sources::StreamingLink;

/// One emit-once event: a previously-absent streaming link appeared
/// for a streamer the user subscribes to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VerifiedRelease {
    pub canonical_id: CanonicalId,
    /// The streamer site as the source reported it ("Netflix",
    /// "Crunchyroll"). Preserve the casing the source uses; downstream
    /// (notifier) decides display form.
    pub streamer: String,
    /// Deep link from the source. Goes straight into the push body.
    pub deep_link: String,
    /// Unix seconds when verification fired.
    pub verified_at: i64,
}

/// Diff `current` against `prev`, emit a [`VerifiedRelease`] for every
/// link that is new AND matches a subscription.
///
/// `subscriptions` is a list of streamer names the user pays for —
/// e.g. `["Netflix", "Crunchyroll"]`. Matching is case-insensitive
/// substring-equality on `site`.
///
/// The function is pure and idempotent: passing identical `prev` and
/// `current` returns an empty vec. Order of emissions matches the
/// order of `current` for determinism in tests + replays.
pub fn detect_new_streaming(
    canonical_id: &CanonicalId,
    prev: &[StreamingLink],
    current: &[StreamingLink],
    subscriptions: &[String],
    verified_at: i64,
) -> Vec<VerifiedRelease> {
    if subscriptions.is_empty() {
        return Vec::new();
    }
    let prev_keys: std::collections::HashSet<(String, String)> = prev
        .iter()
        .map(|l| (l.site.to_ascii_lowercase(), l.url.clone()))
        .collect();
    let subs: std::collections::HashSet<String> = subscriptions
        .iter()
        .map(|s| s.to_ascii_lowercase())
        .collect();

    let mut out = Vec::new();
    for link in current {
        let key = (link.site.to_ascii_lowercase(), link.url.clone());
        if prev_keys.contains(&key) {
            continue;
        }
        if !subs.contains(&key.0) {
            continue;
        }
        out.push(VerifiedRelease {
            canonical_id: canonical_id.clone(),
            streamer: link.site.clone(),
            deep_link: link.url.clone(),
            verified_at,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReleaseKind;

    fn id() -> CanonicalId {
        CanonicalId::new(ReleaseKind::Tv, "severance").unwrap()
    }

    fn link(site: &str, url: &str) -> StreamingLink {
        StreamingLink {
            site: site.into(),
            url: url.into(),
        }
    }

    fn subs(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_subscriptions_emit_nothing() {
        let prev = vec![];
        let cur = vec![link("Netflix", "https://netflix.com/x")];
        let out = detect_new_streaming(&id(), &prev, &cur, &[], 1_000);
        assert!(out.is_empty());
    }

    #[test]
    fn link_already_present_emits_nothing() {
        let prev = vec![link("Netflix", "https://netflix.com/x")];
        let cur = vec![link("Netflix", "https://netflix.com/x")];
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert!(out.is_empty());
    }

    #[test]
    fn new_link_for_subscribed_streamer_emits_event() {
        let prev = vec![];
        let cur = vec![link("Netflix", "https://netflix.com/title/95396")];
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].streamer, "Netflix");
        assert_eq!(out[0].deep_link, "https://netflix.com/title/95396");
        assert_eq!(out[0].verified_at, 1_000);
        assert_eq!(out[0].canonical_id, id());
    }

    #[test]
    fn new_link_for_unsubscribed_streamer_emits_nothing() {
        let prev = vec![];
        let cur = vec![link("HBO Max", "https://hbomax.com/x")];
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert!(out.is_empty());
    }

    #[test]
    fn subscription_match_is_case_insensitive() {
        let prev = vec![];
        let cur = vec![link("NETFLIX", "https://netflix.com/x")];
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["netflix"]), 1_000);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn multiple_new_subscribed_links_emit_in_order() {
        let prev = vec![];
        let cur = vec![
            link("Netflix", "https://netflix.com/x"),
            link("Crunchyroll", "https://crunchyroll.com/y"),
        ];
        let out = detect_new_streaming(
            &id(),
            &prev,
            &cur,
            &subs(&["Netflix", "Crunchyroll"]),
            1_000,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].streamer, "Netflix");
        assert_eq!(out[1].streamer, "Crunchyroll");
    }

    #[test]
    fn link_removed_emits_nothing() {
        // We only fire on appearance, never on disappearance.
        let prev = vec![link("Netflix", "https://netflix.com/x")];
        let cur = vec![];
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert!(out.is_empty());
    }

    #[test]
    fn link_url_change_for_same_streamer_emits_new_event() {
        // Source rewrote the URL — treat as a new playable link.
        let prev = vec![link("Netflix", "https://netflix.com/old")];
        let cur = vec![link("Netflix", "https://netflix.com/new")];
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].deep_link, "https://netflix.com/new");
    }

    #[test]
    fn idempotent_on_identical_inputs_repeated_calls() {
        // Calling twice with the same inputs must give the same
        // result — the sync loop relies on this for at-least-once
        // delivery semantics with notifier-side dedupe.
        let prev = vec![];
        let cur = vec![link("Netflix", "https://netflix.com/x")];
        let a = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        let b = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert_eq!(a, b);
    }

    #[test]
    fn handles_many_links_efficiently() {
        // Sanity check: 1000 prev + 1000 current shouldn't be quadratic.
        let prev: Vec<StreamingLink> = (0..1000)
            .map(|i| link("Netflix", &format!("https://netflix.com/{i}")))
            .collect();
        let mut cur = prev.clone();
        cur.push(link("Netflix", "https://netflix.com/new"));
        let out = detect_new_streaming(&id(), &prev, &cur, &subs(&["Netflix"]), 1_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].deep_link, "https://netflix.com/new");
    }
}
