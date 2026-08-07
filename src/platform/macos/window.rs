//! The main window: an `NSWindow` hosting a webview.
//!
//! The UI is HTML and CSS rather than AppKit views so that it survives the move
//! to other platforms unchanged — the same markup runs on `WKWebView`,
//! `WebView2`, and `WebKitGTK`. Only this file, which owns the window and the
//! bridge, is per-platform.
//!
//! The page talks to the engine over the same [`Request`]/[`Response`] envelopes
//! the CLI uses. That makes the window a client rather than an insider: it can
//! move out of this process, or onto a socket, without the UI changing.

use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::MainThreadOnly;
use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
};
use wry::http::{header, Response as HttpResponse};
use wry::{WebView, WebViewBuilder};

use super::view_model::WindowState;

const INDEX_HTML: &str = include_str!("../../../ui/index.html");

/// The page's origin.
///
/// A host is part of the origin, so it has to be spelled the same here and in
/// every request the page makes; `animesh://snapshot` is a different origin
/// from `animesh://localhost` and would be refused. The page therefore asks
/// with relative paths, and this is the only place the origin is written.
const ORIGIN: &str = "animesh://localhost";

/// Wraps an `NSWindow` so wry can find the view to attach to.
struct WindowTarget(Retained<NSWindow>);

impl HasWindowHandle for WindowTarget {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let view = self.0.contentView().ok_or(HandleError::Unavailable)?;
        let ptr = NonNull::new(Retained::as_ptr(&view).cast_mut().cast())
            .ok_or(HandleError::Unavailable)?;
        // SAFETY: the handle borrows from `self`, which owns the window, so the
        // view outlives the returned handle.
        unsafe {
            Ok(WindowHandle::borrow_raw(RawWindowHandle::AppKit(
                AppKitWindowHandle::new(ptr),
            )))
        }
    }
}

/// The window and its webview, held for the process's lifetime.
pub struct MainWindow {
    window: WindowTarget,
    _webview: WebView,
}

impl std::fmt::Debug for MainWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MainWindow").finish_non_exhaustive()
    }
}

impl MainWindow {
    pub fn open(mtm: MainThreadMarker, state: WindowState) -> Result<Self, String> {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1040.0, 720.0));
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            // The title bar is part of the page, not a separate strip above it.
            | NSWindowStyleMask::FullSizeContentView;

        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        window.setTitle(&NSString::from_str("Animesh"));
        window.setTitlebarAppearsTransparent(true);
        window.center();
        // The app holds the window, so closing it must hide rather than free it.
        // SAFETY: no other code holds a reference that this ownership change
        // could invalidate; the window is created here and kept here.
        unsafe { window.setReleasedWhenClosed(false) };

        let target = WindowTarget(window);
        let webview = WebViewBuilder::new()
            .with_url(ORIGIN)
            .with_custom_protocol("animesh".to_owned(), move |_id, request| {
                let (kind, body) = match request.uri().path() {
                    "/snapshot" => ("application/json", state.snapshot_json()),
                    _ => ("text/html", INDEX_HTML.to_owned()),
                };
                HttpResponse::builder()
                    .header(header::CONTENT_TYPE, kind)
                    .header(header::CACHE_CONTROL, "no-store")
                    .body(std::borrow::Cow::Owned(body.into_bytes()))
                    .unwrap_or_else(|_| HttpResponse::new(std::borrow::Cow::Owned(b"{}".to_vec())))
            })
            .build(&target)
            .map_err(|e| format!("cannot create the webview: {e}"))?;

        show_window(mtm, &target.0);

        // Lives on the main thread for the process's lifetime. Not shared:
        // AppKit and the webview are both main-thread-only, so an Arc would
        // promise a reach nothing can use.
        Ok(Self {
            window: target,
            _webview: webview,
        })
    }

    pub fn show(&self, mtm: MainThreadMarker) {
        show_window(mtm, &self.window.0);
    }
}

/// Brings the window forward, activating the app first.
///
/// An `Accessory` app is not in the activation order, so ordering a window to
/// the front does nothing on its own — it lands behind whatever the owner is
/// actually using. Activating is what a menu-bar app has to do to be seen.
fn show_window(mtm: MainThreadMarker, window: &NSWindow) {
    let app = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
}
