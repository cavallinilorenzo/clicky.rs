# Output lifecycle: warm hold window, lossless cold start

Clicky must hear the first key press after any pause and must not keep the Mac or headphones awake. Rustyvibes 2 stops its output after 15 s of silence and restarts it on the next key press. Measured on 2026-11-05 (macOS 27, AirPods Pro), a cold restart takes 15–57 ms to the first render, with outliers of ~400 ms, so the first keys after a pause go missing. Also measured: any running output IO, even pure silence, makes `coreaudiod` hold a `PreventUserIdleSystemSleep` assertion, unless the process sets `kAudioHardwarePropertySleepingIsAllowed` to 1. With that set, no assertion appeared.

We decided:

- Set `kAudioHardwarePropertySleepingIsAllowed = 1` at launch.
- Keep the output warm for a **hold window** of 2 minutes after the last key press, then go cold. The window must stay below 5 minutes, the point at which idle CPU must read 0.0%.
- A cold start never drops a key press. Queued voices start on the first real render cycle. On Bluetooth outputs a **pre-roll** comes before the first voice; its length is set by listening tests.
- A change of default device or sample rate, a device that disappears, and sleep/wake all fade out, rebuild the output at the device's current format, and restart if warm. A watchdog rebuilds a warm output that stops rendering.
- Request a small hardware buffer only on built-in outputs, never on Bluetooth or USB devices.

## Considered options

- **Always warm**: instant everywhere, but it keeps Bluetooth headphones streaming forever and costs CPU wakeups while idle. Rejected.
- **Short idle stop (Rustyvibes 2)**: loses the first keys after a pause. Rejected.

## Consequences

During the hold window Bluetooth headphones receive silence and stay in use. The Mac itself can still idle-sleep.
