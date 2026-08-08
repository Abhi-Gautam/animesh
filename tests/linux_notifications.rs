//! The freedesktop adapter against a real session bus.
//!
//! Skips when there is no bus, which is every macOS run and any CI job without
//! a session — the assertions below only mean something with a server present.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used)]

use animesh::domain::ids::{AniListId, EpisodeNumber, EventUuid, InstallationUuid, UnixTimestamp};
use animesh::domain::notification::{DeliveryMode, NativeRequest, OsIdentifier};
use animesh::domain::read_models::AuthorizationState;
use animesh::engine::reconciler::NotificationSurface;
use animesh::platform::linux::notifications::DesktopNotifier;

fn request() -> NativeRequest {
    NativeRequest::build(
        OsIdentifier::new(&InstallationUuid::generate(), &EventUuid::generate()),
        DeliveryMode::CatchUp,
        UnixTimestamp::new(1_700_000_000).expect("timestamp"),
        "One Piece",
        EpisodeNumber::new(1169).expect("episode"),
        AniListId::new(21).expect("id"),
    )
}

#[tokio::test]
async fn the_adapter_speaks_the_freedesktop_protocol() {
    if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none() {
        return;
    }

    let Ok(surface) = DesktopNotifier::connect().await else {
        return;
    };

    // These touch no bus, so they hold even on a session with no server.
    assert!(!surface.holds_schedule());
    // Nothing is held between calls, and the reconciler relies on that being
    // an honest empty rather than a lost request.
    assert!(surface.pending().await.expect("pending").is_empty());
    assert!(surface.delivered().await.expect("delivered").is_empty());

    // A bare session bus with nothing implementing the spec — a CI runner —
    // reports Unknown. There is nothing left to assert against, so stop.
    if surface.authorization().await.expect("authorization") != AuthorizationState::Authorized {
        return;
    }

    // The eight-argument Notify signature is the part most likely to be wrong.
    surface.add(&request()).await.expect("notify");

    // Removal is a no-op, not an error.
    surface
        .remove(vec!["whatever".to_owned()])
        .await
        .expect("remove");
}
