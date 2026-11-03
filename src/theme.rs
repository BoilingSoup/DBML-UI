#![allow(dead_code)] // consumed by the canvas milestones

//! Shared dark palette. The on-screen canvas and the SVG exporter use the
//! same values so `:export` matches what you see.

/// The editor/diagram monospace font. Desktop resolves `Font::MONOSPACE`
/// through the system font set; the web build has no system fonts at all,
/// so there the family names the EMBEDDED DejaVu Sans Mono (loaded in
/// main via `.font(..)`) — same glyphs, same 0.6 advance ratio.
pub fn mono() -> iced::Font {
    if cfg!(target_arch = "wasm32") {
        iced::Font::with_name("DejaVu Sans Mono")
    } else {
        iced::Font::MONOSPACE
    }
}

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
/// The dark option of the header-color picker (`HEADER_PALETTE`'s third
/// swatch = this default): the hex a table's resolved header color
/// compares against when a relationship line decides whether it HAS a
/// highlight color of its own.
pub const DARK_HEADER_HEX: &str = "#1f2128";
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

/// dbdiagram's field-setting pill (NN): their dark FIELD_SETTING_BG/TEXT.
pub const FIELD_SETTING_BG: iced::Color = iced::Color::from_rgb(0.4, 0.4, 0.4); // #666666
pub const FIELD_SETTING_TEXT: iced::Color = iced::Color::from_rgb(0.773, 0.788, 0.8); // #c5c5c7

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

/// Soft-blue warning squiggles — dbdiagram's dark theme sets its
/// Monaco `editorWarning.foreground` to #64A0DC80; solid form here.
pub const WARN: iced::Color = iced::Color::from_rgb(0.392, 0.627, 0.863); // #64a0dc

/// Elevated surface for hover tooltips — distinct from PANEL so a tooltip
/// stays visible when it overlaps a table body.
pub const TOOLTIP: iced::Color = iced::Color::from_rgb(0.231, 0.251, 0.278); // #3b4047

// Neovim floating-window look (hover docs, completion menu): a shade of
// the background raised just enough to read as its own window, with a
// quiet border — NormalFloat/FloatBorder rather than the light tooltip.
pub const FLOAT_BG: iced::Color = iced::Color::from_rgb(0.086, 0.106, 0.133); // #161b22
pub const FLOAT_BORDER: iced::Color = iced::Color::from_rgb(0.188, 0.212, 0.239); // #30363d

// Overlay scrollbar thumb (Monaco's dark theme): #797979 at 40% while
// idle/hovered, 70% while grabbed. The track stays invisible.
pub const SCROLLBAR_IDLE_ALPHA: f32 = 0.4;
pub const SCROLLBAR_ACTIVE_ALPHA: f32 = 0.7;

/// Parse `#rgb` / `#rrggbb` (dbdiagram's headercolor / group / ref colors).
pub fn hex_color(spec: &str) -> Option<iced::Color> {
    let h = spec.trim().strip_prefix('#')?;
    match h.len() {
        6 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            Some(iced::Color::from_rgb8(
                ((v >> 16) & 0xff) as u8,
                ((v >> 8) & 0xff) as u8,
                (v & 0xff) as u8,
            ))
        }
        3 => {
            let d = |i: usize| u8::from_str_radix(&h[i..i + 1], 16).ok();
            Some(iced::Color::from_rgb8(
                d(0)? * 17,
                d(1)? * 17,
                d(2)? * 17,
            ))
        }
        _ => None,
    }
}

/// The band color that carries the white chrome (title, glyphs) — see
/// layout::readable_band. Re-exported for the bin-side painters.
pub use dbml_ui::layout::readable_band;

#[cfg(test)]
mod hex_tests {
    use super::hex_color;

    #[test]
    fn parses_the_dbdiagram_color_forms() {
        let c = hex_color("#d35400").unwrap();
        assert!((c.r - 0xd3 as f32 / 255.0).abs() < 1e-6);
        assert!((c.g - 0x54 as f32 / 255.0).abs() < 1e-6);
        assert!(hex_color("#abc").is_some());
        assert!(hex_color("#abcd").is_none());
        assert!(hex_color("d35400").is_none());
    }
}

#[cfg(test)]
mod readable_band_tests {
    use super::*;
    use dbml_ui::layout::contrast_ratio;

    #[test]
    fn dark_and_mid_bands_pass_untouched() {
        // the default near-black, posts' purple, the group blue and
        // users' teal all already carry white chrome — no adjustment
        assert_eq!(readable_band(HEADER_BG), HEADER_BG);
        for hex in ["#7a4f9e", "#1E69FD", "#2d5f8a"] {
            let c = hex_color(hex).unwrap();
            assert_eq!(readable_band(c), c, "{hex} is dark enough");
        }
    }

    #[test]
    fn bright_bands_darken_until_the_white_chrome_reads() {
        let yellow = hex_color("#f1c40f").unwrap();
        let band = readable_band(yellow);
        assert_ne!(band, yellow, "the bright band adjusts");
        assert!(
            contrast_ratio(TEXT, band) >= 3.5,
            "white chrome reads on the adjusted band"
        );
        // the adjustment only darkens (every channel shrinks) — the hue
        // stays recognizably yellow
        assert!(band.r < yellow.r && band.g < yellow.g && band.b < yellow.b);
    }
}
