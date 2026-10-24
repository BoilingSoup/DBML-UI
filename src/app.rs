use std::io;
use std::time::Duration;

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

use crate::ui;

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
}

impl App {
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
            // Poll so debounce ticks and animations can run between events.
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => self.on_key(key),
                    Event::Mouse(mouse) => self.on_mouse(mouse),
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Char('q'), _) => {
                self.should_quit = true;
            }
            (KeyCode::Tab, _) => {
                self.focus = match self.focus {
                    Focus::Editor => Focus::Canvas,
                    Focus::Canvas => Focus::Editor,
                };
            }
            _ => {}
        }
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
            _ => {}
        }
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.left() && col < rect.right() && row >= rect.top() && row < rect.bottom()
}
