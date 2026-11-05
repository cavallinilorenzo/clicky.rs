//! Audio output: the control thread that drives the [`lifecycle`] policy
//! against the CoreAudio [`output`] adapter.
//!
//! The control thread sleeps while cold (no wakeups, no CPU) and is woken by
//! the first key press. While warm it wakes only at the hold-window deadline.

mod ffi;
pub mod lifecycle;
mod output;

use std::sync::atomic::{Ordering, fence};
use std::time::{Duration, Instant};

use crate::diagnostics::{self, Event};
use crate::engine::mixer::Play;
use crate::engine::ring::Consumer;
use crate::engine::{Kick, Shared};
pub use lifecycle::HOLD_WINDOW;
use lifecycle::{Command, Lifecycle};
use output::Output;
pub use output::allow_system_sleep;

/// How long a start waits for its first render cycle before logging without it.
const FIRST_RENDER_WAIT: Duration = Duration::from_secs(2);

/// Starts the audio control thread with the given hold window (normally
/// [`HOLD_WINDOW`]); key presses wake it through the returned [`Kick`].
pub fn spawn(shared: &'static Shared, input: Consumer<Play>, hold: Duration) -> Kick {
    let handle = std::thread::Builder::new()
        .name("clicky-audio".into())
        .spawn(move || Control::new(shared, input, hold).run())
        .expect("failed to spawn the audio control thread");
    Kick::new(handle.thread().clone())
}

struct Control {
    shared: &'static Shared,
    output: Option<Output>,
    lifecycle: Lifecycle,
    seen_key: Option<Instant>,
}

impl Control {
    fn new(shared: &'static Shared, input: Consumer<Play>, hold: Duration) -> Control {
        let output = Output::new(shared, input)
            .map_err(|e| {
                diagnostics::log(Event::Error { context: "audio unit", detail: e.to_string() })
            })
            .ok();
        Control { shared, output, lifecycle: Lifecycle::new(hold), seen_key: None }
    }

    fn run(mut self) -> ! {
        loop {
            if let Some(at) = self.shared.last_key_press()
                && self.seen_key != Some(at)
            {
                self.seen_key = Some(at);
                let command = self.lifecycle.key_pressed(at);
                self.execute(command);
            }
            let warm_since = self.lifecycle.warm_since();
            let command = self.lifecycle.tick(Instant::now());
            if let Command::Stop(reason) = command {
                self.stop(reason.as_str(), warm_since);
                // A key press may have raced the stop: look again before sleeping.
                continue;
            }
            match self.lifecycle.deadline() {
                Some(deadline) => {
                    std::thread::park_timeout(deadline.saturating_duration_since(Instant::now()))
                }
                None => std::thread::park(),
            }
        }
    }

    fn execute(&mut self, command: Command) {
        match command {
            Command::None => {}
            Command::Start => self.start(),
            Command::Stop(reason) => {
                let warm_since = self.lifecycle.warm_since();
                self.stop(reason.as_str(), warm_since);
            }
        }
    }

    fn start(&mut self) {
        let requested = Instant::now();
        self.shared.clear_first_render();
        let result = match self.output.as_mut() {
            Some(output) => output.start().map_err(|e| e.to_string()),
            None => Err("no output unit".to_owned()),
        };
        match result {
            Ok(sample_rate) => {
                let start_ms = ms(requested.elapsed());
                self.shared.warm.store(true, Ordering::SeqCst);
                self.lifecycle.started(Instant::now());
                let first_render = self.wait_for_first_render(requested + FIRST_RENDER_WAIT);
                diagnostics::log(Event::AudioStarted {
                    sample_rate,
                    start_ms,
                    first_render_ms: first_render
                        .map(|t| ms(t.saturating_duration_since(requested))),
                });
            }
            Err(detail) => {
                self.lifecycle.start_failed();
                diagnostics::log(Event::Error { context: "audio start", detail });
            }
        }
    }

    fn stop(&mut self, reason: &'static str, warm_since: Option<Instant>) {
        if let Some(output) = self.output.as_mut() {
            output.stop();
        }
        self.shared.warm.store(false, Ordering::SeqCst);
        // Pairs with the fence in `Kick::kick`: either the listener sees the output
        // cold and wakes us, or we see its key press on the next loop.
        fence(Ordering::SeqCst);
        diagnostics::log(Event::AudioStopped {
            reason,
            warm_secs: warm_since.map_or(0.0, |t| t.elapsed().as_secs_f64()),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
        });
    }

    fn wait_for_first_render(&self, deadline: Instant) -> Option<Instant> {
        loop {
            if let Some(at) = self.shared.first_render() {
                return Some(at);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}
