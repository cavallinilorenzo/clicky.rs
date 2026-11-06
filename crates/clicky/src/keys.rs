//! Which key moved, and which press sound it gets.
//!
//! PRIVACY: the listener hands over only the event type, a keycode (a key
//! *position*, independent of the layout), the auto-repeat flag and the
//! modifier flags (which modifier keys are down). A media key is its
//! `NX_KEYTYPE_*` number. Nothing here can see characters, and [`KeyEvent`]
//! has no field that could carry one.
//!
//! [`KeyState`] decides which events are a physical press or release (one
//! press per physical press, whatever auto-repeat does); [`press_clip`] picks
//! the press sound, falling back through the key's class so no key is silent.
//! A [`Transition::Release`] plays the key's own release sound, if Key Release
//! Sounds is on and the soundpack has one (`engine::Voicer::release`).

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use clicky_pack::Pack;
use clicky_pack::keys::{self, code};

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
    /// `flags` are the event's `CGEventFlags` after the change: which modifiers
    /// are down, never which characters they produce.
    FlagsChanged {
        keycode: u16,
        flags: u64,
    },
    /// A media key (volume, brightness, play…): a system-defined event of
    /// subtype 8. `key` is its `NX_KEYTYPE_*` value.
    Media {
        key: u16,
        down: bool,
        autorepeat: bool,
    },
}

/// A physical key going down or coming back up. The keycode is the one
/// [`press_clip`] takes (media keys are mapped to the key they sit on).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transition {
    Press(u16),
    Release(u16),
}

/// `CGEventFlags` bits (IOKit `IOLLEvent.h`) that tell a modifier press from
/// its release.
mod flag {
    /// Caps Lock is on (its toggle state, not whether it is held).
    pub const ALPHA_SHIFT: u64 = 0x0001_0000;
    pub const SHIFT: u64 = 0x0002_0000;
    pub const CONTROL: u64 = 0x0004_0000;
    pub const OPTION: u64 = 0x0008_0000;
    pub const COMMAND: u64 = 0x0010_0000;
    /// Fn is held.
    pub const SECONDARY_FN: u64 = 0x0080_0000;
    // Device-dependent bits (`NX_DEVICE*KEYMASK`): which side is held.
    pub const DEV_LCTL: u64 = 0x0000_0001;
    pub const DEV_LSHIFT: u64 = 0x0000_0002;
    pub const DEV_RSHIFT: u64 = 0x0000_0004;
    pub const DEV_LCMD: u64 = 0x0000_0008;
    pub const DEV_RCMD: u64 = 0x0000_0010;
    pub const DEV_LALT: u64 = 0x0000_0020;
    pub const DEV_RALT: u64 = 0x0000_0040;
    pub const DEV_RCTL: u64 = 0x0000_2000;
    pub const DEV_ALL: u64 =
        DEV_LCTL | DEV_LSHIFT | DEV_RSHIFT | DEV_LCMD | DEV_RCMD | DEV_LALT | DEV_RALT | DEV_RCTL;
}

/// The auto-repeat flag decides: a key-down with it is never a press, one
/// without it is a new press, even if the key still counts as held (its
/// release was lost, e.g. hidden by Secure Keyboard Entry). Only a burst is
/// held back: while a key is held and is the last key that went down, a
/// key-down without the flag within this long of the previous one is not a
/// press. It is shorter than the shortest initial key-repeat delay macOS
/// offers (225 ms), so a lost release mutes the key for at most this long.
const BURST_WINDOW: Duration = Duration::from_millis(200);

/// Number of `u64` words for one bit per possible `u16` keycode.
const HELD_WORDS: usize = (u16::MAX as usize + 1) / 64;

/// Which keys are down. Turns key events into physical presses and releases:
/// one press per physical press, never auto-repeat. Owned by the tap thread;
/// no allocation after creation.
pub struct KeyState {
    /// One bit per keycode. 8 KiB, so every `u16` is tracked without a branch.
    held: Box<[u64; HELD_WORDS]>,
    /// The last key-down (including repeats): keycode and time.
    last_down: Option<(u16, Instant)>,
    /// Caps Lock's last reported toggle state, once seen.
    caps_on: Option<bool>,
}

impl Default for KeyState {
    fn default() -> KeyState {
        KeyState { held: Box::new([0; HELD_WORDS]), last_down: None, caps_on: None }
    }
}

impl std::fmt::Debug for KeyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately prints no keycodes.
        f.debug_struct("KeyState").finish_non_exhaustive()
    }
}

impl KeyState {
    /// The press or release `event` amounts to, if any. `now` is when it arrived.
    pub fn handle(&mut self, event: KeyEvent, now: Instant) -> Option<Transition> {
        match event {
            KeyEvent::Down { keycode, autorepeat } => self.down(keycode, autorepeat, now),
            KeyEvent::Up { keycode } => self.up(keycode),
            KeyEvent::FlagsChanged { keycode, flags } => self.flags_changed(keycode, flags),
            KeyEvent::Media { key, down, autorepeat } => {
                let keycode = keys::media_keycode(key)?;
                if down { self.down(keycode, autorepeat, now) } else { self.up(keycode) }
            }
        }
    }

    /// Whether `keycode` is down.
    pub fn is_held(&self, keycode: u16) -> bool {
        let (word, bit) = slot(keycode);
        self.held[word] & bit != 0
    }

    /// Forgets every held key, after events may have been lost (the tap was
    /// disabled). A key still held then just sounds again on its next press.
    pub fn reset(&mut self) {
        self.held.fill(0);
        self.last_down = None;
        self.caps_on = None;
    }

    fn set(&mut self, keycode: u16, down: bool) {
        let (word, bit) = slot(keycode);
        if down {
            self.held[word] |= bit;
        } else {
            self.held[word] &= !bit;
        }
    }

    fn down(&mut self, keycode: u16, autorepeat: bool, now: Instant) -> Option<Transition> {
        let last = self.last_down.replace((keycode, now));
        if autorepeat {
            return None;
        }
        let burst = self.is_held(keycode)
            && last.is_some_and(|(k, at)| {
                k == keycode && now.saturating_duration_since(at) < BURST_WINDOW
            });
        if burst {
            return None;
        }
        self.set(keycode, true);
        Some(Transition::Press(keycode))
    }

    fn up(&mut self, keycode: u16) -> Option<Transition> {
        if !self.is_held(keycode) {
            return None;
        }
        self.set(keycode, false);
        Some(Transition::Release(keycode))
    }

    /// A modifier moved. Press or release is read from the flags after the
    /// change: the modifier's device bit tells which side is held.
    fn flags_changed(&mut self, keycode: u16, flags: u64) -> Option<Transition> {
        let down = match modifier(keycode) {
            Modifier::CapsLock => return self.caps_lock(keycode, flags & flag::ALPHA_SHIFT != 0),
            Modifier::Fn => flags & flag::SECONDARY_FN != 0,
            Modifier::Sided { flag: class_flag, device_bit } => {
                if flags & class_flag == 0 {
                    false
                } else if flags & flag::DEV_ALL != 0 {
                    flags & device_bit != 0
                } else {
                    // No device bits (some keyboards and remapping tools): toggle.
                    !self.is_held(keycode)
                }
            }
            Modifier::Unknown => !self.is_held(keycode),
        };
        if down == self.is_held(keycode) {
            return None;
        }
        self.set(keycode, down);
        Some(if down { Transition::Press(keycode) } else { Transition::Release(keycode) })
    }

    /// macOS reports Caps Lock when its toggle state changes, so a change is a
    /// press whether it turned on or off. A report without a change is the key
    /// coming back up, when macOS sends one.
    fn caps_lock(&mut self, keycode: u16, on: bool) -> Option<Transition> {
        if self.caps_on.replace(on) == Some(on) {
            return self.up(keycode);
        }
        self.set(keycode, true);
        Some(Transition::Press(keycode))
    }
}

/// Decides which unrecognised input is worth one log line: the first of each
/// kind per process, described without its keycode. Unrecognised keys still
/// sound (through [`press_clip`]'s fallback); this only keeps the log quiet.
///
/// Split in two so that the tap callback never logs: the callback calls
/// [`note`](LogOnce::note) (an atomic bit: no lock, no allocation), and the
/// tap thread, between run-loop passes, logs what
/// [`take_new`](LogOnce::take_new) hands it.
#[derive(Debug, Default)]
pub struct LogOnce {
    /// One bit per kind seen (set by the tap callback).
    seen: AtomicU8,
    /// One bit per kind already handed out for logging.
    handed_out: AtomicU8,
}

/// The kinds of unrecognised input, one bit each.
const UNRECOGNISED: [(u8, &str); 3] = [
    (1, "a key outside the key table"),
    (2, "a modifier change from a key that is not a modifier"),
    (4, "a media key with no known position"),
];

impl LogOnce {
    /// Remembers `event`'s kind if it is unrecognised. Real-time safe: for
    /// the tap callback.
    pub fn note(&self, event: &KeyEvent) {
        let bit = match *event {
            KeyEvent::Down { keycode, .. } | KeyEvent::Up { keycode } if !known(keycode) => 1,
            KeyEvent::FlagsChanged { keycode, .. }
                if matches!(modifier(keycode), Modifier::Unknown) =>
            {
                2
            }
            KeyEvent::Media { key, .. }
                if keys::media_keycode(key).is_some_and(|k| k >= keys::MEDIA_OTHER_BASE) =>
            {
                4
            }
            _ => return,
        };
        if self.seen.load(Ordering::Relaxed) & bit == 0 {
            self.seen.fetch_or(bit, Ordering::Relaxed);
        }
    }

    /// A description of each kind noted since the last call and never handed
    /// out before, so each kind at most once per process. Never contains a
    /// keycode. Not for the tap callback: its caller logs.
    pub fn take_new(&self) -> impl Iterator<Item = &'static str> {
        let new = self.seen.load(Ordering::Relaxed) & !self.handed_out.load(Ordering::Relaxed);
        self.handed_out.fetch_or(new, Ordering::Relaxed);
        UNRECOGNISED.into_iter().filter(move |(bit, _)| new & bit != 0).map(|(_, kind)| kind)
    }
}

fn known(keycode: u16) -> bool {
    u8::try_from(keycode).is_ok_and(|k| keys::info(k).is_some())
}

fn slot(keycode: u16) -> (usize, u64) {
    (usize::from(keycode / 64), 1 << (keycode % 64))
}

enum Modifier {
    CapsLock,
    Fn,
    Sided {
        flag: u64,
        device_bit: u64,
    },
    /// A flags change from a key that is not a known modifier.
    Unknown,
}

fn modifier(keycode: u16) -> Modifier {
    let sided = |flag, device_bit| Modifier::Sided { flag, device_bit };
    match u8::try_from(keycode).unwrap_or(0xFF) {
        code::CAPS_LOCK => Modifier::CapsLock,
        code::FUNCTION => Modifier::Fn,
        code::SHIFT => sided(flag::SHIFT, flag::DEV_LSHIFT),
        code::RIGHT_SHIFT => sided(flag::SHIFT, flag::DEV_RSHIFT),
        code::CONTROL => sided(flag::CONTROL, flag::DEV_LCTL),
        code::RIGHT_CONTROL => sided(flag::CONTROL, flag::DEV_RCTL),
        code::OPTION => sided(flag::OPTION, flag::DEV_LALT),
        code::RIGHT_OPTION => sided(flag::OPTION, flag::DEV_RALT),
        code::COMMAND => sided(flag::COMMAND, flag::DEV_LCMD),
        code::RIGHT_COMMAND => sided(flag::COMMAND, flag::DEV_RCMD),
        _ => Modifier::Unknown,
    }
}

/// The clip to play for a key press, so no key is ever silent: the pack's own
/// recording of that key, else the recording of a similar key of its class
/// (`clicky_pack::keys::fallbacks`: right Shift → Shift, Keypad Enter →
/// Return, Keypad 7 → 7, F13 → F-row, unknown → A), else the pack's first
/// clip. `None` only for a pack with no clips. Total over every `u16`.
/// Runs on the tap thread: no allocation.
pub fn press_clip(pack: &Pack, keycode: u16) -> Option<u16> {
    pack.press_clip(keycode)
        .or_else(|| keys::fallbacks(keycode).find_map(|key| pack.press_clip(u16::from(key))))
        .or_else(|| (pack.clip_count() > 0).then_some(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clicky_pack::{KEY_SLOTS, Meta};

    use std::time::{Duration, Instant};

    // CGEventFlags, from IOKit's IOLLEvent.h (independent of the code under test).
    const ALPHA_SHIFT: u64 = 0x0001_0000;
    const SHIFT: u64 = 0x0002_0000;
    const CONTROL: u64 = 0x0004_0000;
    const OPTION: u64 = 0x0008_0000;
    const COMMAND: u64 = 0x0010_0000;
    const SECONDARY_FN: u64 = 0x0080_0000;
    const DEV_LCTL: u64 = 0x0001;
    const DEV_LSHIFT: u64 = 0x0002;
    const DEV_RSHIFT: u64 = 0x0004;
    const DEV_LCMD: u64 = 0x0008;
    const DEV_LALT: u64 = 0x0020;
    const DEV_RCTL: u64 = 0x2000;
    /// Set on every flags-changed event from a real keyboard.
    const NON_COALESCED: u64 = 0x0100;

    /// A `KeyState` driven with timestamps in ms.
    struct Keys {
        state: KeyState,
        t0: Instant,
    }

    impl Keys {
        fn new() -> Keys {
            Keys { state: KeyState::default(), t0: Instant::now() }
        }
        fn at(&mut self, ms: u64, event: KeyEvent) -> Option<Transition> {
            self.state.handle(event, self.t0 + Duration::from_millis(ms))
        }
        fn down(&mut self, ms: u64, keycode: u16) -> Option<Transition> {
            self.at(ms, KeyEvent::Down { keycode, autorepeat: false })
        }
        fn repeat(&mut self, ms: u64, keycode: u16) -> Option<Transition> {
            self.at(ms, KeyEvent::Down { keycode, autorepeat: true })
        }
        fn up(&mut self, ms: u64, keycode: u16) -> Option<Transition> {
            self.at(ms, KeyEvent::Up { keycode })
        }
        fn flags(&mut self, ms: u64, keycode: u8, flags: u64) -> Option<Transition> {
            let keycode = u16::from(keycode);
            self.at(ms, KeyEvent::FlagsChanged { keycode, flags: flags | NON_COALESCED })
        }
        fn media(&mut self, ms: u64, key: u16, down: bool, autorepeat: bool) -> Option<Transition> {
            self.at(ms, KeyEvent::Media { key, down, autorepeat })
        }
    }

    use Transition::{Press, Release};
    const J: u16 = 0x26;
    const K: u16 = 0x28;

    #[test]
    fn a_key_sounds_on_press_and_tracks_its_release() {
        let mut keys = Keys::new();
        assert_eq!(keys.down(0, J), Some(Press(J)));
        assert!(keys.state.is_held(J));
        assert_eq!(keys.up(80, J), Some(Release(J)));
        assert!(!keys.state.is_held(J));
        assert_eq!(keys.up(90, J), None, "a release without a press");
        assert_eq!(keys.down(200, J), Some(Press(J)), "the next press sounds again");
    }

    #[test]
    fn holding_a_key_for_three_seconds_sounds_once() {
        let mut keys = Keys::new();
        let presses = std::iter::once(keys.down(0, J))
            .chain((500..3_000).step_by(30).map(|ms| keys.repeat(ms, J)))
            .filter(Option::is_some)
            .count();
        assert_eq!(presses, 1);
        assert_eq!(keys.up(3_000, J), Some(Release(J)));
    }

    #[test]
    fn key_downs_without_the_repeat_flag_cannot_burst() {
        let mut keys = Keys::new();
        assert_eq!(keys.down(0, J), Some(Press(J)));
        for ms in (30..3_000).step_by(30) {
            assert_eq!(keys.down(ms, J), None, "duplicate key-down at {ms} ms");
        }
        // After a pause as long as the initial key-repeat delay, a key-down
        // without the repeat flag is a new press: the flag decides, and a
        // stream of flagless repeats sounds at most once more.
        assert_eq!(keys.down(3_500, J), Some(Press(J)));
        for ms in (3_530..5_000).step_by(30) {
            assert_eq!(keys.down(ms, J), None, "flagless repeat at {ms} ms");
        }
    }

    #[test]
    fn a_lost_key_up_mutes_the_key_only_briefly() {
        // The key went up unseen (e.g. hidden by Secure Keyboard Entry), so it
        // still counts as held when it is pressed again.
        let mut keys = Keys::new();
        assert_eq!(keys.down(0, J), Some(Press(J)));
        assert_eq!(keys.down(300, J), Some(Press(J)), "the next press sounds");
        assert_eq!(keys.repeat(800, J), None, "a flagged repeat never does");
        assert_eq!(keys.up(900, J), Some(Release(J)));
        assert_eq!(keys.down(1_000, J), Some(Press(J)));
    }

    #[test]
    fn a_lost_release_cannot_silence_the_key() {
        // E.g. the key went up while Secure Keyboard Entry hid it.
        let mut keys = Keys::new();
        assert_eq!(keys.down(0, J), Some(Press(J)));
        assert_eq!(keys.down(5_000, J), Some(Press(J)), "long after: a new press");
        assert_eq!(keys.down(5_200, K), Some(Press(K)));
        assert_eq!(keys.down(5_300, J), Some(Press(J)), "another key went down since");
    }

    #[test]
    fn a_reset_forgets_held_keys() {
        let mut keys = Keys::new();
        keys.down(0, J);
        keys.state.reset();
        assert!(!keys.state.is_held(J));
        assert_eq!(keys.down(100, J), Some(Press(J)));
    }

    #[test]
    fn out_of_range_keycodes_sound_without_panicking() {
        let mut keys = Keys::new();
        for keycode in [128, 145, 1_000, u16::MAX] {
            assert_eq!(keys.down(0, keycode), Some(Press(keycode)));
            assert_eq!(keys.repeat(40, keycode), None);
            assert_eq!(keys.up(80, keycode), Some(Release(keycode)));
        }
    }

    #[test]
    fn shift_sounds_once_on_press_and_tracks_its_release() {
        let mut keys = Keys::new();
        let shift = u16::from(code::SHIFT);
        assert_eq!(keys.flags(0, code::SHIFT, SHIFT | DEV_LSHIFT), Some(Press(shift)));
        assert_eq!(keys.flags(5, code::SHIFT, SHIFT | DEV_LSHIFT), None, "duplicate");
        assert_eq!(keys.flags(90, code::SHIFT, 0), Some(Release(shift)));
    }

    #[test]
    fn every_modifier_sounds_on_press_from_its_device_bit() {
        // (keycode, device-independent flag, device bit), left and right.
        let modifiers = [
            (code::SHIFT, SHIFT, DEV_LSHIFT),
            (code::RIGHT_SHIFT, SHIFT, DEV_RSHIFT),
            (code::CONTROL, CONTROL, DEV_LCTL),
            (code::RIGHT_CONTROL, CONTROL, DEV_RCTL),
            (code::OPTION, OPTION, DEV_LALT),
            (code::RIGHT_OPTION, OPTION, 0x0040),
            (code::COMMAND, COMMAND, DEV_LCMD),
            (code::RIGHT_COMMAND, COMMAND, 0x0010),
        ];
        for (keycode, flag, bit) in modifiers {
            let mut keys = Keys::new();
            let k = u16::from(keycode);
            assert_eq!(keys.flags(0, keycode, flag | bit), Some(Press(k)), "{keycode:#x}");
            assert_eq!(keys.flags(90, keycode, 0), Some(Release(k)), "{keycode:#x}");
        }
    }

    #[test]
    fn fn_sounds_once_on_press() {
        let mut keys = Keys::new();
        let f = u16::from(code::FUNCTION);
        assert_eq!(keys.flags(0, code::FUNCTION, SECONDARY_FN), Some(Press(f)));
        assert_eq!(keys.flags(10, code::FUNCTION, SECONDARY_FN), None);
        assert_eq!(keys.flags(90, code::FUNCTION, 0), Some(Release(f)));
    }

    #[test]
    fn left_and_right_of_one_modifier_are_told_apart() {
        let mut keys = Keys::new();
        let (l, r) = (u16::from(code::SHIFT), u16::from(code::RIGHT_SHIFT));
        assert_eq!(keys.flags(0, code::SHIFT, SHIFT | DEV_LSHIFT), Some(Press(l)));
        let both = SHIFT | DEV_LSHIFT | DEV_RSHIFT;
        assert_eq!(keys.flags(50, code::RIGHT_SHIFT, both), Some(Press(r)));
        assert_eq!(keys.flags(100, code::SHIFT, SHIFT | DEV_RSHIFT), Some(Release(l)));
        assert_eq!(keys.flags(150, code::RIGHT_SHIFT, 0), Some(Release(r)));
    }

    #[test]
    fn modifiers_pressed_with_other_keys_sound_once_each() {
        let mut keys = Keys::new();
        let cmd = u16::from(code::COMMAND);
        let shift = u16::from(code::SHIFT);
        let mut played = Vec::new();
        let mut note = |t: Option<Transition>| played.extend(t);
        // ⌘⇧J, with J auto-repeating while the modifiers stay down.
        note(keys.flags(0, code::COMMAND, COMMAND | DEV_LCMD));
        note(keys.flags(20, code::SHIFT, COMMAND | SHIFT | DEV_LCMD | DEV_LSHIFT));
        note(keys.down(40, J));
        for ms in (540..1_500).step_by(30) {
            note(keys.repeat(ms, J));
        }
        note(keys.up(1_500, J));
        note(keys.flags(1_520, code::COMMAND, SHIFT | DEV_LSHIFT));
        note(keys.down(1_540, K)); // ⇧K while Shift is still held
        note(keys.up(1_600, K));
        note(keys.flags(1_620, code::SHIFT, 0));
        assert_eq!(
            played,
            [
                Press(cmd),
                Press(shift),
                Press(J),
                Release(J),
                Release(cmd),
                Press(K),
                Release(K),
                Release(shift)
            ]
        );
    }

    #[test]
    fn modifiers_without_device_bits_toggle() {
        // Some keyboards and remapping tools send only the device-independent flag.
        let mut keys = Keys::new();
        let opt = u16::from(code::OPTION);
        assert_eq!(keys.flags(0, code::OPTION, OPTION), Some(Press(opt)));
        assert_eq!(keys.flags(90, code::OPTION, 0), Some(Release(opt)));
        assert_eq!(keys.flags(200, code::OPTION, OPTION), Some(Press(opt)));
    }

    #[test]
    fn caps_lock_sounds_on_every_press_whether_it_turns_on_or_off() {
        let mut keys = Keys::new();
        let caps = u16::from(code::CAPS_LOCK);
        // macOS reports Caps Lock when its state toggles.
        assert_eq!(keys.flags(0, code::CAPS_LOCK, ALPHA_SHIFT), Some(Press(caps)));
        assert_eq!(keys.flags(500, code::CAPS_LOCK, 0), Some(Press(caps)));
        assert_eq!(keys.flags(900, code::CAPS_LOCK, ALPHA_SHIFT), Some(Press(caps)));
        // A report with no change in state is the key coming back up.
        assert_eq!(keys.flags(1_000, code::CAPS_LOCK, ALPHA_SHIFT), Some(Release(caps)));
        assert_eq!(keys.flags(1_010, code::CAPS_LOCK, ALPHA_SHIFT), None);
        assert_eq!(keys.flags(1_500, code::CAPS_LOCK, 0), Some(Press(caps)));
    }

    #[test]
    fn caps_lock_held_with_shift_still_sounds_once() {
        let mut keys = Keys::new();
        let caps = u16::from(code::CAPS_LOCK);
        let shift = u16::from(code::SHIFT);
        assert_eq!(keys.flags(0, code::SHIFT, SHIFT | DEV_LSHIFT), Some(Press(shift)));
        let on = ALPHA_SHIFT | SHIFT | DEV_LSHIFT;
        assert_eq!(keys.flags(50, code::CAPS_LOCK, on), Some(Press(caps)));
        assert_eq!(keys.flags(150, code::SHIFT, ALPHA_SHIFT), Some(Release(shift)));
    }

    #[test]
    fn a_flags_change_from_an_unknown_key_toggles() {
        let mut keys = Keys::new();
        assert_eq!(keys.flags(0, 0x7F, SHIFT), Some(Press(0x7F)));
        assert_eq!(keys.flags(90, 0x7F, 0), Some(Release(0x7F)));
    }

    #[test]
    fn unrecognised_input_is_reported_once_per_kind_without_its_keycode() {
        let once = LogOnce::default();
        let down = |keycode| KeyEvent::Down { keycode, autorepeat: false };
        let flags = |keycode| KeyEvent::FlagsChanged { keycode, flags: 0 };
        let media = |key| KeyEvent::Media { key, down: true, autorepeat: false };
        let known = [
            down(u16::from(code::A)),
            KeyEvent::Up { keycode: 0x7E },
            flags(u16::from(code::RIGHT_OPTION)),
            media(0), // volume up
        ];
        known.iter().for_each(|event| once.note(event));
        assert_eq!(once.take_new().count(), 0, "known keys are not reported");

        let unknown = [down(0x34), down(145), flags(0x00), media(15), media(99)];
        let mut kinds: Vec<&str> = Vec::new();
        for (i, event) in unknown.iter().cycle().take(5_000).enumerate() {
            once.note(event);
            if i % 7 == 0 {
                // The tap thread drains between run-loop passes, at any point.
                kinds.extend(once.take_new());
            }
        }
        kinds.extend(once.take_new());
        assert_eq!(kinds.len(), 3, "one line per kind, however often: {kinds:?}");
        for kind in kinds {
            assert!(!kind.chars().any(|c| c.is_ascii_digit()), "no keycode in {kind:?}");
        }
        unknown.iter().for_each(|event| once.note(event));
        assert_eq!(once.take_new().count(), 0, "never again in this process");
    }

    #[test]
    fn media_keys_sound_once_on_press() {
        let mut keys = Keys::new();
        let volume_up = u16::from(code::VOLUME_UP);
        assert_eq!(keys.media(0, 0, true, false), Some(Press(volume_up)));
        assert_eq!(keys.media(500, 0, true, true), None, "held volume key repeats");
        assert_eq!(keys.media(530, 0, true, false), None, "repeat with its flag lost");
        assert_eq!(keys.media(600, 0, false, false), Some(Release(volume_up)));
        let play = 16;
        assert_eq!(keys.media(700, play, true, false), Some(Press(0x64)));
        assert_eq!(keys.media(790, play, false, false), Some(Release(0x64)));
        let unknown = keys.media(800, 99, true, false);
        assert!(matches!(unknown, Some(Press(k)) if k > 127), "{unknown:?}");
        assert_eq!(keys.media(900, 4, true, false), None, "Caps Lock comes as flags");
        assert!(keys.media(950, u16::MAX, true, false).is_some());
    }

    /// A pack with one distinct clip per listed key (clip i for keys[i]),
    /// plus one unmapped clip at the end.
    fn pack_with(keys: &[u8]) -> Pack {
        let mut press = [None; KEY_SLOTS];
        for (i, &key) in keys.iter().enumerate() {
            press[usize::from(key)] = Some(i as u16);
        }
        let clips: Vec<Vec<i16>> = (0..=keys.len()).map(|i| vec![i as i16]).collect();
        let meta = Meta { id: "t".into(), ..Meta::default() };
        Pack::new(meta, 44_100, &clips, press, [None; KEY_SLOTS]).unwrap()
    }

    #[test]
    fn keys_the_pack_lacks_use_their_class() {
        use code::*;
        let keys = [A, ONE, RETURN, DELETE, SHIFT, COMMAND, F1, SPACE, UP_ARROW];
        let pack = pack_with(&keys);
        let clip_of = |key: u8| Some(keys.iter().position(|&k| k == key).unwrap() as u16);
        assert_eq!(press_clip(&pack, u16::from(SPACE)), clip_of(SPACE), "its own recording");
        assert_eq!(press_clip(&pack, u16::from(KEYPAD_ENTER)), clip_of(RETURN));
        assert_eq!(press_clip(&pack, 0x53), clip_of(ONE), "Keypad 1 → 1");
        assert_eq!(press_clip(&pack, 0x59), clip_of(ONE), "Keypad 7 → number row");
        assert_eq!(press_clip(&pack, u16::from(RIGHT_SHIFT)), clip_of(SHIFT));
        assert_eq!(press_clip(&pack, u16::from(OPTION)), clip_of(SHIFT));
        assert_eq!(press_clip(&pack, u16::from(FUNCTION)), clip_of(SHIFT));
        assert_eq!(press_clip(&pack, u16::from(RIGHT_COMMAND)), clip_of(COMMAND));
        assert_eq!(press_clip(&pack, u16::from(CAPS_LOCK)), clip_of(SHIFT));
        assert_eq!(press_clip(&pack, 0x69), clip_of(F1), "F13 → F-row");
        assert_eq!(press_clip(&pack, 0x50), clip_of(F1), "F19 → F-row");
        assert_eq!(press_clip(&pack, u16::from(FORWARD_DELETE)), clip_of(DELETE));
        assert_eq!(press_clip(&pack, 0x7D), clip_of(UP_ARROW), "Down Arrow → arrows");
        assert_eq!(press_clip(&pack, 0x34), clip_of(A), "unknown → letter class");
        assert_eq!(press_clip(&pack, 145), clip_of(F1), "brightness key above 127");
        assert_eq!(press_clip(&pack, 60_000), clip_of(A), "out of range");
    }

    #[test]
    fn a_pack_without_a_mapping_still_plays_its_default_sound() {
        let pack = pack_with(&[]);
        assert_eq!(press_clip(&pack, u16::from(code::SPACE)), Some(0));
        assert_eq!(press_clip(&pack, u16::MAX), Some(0));
    }

    #[test]
    fn every_keycode_and_media_key_resolves_to_a_clip() {
        // A pack that recorded only one key, Return: everything still sounds.
        let sparse = pack_with(&[code::RETURN]);
        // A pack with every known key recorded: every key gets its own.
        let full_keys: Vec<u8> = clicky_pack::keys::KEYS.iter().map(|k| k.code).collect();
        let full = pack_with(&full_keys);
        let media = (0..=40).chain([255, 4_096, u16::MAX]).filter_map(keys::media_keycode);
        for keycode in (0..=127).chain(media) {
            assert!(press_clip(&sparse, keycode).is_some(), "keycode {keycode} is silent");
            assert!(press_clip(&full, keycode).is_some(), "keycode {keycode} is silent");
            if let Some(i) = full_keys.iter().position(|&k| u16::from(k) == keycode) {
                assert_eq!(press_clip(&full, keycode), Some(i as u16), "{keycode}");
            }
        }
    }

    #[test]
    fn an_empty_pack_is_silent_without_panicking() {
        let meta = Meta { id: "t".into(), ..Meta::default() };
        let pack = Pack::new(meta, 44_100, &[], [None; KEY_SLOTS], [None; KEY_SLOTS]).unwrap();
        assert_eq!(press_clip(&pack, u16::from(code::A)), None);
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
