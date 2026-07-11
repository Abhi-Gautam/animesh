-- 003_notifications: per-airing notification ledger for the notifier daemon.
-- One row = one (anime, episode) airing the daemon has fired a notification for.
-- The dedup key stops the daemon re-firing the same airing on its own loop
-- re-entry, while the watchlist row still points at that episode (i.e. before
-- the next sync advances next_episode).

CREATE TABLE notifications (
    anilist_id  INTEGER NOT NULL,
    episode     INTEGER NOT NULL,
    airing_at   INTEGER NOT NULL,           -- unix seconds UTC, the airing this row is for
    sent        INTEGER NOT NULL DEFAULT 0, -- bool: 0 = not yet, 1 = fired
    notified_at INTEGER,                    -- unix seconds UTC, null until sent
    PRIMARY KEY (anilist_id, episode)
);
