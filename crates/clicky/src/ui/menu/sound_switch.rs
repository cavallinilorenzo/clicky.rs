//! The on/off switch at the top of the menu: "Clicky" in bold with a switch
//! on the trailing edge. Off silences everything at once and dims the icon.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSControlSize, NSControlStateValueOn, NSFont, NSMenuItem, NSSwitch, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::{MenuBuilder, control_state};
use crate::engine::SOUNDS_ON;
use crate::ui::Ui;

/// Width of the header row, in points.
const WIDTH: f64 = 240.0;
const HEIGHT: f64 = 36.0;
const INSET: f64 = 14.0;

pub fn add(menu: &mut MenuBuilder) {
    let mtm = menu.mtm();
    let (view, switch) = header(mtm, menu.ui().shared.enabled());

    let ui = menu.ui().clone();
    let sender = switch.clone();
    menu.action(move |_| set_sounds_on(&ui, sender.state() == NSControlStateValueOn))
        .attach_control(&switch);

    let ui = menu.ui().clone();
    let shown = switch.clone();
    menu.on_open(move || shown.setState(control_state(ui.shared.enabled())));

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&view));
    menu.add(&item);
}

fn set_sounds_on(ui: &Ui, on: bool) {
    ui.shared.set_enabled(on);
    ui.store.set(&SOUNDS_ON, &on);
    ui.refresh_status();
}

fn header(mtm: MainThreadMarker, on: bool) -> (Retained<NSView>, Retained<NSSwitch>) {
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, WIDTH, HEIGHT));
    let title = NSTextField::labelWithString(&NSString::from_str("Clicky"), mtm);
    title.setFont(Some(&NSFont::boldSystemFontOfSize(13.0)));
    title.sizeToFit();
    let size = title.frame().size;
    title.setFrame(rect(INSET, ((HEIGHT - size.height) / 2.0).round(), size.width, size.height));
    view.addSubview(&title);

    let switch = NSSwitch::new(mtm);
    switch.setControlSize(NSControlSize::Small);
    switch.sizeToFit();
    let size = switch.frame().size;
    switch.setFrame(rect(
        WIDTH - INSET - size.width,
        ((HEIGHT - size.height) / 2.0).round(),
        size.width,
        size.height,
    ));
    switch.setState(control_state(on));
    switch.setToolTip(Some(&NSString::from_str("Keyboard sounds on or off")));
    view.addSubview(&switch);
    (view, switch)
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
