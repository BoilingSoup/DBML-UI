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
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // a press on the popup picks that row; anywhere else places
                // the caret (and dismisses the popup)
                if let Some((rect, first_row, _)) = popup_layout(self.app, bounds) {
                    if rect.contains(local) {
                        let row = ((local.y - rect.y) / ROW_H) as usize;
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::CompletionPicked(first_row + row)),
                        );
                    }
                }
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorPressed(local)),
                )
            }
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

        if let Some((prect, first_row, visible)) = popup_layout(app, bounds) {
            let c = app.completion.as_ref().unwrap();
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    prect.position(),
                    prect.size(),
                    iced::border::Radius::from(6.0),
                ),
                theme::PANEL,
            );
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    prect.position(),
                    prect.size(),
                    iced::border::Radius::from(6.0),
                ),
                canvas::Stroke {
                    style: theme::BORDER.into(),
                    width: 1.0,
                    ..Default::default()
                },
            );
            for (i, item) in c.items[first_row..first_row + visible].iter().enumerate() {
                let sel = first_row + i == c.selected;
                let ry = prect.y + i as f32 * ROW_H;
                if sel {
                    frame.fill_rectangle(
                        Point::new(prect.x + 2.0, ry),
                        Size::new(prect.width - 4.0, ROW_H),
                        Color::from_rgba(0.267, 0.576, 0.973, 0.25),
                    );
                }
                frame.fill_text(Text {
                    content: item.label.clone(),
                    position: Point::new(prect.x + 10.0, ry + ROW_H * 0.5),
                    color: item_kind_color(item.kind),
                    size: (FONT_SIZE - 1.0).into(),
                    font: Font::MONOSPACE,
                    horizontal_alignment: iced::alignment::Horizontal::Left,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
            }
        }
        vec![frame.into_geometry()]
    }
}

/// Popup rows per page and row height.
pub const COMPLETION_ROWS: usize = 8;
pub const ROW_H: f32 = 20.0;

/// Geometry of the open completion popup: rect, first visible row, visible
/// count. Positioned under the caret, clamped/flipped to stay inside the
/// pane. None when the popup is closed.
fn popup_layout(app: &App, bounds: Rectangle) -> Option<(Rectangle, usize, usize)> {
    let c = app.completion.as_ref()?;
    let visible = c.items.len().min(COMPLETION_ROWS);
    let width = c
        .items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(8) as f32
        * char_width()
        + 20.0;
    let height = visible as f32 * ROW_H + 6.0;
    let caret = pos_to_px(app.editor.buf.pos(), app.editor_scroll);
    let mut x = caret.x + char_width() + 2.0;
    let mut y = caret.y + LINE_HEIGHT + 2.0;
    if y + height > bounds.y + bounds.height - 2.0 {
        y = caret.y - height - 2.0;
    }
    if x + width > bounds.x + bounds.width - 2.0 {
        x = bounds.x + bounds.width - 2.0 - width;
    }
    x = x.max(bounds.x + 2.0);
    y = y.max(bounds.y + 2.0);
    // keep the selection visible when scrolled
    let first_row = if c.selected + 1 > visible {
        c.selected + 1 - visible
    } else {
        0
    };
    Some((
        Rectangle::new(Point::new(x, y), Size::new(width, height)),
        first_row,
        visible,
    ))
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use crate::app::{update, App};

    const PANE: Rectangle = Rectangle::new(Point::new(0.0, 0.0), Size::new(400.0, 300.0));

    fn app_with_caret(row: usize, col: usize) -> App {
        let mut app = App::default();
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Char('i'),
                crate::editor::KeyModifiers::NONE,
            ),
        );
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(row, col));
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Space,
                crate::editor::KeyModifiers::CONTROL,
            ),
        );
        app
    }

    #[test]
    fn popup_flips_above_when_near_bottom() {
        // caret on the last visible line of a tall buffer
        let mut app = App::default();
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Char('i'),
                crate::editor::KeyModifiers::NONE,
            ),
        );
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Bottom);
        app.editor_scroll = app.editor.buf.line_count() as f32 - 20.0;
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Space,
                crate::editor::KeyModifiers::CONTROL,
            ),
        );
        assert!(app.completion.is_some(), "popup open");
        let (rect, _, _) = popup_layout(&app, PANE).unwrap();
        let caret = pos_to_px(app.editor.buf.pos(), app.editor_scroll);
        assert!(
            rect.y + rect.height <= caret.y,
            "popup should sit above the caret: rect={rect:?} caret_y={}",
            caret.y
        );
    }

    #[test]
    fn popup_stays_inside_right_edge() {
        // caret near the right edge: popup shifts left, stays inside
        let mut app = app_with_caret(0, 0);
        // long labels to force a wide popup
        if let Some(c) = app.completion.as_mut() {
            c.items.insert(
                0,
                crate::editor::completion::Item {
                    label: "x".repeat(80),
                    kind: crate::editor::completion::ItemKind::Table,
                },
            );
        }
        app.editor_scroll = 0.0;
        // a pane the popup can fit in but not at the caret's default spot
        let narrow = Rectangle::new(Point::new(0.0, 0.0), Size::new(720.0, 300.0));
        let (rect, _, _) = popup_layout(&app, narrow).unwrap();
        assert!(rect.width < narrow.width, "precondition: {rect:?}");
        assert!(
            rect.x + rect.width <= narrow.x + narrow.width + 0.5,
            "{rect:?}"
        );
    }

    #[test]
    fn no_popup_geometry_when_closed() {
        let app = App::default();
        assert!(popup_layout(&app, PANE).is_none());
    }
}

fn item_kind_color(kind: crate::editor::completion::ItemKind) -> Color {
    use crate::editor::completion::ItemKind;
    match kind {
        ItemKind::Keyword => theme::SYN_KEYWORD,
        ItemKind::Table | ItemKind::Enum => theme::SYN_NAME,
        ItemKind::Column | ItemKind::EnumValue => theme::SYN_NUMBER,
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
