//! How CoreAudio listeners and the sleep/wake observer reach the audio control
//! thread: each sets a bit and unparks the thread, nothing more. They run on
//! CoreAudio's notification thread or the main thread, so they must not do
//! any work there; the control thread reads and acts on the bits.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::Thread;

use super::ffi;

/// The system's default output device changed.
pub const DEFAULT_DEVICE: u32 = 1 << 0;
/// A device appeared or disappeared.
pub const DEVICES: u32 = 1 << 1;
/// The bound device's "is alive" flag changed.
pub const ALIVE: u32 = 1 << 2;
/// The bound device's nominal sample rate changed.
pub const SAMPLE_RATE: u32 = 1 << 3;
/// The bound device's output stream format changed.
pub const FORMAT: u32 = 1 << 4;
/// The Mac is going to sleep.
pub const SLEEP: u32 = 1 << 5;
/// The Mac woke up.
pub const WAKE: u32 = 1 << 6;
/// A rebuild asked for explicitly (the self-test).
pub const REQUEST: u32 = 1 << 7;

/// The bits that come from CoreAudio and are confirmed against the device.
pub const DEVICE_BITS: u32 = DEFAULT_DEVICE | DEVICES | ALIVE | SAMPLE_RATE | FORMAT | REQUEST;

static PENDING: AtomicU32 = AtomicU32::new(0);
static CONTROL: OnceLock<Thread> = OnceLock::new();

/// Registers the control thread; signals raised before this wait in `PENDING`.
pub fn attach(thread: Thread) {
    let _ = CONTROL.set(thread);
}

/// Sets `bits` and wakes the control thread. Lock-free and allocation-free.
pub fn raise(bits: u32) {
    PENDING.fetch_or(bits, Ordering::SeqCst);
    if let Some(thread) = CONTROL.get() {
        thread.unpark();
    }
}

/// Takes every pending bit.
pub fn take() -> u32 {
    PENDING.swap(0, Ordering::SeqCst)
}

/// The bit for a changed HAL property (0: none of ours).
pub fn for_selector(selector: u32) -> u32 {
    match selector {
        ffi::kAudioHardwarePropertyDefaultOutputDevice => DEFAULT_DEVICE,
        ffi::kAudioHardwarePropertyDevices => DEVICES,
        ffi::kAudioDevicePropertyDeviceIsAlive => ALIVE,
        ffi::kAudioDevicePropertyNominalSampleRate => SAMPLE_RATE,
        ffi::kAudioDevicePropertyStreamFormat => FORMAT,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_watched_property_has_its_own_bit() {
        let bits = [
            ffi::kAudioHardwarePropertyDefaultOutputDevice,
            ffi::kAudioHardwarePropertyDevices,
            ffi::kAudioDevicePropertyDeviceIsAlive,
            ffi::kAudioDevicePropertyNominalSampleRate,
            ffi::kAudioDevicePropertyStreamFormat,
        ]
        .map(for_selector);
        assert_eq!(bits, [DEFAULT_DEVICE, DEVICES, ALIVE, SAMPLE_RATE, FORMAT]);
        assert_eq!(for_selector(ffi::kAudioDevicePropertyBufferFrameSize), 0);
        assert_eq!(DEVICE_BITS & (SLEEP | WAKE), 0, "sleep and wake are not device changes");
    }
}
