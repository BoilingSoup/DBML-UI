//! dbml-ui — a desktop DBML diagram editor (iced).
//!
//! Core logic lives in the library so it stays unit-testable headlessly:
//! the DBML parser, the schema model, the auto-layout engine, and the
//! SVG/PNG exporters.

#![forbid(unsafe_code)] // the whole editor is written in safe Rust
pub mod dbml;
pub mod layout;
pub mod model;
pub mod render;

/// The embedded editor mono face (DejaVu Sans Mono, Bitstream-Vera
/// licensed): the web build's only font — text rendering (cosmic-text)
/// and the SVG rasterizer both draw exclusively with it there.
pub const MONO_TTF: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");

/// The bold mono face (doc-tooltip titles on the web).
pub const MONO_TTF_BOLD: &[u8] = include_bytes!("../fonts/DejaVuSansMono-Bold.ttf");

/// Fira Sans bold + semibold: the UI text's weight selects (card titles,
/// section headers) need matching faces on the web, where no system
/// fonts back the family up.
pub const FIRA_SANS_BOLD: &[u8] = include_bytes!("../fonts/FiraSans-Bold.ttf");
pub const FIRA_SANS_SEMIBOLD: &[u8] = include_bytes!("../fonts/FiraSans-SemiBold.ttf");

/// Startup document for a fresh session.
pub const SAMPLE_DBML: &str = r#"Project blog {
  database_type: 'PostgreSQL'
  Note: 'A small blog schema'
}

Table users as U [note: 'Registered accounts'] {
  id integer [pk, increment]
  username varchar(80) [not null, unique]
  email varchar(255) [not null]
  role varchar [default: 'member']
  created_at timestamp [default: `now()`]

  indexes {
    (username, email) [unique, name: 'idx_users_contact']
  }
}

Table posts {
  id integer [pk]
  user_id integer [not null, ref: > U.id]
  status post_status [not null, default: 'draft']
  title varchar(200) [not null]
  body text
  published_at timestamp
}

Table comments {
  id integer [pk]
  post_id integer [not null, ref: > posts.id]
  author_id integer [not null, ref: > U.id]
  body text [not null]
}

Enum post_status {
  draft
  published
  archived
}

Ref: comments.author_id > users.id
"#;
