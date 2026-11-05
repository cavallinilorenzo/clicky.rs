# Soundpack sources

The original recordings of every bundled soundpack, unmodified. `cargo xtask packs`
converts them into `target/packs/<id>.clickypack` at build time; `catalog.json`
lists which soundpacks ship and where each one's files live.

| Directory | Upstream | Commit | Licence |
|---|---|---|---|
| `mechvibes/` | https://github.com/hainguyents13/mechvibes-electron (`src/audio/`) | `326252a13e7bef4f1c35d08ef0189b5af6f8ba02` | MIT © 2021 Hai Nguyen (`mechvibes/LICENSE`) |

Layout: `<source>/<upstream pack directory>/`, plus the source's licence file
at `<source>/LICENSE`. Mechvibes packs are one sprite (`sound.ogg`) sliced by
`[start_ms, duration_ms]` per key in `config.json`.

| Soundpack id | Directory | SHA-256 of the sprite |
|---|---|---|
| `cherry-mx-brown-pbt` | `mechvibes/cherrymx-brown-pbt` | `10390a4325fe218b005c509cac28c90bdd21c16bfcc303fe57d51d2588efd047` |

Issue #7 adds the other 20 soundpacks: the remaining Mechvibes sprites under
`mechvibes/`, and kbsim (https://github.com/tplai/kbsim, commit
`ba103f3b0afa9dab80447aa2e7e2ed80b6bd80e4`, `src/assets/audio/`) under `kbsim/`
with its `LICENSE.md`, plus a `kbsim` source adapter in `crates/xtask/src/sources/`.
