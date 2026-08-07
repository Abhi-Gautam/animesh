//! Glue between the engine and the two macOS surfaces.
//!
//! Runs on the engine thread, never the main one. It owns the reconciler, turns
//! menu commands into Library calls, and publishes the snapshot the menu draws
//! from. AppKit types do not appear here — only the channel ends.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::watch;

use crate::domain::read_models::{HealthSnapshot, UpcomingRelease};
use crate::engine::reconciler::{NotificationSurface, Reconciler};
use crate::engine::WakeHandle;
use crate::library::service::Library;

use super::menu_bar::{MenuCommand, MenuModel, MenuRow, MenuState};
use super::notifications::NotificationCenter;

/// How often a pass runs with nothing having asked for one.
///
/// A backstop, not the mechanism: follows, refreshes, and menu opens all
/// trigger reconciliation directly. This only catches the case where macOS
/// dropped a request without telling anyone.
const SAFETY_TICK: Duration = Duration::from_secs(15 * 60);

/// How many releases the menu lists before it stops being a glance.
const MENU_ROWS: u32 = 8;

pub async fn run_surface(
    library: Arc<Library>,
    wake: WakeHandle,
    state: MenuState,
    mut commands: UnboundedReceiver<MenuCommand>,
    shutdown: watch::Sender<bool>,
) {
    let centre = match NotificationCenter::new() {
        Ok(centre) => Some(centre),
        Err(reason) => {
            // Not fatal. The schedule, the CLI, and the menu all still work;
            // only the banners are missing, and health says so.
            tracing::warn!(reason = %reason, "no notification centre; running without banners");
            None
        }
    };
    let reconciler = centre.as_ref().map(|centre| {
        Reconciler::new(
            Arc::clone(&library),
            Arc::clone(centre) as Arc<dyn NotificationSurface>,
        )
    });

    // Settle before waiting for anything. A fresh process knows nothing about
    // what macOS is holding, and the answer is what health reports — waiting
    // for a menu click to find out would leave the app blank until one came.
    settle(reconciler.as_ref()).await;
    republish(&library, &state).await;

    loop {
        let command = tokio::select! {
            command = commands.recv() => command,
            _ = tokio::time::sleep(SAFETY_TICK) => Some(MenuCommand::Opened),
        };

        match command {
            // A closed channel means the main thread is gone, which is the same
            // situation as an explicit quit.
            Some(MenuCommand::Quit) | None => {
                let _ = shutdown.send(true);
                return;
            }
            Some(MenuCommand::RefreshNow) => {
                if let Err(error) = library.trigger_refresh().await {
                    tracing::warn!(error = %error.message, "menu refresh failed");
                }
                wake.trigger();
            }
            Some(MenuCommand::EnableNotifications) => {
                if let Some(centre) = &centre {
                    match centre.request_authorization().await {
                        Ok(granted) => tracing::info!(granted, "authorization answered"),
                        Err(error) => tracing::warn!(error = %error, "authorization failed"),
                    }
                }
            }
            Some(MenuCommand::Opened) => {}
        }

        settle(reconciler.as_ref()).await;
        republish(&library, &state).await;
    }
}

async fn settle(reconciler: Option<&Reconciler>) {
    let Some(reconciler) = reconciler else {
        return;
    };
    match reconciler.settle().await {
        Ok(report) => {
            if !report.is_quiet() {
                tracing::info!(
                    submitted = report.submitted,
                    delivered = report.delivered,
                    removed = report.removed,
                    failed = report.failed,
                    "reconciled"
                );
            }
        }
        Err(error) => tracing::warn!(error = %error.message, "reconciliation failed"),
    }
}

async fn republish(library: &Library, state: &MenuState) {
    let health = match library.health().await {
        Ok(health) => health,
        Err(error) => {
            tracing::warn!(error = %error.message, "cannot read health for the menu");
            return;
        }
    };
    let upcoming = library.upcoming(Some(MENU_ROWS)).await.unwrap_or_default();
    state.publish(model(&health, &upcoming));
}

fn model(health: &HealthSnapshot, upcoming: &[UpcomingRelease]) -> MenuModel {
    MenuModel {
        summary: summary(health),
        rows: upcoming.iter().map(row).collect(),
        // Only the first reason: a menu that lists every problem at once is a
        // log, and the owner acts on one thing at a time.
        attention: health
            .degraded
            .first()
            .map(|reason| reason.remediation().to_owned()),
        notifications_enabled: health.authorization.permits_registration(),
    }
}

fn summary(health: &HealthSnapshot) -> String {
    match health.active_follows {
        0 => "Not following anything yet".to_owned(),
        1 => "Following 1 title".to_owned(),
        count => format!("Following {count} titles"),
    }
}

fn row(release: &UpcomingRelease) -> MenuRow {
    let episode = release
        .episode
        .map_or_else(|| "next episode".to_owned(), |e| format!("Episode {e}"));
    let when = crate::cli::render::local_time(release.scheduled_at);
    MenuRow {
        title: release.display_title.as_str().to_owned(),
        detail: if release.aired {
            format!("{episode} aired {when}")
        } else {
            format!("{episode} {when}")
        },
    }
}
