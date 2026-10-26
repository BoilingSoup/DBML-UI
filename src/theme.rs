#![allow(dead_code)] // consumed by the canvas milestones

//! Shared dark palette. The on-screen canvas and the SVG exporter use the
//! same values so `:export` matches what you see.

pub const BG: iced::Color = iced::Color::from_rgb(0.063, 0.071, 0.086); // #101216
pub const PANEL: iced::Color = iced::Color::from_rgb(0.11, 0.129, 0.157); // #1c2128
pub const BORDER: iced::Color = iced::Color::from_rgb(0.267, 0.298, 0.337); // #444c56
pub const TEXT: iced::Color = iced::Color::from_rgb(0.902, 0.929, 0.953); // #e6edf3
pub const DIM: iced::Color = iced::Color::from_rgb(0.545, 0.580, 0.620); // #8b949e

// Diagram palette, lifted from dbdiagram.io's dark theme: flat charcoal
// canvas, monochrome table boxes with soft drop shadows and near-black
// header bands, muted gray markers and relationship lines.
pub const CANVAS: iced::Color = iced::Color::from_rgb(0.169, 0.180, 0.212); // #2b2e36
pub const TABLE_BG: iced::Color = iced::Color::from_rgb(0.192, 0.208, 0.243); // #31353e
pub const TABLE_BORDER: iced::Color = iced::Color::from_rgb(0.122, 0.133, 0.157); // #1f2228
pub const TABLE_HOVER: iced::Color = iced::Color::from_rgb(0.290, 0.314, 0.376); // #4a5060
/// dbdiagram's hovered-table border + hovered-row background, from their
/// theme table (dark values).
pub const TABLE_HOVER_BORDER: iced::Color =
    iced::Color::from_rgb(0x61 as f32 / 255.0, 0x9B as f32 / 255.0, 0xCC as f32 / 255.0);
pub const ROW_HOVER_BG: iced::Color =
    iced::Color::from_rgb(0x4B as f32 / 255.0, 0x4C as f32 / 255.0, 0x53 as f32 / 255.0);
pub const HEADER_BG: iced::Color = iced::Color::from_rgb(0.122, 0.129, 0.157); // #1f2128
pub const ROW_TEXT: iced::Color = iced::Color::from_rgb(0.910, 0.918, 0.929); // #e8eaed
pub const TYPE_TEXT: iced::Color = iced::Color::from_rgb(0.545, 0.565, 0.612); // #8b909c
/// pk/fk marker glyphs — outline gray, matching dbdiagram's row icons.
pub const MARKER: iced::Color = iced::Color::from_rgb(0.545, 0.565, 0.612); // #8b909c
pub const LINE: iced::Color = iced::Color::from_rgb(0.498, 0.514, 0.553); // #7f838d

/// dbdiagram's hovered-relationship highlight, from their theme table in
/// the site JS (`REF_HIGHLIGHT_COLOR`): `#3EA8DE` in the dark theme,
/// `#475aff` in light. The app is dark, so this is the faithful value.
pub const EDGE_HOVER: iced::Color =
    iced::Color::from_rgb(0x3E as f32 / 255.0, 0xA8 as f32 / 255.0, 0xDE as f32 / 255.0);

/// dbdiagram's hover tooltip is a light pill with dark text.
pub const TIP_BG: iced::Color = iced::Color::from_rgb(0.941, 0.945, 0.953); // #f0f1f3
pub const TIP_TEXT: iced::Color = iced::Color::from_rgb(0.2, 0.212, 0.239); // #33363d

pub const PK: iced::Color = iced::Color::from_rgb(0.824, 0.6, 0.133); // ◆ #d29922
pub const FK: iced::Color = iced::Color::from_rgb(0.267, 0.576, 0.973); // ◇ #4493f8

pub fn app() -> iced::Theme {
    iced::Theme::Dark
}

// Syntax colors (GitHub-dark flavored), used by the editor canvas.
pub const SYN_KEYWORD: iced::Color = iced::Color::from_rgb(1.0, 0.482, 0.447); // #ff7b72
pub const SYN_NAME: iced::Color = iced::Color::from_rgb(0.824, 0.659, 1.0); // #d2a8ff
pub const SYN_TYPE: iced::Color = iced::Color::from_rgb(1.0, 0.651, 0.341); // #ffa657
pub const SYN_STRING: iced::Color = iced::Color::from_rgb(0.647, 0.839, 1.0); // #a5d6ff
pub const SYN_NUMBER: iced::Color = iced::Color::from_rgb(0.475, 0.753, 1.0); // #79c0ff

pub const ERR: iced::Color = iced::Color::from_rgb(0.945, 0.298, 0.298); // #f14c4c

/// Elevated surface for hover tooltips — distinct from PANEL so a tooltip
/// stays visible when it overlaps a table body.
pub const TOOLTIP: iced::Color = iced::Color::from_rgb(0.231, 0.251, 0.278); // #3b4047
