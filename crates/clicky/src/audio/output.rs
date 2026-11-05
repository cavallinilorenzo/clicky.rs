//! The CoreAudio adapter: an AUHAL default-output unit (it follows the
//! system's default output device) and the real-time render callback.
//!
//! Cold means stopped *and* uninitialised, so no device is held. Each start
//! re-reads the device's current sample rate. Device, sample-rate and
//! sleep/wake listeners, the watchdog and the built-in-only buffer size
//! request are issue #4.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/audio/mod.rs`.

use std::ffi::c_void;
use std::fmt;

use super::ffi;
use crate::engine::Shared;
use crate::engine::mixer::{Mixer, Play};
use crate::engine::ring::Consumer;

const MAX_FRAMES_PER_SLICE: u32 = 4096;
/// Used when the device reports no sample rate.
const FALLBACK_RATE: f64 = 48_000.0;

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

fn check(call: &'static str, status: ffi::OSStatus) -> Result<(), OsError> {
    if status == 0 { Ok(()) } else { Err(OsError { call, status }) }
}

/// Lets the Mac idle-sleep while our output runs (ADR 0001). Without this,
/// `coreaudiod` holds a sleep-preventing assertion for any running output.
pub fn allow_system_sleep() -> Result<(), OsError> {
    let address = ffi::AudioObjectPropertyAddress {
        mSelector: ffi::kAudioHardwarePropertySleepingIsAllowed,
        mScope: ffi::kAudioObjectPropertyScopeGlobal,
        mElement: ffi::kAudioObjectPropertyElementMain,
    };
    let allowed: u32 = 1;
    // SAFETY: valid address and a u32 payload of the size passed.
    check("AudioObjectSetPropertyData(SleepingIsAllowed)", unsafe {
        ffi::AudioObjectSetPropertyData(
            ffi::kAudioObjectSystemObject,
            &address,
            0,
            std::ptr::null(),
            std::mem::size_of_val(&allowed) as u32,
            (&raw const allowed).cast(),
        )
    })
}

/// State used by the CoreAudio IO thread while the unit runs, and by the
/// control thread only while it is stopped.
struct RenderState {
    shared: &'static Shared,
    mixer: Mixer,
    input: Consumer<Play>,
}

/// The output unit. Lives on the audio control thread.
pub struct Output {
    unit: Unit,
    /// Leaked on purpose: CoreAudio holds this pointer for the life of the unit,
    /// and the unit lives for the life of the process.
    state: *mut RenderState,
}

impl Output {
    /// Creates the unit up front (the first instantiation is slow), cold.
    pub fn new(shared: &'static Shared, input: Consumer<Play>) -> Result<Output, OsError> {
        let state = Box::into_raw(Box::new(RenderState {
            shared,
            mixer: Mixer::new(FALLBACK_RATE),
            input,
        }));
        let unit = Unit::new(state.cast())?;
        Ok(Output { unit, state })
    }

    /// Goes warm: matches the device's current sample rate and starts IO.
    /// Returns the sample rate.
    pub fn start(&mut self) -> Result<f64, OsError> {
        let rate = self.unit.initialize()?;
        // SAFETY: the unit is not running, so the IO thread does not use the state.
        unsafe { (*self.state).mixer.set_out_rate(rate) };
        if let Err(e) = self.unit.start() {
            self.unit.uninitialize();
            return Err(e);
        }
        Ok(rate)
    }

    /// Goes cold: stops IO and releases the device.
    pub fn stop(&mut self) {
        self.unit.stop();
        self.unit.uninitialize();
    }
}

/// An AUHAL default-output unit with our render callback attached.
struct Unit {
    au: ffi::AudioUnit,
    initialized: bool,
}

impl Unit {
    fn new(state: *mut c_void) -> Result<Unit, OsError> {
        let desc = ffi::AudioComponentDescription {
            componentType: ffi::kAudioUnitType_Output,
            componentSubType: ffi::kAudioUnitSubType_DefaultOutput,
            componentManufacturer: ffi::kAudioUnitManufacturer_Apple,
            ..Default::default()
        };
        // SAFETY: valid description pointer; a null result is handled.
        let component = unsafe { ffi::AudioComponentFindNext(std::ptr::null_mut(), &desc) };
        if component.is_null() {
            return Err(OsError { call: "AudioComponentFindNext", status: -1 });
        }
        let mut au = std::ptr::null_mut();
        // SAFETY: `au` is a valid out-pointer; the instance is disposed in `Drop`.
        check("AudioComponentInstanceNew", unsafe {
            ffi::AudioComponentInstanceNew(component, &mut au)
        })?;
        let unit = Unit { au, initialized: false };
        let callback = ffi::AURenderCallbackStruct { inputProc: render, inputProcRefCon: state };
        unit.set(
            "SetRenderCallback",
            ffi::kAudioUnitProperty_SetRenderCallback,
            ffi::kAudioUnitScope_Input,
            &callback,
        )?;
        Ok(unit)
    }

    /// Sets our input format to the device's current rate and initialises the unit.
    fn initialize(&mut self) -> Result<f64, OsError> {
        self.uninitialize();
        let mut hw = ffi::AudioStreamBasicDescription::default();
        let mut size = std::mem::size_of_val(&hw) as u32;
        // SAFETY: `hw` is a valid out-pointer of `size` bytes.
        check("AudioUnitGetProperty(StreamFormat)", unsafe {
            ffi::AudioUnitGetProperty(
                self.au,
                ffi::kAudioUnitProperty_StreamFormat,
                ffi::kAudioUnitScope_Output,
                0,
                (&raw mut hw).cast(),
                &mut size,
            )
        })?;
        let rate = if hw.mSampleRate > 0.0 { hw.mSampleRate } else { FALLBACK_RATE };
        let format = ffi::AudioStreamBasicDescription {
            mSampleRate: rate,
            mFormatID: ffi::kAudioFormatLinearPCM,
            mFormatFlags: ffi::kAudioFormatFlagIsFloat
                | ffi::kAudioFormatFlagIsPacked
                | ffi::kAudioFormatFlagIsNonInterleaved,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: 2,
            mBitsPerChannel: 32,
            mReserved: 0,
        };
        self.set(
            "SetProperty(StreamFormat)",
            ffi::kAudioUnitProperty_StreamFormat,
            ffi::kAudioUnitScope_Input,
            &format,
        )?;
        self.set(
            "SetProperty(MaximumFramesPerSlice)",
            ffi::kAudioUnitProperty_MaximumFramesPerSlice,
            ffi::kAudioUnitScope_Global,
            &MAX_FRAMES_PER_SLICE,
        )?;
        // SAFETY: the unit is configured and uninitialised.
        check("AudioUnitInitialize", unsafe { ffi::AudioUnitInitialize(self.au) })?;
        self.initialized = true;
        Ok(rate)
    }

    fn start(&self) -> Result<(), OsError> {
        // SAFETY: the unit is initialised.
        check("AudioOutputUnitStart", unsafe { ffi::AudioOutputUnitStart(self.au) })
    }

    fn stop(&self) {
        // SAFETY: stopping is valid in any state; it waits for the current render cycle.
        unsafe { ffi::AudioOutputUnitStop(self.au) };
    }

    fn uninitialize(&mut self) {
        if self.initialized {
            // SAFETY: the unit is stopped and initialised.
            unsafe { ffi::AudioUnitUninitialize(self.au) };
            self.initialized = false;
        }
    }

    fn set<T>(&self, call: &'static str, id: u32, scope: u32, value: &T) -> Result<(), OsError> {
        // SAFETY: `value` points to a live `T` of exactly the size passed.
        check(call, unsafe {
            ffi::AudioUnitSetProperty(
                self.au,
                id,
                scope,
                0,
                (value as *const T).cast(),
                std::mem::size_of::<T>() as u32,
            )
        })
    }
}

impl Drop for Unit {
    fn drop(&mut self) {
        self.stop();
        self.uninitialize();
        // SAFETY: tearing down the instance we created.
        unsafe { ffi::AudioComponentInstanceDispose(self.au) };
    }
}

/// The render callback. Real-time: no allocation, locks, logging or Objective-C.
unsafe extern "C" fn render(
    ref_con: *mut c_void,
    flags: *mut u32,
    _time: *const ffi::AudioTimeStamp,
    _bus: u32,
    frames: u32,
    data: *mut ffi::AudioBufferList,
) -> ffi::OSStatus {
    // SAFETY: CoreAudio hands back the RenderState pointer we registered, and only this
    // thread touches it while the unit runs.
    let state = unsafe { &mut *ref_con.cast::<RenderState>() };
    let shared = state.shared;
    shared.note_render();
    while let Some(play) = state.input.pop() {
        state.mixer.start(&play);
    }
    // SAFETY: CoreAudio provides `mNumberBuffers` valid buffers.
    let buffers = unsafe {
        std::slice::from_raw_parts_mut(
            (&raw mut (*data).mBuffers).cast::<ffi::AudioBuffer>(),
            (*data).mNumberBuffers as usize,
        )
    };
    let n = frames as usize;
    let sounding = match buffers {
        [left, right, ..] => {
            // SAFETY: each buffer holds at least `frames` f32 samples for our format.
            let (left, right) = unsafe { (channel(left, n), channel(right, n)) };
            state.mixer.render(left, right, shared.master_gain())
        }
        [mono] => {
            // SAFETY: as above.
            unsafe { channel(mono, n) }.fill(0.0);
            false
        }
        [] => false,
    };
    if !sounding {
        // SAFETY: `flags` is a valid in/out pointer for the duration of the call.
        unsafe { *flags |= ffi::kAudioUnitRenderAction_OutputIsSilence };
    }
    0
}

/// # Safety
/// `buffer.mData` must point to at least `min(frames, mDataByteSize / 4)` f32 samples
/// that nothing else accesses during the returned borrow.
unsafe fn channel<'a>(buffer: &mut ffi::AudioBuffer, frames: usize) -> &'a mut [f32] {
    let len = (buffer.mDataByteSize as usize / 4).min(frames);
    // SAFETY: guaranteed by the caller.
    unsafe { std::slice::from_raw_parts_mut(buffer.mData.cast::<f32>(), len) }
}
