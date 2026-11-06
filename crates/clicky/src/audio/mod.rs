//! Audio output: the control thread that drives the [`lifecycle`] policy
//! against the CoreAudio [`output`] adapter.
//!
//! The control thread sleeps while cold (no wakeups, no CPU) and is woken by
//! the first key press, a device change ([`hal`] listeners) or sleep/wake
//! ([`power`]), all through [`signals`]. While warm it wakes only at the
//! hold-window deadline and at each watchdog check.

mod device;
mod ffi;
mod hal;
pub mod lifecycle;
mod output;
mod power;
mod signals;

use std::sync::atomic::{AtomicU64, Ordering, fence};
use std::time::{Duration, Instant};

use crate::diagnostics::{self, Event};
use crate::engine::mixer::Play;
use crate::engine::ring::Consumer;
use crate::engine::{Kick, Shared};
use hal::OsError;
pub use hal::allow_system_sleep;
pub use lifecycle::HOLD_WINDOW;
use lifecycle::{Command, Lifecycle, Retry, State, WATCHDOG_INTERVAL};
use output::Output;
pub use power::observe_sleep_wake;

/// How long a start waits for its first render cycle before logging without it.
const FIRST_RENDER_WAIT: Duration = Duration::from_secs(2);

/// How long to let a burst of device notifications settle before acting on a
/// warm output: connecting headphones often changes the default device, then
/// its rate. Well within the 1 s in which the sound must follow the device.
const SETTLE: Duration = Duration::from_millis(100);

/// After the output unit could not be created, the next key press retries no
/// sooner than this; the wait doubles with each failure, up to the maximum.
const UNIT_RETRY_FIRST: Duration = Duration::from_secs(1);
const UNIT_RETRY_MAX: Duration = Duration::from_secs(60);

/// Completed rebuilds, for the self-test.
static REBUILDS: AtomicU64 = AtomicU64::new(0);

/// Starts the audio control thread with the given hold window (normally
/// [`HOLD_WINDOW`]); key presses wake it through the returned [`Kick`].
pub fn spawn(shared: &'static Shared, input: Consumer<Play>, hold: Duration) -> Kick {
    let handle = std::thread::Builder::new()
        .name("clicky-audio".into())
        .spawn(move || Control::new(shared, input, hold).run())
        .expect("failed to spawn the audio control thread");
    signals::attach(handle.thread().clone());
    Kick::new(handle.thread().clone())
}

/// Asks the control thread to rebuild a warm output (the self-test).
pub fn request_rebuild() {
    signals::raise(signals::REQUEST);
}

/// How many rebuilds have completed.
pub fn rebuilds() -> u64 {
    REBUILDS.load(Ordering::SeqCst)
}

struct Control {
    shared: &'static Shared,
    output: Output,
    /// Paces new attempts to create the output unit after a failure.
    unit_retry: Retry<OsError>,
    lifecycle: Lifecycle,
    seen_key: Option<Instant>,
    /// When the running output started, for the stop log. Kept here because
    /// the lifecycle forgets it as soon as it decides to go cold.
    running_since: Option<Instant>,
}

impl Control {
    fn new(shared: &'static Shared, input: Consumer<Play>, hold: Duration) -> Control {
        let mut control = Control {
            shared,
            output: Output::new(shared, input),
            unit_retry: Retry::new(UNIT_RETRY_FIRST, UNIT_RETRY_MAX),
            lifecycle: Lifecycle::new(hold, WATCHDOG_INTERVAL),
            seen_key: None,
            running_since: None,
        };
        // Up front, since the first instantiation is slow; a failure is retried
        // by the next start.
        control.ensure_unit(Instant::now());
        control
    }

    /// Makes sure the output has its unit, creating it if the backoff allows.
    /// Logs each kind of failure once per streak. Returns whether it has one.
    fn ensure_unit(&mut self, now: Instant) -> bool {
        if self.output.has_unit() {
            return true;
        }
        if !self.unit_retry.ready(now) {
            return false;
        }
        match self.output.create_unit() {
            Ok(()) => {
                self.unit_retry.succeeded();
                true
            }
            Err(e) => {
                if self.unit_retry.failed(now, e) {
                    diagnostics::log(Event::Error {
                        context: "audio unit",
                        detail: format!("{e}; retrying on later key presses"),
                    });
                }
                false
            }
        }
    }

    fn run(mut self) -> ! {
        loop {
            let mut acted = false;
            if let Some(at) = self.shared.last_key_press()
                && self.seen_key != Some(at)
            {
                self.seen_key = Some(at);
                let command = self.lifecycle.key_pressed(at);
                acted |= self.execute(command);
            }
            let bits = signals::take();
            if bits != 0 {
                acted |= self.handle_signals(bits);
            }
            let renders = self.renders();
            let command = self.lifecycle.tick(Instant::now(), renders);
            acted |= self.execute(command);
            if acted {
                // A key press or a signal may have raced the command: look again.
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

    fn renders(&self) -> u64 {
        self.output.renders()
    }

    /// Acts on signalled sleep, wake and device changes. Returns whether a
    /// command ran.
    fn handle_signals(&mut self, mut bits: u32) -> bool {
        if bits & signals::DEVICE_BITS != 0 && self.lifecycle.state() != State::Cold {
            std::thread::sleep(SETTLE);
            bits |= signals::take();
        }
        let mut acted = false;
        if bits & signals::SLEEP != 0 {
            self.log_trigger("system will sleep");
            let command = self.lifecycle.will_sleep();
            acted |= self.execute(command);
        }
        if bits & signals::WAKE != 0 {
            self.log_trigger("system did wake");
            let command = self.lifecycle.did_wake(Instant::now());
            acted |= self.execute(command);
        }
        if let Some(trigger) = self.output.confirm(bits & signals::DEVICE_BITS) {
            self.log_trigger(trigger.as_str());
            let command = self.lifecycle.trigger(trigger, Instant::now());
            acted |= self.execute(command);
        }
        acted
    }

    fn log_trigger(&self, trigger: &'static str) {
        diagnostics::log(Event::AudioTrigger { trigger, state: self.lifecycle.state().as_str() });
    }

    /// Runs `command` and whatever the lifecycle asks next. Returns whether
    /// anything ran.
    fn execute(&mut self, command: Command) -> bool {
        let acted = command != Command::None;
        let mut command = command;
        loop {
            command = match command {
                Command::None => break,
                Command::Start(cause) => self.start(cause.as_str(), false),
                Command::Rebuild(trigger) => {
                    self.stop_output(trigger.as_str());
                    self.start(trigger.as_str(), true)
                }
                Command::Stop(reason) => {
                    self.stop_output(reason.as_str());
                    Command::None
                }
            };
        }
        self.sync_warm();
        acted
    }

    /// Starts the output; returns the lifecycle's follow-up.
    fn start(&mut self, cause: &'static str, rebuild: bool) -> Command {
        let requested = Instant::now();
        if !self.ensure_unit(requested) {
            // Already logged (once per kind); the next key press retries.
            return self.lifecycle.start_failed();
        }
        self.shared.clear_first_render();
        match self.output.start().map_err(|e| e.to_string()) {
            Ok(started) => {
                let start_ms = ms(requested.elapsed());
                self.shared.warm.store(true, Ordering::SeqCst);
                self.running_since = Some(Instant::now());
                let follow_up = self.lifecycle.started(Instant::now(), self.renders());
                let first_render = self.wait_for_first_render(requested + FIRST_RENDER_WAIT);
                if rebuild {
                    REBUILDS.fetch_add(1, Ordering::SeqCst);
                }
                diagnostics::log(Event::AudioStartedOn {
                    cause,
                    rebuild,
                    device: started.device,
                    transport: started.transport.as_str(),
                    buffer_frames: started.buffer_frames,
                    pre_roll_ms: ms(started.pre_roll),
                });
                diagnostics::log(Event::AudioStarted {
                    sample_rate: started.sample_rate,
                    start_ms,
                    first_render_ms: first_render
                        .map(|t| ms(t.saturating_duration_since(requested))),
                });
                follow_up
            }
            Err(detail) => {
                let context = if rebuild { "audio rebuild" } else { "audio start" };
                diagnostics::log(Event::Error { context, detail });
                self.lifecycle.start_failed()
            }
        }
    }

    /// Fades out and stops the output, logging why.
    fn stop_output(&mut self, reason: &'static str) {
        let warm_since = self.running_since.take();
        self.output.stop();
        diagnostics::log(Event::AudioStopped {
            reason,
            warm_secs: warm_since.map_or(0.0, |t| t.elapsed().as_secs_f64()),
            dropped: self.shared.dropped.load(Ordering::Relaxed),
        });
    }

    /// Mirrors the lifecycle into `Shared::warm`, which tells key presses
    /// whether they must wake this thread.
    fn sync_warm(&self) {
        if self.lifecycle.state() == State::Cold && self.shared.warm.load(Ordering::SeqCst) {
            self.shared.warm.store(false, Ordering::SeqCst);
            // Pairs with the fence in `Kick::kick`: either the listener sees the output
            // cold and wakes us, or we see its key press on the next loop.
            fence(Ordering::SeqCst);
        }
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
