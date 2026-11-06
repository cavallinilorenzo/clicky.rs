//! "Diagnostics Log": a hidden toggle, the alternate of "About Clicky". AppKit
//! shows it in About's place while Option is held, including when Option is
//! pressed or released with the menu already open. We never read the
//! modifier state ourselves.

use objc2_app_kit::NSEventModifierFlags;
use objc2_foundation::NSString;

use super::{MenuBuilder, control_state};
use crate::diagnostics::{self, LOG_ENABLED};

/// Must come right after `about::add`: an alternate item follows its base
/// item, with the same (empty) key equivalent and another modifier mask.
pub fn add(menu: &mut MenuBuilder) {
    let ui = menu.ui().clone();
    let item = menu.item("Diagnostics Log", move |_| {
        let on = !diagnostics::is_enabled();
        diagnostics::set_enabled(on);
        ui.store.set(&LOG_ENABLED, &on);
    });
    item.setKeyEquivalentModifierMask(NSEventModifierFlags::Option);
    item.setAlternate(true);
    let tooltip = match diagnostics::log_path() {
        Some(path) => {
            format!("Record audio start/stop and errors (never keys) in {}", path.display())
        }
        None => "Unavailable: no home folder to write the log to".to_owned(),
    };
    item.setToolTip(Some(&NSString::from_str(&tooltip)));

    let shown = item.clone();
    menu.on_open(move || shown.setState(control_state(diagnostics::is_enabled())));
}
