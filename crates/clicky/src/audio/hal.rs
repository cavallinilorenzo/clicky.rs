//! Thin CoreAudio HAL adapter: the default output device, its transport, rate,
//! format and buffer size, the system sleep policy, and property listeners.
//! The listener callback runs on CoreAudio's notification thread and only
//! raises a [`signals`] bit.

use std::ffi::c_void;
use std::fmt;

use super::device::Transport;
use super::{ffi, signals};

/// A failed CoreAudio call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsError {
    pub call: &'static str,
    pub status: ffi::OSStatus,
}

impl fmt::Display for OsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failed (OSStatus {})", self.call, self.status)
    }
}

pub fn check(call: &'static str, status: ffi::OSStatus) -> Result<(), OsError> {
    if status == 0 { Ok(()) } else { Err(OsError { call, status }) }
}

pub type DeviceId = ffi::AudioObjectID;

fn address(selector: u32, scope: u32) -> ffi::AudioObjectPropertyAddress {
    ffi::AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: ffi::kAudioObjectPropertyElementMain,
    }
}

const GLOBAL: u32 = ffi::kAudioObjectPropertyScopeGlobal;
const OUTPUT: u32 = ffi::kAudioObjectPropertyScopeOutput;

/// Reads a fixed-size property. `T` must be the C type CoreAudio writes.
fn get<T: Copy + Default>(
    call: &'static str,
    object: ffi::AudioObjectID,
    selector: u32,
    scope: u32,
) -> Result<T, OsError> {
    let mut value = T::default();
    let mut size = std::mem::size_of::<T>() as u32;
    // SAFETY: `value` is a valid out-pointer of `size` bytes.
    check(call, unsafe {
        ffi::AudioObjectGetPropertyData(
            object,
            &address(selector, scope),
            0,
            std::ptr::null(),
            &mut size,
            (&raw mut value).cast(),
        )
    })?;
    Ok(value)
}

fn set<T>(
    call: &'static str,
    object: ffi::AudioObjectID,
    selector: u32,
    scope: u32,
    value: &T,
) -> Result<(), OsError> {
    // SAFETY: `value` points to a live `T` of exactly the size passed.
    check(call, unsafe {
        ffi::AudioObjectSetPropertyData(
            object,
            &address(selector, scope),
            0,
            std::ptr::null(),
            std::mem::size_of::<T>() as u32,
            (value as *const T).cast(),
        )
    })
}

/// Lets the Mac idle-sleep while our output runs (ADR 0001). Without this,
/// `coreaudiod` holds a sleep-preventing assertion for any running output.
pub fn allow_system_sleep() -> Result<(), OsError> {
    let allowed: u32 = 1;
    set(
        "AudioObjectSetPropertyData(SleepingIsAllowed)",
        ffi::kAudioObjectSystemObject,
        ffi::kAudioHardwarePropertySleepingIsAllowed,
        GLOBAL,
        &allowed,
    )
}

/// The system's current default output device.
pub fn default_output_device() -> Result<DeviceId, OsError> {
    let call = "AudioObjectGetPropertyData(DefaultOutputDevice)";
    let id: DeviceId = get(
        call,
        ffi::kAudioObjectSystemObject,
        ffi::kAudioHardwarePropertyDefaultOutputDevice,
        GLOBAL,
    )?;
    if id == ffi::kAudioObjectUnknown { Err(OsError { call, status: -1 }) } else { Ok(id) }
}

pub fn transport(device: DeviceId) -> Transport {
    get::<u32>("TransportType", device, ffi::kAudioDevicePropertyTransportType, GLOBAL)
        .map_or(Transport::Other, Transport::from_code)
}

pub fn nominal_sample_rate(device: DeviceId) -> Option<f64> {
    get::<f64>("NominalSampleRate", device, ffi::kAudioDevicePropertyNominalSampleRate, GLOBAL)
        .ok()
        .filter(|rate| *rate > 0.0)
}

/// The device's output stream format: (sample rate, channels, bits).
pub fn output_format(device: DeviceId) -> Option<(f64, u32, u32)> {
    get::<ffi::AudioStreamBasicDescription>(
        "StreamFormat",
        device,
        ffi::kAudioDevicePropertyStreamFormat,
        OUTPUT,
    )
    .ok()
    .map(|f| (f.mSampleRate, f.mChannelsPerFrame, f.mBitsPerChannel))
}

pub fn is_alive(device: DeviceId) -> bool {
    get::<u32>("DeviceIsAlive", device, ffi::kAudioDevicePropertyDeviceIsAlive, GLOBAL)
        .is_ok_and(|alive| alive != 0)
}

/// This process's IO buffer size on `device`.
pub fn buffer_frames(device: DeviceId) -> Option<u32> {
    get::<u32>("BufferFrameSize", device, ffi::kAudioDevicePropertyBufferFrameSize, GLOBAL).ok()
}

pub fn buffer_range(device: DeviceId) -> Option<(f64, f64)> {
    get::<ffi::AudioValueRange>(
        "BufferFrameSizeRange",
        device,
        ffi::kAudioDevicePropertyBufferFrameSizeRange,
        GLOBAL,
    )
    .ok()
    .map(|r| (r.mMinimum, r.mMaximum))
}

/// Sets this process's IO buffer size on `device` (a per-process setting).
pub fn set_buffer_frames(device: DeviceId, frames: u32) -> Result<(), OsError> {
    set(
        "AudioObjectSetPropertyData(BufferFrameSize)",
        device,
        ffi::kAudioDevicePropertyBufferFrameSize,
        GLOBAL,
        &frames,
    )
}

const SYSTEM_WATCHED: [(u32, u32); 2] = [
    (ffi::kAudioHardwarePropertyDefaultOutputDevice, GLOBAL),
    (ffi::kAudioHardwarePropertyDevices, GLOBAL),
];

const DEVICE_WATCHED: [(u32, u32); 3] = [
    (ffi::kAudioDevicePropertyDeviceIsAlive, GLOBAL),
    (ffi::kAudioDevicePropertyNominalSampleRate, GLOBAL),
    (ffi::kAudioDevicePropertyStreamFormat, OUTPUT),
];

/// Listens for default-device and device-list changes for the life of the process.
pub fn watch_system() -> Result<(), OsError> {
    watch(ffi::kAudioObjectSystemObject, &SYSTEM_WATCHED)
}

/// Listens for `device` going away or changing its rate or format.
pub fn watch_device(device: DeviceId) -> Result<(), OsError> {
    watch(device, &DEVICE_WATCHED)
}

pub fn unwatch_device(device: DeviceId) {
    for &(selector, scope) in &DEVICE_WATCHED {
        // SAFETY: removes the listener `watch` added with the same arguments.
        unsafe {
            ffi::AudioObjectRemovePropertyListener(
                device,
                &address(selector, scope),
                on_change,
                std::ptr::null_mut(),
            )
        };
    }
}

fn watch(object: ffi::AudioObjectID, properties: &[(u32, u32)]) -> Result<(), OsError> {
    let mut result = Ok(());
    for &(selector, scope) in properties {
        // SAFETY: `on_change` is a plain function that touches no client data.
        let status = unsafe {
            ffi::AudioObjectAddPropertyListener(
                object,
                &address(selector, scope),
                on_change,
                std::ptr::null_mut(),
            )
        };
        if result.is_ok() {
            result = check("AudioObjectAddPropertyListener", status);
        }
    }
    result
}

/// Runs on CoreAudio's notification thread: signals the control thread, nothing else.
unsafe extern "C" fn on_change(
    _object: ffi::AudioObjectID,
    count: u32,
    addresses: *const ffi::AudioObjectPropertyAddress,
    _client: *mut c_void,
) -> ffi::OSStatus {
    if addresses.is_null() {
        return 0;
    }
    // SAFETY: CoreAudio passes `count` valid addresses.
    let addresses = unsafe { std::slice::from_raw_parts(addresses, count as usize) };
    let bits = addresses.iter().fold(0, |bits, a| bits | signals::for_selector(a.mSelector));
    if bits != 0 {
        signals::raise(bits);
    }
    0
}
