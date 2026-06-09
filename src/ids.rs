//! Strongly-typed identifiers for the canonical graph.
//!
//! `CanonicalId` is the durable identity for a followed franchise. It
//! is opaque to callers but has a stable serialized form
//! `release:{kind}:{slug}` that's safe to embed in URLs, logs, and the
//! LLM-readable context export.
//!
//! `ReleaseKind` mirrors the V0004 `canonical_release.kind` CHECK
//! constraint exactly. Adding a new kind requires both a migration and
//! a variant here; the parse path will reject anything not listed so
//! we cannot accidentally read a row the binary doesn't understand.

use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, bail, Result};

/// What kind of release this canonical id refers to.
///
/// Mirrors the V0004 `CHECK (kind IN (...))` constraint on
/// `canonical_release.kind`. Keep these in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReleaseKind {
    Tv,
    Anime,
    Film,
    MusicArtist,
}

impl ReleaseKind {
    /// SQL-side spelling used in `canonical_release.kind` and embedded
    /// in `CanonicalId`. Lowercase + snake_case for multi-word kinds.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tv => "tv",
            Self::Anime => "anime",
            Self::Film => "film",
            Self::MusicArtist => "music_artist",
        }
    }
}

impl fmt::Display for ReleaseKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ReleaseKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "tv" => Ok(Self::Tv),
            "anime" => Ok(Self::Anime),
            "film" => Ok(Self::Film),
            "music_artist" => Ok(Self::MusicArtist),
            other => Err(anyhow!("unknown ReleaseKind {other:?}")),
        }
    }
}

/// Opaque durable id for a canonical release. Format
/// `release:{kind}:{slug}`.
///
/// Construct via [`CanonicalId::new`] (kind + slug) or
/// [`CanonicalId::legacy_from_source`] (V0004 backfill form). Round-trip
/// via [`CanonicalId::parse`] / [`CanonicalId::as_str`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalId(String);

impl CanonicalId {
    /// Build from a kind + slug. The slug is taken verbatim — callers
    /// (the LLM canonicalizer, the legacy backfill) own slug generation.
    /// Empty slugs are rejected because they'd produce ambiguous ids.
    pub fn new(kind: ReleaseKind, slug: &str) -> Result<Self> {
        if slug.is_empty() {
            bail!("CanonicalId slug must not be empty");
        }
        if slug.contains(':') {
            bail!("CanonicalId slug must not contain ':' (delimiter)");
        }
        Ok(Self(format!("release:{}:{}", kind.as_str(), slug)))
    }

    /// Deterministic id for a row backfilled from V0001/V0002/V0003
    /// `tracked_item`. The shape matches the V0004 migration SQL
    /// exactly: `release:{kind}:legacy-{source}-{source_id}`. Same
    /// inputs always produce the same id, so re-running the
    /// canonicalizer is idempotent.
    pub fn legacy_from_source(kind: ReleaseKind, source: &str, source_id: &str) -> Self {
        // Unwrap is safe: kind/source/source_id are non-empty by caller
        // contract, and we know the slug we build doesn't contain ':'.
        Self::new(
            kind,
            &format!("legacy-{source}-{source_id}"),
        )
        .expect("legacy slug is well-formed by construction")
    }

    /// Parse a serialized id. Validates kind and prefix.
    pub fn parse(s: &str) -> Result<Self> {
        let mut parts = s.splitn(3, ':');
        let prefix = parts.next().ok_or_else(|| anyhow!("empty CanonicalId"))?;
        if prefix != "release" {
            bail!("CanonicalId must start with 'release:', got {s:?}");
        }
        let kind = parts
            .next()
            .ok_or_else(|| anyhow!("CanonicalId missing kind segment: {s:?}"))?;
        let _ = ReleaseKind::from_str(kind)?;
        let slug = parts
            .next()
            .ok_or_else(|| anyhow!("CanonicalId missing slug segment: {s:?}"))?;
        if slug.is_empty() {
            bail!("CanonicalId has empty slug: {s:?}");
        }
        Ok(Self(s.to_owned()))
    }

    /// Borrow the serialized form. Stable across versions.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Extract the kind segment.
    pub fn kind(&self) -> ReleaseKind {
        // Safe by construction: every CanonicalId went through new()
        // or parse(), both of which validate kind.
        let mid = self.0.split(':').nth(1).expect("validated at construction");
        ReleaseKind::from_str(mid).expect("validated at construction")
    }

    /// Extract the slug segment. Test-only inspection helper.
    #[cfg(test)]
    pub fn slug(&self) -> &str {
        // Safe by construction (see `kind`).
        self.0.splitn(3, ':').nth(2).expect("validated at construction")
    }
}

impl fmt::Display for CanonicalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for CanonicalId {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

// Carry CanonicalId across the rusqlite boundary as a plain TEXT
// column. The Db layer is the only call site; everything else uses the
// typed form.
impl rusqlite::ToSql for CanonicalId {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::Borrowed(
            rusqlite::types::ValueRef::Text(self.0.as_bytes()),
        ))
    }
}

impl rusqlite::types::FromSql for CanonicalId {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = <String as rusqlite::types::FromSql>::column_result(value)?;
        CanonicalId::parse(&s).map_err(|e| {
            rusqlite::types::FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn release_kind_str_matches_sql_constraint() {
        // These four MUST match V0004 CHECK (kind IN (...)). If you add
        // a kind here without a migration, this is a compile error or
        // test failure waiting to happen at runtime.
        assert_eq!(ReleaseKind::Tv.as_str(), "tv");
        assert_eq!(ReleaseKind::Anime.as_str(), "anime");
        assert_eq!(ReleaseKind::Film.as_str(), "film");
        assert_eq!(ReleaseKind::MusicArtist.as_str(), "music_artist");
    }

    #[test]
    fn release_kind_round_trips_through_str() {
        for k in [
            ReleaseKind::Tv,
            ReleaseKind::Anime,
            ReleaseKind::Film,
            ReleaseKind::MusicArtist,
        ] {
            assert_eq!(ReleaseKind::from_str(k.as_str()).unwrap(), k);
        }
    }

    #[test]
    fn release_kind_rejects_unknown_strings() {
        assert!(ReleaseKind::from_str("bogus").is_err());
        assert!(ReleaseKind::from_str("").is_err());
        // We are strict: case matters because the DB stores lowercase.
        assert!(ReleaseKind::from_str("TV").is_err());
    }

    #[test]
    fn new_builds_well_formed_id() {
        let id = CanonicalId::new(ReleaseKind::Tv, "severance").unwrap();
        assert_eq!(id.as_str(), "release:tv:severance");
        assert_eq!(id.kind(), ReleaseKind::Tv);
        assert_eq!(id.slug(), "severance");
    }

    #[test]
    fn new_rejects_empty_slug() {
        let err = CanonicalId::new(ReleaseKind::Tv, "").unwrap_err();
        assert!(format!("{err}").contains("empty"));
    }

    #[test]
    fn new_rejects_slug_with_colon_delimiter() {
        let err = CanonicalId::new(ReleaseKind::Tv, "season:1").unwrap_err();
        assert!(format!("{err}").contains("':'"));
    }

    #[test]
    fn legacy_form_matches_v0004_backfill() {
        // This must exactly match the SQL in V0004:
        //   'release:' || kind || ':legacy-' || source || '-' || source_id
        let id = CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", "21");
        assert_eq!(id.as_str(), "release:anime:legacy-anilist-21");
    }

    #[test]
    fn parse_round_trips_well_formed_ids() {
        for raw in [
            "release:tv:severance",
            "release:anime:legacy-anilist-21",
            "release:film:dune-part-two",
            "release:music_artist:taylor-swift",
        ] {
            let id = CanonicalId::parse(raw).unwrap();
            assert_eq!(id.as_str(), raw);
        }
    }

    #[test]
    fn parse_rejects_malformed_ids() {
        for raw in [
            "",
            "tv:severance",
            "release:bogus:x",
            "release:tv:",
            "release:tv",
            "release",
        ] {
            assert!(
                CanonicalId::parse(raw).is_err(),
                "expected rejection of {raw:?}"
            );
        }
    }

    #[test]
    fn parse_preserves_slugs_with_internal_dashes_and_dots() {
        // Slugs can be arbitrary text without ':'. We round-trip them.
        let id = CanonicalId::parse("release:tv:better-call-saul.s06").unwrap();
        assert_eq!(id.slug(), "better-call-saul.s06");
    }

    #[test]
    fn from_str_is_an_alias_for_parse() {
        let a = CanonicalId::parse("release:tv:foo").unwrap();
        let b: CanonicalId = "release:tv:foo".parse().unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn display_round_trips_with_as_str() {
        let id = CanonicalId::new(ReleaseKind::Tv, "x").unwrap();
        assert_eq!(format!("{id}"), id.as_str());
    }

    #[test]
    fn rusqlite_round_trip() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id TEXT PRIMARY KEY)").unwrap();
        let id = CanonicalId::new(ReleaseKind::Tv, "severance").unwrap();
        conn.execute("INSERT INTO t VALUES (?1)", rusqlite::params![id])
            .unwrap();
        let back: CanonicalId = conn
            .query_row("SELECT id FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn rusqlite_load_rejects_malformed_id_from_db() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id TEXT PRIMARY KEY)").unwrap();
        conn.execute("INSERT INTO t VALUES ('not-a-canonical-id')", [])
            .unwrap();
        let res: rusqlite::Result<CanonicalId> = conn.query_row("SELECT id FROM t", [], |r| r.get(0));
        assert!(res.is_err(), "FromSql must reject malformed id");
    }

    // ------------------------------------------------------------------
    // Property: any (kind, slug) pair we build with `new` round-trips
    // through serialize → parse → accessors with no loss.
    // ------------------------------------------------------------------

    fn arb_kind() -> impl Strategy<Value = ReleaseKind> {
        prop_oneof![
            Just(ReleaseKind::Tv),
            Just(ReleaseKind::Anime),
            Just(ReleaseKind::Film),
            Just(ReleaseKind::MusicArtist),
        ]
    }

    fn arb_slug() -> impl Strategy<Value = String> {
        // ASCII-ish, no ':' (the only structural restriction). Allow
        // dashes, digits, dots, alphabetics. Min 1, max 64.
        "[A-Za-z0-9][A-Za-z0-9._-]{0,63}".prop_filter("no colons", |s| !s.contains(':'))
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]
        #[test]
        fn id_round_trips_kind_and_slug(kind in arb_kind(), slug in arb_slug()) {
            let id = CanonicalId::new(kind, &slug).unwrap();
            prop_assert_eq!(id.kind(), kind);
            prop_assert_eq!(id.slug(), slug.as_str());
            let parsed = CanonicalId::parse(id.as_str()).unwrap();
            prop_assert_eq!(parsed, id);
        }
    }
}
