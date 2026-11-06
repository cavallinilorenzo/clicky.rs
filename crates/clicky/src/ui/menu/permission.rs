//! The permission rows, at the top of the menu: shown only while sounds are
//! unavailable for lack of Input Monitoring, saying so and offering the fix.
//! Refreshed each time the menu opens.

use std::rc::Rc;

use objc2_app_kit::NSMenuItem;
use objc2_foundation::NSString;

use super::MenuBuilder;
use crate::input::permission::{self, Status};
use crate::ui::{Ui, onboarding};

/// What the fix item does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Shows the first-run window (which explains and opens System Settings).
    Setup,
    /// Opens System Settings at Input Monitoring directly.
    OpenSettings,
    /// Relaunches Clicky.
    Relaunch,
}

/// The rows for one status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rows {
    pub headline: &'static str,
    pub hint: &'static str,
    pub fix_title: &'static str,
    pub fix: Fix,
}

/// The rows to show, or `None` when sounds are available.
pub fn rows(status: Status) -> Option<Rows> {
    const HEADLINE: &str = "Sounds unavailable";
    match status {
        Status::Checking | Status::Listening => None,
        Status::Missing => Some(Rows {
            headline: HEADLINE,
            hint: "Clicky needs Input Monitoring to notice key presses",
            fix_title: "Set Up Input Monitoring…",
            fix: Fix::Setup,
        }),
        Status::Revoked => Some(Rows {
            headline: HEADLINE,
            hint: "Input Monitoring was turned off for Clicky",
            fix_title: "Turn It On in System Settings…",
            fix: Fix::OpenSettings,
        }),
        Status::NeedsRelaunch => Some(Rows {
            headline: HEADLINE,
            hint: "Input Monitoring is on; Clicky must restart to use it",
            fix_title: "Relaunch Clicky",
            fix: Fix::Relaunch,
        }),
    }
}

/// Adds the rows at the top of the menu, with a separator below them.
pub fn add(menu: &mut MenuBuilder) {
    let headline = info_item(menu);
    let hint = info_item(menu);
    let ui = menu.ui().clone();
    let fix = menu.item("", move |_| run_fix(&ui));
    let separator = menu.separator();

    let show = move || {
        let rows = rows(permission::status());
        for item in [&separator, &headline, &hint, &fix] {
            item.setHidden(rows.is_none());
        }
        if let Some(rows) = rows {
            headline.setTitle(&NSString::from_str(rows.headline));
            hint.setTitle(&NSString::from_str(rows.hint));
            fix.setTitle(&NSString::from_str(rows.fix_title));
        }
    };
    show();
    menu.on_open(show);
}

/// A greyed-out line of text.
fn info_item(menu: &mut MenuBuilder) -> objc2::rc::Retained<NSMenuItem> {
    let item = NSMenuItem::new(menu.mtm());
    item.setEnabled(false);
    menu.add(&item);
    item
}

fn run_fix(ui: &Rc<Ui>) {
    match rows(permission::status()).map(|r| r.fix) {
        Some(Fix::Setup) => onboarding::open(ui),
        Some(Fix::OpenSettings) => onboarding::open_input_monitoring_settings(),
        Some(Fix::Relaunch) => onboarding::relaunch(ui.mtm),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_shows_while_listening_or_checking() {
        assert_eq!(rows(Status::Listening), None);
        assert_eq!(rows(Status::Checking), None);
    }

    #[test]
    fn a_missing_permission_says_sounds_are_unavailable_and_offers_setup() {
        let r = rows(Status::Missing).unwrap();
        assert!(r.headline.contains("Sounds unavailable"), "{r:?}");
        assert!(r.hint.contains("Input Monitoring"), "{r:?}");
        assert_eq!(r.fix, Fix::Setup);
    }

    #[test]
    fn a_revoked_permission_says_so_and_opens_settings() {
        let r = rows(Status::Revoked).unwrap();
        assert!(r.headline.contains("Sounds unavailable"), "{r:?}");
        assert!(r.hint.contains("turned off"), "{r:?}");
        assert_eq!(r.fix, Fix::OpenSettings);
        assert!(r.fix_title.ends_with('…'), "it opens another app: {r:?}");
    }

    #[test]
    fn a_grant_waiting_for_a_relaunch_offers_it() {
        let r = rows(Status::NeedsRelaunch).unwrap();
        assert_eq!(r.fix, Fix::Relaunch);
        assert_eq!(r.fix_title, "Relaunch Clicky");
    }
}
