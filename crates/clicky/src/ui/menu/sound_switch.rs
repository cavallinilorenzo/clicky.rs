//! The header of the menu: the keycap mark, "Clicky" over a one-word state,
//! and the on/off switch on the trailing edge. Off silences everything at
//! once; the mark loses its homing bar, as the menu bar icon does.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSColor, NSFont, NSImageView, NSMenuItem, NSTextField, NSView};
use objc2_foundation::NSString;

use super::MenuBuilder;
use super::brand::{self, STICK_LEFT, STICK_RIGHT, rect};
use super::switch::{self, Switch};
use crate::engine::SOUNDS_ON;
use crate::ui::Ui;

const HEIGHT: f64 = 52.0;
const MARK: f64 = 26.0;
const GAP: f64 = 9.0;

/// The parts of the header that follow the state.
struct Header {
    view: Retained<NSView>,
    mark: Retained<NSImageView>,
    state: Retained<NSTextField>,
    switch: Retained<Switch>,
}

impl Header {
    fn show(&self, on: bool) {
        paint(&self.mark, &self.state, on);
        self.switch.show(on);
    }
}

/// The mark and the word that follow the state.
fn paint(mark: &NSImageView, state: &NSTextField, on: bool) {
    mark.setImage(Some(&brand::mark(on, MARK)));
    state.setStringValue(&NSString::from_str(if on { "Sounds on" } else { "Sounds off" }));
}

pub fn add(menu: &mut MenuBuilder) {
    let mtm = menu.mtm();
    let ui = menu.ui().clone();
    let header = std::rc::Rc::new(header(mtm, ui.clone()));
    header.show(ui.shared.enabled());

    let shown = header.clone();
    menu.on_open(move || shown.show(ui.shared.enabled()));

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&header.view));
    menu.add(&item);
}

fn set_sounds_on(ui: &Ui, on: bool) {
    ui.shared.set_enabled(on);
    ui.store.set(&SOUNDS_ON, &on);
    ui.refresh_status();
}

fn header(mtm: MainThreadMarker, ui: std::rc::Rc<Ui>) -> Header {
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, brand::WIDTH, HEIGHT));
    let lead = brand::lead();

    let mark = NSImageView::new(mtm);
    mark.setFrame(rect(lead, ((HEIGHT - MARK) / 2.0).round(), MARK, MARK));
    mark.setAutoresizingMask(STICK_LEFT);
    view.addSubview(&mark);

    let text_x = lead + MARK + GAP;
    let title = NSTextField::labelWithString(&NSString::from_str("Clicky"), mtm);
    title.setFont(Some(&NSFont::boldSystemFontOfSize(14.0)));
    title.sizeToFit();
    let title_size = title.frame().size;

    let state = NSTextField::labelWithString(&NSString::from_str("Sounds on"), mtm);
    state.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    state.setTextColor(Some(&NSColor::secondaryLabelColor()));
    state.sizeToFit();
    let state_size = state.frame().size;
    // Sized for the longer word, so the label never clips.
    let state_width = state_size.width.max(60.0);

    let block = title_size.height + state_size.height;
    let top = ((HEIGHT + block) / 2.0).round();
    title.setFrame(rect(text_x, top - title_size.height, title_size.width, title_size.height));
    title.setAutoresizingMask(STICK_LEFT);
    state.setFrame(rect(text_x, top - block, state_width, state_size.height));
    state.setAutoresizingMask(STICK_LEFT);
    view.addSubview(&title);
    view.addSubview(&state);

    let (changed_ui, changed_mark, changed_state) = (ui.clone(), mark.clone(), state.clone());
    let switch = Switch::new(mtm, ui.shared.enabled(), move |on| {
        set_sounds_on(&changed_ui, on);
        paint(&changed_mark, &changed_state, on);
    });
    switch.setFrame(rect(
        brand::WIDTH - brand::trail() - switch::WIDTH,
        ((HEIGHT - switch::HEIGHT) / 2.0).round(),
        switch::WIDTH,
        switch::HEIGHT,
    ));
    switch.setAutoresizingMask(STICK_RIGHT);
    view.addSubview(&switch);
    Header { view, mark, state, switch }
}
