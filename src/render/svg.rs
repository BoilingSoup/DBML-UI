//! Diagram -> SVG. One terminal cell maps to CELL_W x CELL_H SVG units; the
//! rasterizer scales to the actual font metrics times zoom.

use crate::layout::{many_ends, row_icon_path, wrap_chars, EdgeRoute, Layout, NoteBox, RowIcon, TableBox};

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
    let (r, g, bl) = b
        .header_color
        .as_deref()
        .and_then(parse_hex)
        // dbdiagram's dark theme uses one near-black header band for both
        // tables and enums; a DBML headercolor override still wins
        .unwrap_or((0x1f, 0x21, 0x28));
    // the canvas clamps bright bands until the white chrome reads — the
    // export matches it (the same theme::readable_band transform)
    let band = crate::layout::readable_band(iced::Color::from_rgb8(r, g, bl));
    (
        (band.r * 255.0).round() as u8,
        (band.g * 255.0).round() as u8,
        (band.b * 255.0).round() as u8,
    )
}

/// The true content bounds in CELLS: every box, note and group rect,
/// floored at the origin. `layout.extent` only tracks the max side and
/// its note row only counts AUTO-placed stickies — a dragged/resized
/// note below or right of it (and any content at negative cells) sat
/// OUTSIDE the extent, and the export canvas cut it off. Including 0
/// keeps positive-only layouts byte-identical to the old output.
fn content_bounds(layout: &Layout) -> (i32, i32, i32, i32) {
    let mut min_x = 0i32;
    let mut min_y = 0i32;
    let mut max_x = layout.extent.0;
    let mut max_y = layout.extent.1;
    for b in &layout.boxes {
        min_x = min_x.min(b.x);
        min_y = min_y.min(b.y);
        max_x = max_x.max(b.x + b.w);
        max_y = max_y.max(b.y + b.h);
    }
    for n in &layout.notes {
        min_x = min_x.min(n.rect.0);
        min_y = min_y.min(n.rect.1);
        max_x = max_x.max(n.rect.0 + n.rect.2);
        max_y = max_y.max(n.rect.1 + n.rect.3);
    }
    for g in &layout.groups {
        min_x = min_x.min(g.rect.0);
        min_y = min_y.min(g.rect.1);
        max_x = max_x.max(g.rect.0 + g.rect.2);
        max_y = max_y.max(g.rect.1 + g.rect.3);
    }
    (min_x, min_y, max_x, max_y)
}

/// Total SVG size in units, derived from the TRUE content bounds (the
/// layout extent alone misses dragged/resized stickies and anything at
/// negative cells — the export used to cut them off).
pub fn svg_size(layout: &Layout) -> (f32, f32) {
    let (min_x, min_y, max_x, max_y) = content_bounds(layout);
    (
        ((max_x - min_x) as f32 + MARGIN_CELLS * 2.0) * CELL_W,
        ((max_y - min_y) as f32 + MARGIN_CELLS * 2.0) * CELL_H,
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
    // content shifted so negative-cell items land inside the canvas: the
    // offset is 0 for positive-only layouts (identical output), and grows
    // the origin side otherwise
    let (min_x, min_y, _, _) = content_bounds(layout);
    s.push_str(&format!(
        r##"<g transform="translate({:.1} {:.1})">"##,
        -min_x as f32 * CELL_W,
        -min_y as f32 * CELL_H
    ));
    // TableGroup regions render BEHIND everything (the canvas's
    // draw_groups): translucent tinted body, the group-color header band
    // with the label, then a 2px border
    for g in &layout.groups {
        push_group(&mut s, g);
    }
    for r in &layout.edges {
        push_edge(&mut s, r);
    }
    for b in &layout.boxes {
        push_table(&mut s, b, show_text);
    }
    // sticky notes float ABOVE the tables (the canvas draws them last)
    for n in &layout.notes {
        push_note(&mut s, n, show_text);
    }
    s.push_str("</g>");
    s.push_str("</svg>");
    s
}

/// A sticky note: soft-shadowed yellow rounded rectangle with the wrapped
/// text in dark gray — the canvas's draw_notes, mirrored (no name label).
fn push_note(s: &mut String, n: &NoteBox, show_text: bool) {
    let nx = (n.rect.0 as f32 + MARGIN_CELLS) * CELL_W;
    let ny = (n.rect.1 as f32 + MARGIN_CELLS) * CELL_H;
    let nw = n.rect.2 as f32 * CELL_W;
    let nh = n.rect.3 as f32 * CELL_H;
    let k = CELL_W / 10.0;
    let rad = 6.0 * k;
    // soft drop shadow: a translucent rounded rect offset down-right
    s.push_str(&format!(
        r##"<rect x="{:.1}" y="{:.1}" width="{nw:.1}" height="{nh:.1}" rx="{rad:.1}" fill="#000" fill-opacity="0.35"/>"##,
        nx + 2.0 * k,
        ny + 2.0 * k
    ));
    // the sticky itself: plain yellow, no border
    s.push_str(&format!(
        r##"<rect x="{nx:.1}" y="{ny:.1}" width="{nw:.1}" height="{nh:.1}" rx="{rad:.1}" fill="rgb(255,243,176)"/>"##
    ));
    if !show_text {
        return;
    }
    // the wrapped text: the canvas's exact wrap/slot geometry — pad in
    // width cells, one line per 16k slot, capped with a trailing ellipsis
    let pad = 0.4 * CELL_W;
    let line_h = 16.0 * k;
    let wrap_cols = (((nw - 2.0 * pad) / (0.6 * CELL_W)).floor() as usize).max(4);
    let max_lines = ((nh / line_h).floor() as usize).max(1);
    let mut lines = wrap_chars(&n.text, wrap_cols);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            last.push('\u{2026}');
        }
    }
    for (i, line) in lines.iter().enumerate() {
        let cy = ny + pad + (i as f32 + 0.5) * line_h;
        // baseline optically centered in the line slot (cap-height half
        // below the slot's center, the pills' convention)
        s.push_str(&format!(
            r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" fill="#333333">{}</text>"##,
            nx + pad,
            cy + 0.354 * 0.6 * CELL_H,
            0.6 * CELL_H,
            escape(line)
        ));
    }
}

/// The group's rgb triple: its `[color: …]` parsed, the TABLE header
/// near-black otherwise (groups.rs parity — `DEFAULT_GROUP` there is
/// `theme::HEADER_BG`, the same default `header_rgb` uses).
fn group_rgb(color: &Option<String>) -> (u8, u8, u8) {
    let default = (0x1f, 0x21, 0x28);
    let Some(hex) = color else { return default };
    let h = hex.trim_start_matches('#');
    if h.len() != 6 {
        return default;
    }
    let v = |r: std::ops::Range<usize>| u8::from_str_radix(&h[r], 16).unwrap_or(0);
    (v(0..2), v(2..4), v(4..6))
}

fn push_group(s: &mut String, g: &crate::layout::GroupRegion) {
    let gx = (g.rect.0 as f32 + MARGIN_CELLS) * CELL_W;
    let gy = (g.rect.1 as f32 + MARGIN_CELLS) * CELL_H;
    let gw = g.rect.2 as f32 * CELL_W;
    let gh = g.rect.3 as f32 * CELL_H;
    let (r, gc, b) = group_rgb(&g.color);
    let rgb = format!("rgb({r},{gc},{b})");
    // radius matches the canvas: one cell width, on the top corners of
    // the band and all four of the body
    let rad = CELL_W;
    // translucent tinted body
    s.push_str(&format!(
        r##"<rect x="{gx:.1}" y="{gy:.1}" width="{gw:.1}" height="{gh:.1}" rx="{rad:.1}" fill="{rgb}" fill-opacity="0.12"/>"##
    ));
    // header band: one cell tall, rounded top corners only (a rounded
    // rect squared off below its midline), carrying the group name
    let band_h = CELL_H;
    s.push_str(&format!(
        r##"<rect x="{gx:.1}" y="{gy:.1}" width="{gw:.1}" height="{band_h:.1}" rx="{rad:.1}" fill="{rgb}"/>"##
    ));
    s.push_str(&format!(
        r##"<rect x="{gx:.1}" y="{mid:.1}" width="{gw:.1}" height="{bh:.1}" fill="{rgb}"/>"##,
        mid = gy + band_h / 2.0,
        bh = band_h / 2.0,
    ));
    s.push_str(&format!(
        r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" font-weight="bold" fill="#ffffff">{}</text>"##,
        gx + 0.6 * CELL_W,
        gy + CELL_H * 0.70,
        CELL_H * 0.55,
        escape(&g.name)
    ));
    // 2px border over everything
    s.push_str(&format!(
        r##"<rect x="{gx:.1}" y="{gy:.1}" width="{gw:.1}" height="{gh:.1}" rx="{rad:.1}" fill="none" stroke="{rgb}" stroke-width="{sw:.1}"/>"##,
        sw = 2.0 * (CELL_W / 10.0),
    ));
}

/// Route point in SVG units.
fn pt_px(x: f32, y: f32) -> (f32, f32) {
    ((x + MARGIN_CELLS) * CELL_W, (y + MARGIN_CELLS) * CELL_H)
}

/// Unit direction `a -> b` and its perpendicular, in SVG units.
/// dbdiagram's row icon paths (10.5-grid), shared with the canvas.
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
    // the line's color mirrors the canvas: explicit `[color:]`, else the
    // referenced table's header color (`inherited`), else the gray default
    let stroke = r
        .color
        .as_deref()
        .or(r.inherited.as_deref())
        .unwrap_or("#7f838d");
    s.push_str(&format!(
        r##"<path d="{d}" fill="none" stroke="{stroke}" stroke-width="1.5"/>"##
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
        // bold white title at the 0.6-cell inset (the canvas removed the
        // pictogram that used to live left of it)
        s.push_str(&format!(
            r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{title_size}" font-weight="bold" fill="#ffffff">{}</text>"##,
            x + 0.6 * CELL_W,
            y + CELL_H * 0.70,
            escape(&b.title)
        ));
    }
    // column rows: name at the left, gray marker right after it, type
    // right-aligned (matches the on-screen canvas)
    let row_size = CELL_H * 0.55;
    for (i, row) in b.rows.iter().enumerate() {
        if !show_text {
            continue;
        }
        let ry = y + (i as f32 + 1.0) * CELL_H + CELL_H * 0.70;
        // the row's VISUAL center: the canvas centers the text, glyphs and
        // pills on y_of — the cell's midpoint, 0.20*CELL_H above the text
        // baseline (the text's own visual center lands there via its cap
        // height). Anchoring the chrome to the baseline sagged it 4px.
        let cy = ry - CELL_H * 0.20;
        let name_x = x + 1.0 * CELL_W;
        s.push_str(&format!(
            r##"<text x="{name_x:.1}" y="{ry:.1}" font-family="sans-serif" font-size="{row_size}" fill="#e8eaed">{}</text>"##,
            escape(&row.name)
        ));
        // dbdiagram's left icon cluster, the canvas's exact set: pk, fk,
        // unique (the fingerprint glyph), note — note rides ANY custom
        // property, and the estimator keeps the icons hugging the name
        let k = CELL_W / 10.0;
        let mut ux = name_x + crate::layout::Row::est_width(&row.name) * CELL_W + 8.0 * k;
        let has_info = row.note.is_some() || !row.custom.is_empty();
        let mut icon_specs = Vec::new();
        if row.pk {
            icon_specs.push(RowIcon::Pk);
        }
        if row.fk {
            icon_specs.push(RowIcon::Fk);
        }
        if row.unique {
            icon_specs.push(RowIcon::Unique);
        }
        if has_info {
            icon_specs.push(RowIcon::Note);
        }
        // the glyphs are drawn from their grid origin: lift half a glyph
        // so the 10.5px square centers on the row's VISUAL center (the
        // canvas centers them on y_of, not on the text baseline)
        let icon_y = cy - 5.25 * k;
        for icon in icon_specs {
            s.push_str(&format!(
                r##"<path d="{}" transform="translate({ux:.1} {icon_y:.1}) scale({:.3})" fill="#8b909c" fill-opacity="0.7"/>"##,
                row_icon_path(icon),
                10.5 * k / 10.5
            ));
            ux += 14.5 * k;
        }
        // right side: type, then the "Enum" / "Not Null" pills flush
        // right; the type's right edge shifts left of both pills
        let en_w = if row.ty_enum {
            0.6 * CELL_W + 4.0 * 0.42 * CELL_W
        } else {
            0.0
        };
        let nn_w = if row.not_null == Some(true) {
            0.6 * CELL_W + 8.0 * 0.42 * CELL_W
        } else {
            0.0
        };
        if !row.ty.is_empty() {
            let right = x + w - 0.5 * CELL_W - en_w - nn_w;
            s.push_str(&format!(
                r##"<text x="{right:.1}" y="{ry:.1}" font-family="sans-serif" font-size="{row_size}" fill="#8b909c" text-anchor="end">{}</text>"##,
                escape(&row.ty)
            ));
        }
        if row.ty_enum {
            let label = "Enum";
            let tw = label.chars().count() as f32 * 0.42 * CELL_W;
            let pad = 0.24 * CELL_W;
            let px = x + w - 0.5 * CELL_W - nn_w - tw - 2.0 * pad;
            s.push_str(&format!(
                r##"<rect x="{px:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="#8b909c" fill-opacity="0.14"/>"##,
                cy - 0.275 * CELL_H,
                tw + 2.0 * pad,
                0.55 * CELL_H,
                0.12 * CELL_H
            ));
            s.push_str(&format!(
                r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" fill="#8b909c" text-anchor="middle">{}</text>"##,
                px + tw / 2.0 + pad,
                // the pill's smaller font: the visual center sits 0.354 of
                // ITS size above the baseline (the badge_pill convention)
                cy + (0.35 * CELL_H) * 0.354,
                0.35 * CELL_H,
                escape(label)
            ));
        }
        if row.not_null == Some(true) {
            let label = "Not Null";
            let tw = label.chars().count() as f32 * 0.42 * CELL_W;
            let pad = 0.24 * CELL_W;
            let px = x + w - 0.5 * CELL_W - tw - 2.0 * pad;
            s.push_str(&format!(
                r##"<rect x="{px:.1}" y="{:.1}" width="{:.1}" height="{:.1}" rx="{:.1}" fill="#8b909c" fill-opacity="0.14"/>"##,
                cy - 0.275 * CELL_H,
                tw + 2.0 * pad,
                0.55 * CELL_H,
                0.12 * CELL_H
            ));
            s.push_str(&format!(
                r##"<text x="{:.1}" y="{:.1}" font-family="sans-serif" font-size="{:.1}" fill="#8b909c" text-anchor="middle">{}</text>"##,
                px + tw / 2.0 + pad,
                cy + (0.35 * CELL_H) * 0.354,
                0.35 * CELL_H,
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
    fn sticky_notes_export_with_their_wrapped_text() {
        // sticky notes used to be missing from the export entirely
        let src = "Table t { id int }\n\nNote todo {\n  \'\'\'one two three\'\'\'\n}\n";
        let out = svg_of(src);
        assert!(out.contains("rgb(255,243,176)"), "the yellow sticky: {out}");
        assert!(out.contains(">one two three<"), "the wrapped text: {out}");
        // show_text=false keeps the sticky, omits the text
        let l = layout(&build(&parser::parse(src).doc));
        let bare = build_svg_with(&l, false);
        assert!(bare.contains("rgb(255,243,176)"));
        assert!(!bare.contains(">one two three<"));
    }

    #[test]
    fn uncolored_group_renders_the_table_default() {
        // no [color:] on the group: its band is the SAME near-black a
        // table's header uses (group_rgb's default = header_rgb's
        // default), not dbdiagram's old group blue
        let svg = svg_of("Table a { id integer }\nTableGroup g { a }");
        assert!(svg.contains("fill=\"rgb(31,33,40)\""), "group band: {svg}");
        assert!(!svg.contains("rgb(30,105,253)"), "no group blue: {svg}");
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
    fn a_dragged_note_beyond_the_extent_is_not_cut_off() {
        // the regression: the sticky pinned/dragged BELOW the auto row sat
        // outside the extent — the export canvas ended above it and the
        // note was cut off
        let src = "Table t { id int }\nNote a { '''x''' }\nNote b { '''y''' }\n";
        let mut note_pos = std::collections::HashMap::new();
        note_pos.insert("a".to_string(), (30, 40, 14, 5)); // beyond the extent
        let l = crate::layout::layout_with_all_overrides(
            &build(&parser::parse(src).doc),
            &Default::default(),
            &note_pos,
        );
        let svg = build_svg(&l);
        let (w, h) = svg_size(&l);
        // the note's right edge + bottom edge are inside the canvas
        let note_x = (30.0 + 1.0) * CELL_W; // + margin
        let note_bottom = (40.0 + 5.0 + 1.0) * CELL_H;
        assert!(w >= note_x, "{w} covers note right {note_x}");
        assert!(h >= note_bottom, "{h} covers note bottom {note_bottom}");
        // and the note body is in the output (the y-attribute inside)
        assert!(svg.contains("y=\""), "{svg}");
        // the auto note stays at its classic slot
        let svg_b = build_svg(&l);
        assert!(svg_b.contains("243,176"), "yellow present");
    }

    #[test]
    fn negative_cell_content_shifts_into_the_canvas() {
        // tables can be dragged to negative cells (no clamping): the export
        // must shift them inside rather than clip at x/y=0
        let src = "Table t { id int }\n";
        let mut pos = std::collections::HashMap::new();
        pos.insert("t".to_string(), (-6, -8));
        let l = crate::layout::layout_with_all_overrides(
            &build(&parser::parse(src).doc),
            &pos,
            &Default::default(),
        );
        let svg = build_svg(&l);
        // the whole content sits in a group shifted by -min * cell: the
        // box at (-6,-8) lands inside the canvas (coordinates inside the
        // group are relative to it)
        assert!(
            svg.contains("<g transform=\"translate(60.0 160.0)\">"),
            "origin shifted: {}",
            &svg[..220]
        );
        let (w, h) = svg_size(&l);
        // after the shift the box's top-left lands at the origin margin
        // and its right/bottom stay inside: derive from the real box
        let b = &l.boxes[0];
        let (min_x, min_y, _, _) = content_bounds(&l);
        let shifted_right = (b.x + b.w - min_x + 1) as f32 * CELL_W;
        let shifted_bottom = (b.y + b.h - min_y + 1) as f32 * CELL_H;
        assert!(w >= shifted_right, "{w} covers the shifted box width {shifted_right}");
        assert!(h >= shifted_bottom, "{h} covers the shifted box height {shifted_bottom}");
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
