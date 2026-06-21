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
use crate::ingest::{
    AliasObservation, HttpMethod, RawSourcePayload, ReleaseEventObservation, SourceObservation,
    TimePrecision,
};
use crate::library::Library as Facade;
use crate::sources::SourceRegistry;
use crate::store::{CacheEntry, EngagementEvent, EngagementMeta};
use crate::time::FixedClock;
use crate::tui::app::{App, Overlay};
use crate::tui::model::Shelf;
use crate::tui::palette::PaletteMode;
use crate::tui::pane::Windows;
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
            Some(EngagementMeta::Verified {
                streamer: "Netflix".into(),
                url: "https://netflix.com/x".into(),
            }),
        )
        .unwrap();
    let mut subs = Subs::default();
    subs.add(&facade, "Netflix").unwrap();
    let windows = Windows::from_env();
    let shelf = Shelf::load(&facade, now, windows, &subs).unwrap();
    App::new(facade, SourceRegistry::empty(), shelf, windows, subs, now)
}

fn empty_app(now: i64) -> App {
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let subs = Subs::default();
    let windows = Windows::from_env();
    let shelf = Shelf::load(&facade, now, windows, &subs).unwrap();
    App::new(facade, SourceRegistry::empty(), shelf, windows, subs, now)
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
    assert_eq!(
        last.seen(),
        Some(1),
        "expected seen=1, meta was: {:?}",
        last.meta
    );
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

// ---------- Theme picker ----------

#[test]
fn theme_shortcut_opens_picker_and_esc_cancels() {
    let mut app = app_with_one_show(1_700_000_000);
    let original = app.active_theme_id.clone();
    super::handle_key(&mut app, char_key('t'));
    assert_eq!(app.overlay, Overlay::Theme);
    super::handle_key(&mut app, key(KeyCode::Down));
    assert!(app.theme_picker.preview_theme_id.is_some());
    super::handle_key(&mut app, key(KeyCode::Esc));
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.active_theme_id, original);
}

#[test]
fn colon_theme_direct_apply_persists_choice() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "theme latte");
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.active_theme_id, "catppuccin-latte");
    assert_eq!(
        app.facade.kv_get(crate::tui::theme::KV_UI_THEME).unwrap(),
        Some("catppuccin-latte".to_string())
    );
}

#[test]
fn colon_theme_without_arg_opens_picker() {
    let mut app = app_with_one_show(1_700_000_000);
    super::handle_key(&mut app, char_key(':'));
    type_str(&mut app, "theme");
    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.overlay, Overlay::Theme);
}

// ---------- Follow candidate mode (`a`) ----------

#[test]
fn follow_palette_typing_searches_local_candidates_and_enter_follows_selected() {
    let now = 1_700_000_000;
    let mut app = empty_app(now);
    let raw = RawSourcePayload {
        id: "raw:jikan:5114".into(),
        source: "jikan".into(),
        endpoint: "anime_search".into(),
        method: HttpMethod::Get,
        request_key: "jikan:anime:fullmetal".into(),
        request_hash: "req".into(),
        request_json: None,
        http_status: 200,
        response_hash: "resp".into(),
        response_json: r#"{"data":[]}"#.into(),
        fetched_at: now,
        expires_at: None,
        created_at: now,
    };
    app.facade.store_raw_source_payload(&raw).unwrap();
    app.facade
        .store_source_observation(&SourceObservation {
            source: "jikan".into(),
            source_id: "5114".into(),
            raw_payload_id: raw.id.clone(),
            kind: ReleaseKind::Anime,
            display_title: "Fullmetal Alchemist: Brotherhood".into(),
            raw_title: Some("Hagane no Renkinjutsushi".into()),
            description: None,
            status: Some("Finished Airing".into()),
            observed_at: now,
            source_updated_at: None,
            aliases: vec![AliasObservation {
                alias: "FMA Brotherhood".into(),
                locale: Some("en".into()),
                alias_kind: Some("synonym".into()),
                confidence: 0.9,
            }],
            external_ids: vec![],
            release_events: vec![],
            links: vec![],
            images: vec![],
        })
        .unwrap();

    super::handle_key(&mut app, char_key('a'));
    type_str(&mut app, "fma");
    assert_eq!(app.overlay, Overlay::Follow);
    assert_eq!(app.palette.follow_hits.len(), 1);
    assert_eq!(app.palette.follow_hits[0].source, "jikan");

    super::handle_key(&mut app, key(KeyCode::Enter));
    assert_eq!(app.overlay, Overlay::None);
    assert_eq!(app.facade.count_followed().unwrap(), 1);
    assert_eq!(app.shelf.shows.len(), 1);
    assert_eq!(
        app.shelf.shows[0].display_title(),
        "Fullmetal Alchemist: Brotherhood"
    );
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

// ---------- Manifesto-behavior tests (T11) ----------

use crate::tui::app::{PANE_DROPPING, PANE_FOLLOWING, PANE_PLAYABLE};
use crate::tui::command::Command;

/// Helper: follow a show and return its CanonicalId.
fn follow_anime(facade: &Facade, source_id: &str, title: &str) -> CanonicalId {
    let cid = CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", source_id);
    facade
        .follow_with_source(
            &cid,
            ReleaseKind::Anime,
            title,
            "anilist",
            source_id,
            Some(title),
            1.0,
        )
        .unwrap();
    cid
}

#[test]
fn verified_subscribed_show_lands_in_playable() {
    let now = 1_700_000_000;
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let cid = follow_anime(&facade, "1", "Frieren");
    facade
        .engage(
            &cid,
            EngagementEvent::Verified,
            Some(EngagementMeta::Verified {
                streamer: "Crunchyroll".into(),
                url: "https://crunchyroll.com/x".into(),
            }),
        )
        .unwrap();
    let mut subs = Subs::default();
    subs.add(&facade, "Crunchyroll").unwrap();
    let windows = Windows::DEFAULT;
    let shelf = Shelf::load(&facade, now, windows, &subs).unwrap();
    let app = App::new(facade, SourceRegistry::empty(), shelf, windows, subs, now);
    assert_eq!(app.items_in(PANE_PLAYABLE).len(), 1);
    assert_eq!(app.items_in(PANE_DROPPING).len(), 0);
    assert_eq!(app.items_in(PANE_FOLLOWING).len(), 0);
}

#[test]
fn verified_unsubscribed_show_is_following_not_playable() {
    // Verified on Apple TV, but user doesn't subscribe to Apple TV.
    // Per the manifesto: catalogued but not playable — dim row in
    // Following, not promoted to Playable.
    let now = 1_700_000_000;
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let cid = follow_anime(&facade, "42", "Severance");
    facade
        .engage(
            &cid,
            EngagementEvent::Verified,
            Some(EngagementMeta::Verified {
                streamer: "Apple TV".into(),
                url: "https://tv.apple.com/x".into(),
            }),
        )
        .unwrap();
    let subs = Subs::default(); // none subscribed
    let windows = Windows::DEFAULT;
    let shelf = Shelf::load(&facade, now, windows, &subs).unwrap();
    let app = App::new(facade, SourceRegistry::empty(), shelf, windows, subs, now);
    assert_eq!(app.items_in(PANE_PLAYABLE).len(), 0);
    assert_eq!(app.items_in(PANE_FOLLOWING).len(), 1);
}

#[test]
fn copy_context_builds_well_formed_json() {
    let now = 1_700_000_000;
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let _cid = follow_anime(&facade, "1", "Frieren");
    let subs = Subs::default();
    let shelf = Shelf::load(&facade, now, Windows::DEFAULT, &subs).unwrap();
    let show = shelf.shows.first().expect("one show in shelf after follow");
    let v = crate::tui::llm_context::build(&facade, show).unwrap();
    assert_eq!(v["title"], "Frieren");
    assert_eq!(v["kind"], "anime");
    let refs = v["refs"].as_array().expect("refs is array");
    assert!(!refs.is_empty(), "follow attaches a source_ref");
    assert_eq!(refs[0]["source"], "anilist");
    assert_eq!(refs[0]["source_id"], "1");
}

#[test]
fn shelf_load_uses_canonical_schedule_event_not_metadata_cache_schedule() {
    let now = 1_700_000_000;
    let facade = Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap());
    let cid = follow_anime(&facade, "stale", "Frieren");

    facade
        .upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: "stale".into(),
            display_title: Some("Frieren".into()),
            title_english: Some("Frieren".into()),
            title_native: None,
            status: Some("RELEASING".into()),
            total_episodes: Some(12),
            format: Some("TV".into()),
            next_episode_number: Some(99),
            next_episode_airs_at: Some(now + 60),
            fetched_at: now,
            expires_at: now + 6 * 3600,
            cover_image_url: None,
            description: None,
            score: None,
            studios: None,
            streaming_links_json: None,
        })
        .unwrap();

    let raw = RawSourcePayload {
        id: "raw:anilist:stale".into(),
        source: "anilist".into(),
        endpoint: "media".into(),
        method: HttpMethod::Get,
        request_key: "anilist:media:stale".into(),
        request_hash: "req".into(),
        request_json: None,
        http_status: 200,
        response_hash: "resp".into(),
        response_json: "{}".into(),
        fetched_at: now,
        expires_at: None,
        created_at: now,
    };
    facade.store_raw_source_payload(&raw).unwrap();

    let observation = SourceObservation {
        source: "anilist".into(),
        source_id: "stale".into(),
        raw_payload_id: raw.id.clone(),
        kind: ReleaseKind::Anime,
        display_title: "Frieren".into(),
        raw_title: Some("Frieren".into()),
        description: None,
        status: Some("RELEASING".into()),
        observed_at: now,
        source_updated_at: None,
        aliases: vec![],
        external_ids: vec![],
        release_events: vec![ReleaseEventObservation {
            id: "anilist:stale:episode:4".into(),
            event_kind: "episode".into(),
            title: Some("Episode 4".into()),
            season: Some(1),
            episode: Some(4),
            local_date: None,
            local_time: None,
            source_timezone: Some("UTC".into()),
            scheduled_at: Some(now + 3_600),
            precision: TimePrecision::Instant,
            confidence: 0.95,
            observed_at: now,
        }],
        links: vec![],
        images: vec![],
    };
    facade.store_source_observation(&observation).unwrap();
    facade
        .project_canonical_schedule_events(&cid, "anilist", &observation)
        .unwrap();

    let shelf = Shelf::load(&facade, now, Windows::DEFAULT, &Subs::default()).unwrap();
    let show = shelf.shows.first().expect("one show");
    assert_eq!(show.next_episode(), Some(4));
    assert_eq!(show.airs_at(), Some(now + 3_600));
    assert_eq!(show.next_drop_at(), Some(now + 3_600));
    assert_eq!(show.pane, Some(crate::tui::pane::Pane::Dropping));
}

#[test]
fn subs_add_remove_via_command() {
    let mut app = empty_app(1_700_000_000);
    app.dispatch(Command::SubsAdd("Netflix".into()));
    assert!(app.subs.matches("netflix"));
    assert!(app.subs.matches("NETFLIX"), "match is case-insensitive");
    app.dispatch(Command::SubsRemove("Netflix".into()));
    assert!(!app.subs.matches("netflix"));
}
