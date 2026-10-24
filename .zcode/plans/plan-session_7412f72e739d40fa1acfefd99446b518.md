# tui-db — Terminal DBML Diagram Editor

## Goal
A fast ratatui TUI with a vim-mode DBML editor on one side and a live dbdiagram-style ER diagram on the other. Reads/writes `.dbml` files. Full mouse support: drag to pan (tables slide offscreen edges, like the browser app), wheel to zoom when the terminal can render images, draggable pane divider, click a table to jump to its source. Render backend auto-detects in order: **kitty → sixel → text fallback** (zoom disabled in text mode; pan still works).

Git repo is already initialized on `master` (done before planning).

## Dependency stack (verified on crates.io against rustc 1.84)

The critical finding: current `ratatui` is 0.30, but `tui-textarea` 0.7 requires ratatui `^0.29`, while `ratatui-image` 11.x requires ratatui `^0.30` — they can't share a widget tree. The clean all-off-the-shelf pairing is the **ratatui 0.29 line**:

| Crate | Version | Role |
|---|---|---|
| ratatui | 0.29 | TUI framework, diffed rendering |
| crossterm | 0.28 | events + full mouse (drag/wheel/motion) |
| ratatui-image | 9.0.0 | kitty + sixel backends, terminal capability detection (`Picker::from_query_stdio`) |
| tui-textarea | 0.7.0 | editor engine (built for ratatui 0.29; its repo ships a vim-mode example) |
| resvg | **0.45.1** | SVG → PNG rasterization. (0.48 needs MSRV 1.85/1.89 > installed rustc 1.84; 0.45.1 MSRV is 1.67. Bump later if you `rustup update`.) |
| image | 0.25 (`png` feature) | decode/copy RGBA buffers |
| winnow | 1.0 (fallback 0.7 if its MSRV exceeds 1.84 — verify at scaffold) | hand-written DBML parser with spans |
| clap | 4 | CLI |

## Architecture (single binary `tui-db`, worker-thread pipeline)

```
crossterm events ──► app event loop (UI thread, ratatui draw)
     │  edits debounced ~120ms
     ▼
 worker thread: parse (winnow) → model → layout → render
     ├─ text mode: cell grid (box-drawing chars)
     └─ image mode: SVG string → resvg PNG at viewport_px × zoom
 results tagged with generation counter; UI drops stale ones
```

Modules:
- `dbml/` — lexer+parser → AST with byte spans; error recovery (collect all errors, keep going); supports Project, Table (+aliases, settings `pk increment unique not null default note ref color`), Ref (`< > - <>`, composite, inline `[ref: > ...]`), Enum, TableGroup, notes, `//` comments.
- `model/` — AST → normalized schema (tables, columns, refs with cardinality, enums, groups).
- `layout/` — schema → geometry: table boxes (x, y, w, h in abstract units), column-row anchors, edge routing waypoints. Deterministic auto-layout (dependency-BFS layering + row packing) so it's unit-testable; **shared by both renderers** (also powers mouse hit-testing).
- `render/text.rs` — diagram → ratatui widgets: box-drawing borders, column lists, relationship lines (crow's-foot-ish ASCII). Pan = render offset; offscreen clipping falls out of ratatui clipping for free.
- `render/svg.rs` — diagram → SVG (dbdiagram-like styling); doubles as `:export diagram.svg` output.
- `render/raster.rs` — resvg → RGBA sized to `viewport_px × zoom`; cache keyed `(layout_hash, zoom)`. **Pan between zooms is a pure crop of the cached raster — no re-render.** Zoom steps (×1.25, clamp 0.25–8) trigger one re-raster, centered on the cursor.
- `editor/` — tui-textarea + custom vim layer: normal/insert/visual modes; motions `h j k l w b e 0 $ ^ g g G { }`, operators `x dd dw yy p P c?`, insert entries `i a o O`, undo/redo, `/` search with `n N`, and ex commands `:w :q :q! :wq :o <file> :e <file> :export <file.svg|png>`.
- `ui/` — split panes with draggable divider, status bar (filename, modified `●`, render mode, cursor pos, zoom), parser errors surfaced in editor gutter via spans, `?` help overlay.
- `app/` — event loop, debounce, worker orchestration, pan/zoom state (viewport origin + scale), hit-testing for click-to-jump.

## Mouse & key interaction spec
- Left-drag on canvas = pan (both modes; content clips at edges exactly like dbdiagram's browser pan).
- Wheel = zoom at cursor (kitty/sixel only); in text mode wheel scrolls vertically instead, zoom disabled.
- Drag the divider = resize panes; Tab = cycle focus editor ↔ canvas; Ctrl-s = save.
- Click a table = editor cursor jumps to that table's DBML block (hit-test through the shared layout + viewport transform).
- Backend selection: query terminal at startup → kitty → sixel → text. `--render auto|kitty|sixel|text` overrides; `--dump-png out.png` headless export for debugging.

## Performance plan
- UI thread never parses or rasters: debounced worker pipeline, stale generations dropped.
- Structural hashing: skip re-layout when only notes/comments changed; PNG cache reused across pans.
- ratatui diffed buffer redraw; pan updates only dirty cells.

## Milestones (each ends in a working build + git commit on master)
1. **Scaffold** — `cargo init`, deps pinned as above, `.gitignore`, ratatui shell: two panes + status bar + raw mouse plumbing. Verify whole dep graph compiles on rustc 1.84 before proceeding.
2. **DBML parser** — full subset above + golden-file unit tests (valid & invalid fixtures), span-accurate errors.
3. **Layout + text renderer** — first end-to-end: typed DBML → boxes/lines in terminal; pan with offscreen clipping. The app is usable here.
4. **Editor integration** — tui-textarea + vim normal/insert modes, debounced live preview, error gutter, syntax highlighting for DBML keywords.
5. **Files & ex commands** — open/save/save-as round-trip byte-identical, modified flag, CLI flags.
6. **Image mode** — SVG renderer + resvg + ratatui-image; auto-detect kitty→sixel; wheel zoom at cursor + drag pan + raster cache; text fallback intact.
7. **Vim extras & polish** — visual mode, yank/paste, undo/redo, search; click-to-jump; `?` help overlay; `:export`; clippy + fmt + perf pass.

**Stretch (flagged droppable):** drag tables to reposition with positions saved in a `*.dbml.layout.json` sidecar.

## Testing & verification
- `cargo test`: parser fixture corpus, layout determinism, vim keymap state transitions, ex command parsing, raster cache behavior.
- Manual matrix: kitty terminal (images + zoom), sixel terminal, plain terminal (text mode, zoom disabled) — confirm pan moves tables offscreen, `:w`/`:o` round-trips files unchanged, typing latency stays imperceptible on a ~50-table schema.
