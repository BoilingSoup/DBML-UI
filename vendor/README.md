# vendored iced backend patches

Patched copies of two iced 0.13 backend crates, wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`. The app does not
build correctly without them.

- `iced_tiny_skia` (0.13.0) — canvas text was never clipped at all
  (upstream's `fill_text` stamps `INFINITY` as the clip and the draw
  path ignores it, "TODO: Honor layer!"), so text outside a canvas
  widget's bounds painted across neighboring panes. The vendored copy
  stamps the frame's clip into every text item and honors it in the
  draw path.
- `iced_wgpu` (0.13.5) — upstream already stamps the clip in
  `fill_text`; it only needed the same parent-clip clamp that
  `tiny_skia` got (a sliver computed past the canvas widget's edge
  leaked through even with text stamped).

Both are unmodified crate sources from crates.io plus those two local
changes; each carries the upstream MIT license (iced's LICENSE file
does not ship inside the published crates, so it is added here
verbatim).
