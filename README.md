# clicky.rs

**Clicky** is a native macOS menu bar app (macOS 13+, Apple Silicon and Intel)
that plays a mechanical keyboard sound on every key press. It lives in one menu
bar icon, with no Dock icon and no window. It only learns which key moved, never
what is typed, and never goes online.

## Build and run

Needs Rust (stable, 1.88+) and the Xcode command line tools.

```sh
cargo xtask run          # build, sign and launch target/bundle/Clicky.app
cargo xtask bundle       # build and sign without launching
cargo xtask packs        # only convert the soundpacks into target/packs
```

`bundle` and `run` build a universal binary (arm64 + x86_64, joined with
`lipo`); add `--native` to build for this Mac only, which is faster.

The first time, grant **Input Monitoring** to Clicky in System Settings →
Privacy & Security → Input Monitoring (Clicky adds itself to the list when it
starts), then quit and reopen it. Without the permission the app runs but stays
silent.

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test`. `Clicky.app/Contents/MacOS/Clicky --self-test` checks the audio
output path without the permission (inaudible; add `--audible`).

## Signing

macOS ties Input Monitoring to the app's code signature (ADR 0002). An ad-hoc
signature changes with every build, so macOS forgets the permission after each
rebuild. Create a stable local identity once:

```sh
scripts/create-signing-identity.sh
```

It adds a self-signed code-signing certificate named `Clicky Local Signing` to
your login keychain (read the script first). From then on `cargo xtask bundle`
signs with it, and rebuilds keep the permission. Without it, the build signs
ad-hoc and prints a warning. `--adhoc` forces an ad-hoc signature;
`CLICKY_SIGNING_IDENTITY` picks another identity by name or SHA-1.

## Diagnostics log

Open the menu with **Option** held to show **Diagnostics Log**. When on, Clicky
appends audio start/stop (with the time to the first render), input state and
errors to `~/Library/Logs/Clicky/Clicky.log`. It never records keys or
characters.

## Layout

See `docs/architecture.md`. Soundpack source audio and its licences are in
`assets/soundpacks/`; credits are in `THIRD_PARTY_NOTICES.md`.
