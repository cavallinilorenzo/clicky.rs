//! The status item's menu. Each section lives in its own file and adds its
//! items, actions and refresh hook through [`MenuBuilder`]; `build` only lists
//! the sections in menu order.

mod about;
mod diagnostics;
mod launch_at_login;
mod permission;
mod quit;
mod sound_switch;
mod soundpack;

use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSControlStateValue, NSControlStateValueOff, NSControlStateValueOn, NSMenu, NSMenuItem,
};
use objc2_foundation::NSString;

use super::Ui;
use super::target::{ActionTarget, MenuDelegate};

/// The built menu and everything AppKit holds only weakly.
pub struct Menu {
    pub menu: Retained<NSMenu>,
    _targets: Vec<Retained<ActionTarget>>,
    _delegate: Retained<MenuDelegate>,
}

/// Builds the menu. Sections in the order of issue #2; later tickets insert theirs
/// at the marked places.
pub fn build(ui: &Rc<Ui>) -> Menu {
    let mut menu = MenuBuilder::new(ui);
    sound_switch::add(&mut menu);
    soundpack::add(&mut menu); // #7: soundpack picker
    // #6: volume slider
    // #8: Key Release Sounds, Natural Variation, Spatial Stereo
    launch_at_login::add(&mut menu); // #9: Launch at Login
    permission::add(&mut menu); // #9: the permission rows (only while sounds are unavailable)
    about::add(&mut menu); // #7: About
    diagnostics::add(&mut menu);
    quit::add(&mut menu);
    menu.finish()
}

/// What a section uses to add itself to the menu.
pub struct MenuBuilder {
    ui: Rc<Ui>,
    menu: Retained<NSMenu>,
    targets: Vec<Retained<ActionTarget>>,
    on_open: Vec<Box<dyn Fn()>>,
}

impl MenuBuilder {
    fn new(ui: &Rc<Ui>) -> MenuBuilder {
        let menu = NSMenu::new(ui.mtm);
        menu.setAutoenablesItems(false);
        MenuBuilder { ui: ui.clone(), menu, targets: Vec::new(), on_open: Vec::new() }
    }

    pub fn ui(&self) -> &Rc<Ui> {
        &self.ui
    }

    pub fn mtm(&self) -> MainThreadMarker {
        self.ui.mtm
    }

    pub fn add(&mut self, item: &NSMenuItem) {
        self.menu.addItem(item);
    }

    /// Adds a separator and returns it (e.g. to hide it with its section).
    pub fn separator(&mut self) -> Retained<NSMenuItem> {
        let separator = NSMenuItem::separatorItem(self.mtm());
        self.add(&separator);
        separator
    }

    /// A target that runs `action`, kept alive as long as the menu.
    pub fn action(&mut self, action: impl Fn(&AnyObject) + 'static) -> Retained<ActionTarget> {
        let target = ActionTarget::new(self.mtm(), action);
        self.targets.push(target.clone());
        target
    }

    /// A plain menu item whose click runs `action`.
    pub fn item(
        &mut self,
        title: &str,
        action: impl Fn(&AnyObject) + 'static,
    ) -> Retained<NSMenuItem> {
        let item = NSMenuItem::new(self.mtm());
        item.setTitle(&NSString::from_str(title));
        self.action(action).attach_item(&item);
        self.add(&item);
        item
    }

    /// Runs `refresh` each time the menu is about to open.
    pub fn on_open(&mut self, refresh: impl Fn() + 'static) {
        self.on_open.push(Box::new(refresh));
    }

    fn finish(self) -> Menu {
        let hooks = self.on_open;
        let delegate = MenuDelegate::new(self.ui.mtm, move || hooks.iter().for_each(|f| f()));
        self.menu.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Menu { menu: self.menu, _targets: self.targets, _delegate: delegate }
    }
}

pub fn control_state(on: bool) -> NSControlStateValue {
    if on { NSControlStateValueOn } else { NSControlStateValueOff }
}
