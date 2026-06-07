-- V0003: cover art for followed shows.
--
-- Renders happen at follow-time only — search results never get an
-- ASCII conversion. NULL means "no cover yet" (offline at follow, or
-- AniList lacked a cover URL); the detail pane shows a placeholder.

ALTER TABLE tracked_item ADD COLUMN cover_ascii TEXT;
ALTER TABLE tracked_item ADD COLUMN cover_color TEXT;
