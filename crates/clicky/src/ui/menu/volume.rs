//! The volume slider: a speaker glyph on each side of a continuous slider,
//! in the menu as a custom view. Dragging sets the master gain on every
//! move, so the level changes live under typing; the mixer ramps it, so it
//! never steps. While dragging, the soundpack preview plays (from the first
//! move, then again each time it has finished), so the level can be heard
//! without typing. The position is saved (`engine::mixer::VOLUME`).

use std::cell::Cell;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSColor, NSImage, NSImageView, NSMenuItem, NSSlider, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use super::MenuBuilder;
use crate::engine::mixer::{VOLUME, volume_gain};
use crate::engine::preview;
use crate::ui::Ui;

/// Same width as the header row.
const WIDTH: f64 = 240.0;
const HEIGHT: f64 = 30.0;
const INSET: f64 = 14.0;
const GLYPH: f64 = 16.0;
const GAP: f64 = 6.0;
/// The preview (560 ms of presses) plus its last sound's ring.
const PREVIEW_EVERY: Duration = Duration::from_millis(650);

/// When the slider's preview plays: on the first move, then not again
/// until it has played through, so a drag never restarts it.
#[derive(Default)]
struct Feedback {
    last: Option<Instant>,
}

impl Feedback {
    fn due(&mut self, now: Instant) -> bool {
        let due = self.last.is_none_or(|t| now.saturating_duration_since(t) >= PREVIEW_EVERY);
        if due {
            self.last = Some(now);
        }
        due
    }
}

pub fn add(menu: &mut MenuBuilder) {
    let mtm = menu.mtm();
    menu.separator();
    let (view, slider) = row(mtm, menu.ui().store.get(&VOLUME));

    let ui = menu.ui().clone();
    let sender = slider.clone();
    let feedback = Cell::new(Feedback::default());
    menu.action(move |_| {
        set_volume(&ui, sender.doubleValue());
        let mut f = feedback.take();
        if f.due(Instant::now()) {
            preview::play();
        }
        feedback.set(f);
    })
    .attach_control(&slider);

    let ui = menu.ui().clone();
    let shown = slider.clone();
    menu.on_open(move || shown.setDoubleValue(ui.store.get(&VOLUME)));

    let item = NSMenuItem::new(mtm);
    item.setView(Some(&view));
    menu.add(&item);
}

fn set_volume(ui: &Ui, position: f64) {
    let position = position.clamp(0.0, 1.0);
    ui.shared.set_master_gain(volume_gain(position));
    ui.store.set(&VOLUME, &position);
}

fn row(mtm: MainThreadMarker, position: f64) -> (Retained<NSView>, Retained<NSSlider>) {
    let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0.0, 0.0, WIDTH, HEIGHT));
    let y = ((HEIGHT - GLYPH) / 2.0).round();
    if let Some(quiet) = glyph(mtm, "speaker.fill", "Quieter") {
        quiet.setFrame(rect(INSET, y, GLYPH, GLYPH));
        view.addSubview(&quiet);
    }
    if let Some(loud) = glyph(mtm, "speaker.wave.3.fill", "Louder") {
        loud.setFrame(rect(WIDTH - INSET - GLYPH, y, GLYPH, GLYPH));
        view.addSubview(&loud);
    }

    let slider = NSSlider::new(mtm);
    slider.setMinValue(0.0);
    slider.setMaxValue(1.0);
    slider.setDoubleValue(position.clamp(0.0, 1.0));
    slider.setContinuous(true);
    slider.setToolTip(Some(&NSString::from_str("Volume")));
    slider.sizeToFit();
    let height = slider.frame().size.height;
    let left = INSET + GLYPH + GAP;
    slider.setFrame(rect(left, ((HEIGHT - height) / 2.0).round(), WIDTH - 2.0 * left, height));
    view.addSubview(&slider);
    (view, slider)
}

fn glyph(mtm: MainThreadMarker, symbol: &str, description: &str) -> Option<Retained<NSImageView>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(symbol),
        Some(&NSString::from_str(description)),
    )?;
    let view = NSImageView::imageViewWithImage(&image, mtm);
    view.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    Some(view)
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dragging_plays_the_preview_through_instead_of_restarting_it() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut feedback = Feedback::default();
        assert!(feedback.due(ms(0)), "the first move plays it");
        // Slider events at 60 Hz while the preview (560 ms) plays.
        assert!((1..40).all(|i| !feedback.due(ms(i * 16))));
        assert!(feedback.due(ms(650)), "then it plays again while the drag goes on");
        assert!(!feedback.due(ms(700)));
    }
}
