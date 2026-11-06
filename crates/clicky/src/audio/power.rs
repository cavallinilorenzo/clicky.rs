//! Sleep and wake, from `NSWorkspace` notifications. The observers run on the
//! main thread (they are posted there and use no queue) and only raise a
//! [`signals`] bit for the audio control thread.

use std::ptr::NonNull;

use block2::RcBlock;
use objc2_app_kit::{
    NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::NSNotification;

use super::signals;

/// Observes sleep and wake for the life of the process. Call once, at launch;
/// notifications arrive only while the main run loop runs (the app, not the self-test).
pub fn observe_sleep_wake() {
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: AppKit's notification names are immutable statics.
    let names = unsafe {
        [
            (NSWorkspaceWillSleepNotification, signals::SLEEP),
            (NSWorkspaceDidWakeNotification, signals::WAKE),
        ]
    };
    for (name, bit) in names {
        let block = RcBlock::new(move |_: NonNull<NSNotification>| signals::raise(bit));
        // SAFETY: no object filter and no queue (the block runs on the posting
        // thread); the block is `Send`-safe, capturing only a `u32`.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &block)
        };
        // The observation lasts for the life of the app.
        std::mem::forget(token);
    }
}
