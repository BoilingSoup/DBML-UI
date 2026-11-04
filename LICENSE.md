# MIT License

Copyright (c) 2026 BoilingSoup

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

---

## Third-party material

This project is MIT-licensed, but it bundles and depends on works under
other licenses. Everything below is permissive; none of it changes the
license of this project's own code.

### Bundled fonts (in `fonts/`)

These are compiled into the web build and shipped with it, so their
license texts are included verbatim in `fonts/`:

- **DejaVu Sans Mono** (`DejaVuSansMono.ttf`) — Bitstream Vera License /
  public domain contributions, see `fonts/LICENSE-DejaVu.txt`.
- **Fira Sans Bold + SemiBold** (`FiraSans-*.ttf`) — SIL Open Font License
  1.1, see `fonts/LICENSE-FiraSans.txt`. The files are unmodified; the
  reserved-font-name rules therefore pose no restrictions on this project.

### Vendored crate sources (in `vendor/`)

Two iced crates are vendored with small local patches and compiled from
this repository:

- `vendor/iced_tiny_skia` and `vendor/iced_graphics` — MIT, copyright
  The Iced Developers. The MIT texts accompany the upstream crates; the
  patches are additions to MIT-licensed code and stay under MIT.

### Dependencies

Everything else is pulled from crates.io as ordinary dependencies — the
Rust ecosystem is essentially all MIT / Apache-2.0 / BSD / Zlib / ISC /
CC0 (the full per-license list is in the lockfile metadata). Two notable
ones:

- `resvg` / `usvg` (SVG → PNG rendering) are **MPL-2.0**: file-level
  copyleft that does not affect this project's own license. Their source
  remains available at the upstream repositories.
- `dbdiagram.io`'s sample DBML (the starter content seeded by "New
  diagram") is used as a small example schema with attribution here;
  the DBML language itself is an open spec documented at
  <https://dbml.dbdiagram.io/docs>.
