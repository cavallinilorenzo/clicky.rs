//! The on/off switch: a pill in the brand blue with a white knob. AppKit's
//! `NSSwitch` takes its colour from the system accent and turns grey in a
//! menu (a menu is never the key window), so Clicky draws its own.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSColor, NSEvent, NSView};
use objc2_foundation::{NSObjectProtocol, NSRect, NSString};

use super::brand::{accent, rect};

pub const WIDTH: f64 = 38.0;
pub const HEIGHT: f64 = 22.0;
const MARGIN: f64 = 2.0;

pub struct SwitchIvars {
    on: Cell<bool>,
    /// Called with the new state after a click.
    changed: Box<dyn Fn(bool)>,
}

define_class!(
    /// A two-state switch. Clicking flips it and calls its closure.
    // SAFETY: NSView has no subclassing requirements and we implement no `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClickySwitch"]
    #[ivars = SwitchIvars]
    pub struct Switch;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for Switch {}

    impl Switch {
        // SAFETY: the signatures match `-drawRect:`, `-mouseDown:` and
        // `-acceptsFirstMouse:`.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let on = self.ivars().on.get();
            let bounds = self.bounds();
            let (width, height) = (bounds.size.width, bounds.size.height);
            let track = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                bounds,
                height / 2.0,
                height / 2.0,
            );
            if on {
                accent().setFill();
            } else {
                NSColor::tertiaryLabelColor().setFill();
            }
            track.fill();

            let diameter = height - 2.0 * MARGIN;
            let x = if on { width - MARGIN - diameter } else { MARGIN };
            let knob = NSBezierPath::bezierPathWithOvalInRect(rect(x, MARGIN, diameter, diameter));
            NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.18).setStroke();
            knob.setLineWidth(0.5);
            NSColor::whiteColor().setFill();
            knob.fill();
            knob.stroke();
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            let on = !self.ivars().on.get();
            self.ivars().on.set(on);
            self.setNeedsDisplay(true);
            (self.ivars().changed)(on);
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }
    }
);

impl Switch {
    pub fn new(
        mtm: MainThreadMarker,
        on: bool,
        changed: impl Fn(bool) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm)
            .set_ivars(SwitchIvars { on: Cell::new(on), changed: Box::new(changed) });
        // SAFETY: NSView's designated initialiser.
        let this: Retained<Self> =
            unsafe { msg_send![super(this), initWithFrame: rect(0.0, 0.0, WIDTH, HEIGHT)] };
        this.setToolTip(Some(&NSString::from_str("Keyboard sounds on or off")));
        this
    }

    /// Shows `on` without calling the closure.
    pub fn show(&self, on: bool) {
        if self.ivars().on.replace(on) != on {
            self.setNeedsDisplay(true);
        }
    }
}
