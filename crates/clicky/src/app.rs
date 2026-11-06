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

    let shared: &'static Shared = Box::leak(Box::new(restored(&store)));
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
    let voicer = match chosen {
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
            Voicer::new(pack, producer, shared, kick)
        }
        None => {
            let searched: Vec<String> =
                packs::search_dirs().iter().map(|d| d.display().to_string()).collect();
            let detail = format!("no soundpack found in {}", searched.join(", "));
            eprintln!("clicky: {detail}");
            diagnostics::log(Event::Error { context: "soundpack", detail });
            // No current pack, so it plays nothing.
            Voicer::sharing(producer, shared, kick)
        }
    };
    // Even without a soundpack: the listener also supervises the permission,
    // which the menu and the first-run window follow (never stuck at Checking).
    input::start(voicer);

    ui::run(shared, store);
}

/// The engine state the saved settings ask for at launch: on/off, volume
/// and the sound options. (The soundpack is restored by `Library::load_saved`,
/// the diagnostics log above.)
fn restored(store: &Store) -> Shared {
    let shared = Shared::new(store.get(&SOUNDS_ON));
    crate::engine::mixer::restore_volume(&shared, store); // #6: the saved volume
    crate::engine::options::restore(&shared, store); // #8: the saved sound options
    shared
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::engine::mixer::{VOLUME, volume_gain};
    use crate::settings::UserDefaults;
    use crate::ui::menu::SoundOption;

    /// Set in the child process that plays the session before Quit.
    const SUITE_ENV: &str = "CLICKY_TEST_SETTINGS_SUITE";
    const NAME: &str = "app::tests::every_setting_is_as_left_after_quit_and_relaunch";

    /// Every setting the menu writes, saved by one process through
    /// `NSUserDefaults` and read back at launch by another: on/off, soundpack,
    /// volume, the three sound options and the diagnostics log.
    #[test]
    fn every_setting_is_as_left_after_quit_and_relaunch() {
        if let Some(suite) = std::env::var_os(SUITE_ENV) {
            // The session before Quit: change everything from its default, as the menu does.
            let store = Store::with_backend(UserDefaults::suite(suite.to_str().unwrap()));
            let shared = restored(&store);
            store.set(&SOUNDS_ON, &false);
            store.set(&packs::SELECTED_PACK, &"holy-panda".to_owned());
            store.set(&VOLUME, &0.3);
            for option in SoundOption::ALL {
                option.set(false, &shared, &store);
            }
            store.set(&LOG_ENABLED, &true);
            return;
        }
        let suite =
            format!("io.github.cavallinilorenzo.clicky.tests.relaunch.{}", std::process::id());
        let quit = Command::new(std::env::current_exe().unwrap())
            .args([NAME, "--exact", "--test-threads=1"])
            .env(SUITE_ENV, &suite)
            .output()
            .unwrap();
        let child = String::from_utf8_lossy(&quit.stdout);
        assert!(quit.status.success() && child.contains("1 passed"), "{child}");

        // The relaunch: this process has never opened the suite.
        let store = Store::with_backend(UserDefaults::suite(&suite));
        let shared = restored(&store);
        let (sounds_on, pack, log) =
            (shared.enabled(), store.get(&packs::SELECTED_PACK), store.get(&LOG_ENABLED));
        shared.set_enabled(true);
        let gain = shared.master_gain();
        let options: Vec<bool> = SoundOption::ALL.iter().map(|o| o.is_on(&shared)).collect();
        UserDefaults::remove_suite(&suite);

        assert!(!sounds_on, "on/off");
        assert_eq!(pack, "holy-panda");
        assert_eq!(gain, volume_gain(0.3), "volume");
        assert_eq!(options, [false; 3], "Key Release Sounds, Natural Variation, Spatial Stereo");
        assert!(log, "diagnostics log");
    }

    #[test]
    fn a_first_launch_uses_the_defaults() {
        let store = Store::in_memory();
        let shared = restored(&store);
        assert!(shared.enabled());
        assert_eq!(shared.master_gain(), volume_gain(1.0));
        assert!(SoundOption::ALL.iter().all(|o| o.is_on(&shared)));
    }
}
