-- V0005: contract phase for the v0.5 canonical migration.
--
-- V0004 backfilled canonical_release + source_ref + engagement from
-- tracked_item + watch_progress. Now that the TUI and all command
-- paths read and write through Library, the legacy tables can go.
--
-- Refinery wraps each migration in a transaction, so a crash
-- mid-drop rolls back cleanly. The FTS5 search index over
-- metadata_cache is untouched — it remains the local-first picker
-- substrate for sources/anilist.

DROP TABLE watch_progress;
DROP TABLE tracked_item;
