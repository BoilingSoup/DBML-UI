//! Diagram -> SVG. One terminal cell maps to CELL_W x CELL_H SVG units; the
//! rasterizer scales to the actual font metrics times zoom.

use crate::layout::{many_ends, EdgeRoute, Layout, TableBox};
use unicode_width::UnicodeWidthStr;

pub const CELL_W: f32 = 10.0;
pub const CELL_H: f32 = 20.0;
/// Diagram-cell margin around the content.
pub const MARGIN_CELLS: f32 = 1.0;

/// Canvas background color; the rasterizer pads viewports with the same
/// value (alpha is dropped by the halfblocks encoder, so padding must not
/// be transparent).
pub const BG_COLOR: &str = "#2b2e36";
pub const BG_RGBA: (u8, u8, u8, u8) = (0x2b, 0x2e, 0x36, 0xff);

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
        // dbdiagram's dark theme uses one near-black header band for both
        // tables and enums; a DBML headercolor override still wins
        .unwrap_or((0x1f, 0x21, 0x28))
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

/// Route point in SVG units.
fn pt_px(x: f32, y: f32) -> (f32, f32) {
    ((x + MARGIN_CELLS) * CELL_W, (y + MARGIN_CELLS) * CELL_H)
}

/// Unit direction `a -> b` and its perpendicular, in SVG units.
fn dir_normal(a: (f32, f32), b: (f32, f32)) -> ((f32, f32), (f32, f32)) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON {
        ((1.0, 0.0), (0.0, 1.0))
    } else {
        let d = (dx / len, dy / len);
        (d, (-d.1, d.0))
    }
}

/// The "one" end: a perpendicular dash across the line at the table edge.
fn one_tick_path(end: (f32, f32), inner: (f32, f32)) -> String {
    let (_, n) = dir_normal(end, inner);
    let half = 0.30 * CELL_H;
    format!(
        r##"<path d="M {:.1} {:.1} L {:.1} {:.1}" fill="none" stroke="#7f838d" stroke-width="1.5"/>"##,
        end.0 - n.0 * half,
        end.1 - n.1 * half,
        end.0 + n.0 * half,
        end.1 + n.1 * half
    )
}

/// The "many" end: three prongs fanning from the table edge back along
/// the line (dbdiagram's crow's foot).
fn crow_foot_path(end: (f32, f32), inner: (f32, f32)) -> String {
    let (d, n) = dir_normal(end, inner);
    let back = 0.55 * CELL_H;
    let spread = 0.30 * CELL_H;
    let heel = (end.0 + d.0 * back, end.1 + d.1 * back);
    format!(
        r##"<path d="M {ex:.1} {ey:.1} L {hx:.1} {hy:.1} M {ex:.1} {ey:.1} L {ax:.1} {ay:.1} M {ex:.1} {ey:.1} L {bx:.1} {by:.1}" fill="none" stroke="#7f838d" stroke-width="1.5"/>"##,
        ex = end.0,
        ey = end.1,
        hx = heel.0,
        hy = heel.1,
        ax = heel.0 + n.0 * spread,
        ay = heel.1 + n.1 * spread,
        bx = heel.0 - n.0 * spread,
        by = heel.1 - n.1 * spread,
    )
}

fn push_edge(s: &mut String, r: &EdgeRoute) {
    let (x0, y0) = pt_px(r.curve[0].0, r.curve[0].1);
    let (x1, y1) = pt_px(r.curve[1].0, r.curve[1].1);
    let (x2, y2) = pt_px(r.curve[2].0, r.curve[2].1);
    let (x3, y3) = pt_px(r.curve[3].0, r.curve[3].1);
    s.push_str(&format!(
        r##"<path d="M {x0:.1} {y0:.1} C {x1:.1} {y1:.1} {x2:.1} {y2:.1} {x3:.1} {y3:.1}" fill="none" stroke="#7f838d" stroke-width="1.5"/>"##
    ));
    // cardinality end markers, oriented along the (horizontal) end tangents
    let (many_fk, many_ref) = many_ends(r.card);
    if many_fk {
        s.push_str(&crow_foot_path((x0, y0), (x1, y1)));
    } else {
        s.push_str(&one_tick_path((x0, y0), (x1, y1)));
    }
    if many_ref {
        s.push_str(&crow_foot_path((x3, y3), (x2, y2)));
    } else {
        s.push_str(&one_tick_path((x3, y3), (x2, y2)));
    }
}

fn push_table(s: &mut String, b: &TableBox, show_text: bool) {
    let x = (b.x as f32 + MARGIN_CELLS) * CELL_W;
    let y = (b.y as f32 + MARGIN_CELLS) * CELL_H;
    let w = b.w as f32 * CELL_W;
    let h = b.h as f32 * CELL_H;
    let (hr, hg, hb) = header_rgb(b);

    // soft drop shadow: two stacked offset fills (no blur needed)
    let sh = 0.22 * CELL_H;
    s.push_str(&format!(
        r##"<rect x="{:.1}" y="{:.1}" width="{w}" height="{h}" fill="#000" fill-opacity="0.30"/>"##,
        x + 0.45 * sh,
        y + 0.8 * sh
    ));
    s.push_str(&format!(
        r##"<rect x="{:.1}" y="{:.1}" width="{w}" height="{h}" fill="#000" fill-opacity="0.13"/>"##,
        x + 0.9 * sh,
        y + 1.6 * sh
    ));
    // body: sharp corners, thin darker border (dbdiagram-style)
    s.push_str(&format!(
        r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#31353e" stroke="#1f2228"/>"##
    ));
    // header band: solid near-black with a hairline below
    s.push_str(&format!(
        r##"<rect x="{x}" y="{y}" width="{w}" height="{ch}" fill="rgb({hr},{hg},{hb})"/>"##,
        ch = CELL_H
    ));
    s.push_str(&format!(
        r##"<path d="M {x} {ybh} L {xwe} {ybh}" stroke="#1f2228" stroke-width="1"/>"##,
        ybh = y + CELL_H,
        xwe = x + w,
    ));

    let title_size = CELL_H * 0.55;
    if show_text {
        // a tiny table glyph before the title, then the bold white title
        let g = CELL_H * 0.5;
        let gx = x + 0.9 * CELL_W;
        let gy = y + (CELL_H - g) / 2.0;
        let r = g * 0.2;
        s.push_str(&format!(
            r##"<path d="M {gxr:.1} {gy:.1} L {gxe:.1} {gy:.1} L {gxw:.1} {gyr:.1} L {gxw:.1} {gyb:.1} L {gx:.1} {gyb:.1} L {gx:.1} {gyr:.1} Z M {gx:.1} {h1:.1} L {gxw:.1} {h1:.1} M {gx:.1} {h2:.1} L {gxw:.1} {h2:.1}" fill="none" stroke="#e8eaed" stroke-width="1"/>"##,
            gxr = gx + r,
            gxe = gx + g - r,
            gxw = gx + g,
            gyr = gy + r,
            gyb = gy + g,
            h1 = gy + g * 0.42,
            h2 = gy + g * 0.71,
        ));
        s.push_str(&format!(
            r##"<text x="{:.1}" y="{:.1}" font-family="monospace" font-size="{title_size}" font-weight="bold" fill="#ffffff">{}</text>"##,
            gx + g + 0.5 * CELL_W,
            y + CELL_H * 0.70,
            escape(&b.title)
        ));
    }
    // column rows: name at the left, gray marker right after it, type
    // right-aligned (matches the on-screen canvas)
    let row_size = CELL_H * 0.55;
    let char_w = 0.6 * row_size;
    for (i, row) in b.rows.iter().enumerate() {
        if !show_text {
            continue;
        }
        let ry = y + (i as f32 + 1.0) * CELL_H + CELL_H * 0.70;
        let name_x = x + 1.0 * CELL_W;
        s.push_str(&format!(
            r##"<text x="{name_x:.1}" y="{ry:.1}" font-family="monospace" font-size="{row_size}" fill="#e8eaed">{}</text>"##,
            escape(&row.name)
        ));
        // marker icon: key (pk) / chain link (fk), gray outline
        let cx = name_x + row.name.width() as f32 * char_w + 1.4 * CELL_W;
        let cy = ry - row_size * 0.35;
        let (color, sw) = ("#8b909c", (0.085 * row_size).max(1.0));
        if row.pk {
            let g = 0.55 * CELL_H;
            let r = 0.30 * g;
            let bx = cx - 0.18 * g;
            let x0 = bx + r * 0.72;
            let x1 = cx + 0.45 * row_size;
            s.push_str(&format!(
                r#"<circle cx="{bx:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{color}" stroke-width="{sw:.1}"/>"#
            ));
            s.push_str(&format!(
                r#"<path d="M {x0:.1} {cy:.1} L {x1:.1} {cy:.1} M {t1:.1} {cy:.1} V {te:.1} M {t2:.1} {cy:.1} V {te:.1}" fill="none" stroke="{color}" stroke-width="{sw:.1}"/>"#,
                t1 = x1 - 0.16 * row_size,
                t2 = x1 - 0.02 * row_size,
                te = cy + 0.26 * row_size,
            ));
        } else if row.fk {
            let r = 0.26 * row_size;
            let off = 0.30 * row_size;
            s.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{color}" stroke-width="{sw:.1}"/>"#,
                cx - off
            ));
            s.push_str(&format!(
                r#"<circle cx="{:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="{color}" stroke-width="{sw:.1}"/>"#,
                cx + off
            ));
        }
        // badges (Not Null / Unique) after the marker zone
        let badge_size = 0.38 * CELL_H;
        let badge_char = 0.6 * badge_size;
        let mut bx = cx + 1.1 * CELL_W;
        for label in row.badges() {
            let tw = label.chars().count() as f32 * badge_char;
            let pad = 0.30 * CELL_W;
            s.push_str(&format!(
                r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="#8b909c" fill-opacity="0.14"/>"##,
                bx,
                ry - row_size * 0.5,
                tw + 2.0 * pad,
                0.60 * CELL_H,
                0.12 * CELL_H
            ));
            s.push_str(&format!(
                r##"<text x="{:.1}" y="{:.1}" font-family="monospace" font-size="{badge_size}" fill="#8b909c">{}</text>"##,
                bx + pad,
                ry + row_size * 0.02,
                escape(label)
            ));
            bx += tw + 2.0 * pad + 0.30 * CELL_W;
        }
        if !row.ty.is_empty() {
            let right = if row.note.is_some() {
                x + w - 1.8 * CELL_W
            } else {
                x + w - 1.2 * CELL_W
            };
            s.push_str(&format!(
                r##"<text x="{:.1}" y="{ry:.1}" font-family="monospace" font-size="{row_size}" fill="#8b909c" text-anchor="end">{}</text>"##,
                right,
                escape(&row.ty)
            ));
        }
        // note paper icon at the far right
        if row.note.is_some() {
            let nx = x + w - 1.0 * CELL_W;
            let ny = ry - row_size * 0.35;
            let g = 0.55 * CELL_H;
            let x0 = nx - 0.40 * g;
            let x1 = nx + 0.40 * g;
            let y0 = ny - 0.50 * g;
            let y1 = ny + 0.50 * g;
            let fold = 0.22 * g;
            let sw = (0.085 * g).max(1.0);
            s.push_str(&format!(
                r##"<path d="M {x0:.1} {y0:.1} L {x1f:.1} {y0:.1} L {x1:.1} {y0f:.1} L {x1:.1} {y1:.1} L {x0:.1} {y1:.1} Z M {x1f:.1} {y0:.1} L {x1f:.1} {y0f:.1} L {x1:.1} {y0f:.1} M {lx0:.1} {ly:.1} L {lx1:.1} {ly:.1} M {lx0:.1} {ly2:.1} L {lx1:.1} {ly2:.1}" fill="none" stroke="#8b909c" stroke-width="{sw:.1}"/>"##,
                x1f = x1 - fold,
                y0f = y0 + fold,
                lx0 = x0 + 0.14 * g,
                lx1 = x1 - 0.14 * g,
                ly = y0 + 0.42 * g,
                ly2 = y0 + 0.64 * g,
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
        assert!(svg.contains("C "), "cubic edge path");
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
    fn dbdiagram_chrome_is_present() {
        let svg =
            svg_of("Table a { id integer [pk] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n");
        // flat charcoal canvas + monochrome box + near-black header band
        assert!(svg.contains(&format!("fill=\"{BG_COLOR}\"")));
        assert!(svg.contains("fill=\"#31353e\""), "table body");
        assert!(svg.contains("fill=\"rgb(31,33,40)\""), "header band");
        // soft drop shadow layers
        assert!(svg.contains("fill-opacity=\"0.30\""));
        assert!(svg.contains("fill-opacity=\"0.13\""));
        // gray markers after the name (not gold/blue before it)
        assert!(svg.contains("stroke=\"#8b909c\""));
        assert!(!svg.contains("#d29922") && !svg.contains("#4493f8"));
        // line + both cardinality end markers share the line color
        assert_eq!(svg.matches("stroke=\"#7f838d\"").count(), 3);
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
