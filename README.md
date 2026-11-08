# dbml-ui

A database ER-diagram editor built on
[DBML](https://dbml.dbdiagram.io/docs/): edit DBML on the left, watch
the ER diagram render live on the right. Native desktop app
(Rust + [iced](https://iced.rs)).

<img width="1919" height="1016" alt="image" src="https://github.com/user-attachments/assets/5ba1600e-9cd7-4395-9737-003e972dcae0" />


## Features

- Tables, relationships, table groups, sticky notes — most of DBML's
  feature set
- Built-in vim-style editor (a regular editing mode is one toggle
  away)
- Export the diagram as SVG or PNG
- Export the schema as SQL DDL for PostgreSQL, MySQL, SQL Server,
  Oracle and SQLite
- Import SQL dumps — the dialect (PostgreSQL, MySQL, SQL Server,
  Oracle or SQLite) is detected from the dump itself

## Supported platforms

| Platform | Download | Notes |
|---|---|---|
| Linux (x86_64) | tar.gz, .deb, AppImage or .flatpak | glibc 2.29+ (AppImage/Flatpak: 2.31+) — Debian 11+, Ubuntu 20.04+, RHEL 9+, Fedora, Mint 20+, Arch · GTK3 (bundled in the AppImage) · Wayland and X11 |
| Windows 10 / 11 (x86_64) | exe | portable single exe, no installer |
| macOS 11+ (Apple Silicon & Intel) | .app.zip | ad-hoc signed — first launch: right-click → Open, or System Settings → Privacy & Security → Open Anyway (macOS 15+ gatekeeper) |

## Installation

Prebuilt binaries for Linux (tar.gz, .deb, AppImage, .flatpak), Windows
(portable exe) and macOS (.app, Apple silicon + Intel) are on the
[Releases](../../releases) page — grab the download for your OS and
run `dbml-ui`. The Linux tar.gz also ships a `.desktop` launcher and hicolor
icons; the `.deb` installs both system-wide
(`sudo dpkg -i dbml-ui_<version>_amd64.deb`).

The AppImage mounts itself through FUSE (the `fuse3` or `fuse` package;
`libfuse2` is not needed); without FUSE, run it with `--appimage-extract-and-run`.

The .flatpak is a single-file bundle: install it with
`flatpak install ./dbml-ui_<version>_x86_64.flatpak` and launch `dbml-ui`
(or the desktop entry) like any installed app. It runs sandboxed on the
GNOME 47 runtime — file dialogs go through the portals, so Open and Save
reach your whole home directory.

Build from source instead (rustc 1.84+):

```
cargo install --path .
```

## Running for development

```
cargo run --release
```

## License

MIT — see [LICENSE](LICENSE). Bundled fonts and vendored crate
patches carry their own permissive licenses, covered in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
