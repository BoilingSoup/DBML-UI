//! The editor canvas: monospace code text with a gutter, a mode-styled
//! fake cursor, and visual-mode selection spans. Scrolling is a float row
//! offset kept in `App`; the canvas just draws the visible window.

use crate::app::{App, Message};
use crate::editor::buffer::Pos;
use crate::theme;
use iced::widget::canvas::{self, Text};
use iced::{Color, Font, Point, Rectangle, Size};

/// iced's default monospace (Fira Mono) advances exactly 0.6 em.
pub const CHAR_RATIO: f32 = 0.6;
pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 21.0;
pub const GUTTER: f32 = 56.0;

pub fn char_width() -> f32 {
    FONT_SIZE * CHAR_RATIO
}

/// Screen-space origin of a buffer position (local to the editor pane).
pub fn pos_to_px(p: Pos, scroll: f32) -> Point {
    Point::new(
        GUTTER + p.col as f32 * char_width(),
        (p.row as f32 - scroll) * LINE_HEIGHT,
    )
}

/// Buffer position under a local point (clamped into the text).
pub fn px_to_pos(app: &App, p: Point) -> Pos {
    let lines = app.editor.buf.lines();
    let row = ((p.y / LINE_HEIGHT).floor() as usize)
        .saturating_add(app.editor_scroll as usize)
        .min(lines.len().saturating_sub(1));
    let max_col = lines.get(row).map(|l| l.chars().count()).unwrap_or(0);
    let col = if p.x <= GUTTER {
        0
    } else {
        (((p.x - GUTTER) / char_width()).round() as usize).min(max_col)
    };
    Pos { row, col }
}

pub struct EditorView<'a> {
    pub app: &'a App,
}

impl canvas::Program<Message> for EditorView<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use iced::mouse;
        // The canvas widget forwards every window event with window-level
        // coordinates; only react when the pointer is over this canvas.
        let Some(p) = cursor.position_over(bounds) else {
            return (canvas::event::Status::Ignored, None);
        };
        let local = Point::new(p.x - bounds.x, p.y - bounds.y);
        match event {
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let notches = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorScrolled(notches)),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => (
                canvas::event::Status::Captured,
                Some(Message::EditorPressed(local)),
            ),
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::PANEL);

        let app = self.app;
        let lines = app.editor.buf.lines();
        let scroll = app.editor_scroll;
        let visible = (bounds.height / LINE_HEIGHT).ceil() as usize + 1;
        let first = scroll.floor() as usize;
        let sub = scroll - first as f32;

        // selection spans (visual mode)
        if let Some((start, end)) = app.editor.buf.selection_range() {
            let top = pos_to_px(start, scroll);
            let bottom = pos_to_px(end, scroll);
            let full_rows = end.row.saturating_sub(start.row + 1);
            let w = char_width();
            if top.y == bottom.y {
                frame.fill_rectangle(
                    top,
                    Size::new((bottom.x - top.x).max(w), LINE_HEIGHT * 0.9),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
            } else {
                frame.fill_rectangle(
                    top,
                    Size::new(bounds.width - top.x, LINE_HEIGHT * 0.9),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
                if full_rows > 0 {
                    frame.fill_rectangle(
                        Point::new(GUTTER, top.y + LINE_HEIGHT),
                        Size::new(bounds.width - GUTTER, full_rows as f32 * LINE_HEIGHT),
                        Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                    );
                }
                frame.fill_rectangle(
                    Point::new(GUTTER, bottom.y),
                    Size::new((bottom.x - GUTTER).max(w), LINE_HEIGHT * 0.9),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
            }
        }

        // gutter + text, token-colored
        let src = app.source.as_str();
        // byte offset of each line start in `source` (highlight spans index it)
        let mut line_start = 0usize;
        let mut line_starts = Vec::with_capacity(lines.len());
        for l in lines {
            line_starts.push(line_start);
            line_start += l.len() + 1;
        }
        let mut y = -sub * LINE_HEIGHT;
        for (i, line) in lines.iter().enumerate().skip(first).take(visible.max(1)) {
            let text_y = y + LINE_HEIGHT * 0.5;
            frame.fill_text(Text {
                content: format!("{:>4}", i + 1),
                position: Point::new(GUTTER - 8.0, text_y),
                color: theme::DIM,
                size: (FONT_SIZE - 2.0).into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
            if !line.is_empty() {
                let ls = line_starts[i];
                let le = ls + line.len();
                // token chunks intersecting this line, drawn at their char
                // column on the same monospace grid the cursor math uses
                let first_tok = app.highlights.partition_point(|t| t.end <= ls);
                for t in &app.highlights[first_tok..] {
                    if t.start >= le {
                        break;
                    }
                    let s = t.start.max(ls);
                    let e = t.end.min(le);
                    let content = &src[s..e];
                    if content.chars().any(|c| !c.is_whitespace()) {
                        let col = src[ls..s].chars().count();
                        frame.fill_text(Text {
                            content: content.to_string(),
                            position: Point::new(GUTTER + col as f32 * char_width(), text_y),
                            color: token_color(t.kind),
                            size: FONT_SIZE.into(),
                            font: Font::MONOSPACE,
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Center,
                            ..Default::default()
                        });
                    }
                }
            }
            y += LINE_HEIGHT;
        }

        // fake cursor, styled by mode
        let cursor = app.editor.buf.pos();
        let p = pos_to_px(cursor, scroll);
        if p.y > -LINE_HEIGHT && p.y < bounds.height + LINE_HEIGHT {
            let (color, filled) = match app.editor.mode {
                crate::editor::Mode::Insert => (theme::TEXT, false),
                _ => (theme::TEXT, true),
            };
            let rect = if filled {
                // block cursor
                frame.fill_rectangle(
                    p,
                    Size::new(char_width(), LINE_HEIGHT * 0.9),
                    Color::from_rgba(color.r, color.g, color.b, 0.45),
                );
                Size::new(2.0, LINE_HEIGHT * 0.9)
            } else {
                Size::new(2.0, LINE_HEIGHT * 0.9)
            };
            frame.fill_rectangle(p, rect, color);
        }
        vec![frame.into_geometry()]
    }
}

fn token_color(kind: dbml_ui::dbml::highlight::Kind) -> Color {
    use dbml_ui::dbml::highlight::Kind;
    match kind {
        Kind::Keyword => theme::SYN_KEYWORD,
        Kind::Name => theme::SYN_NAME,
        Kind::Type => theme::SYN_TYPE,
        Kind::String => theme::SYN_STRING,
        Kind::Number => theme::SYN_NUMBER,
        Kind::Comment | Kind::Punct => theme::DIM,
        Kind::Ident | Kind::Plain => theme::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use iced::mouse::{self, Cursor};
    use iced::widget::canvas::Program as _;

    const BOUNDS: Rectangle = Rectangle::new(Point::new(100.0, 50.0), Size::new(400.0, 300.0));

    #[test]
    fn press_outside_editor_does_not_move_cursor() {
        // regression: presses in the diagram pane used to leak into the
        // editor canvas and jump the caret
        let app = App::default();
        let view = EditorView { app: &app };
        let outside = Point::new(900.0, 100.0); // diagram-pane territory
        let (status, message) = view.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            BOUNDS,
            Cursor::Available(outside),
        );
        assert!(message.is_none(), "{message:?}");
        assert_eq!(status, canvas::event::Status::Ignored);
    }

    #[test]
    fn press_inside_places_cursor_at_local_position() {
        let app = App::default();
        let view = EditorView { app: &app };
        let (_, message) = view.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            BOUNDS,
            Cursor::Available(Point::new(100.0 + GUTTER + 30.0, 50.0 + 40.0)),
        );
        match message {
            Some(Message::EditorPressed(p)) => {
                assert!((p.x - (GUTTER + 30.0)).abs() < 0.01);
                assert!((p.y - 40.0).abs() < 0.01);
            }
            other => panic!("expected EditorPressed, got {other:?}"),
        }
    }

    #[test]
    fn wheel_outside_editor_does_not_scroll() {
        // regression: zooming the diagram scrolled the editor pane
        let app = App::default();
        let view = EditorView { app: &app };
        let (_, message) = view.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
            }),
            BOUNDS,
            Cursor::Available(Point::new(900.0, 100.0)),
        );
        assert!(message.is_none(), "{message:?}");
    }
}
