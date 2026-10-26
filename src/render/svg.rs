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
/// dbdiagram's row icon paths (10.5-grid), shared with the canvas.
const PK_ICON: &str = "M 6.5625 0.9844 C 8.1935 0.9844 9.5156 2.3065 9.5156 3.9375 C 9.5156 5.5685 8.1935 6.8906 6.5625 6.8906 C 6.1735 6.8906 5.8022 6.8153 5.462 6.6786 L 4.5938 7.5469 H 3.9375 V 8.5313 H 2.9531 V 9.5156 H 0.9844 V 7.5469 L 3.736 4.7952 C 3.6518 4.5171 3.6092 4.2281 3.6094 3.9375 C 3.6094 2.3065 4.9315 0.9844 6.5625 0.9844 Z M 6.5625 0 C 4.388 0 2.625 1.7627 2.625 3.9375 C 2.625 4.1186 2.6373 4.2989 2.6619 4.4772 L 0.1441 6.995 C 0.0519 7.0873 0 7.2125 0 7.343 L 0 10.0078 C 0 10.2797 0.2204 10.5 0.4922 10.5 H 3.4453 C 3.7171 10.5 3.9375 10.2797 3.9375 10.0078 V 9.5156 H 4.4297 C 4.7015 9.5156 4.9219 9.2953 4.9219 9.0234 V 8.6133 L 5.7432 7.7895 C 6.011 7.8464 6.2848 7.875 6.5625 7.875 C 8.737 7.875 10.5 6.1123 10.5 3.9375 C 10.5 1.763 8.7373 0 6.5625 0 Z";
const FK_ICON: &str = "M 4.6244 6.6258 c -0.2002 0 -0.3885 -0.078 -0.5304 -0.2197 c -1.1204 -1.1205 -1.1204 -2.9437 0 -4.0641 l 1.5016 -1.5016 c 0.5419 -0.542 1.2635 -0.8405 2.032 -0.8405 s 1.4901 0.2985 2.032 0.8405 c 1.1204 1.1204 1.1204 2.9436 0 4.0641 l -1.001 1.001 c -0.1416 0.1417 -0.3299 0.2197 -0.5304 0.2198 c -0.2002 0 -0.3885 -0.078 -0.5304 -0.2197 c -0.1416 -0.1417 -0.2198 -0.3301 -0.2198 -0.5305 c 0 -0.2004 0.0781 -0.3888 0.2198 -0.5305 l 1.001 -1.001 c 0.5355 -0.5355 0.5355 -1.4068 0 -1.9423 c -0.2588 -0.2588 -0.6039 -0.4015 -0.9712 -0.4015 s -0.7123 0.1426 -0.9712 0.4015 l -1.5016 1.5016 c -0.5355 0.5355 -0.5355 1.4068 0 1.9422 c 0.2925 0.2925 0.2925 0.7684 0 1.0609 c -0.1419 0.1417 -0.3302 0.2197 -0.5304 0.2197 z M 2.8725 10.5 c -0.7685 0 -1.4901 -0.2985 -2.032 -0.8405 c -1.1204 -1.1204 -1.1204 -2.9436 0 -4.0641 l 0.7509 -0.7509 c 0.1416 -0.1416 0.3302 -0.2197 0.5304 -0.2197 c 0.2002 0 0.3888 0.0781 0.5304 0.2198 c 0.1416 0.1416 0.2198 0.33 0.2198 0.5304 c 0 0.2004 -0.0781 0.3888 -0.2198 0.5305 l -0.7509 0.7508 c -0.5355 0.5354 -0.5355 1.4067 0 1.9422 c 0.2588 0.2588 0.6039 0.4015 0.9712 0.4015 s 0.7123 -0.1425 0.9712 -0.4015 l 1.5016 -1.5016 c 0.5355 -0.5355 0.5355 -1.4068 0 -1.9422 c -0.1416 -0.1417 -0.2198 -0.3301 -0.2198 -0.5305 s 0.0781 -0.3888 0.2198 -0.5305 c 0.1416 -0.1417 0.3302 -0.2197 0.5304 -0.2197 s 0.3888 0.078 0.5304 0.2196 c 1.1204 1.1205 1.1204 2.9437 0 4.0641 l -1.5016 1.5016 c -0.5419 0.542 -1.2635 0.8405 -2.032 0.8405 z";
const NOTE_ICON: &str = "M 9 1.125 C 9.2063 1.125 9.375 1.2938 9.375 1.5 V 6.75 H 7.5 C 7.0852 6.75 6.75 7.0852 6.75 7.5 V 9.375 H 1.5 C 1.2938 9.375 1.125 9.2063 1.125 9 V 1.5 C 1.125 1.2938 1.2938 1.125 1.5 1.125 H 9 Z M 1.5 10.5 H 6.75 H 6.8789 C 7.2773 10.5 7.6594 10.343 7.9406 10.0617 L 10.0617 7.9406 C 10.343 7.6594 10.5 7.2773 10.5 6.8789 V 6.75 V 1.5 C 10.5 0.6727 9.8273 0 9 0 H 1.5 C 0.6727 0 0 0.6727 0 1.5 V 9 C 0 9.8273 0.6727 10.5 1.5 10.5 Z M 3 7.6875 C 3 7.5383 2.9407 7.3952 2.8352 7.2897 C 2.7298 7.1843 2.5867 7.125 2.4375 7.125 C 2.2883 7.125 2.1452 7.1843 2.0398 7.2897 C 1.9343 7.3952 1.875 7.5383 1.875 7.6875 C 1.875 7.8367 1.9343 7.9798 2.0398 8.0853 C 2.1452 8.1907 2.2883 8.25 2.4375 8.25 C 2.5867 8.25 2.7298 8.1907 2.8352 8.0853 C 2.9407 7.9798 3 7.8367 3 7.6875 Z M 2.4375 2.25 C 2.2883 2.25 2.1452 2.3093 2.0398 2.4148 C 1.9343 2.5202 1.875 2.6633 1.875 2.8125 C 1.875 2.9617 1.9343 3.1048 2.0398 3.2102 C 2.1452 3.3157 2.2883 3.375 2.4375 3.375 C 2.5867 3.375 2.7298 3.3157 2.8352 3.2102 C 2.9407 3.1048 3 2.9617 3 2.8125 C 3 2.6633 2.9407 2.5202 2.8352 2.4148 C 2.7298 2.3093 2.5867 2.25 2.4375 2.25 Z M 3 5.25 C 3 5.1008 2.9407 4.9577 2.8352 4.8522 C 2.7298 4.7468 2.5867 4.6875 2.4375 4.6875 C 2.2883 4.6875 2.1452 4.7468 2.0398 4.8522 C 1.9343 4.9577 1.875 5.1008 1.875 5.25 C 1.875 5.3992 1.9343 5.5423 2.0398 5.6478 C 2.1452 5.7532 2.2883 5.8125 2.4375 5.8125 C 2.5867 5.8125 2.7298 5.7532 2.8352 5.6478 C 2.9407 5.5423 3 5.3992 3 5.25 Z";

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

/// The "one" end: a perpendicular tick across the line, sitting in the
/// 10px marker gap before the table edge (their PGt).
fn one_tick_path(end: (f32, f32), inner: (f32, f32)) -> String {
    let (d, n) = dir_normal(end, inner);
    let half = 4.0;
    let out = (end.0 + d.0 * 10.0, end.1 + d.1 * 10.0);
    format!(
        r##"<path d="M {:.1} {:.1} L {:.1} {:.1}" fill="none" stroke="#7f838d" stroke-width="1.5"/>"##,
        out.0 - n.0 * half,
        out.1 - n.1 * half,
        out.0 + n.0 * half,
        out.1 + n.1 * half
    )
}

/// The "many" end: a chevron whose wings sit on the table edge and whose
/// tip points back along the line (9px out), plus an open circle (r=3px)
/// just past the tip at 13px out — their NGt + FGt.
fn many_marker_path(end: (f32, f32), inner: (f32, f32)) -> String {
    let (d, n) = dir_normal(end, inner);
    let spread = 4.0;
    let tip = (end.0 + d.0 * 9.0, end.1 + d.1 * 9.0);
    let c = (end.0 + d.0 * 13.0, end.1 + d.1 * 13.0);
    format!(
        r##"<path d="M {ax:.1} {ay:.1} L {tx:.1} {ty:.1} L {bx:.1} {by:.1}" fill="none" stroke="#7f838d" stroke-width="1.5"/><circle cx="{cx:.1}" cy="{cy:.1}" r="3" fill="none" stroke="#7f838d" stroke-width="1.5"/>"##,
        tx = tip.0,
        ty = tip.1,
        ax = end.0 - n.0 * spread,
        ay = end.1 - n.1 * spread,
        bx = end.0 + n.0 * spread,
        by = end.1 + n.1 * spread,
        cx = c.0,
        cy = c.1,
    )
}

/// Orthogonal waypoint path with filleted corners (dbdiagram's look):
/// straight segments joined by quadratic bends of radius `r` units.
fn rounded_edge_path(pts: &[(f32, f32)], r: f32) -> String {
    if pts.len() < 2 {
        return String::new();
    }
    let mut d = format!("M {:.1} {:.1} ", pts[0].0, pts[0].1);
    for w in pts.windows(3) {
        let (px, py) = (w[0].0, w[0].1);
        let (vx, vy) = (w[1].0, w[1].1);
        let (nx, ny) = (w[2].0, w[2].1);
        let seg_in = ((vx - px).powi(2) + (vy - py).powi(2)).sqrt();
        let seg_out = ((nx - vx).powi(2) + (ny - vy).powi(2)).sqrt();
        let cut = r.min(seg_in / 2.0).min(seg_out / 2.0);
        let din = ((vx - px) / seg_in, (vy - py) / seg_in);
        let dout = ((nx - vx) / seg_out, (ny - vy) / seg_out);
        d.push_str(&format!(
            "L {:.1} {:.1} Q {:.1} {:.1} {:.1} {:.1} ",
            vx - din.0 * cut,
            vy - din.1 * cut,
            vx,
            vy,
            vx + dout.0 * cut,
            vy + dout.1 * cut
        ));
    }
    let last = pts[pts.len() - 1];
    d.push_str(&format!("L {:.1} {:.1}", last.0, last.1));
    d
}

fn push_edge(s: &mut String, r: &EdgeRoute) {
    let units: Vec<(f32, f32)> = r.pts.iter().map(|&(x, y)| pt_px(x, y)).collect();
    let d = rounded_edge_path(&units, 8.0);
    s.push_str(&format!(
        r##"<path d="{d}" fill="none" stroke="#7f838d" stroke-width="1.5"/>"##
    ));
    // cardinality end markers along the (horizontal) end segments
    let first = units[0];
    let second = units[1];
    let last = units[units.len() - 1];
    let penult = units[units.len() - 2];
    let (many_fk, many_ref) = many_ends(r.card);
    if many_fk {
        s.push_str(&many_marker_path(first, second));
    } else {
        s.push_str(&one_tick_path(first, second));
    }
    if many_ref {
        s.push_str(&many_marker_path(last, penult));
    } else {
        s.push_str(&one_tick_path(last, penult));
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
            r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{title_size}" font-weight="bold" fill="#ffffff">{}</text>"##,
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
            r##"<text x="{name_x:.1}" y="{ry:.1}" font-family="sans-serif" font-size="{row_size}" fill="#e8eaed">{}</text>"##,
            escape(&row.name)
        ));
        // dbdiagram's left icon cluster: pk, fk, unique, note — 10.5px
        // glyphs with a 4px gap, at 40% opacity
        let k = CELL_W / 10.0;
        let mut ux = name_x + row.name.width() as f32 * char_w + 8.0 * k;
        let icons = [
            row.pk.then_some(PK_ICON),
            row.fk.then_some(FK_ICON),
            row.note.is_some().then_some(NOTE_ICON),
        ];
        for d in icons.into_iter().flatten() {
            s.push_str(&format!(
                r##"<path d="{d}" transform="translate({ux:.1} {ry:.1}) scale({:.3})" fill="#8b909c" fill-opacity="0.7"/>"##,
                10.5 * k / 10.5
            ));
            ux += 14.5 * k;
        }
        if row.unique {
            let label = "Unique";
            let tw = label.chars().count() as f32 * 0.456 * CELL_W;
            let pad = 0.30 * CELL_W;
            s.push_str(&format!(
                r##"<rect x="{:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="#8b909c" fill-opacity="0.14"/>"##,
                ux,
                ry - 0.30 * CELL_H,
                tw + 2.0 * pad,
                0.60 * CELL_H,
                0.12 * CELL_H
            ));
            s.push_str(&format!(
                r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" fill="#8b909c">{}</text>"##,
                ux + pad,
                ry + row_size * 0.02,
                0.38 * CELL_H,
                escape(label)
            ));
        }
        // right side: type, then the "Not Null" pill flush right; the
        // type's right edge shifts left of the pill (their renderType)
        let nn_w = if row.not_null == Some(true) {
            0.8 * CELL_W + 8.0 * 0.456 * CELL_W
        } else {
            0.0
        };
        if !row.ty.is_empty() {
            let right = x + w - 0.5 * CELL_W - nn_w;
            s.push_str(&format!(
                r##"<text x="{right:.1}" y="{ry:.1}" font-family="sans-serif" font-size="{row_size}" fill="#8b909c" text-anchor="end">{}</text>"##,
                escape(&row.ty)
            ));
        }
        if row.not_null == Some(true) {
            let label = "Not Null";
            let tw = label.chars().count() as f32 * 0.456 * CELL_W;
            let pad = 0.30 * CELL_W;
            let px = x + w - 0.5 * CELL_W - tw - 2.0 * pad;
            s.push_str(&format!(
                r##"<rect x="{px:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="#8b909c" fill-opacity="0.14"/>"##,
                ry - 0.30 * CELL_H,
                tw + 2.0 * pad,
                0.60 * CELL_H,
                0.12 * CELL_H
            ));
            s.push_str(&format!(
                r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" fill="#8b909c">{}</text>"##,
                px + pad,
                ry + row_size * 0.02,
                0.38 * CELL_H,
                escape(label)
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
        assert!(svg.contains("Q ") || svg.contains("L "), "edge path");
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
            svg_of("Table a { id integer [pk, not null] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n");
        // flat charcoal canvas + monochrome box + near-black header band
        assert!(svg.contains(&format!("fill=\"{BG_COLOR}\"")));
        assert!(svg.contains("fill=\"#31353e\""), "table body");
        assert!(svg.contains("fill=\"rgb(31,33,40)\""), "header band");
        // soft drop shadow layers
        assert!(svg.contains("fill-opacity=\"0.30\""));
        assert!(svg.contains("fill-opacity=\"0.13\""));
        // gray icon cluster after the name (filled dbdiagram glyphs), and
        // the NN pill in the field-setting colors
        assert!(svg.contains("fill=\"#8b909c\""));
        assert!(svg.contains(">Not Null</text>"), "Not Null pill");
        assert!(!svg.contains("#d29922") && !svg.contains("#4493f8"));
        // line + cardinality end markers share the line color: the tick on
        // the "one" end and the arrow + circle on the "many" end
        assert_eq!(svg.matches("stroke=\"#7f838d\"").count(), 4);
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
