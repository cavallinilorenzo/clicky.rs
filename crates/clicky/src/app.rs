//! Wires the pieces together at launch:
//!
//! ```text
//! settings ─▶ Shared (atomics) ◀─────────────── menu (main thread)
//! input (tap thread) ─▶ Voicer ─▶ ring ─▶ render callback (CoreAudio IO thread)
//!                          └─ kick ─▶ audio control thread (lifecycle)
//! menu ─▶ preview thread ─▶ Voicer ─▶ its own ring ─┘ (same consumer)
//! ```

use crate::diagnostics::{self, Event, LOG_ENABLED};
use crate::engine::preview::{self, PREVIEW_CAPACITY, Previewer};
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
    crate::engine::mixer::restore_volume(shared, &store); // #6: the saved volume
    let (producer, consumer) = ring::ring(RING_CAPACITY);
    let (consumer, preview_producer) = consumer.with_producer(PREVIEW_CAPACITY);
    let kick = audio::spawn(shared, consumer, audio::HOLD_WINDOW);
    audio::observe_sleep_wake();

    let library = packs::library();
    let (chosen, problems) = library.load_saved(&store.get(&packs::SELECTED_PACK));
    for detail in library.problems.iter().cloned().chain(problems) {
        eprintln!("clicky: {detail}");
        diagnostics::log(Event::Error { context: "soundpack", detail });
    }
    match chosen {
        Some((_, pack)) => {
            diagnostics::log(Event::SoundpackLoaded {
                id: pack.meta.id.clone(),
                clips: pack.clip_count(),
                sample_rate: pack.sample_rate,
            });
            match Previewer::spawn(Voicer::sharing(preview_producer, shared, kick.clone())) {
                Ok(previewer) => preview::install(previewer),
                Err(e) => {
                    diagnostics::log(Event::Error { context: "preview", detail: e.to_string() })
                }
            }
            input::start(Voicer::new(pack, producer, shared, kick));
        }
        None => {
            let searched: Vec<String> =
                packs::search_dirs().iter().map(|d| d.display().to_string()).collect();
            let detail = format!("no soundpack found in {}", searched.join(", "));
            eprintln!("clicky: {detail}");
            diagnostics::log(Event::Error { context: "soundpack", detail });
        }
    }

    ui::run(shared, store);
}
