use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

use crate::dbml::parser::{self, ParseError};
use crate::layout::{self, Layout};
use crate::model;
use crate::ui;

/// Startup document for a fresh session.
pub const SAMPLE_DBML: &str = r#"Project blog {
  database_type: 'PostgreSQL'
  Note: 'A small blog schema'
}

Table users as U [note: 'Registered accounts'] {
  id integer [pk, increment]
  username varchar(80) [not null, unique]
  email varchar(255) [not null]
  role varchar [default: 'member']
  created_at timestamp [default: `now()`]

  indexes {
    (username, email) [unique, name: 'idx_users_contact']
  }
}

Table posts {
  id integer [pk]
  user_id integer [not null, ref: > U.id]
  status post_status [not null, default: 'draft']
  title varchar(200) [not null]
  body text
  published_at timestamp
}

Table comments {
  id integer [pk]
  post_id integer [not null, ref: > posts.id]
  author_id integer [not null, ref: > U.id]
  body text [not null]
}

Enum post_status {
  draft
  published
  archived
}

Ref: comments.author_id > users.id
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    Editor,
    Canvas,
}

/// Screen rectangles of the main regions, refreshed each frame. Used for
/// hit-testing mouse events between draws.
#[derive(Debug, Default, Clone, Copy)]
pub struct Panes {
    pub body: Rect,
    pub editor: Rect,
    pub divider: Rect,
    pub canvas: Rect,
}

#[derive(Debug, Default)]
pub struct App {
    pub should_quit: bool,
    pub focus: Focus,
    pub split: f32,
    pub divider_dragging: bool,
    /// Canvas pan offset in cells; positive shifts diagram content right/down.
    pub pan: (i32, i32),
    pub pan_anchor: Option<(u16, u16)>,
    pub last_mouse: (u16, u16),
    pub panes: Panes,
    pub source: String,
    pub diagram: Option<Layout>,
    pub parse_errors: Vec<ParseError>,
    pub dangling: usize,
    pub editor: crate::editor::Editor,
    /// Text changed since the last re-parse; debounce timer anchor.
    pub dirty: bool,
    pub last_edit: Option<Instant>,
    /// Transient status message (command feedback, errors).
    pub message: Option<String>,
    /// File backing the buffer, if any.
    pub file: Option<std::path::PathBuf>,
    /// Buffer content as of the last load/save, for the modified flag.
    pub saved_text: String,
    /// Renderer override from the CLI (`--render`).
    pub render_pref: Option<crate::render::Mode>,
}

const DEBOUNCE: Duration = Duration::from_millis(120);

impl App {
    pub fn is_modified(&self) -> bool {
        self.editor.text() != self.saved_text
    }

    /// Re-parse the document and rebuild the diagram preview.
    pub fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        let out = parser::parse(src);
        let schema = model::build(&out.doc);
        self.diagram = Some(layout::layout(&schema));
        self.parse_errors = out.errors;
        self.dangling = schema.dangling;
    }

    /// Open a file into the editor.
    pub fn open_file(&mut self, path: &str) -> std::io::Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.file = Some(std::path::PathBuf::from(path));
        self.saved_text = content.clone();
        self.editor.set_text(&content);
        self.set_source(&content);
        self.message = Some(format!(
            "\"{path}\" {}L, {}B",
            content.lines().count(),
            content.len()
        ));
        Ok(())
    }

    /// Write the buffer to disk.
    pub fn save_file(&mut self, path: Option<&str>) -> std::io::Result<()> {
        let target = match path {
            Some(p) => {
                self.file = Some(std::path::PathBuf::from(p));
                std::path::PathBuf::from(p)
            }
            None => match self.file.clone() {
                Some(f) => f,
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "no file name (use :w <file>)",
                    ))
                }
            },
        };
        let text = self.editor.text();
        std::fs::write(&target, &text)?;
        self.saved_text = text;
        self.message = Some(format!(
            "\"{}\" written, {}B",
            target.display(),
            self.saved_text.len()
        ));
        Ok(())
    }

    /// Execute an ex command (already stripped of ':').
    pub fn handle_command(&mut self, input: &str) {
        use crate::commands::ExCommand;
        match crate::commands::parse_ex(input) {
            ExCommand::Write(path) => {
                if let Err(e) = self.save_file(path.as_deref()) {
                    self.message = Some(format!(":w failed: {e}"));
                }
            }
            ExCommand::Quit { force } => {
                if self.is_modified() && !force {
                    self.message = Some("No write since last change (use :w or :q!)".to_string());
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::WriteQuit => {
                if self.file.is_none() {
                    self.message = Some("no file name (use :w <file>)".to_string());
                } else if let Err(e) = self.save_file(None) {
                    self.message = Some(format!(":wq failed: {e}"));
                } else {
                    self.should_quit = true;
                }
            }
            ExCommand::Open { path, force } => {
                if self.is_modified() && !force {
                    self.message =
                        Some("No write since last change (use :e! or :w first)".to_string());
                } else if let Err(e) = self.open_file(&path) {
                    self.message = Some(format!("cannot open \"{path}\": {e}"));
                }
            }
            ExCommand::Export(arg) => {
                self.message = Some(format!(":export {arg} arrives in a later milestone"));
            }
            ExCommand::Unknown(why) => self.message = Some(why),
        }
    }

    pub fn with_sample_source() -> Self {
        let mut app = Self {
            editor: crate::editor::Editor::new(SAMPLE_DBML),
            ..Self::default()
        };
        app.set_source(SAMPLE_DBML);
        app
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let result = self.event_loop(&mut terminal);

        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        result
    }

    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> anyhow::Result<()> {
        while !self.should_quit {
            terminal.draw(|f| ui::draw(f, self))?;
            let timeout = match (self.dirty, self.last_edit) {
                (true, Some(t)) => DEBOUNCE.saturating_sub(t.elapsed()),
                _ => Duration::from_millis(50),
            };
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                    Event::Mouse(mouse) => self.on_mouse(mouse),
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
            if self.dirty && self.last_edit.is_none_or(|t| t.elapsed() >= DEBOUNCE) {
                self.set_source(&self.editor.text());
                self.dirty = false;
                self.last_edit = None;
            }
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('s') => {
                    if let Err(e) = self.save_file(None) {
                        self.message = Some(format!("save failed: {e}"));
                    }
                    return;
                }
                _ => {}
            }
        }
        // The command-line prompt always receives keys, regardless of focus.
        if self.editor.cmdline.is_some() {
            self.dispatch_editor_key(key);
            return;
        }
        if key.code == KeyCode::Tab {
            self.focus = match self.focus {
                Focus::Editor => Focus::Canvas,
                Focus::Canvas => Focus::Editor,
            };
            return;
        }
        match self.focus {
            Focus::Editor => self.dispatch_editor_key(key),
            Focus::Canvas => self.on_canvas_key(key),
        }
    }

    fn dispatch_editor_key(&mut self, key: KeyEvent) {
        match self.editor.handle_key(key) {
            crate::editor::Action::Changed => {
                self.dirty = true;
                self.last_edit = Some(Instant::now());
            }
            crate::editor::Action::Command(cmd) => self.handle_command(&cmd),
            crate::editor::Action::None => {}
        }
    }

    fn on_canvas_key(&mut self, key: KeyEvent) {
        let (dx, dy) = match key.code {
            KeyCode::Char('h') | KeyCode::Left => (-4, 0),
            KeyCode::Char('l') | KeyCode::Right => (4, 0),
            KeyCode::Char('j') | KeyCode::Down => (0, 1),
            KeyCode::Char('k') | KeyCode::Up => (0, -1),
            KeyCode::Char('H') => (-16, 0),
            KeyCode::Char('L') => (16, 0),
            KeyCode::Char('0') => {
                self.pan = (0, 0);
                return;
            }
            _ => return,
        };
        self.pan.0 += dx;
        self.pan.1 += dy;
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let (col, row) = (mouse.column, mouse.row);
        self.last_mouse = (col, row);
        let panes = self.panes;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if col >= panes.divider.left().saturating_sub(0)
                    && col <= panes.divider.right().saturating_add(0)
                    && row >= panes.body.top()
                {
                    self.divider_dragging = true;
                } else if rect_contains(panes.canvas, col, row) {
                    self.focus = Focus::Canvas;
                    self.pan_anchor = Some((col, row));
                } else if rect_contains(panes.editor, col, row) {
                    self.focus = Focus::Editor;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.divider_dragging {
                    if panes.body.width > 0 {
                        let rel = (col.saturating_sub(panes.body.left())) as f32
                            / panes.body.width as f32;
                        self.split = rel.clamp(0.15, 0.85);
                    }
                } else if let Some((last_col, last_row)) = self.pan_anchor {
                    self.pan.0 += col as i32 - last_col as i32;
                    self.pan.1 += row as i32 - last_row as i32;
                    self.pan_anchor = Some((col, row));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.divider_dragging = false;
                self.pan_anchor = None;
            }
            MouseEventKind::ScrollUp => {
                if rect_contains(panes.canvas, col, row) {
                    self.pan.1 += 2;
                } else if rect_contains(panes.editor, col, row) {
                    self.editor.scroll(-2);
                }
            }
            MouseEventKind::ScrollDown => {
                if rect_contains(panes.canvas, col, row) {
                    self.pan.1 -= 2;
                } else if rect_contains(panes.editor, col, row) {
                    self.editor.scroll(2);
                }
            }
            _ => {}
        }
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.left() && col < rect.right() && row >= rect.top() && row < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Action;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    #[test]
    fn typed_table_flows_to_diagram() {
        let mut app = App::with_sample_source();
        assert_eq!(app.diagram.as_ref().unwrap().boxes.len(), 4);

        // G o  Table tmp { id integer [pk] }  Esc
        assert_eq!(app.editor.handle_key(key('G')), Action::None);
        assert_eq!(app.editor.handle_key(key('o')), Action::Changed);
        for c in "Table tmp { id integer [pk] }".chars() {
            assert_eq!(app.editor.handle_key(key(c)), Action::Changed);
        }
        assert_eq!(app.editor.handle_key(esc()), Action::None);

        // what the debounce handler runs:
        app.set_source(&app.editor.text());
        assert!(app.parse_errors.is_empty(), "{:?}", app.parse_errors);
        assert_eq!(app.diagram.as_ref().unwrap().boxes.len(), 5);
        assert!(app
            .diagram
            .as_ref()
            .unwrap()
            .boxes
            .iter()
            .any(|b| b.title == "tmp"));
    }

    #[test]
    fn editor_sample_matches_app_source() {
        let app = App::with_sample_source();
        assert_eq!(app.editor.text(), app.source);
    }

    #[test]
    fn file_round_trip_is_byte_identical() {
        let dir = std::env::temp_dir().join(format!("tui-db-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src_path = dir.join("a.dbml");
        let dst_path = dir.join("b.dbml");
        std::fs::write(&src_path, "Table t {\n  id integer [pk]\n}\n").unwrap();

        let mut app = App::default();
        app.open_file(src_path.to_str().unwrap()).unwrap();
        assert!(!app.is_modified());

        // modify, then save-as and confirm the modified content landed
        app.editor.handle_key(key('G'));
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
        for c in "Table extra { id integer }".chars() {
            app.editor.handle_key(key(c));
        }
        app.editor.handle_key(esc());
        assert!(app.is_modified());
        app.save_file(Some(dst_path.to_str().unwrap())).unwrap();
        assert!(!app.is_modified());
        assert_eq!(
            std::fs::read_to_string(&dst_path).unwrap(),
            "Table t {\n  id integer [pk]\n}\nTable extra { id integer }\n"
        );

        // :q refuses while modified, :q! quits
        app.editor.handle_key(key('A'));
        app.editor.handle_key(key('z'));
        app.editor.handle_key(esc());
        app.handle_command("q");
        assert!(!app.should_quit, ":q must refuse when modified");
        app.handle_command("q!");
        assert!(app.should_quit);

        std::fs::remove_dir_all(&dir).ok();
    }
}
