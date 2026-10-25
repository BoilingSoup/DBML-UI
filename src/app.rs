//! Application state and the iced update loop.

use crate::canvas::diagram::{hit_test, to_cell_f};
use crate::editor::{self, Action, Editor, KeyCode, KeyEvent, KeyModifiers};
use dbml_ui::layout::{self, Layout};
use dbml_ui::{dbml, model};
use iced::{Point, Vector};

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
    /// The divider between the panes was pressed; drag to resize.
    DividerPressed,
    /// Window-relative mouse move (drives divider drags).
    MouseMovedWindow(f32, f32),
    /// Left button released anywhere.
    MouseReleased,
    /// Window resized.
    WindowResized(f32, f32),
    /// A completion popup row was clicked.
    CompletionPicked(usize),
    /// Mouse hover over the editor pane (local px, None when it left).
    EditorHover(Option<Point>),
    /// Animation frame for the editor scroll.
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Canvas,
}

/// Open completion popup: candidate items, the selected index, where the
/// completed word starts in the buffer, and the typed prefix.
/// An in-flight animated scroll, neoscroll.nvim-style: the viewport
/// advances in WHOLE-LINE steps (text never renders at a fractional
/// offset), with the delay between steps shaped by an ease-out curve.
/// `applied` tracks how many steps are already on screen so each tick
/// only draws the difference.
#[derive(Debug, Clone)]
pub struct ScrollAnim {
    pub from: f32,
    pub to: f32,
    pub started: std::time::Instant,
    /// Seconds; see `scroll_duration_override`.
    pub duration: f32,
    /// Whole-line steps already applied.
    pub applied: f32,
    /// Cursor rows coupled to the scroll (neoscroll scrolls the cursor one
    /// line per step alongside the window, gj/gk with ctrl-e/y). None for
    /// view-only animations (G/gg/jump-to-definition).
    pub cursor: Option<(usize, usize)>,
}

/// Duration override in ms for slow-motion verification
/// (DBML_UI_SCROLL_MS); None in normal use.
fn scroll_duration_override() -> Option<f32> {
    std::env::var("DBML_UI_SCROLL_MS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .map(|ms| ms / 1000.0)
}

/// neoscroll's canonical mapping durations.
pub const HALF_PAGE_MS: f32 = 250.0;
pub const FULL_PAGE_MS: f32 = 450.0;

/// Distance-progress at time-fraction `y` under neoscroll's quadratic
/// easing. neoscroll applies easing to the TIME between line steps
/// (ef(x) = 1 - (1-x)^(1/2) maps distance to time); inverting gives the
/// distance covered by a given time — a quadratic ease-out in space:
/// fast start, long decelerating arrival.
fn scroll_progress(y: f32) -> f32 {
    1.0 - (1.0 - y.clamp(0.0, 1.0)).powi(2)
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub items: Vec<crate::editor::completion::Item>,
    pub selected: usize,
    /// Buffer position where the completed word starts.
    pub word: crate::editor::buffer::Pos,
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
    /// Dragging the pane divider.
    Divider,
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
    /// Active animated scroll (neoscroll-style): interpolate editor_scroll
    /// from `from` to `to` over SCROLL_DURATION.
    pub scroll_anim: Option<ScrollAnim>,
    /// Accumulated target of the active animated scroll (repeated Ctrl-d
    /// extends it instead of restarting).
    scroll_target: Option<f32>,
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
    /// File backing the buffer, if any.
    pub file: Option<std::path::PathBuf>,
    /// Buffer contents at the last save/open — modified-flag reference.
    pub saved_text: String,
    /// Quit requested (window close Task follows).
    pub should_quit: bool,
    /// Status-bar toast.
    pub message: Option<String>,
    /// Syntax-highlight tokens for `source` (byte spans), rebuilt with it.
    pub highlights: Vec<dbml_ui::dbml::highlight::Token>,
    /// Open completion popup (insert mode).
    pub completion: Option<Completion>,
    /// Editor-pane hover position (local px) for error tooltips.
    pub editor_hover: Option<Point>,
    /// Help overlay visibility.
    pub show_help: bool,
    /// Pane split fraction (editor share), driven by divider drags.
    pub split: f32,
    /// Window size for split math.
    pub window_size: (f32, f32),
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
            scroll_anim: None,
            scroll_target: None,
            diagram: layout::layout(&model::build(&dbml::parser::parse("").doc)),
            parse_errors: Vec::new(),
            table_pos: Default::default(),
            zoom: crate::view::BASE_ZOOM,
            pan: Vector::default(),
            drag: Drag::None,
            canvas_cursor: None,
            pending_jump: None,
            file: None,
            saved_text: String::new(),
            should_quit: false,
            message: None,
            highlights: Vec::new(),
            completion: None,
            editor_hover: None,
            show_help: false,
            split: 0.4,
            window_size: (1400.0, 900.0),
        };
        app.editor.set_text(&source);
        app.set_source(&source);
        app
    }

    /// Re-parse + re-layout (keeps dragged table positions).
    pub fn set_source(&mut self, src: &str) {
        self.source = src.to_string();
        self.highlights = dbml_ui::dbml::highlight::highlight(src);
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
                // free positioning: boxes may go above/left of the origin
                let pos = (
                    ((cx - grab.0).round() as i32),
                    ((cy - grab.1).round() as i32),
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
            Drag::Divider | Drag::None => {}
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
    let mouse = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
            Some(Message::MouseMovedWindow(position.x, position.y))
        }
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
            Some(Message::MouseReleased)
        }
        _ => None,
    });
    // Full keyboard events, not on_key_press: the logical key it hands out
    // is unmodified (Shift+a arrives as 'a'), which made capitals and
    // shifted symbols impossible. KeyPressed.modified_key has shift and
    // CapsLock applied (everything but Ctrl), and `text` is what typing
    // actually produced.
    let keys = iced::event::listen_with(|event, _status, _id| match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key,
            modified_key,
            modifiers,
            ..
        }) => {
            let code = match &modified_key {
                iced::keyboard::Key::Character(c) => c.chars().next().map(KeyCode::Char),
                iced::keyboard::Key::Named(n) => named_code(n),
                _ => None,
            }?;
            // a Ctrl chord must keep the UNMODIFIED character (Ctrl+r, not Ctrl+R)
            let code = if modifiers.control() {
                match &key {
                    iced::keyboard::Key::Character(c) => {
                        c.chars().next().map(KeyCode::Char).unwrap_or(code)
                    }
                    _ => code,
                }
            } else {
                code
            };
            let m = KeyModifiers::new(modifiers.control(), modifiers.shift(), modifiers.alt());
            Some(Message::KeyPressed(code, m))
        }
        _ => None,
    });
    let tick = if _app.scroll_anim.is_some() {
        iced::time::every(std::time::Duration::from_millis(16)).map(|_| Message::Tick)
    } else {
        iced::Subscription::none()
    };
    iced::Subscription::batch([mouse, keys, tick, window_resize()])
}

fn named_code(n: &iced::keyboard::key::Named) -> Option<KeyCode> {
    Some(match n {
        iced::keyboard::key::Named::Escape => KeyCode::Esc,
        iced::keyboard::key::Named::Enter => KeyCode::Enter,
        iced::keyboard::key::Named::Space => KeyCode::Space,
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
    })
}

fn window_resize() -> iced::Subscription<Message> {
    iced::window::resize_events().map(|(id, size)| {
        let _ = id;
        Message::WindowResized(size.width, size.height)
    })
}

pub fn update(app: &mut App, message: Message) -> iced::Task<Message> {
    handle_message(app, message);
    if app.should_quit {
        return iced::exit();
    }
    iced::Task::none()
}

fn handle_message(app: &mut App, message: Message) {
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
            // instant (neoscroll default) and takes over any animation
            app.scroll_anim = None;
            app.scroll_target = None;
            app.editor_scroll = (app.editor_scroll + n).clamp(0.0, app.max_scroll());
        }
        Message::DividerPressed => app.drag = Drag::Divider,
        Message::MouseMovedWindow(x, _y) => {
            if app.drag == Drag::Divider {
                const PAD: f32 = 16.0;
                let usable = (app.window_size.0 - 2.0 * PAD).max(1.0);
                app.split = ((x - PAD) / usable).clamp(0.15, 0.85);
            }
        }
        Message::MouseReleased => {
            if app.drag == Drag::Divider {
                app.drag = Drag::None;
            }
        }
        Message::WindowResized(w, h) => app.window_size = (w, h),
        Message::CompletionPicked(idx) => app.completion_accept(idx),
        Message::EditorHover(p) => app.editor_hover = p,
        Message::Tick => {
            let Some(anim) = app.scroll_anim.clone() else {
                return;
            };
            let y = (anim.started.elapsed().as_secs_f32() / anim.duration).min(1.0);
            let range = (anim.to - anim.from).abs();
            let dir = (anim.to - anim.from).signum();
            // whole-line steps due by now (ease-out in space)
            let due = (scroll_progress(y) * range).round();
            if due != anim.applied {
                let scroll = anim.from + due * dir;
                if let Some((from_row, to_row)) = anim.cursor {
                    let frac = if range > 0.0 { due / range } else { 1.0 };
                    let row = ((from_row as f32 + (to_row as f32 - from_row as f32) * frac).round()
                        as i32)
                        .clamp(0, app.editor.buf.line_count() as i32 - 1)
                        as usize;
                    let col = app.editor.buf.pos().col;
                    app.editor
                        .buf
                        .move_cursor(crate::editor::buffer::Move::Jump(row, col));
                }
                app.editor_scroll = scroll;
                if let Some(a) = app.scroll_anim.as_mut() {
                    a.applied = due;
                }
                if std::env::var("DBML_UI_DEBUG").is_ok() {
                    eprintln!(
                        "DBG tick y={y:.3} scroll={scroll:.0}/{:.0} cursor coupled={}",
                        anim.to,
                        anim.cursor.is_some()
                    );
                }
            }
            if y >= 1.0 {
                app.editor_scroll = anim.to;
                if let Some((_, to_row)) = anim.cursor {
                    let col = app.editor.buf.pos().col;
                    app.editor
                        .buf
                        .move_cursor(crate::editor::buffer::Move::Jump(to_row, col));
                }
                app.scroll_anim = None;
                app.scroll_target = None;
            }
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
    pub fn is_modified(&self) -> bool {
        self.editor.text() != self.saved_text
    }

    pub fn open_file(&mut self, path: &str) -> std::io::Result<()> {
        let content = std::fs::read_to_string(path)?;
        self.table_pos.clear();
        self.file = Some(std::path::PathBuf::from(path));
        self.saved_text = content.clone();
        self.editor.set_text(&content);
        self.set_source(&content);
        self.editor_scroll = 0.0;
        self.message = Some(format!(
            "\"{path}\" {}L, {}B",
            content.lines().count(),
            content.len()
        ));
        Ok(())
    }

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

    /// Export the diagram to `.svg` or `.png` (2x scale).
    pub fn export_diagram(&self, path: &str) -> anyhow::Result<String> {
        use dbml_ui::render::{raster, svg};
        if path.ends_with(".png") {
            let doc = svg::build_svg(&self.diagram);
            let (sw, sh) = svg::svg_size(&self.diagram);
            let scale = 2.0;
            let img =
                raster::rasterize(&doc, (sw * scale) as u32, (sh * scale) as u32, scale, scale)?;
            img.save(path)?;
        } else {
            std::fs::write(path, svg::build_svg(&self.diagram))?;
        }
        Ok(format!("exported {path}"))
    }

    /// Execute an ex command (already stripped of ':').
    pub fn handle_command(&mut self, input: &str) {
        use dbml_ui::commands::ExCommand;
        match dbml_ui::commands::parse_ex(input) {
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
                let path = if arg.is_empty() {
                    "diagram.svg".to_string()
                } else {
                    arg
                };
                match self.export_diagram(&path) {
                    Ok(msg) => self.message = Some(msg),
                    Err(e) => self.message = Some(format!("export failed: {e}")),
                }
            }
            ExCommand::Unknown(_) => self.message = Some(format!("not a command: {input}")),
        }
    }

    fn key_pressed(&mut self, code: KeyCode, mods: KeyModifiers) {
        let key = KeyEvent::new(code, mods);
        // global shortcuts
        if mods.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') => {
                    if let Err(e) = self.save_file(None) {
                        self.message = Some(format!("save failed: {e}"));
                    }
                    return;
                }
                KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                _ => {}
            }
        }
        // '?' toggles the help overlay (unless typing text)
        let typing = self.focus == Focus::Editor
            && (self.editor.mode == editor::Mode::Insert || self.editor.cmdline.is_some());
        if key.code == KeyCode::Char('?') && !typing {
            self.show_help = !self.show_help;
            return;
        }
        if self.show_help {
            // any other key closes the overlay first
            self.show_help = false;
            return;
        }
        // Tab always toggles pane focus (and dismisses the popup)
        if key.code == KeyCode::Tab {
            self.completion = None;
            self.focus = match self.focus {
                Focus::Editor => Focus::Canvas,
                Focus::Canvas => Focus::Editor,
            };
            return;
        }
        // the cmdline is reachable from either pane
        if self.editor.cmdline.is_some() {
            let action = self.editor.handle_key(key);
            self.after_edit(action);
            return;
        }
        if self.focus == Focus::Editor {
            self.editor_key(key);
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
                self.editor_key(key);
            }
            _ => {}
        }
    }

    /// Keys routed to the editor pane, with the completion popup
    /// intercepting navigation/filter/accept keys first.
    fn editor_key(&mut self, key: KeyEvent) {
        // page scrolling works in every mode, before anything else
        match key.code {
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.half_page(1.0);
                return;
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.half_page(-1.0);
                return;
            }
            KeyCode::PageDown => {
                self.page_scroll(1.0);
                return;
            }
            KeyCode::PageUp => {
                self.page_scroll(-1.0);
                return;
            }
            _ => {}
        }
        // Ctrl+Space toggles the popup (VSCode muscle memory)
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Space {
            if self.editor.mode == editor::Mode::Insert {
                self.toggle_completion();
            }
            return;
        }
        if self.completion.is_some() {
            match key.code {
                KeyCode::Up => {
                    let c = self.completion.as_mut().unwrap();
                    c.selected = c.selected.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    let c = self.completion.as_mut().unwrap();
                    c.selected = (c.selected + 1).min(c.items.len().saturating_sub(1));
                    return;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    let idx = self.completion.as_ref().unwrap().selected;
                    self.completion_accept(idx);
                    return;
                }
                KeyCode::Esc => {
                    self.completion = None;
                    return;
                }
                KeyCode::Char(c) if c.is_alphanumeric() || c == '_' => {
                    let action = self.editor.handle_key(key);
                    self.after_edit(action);
                    self.refilter_completion();
                    return;
                }
                KeyCode::Backspace => {
                    let action = self.editor.handle_key(key);
                    self.after_edit(action);
                    self.refilter_completion();
                    return;
                }
                _ => self.completion = None, // any other key dismisses, then proceeds
            }
        }
        let action = self.editor.handle_key(key);
        // auto-triggers, insert mode only
        if self.editor.mode == editor::Mode::Insert {
            match key.code {
                KeyCode::Char(c) if c.is_alphanumeric() || c == '_' => {
                    self.maybe_autocomplete(false);
                }
                KeyCode::Char('.') => self.maybe_autocomplete(true),
                _ => {}
            }
        }
        self.after_edit(action);
    }

    /// The word being typed before the cursor: its start position and text.
    fn word_before_cursor(&self) -> Option<(crate::editor::buffer::Pos, String)> {
        let (row, col) = self.editor.buf.cursor();
        let line = self.editor.buf.line(row);
        let start = line[..line.chars().count().min(col)]
            .char_indices()
            .collect::<Vec<_>>();
        let mut s = col;
        while s > 0 {
            let ch = line.chars().nth(s - 1)?;
            if ch.is_alphanumeric() || ch == '_' {
                s -= 1;
            } else {
                break;
            }
        }
        let prefix: String = line.chars().skip(s).take(col - s).collect();
        let _ = start;
        (s < col).then_some((crate::editor::buffer::Pos { row, col: s }, prefix))
    }

    /// The byte offset of the cursor in `source` (for highlight lookups).
    fn cursor_byte(&self) -> usize {
        let (row, col) = self.editor.buf.cursor();
        let mut b = 0usize;
        for l in self.editor.buf.lines().iter().take(row) {
            b += l.len() + 1;
        }
        b += self
            .editor
            .buf
            .line(row)
            .chars()
            .take(col)
            .map(char::len_utf8)
            .sum::<usize>();
        b
    }

    /// Inside a string or comment: completions would be noise there.
    fn in_prose(&self) -> bool {
        let b = self.cursor_byte();
        if b == 0 {
            return false;
        }
        let idx = self.highlights.partition_point(|t| t.end < b);
        self.highlights
            .get(idx)
            .map(|t| {
                t.start < b
                    && matches!(
                        t.kind,
                        dbml_ui::dbml::highlight::Kind::String
                            | dbml_ui::dbml::highlight::Kind::Comment
                    )
            })
            .unwrap_or(false)
    }

    fn items_for(
        &self,
        word: crate::editor::buffer::Pos,
        prefix: &str,
    ) -> Vec<crate::editor::completion::Item> {
        let line = self.editor.buf.line(word.row);
        let before: String = line.chars().take(word.col).collect();
        crate::editor::completion::complete(prefix, &before, &self.diagram)
    }

    /// Open the popup if the context warrants it. `after_dot` allows an
    /// empty prefix (member completion right after `table.`).
    fn maybe_autocomplete(&mut self, after_dot: bool) {
        if self.in_prose() {
            self.completion = None;
            return;
        }
        let word = match self.word_before_cursor() {
            Some(w) => w,
            None => {
                if after_dot {
                    // empty prefix right after the dot
                    let (row, col) = self.editor.buf.cursor();
                    (crate::editor::buffer::Pos { row, col }, String::new())
                } else {
                    return;
                }
            }
        };
        // non-dot auto-open needs a real prefix; `.` needs the char before
        // the cursor to actually be the dot
        let after_dot_ctx = after_dot
            && word.1.is_empty()
            && word
                .0
                .col
                .checked_sub(1)
                .and_then(|c| self.editor.buf.line(word.0.row).chars().nth(c))
                == Some('.');
        if !after_dot && word.1.chars().count() < 2 {
            return;
        }
        if after_dot && !after_dot_ctx && word.1.chars().count() < 2 {
            return;
        }
        let items = self.items_for(word.0, &word.1);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion = Some(Completion {
            items,
            selected: 0,
            word: word.0,
        });
    }

    fn toggle_completion(&mut self) {
        if self.completion.is_some() {
            self.completion = None;
        } else {
            let word = self.word_before_cursor().unwrap_or_else(|| {
                let (row, col) = self.editor.buf.cursor();
                (crate::editor::buffer::Pos { row, col }, String::new())
            });
            let items = self.items_for(word.0, &word.1);
            if !items.is_empty() {
                self.completion = Some(Completion {
                    items,
                    selected: 0,
                    word: word.0,
                });
            }
        }
    }

    /// Recompute the popup for the extended/shrunk prefix; close when the
    /// prefix is gone or nothing matches.
    fn refilter_completion(&mut self) {
        let Some(c) = &self.completion else { return };
        let (row, col) = self.editor.buf.cursor();
        let line = self.editor.buf.line(row);
        if row != c.word.row || col < c.word.col {
            self.completion = None;
            return;
        }
        let prefix: String = line
            .chars()
            .skip(c.word.col)
            .take(col - c.word.col)
            .collect();
        if prefix.is_empty() {
            self.completion = None;
            return;
        }
        let items = self.items_for(c.word, &prefix);
        self.completion = if items.is_empty() {
            None
        } else {
            Some(Completion {
                selected: c.selected.min(items.len() - 1),
                items,
                word: c.word,
            })
        };
    }

    /// Replace the typed prefix with the chosen item and keep typing.
    fn completion_accept(&mut self, idx: usize) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let Some(item) = c.items.get(idx) else { return };
        let (row, col) = self.editor.buf.cursor();
        if row != c.word.row || col < c.word.col {
            return;
        }
        self.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(c.word.row, c.word.col));
        self.editor.buf.delete_str(col - c.word.col);
        self.editor.buf.insert_str(&item.label);
        if item.kind == crate::editor::completion::ItemKind::Keyword {
            self.editor.buf.insert_str(" ");
        }
        self.after_edit(Action::Changed);
    }

    fn after_edit(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Changed => {
                let text = self.editor.text();
                self.set_source(&text);
                self.keep_cursor_visible();
            }
            Action::Command(cmd) => self.handle_command(&cmd),
        }
    }

    /// Visible editor lines from the window size (panes + status bar).
    fn page_lines(&self) -> f32 {
        ((self.window_size.1 - 62.0) / crate::canvas::editor::LINE_HEIGHT).max(5.0)
    }

    fn max_scroll(&self) -> f32 {
        (self.editor.buf.line_count() as f32 - self.page_lines()).max(0.0)
    }

    /// Start/re-target a line-stepped animated scroll. `duration_ms` is the
    /// neoscroll duration for the WHOLE remaining distance; repeated
    /// requests extend the accumulated target (neoscroll behavior) and the
    /// animation restarts from the current stepped position.
    fn animate_scroll(&mut self, to: f32, duration_ms: f32, cursor: Option<(usize, usize)>) {
        let to = to.clamp(0.0, self.max_scroll()).round();
        let from = self.editor_scroll.round();
        if (to - from).abs() < 0.5 {
            self.editor_scroll = to;
            self.scroll_anim = None;
            self.scroll_target = None;
            return;
        }
        self.scroll_target = Some(to);
        self.scroll_anim = Some(ScrollAnim {
            from,
            to,
            started: std::time::Instant::now(),
            duration: scroll_duration_override().unwrap_or(duration_ms / 1000.0),
            applied: 0.0,
            cursor,
        });
    }

    /// neoscroll's distance-proportional duration (z-command formula):
    /// a half page takes HALF_PAGE_MS, scaling linearly with distance.
    fn scroll_by_lines(&mut self, lines: f32, duration_ms: f32) {
        self.completion = None;
        let base = self.scroll_target.unwrap_or(self.editor_scroll.round());
        let to = (base + lines).clamp(0.0, self.max_scroll()).round();
        // cursor target travels the same distance as the view, clamped
        let pos = self.editor.buf.pos();
        let rows = self.editor.buf.line_count();
        let cursor_to =
            ((pos.row as f32 + lines).round() as i32).clamp(0, rows as i32 - 1) as usize;
        let scroll_from = self.editor_scroll.round();
        let cursor_from = ((cursor_to as f32 - (to - scroll_from)).max(0.0)).round() as usize;
        self.animate_scroll(to, duration_ms, Some((cursor_from, cursor_to)));
        self.focus = Focus::Editor;
    }

    /// Half-page scroll with the cursor (vim Ctrl-d / Ctrl-u).
    fn half_page(&mut self, dir: f32) {
        self.scroll_by_lines((self.page_lines() / 2.0) * dir, HALF_PAGE_MS);
    }

    /// Full-page scroll with the cursor (PageDown / PageUp).
    fn page_scroll(&mut self, dir: f32) {
        self.scroll_by_lines(self.page_lines() * dir, FULL_PAGE_MS);
    }

    /// Scroll the editor viewport so the cursor stays on screen. Small
    /// corrections snap; big jumps (G, click-to-jump) animate.
    fn keep_cursor_visible(&mut self) {
        let row = self.editor.buf.pos().row as f32;
        let page = self.page_lines();
        let visible = page - 2.0;
        let target = if row < self.editor_scroll + 1.0 {
            (row - 1.0).max(0.0)
        } else if row > self.editor_scroll + visible {
            row - visible
        } else {
            return;
        };
        if (target - self.editor_scroll).abs() > page * 0.75 {
            // view-only animation (cursor already at its destination),
            // duration proportional to the remaining distance
            let ms = Self::scroll_duration_for((target - self.editor_scroll).abs(), page) * 1000.0;
            self.animate_scroll(target.round(), ms, None);
        } else {
            self.scroll_anim = None;
            self.scroll_target = None;
            self.editor_scroll = target.max(0.0);
        }
    }

    /// neoscroll z-command duration: a half page takes HALF_PAGE_MS,
    /// scaling linearly with distance, clamped to keep tiny scrolls snappy
    /// and huge ones bounded.
    fn scroll_duration_for(lines: f32, page: f32) -> f32 {
        (HALF_PAGE_MS * (lines / (page / 2.0)).max(0.05)).clamp(60.0, 600.0) / 1000.0
    }

    /// A click on a diagram table jumps the editor to its definition.
    fn consume_pending_jump(&mut self) {
        if let Some(title) = self.pending_jump.take() {
            if let Some(b) = self.diagram.boxes.iter().find(|b| b.title == title) {
                if let Some(span) = b.name_span.clone() {
                    let (row, col) = dbml::parser::line_col_at(&self.source, span.start);
                    self.editor.jump(row, col);
                    self.focus = Focus::Editor;
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
        let _ = update(&mut app, Message::CanvasPressed(p));
        let _ = update(&mut app, Message::CanvasMoved(Point::new(1990.0, 2004.0)));
        assert_eq!(app.pan, Vector::new(-10.0, 4.0));
        let _ = update(
            &mut app,
            Message::CanvasReleased(Point::new(1990.0, 2004.0)),
        );
        assert_eq!(app.drag, Drag::None);
    }

    #[test]
    fn wheel_zoom_is_cursor_anchored() {
        let mut app = app();
        let anchor = over_box(&app, "users", 7.0, 9.0);
        let _ = update(&mut app, Message::CanvasMoved(anchor));
        let before = crate::canvas::diagram::to_cell_f(anchor, app.zoom, app.pan);
        let _ = update(&mut app, Message::Wheel(1.0));
        let after = crate::canvas::diagram::to_cell_f(anchor, app.zoom, app.pan);
        assert!(app.zoom > 20.0, "zoomed in: {}", app.zoom);
        assert!(
            (before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01,
            "anchor drifted: {before:?} -> {after:?}"
        );

        let _ = update(&mut app, Message::Wheel(-1.0));
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
        let _ = update(&mut app, Message::CanvasPressed(press));
        assert!(matches!(app.drag, Drag::Table { .. }));
        // move 8 cells right, 2 down
        let s = crate::canvas::diagram::cell_size(app.zoom);
        let _ = update(
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

        let _ = update(
            &mut app,
            Message::CanvasReleased(Point::new(press.x + 80.0, press.y + 20.0)),
        );
        assert_eq!(app.pending_jump, None, "a real drag is not a click");
    }

    #[test]
    fn table_drag_moves_above_and_left_of_origin() {
        let mut app = app();
        // users sits at (0,0): press it and drag up-left past the origin
        let press = over_box(&app, "users", 3.0, 3.0);
        let _ = update(&mut app, Message::CanvasPressed(press));
        let s = crate::canvas::diagram::cell_size(app.zoom);
        let _ = update(
            &mut app,
            Message::CanvasMoved(Point::new(
                press.x - 6.0 * s.width,
                press.y - 4.0 * s.height,
            )),
        );
        let b = app
            .diagram
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap();
        assert!(b.x < 0 && b.y < 0, "moved past origin: {:?}", (b.x, b.y));
        assert_eq!(app.table_pos.get("users"), Some(&(b.x, b.y)));
        // and it survives a re-parse (edit)
        let src = app.source.clone();
        app.set_source(&src);
        let b = app
            .diagram
            .boxes
            .iter()
            .find(|b| b.title == "users")
            .unwrap();
        assert!(
            b.x < 0 && b.y < 0,
            "negative override survives: {:?}",
            (b.x, b.y)
        );
    }

    #[test]
    fn click_without_drag_jumps_editor_to_definition() {
        let mut app = app();
        let before = app.editor.buf.cursor();
        let p = over_box(&app, "posts", 4.0, 4.0);
        let _ = update(&mut app, Message::CanvasPressed(p));
        let _ = update(&mut app, Message::CanvasReleased(p));
        assert_eq!(app.focus, Focus::Editor);
        let after = app.editor.buf.cursor();
        assert_ne!(before, after, "cursor moved to the definition");
        let line = app.editor.buf.line(after.0);
        assert!(line.contains("posts"), "on the posts line: {line:?}");
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;

    #[test]
    fn file_round_trip_is_byte_identical() {
        let dir = std::env::temp_dir().join(format!("dbml-ui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.dbml");
        std::fs::write(&path, "Table t {\n  id integer [pk]\n}\n").unwrap();

        let mut app = App::default();
        app.open_file(path.to_str().unwrap()).unwrap();
        assert!(!app.is_modified());

        // edit: dd the second line
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(1, 0));
        let action = app
            .editor
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(action, Action::None);
        assert!(app.is_modified());

        app.save_file(None).unwrap();
        assert!(!app.is_modified());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), app.editor.text());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ex_commands_save_and_export() {
        let dir = std::env::temp_dir().join(format!("dbml-ui-cmd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("e.svg");
        let mut app = App::default();
        app.handle_command(&format!("export {}", out.display()));
        assert!(out.exists(), "svg exported");
        let png = dir.join("e.png");
        app.handle_command(&format!("export {}", png.display()));
        assert!(png.exists(), "png exported");

        let dbml = dir.join("x.dbml");
        app.handle_command(&format!("w {}", dbml.display()));
        assert!(dbml.exists(), "written via :w");
        assert!(!app.is_modified());

        // :q refuses when modified, :q! works
        app.editor
            .handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(app.is_modified());
        app.handle_command("q");
        assert!(!app.should_quit, "refuses with unsaved changes");
        app.handle_command("q!");
        assert!(app.should_quit);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use crate::editor::completion::ItemKind;

    fn insert_app() -> App {
        let mut app = App::default();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        app
    }

    fn ctrl_space() -> Message {
        Message::KeyPressed(KeyCode::Space, KeyModifiers::CONTROL)
    }

    #[test]
    fn ctrl_space_opens_and_toggles() {
        let mut app = insert_app();
        let _ = update(&mut app, ctrl_space());
        let c = app.completion.as_ref().expect("popup open");
        assert!(c.items.iter().any(|i| i.label == "Table"));
        assert!(c.items.iter().any(|i| i.label == "users"));
        let _ = update(&mut app, ctrl_space());
        assert!(app.completion.is_none(), "second Ctrl-Space closes");
    }

    #[test]
    fn typing_two_chars_autocompletes_and_filters() {
        let mut app = insert_app();
        for c in ['T', 'a'] {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        let c = app.completion.as_ref().expect("auto-opened");
        assert!(c.items.iter().any(|i| i.label == "Table"), "{c:?}");
        assert!(!c.items.iter().any(|i| i.label == "users"), "{c:?}");
    }

    #[test]
    fn enter_accepts_keyword_with_trailing_space() {
        let mut app = insert_app();
        for c in ['T', 'a'] {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.completion.is_none());
        assert!(
            app.source.starts_with("Table "),
            "got: {:?}",
            &app.source[..20]
        );
        let (_, col) = app.editor.buf.cursor();
        assert_eq!(col, "Table ".len());
    }

    #[test]
    fn dot_completes_columns_and_accept() {
        let mut app = App::with_source("Table users { id integer [pk] }\nRef: users\n".into());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('i'), KeyModifiers::NONE),
        );
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(1, 0));
        app.editor.buf.move_cursor(crate::editor::buffer::Move::End);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('.'), KeyModifiers::NONE),
        );
        let c = app.completion.as_ref().expect("dot opens columns");
        assert!(
            c.items
                .iter()
                .any(|i| i.label == "id" && i.kind == ItemKind::Column),
            "{c:?}"
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.source.ends_with("Ref: users.id\n"), "{:?}", app.source);
    }

    #[test]
    fn esc_dismisses_but_keeps_insert_mode() {
        let mut app = insert_app();
        let _ = update(&mut app, ctrl_space());
        assert!(app.completion.is_some());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(app.completion.is_none());
        assert_eq!(app.editor.mode, crate::editor::Mode::Insert);
    }

    #[test]
    fn navigation_moves_selection() {
        let mut app = insert_app();
        let _ = update(&mut app, ctrl_space());
        let start = app.completion.as_ref().unwrap().selected;
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(app.completion.as_ref().unwrap().selected, start + 1);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Up, KeyModifiers::NONE),
        );
        assert_eq!(app.completion.as_ref().unwrap().selected, start);
    }

    #[test]
    fn no_popup_inside_strings_or_comments() {
        let mut app = insert_app();
        // cursor inside the project Note string
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(2, 10));
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('a'), KeyModifiers::NONE),
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('b'), KeyModifiers::NONE),
        );
        assert!(app.completion.is_none(), "in prose: {:?}", app.completion);
    }

    #[test]
    fn backspace_refilters_and_closes_on_empty() {
        let mut app = insert_app();
        for c in ['T', 'a'] {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        assert!(app.completion.is_some());
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(
            app.completion.is_some(),
            "still a 1-char... closes below 2? prefix 'T' refilter keeps >=1 matches"
        );
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(app.completion.is_none(), "empty prefix closes");
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;
    use std::time::Duration;

    fn tall_app(lines: usize) -> App {
        let mut src = String::new();
        for i in 0..lines {
            src.push_str(&format!("Table t{i} {{\n  id integer [pk]\n}}\n"));
        }
        App::with_source(src)
    }

    fn force_finish(app: &mut App) {
        if let Some(a) = app.scroll_anim.as_mut() {
            a.started -= Duration::from_secs(1);
        }
        let _ = update(app, Message::Tick);
    }

    #[test]
    fn ctrl_d_couples_cursor_and_uses_half_page_duration() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let page = app.page_lines();
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.scroll_anim.as_ref().expect("animation running");
        assert!((anim.duration - 0.25).abs() < 1e-3, "250ms: {anim:?}");
        assert!((anim.to - page / 2.0).abs() < 1.0, "half page: {anim:?}");
        // neoscroll moves the cursor WITH the view, not ahead of it
        assert_eq!(app.editor.buf.pos().row, 0, "cursor not jumped");
        force_finish(&mut app);
        assert!((app.editor_scroll - page / 2.0).abs() < 1.0);
        assert_eq!(
            app.editor.buf.pos().row,
            (page / 2.0).round() as usize,
            "cursor arrives with the view"
        );
    }

    #[test]
    fn page_scroll_uses_the_full_page_duration() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::PageDown, KeyModifiers::NONE),
        );
        let anim = app.scroll_anim.as_ref().expect("anim");
        assert!((anim.duration - 0.45).abs() < 1e-3, "450ms: {anim:?}");
        assert!((anim.to - app.page_lines()).abs() < 1.0);
    }

    #[test]
    fn steps_are_whole_lines_and_ease_out() {
        // quadratic ease-out in space: at t=0.5, 75% of the distance is due
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.scroll_anim.as_mut().unwrap();
        let half = Duration::from_secs_f32(anim.duration / 2.0);
        anim.started -= half;
        let range = (anim.to - anim.from).abs();
        let _ = update(&mut app, Message::Tick);
        let expected = (scroll_progress(0.5) * range).round();
        assert!(
            (app.editor_scroll - expected).abs() < 0.01,
            "half time -> 75% distance: got {} want {expected}",
            app.editor_scroll
        );
        assert_eq!(app.editor_scroll, app.editor_scroll.round(), "whole lines");
    }

    #[test]
    fn repeated_ctrl_d_stacks_to_a_full_page() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        force_finish(&mut app);
        let after_one = app.editor_scroll;

        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let anim = app.scroll_anim.as_ref().unwrap();
        assert!(
            (anim.to - after_one - app.page_lines() / 2.0).abs() < 1.0,
            "second ctrl-d extends by another half page: {anim:?}"
        );
    }

    #[test]
    fn tick_completes_the_animation_at_the_target() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        let to = app.scroll_anim.as_ref().unwrap().to;
        force_finish(&mut app);
        assert!(app.scroll_anim.is_none());
        assert!((app.editor_scroll - to).abs() < 0.001, "lands exactly");
        assert!(app.editor_scroll >= 0.0);
    }

    #[test]
    fn scroll_clamps_at_buffer_bottom() {
        let mut app = tall_app(40);
        app.window_size = (1400.0, 900.0);
        for _ in 0..12 {
            let _ = update(
                &mut app,
                Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
            );
            force_finish(&mut app);
        }
        assert!(
            app.editor_scroll <= app.max_scroll() + 0.001,
            "clamped: {} vs {}",
            app.editor_scroll,
            app.max_scroll()
        );
    }

    #[test]
    fn scroll_progress_curve_shape() {
        // neoscroll quadratic easing, inverted: distance at time y
        assert_eq!(scroll_progress(0.0), 0.0);
        assert_eq!(scroll_progress(1.0), 1.0);
        assert!((scroll_progress(0.5) - 0.75).abs() < 1e-6, "ease-out");
        let mut prev = -0.001;
        for i in 0..=100 {
            let v = scroll_progress(i as f32 / 100.0);
            assert!(v > prev, "monotonic at {i}");
            assert!((0.0..=1.0).contains(&v));
            prev = v;
        }
    }

    #[test]
    fn proportional_duration_clamps() {
        assert!(
            (App::scroll_duration_for(1.0, 40.0) - 60.0 / 1000.0).abs() < 1e-3,
            "min"
        );
        let half_page = App::scroll_duration_for(20.0, 40.0);
        assert!((half_page - 0.25).abs() < 1e-3, "half page = 250ms");
        assert!(
            (App::scroll_duration_for(200.0, 40.0) - 0.6).abs() < 1e-3,
            "max"
        );
    }

    #[test]
    fn wheel_cancels_the_animation() {
        let mut app = tall_app(120);
        app.window_size = (1400.0, 900.0);
        let _ = update(
            &mut app,
            Message::KeyPressed(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert!(app.scroll_anim.is_some());
        let _ = update(&mut app, Message::EditorScrolled(-2.0));
        assert!(app.scroll_anim.is_none(), "wheel takes over");
    }
}
