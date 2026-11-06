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

The first time, Clicky shows a short window about its one permission,
**Input Monitoring**; its button opens System Settings → Privacy & Security →
Input Monitoring. Once you turn Clicky on there, sounds start (or the window
offers a single Relaunch). Without the permission the app runs but stays
silent, and its menu says how to fix it. To start over:
`tccutil reset ListenEvent io.github.cavallinilorenzo.clicky`.

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

## Soundpacks and credits

Clicky ships 21 soundpacks, grouped in the menu by switch type:

- **Linear**: Cherry MX Black (ABS, PBT), Cherry MX Red (ABS, PBT), Gateron
  Black Ink, Gateron Red Ink, NovelKeys Cream, Alpaca, Turquoise Tealios.
- **Tactile**: Cherry MX Brown (ABS, PBT), Holy Panda, Topre, Topre Purple
  Hybrid (PBT), Everglide Crystal Purple, Everglide Oreo.
- **Clicky**: Cherry MX Blue (ABS, PBT), Kailh Box Navy, SKCM Blue Alps,
  Buckling Spring.

The recordings come from [Mechvibes](https://github.com/hainguyents13/mechvibes-electron)
by Hai Nguyen (commit `326252a13e7bef4f1c35d08ef0189b5af6f8ba02`) and
[kbsim](https://github.com/tplai/kbsim) by Thomas Lai (commit
`ba103f3b0afa9dab80447aa2e7e2ed80b6bd80e4`), both MIT. Much of Clicky's code is
derived from [Rustyvibes 2](https://github.com/KunalBagaria/rustyvibes-v2) by
Kunal Bagaria, MIT. The source audio is committed unmodified in
`assets/soundpacks/` (see `SOURCES.md` there); `cargo xtask packs` converts it
and matches every pack's loudness. **About Clicky** in the menu shows the
version and these credits; the full licences are in `THIRD_PARTY_NOTICES.md`
and, in the app, `Contents/Resources/Licenses`.

## Layout

See `docs/architecture.md`.
