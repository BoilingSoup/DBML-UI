# tui-db

A terminal DBML diagram editor — vim-style editing on the left, a live
[dbdiagram](https://dbdiagram.io)-style ER diagram on the right.

```
┌─ editor ──────────────┐│┌─ canvas ───────────────────────┐
│ Table users as U {    │││ ┌─users──────────┐             │
│   id integer [pk]     │││ │ ◆ id integer   │             │
│   username varchar    │││ │   username ... │──┐          │
│ }                     │││ └────────────────┘  │          │
│ Table posts {         │││       ┌─posts───────┴──┐       │
│   user_id integer ... │││       │ ◇ user_id ...  │       │
└───────────────────────┘││       └────────────────┘       │
 NORMAL  file.dbml  Ln 3, Col 5  tables 2 errors 0
```

## Features

- **DBML editing with vim controls**: normal / insert / visual modes,
  motions (`h j k l w b e 0 $ ^ gg G { }`) with `[count]` (`3j 5G 2$`), the
  `d`/`c` operators composed with counts, motions, and text objects
  (`d3w 2dd 2d2w d$ c0 dG cG dgg ggdG diw ciw daw caw cc dd C D yy p P x`),
  undo/redo, `/` search with `n`/`N`, and ex commands (`:w :q :wq :q!
  :e! :o <file> :export <f.svg|f.png>`).
- **Live preview**: the diagram re-renders ~120ms after you stop typing;
  parse errors are reported in the status bar with the offending line.
- **Renderers** (auto-detected, override with `--render`):
  1. **kitty graphics protocol** — vector-rendered via SVG, zoom ×1.25
     steps (1.0–8.0), pan by drag or `hjkl`
  2. **sixel** — same pipeline, sixel encoding
  3. **text fallback** — box-drawing tables with `◆` pk / `◇` fk markers;
     zoom disabled, pan + wheel scroll
- **Full mouse support**: drag to pan (tables slide offscreen edges just
  like the browser), wheel to zoom (image modes) or scroll (text), drag
  the divider to resize panes, click a table to jump the editor to its
  definition, drag a table to reposition it (kept per-name across edits).
  Virtual scrollbars on the canvas show how much of the diagram is
  offscreen and are draggable to reposition the view.
- **Fast**: a 200-table document parses + lays out in ~0.2ms; the image
  pipeline re-rasterizes only on zoom/layout changes and crops on pan.

## Usage

```
tui-db                    # sample document
tui-db schema.dbml        # open a file
tui-db --render sixel     # force a renderer (auto|kitty|sixel|text)
```

Press `?` inside the app for the keymap. `Ctrl-s` saves, `Ctrl-c` quits.

## Build

```
cargo build --release
```

MSRV: rustc 1.84 (dependency pins in `Cargo.toml` respect it).

## Architecture

```
crossterm events -> app event loop (UI thread)
      edits debounced 120ms -> parse -> model -> layout -> render
                                              text renderer (cells)
                                              svg -> resvg raster -> kitty/sixel
```

- `dbml/` — hand-written recursive-descent parser with byte spans and
  error recovery (broken constructs are skipped, the rest still renders)
- `model.rs` — resolves refs against tables/aliases, marks FKs
- `layout.rs` — deterministic layered layout (referenced tables first),
  orthogonal edge routing that never crosses a table box
- `render/` — `text` (box-drawing into the ratatui buffer), `svg`
  (dbdiagram-style SVG), `raster` (resvg → RGBA at font-size × zoom)
- `editor.rs` — vim modal layer over [tui-textarea]
- `app.rs` — event loop, mouse/keys, pan/zoom state, file I/O, ex commands

## Testing

```
cargo test          # 44 unit + fixture tests
cargo run --example dump_diagram   # render the sample to stdout
cargo run --example perf_check     # pipeline timings
```

[tui-textarea]: https://github.com/rhysd/tui-textarea
