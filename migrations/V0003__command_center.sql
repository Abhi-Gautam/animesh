-- V0001/V0002 and their source identities remain unchanged.
CREATE TABLE discovery_snapshots (
    snapshot_id INTEGER PRIMARY KEY,
    feed_key TEXT NOT NULL CHECK (feed_key IN ('anime_this_season', 'anime_airing_this_week', 'tv_on_now')),
    fetch_id INTEGER REFERENCES source_fetches(fetch_id),
    generated_at INTEGER NOT NULL CHECK (generated_at >= 0),
    expires_at INTEGER NOT NULL CHECK (expires_at > generated_at),
    completeness TEXT NOT NULL CHECK (completeness IN ('complete', 'partial', 'failed')),
    error_code TEXT,
    UNIQUE (snapshot_id, feed_key)
) STRICT;

CREATE TABLE discovery_members (
    snapshot_id INTEGER NOT NULL REFERENCES discovery_snapshots(snapshot_id) ON DELETE CASCADE,
    rank INTEGER NOT NULL CHECK (rank BETWEEN 0 AND 49),
    source_media_id INTEGER NOT NULL REFERENCES source_media(source_media_id),
    observation_id INTEGER NOT NULL,
    PRIMARY KEY (snapshot_id, rank),
    UNIQUE (snapshot_id, source_media_id),
    FOREIGN KEY (observation_id, source_media_id) REFERENCES source_observations(observation_id, source_media_id)
) STRICT;

CREATE TABLE discovery_feeds (
    feed_key TEXT PRIMARY KEY CHECK (feed_key IN ('anime_this_season', 'anime_airing_this_week', 'tv_on_now')),
    current_snapshot_id INTEGER,
    refresh_after INTEGER NOT NULL CHECK (refresh_after >= 0),
    retry_after INTEGER CHECK (retry_after >= 0),
    last_error TEXT,
    FOREIGN KEY (current_snapshot_id, feed_key) REFERENCES discovery_snapshots(snapshot_id, feed_key)
) STRICT;
INSERT INTO discovery_feeds (feed_key, refresh_after) VALUES
    ('anime_this_season', 0), ('anime_airing_this_week', 0), ('tv_on_now', 0);

CREATE TABLE operations (
    operation_id TEXT PRIMARY KEY CHECK (length(operation_id) BETWEEN 1 AND 128),
    target_json TEXT NOT NULL CHECK (json_valid(target_json)),
    state TEXT NOT NULL CHECK (state IN ('queued', 'running', 'completed', 'throttled', 'failed')),
    updated_at INTEGER NOT NULL CHECK (updated_at >= 0),
    message TEXT
) STRICT;
CREATE UNIQUE INDEX one_active_operation_per_target ON operations(target_json)
    WHERE state IN ('queued', 'running');
