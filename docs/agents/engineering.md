# Engineering config

Read by `/verify`, `/judge`, `/build`, `/ship`, `/ui` and `/release`.

## Risk

- floor: low
- sensitive: no
- high-risk paths:
  - `crates/clicky/src/input/**` (global key capture: listen-only tap, keycode and event type only)
  - `crates/clicky/src/keys.rs` (what the app learns about each key)
  - `crates/clicky/src/diagnostics.rs` (the only thing written to disk besides settings; never key data)
  - `crates/clicky/src/audio/output.rs` (real-time render callback: no locks, no allocation)
  - `.github/workflows/**`

## Recipe

- setup: `cargo fetch`
- checks:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
  - `cargo build`
- run: `cargo xtask run` (builds the universal `target/bundle/Clicky.app`, converts the soundpacks, signs it per ADR 0002, quits any running Clicky and opens the new one). `cargo xtask bundle` builds without launching; add `--native` for a faster host-only build. Signing falls back to ad-hoc with a warning when the `Clicky Local Signing` identity is missing.
- exercise:
  - `target/bundle/Clicky.app/Contents/MacOS/Clicky --self-test`: drives the real audio path (voicer → ring → lifecycle → CoreAudio → render callback) with synthetic presses, inaudibly (`--audible` to hear it); expects two cold starts with their first-render times, the output going cold after a short hold window, and `self-test passed`. Needs no permission.
  - `lipo -archs target/bundle/Clicky.app/Contents/MacOS/Clicky` (expect `x86_64 arm64`) and `codesign -dv target/bundle/Clicky.app`.
  - `open --env CLICKY_LOG_STDERR=1 --stderr /tmp/clicky.log target/bundle/Clicky.app`, then read `/tmp/clicky.log` (launch, sleep policy, soundpack loaded, input state); quit with `osascript -e 'tell application id "io.github.cavallinilorenzo.clicky" to quit'`.
  - Real key presses need Input Monitoring granted by hand in System Settings; agents must not grant it. Typing tests are for Lorenzo.
- screenshots: none
- test data: synthetic only, generated in tests; plus the committed source audio in `assets/soundpacks/`

## Review

- second_opinion: none

## Release

- release: none
