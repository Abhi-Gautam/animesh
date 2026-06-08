//! Synthetic-key integration tests for the TUI state machine.
//!
//! These don't touch a real terminal — they drive `handle_key` directly
//! against an in-memory [`Facade`]. They exist because the original `:`
//! → Enter bug slipped past unit tests: the verb registry compiled,
//! the palette compiled, but the wire between them was a `// stub`
//! comment. These tests verify the wire.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ids::{CanonicalId, ReleaseKind};
use crate::library::Library as Facade;
use crate::sources::anilist::AniListClient;
use crate::store::{CacheEntry, EngagementEvent};
use crate::time::FixedClock;
use crate::tui::app::{App, Overlay};
use crate::tui::model::Shelf;
use crate::tui::pane::Windows;
use crate::tui::palette::PaletteMode;
use crate::tui::subs::Subs;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        super::handle_key(app, char_key(c));
    }
}

/// Build an App with one followed show that's verified-playable on a
/// subscribed streamer. This lands the show in the PLAYABLE pane (the
/// default focused pane), so `:watched`, `c`, `d`, etc. all act on a
/// non-empty selection.
fn app_with_one_show(now: i64) -> App {
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let cid = CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", "21");
    facade
        .follow_with_source(
            &cid,
            ReleaseKind::Anime,
            "One Piece",
            "anilist",
            "21",
            Some("One Piece"),
            1.0,
        )
        .unwrap();
    let cache = CacheEntry {
        source: "anilist".into(),
        source_id: "21".into(),
        display_title: Some("One Piece".into()),
        title_english: Some("One Piece".into()),
        title_native: None,
        status: Some("RELEASING".into()),
        total_episodes: Some(12),
        format: Some("TV".into()),
        next_episode_number: Some(1),
        next_episode_airs_at: Some(now + 3600),
        fetched_at: now,
        expires_at: now + 6 * 3600,
        cover_image_url: None,
        description: None,
        score: None,
        studios: None,
        streaming_links_json: None,
    };
    facade.upsert_cache(&cache).unwrap();
    // Verified-on-subscribed → Playable. Lets existing tests select
    // the show under the default focused pane (PANE_PLAYABLE).
    facade
        .engage(
            &cid,
            EngagementEvent::Verified,
            Some(r#"{"streamer":"Netflix","url":"https://netflix.com/x"}"#),
        )
        .unwrap();
    let mut subs = Subs::default();
    subs.add(&facade, "Netflix").unwrap();
    let windows = Windows::from_env();
    let shelf = Shelf::load(&facade, now, windows, &subs).unwrap();
    let client = AniListClient::new();
    App::new(facade, client, shelf, windows, subs, now)
}

fn empty_app(now: i64) -> App {
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let subs = Subs::default();
    let windows = Windows::from_env();
    let shelf = Shelf::load(&facade, now, windows, &subs).unwrap();
    let client = AniListClient::new();
    App::new(facade, client, shelf, windows, subs, now)
}

// ---------- Command mode (`:`) ----------

#[test]
fn colon_open_then_close_with_esc() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    assert_eq!(app.overlay, Overlay::Command);
    super::handle_key(&mut app, key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn colon_watched_enter_increments_progress_and_pushes_toast() {
    let now = 1_700_000_000;
    let mut app = app_with_one_show(now);
    // Pre-condition: 0 watched, show in Playable (verified-on-subscribed).
    assert_eq!(app.shelf.shows[0].seen(), 0);
    assert!(matches!(
        app.shelf.shows[0].pane,
        Some(crate::tui::pane::Pane::Playable)
    ));

    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "watched");
    super::handle_key(&mut app, key(KeyCode::Enter));

    assert_eq!(app.overlay, Overlay::None, "overlay closes after Enter");
    assert_eq!(app.shelf.shows[0].seen(), 1, "watched should increment");
    let toast = app.toasts.visible().unwrap_or("");
    assert!(toast.contains("Marked"), "toast was: {toast:?}");
    assert!(toast.contains("One Piece"), "toast was: {toast:?}");

    // Durable: the engagement event was appended.
    let last = app
        .facade
        .last_engagement(
            &CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", "21"),
            crate::store::EngagementEvent::Completed,
        )
        .unwrap()
        .expect("engagement was persisted");
    let meta = last.meta.expect("seen meta JSON");
    assert!(meta.contains("\"seen\":1"), "meta was: {meta}");
}

#[test]
fn colon_alias_w_works_same_as_watched() {
    // `:w` is the alias for `:watched`. This is the bug-regression
    // proof: a single missing alias would silently break power users.
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "w");
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.shelf.shows[0].seen(), 1);
}

#[test]
fn colon_quit_sets_quit_flag() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "quit");
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert!(app.quit);
}

#[test]
fn colon_unknown_verb_shows_error_toast_and_stays_safe() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "nope");
    super::handle_key(&mut app, key(KeyCode::Enter));
    let toast = app.toasts.visible().unwrap_or("");
    assert!(toast.contains("unknown command"), "toast was: {toast:?}");
    assert!(!app.quit);
    // Critical: state didn't drift.
    assert_eq!(app.shelf.shows[0].seen(), 0);
}

#[test]
fn colon_help_opens_help_overlay() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "help");
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.overlay, Overlay::Help);
}

#[test]
fn colon_enter_on_empty_query_runs_selected_suggestion() {
    // With empty query, suggestions = full catalogue in declared
    // order. The first verb is `watched`, so Enter should run it.
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.shelf.shows[0].seen(), 1);
}

#[test]
fn keymap_w_and_colon_watched_take_same_path() {
    // The whole point of the registry: both inputs converge.
    let now = 1_700_000_000;

    let mut a = app_with_one_show(now);
    super::handle_key(&mut a, char_key('w'));
    let after_w = a.shelf.shows[0].seen();

    let mut b = app_with_one_show(now);
    super::handle_key(&mut b, char_key(':'));
    type_str(&mut b, "watched");
    super::handle_key(&mut b, key(KeyCode::Enter));
    let after_palette = b.shelf.shows[0].seen();

    assert_eq!(after_w, after_palette);
    assert_eq!(after_w, 1);
}

// ---------- Search mode (`/`) ----------

#[test]
fn slash_open_then_esc() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key('/'));
    assert_eq!(app.overlay, Overlay::Search);
    super::handle_key(&mut app, key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}

#[test]
fn slash_typing_filters_hits() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key('/'));
    type_str(&mut app, "one");
    assert!(!app.palette.search_hits.is_empty());
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.overlay, Overlay::None);
}

// ---------- Help (`?`) and Esc ----------

#[test]
fn question_mark_opens_help() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key('?'));
    assert_eq!(app.overlay, Overlay::Help);
    super::handle_key(&mut app, key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
}

// ---------- Ctrl-C ----------

#[test]
fn ctrl_c_quits_from_any_overlay() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    super::handle_key(&mut app, ctrl_c);
    assert!(app.quit);
}

// ---------- Empty state / onboarding ----------

#[test]
fn empty_library_is_first_run() {
    let app = empty_app(1_700_000_000);
    assert!(app.is_first_run());
}

#[test]
fn empty_library_a_opens_follow_overlay() {
    let mut app = empty_app(1_700_000_000);
    super::handle_key(&mut app, char_key('a'));
    assert_eq!(app.overlay, Overlay::Follow);
    assert_eq!(app.palette.mode, PaletteMode::Follow);
}

#[test]
fn empty_library_colon_opens_command_overlay() {
    let mut app = empty_app(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    assert_eq!(app.overlay, Overlay::Command);
}

#[test]
fn empty_library_question_mark_opens_help() {
    let mut app = empty_app(1_700_000_000);
    super::handle_key(&mut app, char_key('?'));
    assert_eq!(app.overlay, Overlay::Help);
}

#[test]
fn empty_library_ignores_navigation_keys() {
    let mut app = empty_app(1_700_000_000);
    super::handle_key(&mut app, char_key('j'));
    super::handle_key(&mut app, char_key('w'));
    // Should be no-ops (no panic, no toast about missing selection).
    assert_eq!(app.overlay, Overlay::None);
    assert!(!app.quit);
}

// ---------- Drop ----------

#[test]
fn colon_drop_enter_removes_show_from_shelf() {
    let mut app = app_with_one_show(1_700_000_000);
    assert_eq!(app.shelf.shows.len(), 1);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "drop");
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.shelf.shows.len(), 0);
    assert!(app.is_first_run(), "dropping last show re-enters first-run");

    // Durable: the canonical_release was dropped.
    let cid = CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", "21");
    let row = app.facade.find_canonical(&cid).unwrap().unwrap();
    assert!(row.dropped_at.is_some(), "canonical_release.dropped_at set");
}
