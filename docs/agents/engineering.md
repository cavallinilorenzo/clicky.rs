# Engineering config

Read by `/verify`, `/judge`, `/build`, `/ship`, `/ui` and `/release`.

## Risk

- floor: low
- sensitive: no
- high-risk paths:
  - none yet

## Recipe

- setup: `cargo fetch`
- checks:
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `cargo test`
  - `cargo build`
- run: `cargo run`, TBD once the binary exists
- exercise: TBD (CLI or app not defined yet)
- screenshots: none
- test data: synthetic only, generated in tests

## Review

- second_opinion: none

## Release

- release: none
