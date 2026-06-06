-- V0001: SP-1 initial schema.
--
-- Establishes the durable / ephemeral split:
--   * tracked_item   — durable. Your relationship with a show. Never auto-evicted.
--   * metadata_cache — ephemeral. AniList state. TTL-bounded. Disposable.
--   * search_fts     — FTS5 index over the cache. Powers the local-first picker.
--   * kv             — small key/value store for sync timestamps, rate-limit state.
--
-- This file is IMMUTABLE once shipped. Future schema changes go in
-- V0002__*.sql, V0003__*.sql, etc. — see
-- docs/superpowers/specs/2026-06-06-sp1-local-library-design.md §9.

-- ---------------------------------------------------------------------------
-- Durable: tracked_item
-- ---------------------------------------------------------------------------

CREATE TABLE tracked_item (
    id             INTEGER PRIMARY KEY,
    source         TEXT    NOT NULL,
    source_id      TEXT    NOT NULL,
    kind           TEXT    NOT NULL,
    display_title  TEXT    NOT NULL,
    followed_at    INTEGER NOT NULL,
    dropped_at     INTEGER,
    user_note      TEXT
);

CREATE UNIQUE INDEX idx_tracked_item_source         ON tracked_item(source, source_id);
CREATE INDEX        idx_tracked_item_kind_dropped   ON tracked_item(kind, dropped_at);
CREATE INDEX        idx_tracked_item_followed_at    ON tracked_item(followed_at);

-- ---------------------------------------------------------------------------
-- Ephemeral: metadata_cache
-- ---------------------------------------------------------------------------

CREATE TABLE metadata_cache (
    source                 TEXT    NOT NULL,
    source_id              TEXT    NOT NULL,
    display_title          TEXT,
    title_english          TEXT,
    title_native           TEXT,
    status                 TEXT,
    total_episodes         INTEGER,
    format                 TEXT,
    next_episode_number    INTEGER,
    next_episode_airs_at   INTEGER,
    fetched_at             INTEGER NOT NULL,
    expires_at             INTEGER NOT NULL,
    PRIMARY KEY (source, source_id)
);

CREATE INDEX idx_metadata_cache_expires_at ON metadata_cache(expires_at);

-- ---------------------------------------------------------------------------
-- Search index: FTS5 over the cache's title columns
-- ---------------------------------------------------------------------------
-- We use an "external content"-ish manual sync via triggers (rather than
-- content='metadata_cache' content_rowid='rowid') so that source and
-- source_id can travel with each row and we don't have to chase rowids
-- back into the cache. The unicode61 tokenizer with diacritic removal
-- handles Japanese romaji and accented English titles gracefully.

CREATE VIRTUAL TABLE search_fts USING fts5(
    source        UNINDEXED,
    source_id     UNINDEXED,
    display_title,
    title_english,
    title_native,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER trg_metadata_cache_ai AFTER INSERT ON metadata_cache BEGIN
    INSERT INTO search_fts(source, source_id, display_title, title_english, title_native)
    VALUES (NEW.source, NEW.source_id, NEW.display_title, NEW.title_english, NEW.title_native);
END;

CREATE TRIGGER trg_metadata_cache_ad AFTER DELETE ON metadata_cache BEGIN
    DELETE FROM search_fts WHERE source = OLD.source AND source_id = OLD.source_id;
END;

CREATE TRIGGER trg_metadata_cache_au AFTER UPDATE ON metadata_cache BEGIN
    DELETE FROM search_fts WHERE source = OLD.source AND source_id = OLD.source_id;
    INSERT INTO search_fts(source, source_id, display_title, title_english, title_native)
    VALUES (NEW.source, NEW.source_id, NEW.display_title, NEW.title_english, NEW.title_native);
END;

-- ---------------------------------------------------------------------------
-- Generic key/value store
-- ---------------------------------------------------------------------------
-- Namespaced keys. Examples:
--   sync.last_attempt_at       — unix seconds
--   sync.last_success_at       — unix seconds
--   sync.last_error            — short text
--   anilist.ratelimit.remaining
--   anilist.ratelimit.reset_at

CREATE TABLE kv (
    key        TEXT    PRIMARY KEY,
    value      TEXT    NOT NULL,
    updated_at INTEGER NOT NULL
);
