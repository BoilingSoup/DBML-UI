//! Diagram -> SVG. One terminal cell maps to CELL_W x CELL_H SVG units; the
//! rasterizer scales to the actual font metrics times zoom.

use crate::layout::{EdgeRoute, Layout, TableBox};
use crate::model::NodeKind;
use unicode_width::UnicodeWidthStr;

pub const CELL_W: f32 = 10.0;
pub const CELL_H: f32 = 20.0;
/// Diagram-cell margin around the content.
pub const MARGIN_CELLS: f32 = 1.0;

/// Canvas background color; the rasterizer pads viewports with the same
/// value (alpha is dropped by the halfblocks encoder, so padding must not
/// be transparent).
pub const BG_COLOR: &str = "#101216";
pub const BG_RGBA: (u8, u8, u8, u8) = (0x10, 0x12, 0x16, 0xff);

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some((
        ((v >> 16) & 0xff) as u8,
        ((v >> 8) & 0xff) as u8,
        (v & 0xff) as u8,
    ))
}

fn header_rgb(b: &TableBox) -> (u8, u8, u8) {
    b.header_color
        .as_deref()
        .and_then(parse_hex)
        .unwrap_or(match b.kind {
            NodeKind::Table => (47, 122, 220),
            NodeKind::Enum => (145, 104, 219),
        })
}

/// Total SVG size in units, derived from the layout extent.
pub fn svg_size(layout: &Layout) -> (f32, f32) {
    (
        (layout.extent.0 as f32 + MARGIN_CELLS * 2.0) * CELL_W,
        (layout.extent.1 as f32 + MARGIN_CELLS * 2.0) * CELL_H,
    )
}

pub fn build_svg(layout: &Layout) -> String {
    build_svg_with(layout, true)
}

/// `show_text: false` omits every label. Below 1:1 zoom the halfblocks
/// pipeline has ~1 pixel per cell to work with — labels are pure noise at
/// that size, so the zoomed-out view renders as a clean outline minimap.
pub fn build_svg_with(layout: &Layout, show_text: bool) -> String {
    let (w, h) = svg_size(layout);
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">"#
    );
    s.push_str(&format!(
        r##"<rect width="100%" height="100%" fill="{}"/>"##,
        BG_COLOR
    ));
    for r in &layout.edges {
        push_edge(&mut s, r);
    }
    for b in &layout.boxes {
        push_table(&mut s, b, show_text);
    }
    s.push_str("</svg>");
    s
}

fn push_edge(s: &mut String, r: &EdgeRoute) {
    let pts: Vec<String> = r
        .pts
        .iter()
        .map(|(x, y)| {
            format!(
                "{:.1},{:.1}",
                (*x as f32 + MARGIN_CELLS + 0.5) * CELL_W,
                (*y as f32 + MARGIN_CELLS + 0.5) * CELL_H
            )
        })
        .collect();
    s.push_str(&format!(
        r##"<polyline points="{}" fill="none" stroke="#6e7681" stroke-width="1.5"/>"##,
        pts.join(" ")
    ));
}

fn push_table(s: &mut String, b: &TableBox, show_text: bool) {
    let x = (b.x as f32 + MARGIN_CELLS) * CELL_W;
    let y = (b.y as f32 + MARGIN_CELLS) * CELL_H;
    let w = b.w as f32 * CELL_W;
    let h = b.h as f32 * CELL_H;
    let (hr, hg, hb) = header_rgb(b);

    // body with rounded corners
    s.push_str(&format!(
        r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="7" fill="#1c2128" stroke="#444c56"/>"##
    ));
    // header band (rounded on top)
    let r = 7.0f32.min(w / 2.0).min(CELL_H / 2.0);
    s.push_str(&format!(
        r##"<path d="M {x} {yy} A {r} {r} 0 0 1 {xr} {y} L {xwr} {y} A {r} {r} 0 0 1 {xw} {yy} L {xw} {yh} L {x} {yh} Z" fill="rgb({hr},{hg},{hb})"/>"##,
        x = x,
        y = y,
        xw = x + w,
        xr = x + r,
        xwr = x + w - r,
        yy = y + r,
        yh = y + CELL_H,
    ));
    // title
    if show_text {
        let title_size = CELL_H * 0.55;
        s.push_str(&format!(
            r##"<text x="{}" y="{:.1}" font-family="monospace" font-size="{title_size}" font-weight="bold" fill="#ffffff">{}</text>"##,
            x + 2.0 * CELL_W,
            y + CELL_H * 0.70,
            escape(&b.title)
        ));
    }
    // column rows
    let row_size = CELL_H * 0.55;
    for (i, row) in b.rows.iter().enumerate() {
        if !show_text {
            continue;
        }
        let ry = y + (i as f32 + 1.0) * CELL_H + CELL_H * 0.70;
        // dbdiagram-style markers: key icon (pk) / chain link (fk),
        // matching the on-screen canvas
        let cx = x + 1.6 * CELL_W + 2.0;
        let cy = ry - row_size * 0.35;
        if row.pk {
            let (color, w) = ("#d29922", (0.085 * row_size).max(1.0));
            let r = 0.30 * row_size;
            let bx = cx - 0.18 * row_size;
            let x0 = bx + r * 0.72;
            let x1 = cx + 0.45 * row_size;
            s.push_str(&format!(
                r#"<circle cx="{bx:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{color}" stroke-width="{w:.1}"/>"#
            ));
            s.push_str(&format!(
                r#"<path d="M {x0:.1} {cy:.1} L {x1:.1} {cy:.1} M {t1:.1} {cy:.1} V {te:.1} M {t2:.1} {cy:.1} V {te:.1}" fill="none" stroke="{color}" stroke-width="{w:.1}"/>"#,
                t1 = x1 - 0.16 * row_size,
                t2 = x1 - 0.02 * row_size,
                te = cy + 0.26 * row_size,
            ));
        } else if row.fk {
            let (color, w) = ("#4493f8", (0.085 * row_size).max(1.0));
            let r = 0.26 * row_size;
            let off = 0.30 * row_size;
            s.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{color}" stroke-width="{w:.1}"/>"#,
                cx - off
            ));
            s.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{color}" stroke-width="{w:.1}"/>"#,
                cx + off
            ));
        }
        s.push_str(&format!(
            r##"<text x="{}" y="{ry:.1}" font-family="monospace" font-size="{row_size}" fill="#e6edf3">{}</text>"##,
            x + 3.2 * CELL_W,
            escape(&row.name)
        ));
        if !row.ty.is_empty() {
            s.push_str(&format!(
                r##"<text x="{}" y="{ry:.1}" font-family="monospace" font-size="{row_size}" fill="#8b949e">{}</text>"##,
                x + (3.2 + row.name.width() as f32 + 0.8) * CELL_W,
                escape(&row.ty)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbml::parser;
    use crate::layout::layout;
    use crate::model::build;

    fn svg_of(src: &str) -> String {
        let out = parser::parse(src);
        build_svg(&layout(&build(&out.doc)))
    }

    #[test]
    fn contains_tables_edges_and_escapes() {
        let svg =
            svg_of("Table a { id integer [pk] }\nTable \"b<x>\" { a_id integer [ref: > a.id] }");
        assert!(svg.contains("<svg"));
        assert!(svg.contains(">a</text>"), "table title");
        assert!(svg.contains(">b&lt;x&gt;</text>"), "escaped title");
        assert!(svg.contains("<polyline"), "edge route");
        assert!(svg.contains("<circle"), "pk key icon marker");
    }

    #[test]
    fn header_color_from_dbml() {
        let svg = svg_of("Table t [headercolor: #123456] { id integer }");
        assert!(svg.contains("fill=\"rgb(18,52,86)\""), "{svg}");
    }

    #[test]
    fn textless_variant_drops_labels_not_boxes() {
        let out = parser::parse("Table users { id integer [pk] }");
        let l = layout(&build(&out.doc));
        let outline = build_svg_with(&l, false);
        assert!(!outline.contains("<text"), "no labels: {outline}");
        assert!(!outline.contains("<circle"), "no pk markers");
        assert!(outline.contains("<rect"), "boxes still drawn");
        assert!(outline.contains(&format!("fill=\"{BG_COLOR}\"")));
        let full = build_svg_with(&l, true);
        assert!(full.contains(">users</text>"));
    }

    #[test]
    fn bg_constants_agree() {
        let (r, g, b) = parse_hex(BG_COLOR).expect("valid hex");
        assert_eq!((r, g, b, BG_RGBA.3), BG_RGBA);
    }

    #[test]
    fn size_scales_with_extent() {
        let svg = svg_of("Table a { id integer }");
        assert!(svg.contains("viewBox=\"0 0 "));
        let (w, h) = svg_size(&layout(&build(
            &parser::parse("Table a { id integer }").doc,
        )));
        assert!(w > 0.0 && h > 0.0);
    }
}
