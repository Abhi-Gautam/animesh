-- 001_watchlist: durable follow list of AniList anime entries.
-- One row = one anime the user wants release-radar coverage for.

CREATE TABLE watchlist (
    anilist_id    INTEGER PRIMARY KEY NOT NULL,
    title         TEXT    NOT NULL,
    title_english TEXT,
    title_romaji  TEXT,
    title_native  TEXT,
    status        TEXT,
    format        TEXT,
    episodes      INTEGER,
    season_year   INTEGER,
    added_at      TEXT    NOT NULL,
    updated_at    TEXT    NOT NULL
);

CREATE INDEX idx_watchlist_status ON watchlist (status);
CREATE INDEX idx_watchlist_added_at ON watchlist (added_at);
