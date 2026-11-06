//! The single menu bar icon. Its glyph shows the app state at a glance:
//! sounds on, sounds off, or sounds unavailable for lack of permission.

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSLineCapStyle, NSMenu, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// What the icon shows. The mark is a keycap seen from above with the homing
/// bar of the F and J keys (`assets/brand/logo/mark.svg`); the bar shows only
/// while sounds play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// The keycap with its homing bar: sounds on.
    On,
    /// A dimmed, struck-through keycap without the bar: sounds off.
    Off,
    /// The keycap with an exclamation mark in place of the bar: no Input
    /// Monitoring permission.
    NeedsPermission,
}

impl Icon {
    /// The missing permission wins, even when sounds are switched off. On
    /// purpose: the switch cannot make sounds without the permission, and the
    /// icon is where the user first sees that something needs fixing (spec:
    /// say so instead of silently making no sound). The cost: with sounds off
    /// and no permission, the icon does not show "off". The menu still shows
    /// the switch's real state below the permission rows.
    pub fn for_state(sounds_on: bool, needs_permission: bool) -> Icon {
        match (needs_permission, sounds_on) {
            (true, _) => Icon::NeedsPermission,
            (false, true) => Icon::On,
            (false, false) => Icon::Off,
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Icon::On => "Clicky",
            Icon::Off => "Clicky (sounds off)",
            Icon::NeedsPermission => "Clicky (sounds unavailable: needs Input Monitoring)",
        }
    }
}

pub struct StatusItem {
    item: Retained<NSStatusItem>,
    mtm: MainThreadMarker,
}

impl StatusItem {
    pub fn new(mtm: MainThreadMarker, menu: &NSMenu) -> StatusItem {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        item.setMenu(Some(menu));
        StatusItem { item, mtm }
    }

    pub fn show(&self, icon: Icon) {
        let Some(button) = self.item.button(self.mtm) else {
            return;
        };
        button.setImage(Some(&image(icon)));
        button.setAppearsDisabled(icon == Icon::Off);
        button.setToolTip(Some(&NSString::from_str(icon.tooltip())));
    }
}

/// An 18 pt template image (the system tints it for the menu bar). Same
/// geometry as `assets/brand/logo/mark.svg`, with the y axis flipped (AppKit
/// puts the origin at the bottom left).
fn image(icon: Icon) -> Retained<NSImage> {
    let block = RcBlock::new(move |_bounds: NSRect| -> Bool {
        let ink = NSColor::blackColor();
        ink.setStroke();
        ink.setFill();
        let body = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(1.75, 1.75, 14.5, 14.5),
            3.75,
            3.75,
        );
        body.setLineWidth(1.5);
        body.stroke();
        match icon {
            Icon::On => {
                let homing_bar = NSBezierPath::bezierPath();
                homing_bar.moveToPoint(NSPoint::new(6.75, 6.25));
                homing_bar.lineToPoint(NSPoint::new(11.25, 6.25));
                homing_bar.setLineWidth(1.6);
                homing_bar.setLineCapStyle(NSLineCapStyle::Round);
                homing_bar.stroke();
            }
            Icon::Off => {
                let slash = NSBezierPath::bezierPath();
                slash.moveToPoint(NSPoint::new(1.5, 1.5));
                slash.lineToPoint(NSPoint::new(16.5, 16.5));
                slash.setLineWidth(1.6);
                slash.stroke();
            }
            Icon::NeedsPermission => {
                let bar = NSBezierPath::bezierPath();
                bar.moveToPoint(NSPoint::new(9.0, 12.25));
                bar.lineToPoint(NSPoint::new(9.0, 8.25));
                bar.setLineWidth(1.8);
                bar.setLineCapStyle(NSLineCapStyle::Round);
                bar.stroke();
                NSBezierPath::bezierPathWithOvalInRect(rect(8.0, 5.0, 2.0, 2.0)).fill();
            }
        }
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(18.0, 18.0), false, &block);
    image.setTemplate(true);
    image.setAccessibilityDescription(Some(&NSString::from_str(icon.tooltip())));
    image
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_follows_the_switch() {
        assert_eq!(Icon::for_state(true, false), Icon::On);
        assert_eq!(Icon::for_state(false, false), Icon::Off);
        assert_ne!(Icon::On.tooltip(), Icon::Off.tooltip());
    }

    /// Draws the real template image at 2x and reports whether the pixel
    /// under the point (x, y) of the mark's 18 pt grid (y down, as in
    /// `mark.svg`) is inked.
    fn inked(icon: Icon, points: &[(f64, f64)]) -> Vec<bool> {
        use objc2::AnyThread;
        use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext};

        // SAFETY: a null plane pointer asks AppKit to allocate the buffer.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                36,
                36,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                0,
                0,
            )
        }
        .expect("bitmap");
        rep.setSize(NSSize::new(18.0, 18.0));
        let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep).expect("context");
        NSGraphicsContext::setCurrentContext(Some(&context));
        image(icon).drawInRect(rect(0.0, 0.0, 18.0, 18.0));
        NSGraphicsContext::setCurrentContext(None);
        points
            .iter()
            .map(|&(x, y)| {
                let pixel = rep.colorAtX_y((x * 2.0) as isize, (y * 2.0) as isize).expect("pixel");
                pixel.alphaComponent() > 0.5
            })
            .collect()
    }

    /// On the keycap's left edge, the homing bar's right end, and the empty
    /// upper left of the keycap (away from the slash and the exclamation mark).
    const EDGE: (f64, f64) = (1.75, 9.0);
    const BAR: (f64, f64) = (11.0, 11.75);
    const MIDDLE: (f64, f64) = (5.0, 5.0);

    #[test]
    fn the_homing_bar_shows_only_while_sounds_play() {
        assert_eq!(inked(Icon::On, &[EDGE, BAR, MIDDLE]), [true, true, false]);
        assert_eq!(inked(Icon::Off, &[EDGE, BAR, MIDDLE]), [true, false, false]);
        assert_eq!(inked(Icon::NeedsPermission, &[EDGE, BAR, MIDDLE]), [true, false, false]);
    }

    #[test]
    fn a_missing_permission_shows_whatever_the_switch_says() {
        assert_eq!(Icon::for_state(true, true), Icon::NeedsPermission);
        assert_eq!(Icon::for_state(false, true), Icon::NeedsPermission);
        assert!(Icon::NeedsPermission.tooltip().contains("Input Monitoring"));
    }
}
