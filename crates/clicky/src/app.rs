//! Wires the pieces together at launch:
//!
//! ```text
//! settings ─▶ Shared (atomics) ◀─────────────── menu (main thread)
//! input (tap thread) ─▶ Voicer ─▶ ring ─▶ render callback (CoreAudio IO thread)
//!                          └─ kick ─▶ audio control thread (lifecycle)
//! ```

use crate::diagnostics::{self, Event, LOG_ENABLED};
use crate::engine::{RING_CAPACITY, SOUNDS_ON, Shared, Voicer, ring};
use crate::settings::Store;
use crate::{audio, input, packs, ui};

/// Starts Clicky. Does not return.
pub fn run() {
    let store = Store::standard();
    if store.get(&LOG_ENABLED) {
        diagnostics::set_enabled(true);
    }
    diagnostics::log(Event::Launched { version: env!("CARGO_PKG_VERSION") });

    match audio::allow_system_sleep() {
        Ok(()) => diagnostics::log(Event::SleepAllowed),
        Err(e) => diagnostics::log(Event::Error { context: "sleep policy", detail: e.to_string() }),
    }

    let shared: &'static Shared = Box::leak(Box::new(Shared::new(store.get(&SOUNDS_ON))));
    let (producer, consumer) = ring::ring(RING_CAPACITY);
    let kick = audio::spawn(shared, consumer, audio::HOLD_WINDOW);

    match packs::load_static(packs::DEFAULT_PACK) {
        Ok(pack) => {
            diagnostics::log(Event::SoundpackLoaded {
                id: pack.meta.id.clone(),
                clips: pack.clip_count(),
                sample_rate: pack.sample_rate,
            });
            input::start(Voicer::new(pack, producer, shared, kick));
        }
        Err(detail) => {
            eprintln!("clicky: {detail}");
            diagnostics::log(Event::Error { context: "soundpack", detail });
        }
    }

    ui::run(shared, store);
}
