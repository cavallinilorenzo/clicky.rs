//! The Input Monitoring permission: checking it, asking for it, and the
//! [`Supervisor`] that decides, from what the tap thread observes, whether the
//! event tap should be created or dropped and what the UI should show.
//!
//! The tap thread publishes the current [`Status`] in an atomic; the UI reads
//! it with [`status`] and is told about changes through [`observe`].

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// What macOS says about Input Monitoring for this process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Granted,
    Denied,
    /// Never asked (first run, or after `tccutil reset ListenEvent`).
    Unknown,
}

/// What the key listener can do, as the UI shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Before the first check at launch.
    Checking,
    /// No permission since launch (first run, or denied before).
    Missing,
    /// The tap runs: key presses make sounds.
    Listening,
    /// The permission went away while Clicky was listening.
    Revoked,
    /// The permission was granted while running, but macOS will not let this
    /// process create the tap until it is relaunched.
    NeedsRelaunch,
}

impl Status {
    /// Whether sounds are unavailable for lack of permission.
    pub fn needs_permission(self) -> bool {
        matches!(self, Status::Missing | Status::Revoked | Status::NeedsRelaunch)
    }
}

/// What the tap thread should do after a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Nothing to change.
    Wait,
    /// Try to create (and enable) the tap, then report with
    /// [`Supervisor::created`] or [`Supervisor::create_failed`].
    Create,
    /// Tear the tap down (permission gone, or the tap died).
    Drop,
}

/// Consecutive failed tap creations, with the permission granted after
/// launch, before Clicky asks to be relaunched.
pub const RELAUNCH_AFTER_FAILURES: u32 = 2;

/// The permission state machine. Pure: the caller checks the permission and
/// the tap and performs the returned [`Action`].
#[derive(Debug)]
pub struct Supervisor {
    status: Status,
    /// The tap ran at some point since launch.
    listened: bool,
    /// The permission was missing at some point since launch, so a grant
    /// happened while running (which may need a relaunch).
    saw_missing: bool,
    failures: u32,
}

impl Default for Supervisor {
    fn default() -> Self {
        Supervisor { status: Status::Checking, listened: false, saw_missing: false, failures: 0 }
    }
}

impl Supervisor {
    pub fn status(&self) -> Status {
        self.status
    }

    /// One check. `tap` is `None` when there is no tap, else whether it is enabled.
    pub fn tick(&mut self, access: Access, tap: Option<bool>) -> Action {
        match (access, tap) {
            (Access::Granted, Some(true)) => {
                self.status = Status::Listening;
                Action::Wait
            }
            // The tap died with the permission still there: recreate it.
            (Access::Granted, Some(false)) => Action::Drop,
            (Access::Granted, None) => Action::Create,
            (Access::Denied | Access::Unknown, tap) => {
                self.saw_missing = true;
                self.failures = 0;
                self.status = if self.listened { Status::Revoked } else { Status::Missing };
                if tap.is_some() { Action::Drop } else { Action::Wait }
            }
        }
    }

    /// The tap was created and enabled.
    pub fn created(&mut self) {
        self.listened = true;
        self.failures = 0;
        self.status = Status::Listening;
    }

    /// Creating the tap failed although the permission is granted.
    pub fn create_failed(&mut self) {
        self.failures += 1;
        if self.saw_missing && self.failures >= RELAUNCH_AFTER_FAILURES {
            self.status = Status::NeedsRelaunch;
        } else if self.status == Status::Checking {
            // Granted at launch yet refused: a stale entry (e.g. an older
            // build's signature). Toggling Clicky in System Settings fixes it.
            self.status = Status::Missing;
        }
    }
}

// --- Published status (tap thread → UI) ---

static STATUS: AtomicU8 = AtomicU8::new(0);
static OBSERVER: OnceLock<fn()> = OnceLock::new();

fn encode(status: Status) -> u8 {
    match status {
        Status::Checking => 0,
        Status::Missing => 1,
        Status::Listening => 2,
        Status::Revoked => 3,
        Status::NeedsRelaunch => 4,
    }
}

fn decode(value: u8) -> Status {
    match value {
        1 => Status::Missing,
        2 => Status::Listening,
        3 => Status::Revoked,
        4 => Status::NeedsRelaunch,
        _ => Status::Checking,
    }
}

/// The listener's current status. Any thread.
pub fn status() -> Status {
    decode(STATUS.load(Ordering::Acquire))
}

/// Registers the function called (on the tap thread) after each status
/// change. Only the first registration counts.
pub fn observe(on_change: fn()) {
    let _ = OBSERVER.set(on_change);
}

/// Stores `status`; returns whether it changed (and then calls the observer).
pub(super) fn publish(status: Status) -> bool {
    let changed = STATUS.swap(encode(status), Ordering::AcqRel) != encode(status);
    if changed && let Some(on_change) = OBSERVER.get() {
        on_change();
    }
    changed
}

// --- macOS ---

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    /// `IOHIDAccessType IOHIDCheckAccess(IOHIDRequestType)`; both are C enums.
    fn IOHIDCheckAccess(request: u32) -> u32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGRequestListenEventAccess() -> bool;
}

const IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

fn access_from(raw: u32) -> Access {
    match raw {
        0 => Access::Granted,
        1 => Access::Denied,
        _ => Access::Unknown,
    }
}

/// Checks Input Monitoring without prompting.
pub fn check() -> Access {
    // SAFETY: no preconditions; only reads the permission state.
    access_from(unsafe { IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) })
}

/// Lists Clicky under Input Monitoring; macOS shows its own prompt the first
/// time only. Call it from a user action, never at launch.
pub fn request() {
    // SAFETY: no preconditions.
    unsafe { CGRequestListenEventAccess() };
}

/// The System Settings page: Privacy & Security → Input Monitoring.
pub const SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent";

#[cfg(test)]
mod tests {
    use super::*;

    use Access::{Denied, Granted, Unknown};

    /// Runs one tick and performs a successful or failed creation if asked.
    fn tick(s: &mut Supervisor, access: Access, tap: Option<bool>, create_ok: bool) -> Action {
        let action = s.tick(access, tap);
        if action == Action::Create {
            if create_ok { s.created() } else { s.create_failed() }
        }
        action
    }

    #[test]
    fn with_the_permission_at_launch_the_tap_starts_at_once() {
        let mut s = Supervisor::default();
        assert_eq!(s.status(), Status::Checking);
        assert_eq!(tick(&mut s, Granted, None, true), Action::Create);
        assert_eq!(s.status(), Status::Listening);
        assert_eq!(s.tick(Granted, Some(true)), Action::Wait);
        assert_eq!(s.status(), Status::Listening);
    }

    #[test]
    fn a_first_run_waits_without_a_tap_and_starts_when_granted() {
        let mut s = Supervisor::default();
        assert_eq!(s.tick(Unknown, None), Action::Wait);
        assert_eq!(s.status(), Status::Missing);
        assert_eq!(s.tick(Denied, None), Action::Wait);
        assert_eq!(s.status(), Status::Missing);
        assert_eq!(tick(&mut s, Granted, None, true), Action::Create);
        assert_eq!(s.status(), Status::Listening);
    }

    #[test]
    fn a_grant_the_process_cannot_use_asks_for_a_relaunch() {
        let mut s = Supervisor::default();
        s.tick(Unknown, None);
        tick(&mut s, Granted, None, false);
        assert_eq!(s.status(), Status::Missing, "one failure may be a race with System Settings");
        tick(&mut s, Granted, None, false);
        assert_eq!(s.status(), Status::NeedsRelaunch);
        // It keeps trying; if the tap comes up after all, no relaunch is needed.
        assert_eq!(tick(&mut s, Granted, None, true), Action::Create);
        assert_eq!(s.status(), Status::Listening);
    }

    #[test]
    fn granted_at_launch_but_refused_is_missing_not_relaunch() {
        let mut s = Supervisor::default();
        for _ in 0..5 {
            tick(&mut s, Granted, None, false);
        }
        assert_eq!(s.status(), Status::Missing, "relaunching would not help");
    }

    #[test]
    fn revoking_while_listening_drops_the_tap_and_says_revoked() {
        let mut s = Supervisor::default();
        tick(&mut s, Granted, None, true);
        assert_eq!(s.tick(Denied, Some(true)), Action::Drop);
        assert_eq!(s.status(), Status::Revoked);
        assert_eq!(s.tick(Denied, None), Action::Wait);
        assert_eq!(s.status(), Status::Revoked);
        // A `tccutil reset` while running reads as unknown: still revoked.
        assert_eq!(s.tick(Unknown, None), Action::Wait);
        assert_eq!(s.status(), Status::Revoked);
    }

    #[test]
    fn granting_again_restores_listening() {
        let mut s = Supervisor::default();
        tick(&mut s, Granted, None, true);
        s.tick(Denied, Some(false));
        assert_eq!(tick(&mut s, Granted, None, true), Action::Create);
        assert_eq!(s.status(), Status::Listening);
    }

    #[test]
    fn regranting_that_needs_a_relaunch_says_so() {
        let mut s = Supervisor::default();
        tick(&mut s, Granted, None, true);
        s.tick(Denied, Some(true));
        tick(&mut s, Granted, None, false);
        tick(&mut s, Granted, None, false);
        assert_eq!(s.status(), Status::NeedsRelaunch);
        // Revoked again before relaunching: back to revoked.
        s.tick(Denied, None);
        assert_eq!(s.status(), Status::Revoked);
    }

    #[test]
    fn a_dead_tap_with_the_permission_is_recreated() {
        let mut s = Supervisor::default();
        tick(&mut s, Granted, None, true);
        assert_eq!(s.tick(Granted, Some(false)), Action::Drop);
        assert_eq!(s.status(), Status::Listening, "no flicker in the UI");
        assert_eq!(tick(&mut s, Granted, None, true), Action::Create);
        assert_eq!(s.status(), Status::Listening);
    }

    #[test]
    fn only_problem_states_need_the_permission() {
        assert!(!Status::Checking.needs_permission());
        assert!(!Status::Listening.needs_permission());
        assert!(Status::Missing.needs_permission());
        assert!(Status::Revoked.needs_permission());
        assert!(Status::NeedsRelaunch.needs_permission());
    }

    #[test]
    fn the_published_status_round_trips() {
        for status in [
            Status::Checking,
            Status::Missing,
            Status::Listening,
            Status::Revoked,
            Status::NeedsRelaunch,
        ] {
            assert_eq!(decode(encode(status)), status);
        }
        assert_eq!(decode(200), Status::Checking);
    }

    #[test]
    fn iohid_access_values_follow_the_sdk() {
        // IOHIDLib.h: kIOHIDAccessTypeGranted, Denied, Unknown (0, 1, 2).
        assert_eq!(access_from(0), Granted);
        assert_eq!(access_from(1), Denied);
        assert_eq!(access_from(2), Unknown);
        assert_eq!(access_from(7), Unknown);
    }
}
