//! The sound engine's shared state and the key-listener side of the
//! lock-free path:
//!
//! ```text
//! tap thread: Voicer::press ──Play──▶ SPSC ring ──▶ render callback: Mixer
//!                  └─ stamps the key time and wakes the audio control thread
//! ```
//!
//! Everything both sides read lives in atomics in [`Shared`], so the render
//! callback never takes a lock. That includes the current soundpack: the menu
//! swaps it with one atomic store, the next key press plays from the new pack,
//! and voices already playing finish from the old one (packs are never freed).

pub mod mixer;
pub mod preview;
pub mod ring;

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering, fence};
use std::thread::Thread;
use std::time::Instant;

use clicky_pack::Pack;

use crate::keys;
use crate::settings::Setting;
use mixer::Play;
use ring::Producer;

/// The on/off switch.
pub const SOUNDS_ON: Setting<bool> = Setting { key: "SoundsOn", default: true };

/// Master gain while sounds are on. Leaves headroom over the soundpack's
/// −1 dBFS clip peaks; issue #6 replaces it with the volume slider.
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
        }
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

    /// Call after stamping the key press.
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

/// The key listener's end of the path: turns a key press into a voice request.
/// Runs on the tap thread: no locks, no allocation. It plays from whichever
/// soundpack [`Shared::pack`] holds at the time of the press.
pub struct Voicer {
    producer: Producer<Play>,
    shared: &'static Shared,
    kick: Kick,
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
        Voicer { producer, shared, kick }
    }

    /// Another voicer playing from the same current soundpack, for a second
    /// thread (the preview). `producer` must feed its own ring.
    pub fn sharing(producer: Producer<Play>, shared: &'static Shared, kick: Kick) -> Voicer {
        Voicer { producer, shared, kick }
    }

    /// Plays the press sound of `keycode` from the current soundpack, unless sounds are off.
    pub fn press(&mut self, keycode: u16) {
        if !self.shared.enabled() {
            return;
        }
        let Some(pack) = self.shared.pack() else { return };
        let Some(samples) = keys::press_clip(pack, keycode).and_then(|c| pack.clip(c)) else {
            return;
        };
        let play = Play { samples, src_rate: pack.sample_rate, gain: 1.0 };
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
