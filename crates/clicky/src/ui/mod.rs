//! The menu bar user interface (AppKit through objc2): one status item and
//! its menu. No Dock icon (`LSUIElement` plus the accessory policy); the only
//! window is the first-run permission window (`onboarding`).

pub mod menu;
pub mod onboarding;
pub mod status_item;
pub mod target;

use std::cell::{OnceCell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate};
use objc2_foundation::NSNotification;

use crate::engine::Shared;
use crate::input::permission;
use crate::settings::Store;
use status_item::{Icon, StatusItem};

/// What the menu sections share. Lives on the main thread for the life of the app.
pub struct Ui {
    pub mtm: MainThreadMarker,
    pub shared: &'static Shared,
    pub store: Store,
    status: OnceCell<StatusItem>,
    onboarding: onboarding::Onboarding,
}

impl Ui {
    /// Redraws the menu bar icon from the current state.
    pub fn refresh_status(&self) {
        if let Some(status) = self.status.get() {
            let needs_permission = permission::status().needs_permission();
            status.show(Icon::for_state(self.shared.enabled(), needs_permission));
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

/// `Clicky --menu`: builds the real menu, as at launch, without running the
/// app, and prints it ([`menu::describe`]). Writes no setting and listens to
/// no key. For tests and the verifier.
pub fn print_menu() {
    let mtm = MainThreadMarker::new().expect("Clicky must start on the main thread");
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let store = Store::standard();
    let shared: &'static Shared =
        Box::leak(Box::new(Shared::new(store.get(&crate::engine::SOUNDS_ON))));
    let ui = Rc::new(Ui {
        mtm,
        shared,
        store,
        status: OnceCell::new(),
        onboarding: onboarding::Onboarding::default(),
    });
    let menu = menu::build(&ui);
    menu.refresh();
    for line in menu::describe(&menu.menu) {
        println!("{line}");
    }
}

fn launch(mtm: MainThreadMarker, shared: &'static Shared, store: Store) {
    let ui = Rc::new(Ui {
        mtm,
        shared,
        store,
        status: OnceCell::new(),
        onboarding: onboarding::Onboarding::default(),
    });
    let menu = menu::build(&ui);
    let _ = ui.status.set(StatusItem::new(mtm, &menu.menu));
    MAIN_UI.with(|cell| {
        let _ = cell.set(ui.clone());
    });
    permission::observe(input_status_changed);
    // The status may have changed before the observer was registered.
    on_input_status(&ui);
    // The menu, its targets and the status item live as long as the app.
    std::mem::forget(menu);
}

/// The listener's permission status changed: the icon and the first-run
/// window follow it (the menu's permission rows refresh when it opens).
fn on_input_status(ui: &Rc<Ui>) {
    ui.refresh_status();
    onboarding::status_changed(ui);
}

thread_local! {
    /// The UI, for callbacks that arrive on the main queue without context.
    static MAIN_UI: OnceCell<Rc<Ui>> = const { OnceCell::new() };
}

/// Called on the tap thread after a status change; hops to the main thread.
fn input_status_changed() {
    // SAFETY: the main queue lives for the whole process; `on_main` takes no context.
    unsafe { dispatch_async_f(&raw const _dispatch_main_q, std::ptr::null_mut(), on_main) };
}

extern "C" fn on_main(_context: *mut c_void) {
    MAIN_UI.with(|cell| {
        if let Some(ui) = cell.get() {
            on_input_status(ui);
        }
    });
}

// libdispatch (part of libSystem): `dispatch_get_main_queue()` is `&_dispatch_main_q`.
unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(
        queue: *const c_void,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
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
