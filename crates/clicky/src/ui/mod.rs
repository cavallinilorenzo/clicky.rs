//! The menu bar user interface (AppKit through objc2): one status item and
//! its menu. No Dock icon, no window (`LSUIElement` plus the accessory policy).

pub mod menu;
pub mod status_item;
pub mod target;

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate};
use objc2_foundation::NSNotification;

use crate::engine::Shared;
use crate::settings::Store;
use status_item::{Icon, StatusItem};

/// What the menu sections share. Lives on the main thread for the life of the app.
pub struct Ui {
    pub mtm: MainThreadMarker,
    pub shared: &'static Shared,
    pub store: Store,
    status: OnceCell<StatusItem>,
}

impl Ui {
    /// Redraws the menu bar icon from the current state.
    pub fn refresh_status(&self) {
        if let Some(status) = self.status.get() {
            status.show(Icon::for_state(self.shared.enabled()));
        }
    }
}

/// Runs the menu bar app on the main thread. Does not return.
pub fn run(shared: &'static Shared, store: Store) {
    let mtm = MainThreadMarker::new().expect("Clicky must start on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = AppDelegate::new(mtm, Box::new(move || launch(mtm, shared, store)));
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}

fn launch(mtm: MainThreadMarker, shared: &'static Shared, store: Store) {
    let ui = Rc::new(Ui { mtm, shared, store, status: OnceCell::new() });
    let menu = menu::build(&ui);
    let _ = ui.status.set(StatusItem::new(mtm, &menu.menu));
    ui.refresh_status();
    // The menu, its targets and the status item live as long as the app.
    std::mem::forget(menu);
}

pub struct AppDelegateIvars {
    on_launch: RefCell<Option<Box<dyn FnOnce()>>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and we implement no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ClickyAppDelegate"]
    #[ivars = AppDelegateIvars]
    struct AppDelegate;

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: the signature matches `-applicationDidFinishLaunching:`.
    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            if let Some(on_launch) = self.ivars().on_launch.borrow_mut().take() {
                on_launch();
            }
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker, on_launch: Box<dyn FnOnce()>) -> Retained<Self> {
        let this = Self::alloc(mtm)
            .set_ivars(AppDelegateIvars { on_launch: RefCell::new(Some(on_launch)) });
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }
}
