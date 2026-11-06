//! `Clicky --bench-typing <secs> [--idle <secs>] [--cycles <n>] [--audible]`:
//! synthetic typing at 10 keys per second through the path the key listener
//! uses (key event → `KeyState` → `Voicer` → ring → lifecycle → CoreAudio →
//! render callback), with the real hold window, so the process's CPU and
//! memory can be measured without anyone typing. Needs no permission.
//!
//! Each cycle types for `<secs>` (one press and, 40 ms later, its release
//! every 100 ms; one warm rebuild halfway, as after a device change), then
//! stays idle for `--idle` seconds (an idle longer than the 2-minute hold
//! window ends cold). After each phase it prints its own CPU use, idle
//! wakeups and memory footprint. Inaudible unless `--audible`. Writes the
//! diagnostics log when the app's Diagnostics Log setting is on.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::diagnostics::{self, Event, LOG_ENABLED};
use crate::engine::{RING_CAPACITY, Shared, Voicer, ring};
use crate::keys::{KeyEvent, KeyState};
use crate::settings::Store;
use crate::{audio, packs};

/// One key every this long: 10 keys per second.
const KEY_PERIOD: Duration = Duration::from_millis(100);
/// How long each synthetic key is held.
const KEY_HOLD: Duration = Duration::from_millis(40);
/// "the quick brown fox⏎" as macOS virtual keycodes.
const SENTENCE: [u16; 20] =
    [17, 4, 14, 49, 12, 32, 34, 8, 40, 49, 11, 15, 31, 13, 45, 49, 3, 31, 7, 36];
/// Practically silent, but the mixer still renders every voice.
const INAUDIBLE_GAIN: f32 = 1e-4;

/// What to run.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub typing: Duration,
    pub idle: Duration,
    pub cycles: u32,
    pub audible: bool,
}

impl Plan {
    /// Parses the arguments after `--bench-typing`.
    pub fn parse(args: &[String]) -> Result<Plan, String> {
        const USAGE: &str =
            "usage: Clicky --bench-typing <secs> [--idle <secs>] [--cycles <n>] [--audible]";
        let secs = |arg: Option<&String>| {
            arg.and_then(|a| a.parse::<f64>().ok())
                .and_then(|s| Duration::try_from_secs_f64(s).ok())
                .ok_or_else(|| USAGE.to_owned())
        };
        let mut args = args.iter();
        let mut plan =
            Plan { typing: secs(args.next())?, idle: Duration::ZERO, cycles: 1, audible: false };
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--idle" => plan.idle = secs(args.next())?,
                "--cycles" => {
                    plan.cycles = args
                        .next()
                        .and_then(|n| n.parse().ok())
                        .filter(|n| *n > 0)
                        .ok_or_else(|| USAGE.to_owned())?;
                }
                "--audible" => plan.audible = true,
                _ => return Err(USAGE.to_owned()),
            }
        }
        Ok(plan)
    }
}

/// Runs `plan`; fails if a key press was dropped.
pub fn run(plan: &Plan) -> Result<(), String> {
    diagnostics::set_stderr(true);
    // The app's own log setting, so a soak leaves a log to grep like a real session.
    if Store::standard().get(&LOG_ENABLED) {
        diagnostics::set_enabled(true);
    }
    audio::allow_system_sleep().map_err(|e| e.to_string())?;
    diagnostics::log(Event::SleepAllowed);
    let pack = packs::load_static(packs::DEFAULT_PACK)?;
    // Every sound option on (their defaults): the most work per key.
    let shared: &'static Shared = Box::leak(Box::new(Shared::new(true)));
    shared.set_pack(pack);
    if !plan.audible {
        shared.set_master_gain(INAUDIBLE_GAIN);
    }
    let (producer, consumer) = ring::ring(RING_CAPACITY);
    let kick = audio::spawn(shared, consumer, audio::HOLD_WINDOW);
    let mut voicer = Voicer::new(pack, producer, shared, kick);
    let mut keys = KeyState::default();
    println!("pid {}", std::process::id());
    let mut presses = 0u64;
    for cycle in 1..=plan.cycles {
        let usage = Usage::now();
        let start = Instant::now();
        let mut rebuilt = false;
        let mut next = start;
        while next.duration_since(start) < plan.typing {
            let keycode = SENTENCE[presses as usize % SENTENCE.len()];
            for event in [KeyEvent::Down { keycode, autorepeat: false }, KeyEvent::Up { keycode }] {
                if let Some(transition) = keys.handle(event, Instant::now()) {
                    voicer.key(transition);
                }
                if matches!(event, KeyEvent::Down { .. }) {
                    std::thread::sleep(KEY_HOLD);
                }
            }
            presses += 1;
            if !rebuilt && next.duration_since(start) >= plan.typing / 2 {
                audio::request_rebuild();
                rebuilt = true;
            }
            next += KEY_PERIOD;
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        report(cycle, "typing", &usage, presses, shared);
        if !plan.idle.is_zero() {
            let usage = Usage::now();
            std::thread::sleep(plan.idle);
            report(cycle, "idle", &usage, presses, shared);
        }
    }
    let dropped = shared.dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        return Err(format!("{dropped} of {presses} key presses were dropped"));
    }
    println!("bench passed: {presses} key presses, {} rebuilds, none dropped", audio::rebuilds());
    Ok(())
}

fn report(cycle: u32, phase: &str, since: &Usage, presses: u64, shared: &Shared) {
    let now = Usage::now();
    let wall = now.at.duration_since(since.at).as_secs_f64();
    println!(
        "cycle {cycle} {phase:<6} {wall:>6.1} s: cpu {:.2} %, {:.1} idle wakeups/s, footprint {:.1} MB, \
         presses {presses}, output {}",
        (now.cpu - since.cpu).as_secs_f64() / wall * 100.0,
        now.wakeups.saturating_sub(since.wakeups) as f64 / wall,
        now.footprint as f64 / 1e6,
        if shared.warm.load(Ordering::SeqCst) { "warm" } else { "cold" },
    );
}

/// This process's resource use so far.
struct Usage {
    at: Instant,
    /// User plus system CPU time.
    cpu: Duration,
    /// Package idle and interrupt wakeups.
    wakeups: u64,
    /// Physical footprint in bytes (what Activity Monitor calls Memory).
    footprint: u64,
}

impl Usage {
    fn now() -> Usage {
        // SAFETY: both calls only fill the zeroed structs passed to them.
        let (rusage, info) = unsafe {
            let mut rusage: libc::rusage = std::mem::zeroed();
            libc::getrusage(libc::RUSAGE_SELF, &mut rusage);
            let mut info: libc::rusage_info_v2 = std::mem::zeroed();
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                (&raw mut info).cast(),
            );
            (rusage, info)
        };
        let time = |t: libc::timeval| {
            Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64)
        };
        Usage {
            at: Instant::now(),
            cpu: time(rusage.ru_utime) + time(rusage.ru_stime),
            wakeups: info.ri_pkg_idle_wkups + info.ri_interrupt_wkups,
            footprint: info.ri_phys_footprint,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn typing_time_alone_runs_one_cycle_without_idle() {
        let plan = Plan::parse(&args(&["60"])).unwrap();
        assert_eq!(
            plan,
            Plan {
                typing: Duration::from_secs(60),
                idle: Duration::ZERO,
                cycles: 1,
                audible: false
            }
        );
    }

    #[test]
    fn idle_cycles_and_audible_are_read() {
        let plan =
            Plan::parse(&args(&["30", "--idle", "150", "--cycles", "8", "--audible"])).unwrap();
        assert_eq!(plan.typing, Duration::from_secs(30));
        assert_eq!(plan.idle, Duration::from_secs(150));
        assert_eq!(plan.cycles, 8);
        assert!(plan.audible);
        assert_eq!(Plan::parse(&args(&["0.5"])).unwrap().typing, Duration::from_millis(500));
    }

    #[test]
    fn bad_arguments_are_errors() {
        for bad in [
            &[][..],
            &["x"],
            &["-1"],
            &["10", "--idle"],
            &["10", "--cycles", "0"],
            &["10", "--fast"],
        ] {
            assert!(Plan::parse(&args(bad)).is_err(), "{bad:?}");
        }
    }
}
