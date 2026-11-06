//! Media keys (volume, brightness, play/pause…): macOS reports them as
//! system-defined events (`NX_SYSDEFINED`), not key-downs.
//!
//! PRIVACY: from such an event Clicky reads only its type, its subtype and,
//! for subtype 8 (aux control buttons), `data1`, which holds nothing but the
//! media key's `NX_KEYTYPE_*` number, its up/down state and a repeat flag.
//! These events carry no characters.
//!
//! The fields are read through `NSEvent`, the documented way. This is the one
//! place the tap thread calls Objective-C and allocates (one autoreleased
//! `NSEvent`, freed by the pool at once); it runs only for system-defined
//! events, never for ordinary key presses.

use objc2::rc::{Retained, autoreleasepool};
use objc2::{ClassType, Encoding, RefEncode, msg_send};
use objc2_app_kit::{NSEvent, NSEventType};

use super::ffi;
use crate::keys::KeyEvent;

/// `NX_SUBTYPE_AUX_CONTROL_BUTTONS`: media and other special keys.
const AUX_CONTROL_BUTTONS: i16 = 8;
/// Key states in bits 8..16 of `data1` (`NX_KEYDOWN`, `NX_KEYUP`).
const KEY_DOWN: isize = 0x0A;
const KEY_UP: isize = 0x0B;

/// `CGEvent`'s opaque struct, so `msg_send!` passes the pointer with the
/// Objective-C type encoding `eventWithCGEvent:` declares.
#[repr(C)]
struct OpaqueCGEvent {
    _private: [u8; 0],
}

// SAFETY: matches `CGEventRef`'s encoding, `^{__CGEvent=}`.
unsafe impl RefEncode for OpaqueCGEvent {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("__CGEvent", &[]));
}

/// The media-key event in a system-defined `event`, if it is one.
pub fn read(event: ffi::CGEventRef) -> Option<KeyEvent> {
    autoreleasepool(|_| {
        // SAFETY: `event` is a valid CGEvent for the duration of the callback.
        let ns: Option<Retained<NSEvent>> =
            unsafe { msg_send![NSEvent::class(), eventWithCGEvent: event.cast::<OpaqueCGEvent>()] };
        let ns = ns?;
        // `subtype` and `data1` are valid only for these types; checked first.
        if ns.r#type() != NSEventType::SystemDefined {
            return None;
        }
        decode(ns.subtype().0, ns.data1())
    })
}

/// Decodes a system-defined event's subtype and `data1` (layout from IOKit's
/// `ev_keymap.h`: key type in bits 16..32, key state in bits 8..16, repeat
/// flag in bit 0).
fn decode(subtype: i16, data1: isize) -> Option<KeyEvent> {
    if subtype != AUX_CONTROL_BUTTONS {
        return None;
    }
    let key = ((data1 >> 16) & 0xFFFF) as u16;
    let down = match (data1 >> 8) & 0xFF {
        KEY_DOWN => true,
        KEY_UP => false,
        _ => return None,
    };
    Some(KeyEvent::Media { key, down, autorepeat: data1 & 1 != 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSPoint;

    /// Runs `f` on a system-defined CGEvent made through NSEvent, as macOS
    /// makes the events the tap receives. Nothing is posted.
    fn with_test_event<R>(subtype: i16, data1: isize, f: impl FnOnce(ffi::CGEventRef) -> R) -> R {
        autoreleasepool(|_| {
            let none: *const AnyObject = std::ptr::null();
            // SAFETY: the documented factory for system-defined events.
            let ns: Option<Retained<NSEvent>> = unsafe {
                msg_send![
                    NSEvent::class(),
                    otherEventWithType: NSEventType::SystemDefined,
                    location: NSPoint::new(0.0, 0.0),
                    modifierFlags: 0usize,
                    timestamp: 0.0f64,
                    windowNumber: 0isize,
                    context: none,
                    subtype: subtype,
                    data1: data1,
                    data2: -1isize
                ]
            };
            let ns = ns.expect("NSEvent made a system-defined event");
            // SAFETY: the CGEvent lives as long as `ns`, which outlives `f`.
            let cg: *mut OpaqueCGEvent = unsafe { msg_send![&*ns, CGEvent] };
            assert!(!cg.is_null());
            f(cg.cast())
        })
    }

    /// `data1` of an aux-control-button event, as IOKit's `ev_keymap.h`
    /// documents it: key type in bits 16..32, key state (0xA down, 0xB up) in
    /// bits 8..16, the repeat flag in bit 0.
    fn data1(key: isize, state: isize, repeat: bool) -> isize {
        (key << 16) | (state << 8) | isize::from(repeat)
    }

    #[test]
    fn media_key_presses_and_releases_are_decoded() {
        assert_eq!(
            decode(8, data1(0, 0xA, false)),
            Some(KeyEvent::Media { key: 0, down: true, autorepeat: false })
        );
        assert_eq!(
            decode(8, data1(16, 0xB, false)),
            Some(KeyEvent::Media { key: 16, down: false, autorepeat: false })
        );
        assert_eq!(
            decode(8, data1(1, 0xA, true)),
            Some(KeyEvent::Media { key: 1, down: true, autorepeat: true })
        );
    }

    #[test]
    fn other_system_defined_events_are_not_keys() {
        assert_eq!(decode(7, data1(0, 0xA, false)), None, "aux mouse buttons");
        assert_eq!(decode(8, data1(0, 0x0, false)), None, "unknown key state");
        assert_eq!(decode(-1, -1), None);
        assert!(decode(8, isize::MIN).is_none());
    }

    #[test]
    fn a_real_system_defined_event_is_read_back() {
        // Builds the event the way macOS delivers it to the tap (NSEvent →
        // CGEvent), without posting it anywhere.
        let mute = with_test_event(8, data1(7, 0xA, false), read);
        assert_eq!(mute, Some(KeyEvent::Media { key: 7, down: true, autorepeat: false }));
        let play_up = with_test_event(8, data1(16, 0xB, true), read);
        assert_eq!(play_up, Some(KeyEvent::Media { key: 16, down: false, autorepeat: true }));
        assert_eq!(with_test_event(7, 0, read), None, "aux mouse buttons");
    }
}
