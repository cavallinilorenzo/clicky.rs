//! The output lifecycle policy of ADR 0001, as a pure state machine: no
//! CoreAudio, no threads, no clock. The caller passes the time of every event,
//! so tests drive it with made-up instants.
//!
//! - Cold: the output is stopped and holds no device.
//! - A key press while cold asks for a start. The press itself is never lost:
//!   its voice waits in the ring and plays on the first render.
//! - Warm: the output runs. It stays warm for the hold window after the last
//!   key press, then goes cold.
//!
//! Issue #4 adds pre-roll, rebuild triggers (device, sample rate, sleep/wake)
//! and the watchdog here, as new events and states.

use std::time::{Duration, Instant};

/// How long the output stays warm after the last key press (ADR 0001: 2 minutes,
/// and always below 5 minutes).
pub const HOLD_WINDOW: Duration = Duration::from_secs(120);

/// The state of the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Stopped; holds no device.
    Cold,
    /// A start was requested and has not completed yet.
    Starting,
    /// Running; a key press is heard at once.
    Warm,
}

/// What the caller must do to the output after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Nothing.
    None,
    /// Start the output, then report `started` or `start_failed`.
    Start,
    /// Stop the output and release the device.
    Stop(StopReason),
}

/// Why the output went cold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// No key press for the whole hold window.
    HoldWindowElapsed,
}

impl StopReason {
    /// Short description for the diagnostics log.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::HoldWindowElapsed => "hold window elapsed",
        }
    }
}

#[derive(Debug)]
pub struct Lifecycle {
    hold: Duration,
    state: State,
    last_key: Option<Instant>,
    warm_since: Option<Instant>,
}

impl Lifecycle {
    pub fn new(hold: Duration) -> Lifecycle {
        Lifecycle { hold, state: State::Cold, last_key: None, warm_since: None }
    }

    #[cfg(test)]
    pub fn state(&self) -> State {
        self.state
    }

    /// A key press happened at `at`.
    pub fn key_pressed(&mut self, at: Instant) -> Command {
        self.last_key = Some(self.last_key.map_or(at, |last| last.max(at)));
        match self.state {
            State::Cold => {
                self.state = State::Starting;
                Command::Start
            }
            State::Starting | State::Warm => Command::None,
        }
    }

    /// The output started at `at`.
    pub fn started(&mut self, at: Instant) -> Command {
        if self.state == State::Starting {
            self.state = State::Warm;
            self.warm_since = Some(at);
        }
        Command::None
    }

    /// The output could not start. It stays cold; the next key press retries.
    pub fn start_failed(&mut self) -> Command {
        if self.state == State::Starting {
            self.state = State::Cold;
        }
        Command::None
    }

    /// Time passed: goes cold once the hold window has elapsed since the last key press.
    pub fn tick(&mut self, now: Instant) -> Command {
        match (self.state, self.deadline()) {
            (State::Warm, Some(deadline)) if now >= deadline => {
                self.state = State::Cold;
                self.warm_since = None;
                Command::Stop(StopReason::HoldWindowElapsed)
            }
            _ => Command::None,
        }
    }

    /// When `tick` next has something to do, if ever.
    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Warm => self.last_key.map(|last| last + self.hold),
            State::Cold | State::Starting => None,
        }
    }

    /// Since when the output has been warm.
    pub fn warm_since(&self) -> Option<Instant> {
        self.warm_since
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_secs(120);

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    #[test]
    fn starts_cold() {
        let lifecycle = Lifecycle::new(HOLD);
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.deadline(), None, "a cold output never wakes up by itself");
    }

    #[test]
    fn a_key_press_while_cold_starts_the_output() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        assert_eq!(lifecycle.key_pressed(t0), Command::Start);
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(lifecycle.started(at(t0, 0)), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
    }

    #[test]
    fn key_presses_during_a_cold_start_do_not_start_it_twice() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        lifecycle.key_pressed(t0);
        assert_eq!(lifecycle.key_pressed(at(t0, 1)), Command::None);
        lifecycle.started(at(t0, 1));
        assert_eq!(
            lifecycle.deadline(),
            Some(at(t0, 1) + HOLD),
            "the hold window counts from the last press, including presses during the start"
        );
    }

    #[test]
    fn stays_warm_through_the_hold_window_then_goes_cold() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        lifecycle.key_pressed(t0);
        lifecycle.started(t0);
        assert_eq!(lifecycle.tick(at(t0, 119)), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
        assert_eq!(lifecycle.tick(at(t0, 120)), Command::Stop(StopReason::HoldWindowElapsed));
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.tick(at(t0, 500)), Command::None, "it stops only once");
    }

    #[test]
    fn every_key_press_while_warm_extends_the_hold_window() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        lifecycle.key_pressed(t0);
        lifecycle.started(t0);
        assert_eq!(lifecycle.key_pressed(at(t0, 100)), Command::None);
        assert_eq!(lifecycle.tick(at(t0, 150)), Command::None);
        assert_eq!(lifecycle.deadline(), Some(at(t0, 220)));
        assert_eq!(lifecycle.tick(at(t0, 220)), Command::Stop(StopReason::HoldWindowElapsed));
    }

    #[test]
    fn a_key_press_after_going_cold_starts_again() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        lifecycle.key_pressed(t0);
        lifecycle.started(t0);
        lifecycle.tick(at(t0, 120));
        assert_eq!(lifecycle.key_pressed(at(t0, 3_600)), Command::Start);
    }

    #[test]
    fn a_failed_start_stays_cold_and_the_next_press_retries() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        lifecycle.key_pressed(t0);
        assert_eq!(lifecycle.start_failed(), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.tick(at(t0, 1_000)), Command::None);
        assert_eq!(lifecycle.key_pressed(at(t0, 2)), Command::Start);
    }

    #[test]
    fn an_out_of_order_press_never_shortens_the_window() {
        let t0 = Instant::now();
        let mut lifecycle = Lifecycle::new(HOLD);
        lifecycle.key_pressed(at(t0, 10));
        lifecycle.started(at(t0, 10));
        lifecycle.key_pressed(at(t0, 5));
        assert_eq!(lifecycle.deadline(), Some(at(t0, 130)));
    }

    #[test]
    fn the_shipped_hold_window_follows_adr_0001() {
        assert_eq!(HOLD_WINDOW, Duration::from_secs(120));
        assert!(HOLD_WINDOW < Duration::from_secs(300));
    }
}
