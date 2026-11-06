//! The first-run window: explains the single permission and why it is safe,
//! offers the way to System Settings, then confirms the grant or asks for one
//! relaunch. [`Model`] decides when it shows; the rest is AppKit.
//!
//! The window layout is derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal
//! Bagaria, https://github.com/KunalBagaria/rustyvibes-v2),
//! `crates/rustyvibes/src/ui/onboarding.rs`.

use std::cell::RefCell;
use std::process::{Command, Stdio};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, available};
use objc2_app_kit::{
    NSApplication, NSBackingStoreType, NSButton, NSColor, NSFont, NSImage, NSImageScaling,
    NSImageView, NSLayoutAttribute, NSStackView, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
    NSWorkspace,
};
use objc2_foundation::{NSArray, NSBundle, NSEdgeInsets, NSPoint, NSRect, NSSize, NSString, NSURL};

use super::Ui;
use super::target::ActionTarget;
use crate::diagnostics::{self, Event};
use crate::input::permission::{self, Status};

/// What the window shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Why Clicky needs Input Monitoring and how to grant it; waiting.
    Explain,
    /// The permission is granted and sounds are on their way.
    Granted,
    /// Granted, but macOS needs Clicky to restart.
    Relaunch,
}

/// Whether the window is shown, and with which step. Pure.
#[derive(Debug, Default)]
pub struct Model {
    shown: Option<Step>,
    /// Closed by the user this launch: not shown again on its own while waiting.
    dismissed: bool,
}

impl Model {
    /// The step on screen, or `None` when the window is hidden.
    pub fn shown(&self) -> Option<Step> {
        self.shown
    }

    /// The listener's status changed (or was read at launch).
    pub fn status_changed(&mut self, status: Status) {
        self.shown = match (status, self.shown) {
            (Status::Checking, shown) => shown,
            // Launch (or a later loss) without permission: explain, unless dismissed.
            (Status::Missing, None) if !self.dismissed => Some(Step::Explain),
            (Status::Missing | Status::Revoked, Some(_)) => Some(Step::Explain),
            (Status::Missing | Status::Revoked, None) => None,
            (Status::Listening, shown) => shown.map(|_| Step::Granted),
            // The one thing left to do: always ask.
            (Status::NeedsRelaunch, _) => Some(Step::Relaunch),
        };
    }

    /// The user asked for the window (from the menu).
    pub fn open(&mut self, status: Status) {
        self.dismissed = false;
        self.shown = Some(step_for(status));
    }

    /// The user closed the window (Done, Not Now or the close button).
    pub fn close(&mut self) {
        self.shown = None;
        self.dismissed = true;
    }
}

fn step_for(status: Status) -> Step {
    match status {
        Status::Listening => Step::Granted,
        Status::NeedsRelaunch => Step::Relaunch,
        Status::Checking | Status::Missing | Status::Revoked => Step::Explain,
    }
}

/// The status line and buttons for a step: (status, primary, secondary).
fn texts(step: Step) -> (&'static str, &'static str, Option<&'static str>) {
    match step {
        Step::Explain => ("Waiting for permission…", "Open System Settings", Some("Not Now")),
        Step::Granted => ("Permission granted. Your keys now make a sound.", "Done", None),
        Step::Relaunch => (
            "Permission granted. macOS needs Clicky to restart before sounds can start.",
            "Relaunch",
            Some("Later"),
        ),
    }
}

const TITLE: &str = "Let Clicky hear your key presses";
const BODY: &str = "Clicky plays a sound each time you press a key. To notice key presses \
                    in other apps, macOS needs one permission from you: Input Monitoring.";
const STEPS: &str = "Click Open System Settings, then turn on Clicky under Privacy & Security \
                     → Input Monitoring. If Clicky is already on, turn it off and on again.";
const PRIVACY: &str = "Clicky learns only which key moved, never what you type. Nothing you \
                       type is read, recorded or sent anywhere.";
const WIDTH: f64 = 440.0;

/// The window and its model. Main thread only; lives in [`Ui`].
#[derive(Default)]
pub struct Onboarding {
    model: RefCell<Model>,
    window: RefCell<Option<Window>>,
}

/// The listener's status changed (or was read at launch): update the window.
pub fn status_changed(ui: &Rc<Ui>) {
    sync_closed(ui);
    ui.onboarding.model.borrow_mut().status_changed(permission::status());
    apply(ui);
}

/// Shows the window for the current status (the menu's "Set Up" item).
pub fn open(ui: &Rc<Ui>) {
    ui.onboarding.model.borrow_mut().open(permission::status());
    apply(ui);
}

fn close(ui: &Rc<Ui>) {
    ui.onboarding.model.borrow_mut().close();
    apply(ui);
}

/// The user may have closed the window with its close button.
fn sync_closed(ui: &Ui) {
    let visible = ui.onboarding.window.borrow().as_ref().is_some_and(|w| w.window.isVisible());
    let mut model = ui.onboarding.model.borrow_mut();
    if model.shown().is_some() && !visible {
        model.close();
    }
}

fn primary_clicked(ui: &Rc<Ui>) {
    let shown = ui.onboarding.model.borrow().shown();
    match shown {
        Some(Step::Explain) => open_input_monitoring_settings(),
        Some(Step::Granted) => close(ui),
        Some(Step::Relaunch) => relaunch(ui.mtm),
        None => {}
    }
}

/// Makes the window match the model.
fn apply(ui: &Rc<Ui>) {
    let shown = ui.onboarding.model.borrow().shown();
    let mut slot = ui.onboarding.window.borrow_mut();
    match shown {
        None => {
            if let Some(window) = slot.as_ref() {
                window.window.orderOut(None);
            }
        }
        Some(step) => {
            let window = slot.get_or_insert_with(|| Window::new(ui));
            window.show_step(step);
            activate(ui.mtm);
            window.window.makeKeyAndOrderFront(None);
            window.window.orderFrontRegardless();
        }
    }
}

/// Lists Clicky under Input Monitoring (macOS prompts the first time only),
/// then opens System Settings at that page. Only ever called from a click.
pub fn open_input_monitoring_settings() {
    permission::request();
    match NSURL::URLWithString(&NSString::from_str(permission::SETTINGS_URL)) {
        Some(url) => {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
        None => diagnostics::log(Event::Error {
            context: "input monitoring settings",
            detail: "bad settings URL".into(),
        }),
    }
}

/// Starts a fresh copy of the bundle once this process has exited, then quits.
/// Outside a `.app` (e.g. `cargo run`) it only logs: quit and start it by hand.
pub fn relaunch(mtm: MainThreadMarker) {
    let bundle = NSBundle::mainBundle().bundlePath().to_string();
    if !bundle.ends_with(".app") {
        diagnostics::log(Event::Error {
            context: "relaunch",
            detail: "not running from an app bundle; quit and start Clicky again".into(),
        });
        return;
    }
    diagnostics::log(Event::Relaunching);
    // Waits (up to 10 s) for this process to exit, so the new copy is never a
    // second instance. The path and pid are arguments, never part of the script.
    let script = r#"i=0
while kill -0 "$1" 2>/dev/null && [ "$i" -lt 100 ]; do sleep 0.1; i=$((i+1)); done
sleep 0.3
exec /usr/bin/open "$0""#;
    let spawned = Command::new("/bin/sh")
        .args(["-c", script, &bundle, &std::process::id().to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => NSApplication::sharedApplication(mtm).terminate(None),
        Err(e) => diagnostics::log(Event::Error { context: "relaunch", detail: e.to_string() }),
    }
}

/// Brings this accessory (`LSUIElement`) app forward so its window gets focus.
fn activate(mtm: MainThreadMarker) {
    let app = NSApplication::sharedApplication(mtm);
    if available!(macos = 14.0) {
        app.activate();
    } else {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }
}

struct Window {
    window: Retained<NSWindow>,
    status: Retained<NSTextField>,
    primary: Retained<NSButton>,
    secondary: Retained<NSButton>,
    _targets: [Retained<ActionTarget>; 2],
}

impl Window {
    fn new(ui: &Rc<Ui>) -> Window {
        let mtm = ui.mtm;
        // SAFETY: standard window creation on the main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect(0.0, 0.0, WIDTH, 320.0),
                NSWindowStyleMask::Titled
                    | NSWindowStyleMask::Closable
                    | NSWindowStyleMask::FullSizeContentView,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: we keep our own strong reference, so AppKit must not release on close.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str("Clicky"));
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
        window.setMovableByWindowBackground(true);

        let icon = NSImageView::new(mtm);
        if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("keyboard"),
            Some(&NSString::from_str("Keyboard")),
        ) {
            icon.setImage(Some(&image));
        }
        icon.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
        icon.widthAnchor().constraintEqualToConstant(72.0).setActive(true);
        icon.heightAnchor().constraintEqualToConstant(56.0).setActive(true);
        let title = label(mtm, TITLE, &NSFont::boldSystemFontOfSize(20.0));
        let body = paragraph(mtm, BODY, 13.0, &NSColor::secondaryLabelColor());
        let steps = paragraph(mtm, STEPS, 13.0, &NSColor::labelColor());
        let privacy = paragraph(mtm, PRIVACY, 12.0, &NSColor::secondaryLabelColor());
        let status = paragraph(mtm, "", 12.0, &NSColor::secondaryLabelColor());

        let on_primary = {
            let ui = ui.clone();
            ActionTarget::new(mtm, move |_| primary_clicked(&ui))
        };
        let on_secondary = {
            let ui = ui.clone();
            ActionTarget::new(mtm, move |_| close(&ui))
        };
        let primary = push_button(mtm, &on_primary);
        primary.setKeyEquivalent(&NSString::from_str("\r"));
        let secondary = push_button(mtm, &on_secondary);
        secondary.setKeyEquivalent(&NSString::from_str("\u{1b}"));

        let buttons =
            stack(mtm, &[&secondary, &primary], NSUserInterfaceLayoutOrientation::Horizontal, 12.0);
        let content = stack(
            mtm,
            &[&icon, &title, &body, &steps, &privacy, &status, &buttons],
            NSUserInterfaceLayoutOrientation::Vertical,
            12.0,
        );
        content.setAlignment(NSLayoutAttribute::CenterX);
        content.setEdgeInsets(NSEdgeInsets { top: 36.0, left: 36.0, bottom: 28.0, right: 36.0 });
        content.setCustomSpacing_afterView(16.0, &icon);
        content.setCustomSpacing_afterView(20.0, &status);
        content.widthAnchor().constraintEqualToConstant(WIDTH).setActive(true);
        window.setContentView(Some(&content));
        window.setContentSize(content.fittingSize());
        window.center();

        Window { window, status, primary, secondary, _targets: [on_primary, on_secondary] }
    }

    fn show_step(&self, step: Step) {
        let (status, primary, secondary) = texts(step);
        let color = match step {
            Step::Explain => NSColor::secondaryLabelColor(),
            Step::Granted => NSColor::systemGreenColor(),
            Step::Relaunch => NSColor::systemOrangeColor(),
        };
        self.status.setStringValue(&NSString::from_str(status));
        self.status.setTextColor(Some(&color));
        self.primary.setTitle(&NSString::from_str(primary));
        self.secondary.setHidden(secondary.is_none());
        if let Some(secondary) = secondary {
            self.secondary.setTitle(&NSString::from_str(secondary));
        }
    }
}

/// A standard push button whose click runs `target`. Its title is set per step.
fn push_button(mtm: MainThreadMarker, target: &ActionTarget) -> Retained<NSButton> {
    // SAFETY: no target or action yet; `attach_control` sets both.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(&NSString::from_str(""), None, None, mtm)
    };
    target.attach_control(&button);
    button
}

fn label(mtm: MainThreadMarker, text: &str, font: &NSFont) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(font));
    label.setAlignment(NSTextAlignment::Center);
    label
}

fn paragraph(
    mtm: MainThreadMarker,
    text: &str,
    size: f64,
    color: &NSColor,
) -> Retained<NSTextField> {
    let paragraph = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    paragraph.setFont(Some(&NSFont::systemFontOfSize(size)));
    paragraph.setTextColor(Some(color));
    paragraph.setAlignment(NSTextAlignment::Center);
    paragraph.setSelectable(false);
    paragraph.setPreferredMaxLayoutWidth(WIDTH - 72.0);
    paragraph
}

fn stack(
    mtm: MainThreadMarker,
    views: &[&NSView],
    orientation: NSUserInterfaceLayoutOrientation,
    spacing: f64,
) -> Retained<NSStackView> {
    let stack = NSStackView::stackViewWithViews(&NSArray::from_slice(views), mtm);
    stack.setOrientation(orientation);
    stack.setSpacing(spacing);
    stack
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_promises_only_key_movement_and_names_the_page() {
        assert!(PRIVACY.contains("only which key moved, never what you type"));
        assert!(STEPS.contains("Privacy & Security") && STEPS.contains("Input Monitoring"));
    }

    #[test]
    fn a_relaunch_is_one_button() {
        let (_, primary, _) = texts(Step::Relaunch);
        assert_eq!(primary, "Relaunch");
        assert_eq!(texts(Step::Granted).2, None, "the confirmation has only Done");
    }

    #[test]
    fn a_launch_without_permission_explains_it() {
        let mut m = Model::default();
        m.status_changed(Status::Checking);
        assert_eq!(m.shown(), None, "nothing flashes while checking");
        m.status_changed(Status::Missing);
        assert_eq!(m.shown(), Some(Step::Explain));
    }

    #[test]
    fn a_launch_with_permission_shows_nothing() {
        let mut m = Model::default();
        m.status_changed(Status::Listening);
        assert_eq!(m.shown(), None);
    }

    #[test]
    fn granting_while_shown_confirms_it() {
        let mut m = Model::default();
        m.status_changed(Status::Missing);
        m.status_changed(Status::Listening);
        assert_eq!(m.shown(), Some(Step::Granted));
        m.close();
        assert_eq!(m.shown(), None);
    }

    #[test]
    fn a_grant_that_needs_a_relaunch_asks_for_it_even_after_not_now() {
        let mut m = Model::default();
        m.status_changed(Status::Missing);
        m.close();
        m.status_changed(Status::Missing);
        assert_eq!(m.shown(), None, "Not Now is respected while waiting");
        m.status_changed(Status::NeedsRelaunch);
        assert_eq!(m.shown(), Some(Step::Relaunch));
    }

    #[test]
    fn granting_after_not_now_does_not_pop_up() {
        let mut m = Model::default();
        m.status_changed(Status::Missing);
        m.close();
        m.status_changed(Status::Listening);
        assert_eq!(m.shown(), None);
    }

    #[test]
    fn revoking_while_running_leaves_it_to_the_menu() {
        let mut m = Model::default();
        m.status_changed(Status::Listening);
        m.status_changed(Status::Revoked);
        assert_eq!(m.shown(), None);
    }

    #[test]
    fn revoking_while_the_confirmation_shows_goes_back_to_explaining() {
        let mut m = Model::default();
        m.status_changed(Status::Missing);
        m.status_changed(Status::Listening);
        m.status_changed(Status::Revoked);
        assert_eq!(m.shown(), Some(Step::Explain));
    }

    #[test]
    fn opening_from_the_menu_shows_the_step_for_the_status() {
        let mut m = Model::default();
        m.status_changed(Status::Revoked);
        m.open(Status::Revoked);
        assert_eq!(m.shown(), Some(Step::Explain));
        m.open(Status::NeedsRelaunch);
        assert_eq!(m.shown(), Some(Step::Relaunch));
        m.open(Status::Listening);
        assert_eq!(m.shown(), Some(Step::Granted));
    }

    #[test]
    fn opening_again_after_not_now_follows_the_grant() {
        let mut m = Model::default();
        m.status_changed(Status::Missing);
        m.close();
        m.open(Status::Missing);
        m.status_changed(Status::Listening);
        assert_eq!(m.shown(), Some(Step::Granted));
    }
}
