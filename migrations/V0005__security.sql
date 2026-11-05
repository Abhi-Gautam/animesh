-- Retention and privacy controls. Immutable after release.
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
    body_pruned INTEGER NOT NULL DEFAULT 0 CHECK (body_pruned IN (0, 1)),
    CHECK (body_pruned = 1 OR body_json IS NOT NULL OR outcome IN ('transport_error', 'timeout', 'too_large')),
    CHECK ((body_json IS NULL) = (byte_length IS NULL))
) STRICT;
INSERT INTO source_fetches_new SELECT *, 0 FROM source_fetches;
DROP TABLE source_fetches;
ALTER TABLE source_fetches_new RENAME TO source_fetches;
CREATE INDEX idx_source_fetches_time ON source_fetches(completed_at DESC);


CREATE TABLE app_preferences (
    singleton_id INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    private_notifications INTEGER NOT NULL DEFAULT 0 CHECK (private_notifications IN (0, 1))
) STRICT;
INSERT INTO app_preferences(singleton_id) VALUES (1);
-- Heal deadlines saved by versions that trusted unbounded Retry-After values.
UPDATE source_runtime_state SET blocked_until = MIN(blocked_until, updated_at + 86400);
CREATE TABLE notification_removals (
    os_identifier TEXT PRIMARY KEY
) STRICT;
