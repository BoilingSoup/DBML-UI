use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::app::{App, Focus};

/// Split the body area into (editor, divider, canvas) using the app's split
/// fraction. Also used outside drawing to hit-test mouse events.
pub fn pane_areas(body: Rect, split: f32) -> (Rect, Rect, Rect) {
    let a = (split * 1000.0).clamp(150.0, 850.0) as u16;
    let chunks = Layout::horizontal([
        Constraint::Fill(a),
        Constraint::Length(1),
        Constraint::Fill(1000 - a),
    ])
    .split(body);
    (chunks[0], chunks[1], chunks[2])
}

fn focus_style(focused: bool) -> Style {
    if focused {
        Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let rows = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).split(area);
    let (editor, divider, canvas) = pane_areas(rows[0], app.split);
    app.panes = crate::app::Panes {
        body: rows[0],
        editor,
        divider,
        canvas,
    };

    draw_editor(f, app, editor);
    draw_divider(f, app, divider);
    draw_canvas(f, app, canvas);
    draw_status(f, app, rows[1]);
    if app.show_help {
        draw_help(f, area);
    }
}

fn draw_help(f: &mut Frame, area: Rect) {
    let w = 64u16.min(area.width);
    let h = 26u16.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let popup = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(ratatui::widgets::Clear, popup);
    let block = Block::bordered()
        .title(" help — press ? to close ")
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(popup);
    f.render_widget(block, popup);

    let key = Style::new().fg(Color::Cyan);
    let dim = Style::new().fg(Color::DarkGray);
    let lines: Vec<Line> = vec![
        Line::from(vec![Span::styled("editor (vim)", key), Span::raw("")]),
        Line::from(vec![
            Span::styled("  i a o O I A", dim),
            Span::raw("  enter insert mode"),
        ]),
        Line::from(vec![
            Span::styled("  Esc", dim),
            Span::raw("           back to normal"),
        ]),
        Line::from(vec![
            Span::styled("  h j k l w b e", dim),
            Span::raw("  motions"),
        ]),
        Line::from(vec![
            Span::styled("  0 $ ^ gg G { }", dim),
            Span::raw("  line/paragraph motions"),
        ]),
        Line::from(vec![
            Span::styled("  v  y / d / x", dim),
            Span::raw("   visual select, yank/delete"),
        ]),
        Line::from(vec![
            Span::styled("  d/c + w b e 0 $ G gg", dim),
            Span::raw("  operator + motion"),
        ]),
        Line::from(vec![
            Span::styled("  diw ciw daw caw cc C", dim),
            Span::raw("  word objects / line"),
        ]),
        Line::from(vec![
            Span::styled("  x dd yy p P D", dim),
            Span::raw("  edits"),
        ]),
        Line::from(vec![
            Span::styled("  3x d3w 2dd 5G 2d2w", dim),
            Span::raw("  counts (2d3w == 6w)"),
        ]),
        Line::from(vec![
            Span::styled("  u  Ctrl-r", dim),
            Span::raw("       undo / redo"),
        ]),
        Line::from(vec![
            Span::styled("  /pat  n  N", dim),
            Span::raw("      search"),
        ]),
        Line::from(vec![
            Span::styled("  :w :q :wq :q! :e!", dim),
            Span::raw(" file commands"),
        ]),
        Line::from(vec![
            Span::styled("  :export f.svg|png", dim),
            Span::raw(" export diagram"),
        ]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::styled("canvas", key)]),
        Line::from(vec![
            Span::styled("  Tab", dim),
            Span::raw("            focus editor/canvas"),
        ]),
        Line::from(vec![
            Span::styled("  drag / hjkl", dim),
            Span::raw("      pan (tables slide offscreen)"),
        ]),
        Line::from(vec![
            Span::styled("  wheel", dim),
            Span::raw("          zoom (halfblocks in text mode)"),
        ]),
        Line::from(vec![
            Span::styled("  0", dim),
            Span::raw("                reset pan"),
        ]),
        Line::from(vec![
            Span::styled("  click table", dim),
            Span::raw("      jump editor to its definition"),
        ]),
        Line::from(vec![
            Span::styled("  drag scrollbar", dim),
            Span::raw("    pan to position; shows offscreen content"),
        ]),
        Line::from(vec![
            Span::styled("  drag table", dim),
            Span::raw("       move a table; click jumps to source"),
        ]),
        Line::from(vec![Span::raw("")]),
        Line::from(vec![Span::styled("other", key)]),
        Line::from(vec![
            Span::styled("  Ctrl-s", dim),
            Span::raw("          save   ·   Ctrl-c quit"),
        ]),
        Line::from(vec![
            Span::styled("  drag divider", dim),
            Span::raw("    resize panes"),
        ]),
    ];
    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn draw_editor(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" editor ")
        .border_style(focus_style(app.focus == Focus::Editor));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Fake cursor via cell style (like tui-textarea's own examples).
    app.editor.ta.set_cursor_style(match app.editor.mode {
        crate::editor::Mode::Insert => Style::new().bg(Color::Cyan).fg(Color::Black),
        crate::editor::Mode::Normal => Style::new().bg(Color::DarkGray).fg(Color::White),
        crate::editor::Mode::Visual => Style::new().bg(Color::Magenta).fg(Color::Black),
    });
    f.render_widget(&app.editor.ta, inner);
}

fn draw_divider(f: &mut Frame, app: &App, area: Rect) {
    let style = if app.divider_dragging {
        Style::new().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    for y in area.top()..area.bottom() {
        f.buffer_mut()[(area.left(), y)]
            .set_symbol(symbols::line::VERTICAL)
            .set_style(style);
    }
}

fn draw_canvas(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .title(" canvas ")
        .border_style(focus_style(app.focus == Focus::Canvas));
    let inner = block.inner(area);
    let view = app.view_rect();
    f.render_widget(block, area);

    match app.render_mode {
        crate::render::Mode::Text if (app.zoom - 1.0).abs() <= f32::EPSILON => {
            if let Some(layout) = &app.diagram {
                crate::render::text::render(f.buffer_mut(), view, layout, app.pan);
            }
        }
        _ => {
            // Detect viewport size changes (resize / divider drag) so the
            // event loop re-crops.
            let (fw, fh) = app.font_px();
            let px = (
                (view.width as u32).saturating_mul(fw).max(1),
                (view.height as u32).saturating_mul(fh).max(1),
            );
            if px != app.canvas_px {
                app.image_due = true;
            }
            if app.render_mode != crate::render::Mode::Kitty {
                // kitty images are placed by raw protocol commands between
                // frames; rendering cells here would only erase them
                if let Some(proto) = app.proto.as_mut() {
                    f.render_widget(ratatui_image::Image::new(&*proto), view);
                } else {
                    let para =
                        Paragraph::new("rasterizing…").style(Style::new().fg(Color::DarkGray));
                    f.render_widget(para, view);
                }
                // live drag feedback via buffer cells (halfblocks zoom);
                // kitty draws its outline as raw escapes after the placement
                if app.render_mode == crate::render::Mode::Text {
                    if let Some(o) = app.drag_outline() {
                        draw_drag_outline(f, view, o.x, o.y, o.w, o.h, &o.title);
                    }
                }
                // table titles as real text over the halfblocks minimap:
                // crisp at any zoom, unlike rasterized labels
                if app.render_mode == crate::render::Mode::Text {
                    draw_minimap_titles(f, app, view);
                }
            }
        }
    }
    draw_scrollbars(f, app, inner, view);
}

/// Bold table titles drawn over the zoomed-out halfblocks minimap, so the
/// outline view still tells you which box is which. Truncated to the box's
/// width at the current zoom.
fn draw_minimap_titles(f: &mut Frame, app: &App, view: Rect) {
    use crate::model::NodeKind;
    let Some(layout) = &app.diagram else { return };
    let buf = f.buffer_mut();
    for b in &layout.boxes {
        let budget = (b.w as f32 * app.zoom).floor() as i32;
        if budget < 4 {
            continue;
        }
        let (sx, sy) = app.diagram_to_screen(b.x, b.y);
        if sx < 0 || sy < 0 || sy >= view.bottom() as i32 || sx >= view.right() as i32 {
            continue;
        }
        let fg = b
            .header_color
            .as_deref()
            .and_then(crate::render::svg::parse_hex)
            .map(|(r, g, b_)| Color::Rgb(r, g, b_))
            .unwrap_or(match b.kind {
                NodeKind::Table => Color::Rgb(79, 140, 247),
                NodeKind::Enum => Color::Rgb(161, 120, 217),
            });
        let style = Style::new()
            .fg(fg)
            .bg(Color::Rgb(16, 18, 22))
            .add_modifier(Modifier::BOLD);
        let max_chars = (budget as usize - 2).min(b.title.chars().count());
        for (i, c) in b.title.chars().take(max_chars).enumerate() {
            let x = sx + 1 + i as i32;
            if x >= view.right() as i32 {
                break;
            }
            if let Some(cell) = buf.cell_mut(Position::new(x as u16, sy as u16)) {
                cell.set_symbol(&c.to_string()).set_style(style);
            }
        }
    }
}

/// Proportional-thumb geometry: (thumb_start, thumb_len) as fractions of
/// the track, for a content size, viewport size, and scroll offset.
/// Content that fits shows a full-track thumb.
fn thumb(content: u32, viewport: u32, offset: i32) -> (f32, f32) {
    if content <= viewport {
        return (0.0, 1.0);
    }
    let content = content as f32;
    let viewport = viewport as f32;
    let offset = offset.clamp(0, (content - viewport) as i32) as f32;
    let size = (viewport / content).clamp(0.05, 1.0);
    let pos = offset / (content - viewport);
    (pos * (1.0 - size), size)
}

/// Inverse of `thumb`: the scroll offset that puts the thumb center at
/// `frac` along the track.
pub fn offset_at(content: u32, viewport: u32, frac: f32) -> i32 {
    if content <= viewport {
        return 0;
    }
    let max = (content - viewport) as f32;
    (frac.clamp(0.0, 1.0) * max).round() as i32
}

/// Scrollbar geometry on screen: track column/row plus the inclusive cell
/// range of each thumb. Shared by drawing and mouse hit-testing.
pub struct ScrollBars {
    /// Vertical: (track_col, thumb_start_row, thumb_end_row).
    pub v: Option<(u16, u16, u16)>,
    /// Horizontal: (track_row, thumb_start_col, thumb_end_col).
    pub h: Option<(u16, u16, u16)>,
}

pub fn scrollbar_rects(app: &App, inner: Rect, view: Rect) -> ScrollBars {
    let mut bars = ScrollBars { v: None, h: None };
    if inner.width < 4 || inner.height < 4 {
        return bars;
    }
    let vx = view.right(); // inner.right() - 1
    let hy = view.bottom();

    let track_h = inner.height.saturating_sub(1); // exclude the h-track row
    let (cv, vv, ov) = app.scroll_v();
    let (s, l) = thumb(cv, vv, ov);
    let len = ((l * track_h as f32).ceil() as u16).clamp(1, track_h);
    let start = ((s * track_h as f32).round() as u16).min(track_h - len);
    let t0 = inner.y + start;
    bars.v = Some((vx, t0, t0 + len - 1));

    let track_w = inner.width.saturating_sub(1); // exclude the v-track col
    let (ch, vh, oh) = app.scroll_h();
    let (s, l) = thumb(ch, vh, oh);
    let len = ((l * track_w as f32).ceil() as u16).clamp(1, track_w);
    let start = ((s * track_w as f32).round() as u16).min(track_w - len);
    let t0 = inner.x + start;
    bars.h = Some((hy, t0, t0 + len - 1));
    bars
}

fn draw_scrollbars(f: &mut Frame, app: &App, inner: Rect, view: Rect) {
    let bars = scrollbar_rects(app, inner, view);
    let buf = f.buffer_mut();
    let track_style = Style::new().bg(Color::Rgb(31, 34, 46));
    let thumb_style = Style::new().bg(Color::Rgb(90, 98, 120));
    if let Some((vx, t0, t1)) = bars.v {
        for y in inner.y..view.bottom() {
            let style = if (t0..=t1).contains(&y) {
                thumb_style
            } else {
                track_style
            };
            if let Some(cell) = buf.cell_mut(Position { x: vx, y }) {
                cell.set_char(' ').set_style(style);
            }
        }
    }
    if let Some((hy, t0, t1)) = bars.h {
        for x in inner.x..view.right() {
            let style = if (t0..=t1).contains(&x) {
                thumb_style
            } else {
                track_style
            };
            if let Some(cell) = buf.cell_mut(Position { x, y: hy }) {
                cell.set_char(' ').set_style(style);
            }
        }
    }
    // corner where the tracks meet
    if let (Some((vx, _, _)), Some((hy, _, _))) = (bars.v, bars.h) {
        if let Some(cell) = buf.cell_mut(Position { x: vx, y: hy }) {
            cell.set_char(' ').set_style(track_style);
        }
    }
}

fn draw_drag_outline(f: &mut Frame, view: Rect, x: i32, y: i32, w: i32, h: i32, title: &str) {
    let style = Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
    let buf = f.buffer_mut();
    let mut put = |x: i32, y: i32, ch: &str| {
        if x >= 0 && y >= 0 && (x as u16) < view.right() && (y as u16) < view.bottom() {
            if let Some(cell) = buf.cell_mut(Position::new(x as u16, y as u16)) {
                cell.set_symbol(ch).set_style(style);
            }
        }
    };
    for dx in 0..w {
        put(x + dx, y, "─");
        put(x + dx, y + h - 1, "─");
    }
    for dy in 0..h {
        put(x, y + dy, "│");
        put(x + w - 1, y + dy, "│");
    }
    put(x, y, "┌");
    put(x + w - 1, y, "┐");
    put(x, y + h - 1, "└");
    put(x + w - 1, y + h - 1, "┘");
    let t: Vec<char> = title.chars().collect();
    for (i, c) in t.iter().enumerate().take((w as usize).saturating_sub(2)) {
        put(x + 1 + i as i32, y, &c.to_string());
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();

    if let Some(cmd) = &app.editor.cmdline {
        spans.push(Span::styled(
            format!(" {}{}▏ ", cmd.kind, cmd.input),
            Style::new().fg(Color::Black).bg(Color::Yellow),
        ));
    } else {
        let (label, style) = match app.focus {
            Focus::Canvas => (" CANVAS ", Style::new().fg(Color::Black).bg(Color::Cyan)),
            Focus::Editor => match app.editor.mode {
                crate::editor::Mode::Insert => (
                    " INSERT ",
                    Style::new().fg(Color::Black).bg(Color::LightGreen),
                ),
                crate::editor::Mode::Normal => {
                    (" NORMAL ", Style::new().fg(Color::Black).bg(Color::White))
                }
                crate::editor::Mode::Visual => {
                    (" VISUAL ", Style::new().fg(Color::Black).bg(Color::Magenta))
                }
            },
        };
        spans.push(Span::styled(label, style));
    }

    // vim-style showcmd: in-progress count / pending operator
    if let Some(sc) = app.editor.showcmd() {
        spans.push(Span::styled(
            format!(" {sc}▏ "),
            Style::new().fg(Color::Black).bg(Color::DarkGray),
        ));
    }

    let (row, col) = app.editor.cursor();
    let name = app
        .file
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "[No Name]".to_string());
    let modified = if app.is_modified() { " [+]" } else { "" };
    spans.push(Span::styled(
        format!(
            " tui-db  {name}{modified}  Ln {}, Col {} ",
            row + 1,
            col + 1
        ),
        Style::new().fg(Color::DarkGray),
    ));
    spans.push(Span::styled(
        format!(
            "tables {} ",
            app.diagram.as_ref().map_or(0, |l| l.boxes.len())
        ),
        Style::new().fg(Color::DarkGray),
    ));
    match app.is_image_mode() {
        true => {
            spans.push(Span::styled(
                format!(
                    "{} zoom {:.0}% ",
                    match app.render_mode {
                        crate::render::Mode::Kitty => "kitty",
                        crate::render::Mode::Sixel => "sixel",
                        crate::render::Mode::Text => "halfblocks",
                    },
                    app.zoom * 100.0
                ),
                Style::new().fg(Color::DarkGray),
            ));
        }
        false => {
            spans.push(Span::styled(
                format!("pan {},{} ", app.pan.0, app.pan.1),
                Style::new().fg(Color::DarkGray),
            ));
        }
    }
    spans.push(Span::styled(
        format!("errors {} ", app.parse_errors.len()),
        Style::new().fg(if app.parse_errors.is_empty() {
            Color::DarkGray
        } else {
            Color::Red
        }),
    ));
    if let Some(first) = app.parse_errors.first() {
        let (ln, _) = crate::dbml::parser::line_col_at(&app.source, first.offset);
        spans.push(Span::styled(
            format!("E{}: {} ", ln + 1, first.message),
            Style::new().fg(Color::Red),
        ));
    }

    if let Some(msg) = &app.message {
        spans.push(Span::styled(
            msg.clone(),
            Style::new().fg(Color::Yellow).bg(Color::Black),
        ));
    } else {
        spans.push(Span::styled(
            " Ctrl-c quit · Tab focus · :cmd · /search",
            Style::new().fg(Color::Blue),
        ));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(Color::Black)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_math() {
        // content that fits shows a full-track thumb
        assert_eq!(thumb(10, 20, 0), (0.0, 1.0));
        assert_eq!(thumb(20, 20, 5), (0.0, 1.0));
        // half the content visible, scrolled a quarter: thumb at 1/4..3/4
        let (s, l) = thumb(200, 100, 25);
        assert!((l - 0.5).abs() < 1e-6);
        assert!((s - 0.125).abs() < 1e-6);
        // at max offset the thumb sits flush at the end
        let (s, l) = thumb(200, 100, 100);
        assert!((s - (1.0 - l)).abs() < 1e-6);
        // tiny viewports clamp to a minimum thumb
        let (_, l) = thumb(1000, 10, 0);
        assert!((l - 0.05).abs() < 1e-6);
        // negative or overflowing offsets clamp
        assert_eq!(thumb(200, 100, -50).0, 0.0);
        assert_eq!(thumb(200, 100, 500).0, 1.0 - thumb(200, 100, 500).1);
    }

    #[test]
    fn offset_math() {
        assert_eq!(offset_at(200, 100, 0.0), 0);
        assert_eq!(offset_at(200, 100, 0.5), 50);
        assert_eq!(offset_at(200, 100, 1.0), 100);
        // content that fits never scrolls
        assert_eq!(offset_at(10, 20, 1.0), 0);
    }

    #[test]
    fn scrollbar_rects_stay_in_track() {
        let mut app = App::with_sample_source();
        app.panes.canvas = Rect::new(40, 0, 60, 38);
        let c = app.panes.canvas;
        let inner = Rect {
            x: c.x + 1,
            y: c.y + 1,
            width: c.width - 2,
            height: c.height - 2,
        };
        let view = app.view_rect();
        let bars = scrollbar_rects(&app, inner, view);
        let (vx, t0, t1) = bars.v.expect("vertical bar");
        assert_eq!(vx, view.right());
        assert!(t0 >= inner.y && t1 < view.bottom(), "{t0}..{t1}");
        let (hy, s0, s1) = bars.h.expect("horizontal bar");
        assert_eq!(hy, view.bottom());
        assert!(s0 >= inner.x && s1 < view.right(), "{s0}..{s1}");
    }
}
