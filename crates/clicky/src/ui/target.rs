//! Small Objective-C classes that forward AppKit callbacks to Rust closures,
//! so each menu section owns its own actions instead of adding methods to one
//! shared delegate class.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSControl, NSMenu, NSMenuDelegate, NSMenuItem};

pub struct ActionIvars {
    action: Box<dyn Fn(&AnyObject)>,
}

define_class!(
    /// The target of one control or menu item; calls its closure with the sender.
    // SAFETY: NSObject has no subclassing requirements and we implement no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClickyActionTarget"]
    #[ivars = ActionIvars]
    pub struct ActionTarget;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for ActionTarget {}

    impl ActionTarget {
        // SAFETY: takes one object argument and returns void, as AppKit actions do.
        #[unsafe(method(perform:))]
        fn perform(&self, sender: &AnyObject) {
            (self.ivars().action)(sender);
        }
    }
);

impl ActionTarget {
    pub fn new(mtm: MainThreadMarker, action: impl Fn(&AnyObject) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionIvars { action: Box::new(action) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// Makes this the target of `item`. AppKit holds targets weakly: keep `self` alive.
    pub fn attach_item(&self, item: &NSMenuItem) {
        // SAFETY: `self` implements `perform:`.
        unsafe {
            item.setTarget(Some(self));
            item.setAction(Some(sel!(perform:)));
        }
    }

    /// Makes this the target of `control`. Keep `self` alive.
    pub fn attach_control(&self, control: &NSControl) {
        // SAFETY: `self` implements `perform:`.
        unsafe {
            control.setTarget(Some(self));
            control.setAction(Some(sel!(perform:)));
        }
    }
}

pub struct MenuDelegateIvars {
    will_open: Box<dyn Fn()>,
}

define_class!(
    /// Calls its closure each time the menu is about to open.
    // SAFETY: NSObject has no subclassing requirements and we implement no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClickyMenuDelegate"]
    #[ivars = MenuDelegateIvars]
    pub struct MenuDelegate;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for MenuDelegate {}

    // SAFETY: the signature matches `-menuNeedsUpdate:`.
    unsafe impl NSMenuDelegate for MenuDelegate {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, _menu: &NSMenu) {
            (self.ivars().will_open)();
        }
    }
);

impl MenuDelegate {
    pub fn new(mtm: MainThreadMarker, will_open: impl Fn() + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuDelegateIvars { will_open: Box::new(will_open) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    /// Runs the closure, as AppKit does when the menu is about to open.
    pub fn will_open(&self) {
        (self.ivars().will_open)();
    }
}
