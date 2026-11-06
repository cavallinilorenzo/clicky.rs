//! The short preview played when a soundpack is picked: a few key presses
//! from the current soundpack, through the normal path (`Voicer` → ring →
//! mixer), so it sounds exactly like typing and wakes a cold output the same way.
//!
//! The presses come from their own thread with their own [`Voicer`] and ring
//! (see `ring::Consumer::with_producer`), so the key listener's ring keeps a
//! single producer and the main thread never waits. Picking again while a
//! preview plays restarts it.
//!
//! The rhythm ("rust ⏎") is Rustyvibes 2's preview flourish (MIT, © 2021–2026
//! Kunal Bagaria, https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/runtime.rs`.

use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use clicky_pack::keys::code;

use super::Voicer;

/// Capacity of the preview ring: enough for one preview with room to spare.
pub const PREVIEW_CAPACITY: usize = 16;

/// The keys of the preview and when each is pressed, in ms from the start.
pub const FLOURISH: [(u8, u32); 6] = [
    (code::R, 0),
    (code::U, 105),
    (code::S, 190),
    (code::T, 290),
    (code::SPACE, 400),
    (code::RETURN, 560),
];

/// Plays previews on request. Cheap to call from the main thread.
pub struct Previewer {
    requests: Sender<()>,
}

impl Previewer {
    /// Starts the preview thread. `voicer` must have its own ring.
    pub fn spawn(voicer: Voicer) -> std::io::Result<Previewer> {
        Previewer::spawn_with(voicer, &FLOURISH)
    }

    fn spawn_with(voicer: Voicer, schedule: &'static [(u8, u32)]) -> std::io::Result<Previewer> {
        let (requests, received) = mpsc::channel();
        std::thread::Builder::new()
            .name("clicky-preview".into())
            .spawn(move || run(voicer, &received, schedule))?;
        Ok(Previewer { requests })
    }

    /// Plays the preview from the current soundpack, restarting one in progress.
    pub fn play(&self) {
        // Fails only if the thread is gone; then there is nothing to preview with.
        let _ = self.requests.send(());
    }
}

static PREVIEWER: OnceLock<Previewer> = OnceLock::new();

/// Makes `previewer` the app's previewer (at launch; later calls are ignored).
pub fn install(previewer: Previewer) {
    let _ = PREVIEWER.set(previewer);
}

/// Plays the preview from the current soundpack, if a previewer is installed.
pub fn play() {
    if let Some(previewer) = PREVIEWER.get() {
        previewer.play();
    }
}

fn run(mut voicer: Voicer, requests: &Receiver<()>, schedule: &[(u8, u32)]) {
    while requests.recv().is_ok() {
        loop {
            match play_once(&mut voicer, requests, schedule) {
                Outcome::Done => break,
                Outcome::Restart => {}
                Outcome::Closed => return,
            }
        }
    }
}

enum Outcome {
    Done,
    Restart,
    Closed,
}

fn play_once(voicer: &mut Voicer, requests: &Receiver<()>, schedule: &[(u8, u32)]) -> Outcome {
    let start = Instant::now();
    for &(key, at_ms) in schedule {
        let due = start + Duration::from_millis(u64::from(at_ms));
        let wait = due.saturating_duration_since(Instant::now());
        if !wait.is_zero() {
            match requests.recv_timeout(wait) {
                Ok(()) => return Outcome::Restart,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Outcome::Closed,
            }
        }
        voicer.press(u16::from(key));
    }
    Outcome::Done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::mixer::Play;
    use crate::engine::tests::pack_with;
    use crate::engine::{Kick, Shared, ring};

    /// A previewer on its own ring, playing from a pack whose A is `[1]` and Space `[2]`.
    fn previewer(
        schedule: &'static [(u8, u32)],
    ) -> (Previewer, ring::Consumer<Play>, &'static Shared) {
        let shared: &'static Shared = Box::leak(Box::new(Shared::new(true)));
        shared.set_pack(pack_with(44_100, [vec![1], vec![2]]));
        let (producer, consumer) = ring::ring(PREVIEW_CAPACITY);
        let voicer = Voicer::sharing(producer, shared, Kick::none());
        (Previewer::spawn_with(voicer, schedule).unwrap(), consumer, shared)
    }

    fn next(ring: &mut ring::Consumer<Play>) -> (i16, Instant) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(play) = ring.pop() {
                return (play.samples[0], Instant::now());
            }
            assert!(Instant::now() < deadline, "the preview never played");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_preview_presses_its_keys_in_rhythm_from_the_current_soundpack() {
        const SCHEDULE: [(u8, u32); 2] = [(code::A, 0), (code::SPACE, 40)];
        let (preview, mut ring, shared) = previewer(&SCHEDULE);
        preview.play();
        let (first, at) = next(&mut ring);
        let (second, later) = next(&mut ring);
        assert_eq!((first, second), (1, 2));
        assert!(later - at >= Duration::from_millis(30), "{:?}", later - at);
        assert!(shared.last_key_press().is_some(), "it wakes the output like a key press");
    }

    #[test]
    fn picking_again_restarts_the_preview() {
        const SCHEDULE: [(u8, u32); 2] = [(code::A, 0), (code::SPACE, 400)];
        let (preview, mut ring, _) = previewer(&SCHEDULE);
        preview.play();
        assert_eq!(next(&mut ring).0, 1);
        preview.play();
        assert_eq!(next(&mut ring).0, 1, "the new preview starts over, before the old one ends");
        assert_eq!(next(&mut ring).0, 2);
        std::thread::sleep(Duration::from_millis(500));
        assert!(ring.pop().is_none(), "the interrupted preview does not finish");
    }

    #[test]
    fn the_preview_spells_rust_return() {
        let keys: Vec<u8> = FLOURISH.iter().map(|&(key, _)| key).collect();
        assert_eq!(keys, [code::R, code::U, code::S, code::T, code::SPACE, code::RETURN]);
        assert!(FLOURISH.windows(2).all(|w| w[0].1 < w[1].1), "in time order");
        assert!(FLOURISH.len() < PREVIEW_CAPACITY);
    }
}
