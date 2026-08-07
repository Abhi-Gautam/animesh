//! Scratch: render the CLI surfaces with a realistic seasonal lineup.
#![allow(clippy::expect_used)]

use animesh::cli::render;
use animesh::domain::ids::{
    AniListId, BoundedText, EpisodeNumber, EventUuid, MediaId, ReleaseEventId, UnixTimestamp,
};
use animesh::domain::media::{MediaStatus, SearchCandidate, TitleSet};
use animesh::domain::read_models::{
    BootstrapState, DegradedReason, FollowOutcome, FollowResult, FollowSummary, Freshness,
    HealthSnapshot, NotificationCounts, RefreshCounts, UpcomingRelease,
};
use animesh::domain::release::FollowState;

fn at(seconds: i64) -> UnixTimestamp {
    UnixTimestamp::new(seconds).expect("valid timestamp")
}

fn text(value: &str) -> BoundedText {
    BoundedText::new("title", 512, value).expect("valid title")
}

const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

struct Show {
    media: i64,
    anilist: i64,
    title: &'static str,
    episode: Option<i64>,
    offset: Option<i64>,
    freshness: Freshness,
}

fn lineup() -> Vec<Show> {
    vec![
        Show {
            media: 4,
            anilist: 185_660,
            title: "Kaiju No. 8 Season 2",
            episode: Some(7),
            offset: Some(40 * 60),
            freshness: Freshness::Fresh,
        },
        Show {
            media: 9,
            anilist: 178_680,
            title: "SAKAMOTO DAYS Part 2",
            episode: Some(19),
            offset: Some(3 * HOUR + 25 * 60),
            freshness: Freshness::Fresh,
        },
        Show {
            media: 2,
            anilist: 166_531,
            title: "Kono Subarashii Sekai ni Shukufuku wo! Kurenai Densetsu Gaiden",
            episode: Some(4),
            offset: Some(11 * HOUR),
            freshness: Freshness::Stale,
        },
        Show {
            media: 1,
            anilist: 21,
            title: "One Piece",
            episode: Some(1169),
            offset: Some(2 * DAY + 2 * HOUR),
            freshness: Freshness::Fresh,
        },
        Show {
            media: 7,
            anilist: 151_514,
            title: "葬送のフリーレン 第2期",
            episode: Some(13),
            offset: Some(3 * DAY + 30 * 60),
            freshness: Freshness::BackingOff,
        },
        Show {
            media: 11,
            anilist: 163_132,
            title: "The Apothecary Diaries Season 3",
            episode: None,
            offset: Some(5 * DAY + 6 * HOUR),
            freshness: Freshness::Fresh,
        },
    ]
}

fn upcoming_row(show: &Show, now: i64) -> Option<UpcomingRelease> {
    Some(UpcomingRelease {
        release_event_id: ReleaseEventId::new(show.media * 10).expect("id"),
        event_uuid: EventUuid::generate(),
        media_id: MediaId::new(show.media).expect("id"),
        anilist_id: AniListId::new(show.anilist).expect("id"),
        display_title: text(show.title),
        episode: show.episode.map(|e| EpisodeNumber::new(e).expect("ep")),
        scheduled_at: at(now + show.offset?),
        schedule_revision: 1,
        last_success_at: Some(at(now - 2 * HOUR)),
        freshness: show.freshness,
    })
}

#[test]
fn print_every_surface() {
    let now_i = chrono::Utc::now().timestamp();
    let now = at(now_i);
    let shows = lineup();

    let rows: Vec<UpcomingRelease> = shows
        .iter()
        .filter_map(|s| upcoming_row(s, now_i))
        .collect();

    println!("\n########## $ animesh next ##########");
    println!("{}", render::upcoming(&rows, now));

    println!("\n########## $ animesh next  (empty) ##########");
    println!("{}", render::upcoming(&[], now));

    // list: everything followed, including two with no scheduled episode.
    let mut summaries: Vec<FollowSummary> = shows
        .iter()
        .map(|s| FollowSummary {
            media_id: MediaId::new(s.media).expect("id"),
            anilist_id: AniListId::new(s.anilist).expect("id"),
            display_title: text(s.title),
            state: FollowState::Active,
            upcoming: upcoming_row(s, now_i),
            last_success_at: Some(at(now_i - 2 * HOUR)),
            freshness: s.freshness,
        })
        .collect();
    summaries.push(FollowSummary {
        media_id: MediaId::new(15).expect("id"),
        anilist_id: AniListId::new(170_942).expect("id"),
        display_title: text("Dandadan Season 3"),
        state: FollowState::Active,
        upcoming: None,
        last_success_at: Some(at(now_i - 9 * DAY)),
        freshness: Freshness::Stale,
    });
    summaries.push(FollowSummary {
        media_id: MediaId::new(18).expect("id"),
        anilist_id: AniListId::new(154_587).expect("id"),
        display_title: text("Solo Leveling Season 3"),
        state: FollowState::Active,
        upcoming: None,
        last_success_at: Some(at(now_i - 31 * DAY)),
        freshness: Freshness::Fresh,
    });

    println!("\n########## $ animesh list ##########");
    println!("{}", render::follows(&summaries, now));

    println!("\n########## $ animesh search chainsaw ##########");
    let candidates = vec![
        SearchCandidate {
            anilist_id: AniListId::new(127_230).expect("id"),
            display_title: text("Chainsaw Man"),
            titles: TitleSet::default(),
            status: MediaStatus::Finished,
            format: Some(text("TV")),
            episode_count: Some(EpisodeNumber::new(12).expect("ep")),
            season_year: Some(2022),
        },
        SearchCandidate {
            anilist_id: AniListId::new(157_371).expect("id"),
            display_title: text("Chainsaw Man - The Movie: Reze Arc"),
            titles: TitleSet::default(),
            status: MediaStatus::Finished,
            format: Some(text("MOVIE")),
            episode_count: Some(EpisodeNumber::new(1).expect("ep")),
            season_year: Some(2025),
        },
        SearchCandidate {
            anilist_id: AniListId::new(190_001).expect("id"),
            display_title: text("Chainsaw Man Season 2"),
            titles: TitleSet::default(),
            status: MediaStatus::NotYetReleased,
            format: Some(text("TV")),
            episode_count: None,
            season_year: None,
        },
        SearchCandidate {
            anilist_id: AniListId::new(21).expect("id"),
            display_title: text("チェンソーマン レゼ篇"),
            titles: TitleSet::default(),
            status: MediaStatus::Hiatus,
            format: Some(text("TV")),
            episode_count: Some(EpisodeNumber::new(24).expect("ep")),
            season_year: Some(2026),
        },
    ];
    println!("{}", render::search(&candidates));

    println!("\n########## $ animesh follow 185660 ##########");
    let result = FollowResult {
        media_id: MediaId::new(4).expect("id"),
        anilist_id: AniListId::new(185_660).expect("id"),
        display_title: text("Kaiju No. 8 Season 2"),
        outcome: FollowOutcome::NewlyFollowed,
        upcoming: upcoming_row(&shows[0], now_i),
        last_success_at: Some(now),
    };
    println!("{}", render::follow_result(&result, now));

    println!("\n########## $ animesh follow 190001  (nothing scheduled) ##########");
    let pending = FollowResult {
        media_id: MediaId::new(22).expect("id"),
        anilist_id: AniListId::new(190_001).expect("id"),
        display_title: text("Chainsaw Man Season 2"),
        outcome: FollowOutcome::NewlyFollowed,
        upcoming: None,
        last_success_at: Some(now),
    };
    println!("{}", render::follow_result(&pending, now));

    println!("\n########## $ animesh follow 21  (already following) ##########");
    let dupe = FollowResult {
        media_id: MediaId::new(1).expect("id"),
        anilist_id: AniListId::new(21).expect("id"),
        display_title: text("One Piece"),
        outcome: FollowOutcome::AlreadyActive,
        upcoming: upcoming_row(&shows[3], now_i),
        last_success_at: Some(now),
    };
    println!("{}", render::follow_result(&dupe, now));

    println!("\n########## $ animesh status ##########");
    let snapshot = HealthSnapshot {
        process_version: "0.1.0".to_owned(),
        schema_version: 1,
        protocol_version: 1,
        app_instance_id: "0f2a9d1c-6b7e-4f5a-9c31-88ab0d4e1c22".to_owned(),
        started_at: at(now_i - 3 * DAY - 4 * HOUR),
        bootstrap: BootstrapState::Ready,
        database_ready: true,
        active_follows: 8,
        earliest_upcoming: upcoming_row(&shows[0], now_i),
        last_success_at: Some(at(now_i - 2 * HOUR)),
        refresh: RefreshCounts {
            due: 2,
            stale: 2,
            backing_off: 1,
            failed: 0,
        },
        source_blocked_until: None,
        notifications: NotificationCounts {
            desired: 8,
            registered: 7,
            failed: 0,
            deferred_capacity: 1,
        },
        last_reconciled_at: Some(at(now_i - 40 * 60)),
        authorization: animesh::domain::read_models::AuthorizationState::Authorized,
        degraded: vec![],
        authorization_observed_at: Some(at(now_i - 40 * 60)),
    };
    println!("{}", render::health(&snapshot, now));

    println!("\n########## $ animesh status  (degraded + rate limited) ##########");
    let bad = HealthSnapshot {
        source_blocked_until: Some(at(now_i + 9 * 60)),
        degraded: vec![
            DegradedReason::NotificationsDenied,
            DegradedReason::SourceRateLimited,
        ],
        last_success_at: None,
        earliest_upcoming: None,
        bootstrap: BootstrapState::Degraded,
        ..snapshot
    };
    println!("{}", render::health(&bad, now));

    println!("\n########## relative() sweep ##########");
    for (label, secs) in [
        ("30s", 30_i64),
        ("5m", 300),
        ("59m", 59 * 60),
        ("3h10m", 3 * HOUR + 600),
        ("23h59m", 23 * HOUR + 59 * 60),
        ("25h", 25 * HOUR),
        ("50h", 50 * HOUR),
        ("6d", 6 * DAY),
        ("13d", 13 * DAY),
        ("past", -7200),
    ] {
        println!(
            "{label:>8} -> {:<14} | {}",
            render::relative(now, at(now_i + secs)),
            render::local_time(at(now_i + secs))
        );
    }
}
