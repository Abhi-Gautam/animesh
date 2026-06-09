-- V0006: source ingestion pipeline.
--
-- Adds the raw -> observation -> candidate substrate used by follow search
-- and future multi-source sync. Existing metadata_cache/search_fts remain in
-- place for current TUI compatibility.

CREATE TABLE raw_source_payload (
    id              TEXT PRIMARY KEY,
    source          TEXT NOT NULL,
    endpoint        TEXT NOT NULL,
    method          TEXT NOT NULL,
    request_key     TEXT NOT NULL,
    request_hash    TEXT NOT NULL,
    request_json    TEXT,
    http_status     INTEGER NOT NULL,
    response_hash   TEXT NOT NULL,
    response_json   TEXT NOT NULL,
    fetched_at      INTEGER NOT NULL,
    expires_at      INTEGER,
    created_at      INTEGER NOT NULL,
    UNIQUE(source, request_hash, response_hash)
);

CREATE INDEX idx_raw_source_payload_request
    ON raw_source_payload(source, endpoint, request_hash);
CREATE INDEX idx_raw_source_payload_expiry
    ON raw_source_payload(source, expires_at);

CREATE TABLE source_observation (
    source            TEXT NOT NULL,
    source_id         TEXT NOT NULL,
    raw_payload_id    TEXT NOT NULL REFERENCES raw_source_payload(id) ON DELETE CASCADE,
    kind              TEXT NOT NULL CHECK (kind IN ('tv','anime','film','music_artist')),
    display_title     TEXT NOT NULL,
    raw_title         TEXT,
    description       TEXT,
    status            TEXT,
    observed_at       INTEGER NOT NULL,
    source_updated_at INTEGER,
    PRIMARY KEY (source, source_id)
);

CREATE INDEX idx_source_observation_kind
    ON source_observation(kind);

CREATE TABLE source_alias_observation (
    source      TEXT NOT NULL,
    source_id   TEXT NOT NULL,
    alias       TEXT NOT NULL,
    locale      TEXT,
    alias_kind  TEXT,
    confidence  REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    PRIMARY KEY (source, source_id, alias),
    FOREIGN KEY (source, source_id) REFERENCES source_observation(source, source_id) ON DELETE CASCADE
);

CREATE TABLE external_id_observation (
    source      TEXT NOT NULL,
    source_id   TEXT NOT NULL,
    id_kind     TEXT NOT NULL,
    id_value    TEXT NOT NULL,
    confidence  REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    PRIMARY KEY (source, source_id, id_kind, id_value),
    FOREIGN KEY (source, source_id) REFERENCES source_observation(source, source_id) ON DELETE CASCADE
);

CREATE INDEX idx_external_id_observation_lookup
    ON external_id_observation(id_kind, id_value);

CREATE TABLE release_event_observation (
    id              TEXT PRIMARY KEY,
    source          TEXT NOT NULL,
    source_id       TEXT NOT NULL,
    event_kind      TEXT NOT NULL,
    title           TEXT,
    season          INTEGER,
    episode         INTEGER,
    local_date      TEXT,
    local_time      TEXT,
    source_timezone TEXT,
    scheduled_at    INTEGER,
    precision       TEXT NOT NULL CHECK (precision IN ('instant','date','month','year','unknown')),
    confidence      REAL NOT NULL CHECK (confidence >= 0.0 AND confidence <= 1.0),
    observed_at     INTEGER NOT NULL,
    FOREIGN KEY (source, source_id) REFERENCES source_observation(source, source_id) ON DELETE CASCADE
);

CREATE INDEX idx_release_event_observation_due
    ON release_event_observation(scheduled_at);
CREATE INDEX idx_release_event_observation_source
    ON release_event_observation(source, source_id);

CREATE TABLE link_observation (
    source    TEXT NOT NULL,
    source_id TEXT NOT NULL,
    site      TEXT NOT NULL,
    url       TEXT NOT NULL,
    link_kind TEXT,
    PRIMARY KEY (source, source_id, site, url),
    FOREIGN KEY (source, source_id) REFERENCES source_observation(source, source_id) ON DELETE CASCADE
);

CREATE TABLE image_observation (
    source     TEXT NOT NULL,
    source_id  TEXT NOT NULL,
    image_kind TEXT NOT NULL,
    url        TEXT NOT NULL,
    width      INTEGER,
    height     INTEGER,
    PRIMARY KEY (source, source_id, image_kind, url),
    FOREIGN KEY (source, source_id) REFERENCES source_observation(source, source_id) ON DELETE CASCADE
);

CREATE TABLE source_candidate (
    source             TEXT NOT NULL,
    source_id          TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('tv','anime','film','music_artist')),
    display_title      TEXT NOT NULL,
    search_text        TEXT NOT NULL,
    first_seen_at      INTEGER NOT NULL,
    last_seen_at       INTEGER NOT NULL,
    expires_at         INTEGER,
    score_hint         REAL,
    PRIMARY KEY (source, source_id),
    FOREIGN KEY (source, source_id) REFERENCES source_observation(source, source_id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE source_candidate_fts USING fts5(
    source UNINDEXED,
    source_id UNINDEXED,
    kind UNINDEXED,
    display_title,
    search_text,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER trg_source_candidate_ai AFTER INSERT ON source_candidate BEGIN
    INSERT INTO source_candidate_fts(source, source_id, kind, display_title, search_text)
    VALUES (NEW.source, NEW.source_id, NEW.kind, NEW.display_title, NEW.search_text);
END;

CREATE TRIGGER trg_source_candidate_ad AFTER DELETE ON source_candidate BEGIN
    DELETE FROM source_candidate_fts WHERE source = OLD.source AND source_id = OLD.source_id;
END;

CREATE TRIGGER trg_source_candidate_au AFTER UPDATE ON source_candidate BEGIN
    DELETE FROM source_candidate_fts WHERE source = OLD.source AND source_id = OLD.source_id;
    INSERT INTO source_candidate_fts(source, source_id, kind, display_title, search_text)
    VALUES (NEW.source, NEW.source_id, NEW.kind, NEW.display_title, NEW.search_text);
END;

CREATE TABLE source_parse_error (
    id             INTEGER PRIMARY KEY,
    raw_payload_id TEXT NOT NULL REFERENCES raw_source_payload(id) ON DELETE CASCADE,
    source         TEXT NOT NULL,
    endpoint       TEXT NOT NULL,
    error          TEXT NOT NULL,
    occurred_at    INTEGER NOT NULL
);
