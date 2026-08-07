//! The status item: Animesh's only visible surface.
//!
//! Everything here runs on the main thread, which AppKit requires. It performs
//! no I/O and holds no engine handle — menu clicks are pushed onto a channel
//! and answered by the engine thread, and what the menu displays comes from a
//! snapshot that thread publishes. That split is what keeps a slow refresh from
//! ever blocking the menu from opening.

use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSMenu, NSMenuDelegate, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString};
use tokio::sync::mpsc::UnboundedSender;

/// What a menu click asks the engine to do.
///
/// Deliberately not a request for data: the menu never waits on the engine, so
/// there is nothing to answer synchronously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuCommand {
    /// The menu is opening; re-read authorization and reconcile.
    Opened,
    RefreshNow,
    EnableNotifications,
    Quit,
}

/// One line in the releases section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuRow {
    pub title: String,
    pub detail: String,
}

/// Everything the menu draws, published by the engine thread.
///
/// A plain snapshot rather than a live query: rendering must never depend on
/// the database being reachable at the moment the owner clicks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MenuModel {
    pub summary: String,
    pub rows: Vec<MenuRow>,
    /// Shown only when there is something the owner can act on.
    pub attention: Option<String>,
    pub notifications_enabled: bool,
}

/// The handle the engine thread writes through.
#[derive(Debug, Clone)]
pub struct MenuState(Arc<Mutex<MenuModel>>);

impl MenuState {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(MenuModel::default())))
    }

    pub fn publish(&self, model: MenuModel) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = model;
        }
    }

    fn read(&self) -> MenuModel {
        self.0.lock().map(|m| m.clone()).unwrap_or_default()
    }
}

impl Default for MenuState {
    fn default() -> Self {
        Self::new()
    }
}

/// Owned by the target object; not `Copy`, so it lives in an ivar.
struct TargetState {
    commands: UnboundedSender<MenuCommand>,
    model: MenuState,
}

define_class!(
    // AppKit sends actions to this object. It exists only to translate a click
    // into a channel send.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AnimeshMenuTarget"]
    #[ivars = TargetState]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    unsafe impl NSMenuDelegate for MenuTarget {
        /// Rebuilt on every open so the menu cannot show a stale schedule, and
        /// so opening it is itself a reconciliation trigger.
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            let state = self.ivars();
            let _ = state.commands.send(MenuCommand::Opened);
            let mtm = MainThreadMarker::from(self);
            populate(menu, self, &state.model.read(), mtm);
        }
    }

    impl MenuTarget {
        #[unsafe(method(refreshNow:))]
        fn refresh_now(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().commands.send(MenuCommand::RefreshNow);
        }

        #[unsafe(method(enableNotifications:))]
        fn enable_notifications(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().commands.send(MenuCommand::EnableNotifications);
        }

        #[unsafe(method(quit:))]
        fn quit(&self, _sender: Option<&AnyObject>) {
            let _ = self.ivars().commands.send(MenuCommand::Quit);
        }
    }
);

impl MenuTarget {
    fn new(
        mtm: MainThreadMarker,
        commands: UnboundedSender<MenuCommand>,
        model: MenuState,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetState { commands, model });
        unsafe { msg_send![super(this), init] }
    }
}

/// The status item and its menu, kept alive for the process's lifetime.
///
/// Dropping this removes the icon from the menu bar, so the app holds it.
pub struct MenuBar {
    _status_item: Retained<NSStatusItem>,
    _target: Retained<MenuTarget>,
}

impl std::fmt::Debug for MenuBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuBar").finish_non_exhaustive()
    }
}

impl MenuBar {
    pub fn install(
        mtm: MainThreadMarker,
        commands: UnboundedSender<MenuCommand>,
        model: MenuState,
    ) -> Self {
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);

        // A text title rather than an icon asset: it renders identically on
        // every appearance and needs no .icns to exist.
        if let Some(button) = item.button(mtm) {
            button.setTitle(&NSString::from_str("◈"));
        }

        let target = MenuTarget::new(mtm, commands, model.clone());
        let menu = NSMenu::new(mtm);
        menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        // Populated once so the menu is correct even if it is opened before the
        // first delegate callback lands.
        populate(&menu, &target, &model.read(), mtm);
        item.setMenu(Some(&menu));

        Self {
            _status_item: item,
            _target: target,
        }
    }
}

fn populate(menu: &NSMenu, target: &MenuTarget, model: &MenuModel, mtm: MainThreadMarker) {
    menu.removeAllItems();

    add_label(menu, &model.summary, mtm);

    if let Some(attention) = &model.attention {
        add_label(menu, attention, mtm);
    }

    if !model.rows.is_empty() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for row in &model.rows {
            add_label(menu, &format!("{}  —  {}", row.title, row.detail), mtm);
        }
    }

    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_action(menu, target, "Refresh Now", sel!(refreshNow:), "r", mtm);
    if !model.notifications_enabled {
        add_action(
            menu,
            target,
            "Enable Notifications…",
            sel!(enableNotifications:),
            "",
            mtm,
        );
    }
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_action(menu, target, "Quit Animesh", sel!(quit:), "q", mtm);
}

/// A non-clickable line. Disabled rather than absent so the layout is stable.
fn add_label(menu: &NSMenu, text: &str, mtm: MainThreadMarker) {
    let item = NSMenuItem::new(mtm);
    item.setTitle(&NSString::from_str(text));
    item.setEnabled(false);
    menu.addItem(&item);
}

fn add_action(
    menu: &NSMenu,
    target: &MenuTarget,
    text: &str,
    action: Sel,
    key: &str,
    mtm: MainThreadMarker,
) {
    let item = NSMenuItem::new(mtm);
    unsafe {
        item.setTitle(&NSString::from_str(text));
        item.setAction(Some(action));
        item.setTarget(Some(target));
        item.setKeyEquivalent(&NSString::from_str(key));
    }
    menu.addItem(&item);
}

/// Configures the process as a background agent with no Dock tile.
///
/// `LSUIElement` in Info.plist covers a bundled launch; this covers the case of
/// running the binary directly, which is what every D1 experiment does.
pub fn become_accessory(mtm: MainThreadMarker) -> Retained<NSApplication> {
    use objc2_app_kit::NSApplicationActivationPolicy;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app
}
