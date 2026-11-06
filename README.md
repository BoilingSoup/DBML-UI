# dbml-ui

A database ER-diagram editor built on
[DBML](https://dbml.dbdiagram.io/docs/): edit DBML on the left, watch
the ER diagram render live on the right. Native desktop app
(Rust + [iced](https://iced.rs)).

## Features

- Tables, relationships, table groups, sticky notes — most of DBML's
  feature set
- Built-in vim-style editor (a regular editing mode is one toggle
  away)
- Export the diagram as SVG or PNG
- Import SQL dumps (the dialect is auto-detected) and export DDL for
  PostgreSQL, MySQL, SQL Server and Oracle

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

## Running for development

```
cargo run --release
```

## License

MIT — see [LICENSE.md](LICENSE.md). Bundled fonts and vendored crate
patches carry their own permissive licenses, covered there as well.
