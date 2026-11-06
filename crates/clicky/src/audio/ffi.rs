//! Minimal AudioToolbox and CoreAudio FFI for an AUHAL output unit and the
//! HAL device properties and listeners the output lifecycle needs.
//!
//! Derived from Rustyvibes 2 (MIT, © 2021–2026 Kunal Bagaria,
//! https://github.com/KunalBagaria/rustyvibes-v2), `crates/rustyvibes/src/audio/ffi.rs`.

#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

pub type OSStatus = i32;
pub type AudioComponent = *mut c_void;
pub type AudioUnit = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioComponentDescription {
    pub componentType: u32,
    pub componentSubType: u32,
    pub componentManufacturer: u32,
    pub componentFlags: u32,
    pub componentFlagsMask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: u32,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

#[repr(C)]
pub struct AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

/// Variable-length in C: `mNumberBuffers` entries follow.
#[repr(C)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [AudioBuffer; 1],
}

/// Opaque here: the render callback never reads the timestamp.
#[repr(C)]
pub struct AudioTimeStamp {
    _private: [u8; 0],
}

pub type AURenderCallback = unsafe extern "C" fn(
    *mut c_void,
    *mut u32,
    *const AudioTimeStamp,
    u32,
    u32,
    *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
pub struct AURenderCallbackStruct {
    pub inputProc: AURenderCallback,
    pub inputProcRefCon: *mut c_void,
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

pub const kAudioUnitType_Output: u32 = fourcc(b"auou");
pub const kAudioUnitSubType_HALOutput: u32 = fourcc(b"ahal");
pub const kAudioUnitManufacturer_Apple: u32 = fourcc(b"appl");
pub const kAudioFormatLinearPCM: u32 = fourcc(b"lpcm");
pub const kAudioFormatFlagIsFloat: u32 = 1 << 0;
pub const kAudioFormatFlagIsPacked: u32 = 1 << 3;
pub const kAudioFormatFlagIsNonInterleaved: u32 = 1 << 5;
pub const kAudioUnitProperty_StreamFormat: u32 = 8;
pub const kAudioUnitProperty_MaximumFramesPerSlice: u32 = 14;
pub const kAudioUnitProperty_SetRenderCallback: u32 = 23;
pub const kAudioUnitScope_Global: u32 = 0;
pub const kAudioUnitScope_Input: u32 = 1;
pub const kAudioUnitScope_Output: u32 = 2;
pub const kAudioUnitRenderAction_OutputIsSilence: u32 = 1 << 4;
pub const kAudioOutputUnitProperty_CurrentDevice: u32 = 2000;

#[link(name = "AudioToolbox", kind = "framework")]
unsafe extern "C" {
    pub fn AudioComponentFindNext(
        component: AudioComponent,
        desc: *const AudioComponentDescription,
    ) -> AudioComponent;
    pub fn AudioComponentInstanceNew(component: AudioComponent, out: *mut AudioUnit) -> OSStatus;
    pub fn AudioComponentInstanceDispose(unit: AudioUnit) -> OSStatus;
    pub fn AudioUnitInitialize(unit: AudioUnit) -> OSStatus;
    pub fn AudioUnitUninitialize(unit: AudioUnit) -> OSStatus;
    pub fn AudioUnitSetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> OSStatus;
    pub fn AudioUnitGetProperty(
        unit: AudioUnit,
        id: u32,
        scope: u32,
        element: u32,
        data: *mut c_void,
        size: *mut u32,
    ) -> OSStatus;
    pub fn AudioOutputUnitStart(unit: AudioUnit) -> OSStatus;
    pub fn AudioOutputUnitStop(unit: AudioUnit) -> OSStatus;
}

pub type AudioObjectID = u32;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioObjectPropertyAddress {
    pub mSelector: u32,
    pub mScope: u32,
    pub mElement: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioValueRange {
    pub mMinimum: f64,
    pub mMaximum: f64,
}

pub type AudioObjectPropertyListenerProc = unsafe extern "C" fn(
    AudioObjectID,
    u32,
    *const AudioObjectPropertyAddress,
    *mut c_void,
) -> OSStatus;

pub const kAudioObjectUnknown: AudioObjectID = 0;
pub const kAudioObjectSystemObject: AudioObjectID = 1;
pub const kAudioObjectPropertyScopeGlobal: u32 = fourcc(b"glob");
pub const kAudioObjectPropertyScopeOutput: u32 = fourcc(b"outp");
pub const kAudioObjectPropertyElementMain: u32 = 0;
pub const kAudioHardwarePropertySleepingIsAllowed: u32 = fourcc(b"slep");
pub const kAudioHardwarePropertyDefaultOutputDevice: u32 = fourcc(b"dOut");
pub const kAudioHardwarePropertyDevices: u32 = fourcc(b"dev#");
pub const kAudioDevicePropertyDeviceIsAlive: u32 = fourcc(b"livn");
pub const kAudioDevicePropertyNominalSampleRate: u32 = fourcc(b"nsrt");
pub const kAudioDevicePropertyStreamFormat: u32 = fourcc(b"sfmt");
pub const kAudioDevicePropertyTransportType: u32 = fourcc(b"tran");
pub const kAudioDevicePropertyBufferFrameSize: u32 = fourcc(b"fsiz");
pub const kAudioDevicePropertyBufferFrameSizeRange: u32 = fourcc(b"fsz#");

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    pub fn AudioObjectSetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: u32,
        data: *const c_void,
    ) -> OSStatus;
    pub fn AudioObjectGetPropertyData(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> OSStatus;
    pub fn AudioObjectAddPropertyListener(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: AudioObjectPropertyListenerProc,
        client: *mut c_void,
    ) -> OSStatus;
    pub fn AudioObjectRemovePropertyListener(
        object: AudioObjectID,
        address: *const AudioObjectPropertyAddress,
        listener: AudioObjectPropertyListenerProc,
        client: *mut c_void,
    ) -> OSStatus;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_the_c_headers() {
        assert_eq!(std::mem::size_of::<AudioStreamBasicDescription>(), 40);
        assert_eq!(std::mem::size_of::<AudioComponentDescription>(), 20);
        assert_eq!(std::mem::size_of::<AudioBuffer>(), 16);
        assert_eq!(std::mem::size_of::<AudioBufferList>(), 24);
        assert_eq!(kAudioUnitType_Output, 0x6175_6F75);
        assert_eq!(std::mem::size_of::<AudioObjectPropertyAddress>(), 12);
        assert_eq!(kAudioHardwarePropertySleepingIsAllowed, 0x736C_6570);
        assert_eq!(std::mem::size_of::<AudioValueRange>(), 16);
        // Values from AudioHardware.h / AudioHardwareBase.h / AudioUnitProperties.h.
        assert_eq!(kAudioUnitSubType_HALOutput, 0x6168_616C);
        assert_eq!(kAudioHardwarePropertyDefaultOutputDevice, 0x644F_7574);
        assert_eq!(kAudioHardwarePropertyDevices, 0x6465_7623);
        assert_eq!(kAudioDevicePropertyDeviceIsAlive, 0x6C69_766E);
        assert_eq!(kAudioDevicePropertyNominalSampleRate, 0x6E73_7274);
        assert_eq!(kAudioDevicePropertyBufferFrameSize, 0x6673_697A);
        assert_eq!(kAudioObjectPropertyScopeOutput, 0x6F75_7470);
    }
}
