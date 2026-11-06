//! The output lifecycle policy of ADR 0001, as a pure state machine: no
//! CoreAudio, no threads, no clock. The caller passes the time of every event,
//! so tests drive it with made-up instants.
//!
//! - Cold: the output is stopped and holds no device.
//! - A key press while cold asks for a start. The press itself is never lost:
//!   its voice waits in the ring and plays on the first render (after the
//!   pre-roll on Bluetooth outputs, which the adapter applies).
//! - Warm: the output runs. It stays warm for the hold window after the last
//!   key press, then goes cold.
//! - A [`Trigger`] (device or format change, device gone, wake, watchdog)
//!   rebuilds a warm output; a cold one needs nothing, since every start reads
//!   the device's current format. A trigger during a start is remembered and
//!   acted on once the start completes.
//! - Sleep stops a warm output; wake restarts it if the hold window is still open.

use std::time::{Duration, Instant};

/// How long the output stays warm after the last key press (ADR 0001: 2 minutes,
/// and always below 5 minutes).
pub const HOLD_WINDOW: Duration = Duration::from_secs(120);

/// How often a warm output must show render progress; a full interval with no
/// render cycle rebuilds it. Longer than the slowest first render seen on
/// Bluetooth (~0.5 s).
pub const WATCHDOG_INTERVAL: Duration = Duration::from_secs(2);

/// The state of the output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Stopped; holds no device.
    Cold,
    /// A start or rebuild was requested and has not completed yet.
    Starting,
    /// Running; a key press is heard at once.
    Warm,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Cold => "cold",
            State::Starting => "starting",
            State::Warm => "warm",
        }
    }
}

/// Why a warm output must be rebuilt at the device's current format. Ordered
/// from most to least severe: when several arrive together, the first wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trigger {
    /// The device the output runs on disappeared.
    DeviceGone,
    /// The system's default output device changed.
    DefaultDeviceChanged,
    /// The device's nominal sample rate changed.
    SampleRateChanged,
    /// The device's stream format changed.
    FormatChanged,
    /// The Mac woke from sleep.
    Wake,
    /// A warm output stopped rendering.
    Watchdog,
    /// Asked for explicitly (the self-test).
    Requested,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::DeviceGone => "output device gone",
            Trigger::DefaultDeviceChanged => "default output changed",
            Trigger::SampleRateChanged => "sample rate changed",
            Trigger::FormatChanged => "stream format changed",
            Trigger::Wake => "wake from sleep",
            Trigger::Watchdog => "watchdog: no render",
            Trigger::Requested => "requested",
        }
    }
}

/// Why a cold output starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    KeyPress,
    /// A trigger: resuming after wake, or retrying a start that failed while
    /// a trigger was pending.
    Trigger(Trigger),
}

impl Cause {
    pub fn as_str(self) -> &'static str {
        match self {
            Cause::KeyPress => "key press",
            Cause::Trigger(trigger) => trigger.as_str(),
        }
    }
}

/// What the caller must do to the output after an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Nothing.
    None,
    /// Start the cold output at the device's current format, then report
    /// `started` or `start_failed`.
    Start(Cause),
    /// Fade out and stop the output, then start it again at the device's
    /// current format; report `started` or `start_failed`.
    Rebuild(Trigger),
    /// Fade out, stop the output and release the device.
    Stop(StopReason),
}

/// Why the output went cold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// No key press for the whole hold window.
    HoldWindowElapsed,
    /// The Mac is going to sleep.
    Sleep,
}

impl StopReason {
    /// Short description for the diagnostics log.
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::HoldWindowElapsed => "hold window elapsed",
            StopReason::Sleep => "system sleep",
        }
    }
}

/// Something that arrived while a start was in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pending {
    Rebuild(Trigger),
    Sleep,
}

/// The watchdog's last look at a warm output.
#[derive(Clone, Copy, Debug)]
struct Watch {
    next: Instant,
    renders: u64,
}

#[derive(Debug)]
pub struct Lifecycle {
    hold: Duration,
    watchdog: Duration,
    state: State,
    last_key: Option<Instant>,
    pending: Option<Pending>,
    resume_on_wake: bool,
    watch: Option<Watch>,
}

impl Lifecycle {
    pub fn new(hold: Duration, watchdog: Duration) -> Lifecycle {
        Lifecycle {
            hold,
            watchdog,
            state: State::Cold,
            last_key: None,
            pending: None,
            resume_on_wake: false,
            watch: None,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// A key press happened at `at`.
    pub fn key_pressed(&mut self, at: Instant) -> Command {
        self.last_key = Some(self.last_key.map_or(at, |last| last.max(at)));
        match self.state {
            State::Cold => {
                self.resume_on_wake = false;
                self.state = State::Starting;
                Command::Start(Cause::KeyPress)
            }
            State::Starting | State::Warm => Command::None,
        }
    }

    /// The output started (or was rebuilt) at `at`; `renders` is the render
    /// cycle count at that moment.
    pub fn started(&mut self, at: Instant, renders: u64) -> Command {
        if self.state != State::Starting {
            return Command::None;
        }
        match self.pending.take() {
            Some(Pending::Rebuild(trigger)) => Command::Rebuild(trigger),
            Some(Pending::Sleep) => self.go_cold_for_sleep(),
            None => {
                self.state = State::Warm;
                self.watch = Some(Watch { next: at + self.watchdog, renders });
                Command::None
            }
        }
    }

    /// The output could not start. It goes cold and the next key press retries,
    /// unless a trigger arrived meanwhile: then the device may have changed, so
    /// it retries once at once.
    pub fn start_failed(&mut self) -> Command {
        if self.state != State::Starting {
            return Command::None;
        }
        self.state = State::Cold;
        self.watch = None;
        match self.pending.take() {
            Some(Pending::Rebuild(trigger)) => {
                self.state = State::Starting;
                Command::Start(Cause::Trigger(trigger))
            }
            Some(Pending::Sleep) | None => Command::None,
        }
    }

    /// Something happened that needs a warm output rebuilt.
    pub fn trigger(&mut self, trigger: Trigger, at: Instant) -> Command {
        match self.state {
            // The next start reads the device's current format anyway.
            State::Cold => Command::None,
            State::Starting => {
                self.pending = Some(match self.pending {
                    Some(Pending::Rebuild(earlier)) => Pending::Rebuild(earlier.min(trigger)),
                    Some(Pending::Sleep) => Pending::Sleep,
                    None => Pending::Rebuild(trigger),
                });
                Command::None
            }
            State::Warm if self.hold_elapsed(at) => self.go_cold(StopReason::HoldWindowElapsed),
            State::Warm => {
                self.state = State::Starting;
                self.watch = None;
                Command::Rebuild(trigger)
            }
        }
    }

    /// The Mac is about to sleep.
    pub fn will_sleep(&mut self) -> Command {
        match self.state {
            State::Cold => {
                self.resume_on_wake = false;
                Command::None
            }
            State::Starting => {
                self.pending = Some(Pending::Sleep);
                Command::None
            }
            State::Warm => self.go_cold_for_sleep(),
        }
    }

    /// The Mac woke up at `at`. A warm output (if it missed the sleep) is
    /// rebuilt; one stopped for sleep restarts if its hold window is still open.
    pub fn did_wake(&mut self, at: Instant) -> Command {
        match self.state {
            State::Cold => {
                let resume = std::mem::take(&mut self.resume_on_wake) && !self.hold_elapsed(at);
                if resume {
                    self.state = State::Starting;
                    Command::Start(Cause::Trigger(Trigger::Wake))
                } else {
                    Command::None
                }
            }
            State::Starting => {
                // Overrides a pending sleep: the Mac is awake again.
                self.pending = Some(match self.pending {
                    Some(Pending::Rebuild(earlier)) => Pending::Rebuild(earlier.min(Trigger::Wake)),
                    Some(Pending::Sleep) | None => Pending::Rebuild(Trigger::Wake),
                });
                Command::None
            }
            State::Warm => self.trigger(Trigger::Wake, at),
        }
    }

    /// Time passed: goes cold once the hold window has elapsed since the last
    /// key press; rebuilds a warm output whose render count (`renders`) did not
    /// move for a whole watchdog interval.
    pub fn tick(&mut self, now: Instant, renders: u64) -> Command {
        if self.state != State::Warm {
            return Command::None;
        }
        if self.hold_elapsed(now) {
            return self.go_cold(StopReason::HoldWindowElapsed);
        }
        match self.watch {
            Some(watch) if now >= watch.next && renders == watch.renders => {
                self.trigger(Trigger::Watchdog, now)
            }
            Some(watch) if now >= watch.next => {
                self.watch = Some(Watch { next: now + self.watchdog, renders });
                Command::None
            }
            _ => Command::None,
        }
    }

    /// When `tick` next has something to do, if ever.
    pub fn deadline(&self) -> Option<Instant> {
        match self.state {
            State::Warm => {
                let hold = self.last_key.map(|last| last + self.hold);
                let watch = self.watch.map(|watch| watch.next);
                hold.into_iter().chain(watch).min()
            }
            State::Cold | State::Starting => None,
        }
    }

    fn hold_elapsed(&self, at: Instant) -> bool {
        self.last_key.is_none_or(|last| at >= last + self.hold)
    }

    fn go_cold(&mut self, reason: StopReason) -> Command {
        self.state = State::Cold;
        self.watch = None;
        Command::Stop(reason)
    }

    fn go_cold_for_sleep(&mut self) -> Command {
        self.resume_on_wake = true;
        self.go_cold(StopReason::Sleep)
    }
}

/// When to try again a step that keeps failing (creating the output unit),
/// and which failures are news. Each attempt is made only when a start is
/// asked for anyway (a key press), never by a timer, and no sooner than the
/// backoff allows: the wait doubles after each failure, up to a maximum.
#[derive(Debug)]
pub struct Retry<K> {
    first: Duration,
    max: Duration,
    delay: Duration,
    next: Option<Instant>,
    /// The kinds of failure seen since the last success.
    seen: Vec<K>,
}

impl<K: PartialEq> Retry<K> {
    pub fn new(first: Duration, max: Duration) -> Retry<K> {
        Retry { first, max, delay: first, next: None, seen: Vec::new() }
    }

    /// Whether an attempt at `now` may run.
    pub fn ready(&self, now: Instant) -> bool {
        self.next.is_none_or(|next| now >= next)
    }

    /// The attempt at `now` failed with `kind`. Returns whether this kind is
    /// new since the last success, that is, whether to log it.
    pub fn failed(&mut self, now: Instant, kind: K) -> bool {
        self.next = Some(now + self.delay);
        self.delay = (self.delay * 2).min(self.max);
        if self.seen.contains(&kind) {
            return false;
        }
        self.seen.push(kind);
        true
    }

    /// The attempt succeeded: the next failure starts over.
    pub fn succeeded(&mut self) {
        self.delay = self.first;
        self.next = None;
        self.seen.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOLD: Duration = Duration::from_secs(120);
    const WATCHDOG: Duration = Duration::from_secs(2);

    fn at(t0: Instant, secs: u64) -> Instant {
        t0 + Duration::from_secs(secs)
    }

    fn lifecycle() -> Lifecycle {
        Lifecycle::new(HOLD, WATCHDOG)
    }

    /// A lifecycle that went warm at `t0` after a key press at `t0`.
    fn warm(t0: Instant) -> Lifecycle {
        let mut lifecycle = lifecycle();
        assert_eq!(lifecycle.key_pressed(t0), Command::Start(Cause::KeyPress));
        assert_eq!(lifecycle.started(t0, 0), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
        lifecycle
    }

    // Retrying a step that failed (creating the output unit).

    #[test]
    fn a_failing_step_is_retried_on_later_attempts_with_backoff() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut retry = Retry::new(Duration::from_secs(1), Duration::from_secs(4));
        assert!(retry.ready(t0), "the first attempt runs");
        assert!(retry.failed(t0, "error A"), "the first failure is logged");
        assert!(!retry.ready(ms(999)), "too soon");
        assert!(retry.ready(ms(1_000)));
        assert!(!retry.failed(ms(1_000), "error A"), "the same failure is not logged again");
        assert!(!retry.ready(ms(2_999)), "the wait doubles");
        assert!(retry.ready(ms(3_000)));
        assert!(retry.failed(ms(3_000), "error B"), "a new kind of failure is logged once");
        assert!(!retry.failed(ms(7_000), "error B"));
        assert!(!retry.ready(ms(10_999)), "capped at the maximum");
        assert!(retry.ready(ms(11_000)));
        assert!(!retry.failed(ms(11_000), "error A"));
    }

    #[test]
    fn a_success_resets_the_retry() {
        let t0 = Instant::now();
        let mut retry = Retry::new(Duration::from_secs(1), Duration::from_secs(60));
        retry.failed(t0, "error A");
        retry.failed(at(t0, 1), "error A");
        retry.succeeded();
        assert!(retry.ready(at(t0, 1)), "no wait after a success");
        assert!(retry.failed(at(t0, 2), "error A"), "a failure after a success is news");
        assert!(retry.ready(at(t0, 3)), "and waits the first delay again");
    }

    // Cold start and hold window.

    #[test]
    fn starts_cold() {
        let lifecycle = lifecycle();
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.deadline(), None, "a cold output never wakes up by itself");
    }

    #[test]
    fn a_key_press_while_cold_starts_the_output() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        assert_eq!(lifecycle.key_pressed(t0), Command::Start(Cause::KeyPress));
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(lifecycle.deadline(), None, "nothing to time while starting");
        assert_eq!(lifecycle.started(at(t0, 0), 0), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
    }

    #[test]
    fn key_presses_during_a_cold_start_do_not_start_it_twice() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        assert_eq!(lifecycle.key_pressed(at(t0, 1)), Command::None);
        assert_eq!(lifecycle.state(), State::Starting);
        lifecycle.started(at(t0, 1), 0);
        assert_eq!(
            lifecycle.deadline(),
            Some(at(t0, 1) + WATCHDOG),
            "the watchdog looks first; the hold window counts from the last press"
        );
        assert_eq!(lifecycle.tick(at(t0, 120), 9), Command::None, "the press at 1 s holds it");
        assert_eq!(lifecycle.tick(at(t0, 121), 9), Command::Stop(StopReason::HoldWindowElapsed));
    }

    #[test]
    fn stays_warm_through_the_hold_window_then_goes_cold() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.tick(at(t0, 119), 1), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
        assert_eq!(lifecycle.tick(at(t0, 120), 2), Command::Stop(StopReason::HoldWindowElapsed));
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.tick(at(t0, 500), 2), Command::None, "it stops only once");
        assert_eq!(lifecycle.deadline(), None);
    }

    #[test]
    fn every_key_press_while_warm_extends_the_hold_window() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.key_pressed(at(t0, 100)), Command::None);
        assert_eq!(lifecycle.tick(at(t0, 150), 1), Command::None);
        assert_eq!(lifecycle.tick(at(t0, 220), 2), Command::Stop(StopReason::HoldWindowElapsed));
    }

    #[test]
    fn a_key_press_after_going_cold_starts_again() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.tick(at(t0, 120), 1);
        assert_eq!(lifecycle.key_pressed(at(t0, 3_600)), Command::Start(Cause::KeyPress));
    }

    #[test]
    fn a_failed_start_stays_cold_and_the_next_press_retries() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        assert_eq!(lifecycle.start_failed(), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.tick(at(t0, 1_000), 0), Command::None);
        assert_eq!(lifecycle.key_pressed(at(t0, 2)), Command::Start(Cause::KeyPress));
    }

    #[test]
    fn an_out_of_order_press_never_shortens_the_window() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(at(t0, 10));
        lifecycle.started(at(t0, 10), 0);
        lifecycle.key_pressed(at(t0, 5));
        assert_eq!(lifecycle.tick(at(t0, 129), 1), Command::None);
        assert_eq!(lifecycle.tick(at(t0, 130), 1), Command::Stop(StopReason::HoldWindowElapsed));
    }

    #[test]
    fn reports_out_of_turn_are_ignored() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        assert_eq!(lifecycle.started(t0, 0), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.start_failed(), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
    }

    #[test]
    fn the_shipped_hold_window_follows_adr_0001() {
        assert_eq!(HOLD_WINDOW, Duration::from_secs(120));
        assert!(HOLD_WINDOW < Duration::from_secs(300));
    }

    // Rebuild triggers.

    const DEVICE_TRIGGERS: [Trigger; 4] = [
        Trigger::DeviceGone,
        Trigger::DefaultDeviceChanged,
        Trigger::SampleRateChanged,
        Trigger::FormatChanged,
    ];

    #[test]
    fn a_trigger_while_warm_rebuilds_the_output() {
        for trigger in DEVICE_TRIGGERS {
            let t0 = Instant::now();
            let mut lifecycle = warm(t0);
            assert_eq!(lifecycle.trigger(trigger, at(t0, 5)), Command::Rebuild(trigger));
            assert_eq!(lifecycle.state(), State::Starting);
            assert_eq!(lifecycle.started(at(t0, 6), 40), Command::None);
            assert_eq!(lifecycle.state(), State::Warm, "{trigger:?}");
        }
    }

    #[test]
    fn a_trigger_while_cold_needs_nothing() {
        for trigger in DEVICE_TRIGGERS {
            let t0 = Instant::now();
            let mut lifecycle = lifecycle();
            assert_eq!(lifecycle.trigger(trigger, t0), Command::None);
            assert_eq!(lifecycle.state(), State::Cold);
            assert_eq!(lifecycle.key_pressed(at(t0, 1)), Command::Start(Cause::KeyPress));
        }
    }

    #[test]
    fn a_trigger_during_a_start_rebuilds_once_the_start_completes() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        assert_eq!(lifecycle.trigger(Trigger::DefaultDeviceChanged, t0), Command::None);
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(
            lifecycle.started(at(t0, 1), 0),
            Command::Rebuild(Trigger::DefaultDeviceChanged),
            "the start may have bound the old device"
        );
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(lifecycle.started(at(t0, 2), 0), Command::None, "only one rebuild");
        assert_eq!(lifecycle.state(), State::Warm);
    }

    #[test]
    fn a_trigger_during_a_failed_start_retries_the_start_once() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        lifecycle.trigger(Trigger::DeviceGone, t0);
        assert_eq!(lifecycle.start_failed(), Command::Start(Cause::Trigger(Trigger::DeviceGone)));
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(lifecycle.start_failed(), Command::None, "no retry loop");
        assert_eq!(lifecycle.state(), State::Cold);
    }

    #[test]
    fn several_triggers_during_a_start_keep_the_most_severe() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        lifecycle.trigger(Trigger::SampleRateChanged, t0);
        lifecycle.trigger(Trigger::DeviceGone, t0);
        lifecycle.trigger(Trigger::FormatChanged, t0);
        assert_eq!(lifecycle.started(t0, 0), Command::Rebuild(Trigger::DeviceGone));
    }

    #[test]
    fn key_presses_during_a_rebuild_are_kept_and_extend_the_window() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.trigger(Trigger::SampleRateChanged, at(t0, 100));
        assert_eq!(lifecycle.key_pressed(at(t0, 101)), Command::None, "no second start");
        lifecycle.started(at(t0, 101), 0);
        assert_eq!(lifecycle.tick(at(t0, 220), 1), Command::None);
        assert_eq!(lifecycle.tick(at(t0, 221), 2), Command::Stop(StopReason::HoldWindowElapsed));
    }

    #[test]
    fn a_failed_rebuild_goes_cold_and_the_next_press_retries() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.trigger(Trigger::DeviceGone, at(t0, 3));
        assert_eq!(lifecycle.start_failed(), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.deadline(), None);
        assert_eq!(lifecycle.key_pressed(at(t0, 4)), Command::Start(Cause::KeyPress));
    }

    #[test]
    fn a_trigger_after_the_hold_window_stops_instead_of_rebuilding() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(
            lifecycle.trigger(Trigger::DefaultDeviceChanged, at(t0, 130)),
            Command::Stop(StopReason::HoldWindowElapsed)
        );
        assert_eq!(lifecycle.state(), State::Cold);
    }

    // Watchdog.

    #[test]
    fn a_warm_output_that_keeps_rendering_is_left_alone() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.deadline(), Some(at(t0, 2)), "first look one interval after start");
        assert_eq!(lifecycle.tick(at(t0, 2), 100), Command::None);
        assert_eq!(lifecycle.deadline(), Some(at(t0, 4)));
        assert_eq!(lifecycle.tick(at(t0, 4), 200), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
    }

    #[test]
    fn a_warm_output_that_stops_rendering_is_rebuilt() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.tick(at(t0, 2), 100), Command::None);
        assert_eq!(lifecycle.tick(at(t0, 3), 100), Command::None, "not a full interval yet");
        assert_eq!(lifecycle.tick(at(t0, 4), 100), Command::Rebuild(Trigger::Watchdog));
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(lifecycle.deadline(), None);
        assert_eq!(lifecycle.started(at(t0, 5), 100), Command::None);
        assert_eq!(lifecycle.deadline(), Some(at(t0, 7)), "a fresh interval after the rebuild");
    }

    #[test]
    fn an_output_that_never_renders_after_its_start_is_rebuilt() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        lifecycle.started(t0, 7);
        assert_eq!(lifecycle.tick(at(t0, 2), 7), Command::Rebuild(Trigger::Watchdog));
    }

    #[test]
    fn the_hold_window_wins_over_the_watchdog() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.tick(at(t0, 118), 5);
        assert_eq!(lifecycle.deadline(), Some(at(t0, 120)), "the earlier of the two deadlines");
        assert_eq!(lifecycle.tick(at(t0, 120), 5), Command::Stop(StopReason::HoldWindowElapsed));
    }

    #[test]
    fn the_watchdog_waits_longer_than_a_slow_bluetooth_first_render() {
        assert!(WATCHDOG_INTERVAL >= Duration::from_secs(1));
        assert!(WATCHDOG_INTERVAL < HOLD_WINDOW);
    }

    // Sleep and wake.

    #[test]
    fn sleep_stops_a_warm_output_and_wake_restarts_it() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.will_sleep(), Command::Stop(StopReason::Sleep));
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.deadline(), None);
        assert_eq!(lifecycle.did_wake(at(t0, 11)), Command::Start(Cause::Trigger(Trigger::Wake)));
        assert_eq!(lifecycle.state(), State::Starting);
        assert_eq!(lifecycle.started(at(t0, 12), 0), Command::None);
        assert_eq!(lifecycle.state(), State::Warm);
        assert_eq!(
            lifecycle.tick(at(t0, 120), 9),
            Command::Stop(StopReason::HoldWindowElapsed),
            "the hold window still counts from the last key press"
        );
    }

    #[test]
    fn wake_leaves_an_output_cold_if_it_was_cold_at_sleep() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        assert_eq!(lifecycle.will_sleep(), Command::None);
        assert_eq!(lifecycle.did_wake(at(t0, 600)), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
    }

    #[test]
    fn wake_after_the_hold_window_stays_cold() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.will_sleep();
        assert_eq!(lifecycle.did_wake(at(t0, 121)), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.key_pressed(at(t0, 122)), Command::Start(Cause::KeyPress));
    }

    #[test]
    fn wake_resumes_only_once() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.will_sleep();
        lifecycle.did_wake(at(t0, 2));
        lifecycle.started(at(t0, 2), 0);
        lifecycle.will_sleep();
        lifecycle.did_wake(at(t0, 4));
        lifecycle.start_failed();
        assert_eq!(lifecycle.did_wake(at(t0, 5)), Command::None, "a second wake without sleep");
    }

    #[test]
    fn a_key_press_while_asleep_starts_as_usual_and_wake_then_rebuilds() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        lifecycle.will_sleep();
        assert_eq!(lifecycle.key_pressed(at(t0, 2)), Command::Start(Cause::KeyPress));
        lifecycle.started(at(t0, 2), 0);
        assert_eq!(lifecycle.did_wake(at(t0, 3)), Command::Rebuild(Trigger::Wake));
    }

    #[test]
    fn wake_while_warm_rebuilds_the_output() {
        let t0 = Instant::now();
        let mut lifecycle = warm(t0);
        assert_eq!(lifecycle.did_wake(at(t0, 1)), Command::Rebuild(Trigger::Wake));
    }

    #[test]
    fn sleep_during_a_start_stops_once_the_start_completes() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        assert_eq!(lifecycle.will_sleep(), Command::None);
        assert_eq!(lifecycle.trigger(Trigger::FormatChanged, t0), Command::None);
        assert_eq!(lifecycle.started(at(t0, 1), 0), Command::Stop(StopReason::Sleep));
        assert_eq!(lifecycle.state(), State::Cold);
        assert_eq!(lifecycle.did_wake(at(t0, 2)), Command::Start(Cause::Trigger(Trigger::Wake)));
    }

    #[test]
    fn sleep_during_a_start_that_fails_stays_cold() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        lifecycle.will_sleep();
        assert_eq!(lifecycle.start_failed(), Command::None);
        assert_eq!(lifecycle.state(), State::Cold);
    }

    #[test]
    fn sleep_and_wake_during_a_start_rebuild_once_it_completes() {
        let t0 = Instant::now();
        let mut lifecycle = lifecycle();
        lifecycle.key_pressed(t0);
        lifecycle.will_sleep();
        assert_eq!(lifecycle.did_wake(at(t0, 1)), Command::None);
        assert_eq!(lifecycle.started(at(t0, 1), 0), Command::Rebuild(Trigger::Wake));
    }

    #[test]
    fn triggers_are_ordered_most_severe_first() {
        let mut triggers = [
            Trigger::Requested,
            Trigger::Watchdog,
            Trigger::FormatChanged,
            Trigger::Wake,
            Trigger::SampleRateChanged,
            Trigger::DefaultDeviceChanged,
            Trigger::DeviceGone,
        ];
        triggers.sort();
        assert_eq!(triggers[0], Trigger::DeviceGone);
        assert_eq!(triggers[1], Trigger::DefaultDeviceChanged);
        assert_eq!(triggers[6], Trigger::Requested);
    }
}
