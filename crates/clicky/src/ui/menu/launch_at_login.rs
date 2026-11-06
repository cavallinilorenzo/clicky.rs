//! "Launch at Login": a checkbox over `SMAppService.mainAppService`
//! (macOS 13+). It shows the real system state each time the menu opens, so
//! a change made in System Settings → General → Login Items is reflected.
//! The logic sits behind the [`LoginItem`] seam; [`MainAppService`] is the
//! thin system adapter.

use std::rc::Rc;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2_app_kit::{NSControlStateValueMixed, NSMenuItem};
use objc2_foundation::{NSError, NSString};

use super::{MenuBuilder, control_state};
use crate::diagnostics::{self, Event};

/// `SMAppServiceStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginStatus {
    NotRegistered,
    Enabled,
    /// Registered, but the user must allow it in Login Items.
    RequiresApproval,
    /// The system cannot find the app's login item (e.g. not run from a bundle).
    NotFound,
}

impl LoginStatus {
    /// A fixed description for the diagnostics log.
    pub fn describe(self) -> &'static str {
        match self {
            LoginStatus::NotRegistered => "off",
            LoginStatus::Enabled => "on",
            LoginStatus::RequiresApproval => "on, waiting for approval in Login Items",
            LoginStatus::NotFound => "not found",
        }
    }
}

/// The system login item, behind a seam so the logic is testable without
/// registering anything on the Mac running the tests.
pub trait LoginItem {
    fn status(&self) -> LoginStatus;
    fn register(&self) -> Result<(), String>;
    fn unregister(&self) -> Result<(), String>;
    /// Opens System Settings → General → Login Items.
    fn open_settings(&self);
}

/// The checkbox's look.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    On,
    Off,
    /// Registered but not yet allowed.
    Mixed,
}

/// What the menu shows for a status: the checkbox and an optional note row.
pub fn view(status: LoginStatus) -> (Check, Option<&'static str>) {
    match status {
        LoginStatus::Enabled => (Check::On, None),
        LoginStatus::NotRegistered | LoginStatus::NotFound => (Check::Off, None),
        LoginStatus::RequiresApproval => {
            (Check::Mixed, Some("Needs Your Approval in Login Items…"))
        }
    }
}

/// The result of a click on the checkbox.
#[derive(Debug, PartialEq, Eq)]
pub struct Toggled {
    /// The system state after the change (re-read).
    pub status: LoginStatus,
    /// The system's error, if the change failed. Never contains key data.
    pub error: Option<String>,
}

/// Flips Launch at Login and returns the new state.
pub fn toggle(item: &dyn LoginItem) -> Toggled {
    let result = match item.status() {
        LoginStatus::Enabled | LoginStatus::RequiresApproval => item.unregister(),
        LoginStatus::NotRegistered | LoginStatus::NotFound => item.register(),
    };
    let status = item.status();
    if result.is_ok() && status == LoginStatus::RequiresApproval {
        item.open_settings();
    }
    Toggled { status, error: result.err() }
}

pub fn add(menu: &mut MenuBuilder) {
    let Some(service) = MainAppService::get() else {
        diagnostics::log(Event::Error {
            context: "launch at login",
            detail: "SMAppService is unavailable".into(),
        });
        return;
    };
    let service = Rc::new(service);
    let checkbox = NSMenuItem::new(menu.mtm());
    checkbox.setTitle(&NSString::from_str("Launch at Login"));
    menu.add(&checkbox);
    let approve = {
        let service = service.clone();
        menu.item("", move |_| service.open_settings())
    };

    let show = {
        let (checkbox, approve) = (checkbox.clone(), approve.clone());
        move |status: LoginStatus| {
            let (check, note) = view(status);
            checkbox.setState(match check {
                Check::On => control_state(true),
                Check::Off => control_state(false),
                Check::Mixed => NSControlStateValueMixed,
            });
            approve.setHidden(note.is_none());
            if let Some(note) = note {
                approve.setTitle(&NSString::from_str(note));
            }
        }
    };
    let show = Rc::new(show);
    {
        let (service, show) = (service.clone(), show.clone());
        menu.action(move |_| {
            let toggled = toggle(&*service);
            if let Some(detail) = toggled.error {
                diagnostics::log(Event::Error { context: "launch at login", detail });
            }
            diagnostics::log(Event::LaunchAtLogin { status: toggled.status.describe() });
            show(toggled.status);
        })
        .attach_item(&checkbox);
    }
    show(service.status());
    // The real system state, read each time: the user may change it in System Settings.
    menu.on_open(move || show(service.status()));
}

// ServiceManagement is linked for `SMAppService`, reached dynamically.
#[link(name = "ServiceManagement", kind = "framework")]
unsafe extern "C" {}

/// `SMAppService.mainAppService`: Clicky itself as a login item.
///
/// Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
/// https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/ui/login.rs`.
struct MainAppService(Retained<AnyObject>);

impl MainAppService {
    fn get() -> Option<MainAppService> {
        let class = AnyClass::get(c"SMAppService")?;
        // SAFETY: `+[SMAppService mainAppService]` (macOS 13+) returns a non-nil object.
        Some(MainAppService(unsafe { msg_send![class, mainAppService] }))
    }
}

impl LoginItem for MainAppService {
    fn status(&self) -> LoginStatus {
        // SAFETY: `-[SMAppService status]` returns an NSInteger-backed enum.
        let raw: isize = unsafe { msg_send![&*self.0, status] };
        status_from(raw)
    }

    fn register(&self) -> Result<(), String> {
        // SAFETY: takes a trailing `NSError **` and returns BOOL.
        let result: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*self.0, registerAndReturnError: _] };
        result.map_err(|e| e.localizedDescription().to_string())
    }

    fn unregister(&self) -> Result<(), String> {
        // SAFETY: takes a trailing `NSError **` and returns BOOL.
        let result: Result<(), Retained<NSError>> =
            unsafe { msg_send![&*self.0, unregisterAndReturnError: _] };
        result.map_err(|e| e.localizedDescription().to_string())
    }

    fn open_settings(&self) {
        if let Some(class) = AnyClass::get(c"SMAppService") {
            // SAFETY: class method without arguments or return value (macOS 13+).
            let _: () = unsafe { msg_send![class, openSystemSettingsLoginItems] };
        }
    }
}

/// `SMAppServiceStatus` (SMAppService.h): NotRegistered, Enabled,
/// RequiresApproval, NotFound.
fn status_from(raw: isize) -> LoginStatus {
    match raw {
        1 => LoginStatus::Enabled,
        2 => LoginStatus::RequiresApproval,
        3 => LoginStatus::NotFound,
        _ => LoginStatus::NotRegistered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn sm_app_service_status_values_follow_the_sdk() {
        assert_eq!(status_from(0), LoginStatus::NotRegistered);
        assert_eq!(status_from(1), LoginStatus::Enabled);
        assert_eq!(status_from(2), LoginStatus::RequiresApproval);
        assert_eq!(status_from(3), LoginStatus::NotFound);
    }

    /// An in-memory login item that records what it was asked to do.
    struct Fake {
        status: RefCell<LoginStatus>,
        after_register: LoginStatus,
        fail: Option<&'static str>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Fake {
        fn new(status: LoginStatus) -> Fake {
            Fake {
                status: RefCell::new(status),
                after_register: LoginStatus::Enabled,
                fail: None,
                calls: RefCell::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }

    impl LoginItem for Fake {
        fn status(&self) -> LoginStatus {
            *self.status.borrow()
        }
        fn register(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("register");
            if let Some(e) = self.fail {
                return Err(e.into());
            }
            *self.status.borrow_mut() = self.after_register;
            Ok(())
        }
        fn unregister(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("unregister");
            if let Some(e) = self.fail {
                return Err(e.into());
            }
            *self.status.borrow_mut() = LoginStatus::NotRegistered;
            Ok(())
        }
        fn open_settings(&self) {
            self.calls.borrow_mut().push("open settings");
        }
    }

    #[test]
    fn the_checkbox_shows_the_system_state() {
        assert_eq!(view(LoginStatus::Enabled), (Check::On, None));
        assert_eq!(view(LoginStatus::NotRegistered), (Check::Off, None));
        assert_eq!(view(LoginStatus::NotFound), (Check::Off, None));
        let (check, note) = view(LoginStatus::RequiresApproval);
        assert_eq!(check, Check::Mixed);
        assert!(note.unwrap().contains("Login Items"), "{note:?}");
    }

    #[test]
    fn turning_it_on_registers() {
        let item = Fake::new(LoginStatus::NotRegistered);
        let toggled = toggle(&item);
        assert_eq!(toggled, Toggled { status: LoginStatus::Enabled, error: None });
        assert_eq!(item.calls(), ["register"]);
    }

    #[test]
    fn turning_it_off_unregisters() {
        let item = Fake::new(LoginStatus::Enabled);
        assert_eq!(toggle(&item).status, LoginStatus::NotRegistered);
        assert_eq!(item.calls(), ["unregister"]);
    }

    #[test]
    fn a_registration_waiting_for_approval_opens_login_items() {
        let mut item = Fake::new(LoginStatus::NotRegistered);
        item.after_register = LoginStatus::RequiresApproval;
        assert_eq!(toggle(&item).status, LoginStatus::RequiresApproval);
        assert_eq!(item.calls(), ["register", "open settings"]);
    }

    #[test]
    fn clicking_while_waiting_for_approval_turns_it_off() {
        let item = Fake::new(LoginStatus::RequiresApproval);
        assert_eq!(toggle(&item).status, LoginStatus::NotRegistered);
        assert_eq!(item.calls(), ["unregister"]);
    }

    #[test]
    fn a_not_found_item_is_registered_afresh() {
        let item = Fake::new(LoginStatus::NotFound);
        assert_eq!(toggle(&item).status, LoginStatus::Enabled);
        assert_eq!(item.calls(), ["register"]);
    }

    #[test]
    fn a_failure_reports_the_error_and_the_unchanged_state() {
        let mut item = Fake::new(LoginStatus::NotRegistered);
        item.fail = Some("Operation not permitted");
        let toggled = toggle(&item);
        assert_eq!(toggled.status, LoginStatus::NotRegistered);
        assert_eq!(toggled.error.as_deref(), Some("Operation not permitted"));
        assert_eq!(item.calls(), ["register"], "no Login Items detour after a failure");
    }
}
