//! The freedesktop notification spec behind the engine's [`NotificationSurface`].
//!
//! Nothing here schedules. `Notify` puts a banner on screen at the moment it is
//! called, the returned id is only good for closing it again, and there is no
//! way to ask the server what it is holding — because it is holding nothing.
//! That is why [`NotificationSurface::holds_schedule`] is false: the reconciler
//! then submits only at airtime and treats the submission as the delivery.
//!
//! There is also no permission model. The desktop decides whether to display
//! what arrives, and never tells the sender — a muted app still gets a
//! successful `Notify`. So the only knowable states are "a server answered" and
//! "there is no server", and the control the owner actually has lives in their
//! desktop's notification settings, which is why the `desktop-entry` hint below
//! is not optional.

use std::collections::HashMap;
use std::sync::Arc;

use zbus::zvariant::Value;

use crate::domain::notification::NativeRequest;
use crate::domain::read_models::AuthorizationState;
use crate::engine::reconciler::{NotificationSurface, PendingRequest, SurfaceError, SurfaceFuture};

/// The `.desktop` file basename, minus the extension.
///
/// Passed as the `desktop-entry` hint on every notification. Without it GNOME
/// and KDE cannot match these banners to an application, so animesh never
/// appears in their per-app notification settings — and the owner has no way to
/// turn it off short of stopping the daemon. On this platform the hint *is* the
/// off switch.
pub const DESKTOP_ENTRY: &str = "animesh";

/// Shown by the server as the sending application.
const APP_NAME: &str = "Animesh";

/// Let the server apply its own timeout. A release banner is worth the same
/// dwell as anything else the owner has configured.
const EXPIRE_DEFAULT: i32 = -1;

/// Normal urgency. A new episode is not a system alert.
const URGENCY_NORMAL: u8 = 1;

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications"
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn get_server_information(&self) -> zbus::Result<(String, String, String, String)>;
}

/// A live connection to the session's notification server.
pub struct DesktopNotifier {
    proxy: NotificationsProxy<'static>,
}

impl std::fmt::Debug for DesktopNotifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopNotifier").finish_non_exhaustive()
    }
}

impl DesktopNotifier {
    /// Connects to the session bus.
    ///
    /// Fails on a headless box, in a container, or over SSH without a session —
    /// all of which are ordinary ways to run the daemon, so the caller logs it
    /// and carries on without banners rather than treating it as fatal.
    pub async fn connect() -> Result<Arc<Self>, String> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| format!("no session bus: {e}"))?;
        let proxy = NotificationsProxy::new(&connection)
            .await
            .map_err(|e| format!("no notification server: {e}"))?;
        Ok(Arc::new(Self { proxy }))
    }
}

impl NotificationSurface for DesktopNotifier {
    /// Notify fires now and keeps no pending list, so the daemon owns the
    /// timing and has to be alive at airtime.
    fn holds_schedule(&self) -> bool {
        false
    }

    /// There is no permission to query, so this reports reachability instead.
    ///
    /// A server that answers means a banner can be submitted; whether the owner
    /// has muted animesh in their desktop settings is deliberately invisible to
    /// us and is not ours to track. No server means `Unknown`, which blocks
    /// registration without raising a degraded reason — a headless machine is
    /// not a fault to be reported, it is a machine that does not want banners.
    fn authorization(&self) -> SurfaceFuture<'_, AuthorizationState> {
        Box::pin(async move {
            match self.proxy.get_server_information().await {
                Ok(_) => Ok(AuthorizationState::Authorized),
                Err(_) => Ok(AuthorizationState::Unknown),
            }
        })
    }

    /// Always empty: the server holds nothing between calls.
    ///
    /// Safe only because `holds_schedule` is false. The reconciler then never
    /// treats an empty list as "the OS lost our request".
    fn pending(&self) -> SurfaceFuture<'_, Vec<PendingRequest>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    /// Always empty: there is no delivered list to read.
    ///
    /// Delivery is recorded from a successful submit instead.
    fn delivered(&self) -> SurfaceFuture<'_, Vec<String>> {
        Box::pin(async move { Ok(Vec::new()) })
    }

    fn add<'a>(&'a self, request: &'a NativeRequest) -> SurfaceFuture<'a, ()> {
        Box::pin(async move {
            let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
            hints.insert("desktop-entry", Value::from(DESKTOP_ENTRY));
            hints.insert("urgency", Value::from(URGENCY_NORMAL));

            self.proxy
                .notify(
                    APP_NAME,
                    // Never replace: each episode is its own banner, and the
                    // ids the server hands back do not survive a restart.
                    0,
                    "",
                    &request.title,
                    &request.body,
                    &[],
                    hints,
                    EXPIRE_DEFAULT,
                )
                .await
                .map(|_| ())
                // Every failure here is worth retrying: the bus may be
                // restarting, and the durable backoff makes retrying the safe
                // default. Classifying one as permanent would abandon the
                // episode for good.
                .map_err(|e| SurfaceError::Transient(e.to_string()))
        })
    }

    /// A no-op.
    ///
    /// Nothing is pending to withdraw, and the only thing `CloseNotification`
    /// could reach is a banner already on screen — which the owner may be in
    /// the middle of reading. Yanking it because a follow was dropped a second
    /// later would be worse than leaving it.
    fn remove(&self, _identifiers: Vec<String>) -> SurfaceFuture<'_, ()> {
        Box::pin(async move { Ok(()) })
    }
}
