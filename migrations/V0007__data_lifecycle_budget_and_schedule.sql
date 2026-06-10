-- V0007: data lifecycle request budget + followed schedule projection.
--
-- Defines durable state for:
--   * source search cache (prevents repeated Enter searches from re-querying)
--   * source_ref refresh scheduling/backoff (bounded sync)
--   * canonical schedule events (serving projection from source observations)

CREATE TABLE source_search_cache (
    source          TEXT NOT NULL,
    query_key       TEXT NOT NULL,
    last_success_at INTEGER,
    next_due_at     INTEGER,
    PRIMARY KEY (source, query_key)
);

CREATE INDEX idx_source_search_cache_due
    ON source_search_cache(next_due_at);

CREATE TABLE source_ref_refresh_state (
    source          TEXT NOT NULL,
    source_id       TEXT NOT NULL,
    last_attempt_at INTEGER,
    last_success_at INTEGER,
    last_error      TEXT,
    next_due_at     INTEGER,
    failure_count   INTEGER NOT NULL DEFAULT 0 CHECK (failure_count >= 0),
    PRIMARY KEY (source, source_id),
    FOREIGN KEY (source, source_id) REFERENCES source_ref(source, source_id) ON DELETE CASCADE
);

CREATE INDEX idx_source_ref_refresh_state_due
    ON source_ref_refresh_state(next_due_at);

CREATE TABLE canonical_schedule_event (
    id               TEXT PRIMARY KEY,
    canonical_id     TEXT NOT NULL REFERENCES canonical_release(id) ON DELETE CASCADE,
    source           TEXT NOT NULL,
    source_event_id  TEXT NOT NULL,
    event_kind       TEXT NOT NULL,
    title            TEXT,
    season           INTEGER,
    episode          INTEGER,
    local_date       TEXT,
    local_time       TEXT,
    source_timezone  TEXT,
    scheduled_at     INTEGER,
    precision        TEXT NOT NULL CHECK (precision IN ('instant','date','month','year','unknown')),
    confidence       REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    observed_at      INTEGER NOT NULL,
    superseded_at    INTEGER,
    FOREIGN KEY (source_event_id) REFERENCES release_event_observation(id) ON DELETE CASCADE,
    UNIQUE (canonical_id, source, source_event_id)
);

CREATE INDEX idx_canonical_schedule_event_canonical_due
    ON canonical_schedule_event(canonical_id, scheduled_at)
    WHERE superseded_at IS NULL;

CREATE INDEX idx_canonical_schedule_event_due
    ON canonical_schedule_event(scheduled_at)
    WHERE superseded_at IS NULL;
