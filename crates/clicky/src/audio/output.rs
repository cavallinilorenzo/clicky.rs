//! The CoreAudio adapter: an AUHAL output unit bound to the system's current
//! default output device, and the real-time render callback.
//!
//! Cold means stopped *and* uninitialised, so no device is held. Every start
//! binds the current default device at its current nominal sample rate, asks
//! for a small IO buffer only on built-in outputs, arms the pre-roll on
//! Bluetooth outputs, and listens to the device until it stops. A stop fades
//! the mix out within one render cycle first.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/audio/mod.rs`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::device::{self, Transport};
use super::hal::{self, DeviceId, OsError, OutputFormat, check};
use super::lifecycle::Trigger;
use super::{ffi, signals};
use crate::engine::Shared;
use crate::engine::mixer::{Mixer, Play};
use crate::engine::ring::Consumer;

const MAX_FRAMES_PER_SLICE: u32 = 4096;
/// Used when the device reports no sample rate.
const FALLBACK_RATE: f64 = 48_000.0;
/// How long a stop waits for the render callback to fade the mix out. One
/// render cycle is at most 4096 frames (~93 ms at 44.1 kHz), usually far less.
const FADE_TIMEOUT: Duration = Duration::from_millis(100);

/// Atomics shared by the control thread and the render callback.
pub struct RenderSignals {
    /// Render cycles so far (the watchdog's evidence of progress).
    renders: AtomicU64,
    /// Asked by the control thread: fade the mix to silence and hold it there.
    fade: AtomicBool,
    /// Set by the render callback once a faded block has been rendered.
    faded: AtomicBool,
}

impl RenderSignals {
    pub fn new() -> RenderSignals {
        RenderSignals {
            renders: AtomicU64::new(0),
            fade: AtomicBool::new(false),
            faded: AtomicBool::new(false),
        }
    }

    pub fn renders(&self) -> u64 {
        self.renders.load(Ordering::Relaxed)
    }
}

/// What one render cycle produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Block {
    /// Voices sounded.
    Sound,
    /// Nothing sounded: zeros, which CoreAudio may skip where the transport
    /// allows it (`RenderState::flags_silence`).
    Silence,
    /// Pre-roll silence, written out on purpose so the device keeps streaming.
    PreRoll,
}

/// State used by the CoreAudio IO thread while the unit runs, and by the
/// control thread only while it is stopped.
pub struct RenderState {
    shared: &'static Shared,
    signals: &'static RenderSignals,
    mixer: Mixer,
    input: Consumer<Play>,
    /// Frames of pre-roll silence left before queued voices may start.
    pre_roll: u32,
    /// Whether silent blocks may be flagged as silence (not on Bluetooth,
    /// `Transport::flags_silence`).
    flag_silence: bool,
}

impl RenderState {
    pub fn new(
        shared: &'static Shared,
        signals: &'static RenderSignals,
        input: Consumer<Play>,
    ) -> RenderState {
        RenderState {
            shared,
            signals,
            mixer: Mixer::new(FALLBACK_RATE),
            input,
            pre_roll: 0,
            flag_silence: true,
        }
    }

    /// Readies a start at `rate`. Only while the unit is stopped. A fresh mixer
    /// means the first voice plays at full gain from its first sample, with no
    /// ramp left over from the fade that ended the previous run.
    /// `flag_silence` is the transport's [`Transport::flags_silence`].
    pub fn prepare(&mut self, rate: f64, pre_roll_frames: u32, flag_silence: bool) {
        self.mixer = Mixer::new(rate);
        self.pre_roll = pre_roll_frames;
        self.flag_silence = flag_silence;
        self.signals.fade.store(false, Ordering::Release);
        self.signals.faded.store(false, Ordering::Release);
    }

    /// Whether the render callback flags `block` as silence, so CoreAudio
    /// may skip it. Only silent blocks, and never on an output that must keep
    /// streaming (Bluetooth): those get the zeros `fill` wrote.
    pub fn flags_silence(&self, block: Block) -> bool {
        block == Block::Silence && self.flag_silence
    }

    /// One render cycle into `left` and `right`. Real-time: no allocation,
    /// locks, logging or Objective-C.
    pub fn fill(&mut self, left: &mut [f32], right: &mut [f32]) -> Block {
        self.signals.renders.fetch_add(1, Ordering::Relaxed);
        self.shared.note_render();
        if self.signals.fade.load(Ordering::Acquire) {
            // Ramps from the last block's gain to 0 and drops the voices. Queued
            // presses stay in the ring for the next start.
            let sounding = self.mixer.render(left, right, 0.0);
            self.signals.faded.store(true, Ordering::Release);
            return if sounding { Block::Sound } else { Block::Silence };
        }
        if self.pre_roll > 0 {
            // The pre-roll ends with this block; queued voices start on the next.
            self.pre_roll = self.pre_roll.saturating_sub(left.len() as u32);
            left.fill(0.0);
            right.fill(0.0);
            return Block::PreRoll;
        }
        while let Some(play) = self.input.pop() {
            self.mixer.start(&play);
        }
        if self.mixer.render(left, right, self.shared.master_gain()) {
            Block::Sound
        } else {
            Block::Silence
        }
    }

    /// A cycle the mix cannot fill (an unexpected channel layout): silence,
    /// but still counted.
    fn fill_silence(&mut self, out: &mut [f32]) {
        self.signals.renders.fetch_add(1, Ordering::Relaxed);
        self.shared.note_render();
        out.fill(0.0);
    }
}

/// What a start bound.
#[derive(Clone, Copy, Debug)]
pub struct Started {
    pub device: DeviceId,
    pub transport: Transport,
    pub sample_rate: f64,
    /// This process's IO buffer on the device after the start.
    pub buffer_frames: Option<u32>,
    pub pre_roll: Duration,
}

/// The device a running output is bound to, as it was at the start.
#[derive(Clone, Copy, Debug)]
struct Bound {
    device: DeviceId,
    rate: f64,
    format: Option<OutputFormat>,
}

/// The output unit. Lives on the audio control thread.
pub struct Output {
    unit: Unit,
    /// Leaked on purpose: CoreAudio holds this pointer for the life of the unit,
    /// and the output lives for the life of the process.
    state: *mut RenderState,
    signals: &'static RenderSignals,
    bound: Option<Bound>,
}

impl Output {
    /// Creates the unit up front (the first instantiation is slow), cold, and
    /// starts listening for default-device changes.
    pub fn new(shared: &'static Shared, input: Consumer<Play>) -> Result<Output, OsError> {
        let signals: &'static RenderSignals = Box::leak(Box::new(RenderSignals::new()));
        let state = Box::into_raw(Box::new(RenderState::new(shared, signals, input)));
        let unit = Unit::new(state.cast())?;
        hal::watch_system()?;
        Ok(Output { unit, state, signals, bound: None })
    }

    /// Render cycles so far.
    pub fn renders(&self) -> u64 {
        self.signals.renders()
    }

    /// Goes warm on the current default output device at its current format.
    pub fn start(&mut self) -> Result<Started, OsError> {
        let device = hal::default_output_device()?;
        let transport = hal::transport(device);
        if let Some(frames) =
            transport.buffer_request().map(|r| device::clamp_buffer(r, hal::buffer_range(device)))
            && hal::buffer_frames(device) != Some(frames)
        {
            // Best effort: the default buffer only costs latency.
            let _ = hal::set_buffer_frames(device, frames);
        }
        let nominal = hal::nominal_sample_rate(device);
        let rate = match self.unit.initialize(device, nominal) {
            Ok(rate) => rate,
            Err(_) => {
                // A unit that was bound to a vanished device can stay broken:
                // start over with a fresh one.
                self.unit = Unit::new(self.state.cast())?;
                self.unit.initialize(device, nominal)?
            }
        };
        let pre_roll = transport.pre_roll();
        // A one-channel device gets only our left channel (AUHAL drops the rest).
        let mono = self.unit.device_channels() == Some(1);
        // SAFETY: the unit is not running, so the IO thread does not use the state.
        unsafe {
            (*self.state).prepare(rate, device::frames(pre_roll, rate), transport.flags_silence());
            (*self.state).mixer.set_mono(mono);
        }
        // Best effort: without the listeners the watchdog still catches a dead device.
        let _ = hal::watch_device(device);
        if let Err(e) = self.unit.start() {
            hal::unwatch_device(device);
            self.unit.uninitialize();
            return Err(e);
        }
        self.bound = Some(Bound { device, rate, format: hal::output_format(device) });
        Ok(Started {
            device,
            transport,
            sample_rate: rate,
            buffer_frames: hal::buffer_frames(device),
            pre_roll,
        })
    }

    /// Goes cold: fades out, stops IO and releases the device.
    pub fn stop(&mut self) {
        if let Some(bound) = self.bound.take() {
            self.fade_out();
            hal::unwatch_device(bound.device);
        }
        self.unit.stop();
        self.unit.uninitialize();
    }

    fn fade_out(&self) {
        self.signals.fade.store(true, Ordering::Release);
        let deadline = Instant::now() + FADE_TIMEOUT;
        while !self.signals.faded.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Which of the signalled changes (`signals::DEVICE_BITS`) really affect
    /// the output, most severe first. Spurious notifications (the same device,
    /// the same rate) are dropped. While cold only a new default device is
    /// reported, for the log: the next start binds it anyway.
    pub fn confirm(&self, bits: u32) -> Option<Trigger> {
        let mut found = Vec::new();
        if bits & signals::REQUEST != 0 {
            found.push(Trigger::Requested);
        }
        let Some(bound) = self.bound else {
            if bits & signals::DEFAULT_DEVICE != 0 {
                found.push(Trigger::DefaultDeviceChanged);
            }
            return found.into_iter().min();
        };
        if bits & (signals::DEVICES | signals::ALIVE) != 0 && !hal::is_alive(bound.device) {
            found.push(Trigger::DeviceGone);
        }
        if bits & signals::DEFAULT_DEVICE != 0
            && hal::default_output_device().is_ok_and(|d| d != bound.device)
        {
            found.push(Trigger::DefaultDeviceChanged);
        }
        if bits & signals::SAMPLE_RATE != 0
            && hal::nominal_sample_rate(bound.device).is_some_and(|r| r != bound.rate)
        {
            found.push(Trigger::SampleRateChanged);
        }
        if bits & signals::FORMAT != 0 && hal::output_format(bound.device) != bound.format {
            found.push(Trigger::FormatChanged);
        }
        found.into_iter().min()
    }
}

/// An AUHAL output unit with our render callback attached.
struct Unit {
    au: ffi::AudioUnit,
    initialized: bool,
}

impl Unit {
    fn new(state: *mut c_void) -> Result<Unit, OsError> {
        let desc = ffi::AudioComponentDescription {
            componentType: ffi::kAudioUnitType_Output,
            componentSubType: ffi::kAudioUnitSubType_HALOutput,
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

    /// Binds `device`, sets our input format to its rate (`nominal`, else the
    /// device-side format of the unit) and initialises the unit. Returns the rate.
    fn initialize(&mut self, device: DeviceId, nominal: Option<f64>) -> Result<f64, OsError> {
        self.uninitialize();
        self.set(
            "SetProperty(CurrentDevice)",
            ffi::kAudioOutputUnitProperty_CurrentDevice,
            ffi::kAudioUnitScope_Global,
            &device,
        )?;
        let rate = match nominal {
            Some(rate) => rate,
            None => self.device_side_rate()?,
        };
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

    fn device_side_rate(&self) -> Result<f64, OsError> {
        let hw = self.device_side_format()?;
        Ok(if hw.mSampleRate > 0.0 { hw.mSampleRate } else { FALLBACK_RATE })
    }

    /// The bound device's channel count (all its output streams together).
    fn device_channels(&self) -> Option<u32> {
        self.device_side_format().ok().map(|f| f.mChannelsPerFrame).filter(|&n| n > 0)
    }

    fn device_side_format(&self) -> Result<ffi::AudioStreamBasicDescription, OsError> {
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
        Ok(hw)
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
    // SAFETY: CoreAudio provides `mNumberBuffers` valid buffers.
    let buffers = unsafe {
        std::slice::from_raw_parts_mut(
            (&raw mut (*data).mBuffers).cast::<ffi::AudioBuffer>(),
            (*data).mNumberBuffers as usize,
        )
    };
    let n = frames as usize;
    let block = match buffers {
        [left, right, ..] => {
            // SAFETY: each buffer holds at least `frames` f32 samples for our format.
            let (left, right) = unsafe { (channel(left, n), channel(right, n)) };
            state.fill(left, right)
        }
        [mono] => {
            // SAFETY: as above.
            state.fill_silence(unsafe { channel(mono, n) });
            Block::Silence
        }
        [] => Block::Silence,
    };
    if state.flags_silence(block) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::MASTER_GAIN;
    use crate::engine::ring::{self, Producer};

    static CLIP: [i16; 4_096] = [16_384; 4_096];
    /// One sample of `CLIP` at the master gain, unramped.
    const LEVEL: f32 = 0.5 * MASTER_GAIN;

    fn press() -> Play {
        Play { samples: &CLIP, src_rate: 48_000, gain: 1.0, pan: 0.0, pitch: 1.0 }
    }

    fn render_state() -> (RenderState, Producer<Play>, &'static RenderSignals) {
        let shared: &'static Shared = Box::leak(Box::new(Shared::new(true)));
        let signals: &'static RenderSignals = Box::leak(Box::new(RenderSignals::new()));
        let (producer, consumer) = ring::ring(16);
        (RenderState::new(shared, signals, consumer), producer, signals)
    }

    fn block(state: &mut RenderState, frames: usize) -> (Block, Vec<f32>) {
        let (mut left, mut right) = (vec![9.0; frames], vec![9.0; frames]);
        let block = state.fill(&mut left, &mut right);
        assert_eq!(left, right);
        (block, left)
    }

    #[test]
    fn a_press_queued_during_a_cold_start_plays_at_full_level_on_the_first_render() {
        let (mut state, mut ring, _) = render_state();
        assert!(ring.push(press()), "pressed while the output was still starting");
        state.prepare(48_000.0, 0, true);
        let (block, out) = block(&mut state, 128);
        assert_eq!(block, Block::Sound);
        assert_eq!(out[0], LEVEL, "no fade-in on the first sample");
        assert!(out.iter().all(|s| *s == LEVEL));
    }

    #[test]
    fn silence_is_reported_as_silence() {
        let (mut state, _ring, _) = render_state();
        state.prepare(48_000.0, 0, true);
        let (block, out) = block(&mut state, 64);
        assert_eq!(block, Block::Silence);
        assert!(out.iter().all(|s| *s == 0.0));
        assert!(state.flags_silence(block), "CoreAudio may skip it");
    }

    #[test]
    fn a_warm_output_that_must_keep_streaming_never_flags_silence() {
        // Bluetooth (ADR 0001): the headphones get real zeros through the hold window.
        let (mut state, mut ring, _) = render_state();
        state.prepare(48_000.0, 0, false);
        let (silent, out) = block(&mut state, 64);
        assert_eq!(silent, Block::Silence);
        assert!(out.iter().all(|s| *s == 0.0), "true zeros");
        assert!(!state.flags_silence(silent));
        ring.push(press());
        let (sound, _) = block(&mut state, 64);
        assert!(!state.flags_silence(sound));
    }

    #[test]
    fn the_pre_roll_holds_queued_presses_back_then_plays_them() {
        let (mut state, mut ring, _) = render_state();
        ring.push(press());
        state.prepare(48_000.0, 200, true);
        for _ in 0..2 {
            let (block, out) = block(&mut state, 128);
            assert_eq!(block, Block::PreRoll, "written-out silence, not flagged as silence");
            assert!(out.iter().all(|s| *s == 0.0));
        }
        ring.push(press());
        let (block, out) = block(&mut state, 128);
        assert_eq!(block, Block::Sound);
        assert_eq!(out[0], 2.0 * LEVEL, "both presses, from their first sample");
    }

    #[test]
    fn a_fade_ramps_to_silence_within_one_block_and_keeps_queued_presses() {
        let (mut state, mut ring, signals) = render_state();
        state.prepare(48_000.0, 0, true);
        ring.push(press());
        assert_eq!(block(&mut state, 64).0, Block::Sound);

        signals.fade.store(true, Ordering::Release);
        ring.push(press());
        let (_, fade) = block(&mut state, 64);
        assert!(fade.windows(2).all(|w| w[1] <= w[0]), "ramps down: {fade:?}");
        assert!(fade.last().unwrap().abs() < 1e-6, "ends below -120 dBFS: {fade:?}");
        assert!(signals.faded.load(Ordering::Acquire));
        let (block_after, after) = block(&mut state, 64);
        assert_eq!(block_after, Block::Silence, "stays silent until the stop");
        assert!(after.iter().all(|s| *s == 0.0));

        state.prepare(44_100.0, 0, true);
        assert!(!signals.fade.load(Ordering::Acquire));
        let (block_rebuilt, rebuilt) = block(&mut state, 64);
        assert_eq!(block_rebuilt, Block::Sound, "the press made during the fade plays after it");
        assert!(rebuilt[0] > 0.0);
    }

    #[test]
    fn every_render_cycle_counts_for_the_watchdog() {
        let (mut state, _ring, signals) = render_state();
        state.prepare(48_000.0, 64, true);
        block(&mut state, 64);
        block(&mut state, 64);
        signals.fade.store(true, Ordering::Release);
        block(&mut state, 64);
        state.fill_silence(&mut [0.0; 8]);
        assert_eq!(signals.renders(), 4);
    }
}
