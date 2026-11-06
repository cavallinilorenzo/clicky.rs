//! The sound engine's shared state and the key-listener side of the
//! lock-free path:
//!
//! ```text
//! tap thread: Voicer::key ──Play──▶ SPSC ring ──▶ render callback: Mixer
//!                  └─ stamps the key time and wakes the audio control thread
//! ```
//!
//! The voicer applies the sound options (`options`): release sounds on
//! key-up, natural variation (gain and pitch per keystroke) and spatial
//! stereo (pan by key position).
//!
//! Everything both sides read lives in atomics in [`Shared`], so the render
//! callback never takes a lock. That includes the current soundpack: the menu
//! swaps it with one atomic store, the next key press plays from the new pack,
//! and voices already playing finish from the old one (packs are never freed).

pub mod mixer;
pub mod options;
pub mod preview;
pub mod ring;

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering, fence};
use std::thread::Thread;
use std::time::Instant;

use clicky_pack::Pack;

use crate::keys::{self, Transition};
use crate::settings::Setting;
use mixer::Play;
use options::Rng;
use ring::Producer;

/// The on/off switch.
pub const SOUNDS_ON: Setting<bool> = Setting { key: "SoundsOn", default: true };

/// Master gain at the volume slider's maximum (`mixer::volume_gain`). Leaves
/// headroom over the soundpack's −1 dBFS clip peaks, natural variation and
/// panning, so a lone keystroke never reaches the limiter.
pub const MASTER_GAIN: f32 = 0.6;

/// Capacity of the key-to-mixer ring (voices waiting for the next render).
pub const RING_CAPACITY: usize = 256;

/// State shared by the main thread, the key listener, the audio control
/// thread and the render callback. Lives for the whole process.
pub struct Shared {
    /// Sounds on (the menu switch).
    enabled: AtomicBool,
    /// Master gain as `f32` bits.
    master: AtomicU32,
    /// Whether the output is warm; while it is, key presses need not wake the
    /// control thread.
    pub warm: AtomicBool,
    /// Time of the last key press, in ns since `epoch` plus one (0: none yet).
    last_key: AtomicU64,
    /// Time of the first render after a start, same encoding (0: not yet).
    first_render: AtomicU64,
    /// Key presses lost because the ring was full.
    pub dropped: AtomicU64,
    epoch: Instant,
    /// The current soundpack (null: none yet). Only ever set from a `&'static Pack`.
    pack: AtomicPtr<Pack>,
    /// Key Release Sounds (has no effect for a pack without release sounds).
    release_sounds: AtomicBool,
    /// Spatial Stereo.
    spatial: AtomicBool,
}

impl Shared {
    pub fn new(enabled: bool) -> Shared {
        Shared {
            enabled: AtomicBool::new(enabled),
            master: AtomicU32::new(MASTER_GAIN.to_bits()),
            warm: AtomicBool::new(false),
            last_key: AtomicU64::new(0),
            first_render: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            epoch: Instant::now(),
            pack: AtomicPtr::new(std::ptr::null_mut()),
            release_sounds: AtomicBool::new(options::RELEASE_SOUNDS.default),
            spatial: AtomicBool::new(options::SPATIAL_STEREO.default),
        }
    }

    pub fn release_sounds(&self) -> bool {
        self.release_sounds.load(Ordering::Relaxed)
    }

    pub fn set_release_sounds(&self, on: bool) {
        self.release_sounds.store(on, Ordering::Relaxed);
    }

    pub fn spatial(&self) -> bool {
        self.spatial.load(Ordering::Relaxed)
    }

    pub fn set_spatial(&self, on: bool) {
        self.spatial.store(on, Ordering::Relaxed);
    }

    /// The soundpack key presses play from. Lock-free: one atomic load.
    pub fn pack(&self) -> Option<&'static Pack> {
        let pack = self.pack.load(Ordering::Acquire);
        // SAFETY: `pack` is null or came from a `&'static Pack` in `set_pack`; packs
        // are never freed, so it stays valid for the life of the process.
        unsafe { pack.cast_const().as_ref() }
    }

    /// Switches soundpack. The next key press uses `pack`; voices already
    /// playing finish from the previous pack.
    pub fn set_pack(&self, pack: &'static Pack) {
        self.pack.store(std::ptr::from_ref(pack).cast_mut(), Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    /// Sets the master gain used while sounds are on (issue #6: the volume slider).
    pub fn set_master_gain(&self, gain: f32) {
        self.master.store(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// The gain the mixer should reach: 0 while sounds are off.
    pub fn master_gain(&self) -> f32 {
        if self.enabled() { f32::from_bits(self.master.load(Ordering::Relaxed)) } else { 0.0 }
    }

    /// Time of the last key press. Real-time safe.
    pub fn last_key_press(&self) -> Option<Instant> {
        self.decode(self.last_key.load(Ordering::Acquire))
    }

    /// Time of the first render since `clear_first_render`.
    pub fn first_render(&self) -> Option<Instant> {
        self.decode(self.first_render.load(Ordering::Acquire))
    }

    pub fn clear_first_render(&self) {
        self.first_render.store(0, Ordering::Release);
    }

    /// Called by the render callback on every cycle; records only the first.
    /// Real-time safe: one clock read and atomics.
    pub fn note_render(&self) {
        if self.first_render.load(Ordering::Relaxed) == 0 {
            self.first_render.store(self.encode(Instant::now()), Ordering::Release);
        }
    }

    fn note_key_press(&self, at: Instant) {
        self.last_key.store(self.encode(at), Ordering::Release);
    }

    fn encode(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.epoch).as_nanos() as u64 + 1
    }

    fn decode(&self, stamp: u64) -> Option<Instant> {
        stamp.checked_sub(1).map(|ns| self.epoch + std::time::Duration::from_nanos(ns))
    }
}

/// Wakes the audio control thread after a key press while the output is not warm.
#[derive(Clone)]
pub struct Kick {
    thread: Option<Thread>,
}

impl Kick {
    pub fn new(thread: Thread) -> Kick {
        Kick { thread: Some(thread) }
    }

    /// A kick that wakes nothing, for tests.
    #[cfg(test)]
    pub fn none() -> Kick {
        Kick { thread: None }
    }

    /// Call after stamping the key press. Only when the output is not warm
    /// does it unpark the control thread, the tap thread's one system call.
    fn kick(&self, shared: &Shared) {
        // Pairs with the fence after the control thread clears `warm`.
        fence(Ordering::SeqCst);
        if !shared.warm.load(Ordering::SeqCst)
            && let Some(thread) = &self.thread
        {
            thread.unpark();
        }
    }
}

/// The key listener's end of the path: turns a key press or release into a
/// voice request. Runs on the tap thread: no locks, no allocation, and no
/// system call except one: a press while the output is not warm wakes the
/// audio control thread ([`Kick`], `Thread::unpark`, which may enter the
/// kernel). That happens only for presses while the output is cold or still
/// starting; while warm (the hold window after each press) a press makes no
/// system call. It plays
/// from whichever soundpack [`Shared::pack`] holds at the time of the key,
/// with the options [`Shared`] holds then.
pub struct Voicer {
    producer: Producer<Play>,
    shared: &'static Shared,
    kick: Kick,
    /// Natural Variation's draws.
    rng: Rng,
}

impl Voicer {
    /// A voicer whose current soundpack is `pack` (stored in `shared`, where the
    /// menu can switch it).
    pub fn new(
        pack: &'static Pack,
        producer: Producer<Play>,
        shared: &'static Shared,
        kick: Kick,
    ) -> Voicer {
        shared.set_pack(pack);
        Voicer { producer, shared, kick, rng: Rng::seeded() }
    }

    /// Another voicer playing from the same current soundpack, for a second
    /// thread (the preview). `producer` must feed its own ring.
    pub fn sharing(producer: Producer<Play>, shared: &'static Shared, kick: Kick) -> Voicer {
        Voicer { producer, shared, kick, rng: Rng::seeded() }
    }

    /// Plays what a physical key transition sounds like.
    pub fn key(&mut self, transition: Transition) {
        match transition {
            Transition::Press(keycode) => self.press(keycode),
            Transition::Release(keycode) => self.release(keycode),
        }
    }

    /// Plays the press sound of `keycode` from the current soundpack, unless sounds are off.
    pub fn press(&mut self, keycode: u16) {
        if !self.shared.enabled() {
            return;
        }
        let Some(pack) = self.shared.pack() else { return };
        if let Some(clip) = keys::press_clip(pack, keycode) {
            self.play(pack, clip, keycode);
        }
    }

    /// Plays the release sound of `keycode`, if Key Release Sounds is on and
    /// the current soundpack recorded one for that key. There is no fallback
    /// to another key's release: the pack's press sound may already hold one.
    pub fn release(&mut self, keycode: u16) {
        if !self.shared.enabled() || !self.shared.release_sounds() {
            return;
        }
        let Some(pack) = self.shared.pack() else { return };
        if let Some(clip) = pack.release_clip(keycode) {
            self.play(pack, clip, keycode);
        }
    }

    fn play(&mut self, pack: &'static Pack, clip: u16, keycode: u16) {
        let Some(samples) = pack.clip(clip) else { return };
        let (gain, pitch) = options::variation(self.rng.unit(), self.rng.unit());
        let pan = if self.shared.spatial() { options::key_pan(keycode) } else { 0.0 };
        let play = Play { samples, src_rate: pack.sample_rate, gain, pan, pitch };
        if !self.producer.push(play) {
            self.shared.dropped.fetch_add(1, Ordering::Relaxed);
        }
        self.shared.note_key_press(Instant::now());
        self.kick.kick(self.shared);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clicky_pack::keys::code;
    use clicky_pack::{KEY_SLOTS, Meta};

    fn pack() -> &'static Pack {
        pack_with(44_100, [vec![1, 2], vec![3, 4, 5]])
    }

    /// A pack whose A plays `clips[0]` and Space `clips[1]`.
    pub(crate) fn pack_with(rate: u32, clips: [Vec<i16>; 2]) -> &'static Pack {
        let mut press = [None; KEY_SLOTS];
        press[usize::from(code::A)] = Some(0);
        press[usize::from(code::SPACE)] = Some(1);
        let meta = Meta { id: "t".into(), ..Meta::default() };
        Box::leak(Box::new(Pack::new(meta, rate, &clips, press, [None; KEY_SLOTS]).unwrap()))
    }

    fn voicer(enabled: bool) -> (Voicer, ring::Consumer<Play>, &'static Shared) {
        let shared: &'static Shared = Box::leak(Box::new(Shared::new(enabled)));
        let (producer, consumer) = ring::ring(4);
        (Voicer::new(pack(), producer, shared, Kick::none()), consumer, shared)
    }

    /// Release sound of A in [`releasing_pack`].
    const A_UP: [i16; 2] = [-7, -8];

    /// A pack with a press sound for A, Space, J and Shift, and a release
    /// sound for A, J and Shift only (not Space).
    fn releasing_pack() -> &'static Pack {
        let mut press = [None; KEY_SLOTS];
        let mut release = [None; KEY_SLOTS];
        for key in [code::A, code::SPACE, code::J, code::SHIFT] {
            press[usize::from(key)] = Some(0);
        }
        for key in [code::A, code::J, code::SHIFT] {
            release[usize::from(key)] = Some(1);
        }
        let clips = [vec![1_000, 2_000, 3_000], A_UP.to_vec()];
        let meta = Meta { id: "r".into(), ..Meta::default() };
        Box::leak(Box::new(Pack::new(meta, 48_000, &clips, press, release).unwrap()))
    }

    /// A voicer with every option set as given and a roomy ring.
    fn voicer_with(
        pack: &'static Pack,
        (release, spatial): (bool, bool),
    ) -> (Voicer, ring::Consumer<Play>, &'static Shared) {
        let shared: &'static Shared = Box::leak(Box::new(Shared::new(true)));
        shared.set_release_sounds(release);
        shared.set_spatial(spatial);
        let (producer, consumer) = ring::ring(1_024);
        (Voicer::new(pack, producer, shared, Kick::none()), consumer, shared)
    }

    fn drain(ring: &mut ring::Consumer<Play>) -> Vec<Play> {
        std::iter::from_fn(|| ring.pop()).collect()
    }

    /// Renders `play` alone at 48 kHz: (left, right).
    fn rendered(play: &Play) -> (Vec<f32>, Vec<f32>) {
        let mut mixer = mixer::Mixer::new(48_000.0);
        mixer.start(play);
        let (mut l, mut r) = (vec![0.0; 64], vec![0.0; 64]);
        mixer.render(&mut l, &mut r, 1.0);
        (l, r)
    }

    #[test]
    fn each_keystroke_varies_within_its_bounds() {
        let (mut voicer, mut ring, _) = voicer_with(releasing_pack(), (true, false));
        for _ in 0..500 {
            voicer.press(u16::from(code::A));
        }
        let plays = drain(&mut ring);
        let db: Vec<f32> = plays.iter().map(|p| 20.0 * p.gain.log10()).collect();
        let cents: Vec<f32> = plays.iter().map(|p| 1_200.0 * p.pitch.log2()).collect();
        let range =
            |x: &[f32]| x.iter().fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let (db_lo, db_hi) = range(&db);
        let (c_lo, c_hi) = range(&cents);
        assert!(db_lo >= -1.5 - 1e-4 && db_hi <= 1.5 + 1e-4, "{db_lo}..{db_hi} dB");
        assert!(c_lo >= -35.0 - 1e-3 && c_hi <= 35.0 + 1e-3, "{c_lo}..{c_hi} cents");
        assert!(db_lo < -1.2 && db_hi > 1.2, "uses the range: {db_lo}..{db_hi} dB");
        assert!(c_lo < -30.0 && c_hi > 30.0, "uses the range: {c_lo}..{c_hi} cents");
        let distinct = plays.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(distinct, plays.len() - 1, "each keystroke differs from the one before");
        assert!(plays.iter().all(|p| p.pan == 0.0), "variation does not pan");
        assert_ne!(rendered(&plays[0]), rendered(&plays[1]), "and they sound different");
    }

    #[test]
    fn with_spatial_stereo_on_keys_are_panned_by_their_position() {
        let (mut voicer, mut ring, _) = voicer_with(releasing_pack(), (true, true));
        voicer.press(u16::from(code::SHIFT));
        voicer.press(u16::from(code::SPACE));
        voicer.press(u16::from(code::RETURN));
        let [shift, space, ret] = drain(&mut ring).try_into().unwrap();
        assert!(shift.pan < -0.3 && ret.pan > 0.3, "{} {}", shift.pan, ret.pan);
        assert!(space.pan.abs() < 0.02);
        let (l, r) = rendered(&shift);
        assert!(l[1] > 1.6 * r[1], "Shift sounds on the left: {} vs {}", l[1], r[1]);
        let (l, r) = rendered(&ret);
        assert!(r[1] > 1.6 * l[1], "Return sounds on the right: {} vs {}", l[1], r[1]);
    }

    #[test]
    fn with_spatial_stereo_off_every_key_is_centred() {
        let (mut voicer, mut ring, _) = voicer_with(releasing_pack(), (true, false));
        voicer.press(u16::from(code::SHIFT));
        voicer.press(u16::from(code::RETURN));
        voicer.release(u16::from(code::SHIFT));
        for play in drain(&mut ring) {
            assert_eq!(play.pan, 0.0);
            let (l, r) = rendered(&play);
            assert_eq!(l, r);
        }
    }

    #[test]
    fn a_release_sounds_only_when_enabled_and_the_pack_has_one_for_the_key() {
        let a = u16::from(code::A);
        let (mut voicer, mut ring, shared) = voicer_with(releasing_pack(), (true, false));
        voicer.release(a);
        let plays = drain(&mut ring);
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].samples, &A_UP);
        voicer.release(u16::from(code::SPACE));
        assert!(ring.pop().is_none(), "Space has no release sound in this pack");
        shared.set_release_sounds(false);
        voicer.release(a);
        assert!(ring.pop().is_none(), "Key Release Sounds off");
        shared.set_release_sounds(true);
        shared.set_enabled(false);
        voicer.release(a);
        assert!(ring.pop().is_none(), "sounds off");

        // A pack recorded without release sounds stays silent on key-up.
        let (mut voicer, mut ring, _) = voicer_with(pack(), (true, false));
        voicer.release(a);
        voicer.release(u16::from(code::SPACE));
        assert!(ring.pop().is_none());
    }

    #[test]
    fn a_release_wakes_the_output_like_a_press() {
        let (mut voicer, _ring, shared) = voicer_with(releasing_pack(), (true, false));
        voicer.release(u16::from(code::A));
        assert!(shared.last_key_press().is_some());
    }

    #[test]
    fn holding_a_key_three_seconds_gives_one_press_and_one_release() {
        use crate::keys::{KeyEvent, KeyState};
        use std::time::Duration;
        const SHIFT_DOWN: u64 = 0x0002_0000 | 0x0002 | 0x0100;
        let (j, shift) = (u16::from(code::J), u16::from(code::SHIFT));
        let held = [
            (KeyEvent::Down { keycode: j, autorepeat: false }, KeyEvent::Up { keycode: j }, j),
            (
                KeyEvent::FlagsChanged { keycode: shift, flags: SHIFT_DOWN },
                KeyEvent::FlagsChanged { keycode: shift, flags: 0x0100 },
                shift,
            ),
        ];
        for (down, up, key) in held {
            let (mut voicer, mut ring, _) = voicer_with(releasing_pack(), (true, true));
            let mut keys = KeyState::default();
            let t0 = Instant::now();
            let mut at = |ms: u64, event| {
                if let Some(t) = keys.handle(event, t0 + Duration::from_millis(ms)) {
                    voicer.key(t);
                }
            };
            at(0, down);
            for ms in (500..3_000).step_by(30) {
                // Auto-repeat (modifiers repeat as more flags-changed events).
                let repeat = match down {
                    KeyEvent::Down { keycode, .. } => KeyEvent::Down { keycode, autorepeat: true },
                    other => other,
                };
                at(ms, repeat);
            }
            at(3_000, up);
            let plays = drain(&mut ring);
            assert_eq!(plays.len(), 2, "key {key:#x}: {plays:?}");
            assert_eq!(plays[0].samples, &[1_000, 2_000, 3_000], "the press first");
            assert_eq!(plays[1].samples, &A_UP, "then the release");
        }
    }

    #[test]
    fn a_press_queues_the_keys_sound_and_stamps_the_time() {
        let (mut voicer, mut ring, shared) = voicer(true);
        let before = Instant::now();
        voicer.press(u16::from(code::SPACE));
        let play = ring.pop().expect("a voice is queued for the render callback");
        assert_eq!(play.samples, &[3, 4, 5]);
        assert_eq!(play.src_rate, 44_100);
        assert!(shared.last_key_press().is_some_and(|t| t >= before));
    }

    #[test]
    fn switching_soundpack_takes_effect_from_the_next_press_without_cutting_voices() {
        let (mut voicer, mut ring, shared) = voicer(true);
        voicer.press(u16::from(code::SPACE));
        let blue = pack_with(48_000, [vec![7], vec![8, 9]]);
        shared.set_pack(blue);
        voicer.press(u16::from(code::SPACE));
        let before = ring.pop().unwrap();
        let after = ring.pop().unwrap();
        assert_eq!(before.samples, &[3, 4, 5], "a press queued before the switch keeps its sound");
        assert_eq!((after.samples, after.src_rate), (&[8i16, 9][..], 48_000));
        assert!(std::ptr::eq(shared.pack().unwrap(), blue));
    }

    #[test]
    fn nothing_plays_without_a_soundpack() {
        let shared: &'static Shared = Box::leak(Box::new(Shared::new(true)));
        assert!(shared.pack().is_none());
    }

    #[test]
    fn nothing_plays_while_sounds_are_off() {
        let (mut voicer, mut ring, shared) = voicer(false);
        voicer.press(u16::from(code::SPACE));
        assert!(ring.pop().is_none());
        assert_eq!(shared.last_key_press(), None, "the output is not woken either");
        assert_eq!(shared.master_gain(), 0.0);
    }

    #[test]
    fn a_full_ring_drops_and_counts_instead_of_blocking() {
        let (mut voicer, _ring, shared) = voicer(true);
        for _ in 0..6 {
            voicer.press(u16::from(code::A));
        }
        assert_eq!(shared.dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn the_switch_controls_the_master_gain() {
        let shared = Shared::new(true);
        assert_eq!(shared.master_gain(), MASTER_GAIN);
        shared.set_enabled(false);
        assert_eq!(shared.master_gain(), 0.0);
    }

    #[test]
    fn only_the_first_render_after_a_clear_is_recorded() {
        let shared = Shared::new(true);
        assert_eq!(shared.first_render(), None);
        shared.note_render();
        let first = shared.first_render().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        shared.note_render();
        assert_eq!(shared.first_render(), Some(first));
        shared.clear_first_render();
        assert_eq!(shared.first_render(), None);
    }
}
