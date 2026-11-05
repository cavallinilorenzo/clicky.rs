//! The single menu bar icon. Its glyph shows the app state at a glance.
//! Issue #9 adds a "needs permission" state.

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSBezierPath, NSColor, NSImage, NSMenu, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// What the icon shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    /// A keycap: sounds on.
    On,
    /// A dimmed, struck-through keycap: sounds off.
    Off,
}

impl Icon {
    pub fn for_state(sounds_on: bool) -> Icon {
        if sounds_on { Icon::On } else { Icon::Off }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Icon::On => "Clicky",
            Icon::Off => "Clicky (sounds off)",
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

/// An 18 pt template image (the system tints it for the menu bar).
fn image(icon: Icon) -> Retained<NSImage> {
    let block = RcBlock::new(move |_bounds: NSRect| -> Bool {
        let ink = NSColor::blackColor();
        ink.setStroke();
        ink.setFill();
        let body = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(2.25, 2.75, 13.5, 12.0),
            3.2,
            3.2,
        );
        body.setLineWidth(1.5);
        body.stroke();
        match icon {
            Icon::On => {
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    rect(5.25, 6.25, 7.5, 6.0),
                    1.6,
                    1.6,
                )
                .fill();
            }
            Icon::Off => {
                let slash = NSBezierPath::bezierPath();
                slash.moveToPoint(NSPoint::new(1.5, 1.5));
                slash.lineToPoint(NSPoint::new(16.5, 16.5));
                slash.setLineWidth(1.6);
                slash.stroke();
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
        assert_eq!(Icon::for_state(true), Icon::On);
        assert_eq!(Icon::for_state(false), Icon::Off);
        assert_ne!(Icon::On.tooltip(), Icon::Off.tooltip());
    }
}
