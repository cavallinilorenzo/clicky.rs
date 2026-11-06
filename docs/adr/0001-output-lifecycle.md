# Output lifecycle: warm hold window, lossless cold start

Clicky must hear the first key press after any pause and must not keep the Mac or headphones awake. Rustyvibes 2 stops its output after 15 s of silence and restarts it on the next key press. Measured on 2026-11-05 (macOS 27, AirPods Pro), a cold restart takes 15–57 ms to the first render, with outliers of ~400 ms, so the first keys after a pause go missing. Also measured: any running output IO, even pure silence, makes `coreaudiod` hold a `PreventUserIdleSystemSleep` assertion, unless the process sets `kAudioHardwarePropertySleepingIsAllowed` to 1. With that set, no assertion appeared.

We decided:

- Set `kAudioHardwarePropertySleepingIsAllowed = 1` at launch.
- Keep the output warm for a **hold window** of 2 minutes after the last key press, then go cold. The window must stay below 5 minutes, the point at which idle CPU must read 0.0%.
- A cold start never drops a key press. Queued voices start on the first real render cycle. On Bluetooth outputs a **pre-roll** comes before the first voice; its length is set by listening tests.
- A change of default device or sample rate, a device that disappears, and sleep/wake all fade out, rebuild the output at the device's current format, and restart if warm. A watchdog rebuilds a warm output that stops rendering.
- Request a small hardware buffer only on built-in outputs, never on Bluetooth or USB devices.

## Values in use

- **Pre-roll: 250 ms on Bluetooth outputs, provisional.** `BLUETOOTH_PRE_ROLL` in `audio/device.rs` is the only place it is set. AirPods cold starts measured ~470–490 ms to the first render (#3); the pre-roll is silence played after that first render, because A2DP sinks often swallow the first ~100–250 ms of a resumed stream. It stays provisional until Lorenzo's listening tests (device × 1/5/30 min × 10) confirm or replace it; record the chosen value and the date here. It applies after every cold start and rebuild on Bluetooth, never on other transports.
- **Warm silence on Bluetooth: true zeros.** While warm, a Bluetooth output renders zeros and never flags a block `kAudioUnitRenderAction_OutputIsSilence`, so the A2DP stream does not pause during the hold window (`Transport::flags_silence` in `audio/device.rs`). Other transports flag silent blocks, which lets CoreAudio skip them.
- **Built-in buffer: 128 frames** (2.7–2.9 ms at 44.1–48 kHz), requested per process and clamped to the device's range.
- **Watchdog: 2 s.** A warm output whose render count did not move for a whole interval is rebuilt.
- **Settle: 100 ms.** Device notifications for a warm output wait this long so that a burst (default device, then its rate) causes one rebuild, well within the 1 s budget.
- **Sleep stops a warm output; wake restarts it** only if the hold window is still open, and rebuilds an output that missed the sleep.

## Considered options

- **Always warm**: instant everywhere, but it keeps Bluetooth headphones streaming forever and costs CPU wakeups while idle. Rejected.
- **Short idle stop (Rustyvibes 2)**: loses the first keys after a pause. Rejected.

## Consequences

During the hold window Bluetooth headphones receive silence and stay in use. The Mac itself can still idle-sleep.

The spec's "within 30 ms for the first press after an idle period" is met only for idle periods shorter than the hold window. Measured so far:

- A press while warm, that is, within 2 minutes of the previous press, plays on the next render cycle. On built-in outputs that is a 128-frame buffer (about 3 ms) plus the device's output latency, well within the 30 ms. The hold window exists to cover these pauses.
- The first press after more than 2 minutes idle is a cold start, and it is not reliably within 30 ms. The press is never lost, but it is late. On 2026-11-05 (macOS 27, AirPods Pro, see above) a cold restart took 15–57 ms to the first render, with outliers of ~400 ms. Clicky's own AirPods cold starts took ~470–490 ms (#3), plus the 250 ms pre-roll. On 2026-11-06 (macOS 27, built-in speakers, `--self-test`, 10 cold starts) key press to first render took 13–79 ms. The first start in a process was the slowest (28–79 ms, as it creates the audio unit). Later ones took 13–45 ms. These are times to the first render; the sound comes out later still, by the device's output latency.

So on built-in speakers the gap is the first press after more than 2 minutes idle: it lands somewhere from just under to well over 30 ms. This ADR leaves the hold window at 2 minutes. Lorenzo's onset measurement decides whether that is acceptable: record a scripted key press on built-in speakers after more than 2 minutes idle, and compare the sound onset with 30 ms. If the gap matters, the options are a longer hold window (below 5 minutes, at the cost of idle CPU and Bluetooth streaming) or a cheaper cold start (for example, keeping the unit initialised while cold). Record the result and the date here.

Another process's audio tap can still hold the Mac awake during the hold window. Measured on 2026-11-06 (macOS 27, built-in speakers): while any output runs, `coreaudiod` holds a `PreventUserIdleSystemSleep` assertion named `com.apple.audio.AudioTap-….context.preventuseridlesleep` (resource `audio-in`). `pmset -g assertions -v` shows it as "Created for PID" of the tapping process (`corespeechd`, Siri), not Clicky. `afplay` alone raises it too, along with its own `BuiltInSpeakerDevice` assertion, which Clicky never raises. It lasts while Clicky is warm and is gone about 10 s after the output goes cold. So on a Mac with such a tap, idle sleep may come up to about 2 minutes after the last key press, not at a 1-minute idle-sleep setting. Clicky cannot opt out of another process's tap. Only a shorter hold window would shorten this, at the cost of more cold starts, and that choice is open.
