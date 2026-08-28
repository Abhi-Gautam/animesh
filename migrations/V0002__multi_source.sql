-- Immutable after release: any change is V0003.
--
-- V0001 pinned media.kind to 'anime' and every source column to 'anilist'.
-- SQLite cannot ALTER a CHECK, so the constrained tables are rebuilt. The
-- migration runner turns foreign_keys off around this file: a PRAGMA inside
-- the transaction is a no-op.

CREATE TABLE media_new (
    media_id      INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind IN ('anime', 'tv')),
    display_title TEXT NOT NULL CHECK (length(display_title) BETWEEN 1 AND 512),
    created_at    INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at    INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;
INSERT INTO media_new SELECT * FROM media;
DROP TABLE media;
ALTER TABLE media_new RENAME TO media;

CREATE TABLE source_fetches_new (
    fetch_id             INTEGER PRIMARY KEY,
    attempt_uuid         TEXT NOT NULL UNIQUE,
    source               TEXT NOT NULL CHECK (source IN ('anilist', 'tvmaze')),
    request_kind         TEXT NOT NULL CHECK (request_kind IN ('detail', 'batch')),
    request_fingerprint  TEXT NOT NULL CHECK (length(request_fingerprint) BETWEEN 1 AND 512),
    requested_at         INTEGER NOT NULL CHECK (requested_at >= 0),
    completed_at         INTEGER NOT NULL CHECK (completed_at >= requested_at),
    outcome              TEXT NOT NULL CHECK (outcome IN (
                              'success', 'http_error', 'graphql_error', 'decode_error',
                              'transport_error', 'timeout', 'too_large', 'integrity_error'
                         )),
    http_status          INTEGER CHECK (http_status BETWEEN 100 AND 599),
    retry_after          INTEGER CHECK (retry_after >= 0),
    rate_limit_remaining INTEGER CHECK (rate_limit_remaining >= 0),
    rate_limit_reset_at  INTEGER CHECK (rate_limit_reset_at >= 0),
    body_json            TEXT,
    byte_length          INTEGER CHECK (byte_length >= 0),
    error_code           TEXT,
    CHECK (body_json IS NOT NULL OR outcome IN ('transport_error', 'timeout', 'too_large')),
    CHECK ((body_json IS NULL) = (byte_length IS NULL))
) STRICT;
INSERT INTO source_fetches_new SELECT * FROM source_fetches;
DROP TABLE source_fetches;
ALTER TABLE source_fetches_new RENAME TO source_fetches;
CREATE INDEX idx_source_fetches_time ON source_fetches(completed_at DESC);

CREATE TABLE source_media_new (
    source_media_id        INTEGER PRIMARY KEY,
    source                 TEXT NOT NULL CHECK (source IN ('anilist', 'tvmaze')),
    source_id              INTEGER NOT NULL CHECK (source_id BETWEEN 1 AND 2147483647),
    media_id               INTEGER NOT NULL REFERENCES media(media_id),
    current_observation_id INTEGER,
    created_at             INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at             INTEGER NOT NULL CHECK (updated_at >= created_at),
    UNIQUE (source, source_id),
    UNIQUE (source_media_id, media_id),
    FOREIGN KEY (current_observation_id, source_media_id)
        REFERENCES source_observations(observation_id, source_media_id)
) STRICT;
INSERT INTO source_media_new SELECT * FROM source_media;
DROP TABLE source_media;
ALTER TABLE source_media_new RENAME TO source_media;
CREATE INDEX idx_source_media_media ON source_media(media_id);

CREATE TABLE source_runtime_state_new (
    source               TEXT PRIMARY KEY CHECK (source IN ('anilist', 'tvmaze')),
    blocked_until        INTEGER CHECK (blocked_until >= 0),
    rate_limit_remaining INTEGER CHECK (rate_limit_remaining >= 0),
    rate_limit_reset_at  INTEGER CHECK (rate_limit_reset_at >= 0),
    updated_at           INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;
INSERT INTO source_runtime_state_new SELECT * FROM source_runtime_state;
DROP TABLE source_runtime_state;
ALTER TABLE source_runtime_state_new RENAME TO source_runtime_state;

-- TVmaze episode numbers restart each season. NULL for AniList.
ALTER TABLE source_observations ADD COLUMN next_season INTEGER
    CHECK (next_season IS NULL OR next_season BETWEEN 1 AND 2147483647);
ALTER TABLE release_events ADD COLUMN season INTEGER
    CHECK (season IS NULL OR season BETWEEN 1 AND 2147483647);
