//! The optional diagnostics log: what the app did (audio start/stop with
//! timings, input state, errors), with timestamps, in
//! `~/Library/Logs/Clicky/Clicky.log`. Off by default; toggled from the
//! Option-held "Diagnostics Log" menu item.
//!
//! Privacy by construction: only the [`Event`] variants below can be logged,
//! and none of them carries a keycode or a character. Never add one that does.
//! New areas (device changes in #4, permission in #9) add variants here.
//!
//! Never call [`log`] from the render callback: it takes a lock and writes a file.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::settings::Setting;

/// Whether the diagnostics log is on.
pub const LOG_ENABLED: Setting<bool> = Setting { key: "DiagnosticsLog", default: false };

/// Set to `1` to mirror log lines to stderr (useful with `cargo run`).
pub const STDERR_ENV: &str = "CLICKY_LOG_STDERR";

/// A log file larger than this is moved to `Clicky.log.1` when logging starts.
const ROTATE_BYTES: u64 = 2_000_000;

/// Everything the log can record.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Launched {
        version: &'static str,
    },
    LogStarted {
        version: &'static str,
    },
    LogStopped,
    SoundpackLoaded {
        id: String,
        clips: usize,
        sample_rate: u32,
    },
    /// The output went warm. `start_ms` is the start call; `first_render_ms`
    /// is from the start request to the first render cycle (`None`: none within the wait).
    AudioStarted {
        sample_rate: f64,
        start_ms: f64,
        first_render_ms: Option<f64>,
    },
    /// The output went cold after being warm for `warm_secs`.
    AudioStopped {
        reason: &'static str,
        warm_secs: f64,
        dropped: u64,
    },
    SleepAllowed,
    InputTapStarted,
    InputPermissionMissing,
    InputTapReenabled {
        cause: &'static str,
    },
    /// Something failed. `detail` must never contain key data.
    Error {
        context: &'static str,
        detail: String,
    },
    /// Input of a kind Clicky does not recognise (logged once per kind per
    /// launch). `kind` is a fixed description from `keys::LogOnce`: never a keycode.
    UnrecognisedInput {
        kind: &'static str,
    },
    /// Why and where the output (re)started; logged just before its
    /// [`Event::AudioStarted`] timings. `device` is a CoreAudio object id.
    AudioStartedOn {
        cause: &'static str,
        rebuild: bool,
        device: u32,
        transport: &'static str,
        buffer_frames: Option<u32>,
        pre_roll_ms: f64,
    },
    /// Something that may need the output rebuilt (a device change, sleep,
    /// wake), and the output's state when it arrived.
    AudioTrigger {
        trigger: &'static str,
        state: &'static str,
    },
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Event::Launched { version } => write!(f, "app: Clicky {version} launched"),
            Event::LogStarted { version } => write!(f, "log: started (Clicky {version})"),
            Event::LogStopped => write!(f, "log: stopped"),
            Event::SoundpackLoaded { id, clips, sample_rate } => {
                write!(f, "soundpack: loaded {id} ({clips} clips, {sample_rate} Hz)")
            }
            Event::AudioStarted { sample_rate, start_ms, first_render_ms } => {
                write!(f, "audio: started at {sample_rate:.0} Hz, start call {start_ms:.1} ms, ")?;
                match first_render_ms {
                    Some(ms) => write!(f, "first render after {ms:.1} ms"),
                    None => write!(f, "no render yet"),
                }
            }
            Event::AudioStopped { reason, warm_secs, dropped } => {
                write!(f, "audio: stopped ({reason}) after {warm_secs:.1} s warm")?;
                if *dropped > 0 {
                    write!(f, ", {dropped} key presses dropped so far (ring full)")?;
                }
                Ok(())
            }
            Event::SleepAllowed => write!(f, "audio: system sleep allowed while output runs"),
            Event::InputTapStarted => write!(f, "input: listening (listen-only event tap)"),
            Event::InputPermissionMissing => {
                write!(f, "input: Input Monitoring permission missing; retrying")
            }
            Event::InputTapReenabled { cause } => write!(f, "input: tap re-enabled ({cause})"),
            Event::Error { context, detail } => write!(f, "error: {context}: {detail}"),
            Event::UnrecognisedInput { kind } => {
                write!(f, "input: saw {kind}; it plays a fallback sound (logged once per launch)")
            }
            Event::AudioStartedOn {
                cause,
                rebuild,
                device,
                transport,
                buffer_frames,
                pre_roll_ms,
            } => {
                let verb = if *rebuild { "rebuilt" } else { "starting" };
                write!(f, "audio: {verb} ({cause}) on device {device} ({transport}), ")?;
                match buffer_frames {
                    Some(frames) => write!(f, "IO buffer {frames} frames, ")?,
                    None => write!(f, "IO buffer unknown, ")?,
                }
                if *pre_roll_ms > 0.0 {
                    write!(f, "pre-roll {pre_roll_ms:.0} ms")
                } else {
                    write!(f, "no pre-roll")
                }
            }
            Event::AudioTrigger { trigger, state } => {
                write!(f, "audio: {trigger} (output {state})")
            }
        }
    }
}

/// One log line: UTC timestamp, then the event.
pub fn format_line(at: SystemTime, event: &Event) -> String {
    format!("{} {event}\n", utc_timestamp(at))
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn utc_timestamp(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// The log file, opened while logging is on.
pub struct Logger {
    path: PathBuf,
    file: Option<File>,
    stderr: bool,
}

impl Logger {
    pub fn new(path: PathBuf, stderr: bool) -> Logger {
        Logger { path, file: None, stderr }
    }

    pub fn is_enabled(&self) -> bool {
        self.file.is_some()
    }

    /// Opens (creating the folder, rotating a large file) or closes the log file.
    pub fn set_enabled(&mut self, on: bool, version: &'static str) -> std::io::Result<()> {
        match (on, self.file.is_some()) {
            (true, false) => {
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                rotate(&self.path)?;
                self.file = Some(OpenOptions::new().create(true).append(true).open(&self.path)?);
                self.write(SystemTime::now(), &Event::LogStarted { version });
            }
            (false, true) => {
                self.write(SystemTime::now(), &Event::LogStopped);
                self.file = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn write(&mut self, at: SystemTime, event: &Event) {
        if self.file.is_none() && !self.stderr {
            return;
        }
        let line = format_line(at, event);
        if self.stderr {
            eprint!("{line}");
        }
        if let Some(file) = &mut self.file {
            // A failed write must never take the app down; the line is lost.
            let _ = file.write_all(line.as_bytes());
        }
    }
}

fn rotate(path: &Path) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > ROTATE_BYTES) {
        std::fs::rename(path, path.with_extension("log.1"))?;
    }
    Ok(())
}

/// `~/Library/Logs/Clicky/Clicky.log`.
pub fn log_path() -> PathBuf {
    let home = std::env::var_os("HOME").map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
    home.join("Library/Logs/Clicky/Clicky.log")
}

static LOGGER: Mutex<Option<Logger>> = Mutex::new(None);

fn with_logger<R>(f: impl FnOnce(&mut Logger) -> R) -> R {
    let mut guard = LOGGER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let logger = guard.get_or_insert_with(|| {
        Logger::new(log_path(), std::env::var_os(STDERR_ENV).is_some_and(|v| v == "1"))
    });
    f(logger)
}

/// Mirrors log lines to stderr, whether or not the log file is on.
pub fn set_stderr(on: bool) {
    with_logger(|logger| logger.stderr = on);
}

/// Turns the process-wide log on or off.
pub fn set_enabled(on: bool) {
    let result = with_logger(|logger| logger.set_enabled(on, env!("CARGO_PKG_VERSION")));
    if let Err(e) = result {
        eprintln!("clicky: cannot open {}: {e}", log_path().display());
    }
}

pub fn is_enabled() -> bool {
    with_logger(|logger| logger.is_enabled())
}

/// Records `event` if the log is on. Not real-time safe.
pub fn log(event: Event) {
    with_logger(|logger| logger.write(SystemTime::now(), &event));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64, millis: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis)
    }

    #[test]
    fn timestamps_are_utc_with_milliseconds() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        assert_eq!(utc_timestamp(at(1_700_000_000, 42)), "2023-11-14T22:13:20.042Z");
        assert_eq!(utc_timestamp(at(951_782_400, 0)), "2000-02-29T00:00:00.000Z");
        assert_eq!(utc_timestamp(at(4_102_444_799, 999)), "2099-12-31T23:59:59.999Z");
    }

    #[test]
    fn an_audio_start_records_both_timings() {
        let event = Event::AudioStarted {
            sample_rate: 48_000.0,
            start_ms: 12.34,
            first_render_ms: Some(31.06),
        };
        assert_eq!(
            format_line(at(1_700_000_000, 0), &event),
            "2023-11-14T22:13:20.000Z audio: started at 48000 Hz, start call 12.3 ms, \
             first render after 31.1 ms\n"
        );
        let silent =
            Event::AudioStarted { sample_rate: 44_100.0, start_ms: 1.0, first_render_ms: None };
        assert!(silent.to_string().ends_with("no render yet"));
    }

    #[test]
    fn an_audio_stop_records_why_and_how_long_it_was_warm() {
        let event =
            Event::AudioStopped { reason: "hold window elapsed", warm_secs: 125.04, dropped: 0 };
        assert_eq!(event.to_string(), "audio: stopped (hold window elapsed) after 125.0 s warm");
        let lossy = Event::AudioStopped { reason: "x", warm_secs: 1.0, dropped: 3 };
        assert!(lossy.to_string().contains("3 key presses dropped"));
    }

    #[test]
    fn unrecognised_input_names_its_kind_only() {
        let event = Event::UnrecognisedInput { kind: "a media key with no known position" };
        assert_eq!(
            event.to_string(),
            "input: saw a media key with no known position; it plays a fallback sound \
             (logged once per launch)"
        );
    }

    #[test]
    fn a_start_records_its_cause_and_device() {
        let start = Event::AudioStartedOn {
            cause: "key press",
            rebuild: false,
            device: 73,
            transport: "built-in",
            buffer_frames: Some(128),
            pre_roll_ms: 0.0,
        };
        assert_eq!(
            start.to_string(),
            "audio: starting (key press) on device 73 (built-in), IO buffer 128 frames, no pre-roll"
        );
        let rebuild = Event::AudioStartedOn {
            cause: "default output changed",
            rebuild: true,
            device: 80,
            transport: "Bluetooth",
            buffer_frames: None,
            pre_roll_ms: 250.0,
        };
        assert_eq!(
            rebuild.to_string(),
            "audio: rebuilt (default output changed) on device 80 (Bluetooth), \
             IO buffer unknown, pre-roll 250 ms"
        );
    }

    #[test]
    fn a_trigger_records_the_state_it_found() {
        let event = Event::AudioTrigger { trigger: "sample rate changed", state: "warm" };
        assert_eq!(event.to_string(), "audio: sample rate changed (output warm)");
    }

    #[test]
    fn errors_name_their_context() {
        let event = Event::Error { context: "audio start", detail: "OSStatus -10851".into() };
        assert_eq!(event.to_string(), "error: audio start: OSStatus -10851");
    }

    #[test]
    fn the_file_gets_lines_only_while_logging_is_on() {
        let dir = std::env::temp_dir().join(format!("clicky-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Logs/Clicky.log");
        let mut logger = Logger::new(path.clone(), false);
        logger.write(at(1, 0), &Event::InputTapStarted);
        assert!(!path.exists(), "nothing is written while the log is off");

        logger.set_enabled(true, "9.9.9").unwrap();
        logger.write(at(2, 0), &Event::InputTapStarted);
        logger.set_enabled(false, "9.9.9").unwrap();
        logger.write(at(3, 0), &Event::SleepAllowed);

        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].ends_with("log: started (Clicky 9.9.9)"));
        assert_eq!(lines[1], "1970-01-01T00:00:02.000Z input: listening (listen-only event tap)");
        assert!(lines[2].ends_with("log: stopped"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_large_log_is_rotated_when_logging_starts() {
        let dir = std::env::temp_dir().join(format!("clicky-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Clicky.log");
        std::fs::write(&path, vec![b'x'; ROTATE_BYTES as usize + 1]).unwrap();
        let mut logger = Logger::new(path.clone(), false);
        logger.set_enabled(true, "1").unwrap();
        assert!(dir.join("Clicky.log.1").exists());
        assert!(std::fs::metadata(&path).unwrap().len() < 200);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
