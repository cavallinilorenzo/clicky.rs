//! "Quit Clicky" (⌘Q), the last item.

use objc2::{MainThreadOnly, sel};
use objc2_app_kit::NSMenuItem;
use objc2_foundation::NSString;

use super::MenuBuilder;

pub fn add(menu: &mut MenuBuilder) {
    menu.separator();
    // SAFETY: `terminate:` is implemented by NSApplication; a nil target reaches it
    // through the responder chain.
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(menu.mtm()),
            &NSString::from_str("Quit Clicky"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        )
    };
    menu.add(&quit);
}
