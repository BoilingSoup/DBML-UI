//! Text-mode renderer: draws the layout into a ratatui buffer with
//! box-drawing characters, applying the pan offset and clipping anything
//! outside the viewport.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::layout::{Layout, TableBox};

pub fn render(buf: &mut Buffer, area: Rect, layout: &Layout, pan: (i32, i32)) {
    for r in &layout.edges {
        draw_route(buf, area, &r.pts, pan);
    }
    for b in &layout.boxes {
        draw_table(buf, area, b, pan);
    }
}

// --- connection lines ----------------------------------------------------

/// Connection flags per box-drawing char: (north, east, south, west).
/// `None` means "not a line character".
fn conns(c: char) -> Option<(bool, bool, bool, bool)> {
    Some(match c {
        '─' => (false, true, false, true),
        '│' => (true, false, true, false),
        '┌' => (false, true, true, false),
        '┐' => (false, false, true, true),
        '└' => (true, true, false, false),
        '┘' => (true, false, false, true),
        '├' => (true, true, true, false),
        '┤' => (true, false, true, true),
        '┬' => (false, true, true, true),
        '┴' => (true, true, false, true),
        '┼' => (true, true, true, true),
        ' ' => (false, false, false, false),
        _ => return None,
    })
}

fn char_for(n: bool, e: bool, s: bool, w: bool) -> char {
    // Indexed by n | e<<1 | s<<2 | w<<3.
    const T: [char; 16] = [
        ' ', '│', '─', '└', '│', '│', '┌', '├', '─', '┘', '─', '┴', '┐', '┤', '┬', '┼',
    ];
    let idx = (n as usize) | ((e as usize) << 1) | ((s as usize) << 2) | ((w as usize) << 3);
    T[idx]
}

fn set_line_cell(buf: &mut Buffer, area: Rect, x: i32, y: i32, dir: char) {
    if x < area.x as i32
        || y < area.y as i32
        || x >= area.right() as i32
        || y >= area.bottom() as i32
    {
        return;
    }
    let pos = Position::new(x as u16, y as u16);
    let Some(cell) = buf.cell_mut(pos) else {
        return;
    };
    let existing = cell.symbol().chars().next().unwrap_or(' ');
    let (n, e, s, w) = match conns(existing) {
        Some(c) => c,
        None => {
            // Existing non-line content: don't clobber.
            return;
        }
    };
    let (n, e, s, w) = match dir {
        'H' => (n, true, s, true),
        'V' => (true, e, true, w),
        _ => (n, e, s, w),
    };
    cell.set_char(char_for(n, e, s, w))
        .set_style(Style::new().fg(Color::DarkGray));
}

fn draw_route(buf: &mut Buffer, area: Rect, pts: &[(i32, i32)], pan: (i32, i32)) {
    if pts.len() < 2 {
        return;
    }
    // diagram coords -> absolute buffer coords (view origin + pan)
    let (ox, oy) = (area.x as i32, area.y as i32);
    let pts: Vec<(i32, i32)> = pts
        .iter()
        .map(|&(x, y)| (ox + x + pan.0, oy + y + pan.1))
        .collect();
    // Corners first (they need both segment directions).
    for w in pts.windows(2) {
        let (x1, y1) = w[0];
        let (x2, y2) = w[1];
        if y1 == y2 {
            let (a, b) = if x1 <= x2 { (x1, x2) } else { (x2, x1) };
            for x in a..=b {
                set_line_cell(buf, area, x, y1, 'H');
            }
        } else {
            let (a, b) = if y1 <= y2 { (y1, y2) } else { (y2, y1) };
            for y in a..=b {
                set_line_cell(buf, area, x1, y, 'V');
            }
        }
    }
}

// --- tables ---------------------------------------------------------------

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
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

fn header_style(b: &TableBox) -> Style {
    let (r, g, bl) = match b.header_color.as_deref().and_then(parse_hex) {
        Some(rgb) => rgb,
        None => match b.kind {
            crate::model::NodeKind::Table => (47, 105, 212),
            crate::model::NodeKind::Enum => (122, 86, 168),
        },
    };
    Style::new()
        .fg(Color::White)
        .bg(Color::Rgb(r, g, bl))
        .add_modifier(Modifier::BOLD)
}

fn put(buf: &mut Buffer, area: Rect, x: i32, y: i32, ch: char, style: Style) {
    if x < area.x as i32
        || y < area.y as i32
        || x >= area.right() as i32
        || y >= area.bottom() as i32
    {
        return;
    }
    if let Some(cell) = buf.cell_mut(Position::new(x as u16, y as u16)) {
        cell.set_char(ch).set_style(style);
    }
}

fn put_str(buf: &mut Buffer, area: Rect, x: i32, y: i32, s: &str, style: Style) {
    let mut cx = x;
    for c in s.chars() {
        if cx >= area.right() as i32 {
            break;
        }
        put(buf, area, cx, y, c, style);
        cx += c.width().unwrap_or(0) as i32;
    }
}

fn draw_table(buf: &mut Buffer, area: Rect, b: &TableBox, pan: (i32, i32)) {
    let x0 = area.x as i32 + b.x + pan.0;
    let y0 = area.y as i32 + b.y + pan.1;
    // Offscreen in either axis: skip entirely.
    if x0 >= area.right() as i32 || x0 + b.w <= area.x as i32 {
        return;
    }
    if y0 >= area.bottom() as i32 || y0 + b.h <= area.y as i32 {
        return;
    }

    let border = Style::new().fg(Color::White);
    let hstyle = header_style(b);

    // top border with the title embedded
    put(buf, area, x0, y0, '┌', border);
    for x in (x0 + 1)..(x0 + b.w - 1) {
        put(buf, area, x, y0, '─', border);
    }
    put(buf, area, x0 + b.w - 1, y0, '┐', border);
    // header background spans the full first row
    for x in x0..(x0 + b.w) {
        if let Some(cell) = buf.cell_mut(Position::new(x as u16, y0 as u16)) {
            if x >= area.x as i32 && x < area.right() as i32 {
                cell.set_style(hstyle);
            }
        }
    }
    put_str(buf, area, x0 + 2, y0, &b.title, hstyle);

    // column rows
    for (i, row) in b.rows.iter().enumerate() {
        let y = y0 + 1 + i as i32;
        put(buf, area, x0, y, '│', border);
        put(buf, area, x0 + b.w - 1, y, '│', border);
        let (marker, mstyle) = if row.pk {
            ('◆', Style::new().fg(Color::Yellow))
        } else if row.fk {
            ('◇', Style::new().fg(Color::Cyan))
        } else {
            (' ', Style::new())
        };
        put(buf, area, x0 + 2, y, marker, mstyle);
        put_str(
            buf,
            area,
            x0 + 4,
            y,
            &row.name,
            Style::new().fg(Color::White),
        );
        if !row.ty.is_empty() {
            let tx = x0 + 4 + row.name.width() as i32 + 1;
            put_str(buf, area, tx, y, &row.ty, Style::new().fg(Color::DarkGray));
        }
    }

    // bottom border
    let yb = y0 + b.h - 1;
    put(buf, area, x0, yb, '└', border);
    for x in (x0 + 1)..(x0 + b.w - 1) {
        put(buf, area, x, yb, '─', border);
    }
    put(buf, area, x0 + b.w - 1, yb, '┘', border);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbml::parser;
    use crate::layout::layout;
    use crate::model::build;

    fn rendered(src: &str, area: Rect, pan: (i32, i32)) -> (Buffer, Layout) {
        let out = parser::parse(src);
        let l = layout(&build(&out.doc));
        let mut buf = Buffer::empty(area);
        render(&mut buf, area, &l, pan);
        (buf, l)
    }

    fn sym(buf: &Buffer, x: u16, y: u16) -> String {
        buf.cell(Position::new(x, y)).unwrap().symbol().to_string()
    }

    #[test]
    fn renders_table_and_title() {
        let src = "Table users { id integer [pk] }";
        let (buf, _) = rendered(src, Rect::new(0, 0, 40, 10), (0, 0));
        assert_eq!(sym(&buf, 0, 0), "┌");
        let top: String = (0..14).map(|x| sym(&buf, x, 0)).collect();
        assert!(top.contains("users"), "{top:?}");
        assert_eq!(sym(&buf, 0, 1), "│");
        // marker sits at x0 + 2
        assert_eq!(sym(&buf, 2, 1), "◆");
        assert_eq!(sym(&buf, 0, 2), "└");
    }

    #[test]
    fn renders_relative_to_area_origin() {
        // regression: the canvas pane sits mid-screen; boxes must be drawn
        // relative to the view rect, not at absolute buffer origin
        let src = "Table users { id integer [pk] }";
        let (buf, _) = rendered(src, Rect::new(20, 5, 40, 10), (0, 0));
        assert_eq!(sym(&buf, 20, 5), "┌", "box corner at the view origin");
        let top: String = (20..34).map(|x| sym(&buf, x, 5)).collect();
        assert!(top.contains("users"), "{top:?}");
        assert_eq!(sym(&buf, 20, 6), "│");
        // and pan still shifts within the offset area
        let (buf, _) = rendered(src, Rect::new(20, 5, 40, 10), (3, 2));
        assert_eq!(sym(&buf, 23, 7), "┌");
        assert_eq!(sym(&buf, 20, 5), " ", "old position cleared by diff");
    }

    #[test]
    fn pan_shifts_content() {
        let src = "Table users { id integer [pk] }";
        let (a, _) = rendered(src, Rect::new(0, 0, 40, 10), (0, 0));
        let (b, _) = rendered(src, Rect::new(0, 0, 40, 10), (3, 2));
        assert_eq!(sym(&a, 0, 0), "┌");
        assert_eq!(sym(&b, 3, 2), "┌");
        // original position no longer has the corner
        assert_ne!(sym(&b, 0, 0), "┌");
    }

    #[test]
    fn fully_offscreen_leaves_area_empty() {
        let src = "Table users { id integer [pk] }";
        let (buf, _) = rendered(src, Rect::new(0, 0, 40, 10), (-500, -500));
        for y in 0..10 {
            for x in 0..40 {
                assert_eq!(sym(&buf, x, y), " ", "at {x},{y}");
            }
        }
        // and far positive pan
        let (buf, _) = rendered(src, Rect::new(0, 0, 40, 10), (500, 500));
        for y in 0..10 {
            for x in 0..40 {
                assert_eq!(sym(&buf, x, y), " ", "at {x},{y}");
            }
        }
    }

    #[test]
    fn partial_clip_does_not_panic_and_keeps_visible_part() {
        let src = "Table users { id integer [pk] name varchar }";
        // table starts left of the viewport; right part must be visible
        let (buf, _) = rendered(src, Rect::new(0, 0, 8, 5), (-2, 0));
        let row1: String = (0..8).map(|x| sym(&buf, x, 1)).collect();
        assert!(row1.contains('◆'), "{row1:?}");
        assert!(row1.contains("id"), "{row1:?}");
    }

    #[test]
    fn edges_draw_between_tables() {
        let src = "
Table a { id integer [pk] }
Table b { id integer [pk] ref_a integer }
Ref: b.ref_a > a.id
";
        let (buf, l) = rendered(src, Rect::new(0, 0, 60, 10), (0, 0));
        // a line char must exist strictly between the two boxes (the gap
        // region), not just on their borders
        let a_box = l.boxes.iter().find(|b| b.title == "a").unwrap();
        let b_box = l.boxes.iter().find(|b| b.title == "b").unwrap();
        assert!(b_box.x > a_box.right(), "expected side-by-side layout");
        let gap = a_box.right()..b_box.x;
        let mut found = false;
        for y in 0..10 {
            for x in gap.clone() {
                let s = sym(&buf, x as u16, y);
                if matches!(s.as_str(), "─" | "│" | "┐" | "┌" | "└" | "┘" | "┼") {
                    found = true;
                }
            }
        }
        assert!(found, "no edge line drawn in gap {gap:?}");
    }
}
