# dbml-ui

A desktop DBML diagram editor — vim-style editing on the left, a live
[dbdiagram](https://dbdiagram.io)-style ER diagram on the right, rendered
on the GPU with [iced](https://iced.rs).

Previously a terminal app (`tui-db`); migrated to a native window where
the diagram is a real canvas: continuous zoom, pixel-precise pan, and
crisp text at any scale — no terminal graphics protocols involved.

## Features

- **DBML editing with vim controls**: normal / insert / visual modes,
  motions (`h j k l w b e 0 $ ^ gg G { }`) with `[count]` (`3j 5G 2$`), the
  `d`/`c` operators composed with counts, motions, and text objects
  (`d3w 2dd 2d2w d$ c0 dG cG dgg ggdG diw ciw daw caw cc dd C D yy p P x`),
  `r` replace, visual `u`/`U` re-casing, undo/redo, `/` search with `n`/`N`,
  and ex commands (`:w :q :wq :q! :e! :o <file> :export <f.svg|f.png>`
  `:export sql [pg|mysql|mssql|oracle]`).
  Keywords are case-insensitive (`table`, `TABLE`, `TaBlE` all parse).
- **Smooth scrolling**: `Ctrl-d`/`Ctrl-u` and PgUp/PgDn animate over 250ms
  (neoscroll-style easing, re-targetable mid-flight); arrows, Home, End,
  and Delete work in every mode; Shift and CapsLock behave like any text
  editor.
- **Live preview**: the diagram re-parses and re-renders on every
  keystroke (a 200-table document parses + lays out in ~0.2ms).
- **Fluid canvas**: wheel zoom anchored at the cursor (continuous,
  25%–800%), drag to pan (content moves offscreen freely), hover
  highlights tables, drag a table to reposition it **live** (positions
  survive edits for the session). Click a table to jump the editor to
  its definition.
- **Files**: open via `dbml-ui <file>`, `:o`, or Ctrl+O (system file
  picker); saves round-trip byte-identical; `:export` writes the
  dark-themed SVG or a 2x PNG.
- **SQL round-trip**: `:export sql [dialect]` writes Postgres / MySQL /
  MSSQL / Oracle DDL from the diagram; opening a `.sql` dump (or
  `dbml-ui dump.sql`) imports it back into DBML. Both directions are
  pinned by 90+ golden fixtures vendored from the upstream dbml
  project's own test suite.
- **Status bar**: mode chip, cmdline + showcmd, filename with `[+]`
  modified flag, Ln/Col, table count, zoom, parse errors.
- Draggable divider between the panes; `?` opens the key reference.

## Usage

```
cargo run --release -- [file.dbml]
```

| Interaction | Action |
|---|---|
| wheel | zoom at cursor |
| drag empty canvas | pan |
| drag a table | move it (live re-layout) |
| click a table | editor jumps to its definition |
| click editor | place the cursor |
| Tab | switch pane focus |
| Ctrl-s / Ctrl-c | save / quit |
| ? | key reference |

## Architecture

```
src/
  main.rs        iced entry (window, theme, subscriptions, CLI arg)
  app.rs         state + update loop (pan/zoom/drag, keys, files, commands)
  view.rs        pane split + divider + status bar + help overlay
  canvas/
    diagram.rs   ER diagram renderer (boxes/headers/rows/edges at zoom)
    editor.rs    code editor renderer (gutter, cursor, selection)
  editor/
    buffer.rs    text buffer (motions/edits/undo/search, tui-textarea semantics)
    mod.rs        vim engine (modes, operators, counts, registers, cmdline)
lib (dbml-ui):
  dbml/          parser (spans, error recovery) + AST + canonical fmt;
                 SQL exporter and SQL dump importer (pg mysql mssql oracle)
  model.rs       AST -> schema (refs, fk marking, dangling count)
  layout.rs      layered auto-layout + user position overrides
  commands.rs    ex-command parsing
  render/        SVG builder + PNG rasterizer (:export)
```

The parser, model, layout, vim engine, and exporters are pure library
code with thorough unit tests (180+); the GUI layer is thin on top.

## MSRV / dependencies

Built against rustc 1.84 with iced 0.13 (MSRV 1.80). A few transitive
dependencies are pinned in the lockfile to versions that predate
edition-2024/MSRV-1.85 releases (calloop, smithay-clipboard,
proc-macro-crate, wayland-protocols, indexmap, async-lock). On a newer
toolchain, iced 0.14 should be a mechanical upgrade.
