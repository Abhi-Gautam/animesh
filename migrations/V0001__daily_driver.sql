-- Plan 001 section 9. Immutable after release: any change is V0002.
--
-- The layering is Bronze (source_fetches), Silver (source_observations), Gold
-- (media, source_media, follows, release_events, notification_jobs). Constraints
-- carry the invariants rather than trusting Rust callers, so a bug in the
-- Library surfaces as a failed transaction instead of silent corruption.

CREATE TABLE engine_state (
    singleton_id      INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    installation_uuid TEXT NOT NULL UNIQUE,
    created_at        INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at        INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;

CREATE TABLE media (
    media_id      INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL CHECK (kind = 'anime'),
    display_title TEXT NOT NULL CHECK (length(display_title) BETWEEN 1 AND 512),
    created_at    INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at    INTEGER NOT NULL CHECK (updated_at >= created_at)
) STRICT;

-- Bronze. Every response occurrence, including failures.
--
-- The body is stored inline. A content-addressed blob table with hash
-- deduplication would save roughly 2 MB/year at dogfood scale, which does not
-- pay for a second table and a conditional foreign key.
--
-- body_json is nullable so a future retention sweep can null old bodies without
-- a migration. No such sweep ships in Plan 001.
CREATE TABLE source_fetches (
    fetch_id             INTEGER PRIMARY KEY,
    attempt_uuid         TEXT NOT NULL UNIQUE,
    source               TEXT NOT NULL CHECK (source = 'anilist'),
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
    -- Only a failure that never produced bytes may lack a body.
    CHECK (body_json IS NOT NULL OR outcome IN ('transport_error', 'timeout', 'too_large')),
    CHECK ((body_json IS NULL) = (byte_length IS NULL))
) STRICT;

CREATE INDEX idx_source_fetches_time ON source_fetches(completed_at DESC);

-- source_id is an integer, not a zero-padded text key with a GLOB check. A
-- non-numeric source would be a deliberate migration, not a cost paid up front
-- for a generic schema that section 2 lists as a non-goal.
CREATE TABLE source_media (
    source_media_id        INTEGER PRIMARY KEY,
    source                 TEXT NOT NULL CHECK (source = 'anilist'),
    source_id              INTEGER NOT NULL CHECK (source_id BETWEEN 1 AND 2147483647),
    media_id               INTEGER NOT NULL REFERENCES media(media_id),
    current_observation_id INTEGER,
    created_at             INTEGER NOT NULL CHECK (created_at >= 0),
    updated_at             INTEGER NOT NULL CHECK (updated_at >= created_at),
    UNIQUE (source, source_id),
    -- Lets release_events prove its media_id agrees with its source_media_id.
    UNIQUE (source_media_id, media_id),
    -- The current pointer must name an observation of this same source media.
    FOREIGN KEY (current_observation_id, source_media_id)
        REFERENCES source_observations(observation_id, source_media_id)
) STRICT;

CREATE INDEX idx_source_media_media ON source_media(media_id);

-- Silver. One row per successfully parsed item, including repeated identical
-- facts: the occurrence is itself evidence that the source still asserted them.
CREATE TABLE source_observations (
    observation_id  INTEGER PRIMARY KEY,
    source_media_id INTEGER NOT NULL REFERENCES source_media(source_media_id),
    fetch_id        INTEGER NOT NULL REFERENCES source_fetches(fetch_id),
    parser_version  INTEGER NOT NULL CHECK (parser_version > 0),
    observed_at     INTEGER NOT NULL CHECK (observed_at >= 0),
    display_title   TEXT NOT NULL CHECK (length(display_title) BETWEEN 1 AND 512),
    title_english   TEXT CHECK (length(title_english) <= 512),
    title_romaji    TEXT CHECK (length(title_romaji) <= 512),
    title_native    TEXT CHECK (length(title_native) <= 512),
    status          TEXT NOT NULL CHECK (status IN (
                        'releasing', 'not_yet_released', 'finished',
                        'cancelled', 'hiatus', 'unknown'
                    )),
    status_raw      TEXT CHECK (length(status_raw) <= 64),
    format_raw      TEXT CHECK (length(format_raw) <= 64),
    episode_count   INTEGER CHECK (episode_count BETWEEN 1 AND 2147483647),
    season_year     INTEGER CHECK (season_year BETWEEN 1900 AND 9999),
    next_episode    INTEGER CHECK (next_episode BETWEEN 1 AND 2147483647),
    next_airing_at  INTEGER CHECK (next_airing_at >= 0),
    UNIQUE (observation_id, source_media_id),
    -- The pair is indivisible: a half-populated schedule would be
    -- indistinguishable from an explicit null and would withdraw a real event.
    CHECK ((next_episode IS NULL) = (next_airing_at IS NULL))
) STRICT;

CREATE INDEX idx_observations_source_time
    ON source_observations(source_media_id, observed_at DESC);

-- A response may replace Silver or Gold state only when its generation still
-- matches. This is what stops a slow response, or a drop that raced it, from
-- rolling state backward.
CREATE TABLE source_refresh_state (
    source_media_id      INTEGER PRIMARY KEY REFERENCES source_media(source_media_id),
    request_generation   INTEGER NOT NULL DEFAULT 0 CHECK (request_generation >= 0),
    last_attempt_at      INTEGER CHECK (last_attempt_at >= 0),
    last_success_at      INTEGER CHECK (last_success_at >= 0),
    refresh_after        INTEGER NOT NULL CHECK (refresh_after >= 0),
    consecutive_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_failures >= 0),
    last_error_code      TEXT,
    last_error_at        INTEGER CHECK (last_error_at >= 0),
    retry_after          INTEGER CHECK (retry_after >= 0),
    updated_at           INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;

CREATE INDEX idx_refresh_due ON source_refresh_state(refresh_after);

-- Survives restart, so a 429 keeps blocking search, follow, and refresh across
-- a relaunch instead of being forgotten with the process.
CREATE TABLE source_runtime_state (
    source               TEXT PRIMARY KEY CHECK (source = 'anilist'),
    blocked_until        INTEGER CHECK (blocked_until >= 0),
    rate_limit_remaining INTEGER CHECK (rate_limit_remaining >= 0),
    rate_limit_reset_at  INTEGER CHECK (rate_limit_reset_at >= 0),
    updated_at           INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;

CREATE TABLE follows (
    media_id    INTEGER PRIMARY KEY REFERENCES media(media_id),
    state       TEXT NOT NULL CHECK (state IN ('active', 'dropped')),
    followed_at INTEGER NOT NULL CHECK (followed_at >= 0),
    updated_at  INTEGER NOT NULL CHECK (updated_at >= followed_at)
) STRICT;

CREATE INDEX idx_follows_state ON follows(state);

CREATE TABLE release_events (
    release_event_id    INTEGER PRIMARY KEY,
    event_uuid          TEXT NOT NULL UNIQUE,
    media_id            INTEGER NOT NULL,
    source_media_id     INTEGER NOT NULL,
    source_event_key    TEXT NOT NULL CHECK (length(source_event_key) BETWEEN 1 AND 128),
    sequence_number     INTEGER CHECK (sequence_number BETWEEN 1 AND 2147483647),
    scheduled_at        INTEGER NOT NULL CHECK (scheduled_at >= 0),
    state               TEXT NOT NULL CHECK (state IN (
                            'scheduled', 'elapsed', 'withdrawn', 'superseded'
                        )),
    schedule_revision   INTEGER NOT NULL CHECK (schedule_revision >= 1),
    first_observed_at   INTEGER NOT NULL CHECK (first_observed_at >= 0),
    last_observed_at    INTEGER NOT NULL CHECK (last_observed_at >= first_observed_at),
    last_observation_id INTEGER NOT NULL,
    UNIQUE (source_media_id, source_event_key),
    FOREIGN KEY (source_media_id, media_id)
        REFERENCES source_media(source_media_id, media_id),
    FOREIGN KEY (last_observation_id, source_media_id)
        REFERENCES source_observations(observation_id, source_media_id)
) STRICT;

-- At most one scheduled event per source media. Two would mean two competing
-- notifications for the same show.
CREATE UNIQUE INDEX idx_one_scheduled_event_per_source_media
    ON release_events(source_media_id) WHERE state = 'scheduled';
CREATE INDEX idx_release_events_due ON release_events(state, scheduled_at);
CREATE INDEX idx_release_events_media ON release_events(media_id, state, scheduled_at);

-- desired_request_json is the canonical serialization of every field that
-- affects what macOS would present. Change detection is string equality on it.
-- A hash would save nothing at ~200 bytes and would make every mismatch
-- undebuggable.
--
-- There is no `ambiguous` state and no in-flight attempt columns. Every
-- reconciliation pass reads the OS pending and delivered lists first and treats
-- them as authoritative, so a crash between OS acceptance and commit is
-- resolved by observation rather than speculative bookkeeping.
CREATE TABLE notification_jobs (
    notification_key      TEXT PRIMARY KEY,
    release_event_id      INTEGER NOT NULL UNIQUE REFERENCES release_events(release_event_id),
    os_identifier         TEXT NOT NULL UNIQUE,
    desired_at            INTEGER NOT NULL CHECK (desired_at >= 0),
    desired_revision      INTEGER NOT NULL CHECK (desired_revision >= 1),
    desired_request_json  TEXT NOT NULL CHECK (length(desired_request_json) BETWEEN 1 AND 4096),
    state                 TEXT NOT NULL CHECK (state IN (
                              'desired', 'registered', 'delivered',
                              'failed', 'expired', 'cancelled'
                          )),
    registered_revision   INTEGER CHECK (registered_revision >= 1),
    registered_at         INTEGER CHECK (registered_at >= 0),
    delivered_observed_at INTEGER CHECK (delivered_observed_at >= 0),
    last_attempt_at       INTEGER CHECK (last_attempt_at >= 0),
    retry_after           INTEGER CHECK (retry_after >= 0),
    attempt_count         INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    last_error_code       TEXT,
    updated_at            INTEGER NOT NULL CHECK (updated_at >= 0),
    -- A registered job must record which revision the OS actually holds.
    CHECK (state <> 'registered' OR registered_revision IS NOT NULL),
    CHECK (state <> 'delivered' OR delivered_observed_at IS NOT NULL)
) STRICT;

CREATE INDEX idx_notification_jobs_plan ON notification_jobs(state, desired_at);

CREATE TABLE notification_surface_state (
    singleton_id              INTEGER PRIMARY KEY CHECK (singleton_id = 1),
    authorization             TEXT NOT NULL CHECK (authorization IN (
                                  'unknown', 'not_determined', 'denied',
                                  'authorized', 'provisional', 'ephemeral'
                              )),
    authorization_observed_at INTEGER CHECK (authorization_observed_at >= 0),
    last_reconciled_at        INTEGER CHECK (last_reconciled_at >= 0),
    last_error_code           TEXT,
    updated_at                INTEGER NOT NULL CHECK (updated_at >= 0)
) STRICT;
