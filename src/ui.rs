use ratatui::layout::{Constraint, Layout, Rect};
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
}

fn draw_editor(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(" editor ")
        .border_style(focus_style(app.focus == Focus::Editor));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let placeholder = "// DBML editor arrives in M4\nTable users {\n  id integer [pk]\n}";
    let para = Paragraph::new(placeholder).style(Style::new().fg(Color::Gray));
    f.render_widget(para, inner);
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

fn draw_canvas(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .title(" canvas ")
        .border_style(focus_style(app.focus == Focus::Canvas));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Pannable dot grid: proves mouse pan + offscreen clipping before the
    // real diagram renderer lands (M3).
    let style = Style::new().fg(Color::DarkGray);
    let buf = f.buffer_mut();
    let step = 4u16;
    let gx: i32 = -(app.pan.0.rem_euclid(step as i32));
    let gy: i32 = -(app.pan.1.rem_euclid(step as i32));
    let mut x = gx;
    while x < inner.width as i32 {
        let mut y = gy;
        while y < inner.height as i32 {
            let (cx, cy) = (x + inner.x as i32, y + inner.y as i32);
            if cx >= inner.x as i32
                && cx < inner.right() as i32
                && cy >= inner.y as i32
                && cy < inner.bottom() as i32
            {
                buf[(cx as u16, cy as u16)].set_char('·').set_style(style);
            }
            y += step as i32;
        }
        x += step as i32;
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let mode = match app.focus {
        Focus::Editor => "EDITOR",
        Focus::Canvas => "CANVAS",
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" -- {mode} -- "),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(" tui-db "),
        Span::styled("render: text ", Style::new().fg(Color::DarkGray)),
        Span::styled(
            format!("pan {},{} ", app.pan.0, app.pan.1),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("split {:.0}% ", app.split * 100.0),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("mouse {},{} ", app.last_mouse.0, app.last_mouse.1),
            Style::new().fg(Color::DarkGray),
        ),
        Span::styled(
            "q quit · Tab focus · drag pan",
            Style::new().fg(Color::Blue),
        ),
    ]);
    f.render_widget(
        Paragraph::new(line).style(Style::new().bg(Color::Black)),
        area,
    );
}
