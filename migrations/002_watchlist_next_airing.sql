-- 002_watchlist_next_airing: cache schedule fields from AniList media fetch.
-- Populated when an entry is added/refreshed via `watchlist <id>`.

ALTER TABLE watchlist ADD COLUMN next_episode INTEGER;
ALTER TABLE watchlist ADD COLUMN next_airing_at INTEGER; -- unix seconds UTC, null if none
