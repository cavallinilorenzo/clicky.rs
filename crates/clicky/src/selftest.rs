//! `Clicky --self-test [--audible]`: drives the real audio path (voicer → ring
//! → lifecycle → CoreAudio output → render callback) with synthetic key presses,
//! without the key listener or the menu, so it needs no permission. It checks
//! a cold start, a rebuild while warm (as after a device change), the hold
//! window going cold, and a second cold start, and mirrors the diagnostics log
//! to stderr. Inaudible unless `--audible`.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use clicky_pack::keys::code;

use crate::diagnostics::{self, Event};
use crate::engine::{RING_CAPACITY, Shared, Voicer, ring};
use crate::{audio, packs};

/// Short, so the test sees the output go cold.
const HOLD: Duration = Duration::from_millis(800);
/// Practically silent, but the mixer still renders every voice.
const INAUDIBLE_GAIN: f32 = 1e-4;

pub fn run(audible: bool) -> Result<(), String> {
    diagnostics::set_stderr(true);
    match audio::allow_system_sleep() {
        Ok(()) => diagnostics::log(Event::SleepAllowed),
        Err(e) => return Err(e.to_string()),
    }
    let pack = packs::load_static(packs::DEFAULT_PACK)?;
    diagnostics::log(Event::SoundpackLoaded {
        id: pack.meta.id.clone(),
        clips: pack.clip_count(),
        sample_rate: pack.sample_rate,
    });
    let shared: &'static Shared = Box::leak(Box::new(Shared::new(true)));
    if !audible {
        shared.set_master_gain(INAUDIBLE_GAIN);
    }
    let (producer, consumer) = ring::ring(RING_CAPACITY);
    let kick = audio::spawn(shared, consumer, HOLD);
    let mut voicer = Voicer::new(pack, producer, shared, kick);

    for round in 1..=2 {
        let pressed = Instant::now();
        voicer.press(u16::from(code::A));
        wait_for("the output to go warm", Duration::from_secs(3), || {
            shared.warm.load(Ordering::SeqCst) && shared.first_render().is_some()
        })?;
        let first_render = shared.first_render().unwrap_or(pressed);
        println!(
            "round {round}: cold start, key press to first render {:.1} ms",
            first_render.saturating_duration_since(pressed).as_secs_f64() * 1e3
        );
        std::thread::sleep(Duration::from_millis(100));
        if round == 1 {
            // A rebuild while warm, with a press arriving while it runs.
            let rebuilds = audio::rebuilds();
            let requested = Instant::now();
            audio::request_rebuild();
            voicer.press(u16::from(code::S));
            wait_for("the warm output to be rebuilt", Duration::from_secs(3), || {
                audio::rebuilds() > rebuilds && shared.first_render().is_some()
            })?;
            let first_render = shared.first_render().unwrap_or(requested);
            println!(
                "round {round}: rebuilt while warm, request to first render {:.1} ms",
                first_render.saturating_duration_since(requested).as_secs_f64() * 1e3
            );
        }
        voicer.press(u16::from(code::SPACE));
        wait_for("the hold window to elapse", HOLD + Duration::from_secs(3), || {
            !shared.warm.load(Ordering::SeqCst)
        })?;
        println!("round {round}: cold again after the hold window");
    }
    let dropped = shared.dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        return Err(format!("{dropped} key presses were dropped"));
    }
    println!("self-test passed");
    Ok(())
}

fn wait_for(what: &str, timeout: Duration, done: impl Fn() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while !done() {
        if Instant::now() > deadline {
            return Err(format!("timed out waiting for {what}"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}
