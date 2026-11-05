//! "Diagnostics Log": a hidden toggle, shown only when the menu is opened
//! with Option held. Reading the modifier state tells us only that Option is
//! down, never what is typed.

use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use objc2_foundation::NSString;

use super::{MenuBuilder, control_state};
use crate::diagnostics::{self, LOG_ENABLED};

pub fn add(menu: &mut MenuBuilder) {
    let separator = menu.separator();
    let ui = menu.ui().clone();
    let item = menu.item("Diagnostics Log", move |_| {
        let on = !diagnostics::is_enabled();
        diagnostics::set_enabled(on);
        ui.store.set(&LOG_ENABLED, &on);
    });
    let tooltip = format!(
        "Record audio start/stop and errors (never keys) in {}",
        diagnostics::log_path().display()
    );
    item.setToolTip(Some(&NSString::from_str(&tooltip)));

    let shown = item.clone();
    menu.on_open(move || {
        let option = NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Option);
        separator.setHidden(!option);
        shown.setHidden(!option);
        shown.setState(control_state(diagnostics::is_enabled()));
    });
}
