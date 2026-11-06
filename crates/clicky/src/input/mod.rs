//! Global key listener: a listen-only `CGEventTap` on its own high-priority
//! thread. It needs only the Input Monitoring permission, cannot change or
//! block events, and reads from each event only its type, its keycode, the
//! auto-repeat flag of key-downs and the modifier flags of flags-changed
//! events (which modifier keys are down). Media keys arrive as system-defined
//! events; `media` reads their key type. It never asks for characters, so
//! what the user types is never seen.
//!
//! The thread also supervises the Input Monitoring permission
//! (`permission::Supervisor`): it waits without a tap while the permission is
//! missing, drops the tap when the permission is revoked, recreates it when
//! the permission comes back, and publishes the status for the UI. It never
//! asks for the permission itself; the first-run window does, on a click.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/input/mod.rs`.

mod ffi;
mod media;
pub mod permission;

use std::ffi::c_void;
use std::time::{Duration, Instant};

use crate::diagnostics::{self, Event};
use crate::engine::Voicer;
use crate::keys::{KeyEvent, KeyState, LogOnce, Transition};

/// How often to check the permission while there is no tap.
const WAITING_CHECK: Duration = Duration::from_secs(1);
/// How often to check the permission and the tap while listening.
const LISTENING_CHECK: Duration = Duration::from_secs(3);

struct TapState {
    voicer: Voicer,
    keys: KeyState,
    unrecognised: LogOnce,
    tap: ffi::CFMachPortRef,
}

/// Starts the listener thread. It waits for the permission if needed, then
/// runs the tap for the life of the process, following the permission.
pub fn start(voicer: Voicer) {
    std::thread::Builder::new()
        .name("clicky-input".into())
        .spawn(move || run(voicer))
        .expect("failed to spawn the input thread");
}

fn run(voicer: Voicer) {
    // SAFETY: adjusts the QoS of the current thread only.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
    // Never freed, so it outlives every tap.
    let state = Box::into_raw(Box::new(TapState {
        voicer,
        keys: KeyState::default(),
        unrecognised: LogOnce::default(),
        tap: std::ptr::null_mut(),
    }));
    let mut supervisor = permission::Supervisor::default();
    let mut tap: Option<Tap> = None;
    loop {
        let alive = tap.as_ref().map(Tap::is_enabled);
        match supervisor.tick(permission::check(), alive) {
            permission::Action::Wait => {}
            permission::Action::Create => match Tap::create(state) {
                Some(created) => {
                    tap = Some(created);
                    supervisor.created();
                }
                None => supervisor.create_failed(),
            },
            permission::Action::Drop => {
                tap = None;
                // SAFETY: the tap is gone, so its callback cannot run; this thread owns `state`.
                unsafe {
                    (*state).tap = std::ptr::null_mut();
                    // Key-ups are not seen without a tap.
                    (*state).keys.reset();
                }
            }
        }
        let status = supervisor.status();
        if permission::publish(status) {
            log_status(status);
        }
        match &tap {
            // SAFETY: runs this thread's run loop, which delivers the tap's events.
            Some(_) => unsafe {
                ffi::CFRunLoopRunInMode(
                    ffi::kCFRunLoopDefaultMode,
                    LISTENING_CHECK.as_secs_f64(),
                    false,
                );
            },
            None => std::thread::sleep(WAITING_CHECK),
        }
    }
}

fn log_status(status: permission::Status) {
    use permission::Status;
    match status {
        Status::Checking => {}
        Status::Missing => diagnostics::log(Event::InputPermissionMissing),
        Status::Listening => diagnostics::log(Event::InputTapStarted),
        Status::Revoked => diagnostics::log(Event::InputPermissionRevoked),
        Status::NeedsRelaunch => diagnostics::log(Event::InputNeedsRelaunch),
    }
}

/// A live event tap on the current thread's run loop. Dropping it removes and
/// invalidates it.
struct Tap {
    port: ffi::CFMachPortRef,
    source: ffi::CFRunLoopSourceRef,
}

impl Tap {
    /// Creates and enables the tap; `None` without the permission.
    fn create(state: *mut TapState) -> Option<Tap> {
        let mask = (1u64 << ffi::kCGEventKeyDown)
            | (1u64 << ffi::kCGEventKeyUp)
            | (1u64 << ffi::kCGEventFlagsChanged)
            | (1u64 << ffi::kCGEventSystemDefined);
        // SAFETY: `state` is never freed, so it outlives the tap.
        let port = unsafe {
            ffi::CGEventTapCreate(
                ffi::kCGSessionEventTap,
                ffi::kCGHeadInsertEventTap,
                ffi::kCGEventTapOptionListenOnly,
                mask,
                tap_callback,
                state.cast(),
            )
        };
        if port.is_null() {
            return None;
        }
        // SAFETY: `port` is a valid mach port; the callback runs only on this
        // thread's run loop, so writing `state.tap` here does not race.
        unsafe {
            let source = ffi::CFMachPortCreateRunLoopSource(std::ptr::null(), port, 0);
            if source.is_null() {
                ffi::CFMachPortInvalidate(port);
                ffi::CFRelease(port.cast_const());
                return None;
            }
            (*state).tap = port;
            ffi::CFRunLoopAddSource(ffi::CFRunLoopGetCurrent(), source, ffi::kCFRunLoopCommonModes);
            ffi::CGEventTapEnable(port, true);
            Some(Tap { port, source })
        }
    }

    fn is_enabled(&self) -> bool {
        // SAFETY: `port` is valid until `drop`.
        unsafe { ffi::CGEventTapIsEnabled(self.port) }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: both were created by `Tap::create` on this thread and are released once.
        unsafe {
            ffi::CFRunLoopRemoveSource(
                ffi::CFRunLoopGetCurrent(),
                self.source,
                ffi::kCFRunLoopCommonModes,
            );
            ffi::CFMachPortInvalidate(self.port);
            ffi::CFRelease(self.source.cast_const());
            ffi::CFRelease(self.port.cast_const());
        }
    }
}

/// Event-tap callback. Ordinary key events take no locks and allocate nothing;
/// see `media` for system-defined events.
///
/// PRIVACY (for review): from each event this reads only its type, the
/// keycode, the auto-repeat flag and the modifier flags, and for media keys
/// the key type in `data1` (`media::read`). It never calls
/// `CGEventKeyboardGetUnicodeString` or anything else that yields characters,
/// and it never logs a keycode.
unsafe extern "C" fn tap_callback(
    _proxy: ffi::CGEventTapProxy,
    kind: u32,
    event: ffi::CGEventRef,
    user_info: *mut c_void,
) -> ffi::CGEventRef {
    // SAFETY: `user_info` is the TapState owned by this thread for the life of the tap.
    let state = unsafe { &mut *user_info.cast::<TapState>() };
    // SAFETY (both field reads): `event` is a valid CGEvent for this callback.
    let field = |id| unsafe { ffi::CGEventGetIntegerValueField(event, id) };
    let keycode = || u16::try_from(field(ffi::kCGKeyboardEventKeycode)).unwrap_or(u16::MAX);
    let key_event = match kind {
        ffi::kCGEventTapDisabledByTimeout | ffi::kCGEventTapDisabledByUserInput => {
            // SAFETY: `state.tap` is the live tap.
            unsafe { ffi::CGEventTapEnable(state.tap, true) };
            // Key-ups may have been missed while the tap was off.
            state.keys.reset();
            let cause =
                if kind == ffi::kCGEventTapDisabledByTimeout { "timeout" } else { "user input" };
            diagnostics::log(Event::InputTapReenabled { cause });
            None
        }
        ffi::kCGEventKeyDown => Some(KeyEvent::Down {
            keycode: keycode(),
            autorepeat: field(ffi::kCGKeyboardEventAutorepeat) != 0,
        }),
        ffi::kCGEventKeyUp => Some(KeyEvent::Up { keycode: keycode() }),
        ffi::kCGEventFlagsChanged => Some(KeyEvent::FlagsChanged {
            keycode: keycode(),
            // SAFETY: `event` is a valid CGEvent for this callback.
            flags: unsafe { ffi::CGEventGetFlags(event) },
        }),
        ffi::kCGEventSystemDefined => media::read(event),
        _ => None,
    };
    let Some(key_event) = key_event else { return event };
    if let Some(kind) = state.unrecognised.first(&key_event) {
        // At most once per kind per launch; the kind never names the key.
        diagnostics::log(Event::UnrecognisedInput { kind });
    }
    match state.keys.handle(key_event, Instant::now()) {
        Some(Transition::Press(keycode)) => state.voicer.press(keycode),
        // Release sounds are issue #8.
        Some(Transition::Release(_)) | None => {}
    }
    event
}
