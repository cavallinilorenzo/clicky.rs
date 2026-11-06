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
tap thread                                 │ reads enabled / master gain
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
  `Voicer` picks the clip through `keys::press_clip` and pushes a `Play` into
  the lock-free ring. It never blocks: a full ring drops the press and counts it.
- The **render callback** drains the ring into the `Mixer` and renders. No locks,
  no allocation, no logging. It records the first render after each start.
- The **audio control thread** owns the output. It sleeps while cold. A key press
  while cold wakes it; it starts the output (the press waits in the ring and plays
  on the first render, after the pre-roll on Bluetooth). While warm it wakes only
  at the hold-window deadline and the watchdog check. CoreAudio listeners and the
  sleep/wake observers only set a bit in `audio::signals` and unpark it; it then
  fades out and rebuilds the output at the device's current format.
- The **main thread** runs the menu. The on/off switch writes `Shared` and the
  settings; turning sounds off makes the mixer ramp to silence within one block.
- Soundpacks are built at build time (`cargo xtask packs`) from the committed
  source audio. At launch the app reads every pack's description from
  `Clicky.app/Contents/Resources/Packs` and the audio of the saved one; a pack's
  audio is read the first time it is picked and kept for the life of the process.
- The **preview thread** plays the pick preview through its own `Voicer` and ring,
  drained by the render callback after the key ring.

## Module map (`crates/clicky/src`)

| Module | What it does | Pure / tested |
|---|---|---|
| `main.rs` | Arguments (`--version`, `--self-test`), then `app::run`. | |
| `app.rs` | Launch wiring: settings, sleep policy, soundpack, audio, input, UI. | |
| `settings.rs` | Typed `Setting<T>` + `Store` over `NSUserDefaults` (in-memory backend for tests). Each feature declares its own `Setting` const next to its code. | yes |
| `diagnostics.rs` | The optional log: the `Event` enum (no key data, by construction), line formatting, the file logger. `LOG_ENABLED` setting. | yes |
| `keys.rs` | `KeyEvent`, `KeyState` (press/release tracking: held-key set, auto-repeat suppression, modifiers from flags-changed and device bits, Caps Lock toggles, media keys), `Transition`, `press_clip` (own recording → key class → default clip), `LogOnce` (unrecognised input logged once per kind). | yes |
| `packs.rs` | The `Library` of bundled soundpacks (descriptions read at launch, audio on first use, never freed), grouped by `SwitchType`; `load_saved` (saved id → default → first that loads); `DEFAULT_PACK`, `SELECTED_PACK` setting. | yes |
| `engine/mod.rs` | `Shared` atomics (including the current soundpack, an `AtomicPtr` swapped by the picker), `Kick`, `Voicer`; `SOUNDS_ON` setting, `MASTER_GAIN`. | yes |
| `engine/preview.rs` | The pick preview: its own thread, `Voicer` and ring press a short "rust ⏎" from the current soundpack; picking again restarts it. | yes |
| `engine/ring.rs` | Wait-free SPSC ring; `Consumer::with_producer` chains a second ring (the preview's) into the same consumer. | yes |
| `engine/mixer.rs` | Voice pool, resampling, master-gain ramp, clamp. | yes |
| `audio/lifecycle.rs` | ADR 0001 policy as a pure state machine (cold / starting / warm, hold window, rebuild triggers, sleep/wake, watchdog); time and render counts are passed in. | yes |
| `audio/device.rs` | Transport kinds and what they imply: small buffer only on built-in, `BLUETOOTH_PRE_ROLL` (provisional) only on Bluetooth. | yes |
| `audio/mod.rs` | The control thread: drives the lifecycle against the output, confirms device signals, logs every start, stop, rebuild and trigger with its cause and first-render time. | |
| `audio/signals.rs` | Signal bits from CoreAudio listeners and sleep/wake to the control thread (set a bit, unpark). | yes |
| `audio/hal.rs` | CoreAudio HAL queries (default device, transport, rate, format, buffer), `allow_system_sleep`, property listeners. | |
| `audio/power.rs` | `NSWorkspace` will-sleep / did-wake observers. | |
| `audio/output.rs` | CoreAudio adapter: AUHAL unit bound to the current default device at its current rate, render callback with pre-roll and fade-out, rebuild. | yes (render state) |
| `audio/ffi.rs`, `input/ffi.rs` | Hand-written FFI (AudioToolbox, CoreAudio, CoreGraphics, CoreFoundation). | layout tests |
| `input/mod.rs` | The listen-only event tap thread (key down/up, flags-changed, system-defined) and its minimal permission retry. | |
| `input/media.rs` | Media keys: reads a system-defined event's subtype and `data1` through `NSEvent` (the tap thread's only Objective-C call). | yes (decode, real NSEvent round trip) |
| `selftest.rs` | `--self-test`: exercises the audio path without the key listener (cold start, rebuild while warm, hold window, second cold start). | |
| `ui/mod.rs` | `Ui` (what sections share), app delegate, `run`. | |
| `ui/target.rs` | `ActionTarget` / `MenuDelegate`: Objective-C shims that call Rust closures, so sections never add methods to a shared class. | |
| `ui/status_item.rs` | The menu bar icon and its states. | yes (state) |
| `ui/menu/mod.rs` | `MenuBuilder` and `build`, which lists the sections in menu order. | |
| `ui/menu/sound_switch.rs`, `soundpack.rs`, `about.rs`, `diagnostics.rs`, `quit.rs` | One file per menu section. `soundpack.rs`: grouped picker, `choose` (switch + save). `about.rs`: About panel, `credits_text`. | partly (`choose`, `credits_text`) |

`crates/xtask/src`: `catalog.rs` (which packs ship, in menu order), `sources/` (one adapter per
source layout: `mechvibes.rs`, `kbsim.rs`), `decode.rs`, `dsp.rs` (trim, normalise, dither),
`loudness.rs` (K-weighted 100 ms attack loudness), `packs.rs` (pipeline + invariants, incl. the
all-21-packs loudness/onset test), `bundle.rs` (universal build, Info.plist,
bundle layout), `sign.rs` (ADR 0002 signing).

## Where each follow-up ticket works

| Ticket | Own files (mostly new) | Shared lines it touches |
|---|---|---|
| #4 Output lifecycle | `audio/lifecycle.rs` (new events/states: device change, sample rate, sleep/wake, watchdog, pre-roll), `audio/output.rs` (listeners, rebuild, built-in buffer size), `audio/mod.rs`, new `audio/devices.rs` etc., `audio/ffi.rs` | new `Event` variants in `diagnostics.rs` |
| #5 Key mapping (done) | `keys.rs`, `input/mod.rs`, `input/media.rs`, `input/ffi.rs`, `clicky-pack/src/keys.rs` (`KeyClass`, `class`, `fallbacks`, `media_keycode`) | `Event::UnrecognisedInput` in `diagnostics.rs`; `Voicer::press` unchanged |
| #6 Mixer, limiter, volume | `engine/mixer.rs` (+ an offline stress-render test), new `ui/menu/volume.rs` | one line in `ui/menu/mod.rs::build`; `VOLUME` setting next to the mixer; `Shared::set_master_gain` already exists |
| #7 21 packs, picker, About | `assets/soundpacks/**`, `crates/xtask/src/sources/kbsim.rs`, `catalog.rs`, `packs.rs` (app: library of all packs, `SELECTED_PACK` setting), new `ui/menu/soundpack.rs`, `ui/menu/about.rs`, `THIRD_PARTY_NOTICES.md` | two lines in `ui/menu/mod.rs::build`; `Voicer` needs a switchable pack (e.g. an atomic pointer) in `engine/mod.rs`; the `SourceKind` enum |
| #8 Sound options | `keys.rs`/`engine` voicer options, new `ui/menu/options.rs`; release sounds hang off `Transition::Release` in `input/mod.rs` | one line in `ui/menu/mod.rs::build`; `Play` gains per-voice pan/pitch |
| #9 First run, permission, Launch at Login | new `ui/onboarding.rs`, `ui/menu/launch_at_login.rs`, `ui/menu/permission.rs`, `input/permission.rs` | one line each in `ui/menu/mod.rs::build`; a `NeedsPermission` state in `ui/status_item.rs`; `input::start`'s retry loop |

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
