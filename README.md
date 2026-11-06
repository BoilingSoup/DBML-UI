# dbml-ui

A DBML diagram editor — vim-style editing on the left, a live ER diagram
on the right. Runs as a native desktop app (Rust +
[iced](https://iced.rs), GPU-rendered).

## DBML features

- **Table** blocks: aliases (`Table users as U`), table notes
  (`Table t [note: '...']`), header colors (`headercolor: #3498DB`)
- **Columns**: types, `[pk]` / `[primary key]`, `[unique]`, `[not null]`,
  `[increment]`, `[default: ...]`, `[note: '...']`, inline
  `[ref: > users.id]` refs, pk-bold rendering
- **Ref** blocks and inline refs with all cardinalities — many-to-one
  (`>`), one-to-many (`<`), one-to-one (`-`), plus the `?>` / `<?`
  unnamed variants — named (`Ref user_posts: ...`) and anonymous
- **Enum** blocks
- **TableGroup** blocks
- **Records** blocks (sample data rows, viewable and editable)
- **Project** blocks (`name`, `database_type`, `note`)
- **Indexes** blocks (`unique`, `pk`, composite)
- Notes as `'''multi-line'''` or `'single-line'` strings, `//` and
  `/* */` comments
- The diagram re-parses and re-renders live on every keystroke

## Editor

- Vim modal editing: normal / insert / visual modes, motions with
  `[count]`, `d`/`c` operators with text objects, `yy`/`p`/`P`, `r`,
  `/` search, undo/redo — or a Regular notepad mode (the toggle
  persists)
- Smooth animated scrolling (`Ctrl-d`/`Ctrl-u`), hover tooltips (table
  and column docs, with the diagram's Not null / Enum badges), a
  completion popup, goto-definition (`gd`, `Ctrl-o` back), a
  diagnostics panel with quick fixes

## Diagram

- Live ER diagram beside the editor: drag tables, TableGroups and
  sticky notes (raise order kept); dragged positions persist in the
  session until the file is closed, surviving restarts
- Auto-arrange (left-right layered, referenced tables first) — fresh
  opens and new diagrams start arranged — plus Zoom to Fit
- dbdiagram-style hover cards on tables, columns and groups: note,
  enum values, default, custom properties
- Relationship highlighting: hover a line or a column, or highlight
  every relationship at once
- Records render on the tables as an overlay; sticky notes live on the
  canvas, edited through the source

## Installation

Prebuilt binaries for Linux (tar.gz, .deb), Windows (zip) and macOS
(.app, Apple silicon + Intel) are on the
[Releases](../../releases) page — grab the archive for your OS and run
`dbml-ui`. The Linux tar.gz also ships a `.desktop` launcher and hicolor
icons; the `.deb` installs both system-wide
(`sudo dpkg -i dbml-ui_<version>_amd64.deb`).

Build from source instead (rustc 1.84+):

```
cargo install --path .
```

The window icon embeds at build time on Windows and ships in the bundles
elsewhere; the icon set renders from `icons/app-icon.svg` via
`cargo run --release --example render-icon` if you ever need to
regenerate it.

## Running for development

```
cargo run --release
```

## License

MIT — see [LICENSE.md](LICENSE.md). Bundled fonts and vendored crate
patches carry their own permissive licenses, covered there as well.
