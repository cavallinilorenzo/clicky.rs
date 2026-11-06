//! What the custom rows of the menu share: the brand blue, the insets that
//! line them up with the system's own rows, and the keycap mark.
//!
//! The blue is the palette's Navy (`#2F5BD3`) on a light menu and Sky
//! (`#7FA2FF`) on a dark one (`assets/brand/README.md`). It is a dynamic
//! colour, so every control that uses it follows the appearance by itself.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2::available;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{
    NSAppearance, NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSImage, NSLineCapStyle,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

/// Width of the custom rows. The menu is as wide as its widest row; the
/// controls follow it through their autoresizing masks.
pub const WIDTH: f64 = 264.0;

/// The distance from the menu's edge to where the system's rows put their
/// text. macOS 26 widened it.
pub fn lead() -> f64 {
    if available!(macos = 26.0) { 30.0 } else { 21.0 }
}

/// The distance from the menu's right edge to where the system's rows end
/// (a submenu's chevron).
pub fn trail() -> f64 {
    if available!(macos = 26.0) { 20.0 } else { 14.0 }
}

pub fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

pub fn srgb(hex: u32) -> Retained<NSColor> {
    let channel = |shift: u32| f64::from((hex >> shift) & 0xFF) / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(channel(16), channel(8), channel(0), 1.0)
}

/// The brand blue for the current appearance.
pub fn accent() -> Retained<NSColor> {
    let provider = RcBlock::new(|appearance: NonNull<NSAppearance>| -> NonNull<NSColor> {
        // SAFETY: AppKit passes a valid appearance.
        let dark = unsafe { appearance.as_ref() }.name().to_string().contains("Dark");
        let colour = srgb(if dark { 0x7FA2FF } else { 0x2F5BD3 });
        // SAFETY: the colour is autoreleased, so it outlives this call as
        // AppKit expects of a dynamic provider.
        unsafe { NonNull::new_unchecked(Retained::autorelease_ptr(colour)) }
    });
    // SAFETY: the provider returns a valid colour.
    unsafe { NSColor::colorWithName_dynamicProvider(None, &provider) }
}

/// Follows the row's right edge, keeping its size and its vertical centre.
pub const STICK_RIGHT: NSAutoresizingMaskOptions = NSAutoresizingMaskOptions(
    NSAutoresizingMaskOptions::ViewMinXMargin.0
        | NSAutoresizingMaskOptions::ViewMinYMargin.0
        | NSAutoresizingMaskOptions::ViewMaxYMargin.0,
);
/// Stretches with the row's width, keeping its vertical centre.
pub const STRETCH: NSAutoresizingMaskOptions = NSAutoresizingMaskOptions(
    NSAutoresizingMaskOptions::ViewWidthSizable.0
        | NSAutoresizingMaskOptions::ViewMinYMargin.0
        | NSAutoresizingMaskOptions::ViewMaxYMargin.0,
);
/// Keeps its place at the left edge and its vertical centre.
pub const STICK_LEFT: NSAutoresizingMaskOptions = NSAutoresizingMaskOptions(
    NSAutoresizingMaskOptions::ViewMinYMargin.0 | NSAutoresizingMaskOptions::ViewMaxYMargin.0,
);

/// The keycap mark (`assets/brand/logo/mark-accent.svg`): an ink keycap, and
/// the homing bar in the brand blue while sounds are on. `size` in points.
pub fn mark(on: bool, size: f64) -> Retained<NSImage> {
    let block = RcBlock::new(move |bounds: NSRect| -> Bool {
        let unit = bounds.size.width / 18.0;
        let at = |x: f64, y: f64| NSPoint::new(x * unit, y * unit);
        let ink = NSColor::labelColor();
        ink.setStroke();
        let body = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
            rect(1.75 * unit, 1.75 * unit, 14.5 * unit, 14.5 * unit),
            3.75 * unit,
            3.75 * unit,
        );
        body.setLineWidth(1.5 * unit);
        body.stroke();
        if on {
            accent().setStroke();
            let bar = NSBezierPath::bezierPath();
            bar.moveToPoint(at(6.75, 6.25));
            bar.lineToPoint(at(11.25, 6.25));
            bar.setLineWidth(1.9 * unit);
            bar.setLineCapStyle(NSLineCapStyle::Round);
            bar.stroke();
        }
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(size, size), false, &block);
    image.setAccessibilityDescription(Some(&NSString::from_str("Clicky")));
    image
}
