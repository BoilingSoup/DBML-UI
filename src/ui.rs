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
    f.render_widget(block, area);

    match app.render_mode {
        crate::render::Mode::Text => {
            if let Some(layout) = &app.diagram {
                crate::render::text::render(f.buffer_mut(), inner, layout, app.pan);
            }
        }
        crate::render::Mode::Kitty | crate::render::Mode::Sixel => {
            // Detect viewport size changes (resize / divider drag) so the
            // event loop re-crops.
            let (fw, fh) = app.font_px();
            let px = (
                (inner.width as u32).saturating_mul(fw).max(1),
                (inner.height as u32).saturating_mul(fh).max(1),
            );
            if px != app.canvas_px {
                app.image_due = true;
            }
            if let Some(proto) = app.proto.as_mut() {
                f.render_widget(ratatui_image::Image::new(&*proto), inner);
            } else {
                let para = Paragraph::new("rasterizing…").style(Style::new().fg(Color::DarkGray));
                f.render_widget(para, inner);
            }
        }
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
            },
        };
        spans.push(Span::styled(label, style));
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
    match app.render_mode {
        crate::render::Mode::Kitty | crate::render::Mode::Sixel => {
            spans.push(Span::styled(
                format!(
                    "{} zoom {:.0}% ",
                    match app.render_mode {
                        crate::render::Mode::Kitty => "kitty",
                        _ => "sixel",
                    },
                    app.zoom * 100.0
                ),
                Style::new().fg(Color::DarkGray),
            ));
        }
        crate::render::Mode::Text => {
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
