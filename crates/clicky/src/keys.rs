//! Which key moved, and which press sound it gets.
//!
//! The listener hands over only a keycode (a key *position*, independent of
//! the layout) and the event type. Nothing here can see characters.
//!
//! Tracer behaviour: a key going down plays its press sound; auto-repeat,
//! key-up and modifier (flags-changed) events make no sound yet. Issue #5
//! replaces `KeyState` with full press/release and modifier tracking and adds
//! the per-class fallback in `press_clip`.

use clicky_pack::Pack;
use clicky_pack::keys::code;

/// A keyboard event, reduced to what Clicky is allowed to know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEvent {
    Down {
        keycode: u16,
        autorepeat: bool,
    },
    Up {
        keycode: u16,
    },
    /// A modifier key moved (Shift, Control, Option, Command, Fn, Caps Lock).
    FlagsChanged {
        keycode: u16,
    },
}

/// Turns key events into press sounds.
#[derive(Debug, Default)]
pub struct KeyState {}

impl KeyState {
    /// The keycode whose press sound should play for `event`, if any.
    pub fn press(&mut self, event: KeyEvent) -> Option<u16> {
        match event {
            KeyEvent::Down { keycode, autorepeat: false } => Some(keycode),
            KeyEvent::Down { autorepeat: true, .. }
            | KeyEvent::Up { .. }
            | KeyEvent::FlagsChanged { .. } => None,
        }
    }
}

/// The clip to play for a key press: the pack's own recording of that key,
/// else the recording of A, so no key is ever silent.
pub fn press_clip(pack: &Pack, keycode: u16) -> Option<u16> {
    pack.press_clip(keycode).or_else(|| pack.press_clip(u16::from(code::A)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clicky_pack::{KEY_SLOTS, Meta};

    #[test]
    fn a_key_going_down_plays_once() {
        let mut keys = KeyState::default();
        assert_eq!(keys.press(KeyEvent::Down { keycode: 0x31, autorepeat: false }), Some(0x31));
        assert_eq!(keys.press(KeyEvent::Down { keycode: 0x31, autorepeat: true }), None);
        assert_eq!(keys.press(KeyEvent::Up { keycode: 0x31 }), None);
    }

    #[test]
    fn modifiers_are_silent_in_the_tracer() {
        let mut keys = KeyState::default();
        assert_eq!(keys.press(KeyEvent::FlagsChanged { keycode: 0x38 }), None);
    }

    #[test]
    fn a_key_the_pack_lacks_falls_back_to_a() {
        let mut press = [None; KEY_SLOTS];
        press[usize::from(code::A)] = Some(0);
        press[usize::from(code::SPACE)] = Some(1);
        let meta = Meta { id: "t".into(), ..Meta::default() };
        let pack = Pack::new(meta, 44_100, &[vec![1], vec![2]], press, [None; KEY_SLOTS]).unwrap();
        assert_eq!(press_clip(&pack, u16::from(code::SPACE)), Some(1));
        assert_eq!(press_clip(&pack, 0x34), Some(0), "unknown keycode");
        assert_eq!(press_clip(&pack, 4_000), Some(0), "out-of-range keycode");
    }
}
