# Soundpack sources

The original recordings of every bundled soundpack, unmodified. `cargo xtask packs`
converts them into `target/packs/<id>.clickypack` at build time; `catalog.json`
lists which soundpacks ship, in menu order, their switch type and where each
one's files live.

| Directory | Upstream | Commit | Licence |
|---|---|---|---|
| `mechvibes/` | https://github.com/hainguyents13/mechvibes-electron (`src/audio/`) | `326252a13e7bef4f1c35d08ef0189b5af6f8ba02` | MIT © 2021 Hai Nguyen (`mechvibes/LICENSE`) |
| `kbsim/` | https://github.com/tplai/kbsim (`src/assets/audio/`) | `ba103f3b0afa9dab80447aa2e7e2ed80b6bd80e4` | MIT © Thomas Lai (`kbsim/LICENSE.md`) |

Layout: `<source>/<upstream pack directory>/`, plus the source's licence file
at `<source>/LICENSE*`, all copied byte for byte from the commits above.

- **Mechvibes** packs are one sprite (`sound.ogg`, or `purple.ogg` / `oreo.ogg`
  for the Everglide packs) sliced by `[start_ms, duration_ms]` per key in
  `config.json`, keyed by libuiohook scan codes. Only Cherry MX Black ABS has
  key-up (`"N-up"`) defines, for 48 keys.
- **kbsim** packs are `press/GENERIC_R0`…`R4` (one recording per keyboard row)
  plus `press/{SPACE,ENTER,BACKSPACE}`, and `release/GENERIC` plus the same three
  specials, as MP3. `bluealps/release/GENERIC_long.mp3` is kept as upstream ships
  it but not used (kbsim does not use it either).

| Soundpack id | Switch type | Directory | SHA-256 of the sprite |
|---|---|---|---|
| `cherry-mx-black-abs` | Linear | `mechvibes/cherrymx-black-abs` | `8c23cf05846e7126c3d3b7cde161395dcbabc084493458716bffbdf491ed14ab` |
| `cherry-mx-black-pbt` | Linear | `mechvibes/cherrymx-black-pbt` | `29f3ce7f6b700db7076831ceb2ef8b616652538195690707763563b4a8805b37` |
| `cherry-mx-red-abs` | Linear | `mechvibes/cherrymx-red-abs` | `330f91fc14f5a5232594f17058f02603ea67226fafd362fcf94e90463dbdcb40` |
| `cherry-mx-red-pbt` | Linear | `mechvibes/cherrymx-red-pbt` | `d02e33aa7b4147187306058c08de16c5773db8754c218228b579e05a8e6b3ed8` |
| `gateron-black-ink` | Linear | `kbsim/blackink` | |
| `gateron-red-ink` | Linear | `kbsim/redink` | |
| `novelkeys-cream` | Linear | `kbsim/cream` | |
| `alpaca` | Linear | `kbsim/alpaca` | |
| `turquoise-tealios` | Linear | `kbsim/turquoise` | |
| `cherry-mx-brown-abs` | Tactile | `mechvibes/cherrymx-brown-abs` | `1a77001c5741aff8102c967ba7b8f42d40771506f185174ff40d0de78a265385` |
| `cherry-mx-brown-pbt` (default) | Tactile | `mechvibes/cherrymx-brown-pbt` | `10390a4325fe218b005c509cac28c90bdd21c16bfcc303fe57d51d2588efd047` |
| `holy-panda` | Tactile | `kbsim/holypanda` | |
| `topre` | Tactile | `kbsim/topre` | |
| `topre-purple-hybrid-pbt` | Tactile | `mechvibes/topre-purple-hybrid-pbt` | `af073650844bced0050df018bf872b16307a1143c8f356be5ea75340e6dc0298` |
| `everglide-crystal-purple` | Tactile | `mechvibes/eg-crystal-purple` | `8f5da33383bcbc610fb510e04571c65d07ef6bd214d82fbc39738ee1810b5d84` |
| `everglide-oreo` | Tactile | `mechvibes/eg-oreo` | `6e40d3e8acaf32b0b19b777fb628c0f563a7d29dc4b6b652c1dadff31c828a98` |
| `cherry-mx-blue-abs` | Clicky | `mechvibes/cherrymx-blue-abs` | `8cf6f7dc90f869b5544c4ec8d1fdb8d975c057d9383020920b03cf46323b1ca1` |
| `cherry-mx-blue-pbt` | Clicky | `mechvibes/cherrymx-blue-pbt` | `68d46de5a091e4d6daf468327c87e3e75494e645dacc8d55418d5151aaf84349` |
| `kailh-box-navy` | Clicky | `kbsim/boxnavy` | |
| `skcm-blue-alps` | Clicky | `kbsim/bluealps` | |
| `buckling-spring` | Clicky | `kbsim/buckling` | |

Deliberately not shipped (as in Rustyvibes 2): kbsim `mxblack`, `mxblue` and
`mxbrown` (Mechvibes has keycap-specific recordings of the same switches); Mechvibes `*-travel`,
`holy-pandas` and `turquoise` (copies of the kbsim recordings) and `nk-cream`
(kbsim's NovelKeys Cream is used instead).

Size: about 19.2 MiB for the Mechvibes sprites and 0.5 MiB for kbsim. The
converted packs in the app (mono 16-bit, trimmed) total about 15 MB.
