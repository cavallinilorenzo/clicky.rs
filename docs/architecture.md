# Architecture

Clicky is a Cargo workspace of three crates:

| Crate | Role |
|---|---|
| `crates/clicky` | The menu bar app (binary `clicky`, bundled as `Clicky.app`). |
| `crates/clicky-pack` | The `.clickypack` soundpack format, macOS key geometry, key classes, fallbacks and media-key positions (`keys`) and the Mechvibes key-code table. Shared by the app (reader) and the build tool (writer). |
| `crates/xtask` | Build tooling, run as `cargo xtask`: the soundpack pipeline, the universal app bundle and signing. |

## Data flow

```text
                  main thread (AppKit)
                  ui::menu sections ──▶ Shared (atomics) ◀── settings::Store (NSUserDefaults)
                                           ▲
tap thread                                 │ reads enabled / master gain / options
input (CGEventTap, listen-only)            │
  └▶ keys::KeyState ─▶ engine::Voicer ─Play─▶ engine::ring (SPSC) ─▶ render callback
                         │                                              (CoreAudio IO thread)
                         └ stamps the key time, wakes ─▶ audio control thread       │
                                                          audio::lifecycle (policy)  │
                                                          audio::output (AUHAL) ─────┘
```

- The **tap thread** sees only the event type, keycode, auto-repeat flag and
  modifier flags (and a media key's type). `KeyState` decides whether an event
  is a physical press (one per press, never auto-repeat) or release;
  `Voicer::key` picks the clip (a press through `keys::press_clip`; a release
  only from the key's own release recording, when Key Release Sounds is on),
  draws Natural Variation's gain and pitch (xorshift, no allocation), takes the
  key's pan for Spatial Stereo (`engine::options`), and pushes a `Play` into
  the lock-free ring. It never blocks: a full ring drops the press and counts it.
  The same thread supervises the Input Monitoring permission
  (`input::permission::Supervisor`): every second while there is no tap, every
  3 s while listening, it checks the permission (`IOHIDCheckAccess`, no prompt)
  and the tap, creates, drops or recreates the tap, and publishes a `Status`
  (missing / listening / revoked / needs relaunch). Each change is posted to the
  main queue, where the status icon and the first-run window follow it.
- The **render callback** drains the ring into the `Mixer` and renders. No locks,
  no allocation, no logging. It records the first render after each start. The
  mixer resamples each voice to the device rate (times its pitch), pans it,
  fades out stolen voices, ramps the master gain per frame and ends in a
  zero-latency look-ahead limiter, linked across both channels (see below). On a
  one-channel device, which CoreAudio feeds only our left channel, every voice
  is mixed centred.
- The **audio control thread** owns the output. It sleeps while cold. A key press
  while cold wakes it; it starts the output (the press waits in the ring and plays
  on the first render, after the pre-roll on Bluetooth). While warm it wakes only
  at the hold-window deadline and the watchdog check. CoreAudio listeners and the
  sleep/wake observers only set a bit in `audio::signals` and unpark it; it then
  fades out and rebuilds the output at the device's current format.
- The **main thread** runs the menu. The on/off switch writes `Shared` and the
  settings; turning sounds off makes the mixer ramp to silence within one block.
  The volume slider sets the master gain (`Shared::set_master_gain`) on every move
  and saves `VOLUME`; `app.rs` applies the saved volume at launch. The three sound
  options flip an atomic in `Shared` and save their setting; `app.rs` restores
  them at launch (`engine::options::restore`).
- Soundpacks are built at build time (`cargo xtask packs`) from the committed
  source audio. At launch the app reads every pack's description from
  `Clicky.app/Contents/Resources/Packs` and the audio of the saved one; a pack's
  audio is read the first time it is picked and kept for the life of the process.
- The **preview thread** plays the pick preview through its own `Voicer` and ring,
  drained by the render callback after the key ring.

## Module map (`crates/clicky/src`)

| Module | What it does | Pure / tested |
|---|---|---|
| `main.rs` | Arguments (`--version`, `--self-test`, `--bench-typing`, `--menu [--needs-permission]`), then `app::run`. | |
| `app.rs` | Launch wiring: settings (`restored`: on/off, volume, sound options), sleep policy, soundpack, audio, input, UI. | yes (`restored`, across a real relaunch) |
| `settings.rs` | Typed `Setting<T>` + `Store` over `NSUserDefaults` (in-memory backend for tests). Each feature declares its own `Setting` const next to its code. | yes |
| `diagnostics.rs` | The optional log: the `Event` enum (no key data, by construction), line formatting, the file logger. `LOG_ENABLED` setting. | yes |
| `keys.rs` | `KeyEvent`, `KeyState` (press/release tracking: held-key set, auto-repeat suppression, modifiers from flags-changed and device bits, Caps Lock toggles, media keys), `Transition` (both played by `Voicer::key`), `press_clip` (own recording → key class → default clip), `LogOnce` (unrecognised input noted by the tap callback, logged by the tap thread once per kind). | yes |
| `packs.rs` | The `Library` of bundled soundpacks (descriptions read at launch, audio on first use, never freed), grouped by `SwitchType`; `load_saved` (saved id → default → first that loads); `DEFAULT_PACK`, `SELECTED_PACK` setting. | yes |
| `engine/mod.rs` | `Shared` atomics (including the current soundpack, an `AtomicPtr` swapped by the picker, and the three sound options), `Kick`, `Voicer` (`key`, `press`, `release`; applies the options); `SOUNDS_ON` setting, `MASTER_GAIN` (the volume slider's maximum). | yes |
| `engine/options.rs` | The sound options: `RELEASE_SOUNDS`, `NATURAL_VARIATION`, `SPATIAL_STEREO` settings (all on by default), `restore`, `variation` (±1.5 dB, ±35 cents), `key_pan` (key x position, at most ±0.4), the tap thread's xorshift64* `Rng`. | yes |
| `engine/preview.rs` | The pick preview: its own thread, `Voicer` and ring press a short "rust ⏎" from the current soundpack; picking again restarts it. | yes |
| `engine/ring.rs` | Wait-free SPSC ring; `Consumer::with_producer` chains a second ring (the preview's) into the same consumer. | yes |
| `engine/mixer.rs` | Voice pool (32 voices + 16 slots for stolen voices fading out over 5 ms), stereo mix: per-voice pitch (0.9–1.05) and equal-power pan (`pan_gains`, centre = unity, bit-identical channels), `set_mono` (every voice centred), master-gain ramp (≤ full scale per 20 ms, one-pole smoothed, per frame), headroom, `VOLUME` setting and `volume_gain` (dB curve). | yes |
| `engine/mixer/resample.rs` | Kaiser-windowed sinc (32 zero crossings, β = 9, cutoff 0.95 × the lower Nyquist), 256-phase polyphase tables built and cached off the real-time path. | yes (via the mixer) |
| `engine/mixer/limiter.rs` | Look-ahead peak limiter, ceiling −1 dBFS, 1.5 ms look-ahead with no latency (voices are rendered ahead), 60 ms release; one gain for both channels, set by the louder. | yes (via the mixer) |
| `engine/mixer/stress.rs` | The offline stress render (tests only): fast typing, mashing, 10 keys held 60 s, steal storm, slider while typing, at 44.1/48/96 kHz, with Natural Variation and Spatial Stereo on (and fast typing with both off); both channels checked. | test |
| `audio/lifecycle.rs` | ADR 0001 policy as a pure state machine (cold / starting / warm, hold window, rebuild triggers, sleep/wake, watchdog), and `Retry` (backoff and log-once for creating the output unit); time and render counts are passed in. | yes |
| `audio/device.rs` | Transport kinds and what they imply: small buffer only on built-in; `BLUETOOTH_PRE_ROLL` (provisional) and `flags_silence` (false: true zeros while warm, never `OutputIsSilence`) only on Bluetooth. | yes |
| `audio/mod.rs` | The control thread: drives the lifecycle against the output, creates the output unit (retried on later key presses, 1 s doubling to 60 s, if it fails), confirms device signals, logs every start, stop, rebuild and trigger with its cause and first-render time. | |
| `audio/signals.rs` | Signal bits from CoreAudio listeners and sleep/wake to the control thread (set a bit, unpark). | yes |
| `audio/hal.rs` | CoreAudio HAL queries (default device, transport, rate, format, buffer), `allow_system_sleep`, property listeners. | |
| `audio/power.rs` | `NSWorkspace` will-sleep / did-wake observers. | |
| `audio/output.rs` | CoreAudio adapter: AUHAL unit bound to the current default device at its current rate, render callback with pre-roll and fade-out, rebuild; tells the mixer when the device has one channel. | yes (render state) |
| `audio/ffi.rs`, `input/ffi.rs` | Hand-written FFI (AudioToolbox, CoreAudio, CoreGraphics, CoreFoundation). | layout tests |
| `input/mod.rs` | The listen-only event tap thread (key down/up, flags-changed, system-defined); creates, drops and recreates the tap as `permission::Supervisor` says. | |
| `input/permission.rs` | Input Monitoring: `check` (`IOHIDCheckAccess`), `request` (from a click only), `Supervisor` (permission × tap → action and `Status`), the published status and its observer. | yes (state machine) |
| `input/media.rs` | Media keys: reads a system-defined event's subtype and `data1` through `NSEvent` (the tap thread's only Objective-C call). | yes (decode, real NSEvent round trip) |
| `selftest.rs` | `--self-test`: exercises the audio path without the key listener (cold start, rebuild while warm, hold window, second cold start). | |
| `bench.rs` | `--bench-typing`: synthetic typing at 10 keys/s through `KeyState` → `Voicer` → ring → real output with the real hold window, idle phases, a rebuild per cycle; reports the process's CPU, idle wakeups and footprint per phase; writes the diagnostics log when its setting is on. | yes (arguments) |
| `ui/mod.rs` | `Ui` (what sections share), app delegate, `run`; hops input status changes to the main queue. | |
| `ui/onboarding.rs` | The first-run window: `Model` (when it shows and which step: explain / granted / relaunch), the window, opening Input Monitoring settings, `relaunch`. | yes (model) |
| `ui/target.rs` | `ActionTarget` / `MenuDelegate`: Objective-C shims that call Rust closures, so sections never add methods to a shared class. | |
| `ui/status_item.rs` | The menu bar icon and its states (on, off, needs permission). Needs permission wins over off: the icon flags what must be fixed, and the menu still shows the switch's state. | yes (state) |
| `ui/menu/mod.rs` | `MenuBuilder` and `build`, which lists the sections in menu order; `describe` prints the built menu for `--menu`. | yes (`tests/menu_order.rs` runs `--menu`) |
| `ui/menu/sound_switch.rs`, `soundpack.rs`, `volume.rs`, `about.rs`, `diagnostics.rs`, `quit.rs` | One file per menu section. `soundpack.rs`: grouped picker, `choose` (switch + save). `about.rs`: About panel, `credits_text`. | partly (`choose`, `credits_text`) |
| `ui/menu/options.rs` | Key Release Sounds, Natural Variation, Spatial Stereo checkboxes after the volume slider; Key Release Sounds greyed out with "This soundpack has no release sounds" (subtitle on macOS 14.4+, note row before; tooltip) for packs without them (`Entry::has_release`), refreshed on each menu open. | yes (`view`, `SoundOption::set`) |
| `ui/menu/launch_at_login.rs` | "Launch at Login" over `SMAppService.mainAppService`, behind the `LoginItem` seam; reads the real state on each menu open. | yes (toggle, view) |
| `ui/menu/permission.rs` | The rows shown at the top of the menu, above the switch, while sounds are unavailable for lack of permission, with the fix. | yes (rows; `tests/menu_order.rs` via `--menu --needs-permission`) |

`crates/xtask/src`: `catalog.rs` (which packs ship, in menu order), `sources/` (one adapter per
source layout: `mechvibes.rs`, `kbsim.rs`), `decode.rs`, `dsp.rs` (trim, normalise, dither),
`loudness.rs` (K-weighted 100 ms attack loudness), `packs.rs` (pipeline + invariants, incl. the
all-21-packs loudness/onset test), `bundle.rs` (universal build, Info.plist,
bundle layout), `sign.rs` (ADR 0002 signing), `audit.rs` (run by `bundle`: no networking or
typed-text APIs in the binary, no network entitlements, macOS 13.0 on every slice).

## Where each follow-up ticket works

| Ticket | Own files (mostly new) | Shared lines it touches |
|---|---|---|
| #4 Output lifecycle | `audio/lifecycle.rs` (new events/states: device change, sample rate, sleep/wake, watchdog, pre-roll), `audio/output.rs` (listeners, rebuild, built-in buffer size), `audio/mod.rs`, new `audio/devices.rs` etc., `audio/ffi.rs` | new `Event` variants in `diagnostics.rs` |
| #5 Key mapping (done) | `keys.rs`, `input/mod.rs`, `input/media.rs`, `input/ffi.rs`, `clicky-pack/src/keys.rs` (`KeyClass`, `class`, `fallbacks`, `media_keycode`) | `Event::UnrecognisedInput` in `diagnostics.rs`; `Voicer::press` unchanged |
| #6 Mixer, limiter, volume (done) | `engine/mixer.rs`, `engine/mixer/{resample,limiter,stress}.rs`, `ui/menu/volume.rs` | one line in `ui/menu/mod.rs::build`; one line in `app.rs` (saved volume at launch); `VOLUME` setting in `engine/mixer.rs`; objc2 features `NSImageView`, `NSSlider` |
| #7 21 packs, picker, About | `assets/soundpacks/**`, `crates/xtask/src/sources/kbsim.rs`, `catalog.rs`, `packs.rs` (app: library of all packs, `SELECTED_PACK` setting), new `ui/menu/soundpack.rs`, `ui/menu/about.rs`, `THIRD_PARTY_NOTICES.md` | two lines in `ui/menu/mod.rs::build`; `Voicer` needs a switchable pack (e.g. an atomic pointer) in `engine/mod.rs`; the `SourceKind` enum |
| #8 Sound options (done) | `engine/options.rs`, `engine/mod.rs` (`Voicer::key`/`release`, option atomics), `engine/mixer.rs` + `limiter.rs` (stereo, pitch, pan), `stress.rs`, new `ui/menu/options.rs` | one line each in `ui/menu/mod.rs::build` and `app.rs`; `input/mod.rs` calls `Voicer::key`; `audio/output.rs` reports a mono device; `Play` gained `pan` and `pitch` |
| #9 First run, permission, Launch at Login (done) | new `ui/onboarding.rs`, `ui/menu/launch_at_login.rs`, `ui/menu/permission.rs`, `input/permission.rs` | one line each in `ui/menu/mod.rs::build`; a `NeedsPermission` state in `ui/status_item.rs`; `input::start`'s retry loop |

### Hot spots

Merge conflicts are most likely in, in this order:

1. `ui/menu/mod.rs::build`: one line per section, in menu order. Keep each insertion to
   one line at its marked comment.
2. `diagnostics::Event` and its `Display` match: add variants at the end of the enum and
   of the match.
3. `engine/mod.rs` (`Shared`, `Voicer`, `Play` in `engine/mixer.rs`): #5, #6, #7 and #8
   all reach the voicer. Add fields rather than reshaping.
4. `app.rs` wiring: add a line, don't restructure.
5. `crates/clicky/Cargo.toml` objc2 feature lists: add features in alphabetical order.

Settings are not a hot spot: each ticket adds its own `Setting` const in its own module.
Never rename a shipped settings key (`SoundsOn`, `DiagnosticsLog`).

## Real-time and privacy rules

- The render callback (`audio/output.rs::render`) and everything it calls
  (`Mixer`, `Consumer::pop`, `Shared` atomics) must not lock, allocate, log or call
  Objective-C.
- The tap callback reads only the event type, the keycode, the auto-repeat flag and
  the modifier flags; for media keys, the key type in `data1` (`input/media.rs`, the
  only Objective-C call on the tap thread, and only for system-defined events). Never
  call `CGEventKeyboardGetUnicodeString` or anything that yields characters, and never
  log a keycode. `diagnostics::Event` has no variant that can carry one
  (`UnrecognisedInput` carries only a fixed description of the kind).
- The tap callback (`input/mod.rs::tap_callback`) and everything it calls
  (`KeyState`, `Voicer`, `Producer::push`, `Shared` atomics) must not lock, allocate
  or log, for any event. A re-enabled tap and unrecognised input (`LogOnce::note`)
  only set atomic bits; the tap thread logs them after each run-loop pass
  (`drain_log`, up to 3 s later), unrecognised input at most once per kind per
  process. Exceptions: `media::read` (above), and one system call: a press while the
  output is cold or still starting unparks the audio control thread (`Kick::kick`,
  `Thread::unpark`). While the output is warm a press makes no system call.

## Sound quality (`engine/mixer`)

Measured by the tests (`cargo test`), in the dev profile, on synthetic signals.

| What | How it is measured | Result |
|---|---|---|
| Resampler passband | Sines 100 Hz–18 kHz at 0.5 FS, 44.1↔48 kHz and 44.1/48→96 kHz, least-squares fit | gain within ±0.0003 dB (test bound ±0.05 dB) |
| Resampler images, aliases and interpolation error | Residual after the fit, same sines | −91 to −99 dB, which is the 16-bit quantisation floor of the test clip (bound −80 dB) |
| Alias rejection | 22.8 / 23.5 kHz at 48 kHz played on a 44.1 kHz device | −82.7 / −90.8 dB (bound −80 dB) |
| Full scale | Every sample of both channels of every stress render | peak −1.00 dBFS (the ceiling) under mashing, steal storm and 10 held keys, with variation (±1.5 dB, ±35 cents) and panning (up to +2 dB on the near side) |
| Discontinuities | Peak of an 8th-order 8 kHz high-pass of each channel (test clips have nothing above 2 kHz), outside 2.5 ms after each audible keystroke onset; a test proves it catches a −50 dBFS step and the steal storm catches a hard-cut steal | worst −72.7 dBFS (steal storm), −75.9 (mashing), −76.7 (fast typing), −77.9 (10 held keys), −80.5 (slider while typing), −80.0 (fast typing, options off); bound −66 dBFS |
| Volume ramp | DC held through a random volume sweep at 44.1/48/96 kHz | no frame step above full scale / 20 ms |

The limiter adds no latency. The mixer renders every voice 1.5 ms ahead of what it
outputs, and the limiter turns the gain down along a smooth curve before each peak
arrives. The only peaks it cannot foresee are those of a keystroke that starts in the
current block, within its first 1.5 ms. A final per-frame `min` keeps the ceiling
exact there too. That is the only place where the gain can move abruptly, and it
happens at a keystroke onset.

