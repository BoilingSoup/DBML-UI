# tui-db → **dbml-ui**: migrate to an iced desktop app

## Decisions (locked in with user)
- **Replace in place + rename** to `dbml-ui` (crate and binary). TUI code is deleted in the first milestone — recoverable from git history if ever needed.
- **iced 0.13.1** (MSRV 1.80 — fits the installed rustc 1.84; iced 0.14 requires 1.88). If transitive deps drift above 1.84, pin them the same way the current Cargo.toml already does. A later 0.14 bump is a mechanical follow-up if the toolchain is ever updated.
- Vim engine is **ported onto a custom buffer**, not iced's `text_editor` widget (its docs confirm no programmatic cursor/selection control — it would fight vim mode).

## What carries over unchanged vs. what dies

**Carries (moved as-is, tests intact):** `dbml/` parser + AST (~2k lines, 12 tests), `model.rs`, `layout.rs` (incl. `layout_with_overrides` for dragged tables), `commands.rs` (ex `:w/:q/:o/:export`), `render/svg.rs` + `render/raster.rs` (kept for `:export svg|png`; its dark palette becomes the shared theme module), `SAMPLE_DBML`.

**Dies:** ratatui/crossterm/tui-textarea/ratatui-image/base64/flate2 deps, `render/text.rs`, `render/kitty.rs`, the entire kitty/halfblocks pipeline, `ui.rs`, old `app.rs`/`main.rs`, scrollbars/throttles/debounce (parse is 0.2ms — reparse every keystroke, no debounce).

**Ports (copy + adapt, tests carried as the spec):** `editor.rs` vim engine (modes, Pending ops, counts, registers, search, cmdline) onto a new custom `Buffer` that mirrors the tui-textarea API surface it used (lines/cursor/insert/delete/CursorMove-like enum/selection/search).

## Target layout (single crate `dbml-ui`)

```
src/
  main.rs           iced entry: application(title, update, view).theme().subscription().run();
                    manual CLI arg parse (FILE) — clap dropped
  app.rs            State (source, layout, table_pos, zoom, pan_px, focus, editor,
                    cmdline, errors, file/saved_text) + Message enum + update()
  view.rs           row![editor canvas, divider, diagram canvas] + status bar + help overlay
  theme.rs          shared dark palette (extracted from svg.rs constants)
  canvas/
    diagram.rs      canvas::Program: boxes/header bands/rows/◆◇/edges from Layout × scale;
                    two canvas::Cache (content, hover/drag overlay); hit-test helpers
    editor.rs       canvas::Program: gutter, lines, fake cursor (mode-colored),
                    visual-selection spans, search highlights
  interaction.rs    mouse_area + event::listen plumbing: wheel zoom (cursor-anchored,
                    continuous), drag pan, table drag, click-to-jump / click-to-position
  editor/
    buffer.rs       custom Buffer (Vec<String>, cursor, selection) + its tests
    mod.rs          vim engine port (from src/editor.rs) + ported tests
  dbml/ model.rs layout.rs commands.rs render/{svg,raster}.rs   moved as-is
```

## Key mechanics
- **Zoom:** continuous `scale *= exp(wheel_delta)`, clamp [0.25, 8], cursor-anchored (port `zoom_at` math to pixel space). Wheel deltas from the event subscription; last cursor position tracked via `mouse_area::on_move`.
- **Pan:** drag on empty canvas moves `pan_px` freely; content can go offscreen (same UX as before).
- **Table drag:** hit-test via inverse transform; **live** re-layout per mouse move (`layout_with_overrides` ≈0.2ms — no more deferring to release), committed to `table_pos` on release (survives edits, in-memory).
- **Diagram drawing:** mirrors svg.rs geometry (CELL_W/CELL_H units × scale px; rounded-rect paths, polyline edges, monospace canvas Text sized by scale — crisp at every zoom, no frame.scale trickery).
- **Editor rendering:** monospace char metrics measured once at startup (not hardcoded), gutter numbers, Ln/Col tracking, `?` help overlay.
- **Live preview/errors:** parse per keystroke; errors in status bar + gutter marks on offending lines.
- **Click-to-jump:** existing `name_span`s → editor cursor; unchanged logic.

## Milestones (each ends with a green build + commit)
1. **Rename + purge + skeleton** — Cargo.toml → `dbml-ui`, delete all TUI modules/deps, move surviving modules, add iced 0.13.1, empty window titled dbml-ui. Moved-module tests green.
2. **Diagram canvas** — static dark-theme render of SAMPLE_DBML (boxes, headers, rows, markers, edges) + split panes + status bar placeholders.
3. **Interactions** — wheel zoom (anchored, continuous), drag pan, hover highlight, live table drag, click-to-jump.
4. **Editor** — Buffer + vim engine port with all tests; editor canvas (gutter/cursor/selection/search); keyboard routing + Tab focus; live reparse + error display.
5. **Files & commands** — CLI file arg, `:w :q :q! :wq :o :e :export`, Ctrl-s, modified flag, byte-identical round-trip test.
6. **Polish** — help overlay, draggable divider, README rewrite, fmt/clippy/full suite.

## Verification & risks
- `cargo test` after every milestone (parser/model/layout/command suites unchanged; editor tests ported; new tests for zoom-anchor math, hit-test transforms, Buffer ops).
- **Honest limitation:** this sandbox has no display — I can compile and unit-test but not open the window. Visual/interaction verification happens on your desktop via `cargo run -- file.dbml`; I'll flag anything I couldn't exercise.
- Risks: iced 0.13 is mature but its API is exacting — expect small adjustments during implementation; transitive MSRV drift handled by pinning as today.