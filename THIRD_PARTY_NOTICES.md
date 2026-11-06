# Third-party notices

Clicky ships soundpack recordings and code derived from the projects below,
and links the Rust crates listed at the end. Each is used under the licence
quoted here; for crates offered under several licences, Clicky uses them under
the MIT licence.

## Soundpack recordings

### Mechvibes

Cherry MX Black, Red, Brown and Blue (ABS and PBT each), Topre Purple Hybrid
(PBT), Everglide Crystal Purple and Everglide Oreo, from Mechvibes by Hai
Nguyen, https://github.com/hainguyents13/mechvibes-electron (commit
`326252a13e7bef4f1c35d08ef0189b5af6f8ba02`, `src/audio/`), MIT License. The
original files are kept unmodified in `assets/soundpacks/mechvibes/`; Clicky
converts them at build time (decode, slice per key, trim, loudness-match).

The repository was published as `hainguyents13/mechvibes` and later renamed to
`hainguyents13/mechvibes-electron`; the commit above lives there.

```
MIT License

Copyright (c) 2021 Hai Nguyen

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### kbsim

Gateron Black Ink, Gateron Red Ink, NovelKeys Cream, Alpaca, Turquoise
Tealios, Holy Panda, Topre, Kailh Box Navy, SKCM Blue Alps and Buckling Spring,
from kbsim by Thomas Lai, https://github.com/tplai/kbsim (commit
`ba103f3b0afa9dab80447aa2e7e2ed80b6bd80e4`, `src/assets/audio/`), MIT License.
The original files are kept unmodified in `assets/soundpacks/kbsim/`; Clicky
converts them at build time (decode, map rows to keys, trim, loudness-match).

```
Copyright (c) Thomas Lai

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the "Software"), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## Code

### Rustyvibes 2

The soundpack pipeline (`crates/xtask`: decoding, Mechvibes sprite slicing,
the kbsim adapter, trimming, dithering, app bundling), the macOS key geometry
and Mechvibes key-code table (`crates/clicky-pack`), the event-tap and
AudioToolbox FFI, the SPSC ring, the AUHAL output adapter, the soundpack
picker, the About credits and the pick preview's rhythm (`crates/clicky`) are
derived from Rustyvibes 2 by Kunal Bagaria,
https://github.com/KunalBagaria/rustyvibes-v2, MIT License. Each derived file
says so in its header.

```
MIT License

Copyright (c) 2021–2026 Kunal Bagaria

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Rust crates linked into the app

| Crate | Version | Licence |
|---|---|---|
| bitflags | v2.13.2 | MIT OR Apache-2.0 |
| block2 | v0.6.2 | MIT |
| libc | v0.2.191 | MIT OR Apache-2.0 |
| objc2 | v0.6.5 | MIT |
| objc2-app-kit | v0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-core-foundation | v0.3.2 | Zlib OR Apache-2.0 OR MIT |
| objc2-encode | v4.1.0 | MIT |
| objc2-foundation | v0.3.2 | MIT |

Build-time only (not shipped in the app): `symphonia` (MPL-2.0) decodes the
source audio and `serde_json` (MIT OR Apache-2.0) reads the soundpack configs,
both in `crates/xtask`.

### objc2, block2, objc2-encode, objc2-foundation and the objc2 framework crates

https://github.com/madsmtm/objc2

```
Copyright 2026 Mads Marquart

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### bitflags

https://github.com/bitflags/bitflags

```
Copyright (c) 2014 The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```

### libc

https://github.com/rust-lang/libc

```
Copyright (c) The Rust Project Developers

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
```
