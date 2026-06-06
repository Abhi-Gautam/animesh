-- V0002: SP-1.5 additions.
--
-- 1. watch_progress — durable. The 'w' key writes here. Source-neutral
--    (no FK to tracked_item) so SP-7 generalization can route to it
--    from any source/kind. Losing this table is data loss; it must
--    survive cache migrations.
--
-- 2. metadata_cache gets the rest of the AniList Media payload the
--    TUI's detail pane needs: cover image url, description, score,
--    studios, and a JSON-encoded list of streaming external links.
--    Still ephemeral — TTL bounds it.

CREATE TABLE watch_progress (
    source     TEXT    NOT NULL,
    source_id  TEXT    NOT NULL,
    seen       INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source, source_id)
);

ALTER TABLE metadata_cache ADD COLUMN cover_image_url    TEXT;
ALTER TABLE metadata_cache ADD COLUMN description        TEXT;
ALTER TABLE metadata_cache ADD COLUMN score              REAL;
ALTER TABLE metadata_cache ADD COLUMN studios            TEXT;       -- comma-joined
ALTER TABLE metadata_cache ADD COLUMN streaming_links_json TEXT;     -- JSON array of {site,url,color}
