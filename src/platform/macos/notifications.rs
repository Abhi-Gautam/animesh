//! `UNUserNotificationCenter` behind the engine's [`NotificationSurface`] port.
//!
//! Every Apple call here is completion-handler based and lands on an arbitrary
//! internal queue. Each one is bridged to a `oneshot` so the reconciler, which
//! is ordinary async Rust, never blocks a thread waiting on a callback and
//! never has to know a run loop exists.

use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_foundation::{
    NSArray, NSCalendar, NSCalendarUnit, NSDate, NSDictionary, NSError, NSString, NSTimeZone,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNCalendarNotificationTrigger, UNMutableNotificationContent,
    UNNotification, UNNotificationRequest, UNNotificationSettings, UNNotificationSound,
    UNUserNotificationCenter,
};
use tokio::sync::oneshot;

use crate::domain::notification::{DeliveryMode, NativeRequest};
use crate::domain::read_models::AuthorizationState;
use crate::engine::reconciler::{NotificationSurface, PendingRequest, SurfaceError, SurfaceFuture};

/// Where the canonical request JSON is stashed so a later pass can tell whether
/// a pending request is still the one Animesh wants.
///
/// The OS will not hand back the trigger it holds in comparable form, so change
/// detection has to travel with the request itself.
const USER_INFO_REQUEST: &str = "animesh.request";

/// The live notification centre.
///
/// `UNUserNotificationCenter` is documented as thread-safe, so this holds the
/// singleton and does no locking of its own.
pub struct NotificationCenter {
    centre: Retained<UNUserNotificationCenter>,
}

// SAFETY: every method used here is a UNUserNotificationCenter API Apple
// documents as callable from any thread, and the Retained pointer is only ever
// read. The reconciler serializes passes, so no two calls overlap in practice.
unsafe impl Send for NotificationCenter {}
unsafe impl Sync for NotificationCenter {}

impl std::fmt::Debug for NotificationCenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationCenter").finish_non_exhaustive()
    }
}

impl NotificationCenter {
    /// Fails outside an app bundle.
    ///
    /// `currentNotificationCenter` traps when there is no bundle identifier, so
    /// `cargo run` on the bare binary would abort rather than return an error.
    /// Checking first turns that into a message the owner can act on.
    pub fn new() -> Result<Arc<Self>, String> {
        if !running_in_bundle() {
            return Err(
                "not running from an app bundle; install with `cargo xtask install`".to_owned(),
            );
        }
        let centre = UNUserNotificationCenter::currentNotificationCenter();
        Ok(Arc::new(Self { centre }))
    }

    /// Raises the system permission prompt.
    ///
    /// Deliberately not called during reconciliation: the plan requires the
    /// prompt to follow a user action, so the menu and the first successful
    /// follow are its only callers.
    pub async fn request_authorization(&self) -> Result<bool, SurfaceError> {
        let (tx, rx) = oneshot::channel();
        {
            let sender = std::cell::RefCell::new(Some(tx));
            let handler =
                RcBlock::new(move |granted: objc2::runtime::Bool, error: *mut NSError| {
                    if let Some(tx) = sender.borrow_mut().take() {
                        let outcome = if error.is_null() {
                            Ok(granted.as_bool())
                        } else {
                            Err(describe(error))
                        };
                        let _ = tx.send(outcome);
                    }
                });
            {
                self.centre
                    .requestAuthorizationWithOptions_completionHandler(
                        UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                        &handler,
                    );
            }
        }

        match rx.await {
            Ok(Ok(granted)) => Ok(granted),
            Ok(Err(message)) => Err(SurfaceError::Permanent(message)),
            Err(_) => Err(SurfaceError::Transient(
                "the authorization callback never fired".to_owned(),
            )),
        }
    }
}

/// Whether this process is running from a bundle with an identifier.
fn running_in_bundle() -> bool {
    use objc2_foundation::NSBundle;
    let bundle = NSBundle::mainBundle();
    bundle.bundleIdentifier().is_some()
}

fn describe(error: *mut NSError) -> String {
    if error.is_null() {
        return "unknown error".to_owned();
    }
    // SAFETY: checked non-null; the callback owns the reference for its
    // duration and we only read from it.
    let error = unsafe { &*error };
    error.localizedDescription().to_string()
}

/// Maps an `UNNotificationSettings` authorization to the domain enum.
fn authorization_of(settings: &UNNotificationSettings) -> AuthorizationState {
    use objc2_user_notifications::UNAuthorizationStatus;

    match settings.authorizationStatus() {
        UNAuthorizationStatus::NotDetermined => AuthorizationState::NotDetermined,
        UNAuthorizationStatus::Denied => AuthorizationState::Denied,
        UNAuthorizationStatus::Authorized => AuthorizationState::Authorized,
        UNAuthorizationStatus::Provisional => AuthorizationState::Provisional,
        UNAuthorizationStatus::Ephemeral => AuthorizationState::Ephemeral,
        // A status this build predates. Treated as unknown rather than as
        // permission, so nothing is submitted on a guess.
        _ => AuthorizationState::Unknown,
    }
}

/// Rebuilds a [`PendingRequest`] from what the OS is holding.
fn pending_of(request: &UNNotificationRequest) -> PendingRequest {
    let identifier = request.identifier().to_string();
    let content = request.content();
    let info = content.userInfo();

    let key = NSString::from_str(USER_INFO_REQUEST);
    let json = info.objectForKey(&key).and_then(|value| {
        // Anything other than the string we stored is a request this build did
        // not write, which the reconciler treats as a mismatch.
        value.downcast::<NSString>().ok().map(|s| s.to_string())
    });

    PendingRequest {
        os_identifier: identifier,
        request_json: json,
    }
}

/// Builds the native request, carrying its canonical JSON in `userInfo`.
fn build_request(request: &NativeRequest) -> Result<Retained<UNNotificationRequest>, SurfaceError> {
    let json = request
        .canonical_json()
        .map_err(|e| SurfaceError::Permanent(format!("cannot serialize the request: {e}")))?;

    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&request.title));
    content.setBody(&NSString::from_str(&request.body));
    if request.sound {
        content.setSound(Some(&UNNotificationSound::defaultSound()));
    }

    let keys = [
        NSString::from_str(USER_INFO_REQUEST),
        NSString::from_str("animesh.url"),
    ];
    let values: [Retained<NSString>; 2] =
        [NSString::from_str(&json), NSString::from_str(&request.url)];
    let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let value_refs: Vec<&AnyObject> = values.iter().map(|v| &**v as &AnyObject).collect();
    let info = NSDictionary::from_slices(&key_refs, &value_refs);

    // SAFETY: the key and value parameters on NSDictionary are phantom, so the
    // concrete NSString key type and the erased AnyObject one share a layout.
    // setUserInfo takes the erased form.
    let info: Retained<NSDictionary<AnyObject, AnyObject>> =
        unsafe { Retained::cast_unchecked(info) };
    unsafe { content.setUserInfo(&info) };

    // Catch-up fires on submission: the airtime already passed, so a calendar
    // trigger in the past would never fire at all.
    let trigger = match request.mode {
        DeliveryMode::CatchUp => None,
        DeliveryMode::Scheduled => Some(calendar_trigger(request.fire_at.get())?),
    };

    let identifier = NSString::from_str(request.os_identifier.as_str());
    Ok(
        UNNotificationRequest::requestWithIdentifier_content_trigger(
            &identifier,
            &content,
            trigger.as_deref().map(|t| t as _),
        ),
    )
}

/// A non-repeating trigger at an absolute instant, in UTC.
///
/// UTC components rather than local ones: the owner's timezone can change
/// between registration and airtime, and a local-component trigger would move
/// the notification with it.
fn calendar_trigger(
    unix_seconds: i64,
) -> Result<Retained<UNCalendarNotificationTrigger>, SurfaceError> {
    let date = NSDate::dateWithTimeIntervalSince1970(unix_seconds as f64);

    // A fresh Gregorian calendar, never `currentCalendar`: that one is shared
    // process-wide, so setting a timezone on it would reach every other user of
    // it. Gregorian explicitly, because the components below are only correct
    // under it.
    let calendar = unsafe {
        NSCalendar::initWithCalendarIdentifier(
            NSCalendar::alloc(),
            objc2_foundation::NSCalendarIdentifierGregorian,
        )
    }
    .ok_or_else(|| SurfaceError::Permanent("no Gregorian calendar".to_owned()))?;

    let utc = NSTimeZone::timeZoneWithAbbreviation(&NSString::from_str("UTC"))
        .ok_or_else(|| SurfaceError::Permanent("no UTC timezone".to_owned()))?;
    calendar.setTimeZone(&utc);

    let units = NSCalendarUnit::Year
        | NSCalendarUnit::Month
        | NSCalendarUnit::Day
        | NSCalendarUnit::Hour
        | NSCalendarUnit::Minute
        | NSCalendarUnit::Second;
    let components = calendar.components_fromDate(units, &date);
    components.setTimeZone(Some(&utc));

    Ok(
        UNCalendarNotificationTrigger::triggerWithDateMatchingComponents_repeats(
            &components,
            false,
        ),
    )
}

impl NotificationSurface for NotificationCenter {
    fn authorization(&self) -> SurfaceFuture<'_, AuthorizationState> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            // Scoped so the block is released before the await. The OS retains
            // its own copy; holding ours across a suspend point would make the
            // future non-Send for no benefit.
            {
                let sender = std::cell::RefCell::new(Some(tx));
                let handler =
                    RcBlock::new(move |settings: std::ptr::NonNull<UNNotificationSettings>| {
                        if let Some(tx) = sender.borrow_mut().take() {
                            // SAFETY: the OS guarantees a valid settings object
                            // for the duration of the callback.
                            let state = authorization_of(unsafe { settings.as_ref() });
                            let _ = tx.send(state);
                        }
                    });
                self.centre
                    .getNotificationSettingsWithCompletionHandler(&handler);
            }

            rx.await.map_err(|_| {
                SurfaceError::Transient("the settings callback never fired".to_owned())
            })
        })
    }

    fn pending(&self) -> SurfaceFuture<'_, Vec<PendingRequest>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            {
                let sender = std::cell::RefCell::new(Some(tx));
                let handler = RcBlock::new(
                    move |requests: std::ptr::NonNull<NSArray<UNNotificationRequest>>| {
                        if let Some(tx) = sender.borrow_mut().take() {
                            // SAFETY: valid for the callback's duration.
                            let requests = unsafe { requests.as_ref() };
                            let pending = requests.to_vec().iter().map(|r| pending_of(r)).collect();
                            let _ = tx.send(pending);
                        }
                    },
                );
                {
                    self.centre
                        .getPendingNotificationRequestsWithCompletionHandler(&handler);
                }
            }

            rx.await
                .map_err(|_| SurfaceError::Transient("the pending callback never fired".to_owned()))
        })
    }

    fn delivered(&self) -> SurfaceFuture<'_, Vec<String>> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            {
                let sender = std::cell::RefCell::new(Some(tx));
                let handler = RcBlock::new(
                    move |notifications: std::ptr::NonNull<NSArray<UNNotification>>| {
                        if let Some(tx) = sender.borrow_mut().take() {
                            // SAFETY: valid for the callback's duration.
                            let notifications = unsafe { notifications.as_ref() };
                            let ids = notifications
                                .to_vec()
                                .iter()
                                .map(|n| n.request().identifier().to_string())
                                .collect::<Vec<_>>();
                            let _ = tx.send(ids);
                        }
                    },
                );
                {
                    self.centre
                        .getDeliveredNotificationsWithCompletionHandler(&handler);
                }
            }

            rx.await.map_err(|_| {
                SurfaceError::Transient("the delivered callback never fired".to_owned())
            })
        })
    }

    fn add<'a>(&'a self, request: &'a NativeRequest) -> SurfaceFuture<'a, ()> {
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            {
                let native = build_request(request)?;
                let sender = std::cell::RefCell::new(Some(tx));
                let handler = RcBlock::new(move |error: *mut NSError| {
                    if let Some(tx) = sender.borrow_mut().take() {
                        let _ = tx.send(if error.is_null() {
                            Ok(())
                        } else {
                            Err(describe(error))
                        });
                    }
                });
                {
                    self.centre
                        .addNotificationRequest_withCompletionHandler(&native, Some(&handler));
                }
            }

            match rx.await {
                Ok(Ok(())) => Ok(()),
                // Apple does not distinguish retryable add failures, and the
                // durable backoff makes retrying the safe default: a permanent
                // classification would abandon the episode forever.
                Ok(Err(message)) => Err(SurfaceError::Transient(message)),
                Err(_) => Err(SurfaceError::Transient(
                    "the add callback never fired".to_owned(),
                )),
            }
        })
    }

    fn remove(&self, identifiers: Vec<String>) -> SurfaceFuture<'_, ()> {
        Box::pin(async move {
            let strings: Vec<Retained<NSString>> = identifiers
                .iter()
                .map(|id| NSString::from_str(id))
                .collect();
            let refs: Vec<&NSString> = strings.iter().map(|s| &**s).collect();
            let array = NSArray::from_slice(&refs);

            // Synchronous with no completion handler, so there is nothing to
            // await and nothing that can fail.
            {
                self.centre
                    .removePendingNotificationRequestsWithIdentifiers(&array);
            }
            Ok(())
        })
    }
}
