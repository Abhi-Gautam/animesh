//! Local-first fuzzy-search picker UI (crossterm).
//!
//! Search runs against the SQLite FTS5 index. The network is only
//! touched when the user commits (Enter) or explicitly requests an
//! AniList fallback (Tab). Reused by `follow`, `unfollow`, `drop`,
//! and future SP-4/SP-6 surfaces.

// Implementation lands in T19. Stub keeps the module tree compiling.
