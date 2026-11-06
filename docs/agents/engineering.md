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
- run: `cargo xtask run` (builds the universal `target/bundle/Clicky.app`, converts the soundpacks, signs it per ADR 0002, audits it, quits any running Clicky and opens the new one). `cargo xtask bundle` builds without launching; add `--native` for a faster host-only build. Signing falls back to ad-hoc with a warning when the `Clicky Local Signing` identity is missing.
- exercise (`C=target/bundle/Clicky.app/Contents/MacOS/Clicky`):
  - `$C --self-test`: drives the real audio path (voicer → ring → lifecycle → CoreAudio → render callback) with synthetic presses, inaudibly (`--audible` to hear it); expects a cold start, a rebuild while warm, the output going cold after a short (0.8 s) hold window, a second cold start, each with its first-render time, and `self-test passed`. Needs no permission.
  - `$C --bench-typing <secs> [--idle <secs>] [--cycles <n>] [--audible]`: synthetic typing at 10 keys/s (press, release 40 ms later) through the listener's path (`KeyState` → `Voicer` → ring → lifecycle with the real 2-minute hold window → CoreAudio), one warm rebuild halfway through each typing phase; after each phase it prints the process's CPU %, idle wakeups/s and physical footprint, and ends with `bench passed` (fails if a press was dropped). Measure CPU at 10 keys/s with `--bench-typing 90` plus `top -pid <pid> -stats cpu,idlew,mem`; a short soak is `--bench-typing 60 --idle 150 --cycles 7` (25 min, a cold start and a rebuild per cycle). Needs no permission.
  - `$C --menu`: builds the real menu without running the app and prints one line per item (`<switch>`, `<slider>`, `<submenu>`, titles, `---`, `[hidden] ` prefix); `crates/clicky/tests/menu_order.rs` checks it against issue #2's order. Writes nothing.
  - `cargo xtask audit [app]` (also run by `bundle`): fails on networking imports, libraries, run-time class names or entitlements, on typed-text APIs (`CGEventKeyboardGetUnicodeString`, `UCKeyTranslate`, NSEvent `characters…` selectors), and on any slice whose minimum macOS is not 13.0.
  - `lipo -archs $C` (expect `x86_64 arm64`), `vtool -show-build $C` (minos 13.0 on both) and `codesign -dv target/bundle/Clicky.app`.
  - Idle cost of the real app: open it untouched, wait past the 2-minute hold window plus 5 minutes, then `top -l 3 -s 5 -pid <pid> -stats cpu,idlew,mem`, `footprint <pid>`, `lsof -nP -i -a -p <pid>` (expect nothing) and `pmset -g assertions` (expect nothing created for Clicky's pid). `osascript` / accessibility may read the status item's menu, but never click inside it.
  - `open --env CLICKY_LOG_STDERR=1 --stderr /tmp/clicky.log target/bundle/Clicky.app`, then read `/tmp/clicky.log` (launch, sleep policy, soundpack loaded, input state); quit with `osascript -e 'tell application id "io.github.cavallinilorenzo.clicky" to quit'`.
  - Real key presses need Input Monitoring granted by hand in System Settings; agents must not grant it. Typing tests are for Lorenzo.
- screenshots: none
- test data: synthetic only, generated in tests; plus the committed source audio in `assets/soundpacks/`

## Review

- second_opinion: none

## Release

- release: none
