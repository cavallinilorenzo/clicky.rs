//! Global key listener: a listen-only `CGEventTap` on its own high-priority
//! thread. It needs only the Input Monitoring permission, cannot change or
//! block events, and reads exactly two things from each event: its type and
//! its keycode (plus the auto-repeat flag of key-down events). It never asks
//! for characters, so what the user types is never seen.
//!
//! Permission handling here is minimal (request once, retry quietly);
//! issue #9 owns the first-run window and revocation.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/input/mod.rs`.

mod ffi;

use std::ffi::c_void;
use std::time::Duration;

use crate::diagnostics::{self, Event};
use crate::engine::Voicer;
use crate::keys::{KeyEvent, KeyState};

/// How often to retry creating the tap while the permission is missing.
const PERMISSION_RETRY: Duration = Duration::from_secs(2);

struct TapState {
    voicer: Voicer,
    keys: KeyState,
    tap: ffi::CFMachPortRef,
}

/// Starts the listener thread. It waits for the permission if needed, then
/// runs the tap for the life of the process.
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
    let state = Box::into_raw(Box::new(TapState {
        voicer,
        keys: KeyState::default(),
        tap: std::ptr::null_mut(),
    }));
    let mask = (1u64 << ffi::kCGEventKeyDown)
        | (1u64 << ffi::kCGEventKeyUp)
        | (1u64 << ffi::kCGEventFlagsChanged);
    let mut asked = false;
    let tap = loop {
        // SAFETY: `state` is never freed, so it outlives the tap.
        let tap = unsafe {
            ffi::CGEventTapCreate(
                ffi::kCGSessionEventTap,
                ffi::kCGHeadInsertEventTap,
                ffi::kCGEventTapOptionListenOnly,
                mask,
                tap_callback,
                state.cast(),
            )
        };
        if !tap.is_null() {
            break tap;
        }
        if !asked {
            asked = true;
            diagnostics::log(Event::InputPermissionMissing);
            // Lists Clicky under Input Monitoring (and prompts the first time).
            // SAFETY: no preconditions.
            unsafe { ffi::CGRequestListenEventAccess() };
        }
        std::thread::sleep(PERMISSION_RETRY);
    };
    // SAFETY: `tap` is a valid mach port; the source and the tap live forever.
    unsafe {
        (*state).tap = tap;
        let source = ffi::CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
        ffi::CFRunLoopAddSource(ffi::CFRunLoopGetCurrent(), source, ffi::kCFRunLoopCommonModes);
        ffi::CGEventTapEnable(tap, true);
    }
    diagnostics::log(Event::InputTapStarted);
    // SAFETY: runs this thread's run loop forever.
    unsafe { ffi::CFRunLoopRun() };
}

/// Event-tap callback. No allocation and no locks on the key path.
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
        ffi::kCGEventFlagsChanged => Some(KeyEvent::FlagsChanged { keycode: keycode() }),
        _ => None,
    };
    if let Some(keycode) = key_event.and_then(|e| state.keys.press(e)) {
        state.voicer.press(keycode);
    }
    event
}
