-- V0004: v0.5 canonical schema.
--
-- Introduces the canonical_release × source_ref × engagement substrate
-- that lets many noisy sources (AniList, TMDB, TVMaze, MusicBrainz) fan
-- into one normalized graph. The unit of follow becomes the franchise,
-- not (source, source_id).
--
-- Expand-then-contract: this migration leaves tracked_item +
-- watch_progress in place to keep the binary shippable across the
-- code refactor. V0005 will DROP them once the CRUD layer is fully
-- ported to canonical_release.
--
-- Backfill rules:
--   * Every tracked_item row produces one canonical_release + one
--     source_ref. The canonical id is synthesized deterministically as
--     'release:{kind}:legacy-{source}-{source_id}' so we can spot it
--     later and replay the LLM canonicalizer.
--   * Every watch_progress row joins (via source_ref) onto its canonical
--     and produces an 'engagement' event with event='completed' and
--     meta carrying the original `seen` count.
--   * Orphan watch_progress (no matching tracked_item) is silently
--     dropped by the INNER JOIN.

-- ---------------------------------------------------------------------------
-- canonical_release — the durable franchise row.
-- ---------------------------------------------------------------------------
-- followed_at IS NULL  → never followed (created during canonicalization
--                        before the user said "yes").
-- dropped_at  IS NULL  → currently followed (or never followed).
-- dropped_at  NOT NULL → soft-deleted; restoring re-clears it.

CREATE TABLE canonical_release (
    id            TEXT    PRIMARY KEY,
    kind          TEXT    NOT NULL CHECK (kind IN ('tv','anime','film','music_artist')),
    display_title TEXT    NOT NULL,
    cover_ascii   TEXT,
    cover_color   TEXT,
    followed_at   INTEGER,
    dropped_at    INTEGER,
    user_note     TEXT,
    created_at    INTEGER NOT NULL
);

CREATE INDEX idx_canonical_release_followed
    ON canonical_release(followed_at)
    WHERE followed_at IS NOT NULL;
CREATE INDEX idx_canonical_release_kind ON canonical_release(kind);

-- ---------------------------------------------------------------------------
-- source_ref — (source, source_id) → canonical_release mapping.
-- ---------------------------------------------------------------------------
-- confidence is 1.0 for user-confirmed / legacy-migrated rows and the
-- LLM canonicalizer's stated confidence otherwise.

CREATE TABLE source_ref (
    canonical_id TEXT NOT NULL REFERENCES canonical_release(id) ON DELETE CASCADE,
    source       TEXT NOT NULL,
    source_id    TEXT NOT NULL,
    raw_title    TEXT,
    confidence   REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    PRIMARY KEY (source, source_id)
);

CREATE INDEX idx_source_ref_canonical ON source_ref(canonical_id);

-- ---------------------------------------------------------------------------
-- engagement — append-only event log.
-- ---------------------------------------------------------------------------
-- Subsumes the old watch_progress 'seen' counter (now an event with
-- meta={"seen":N}). Adds:
--   opened   — user followed a deep link
--   completed — finished consumption (or, legacy, "seen" count)
--   paused   — explicit pause / snooze of an open session
--   rated    — user rated (meta carries score)
--   snoozed  — user snoozed a notification
--   verified — system event: source confirmed the title is playable
--              on a subscribed streamer (the "verify-then-notify" moat)

CREATE TABLE engagement (
    id           INTEGER PRIMARY KEY,
    canonical_id TEXT    NOT NULL REFERENCES canonical_release(id) ON DELETE CASCADE,
    event        TEXT    NOT NULL CHECK (event IN (
                     'opened','completed','paused','rated','snoozed','verified'
                 )),
    occurred_at  INTEGER NOT NULL,
    meta         TEXT
);

CREATE INDEX idx_engagement_canonical_at ON engagement(canonical_id, occurred_at);
CREATE INDEX idx_engagement_event_at     ON engagement(event, occurred_at);

-- ---------------------------------------------------------------------------
-- canonicalization_cache — idempotency log for the LLM service.
-- ---------------------------------------------------------------------------
-- ON DELETE SET NULL (not CASCADE) so the cache survives canonical
-- mergers: the (source, source_id) decision history is preserved even
-- when the underlying canonical row is replaced.

CREATE TABLE canonicalization_cache (
    source       TEXT    NOT NULL,
    source_id    TEXT    NOT NULL,
    canonical_id TEXT             REFERENCES canonical_release(id) ON DELETE SET NULL,
    decided_at   INTEGER NOT NULL,
    decided_by   TEXT    NOT NULL,
    PRIMARY KEY (source, source_id)
);

-- ---------------------------------------------------------------------------
-- Backfill: tracked_item → canonical_release + source_ref
-- ---------------------------------------------------------------------------

INSERT INTO canonical_release
    (id, kind, display_title, cover_ascii, cover_color,
     followed_at, dropped_at, user_note, created_at)
SELECT
    'release:' || kind || ':legacy-' || source || '-' || source_id,
    kind,
    display_title,
    cover_ascii,
    cover_color,
    followed_at,
    dropped_at,
    user_note,
    followed_at
FROM tracked_item;

INSERT INTO source_ref
    (canonical_id, source, source_id, raw_title, confidence)
SELECT
    'release:' || kind || ':legacy-' || source || '-' || source_id,
    source,
    source_id,
    display_title,
    1.0
FROM tracked_item;

-- ---------------------------------------------------------------------------
-- Backfill: watch_progress → engagement (event='completed', meta=seen)
-- ---------------------------------------------------------------------------
-- INNER JOIN against source_ref silently drops orphan watch_progress
-- rows that have no tracked_item parent. json_object() is built-in to
-- SQLite >= 3.38 (rusqlite bundled); we use it instead of string concat
-- to get correct escaping for free.

INSERT INTO engagement (canonical_id, event, occurred_at, meta)
SELECT
    sr.canonical_id,
    'completed',
    wp.updated_at,
    json_object('seen', wp.seen)
FROM watch_progress wp
JOIN source_ref sr
  ON sr.source = wp.source
 AND sr.source_id = wp.source_id;
