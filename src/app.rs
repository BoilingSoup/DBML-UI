//! Application state and the iced update loop.

use crate::canvas::diagram::{hit_test, to_cell_f};
use crate::editor::{self, Action, Editor, KeyCode, KeyEvent, KeyModifiers};
use dbml_ui::layout::{self, Layout};
use dbml_ui::{dbml, model};
use iced::{Point, Vector};

/// Editor pane height in lines; updated by the view each frame when the
/// pane is laid out (used to clamp scrolling).
pub const EDITOR_VISIBLE_LINES: f32 = 40.0;

#[derive(Debug, Clone)]
pub enum Message {
    CanvasPressed(Point),
    CanvasMoved(Point),
    CanvasReleased(Point),
    /// Wheel notches (normalized: lines, or pixels / 40).
    Wheel(f32),
    /// A keypress, mapped from iced's keyboard events.
    KeyPressed(KeyCode, KeyModifiers),
    /// Wheel notches over the editor pane.
    EditorScrolled(f32),
    /// Left-press in the editor pane (local px).
    EditorPressed(Point),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Canvas,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    None,
    /// Dragging empty space: pan the view, anchored at the press point.
    Pan {
        anchor: Point,
    },
    /// Dragging a table: live re-layout at the grabbed offset.
    Table {
        title: String,
        grab: (f32, f32),
        press: Point,
        moved: bool,
    },
}

pub struct App {
    /// Raw DBML source shown in the editor.
    pub source: String,
    /// Vim editor over the source.
    pub editor: Editor,
    /// Which pane receives keys.
    pub focus: Focus,
    /// Editor viewport scroll, in lines.
    pub editor_scroll: f32,
    /// Parsed + laid out diagram for the current source.
    pub diagram: Layout,
    pub parse_errors: Vec<dbml::parser::ParseError>,
    /// User-dragged table positions (diagram cells, by table/enum name);
    /// survive re-parses while editing.
    pub table_pos: std::collections::HashMap<String, (i32, i32)>,
    /// Diagram zoom: pixels per diagram cell height.
    pub zoom: f32,
    /// Viewport offset into the zoomed diagram, in pixels.
    pub pan: Vector,
    /// Mouse interaction in flight.
    pub drag: Drag,
    /// Last known cursor position over the diagram canvas (local px).
    pub canvas_cursor: Option<Point>,
    /// Table that was clicked (no drag) — the editor jumps to its
    /// definition.
    pub pending_jump: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self::with_source(dbml_ui::SAMPLE_DBML.to_string())
    }
}

impl App {
    pub fn with_source(source: String) -> Self {
        let mut app = Self {
            source: String::new(),
            editor: Editor::default(),
            focus: Focus::Editor,
            editor_scroll: 0.0,
            diagram: layout::layout(&model::build(&dbml::parser::parse("").doc)),
            parse_errors: Vec::new(),
            table_pos: Default::default(),
            zoom: crate::view::BASE_ZOOM,
            pan: Vector::default(),
            drag: Drag::None,
            canvas_cursor: None,
            pending_jump: None,
        };
        app.editor.set_text(&source);
        app.set_source(&source);
        app
    }

    /// Re-parse + re-layout (keeps dragged table positions).
    pub fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        let out = dbml::parser::parse(src);
        let schema = model::build(&out.doc);
        self.diagram = layout::layout_with_overrides(&schema, &self.table_pos);
        self.parse_errors = out.errors;
    }

    fn canvas_pressed(&mut self, p: Point) {
        self.canvas_cursor = Some(p);
        match hit_test(&self.diagram, self.zoom, self.pan, p) {
            Some(b) => {
                let (cx, cy) = to_cell_f(p, self.zoom, self.pan);
                self.drag = Drag::Table {
                    title: b.title.clone(),
                    grab: (cx - b.x as f32, cy - b.y as f32),
                    press: p,
                    moved: false,
                };
            }
            None => self.drag = Drag::Pan { anchor: p },
        }
    }

    fn canvas_moved(&mut self, p: Point) {
        self.canvas_cursor = Some(p);
        match self.drag.clone() {
            Drag::Pan { anchor } => {
                self.pan = self.pan + Vector::new(p.x - anchor.x, p.y - anchor.y);
                self.drag = Drag::Pan { anchor: p };
            }
            Drag::Table {
                title,
                grab,
                press,
                moved,
            } => {
                let moved = moved || p.distance(press) > 3.0;
                let (cx, cy) = to_cell_f(p, self.zoom, self.pan);
                let pos = (
                    ((cx - grab.0).round() as i32).max(0),
                    ((cy - grab.1).round() as i32).max(0),
                );
                let src = self.source.clone();
                self.table_pos.insert(title.clone(), pos);
                self.set_source(&src);
                self.drag = Drag::Table {
                    title,
                    grab,
                    press,
                    moved,
                };
            }
            Drag::None => {}
        }
    }

    fn canvas_released(&mut self, p: Point) {
        self.canvas_cursor = Some(p);
        if let Drag::Table {
            title,
            press,
            moved,
            ..
        } = &self.drag
        {
            if !*moved && p.distance(*press) <= 3.0 {
                self.pending_jump = Some(title.clone());
            }
        }
        self.drag = Drag::None;
    }

    /// Zoom continuously, keeping the point under the cursor stationary.
    fn wheel(&mut self, notches: f32) {
        let old = self.zoom;
        let new = (old * (notches * 0.22f32).exp()).clamp(5.0, 160.0);
        if (new - old).abs() < f32::EPSILON {
            return;
        }
        if let Some(p) = self.canvas_cursor {
            let s = new / old;
            self.pan = Vector::new(p.x - (p.x - self.pan.x) * s, p.y - (p.y - self.pan.y) * s);
        }
        self.zoom = new;
    }
}

pub fn subscription(_app: &App) -> iced::Subscription<Message> {
    iced::keyboard::on_key_press(|key, mods| {
        let code = match key {
            iced::keyboard::Key::Character(c) => c.chars().next().map(KeyCode::Char),
            iced::keyboard::Key::Named(n) => Some(match n {
                iced::keyboard::key::Named::Escape => KeyCode::Esc,
                iced::keyboard::key::Named::Enter => KeyCode::Enter,
                iced::keyboard::key::Named::Backspace => KeyCode::Backspace,
                iced::keyboard::key::Named::Tab => KeyCode::Tab,
                iced::keyboard::key::Named::ArrowLeft => KeyCode::Left,
                iced::keyboard::key::Named::ArrowRight => KeyCode::Right,
                iced::keyboard::key::Named::ArrowUp => KeyCode::Up,
                iced::keyboard::key::Named::ArrowDown => KeyCode::Down,
                iced::keyboard::key::Named::Home => KeyCode::Home,
                iced::keyboard::key::Named::End => KeyCode::End,
                iced::keyboard::key::Named::PageUp => KeyCode::PageUp,
                iced::keyboard::key::Named::PageDown => KeyCode::PageDown,
                iced::keyboard::key::Named::Delete => KeyCode::Delete,
                _ => return None,
            }),
            _ => None,
        }?;
        let m = KeyModifiers::new(mods.control(), mods.shift(), mods.alt());
        Some(Message::KeyPressed(code, m))
    })
}

pub fn update(app: &mut App, message: Message) {
    match message {
        Message::CanvasPressed(p) => app.canvas_pressed(p),
        Message::CanvasMoved(p) => app.canvas_moved(p),
        Message::CanvasReleased(p) => {
            app.canvas_released(p);
            app.consume_pending_jump();
        }
        Message::Wheel(n) => app.wheel(n),
        Message::KeyPressed(code, mods) => app.key_pressed(code, mods),
        Message::EditorScrolled(n) => {
            app.editor_scroll =
                (app.editor_scroll + n).clamp(0.0, app.editor.buf.line_count() as f32 - 4.0);
        }
        Message::EditorPressed(p) => {
            let pos = crate::canvas::editor::px_to_pos(app, p);
            app.editor
                .buf
                .move_cursor(editor::buffer::Move::Jump(pos.row, pos.col));
            if app.editor.mode == editor::Mode::Visual {
                // clicking re-anchors the selection
                app.editor.buf.cancel_selection();
                app.editor.buf.start_selection();
            }
            app.focus = Focus::Editor;
        }
    }
}

impl App {
    fn key_pressed(&mut self, code: KeyCode, mods: KeyModifiers) {
        let key = KeyEvent::new(code, mods);
        // Tab always toggles pane focus
        if key.code == KeyCode::Tab {
            self.focus = match self.focus {
                Focus::Editor => Focus::Canvas,
                Focus::Canvas => Focus::Editor,
            };
            return;
        }
        // the cmdline is reachable from either pane
        if self.editor.cmdline.is_some() || self.focus == Focus::Editor {
            let action = self.editor.handle_key(key);
            self.after_edit(action);
            return;
        }
        // canvas focus: hjkl/arrows pan, ':'/'/' open the cmdline
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.pan.x += 60.0,
            KeyCode::Char('l') | KeyCode::Right => self.pan.x -= 60.0,
            KeyCode::Char('k') | KeyCode::Up => self.pan.y += 60.0,
            KeyCode::Char('j') | KeyCode::Down => self.pan.y -= 60.0,
            KeyCode::Char('0') => self.pan = Vector::new(40.0, 40.0),
            KeyCode::Char(':') | KeyCode::Char('/') => {
                self.focus = Focus::Editor;
                let action = self.editor.handle_key(key);
                self.after_edit(action);
            }
            _ => {}
        }
    }

    fn after_edit(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Changed => {
                let text = self.editor.text();
                self.set_source(&text);
                self.keep_cursor_visible();
            }
            // ex commands land with the file milestone
            Action::Command(_) => {}
        }
    }

    /// Scroll the editor viewport so the cursor stays on screen.
    fn keep_cursor_visible(&mut self) {
        let row = self.editor.buf.pos().row as f32;
        let visible = EDITOR_VISIBLE_LINES - 3.0;
        if row < self.editor_scroll + 1.0 {
            self.editor_scroll = (row - 1.0).max(0.0);
        } else if row > self.editor_scroll + visible {
            self.editor_scroll = row - visible;
        }
    }

    /// A click on a diagram table jumps the editor to its definition.
    fn consume_pending_jump(&mut self) {
        if let Some(title) = self.pending_jump.take() {
            if let Some(b) = self.diagram.boxes.iter().find(|b| b.title == title) {
                if let Some(span) = b.name_span.clone() {
                    let (row, col) = dbml::parser::line_col_at(&self.source, span.start);
                    self.editor.jump(row, col);
                    self.focus = Focus::Editor;
                    self.editor_scroll = (row as f32 - 2.0).max(0.0);
                    self.keep_cursor_visible();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::with_source(dbml_ui::SAMPLE_DBML.to_string())
    }

    /// Screen point of a table box corner plus an offset.
    fn over_box(app: &App, title: &str, dx: f32, dy: f32) -> Point {
        let b = app.diagram.boxes.iter().find(|b| b.title == title).unwrap();
        crate::canvas::diagram::to_px(b.x, b.y, app.zoom, app.pan) + Vector::new(dx, dy)
    }

    #[test]
    fn drag_pan_moves_viewport() {
        let mut app = app();
        // empty space, far from any box
        let p = Point::new(2000.0, 2000.0);
        update(&mut app, Message::CanvasPressed(p));
        update(&mut app, Message::CanvasMoved(Point::new(1990.0, 2004.0)));
        assert_eq!(app.pan, Vector::new(-10.0, 4.0));
        update(
            &mut app,
            Message::CanvasReleased(Point::new(1990.0, 2004.0)),
        );
        assert_eq!(app.drag, Drag::None);
    }

    #[test]
    fn wheel_zoom_is_cursor_anchored() {
        let mut app = app();
        let anchor = over_box(&app, "users", 7.0, 9.0);
        update(&mut app, Message::CanvasMoved(anchor));
        let before = crate::canvas::diagram::to_cell_f(anchor, app.zoom, app.pan);
        update(&mut app, Message::Wheel(1.0));
        let after = crate::canvas::diagram::to_cell_f(anchor, app.zoom, app.pan);
        assert!(app.zoom > 20.0, "zoomed in: {}", app.zoom);
        assert!(
            (before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01,
            "anchor drifted: {before:?} -> {after:?}"
        );

        update(&mut app, Message::Wheel(-1.0));
        assert!(
            (app.zoom - 20.0).abs() < 1e-3,
            "round trip zoom, got {}",
            app.zoom
        );
    }

    #[test]
    fn table_drag_relayouts_live() {
        let mut app = app();
        let press = over_box(&app, "posts", 3.0, 3.0);
        let before = app
            .diagram
            .boxes
            .iter()
            .find(|b| b.title == "posts")
            .unwrap()
            .clone();
        update(&mut app, Message::CanvasPressed(press));
        assert!(matches!(app.drag, Drag::Table { .. }));
        // move 8 cells right, 2 down
        let s = crate::canvas::diagram::cell_size(app.zoom);
        update(
            &mut app,
            Message::CanvasMoved(Point::new(
                press.x + 8.0 * s.width,
                press.y + 2.0 * s.height,
            )),
        );
        let after = app
            .diagram
            .boxes
            .iter()
            .find(|b| b.title == "posts")
            .unwrap();
        assert_eq!((after.x, after.y), (before.x + 8, before.y + 2));
        assert_eq!(
            app.table_pos.get("posts"),
            Some(&(before.x + 8, before.y + 2))
        );

        update(
            &mut app,
            Message::CanvasReleased(Point::new(press.x + 80.0, press.y + 20.0)),
        );
        assert_eq!(app.pending_jump, None, "a real drag is not a click");
    }

    #[test]
    fn click_without_drag_jumps_editor_to_definition() {
        let mut app = app();
        let before = app.editor.buf.cursor();
        let p = over_box(&app, "posts", 4.0, 4.0);
        update(&mut app, Message::CanvasPressed(p));
        update(&mut app, Message::CanvasReleased(p));
        assert_eq!(app.focus, Focus::Editor);
        let after = app.editor.buf.cursor();
        assert_ne!(before, after, "cursor moved to the definition");
        let line = app.editor.buf.line(after.0);
        assert!(line.contains("posts"), "on the posts line: {line:?}");
    }
}
