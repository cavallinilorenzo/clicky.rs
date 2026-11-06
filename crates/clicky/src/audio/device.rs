//! What kind of output device the output runs on, and what that means for it
//! (ADR 0001): a small hardware buffer only on built-in outputs; a pre-roll,
//! and true zeros instead of flagged silence while warm, only on Bluetooth
//! outputs. Pure: the CoreAudio queries live in `hal`.

use std::time::Duration;

/// Silence played on a Bluetooth output after each (re)start, before the first
/// voice, so that the headphones are streaming when it arrives.
///
/// PROVISIONAL (ADR 0001): to be set by Lorenzo's listening tests (device ×
/// interval × 10). Cold starts on AirPods measured ~470–490 ms to the first
/// render; A2DP sinks commonly swallow the first ~100–250 ms after the stream
/// resumes, so 250 ms errs on the side of the press being heard. It only
/// delays the first press after the output went cold (a pause of at least
/// the hold window), or after a rebuild.
pub const BLUETOOTH_PRE_ROLL: Duration = Duration::from_millis(250);

/// The IO buffer this process asks for on built-in outputs: 128 frames is
/// 2.7–2.9 ms at 44.1–48 kHz, so a warm press reaches the speakers well within
/// 15 ms. It is a per-process setting of the device, not a system-wide one.
pub const BUILT_IN_BUFFER_FRAMES: u32 = 128;

/// The device's transport, from `kAudioDevicePropertyTransportType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    BuiltIn,
    Bluetooth,
    Usb,
    AirPlay,
    /// HDMI or DisplayPort (external displays).
    Display,
    /// Aggregate or virtual devices (e.g. multi-output, loopback drivers).
    Virtual,
    /// Thunderbolt, PCI, FireWire, AVB, or anything unknown.
    Other,
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

impl Transport {
    pub fn from_code(code: u32) -> Transport {
        const BUILT_IN: u32 = fourcc(b"bltn");
        const BLUETOOTH: u32 = fourcc(b"blue");
        const BLUETOOTH_LE: u32 = fourcc(b"blea");
        const USB: u32 = fourcc(b"usb ");
        const AIRPLAY: u32 = fourcc(b"airp");
        const HDMI: u32 = fourcc(b"hdmi");
        const DISPLAY_PORT: u32 = fourcc(b"dprt");
        const AGGREGATE: u32 = fourcc(b"grup");
        const VIRTUAL: u32 = fourcc(b"virt");
        match code {
            BUILT_IN => Transport::BuiltIn,
            BLUETOOTH | BLUETOOTH_LE => Transport::Bluetooth,
            USB => Transport::Usb,
            AIRPLAY => Transport::AirPlay,
            HDMI | DISPLAY_PORT => Transport::Display,
            AGGREGATE | VIRTUAL => Transport::Virtual,
            _ => Transport::Other,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Transport::BuiltIn => "built-in",
            Transport::Bluetooth => "Bluetooth",
            Transport::Usb => "USB",
            Transport::AirPlay => "AirPlay",
            Transport::Display => "display",
            Transport::Virtual => "virtual",
            Transport::Other => "other",
        }
    }

    /// The IO buffer size to request, if any: built-in outputs only. Bluetooth
    /// and USB devices keep their own buffer (smaller ones risk dropouts there).
    pub fn buffer_request(self) -> Option<u32> {
        (self == Transport::BuiltIn).then_some(BUILT_IN_BUFFER_FRAMES)
    }

    /// Silence before the first voice after a start.
    pub fn pre_roll(self) -> Duration {
        if self == Transport::Bluetooth { BLUETOOTH_PRE_ROLL } else { Duration::ZERO }
    }

    /// Whether a warm output may flag a silent block as silence
    /// (`kAudioUnitRenderAction_OutputIsSilence`), which lets CoreAudio skip
    /// it. Not on Bluetooth: there the headphones get true zeros through the
    /// hold window, so the A2DP stream does not pause and the next press is
    /// not swallowed by its resumption (ADR 0001).
    pub fn flags_silence(self) -> bool {
        self != Transport::Bluetooth
    }
}

/// Fits a requested buffer size into the device's allowed range
/// (`kAudioDevicePropertyBufferFrameSizeRange`, unknown: `None`).
pub fn clamp_buffer(requested: u32, range: Option<(f64, f64)>) -> u32 {
    match range {
        Some((min, max)) if min >= 1.0 && min <= max => {
            f64::from(requested).clamp(min.ceil(), max.floor()) as u32
        }
        _ => requested,
    }
}

/// `duration` as a whole number of frames at `rate`.
pub fn frames(duration: Duration, rate: f64) -> u32 {
    if rate > 0.0 { (duration.as_secs_f64() * rate).round() as u32 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transports_are_read_from_their_core_audio_codes() {
        // Values of kAudioDeviceTransportType* in AudioHardwareBase.h.
        assert_eq!(Transport::from_code(0x626C_746E), Transport::BuiltIn); // 'bltn'
        assert_eq!(Transport::from_code(0x626C_7565), Transport::Bluetooth); // 'blue'
        assert_eq!(Transport::from_code(0x626C_6561), Transport::Bluetooth); // 'blea' (LE)
        assert_eq!(Transport::from_code(0x7573_6220), Transport::Usb); // 'usb '
        assert_eq!(Transport::from_code(0x6169_7270), Transport::AirPlay); // 'airp'
        assert_eq!(Transport::from_code(0x6864_6D69), Transport::Display); // 'hdmi'
        assert_eq!(Transport::from_code(0x6470_7274), Transport::Display); // 'dprt'
        assert_eq!(Transport::from_code(0x6772_7570), Transport::Virtual); // 'grup'
        assert_eq!(Transport::from_code(0x7669_7274), Transport::Virtual); // 'virt'
        assert_eq!(Transport::from_code(0x7468_756E), Transport::Other); // 'thun'
        assert_eq!(Transport::from_code(0), Transport::Other);
    }

    #[test]
    fn only_built_in_outputs_get_a_small_buffer() {
        assert_eq!(Transport::BuiltIn.buffer_request(), Some(BUILT_IN_BUFFER_FRAMES));
        for other in [
            Transport::Bluetooth,
            Transport::Usb,
            Transport::AirPlay,
            Transport::Display,
            Transport::Virtual,
            Transport::Other,
        ] {
            assert_eq!(other.buffer_request(), None, "{other:?}");
        }
    }

    #[test]
    fn only_bluetooth_outputs_get_a_pre_roll() {
        assert_eq!(Transport::Bluetooth.pre_roll(), BLUETOOTH_PRE_ROLL);
        assert!(BLUETOOTH_PRE_ROLL > Duration::ZERO);
        for other in [Transport::BuiltIn, Transport::Usb, Transport::Display, Transport::Other] {
            assert_eq!(other.pre_roll(), Duration::ZERO, "{other:?}");
        }
    }

    #[test]
    fn bluetooth_outputs_get_real_zeros_so_the_headphones_keep_streaming() {
        assert!(!Transport::Bluetooth.flags_silence());
        for other in [
            Transport::BuiltIn,
            Transport::Usb,
            Transport::AirPlay,
            Transport::Display,
            Transport::Virtual,
            Transport::Other,
        ] {
            assert!(other.flags_silence(), "{other:?}");
        }
    }

    #[test]
    fn a_buffer_request_is_kept_within_the_devices_range() {
        assert_eq!(clamp_buffer(128, Some((14.0, 4096.0))), 128);
        assert_eq!(clamp_buffer(128, Some((256.0, 4096.0))), 256);
        assert_eq!(clamp_buffer(128, Some((15.0, 64.0))), 64);
        assert_eq!(clamp_buffer(128, None), 128);
        assert_eq!(clamp_buffer(128, Some((4096.0, 14.0))), 128, "a nonsense range is ignored");
    }

    #[test]
    fn durations_become_frames_at_the_device_rate() {
        assert_eq!(frames(Duration::from_millis(250), 48_000.0), 12_000);
        assert_eq!(frames(Duration::from_millis(250), 44_100.0), 11_025);
        assert_eq!(frames(Duration::from_millis(1), 96_000.0), 96);
        assert_eq!(frames(Duration::ZERO, 48_000.0), 0);
        assert_eq!(frames(Duration::from_secs(1), 0.0), 0);
    }
}
