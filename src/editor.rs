//! Vim modal editing layer over tui-textarea.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, TextArea};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
}

/// Result of handling a key in the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    /// Text changed; the preview debounce should restart.
    Changed,
    /// An ex command was submitted (`:w`, `:q`, ...); handled by the app.
    Command(String),
}

/// Active command-line input: '/' search or ':' ex command.
#[derive(Debug, Clone)]
pub struct CmdLine {
    pub kind: char,
    pub input: String,
}

pub struct Editor {
    pub ta: TextArea<'static>,
    pub mode: Mode,
    /// Multi-key sequences in flight: pending 'g' (gg), 'd' (dd/dw), 'y' (yy).
    pending: Option<char>,
    pub cmdline: Option<CmdLine>,
    register: Vec<String>,
    /// Source ended with a newline; preserved by `text()` for byte-identical
    /// file round-trips.
    trailing_newline: bool,
}

impl Default for Editor {
    fn default() -> Self {
        Editor::new("")
    }
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("mode", &self.mode)
            .field("cursor", &self.ta.cursor())
            .field("lines", &self.ta.lines().len())
            .field("cmdline", &self.cmdline)
            .finish()
    }
}

impl Editor {
    pub fn new(source: &str) -> Self {
        let mut ta = TextArea::from(source.lines().map(|l| l.to_string()).collect::<Vec<_>>());
        ta.set_line_number_style(ratatui::style::Style::new().fg(ratatui::style::Color::DarkGray));
        Self {
            ta,
            mode: Mode::Normal,
            pending: None,
            cmdline: None,
            register: Vec::new(),
            trailing_newline: source.ends_with('\n'),
        }
    }

    pub fn text(&self) -> String {
        let mut t = self.ta.lines().join("\n");
        if self.trailing_newline && !t.is_empty() {
            t.push('\n');
        }
        t
    }

    pub fn set_text(&mut self, source: &str) {
        self.ta = TextArea::from(source.lines().map(|l| l.to_string()).collect::<Vec<_>>());
        self.ta.set_line_number_style(
            ratatui::style::Style::new().fg(ratatui::style::Color::DarkGray),
        );
        self.trailing_newline = source.ends_with('\n');
    }

    pub fn cursor(&self) -> (usize, usize) {
        self.ta.cursor()
    }

    pub fn mode_str(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if let Some(mut cmd) = self.cmdline.take() {
            let (action, keep_open) = self.handle_cmdline_key(key, &mut cmd);
            if keep_open {
                self.cmdline = Some(cmd);
            }
            return action;
        }
        match self.mode {
            Mode::Insert => self.handle_insert_key(key),
            Mode::Normal => self.handle_normal_key(key),
        }
    }

    fn handle_cmdline_key(&mut self, key: KeyEvent, cmd: &mut CmdLine) -> (Action, bool) {
        match key.code {
            KeyCode::Esc => (Action::None, false),
            KeyCode::Enter => {
                let input = std::mem::take(&mut cmd.input);
                if cmd.kind == ':' {
                    (Action::Command(input), false)
                } else {
                    self.run_search(&input);
                    (Action::None, false)
                }
            }
            KeyCode::Backspace => {
                let keep = cmd.input.pop().is_some();
                (Action::None, keep)
            }
            KeyCode::Char(c) => {
                cmd.input.push(c);
                (Action::None, true)
            }
            _ => (Action::None, true),
        }
    }

    fn run_search(&mut self, pattern: &str) {
        if self.ta.set_search_pattern(pattern).is_ok() {
            let _ = self.ta.search_forward(true);
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.ta.move_cursor(CursorMove::Back);
                Action::None
            }
            KeyCode::Char(c) => {
                self.ta.insert_char(c);
                Action::Changed
            }
            KeyCode::Enter => {
                self.ta.insert_newline();
                Action::Changed
            }
            KeyCode::Backspace => {
                self.ta.delete_char();
                Action::Changed
            }
            KeyCode::Tab => Action::None, // focus switching handled by app
            _ => Action::None,
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Action {
        if let Some(p) = self.pending.take() {
            return self.handle_sequence(p, key.code);
        }

        let m = key.modifiers;
        match key.code {
            KeyCode::Char('r') if m.contains(KeyModifiers::CONTROL) => {
                let _ = self.ta.redo();
                Action::Changed
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.ta.move_cursor(CursorMove::Back);
                Action::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.ta.move_cursor(CursorMove::Forward);
                Action::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.ta.move_cursor(CursorMove::Down);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.ta.move_cursor(CursorMove::Up);
                Action::None
            }
            KeyCode::Char('w') => {
                self.ta.move_cursor(CursorMove::WordForward);
                Action::None
            }
            KeyCode::Char('b') => {
                self.ta.move_cursor(CursorMove::WordBack);
                Action::None
            }
            KeyCode::Char('e') => {
                self.ta.move_cursor(CursorMove::WordEnd);
                Action::None
            }
            KeyCode::Char('0') | KeyCode::Home => {
                self.ta.move_cursor(CursorMove::Head);
                Action::None
            }
            KeyCode::Char('$') | KeyCode::End => {
                self.ta.move_cursor(CursorMove::End);
                Action::None
            }
            KeyCode::Char('^') => {
                self.ta.move_cursor(CursorMove::Head);
                Action::None
            }
            KeyCode::Char('g') => {
                self.pending = Some('g');
                Action::None
            }
            KeyCode::Char('G') => {
                self.ta.move_cursor(CursorMove::Bottom);
                Action::None
            }
            KeyCode::Char('{') => {
                self.ta.move_cursor(CursorMove::ParagraphBack);
                Action::None
            }
            KeyCode::Char('}') => {
                self.ta.move_cursor(CursorMove::ParagraphForward);
                Action::None
            }
            KeyCode::Char('i') => {
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('a') => {
                self.ta.move_cursor(CursorMove::Forward);
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('I') => {
                self.ta.move_cursor(CursorMove::Head);
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('A') => {
                self.ta.move_cursor(CursorMove::End);
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('o') => {
                self.ta.move_cursor(CursorMove::End);
                self.ta.insert_newline();
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('O') => {
                self.ta.move_cursor(CursorMove::Head);
                self.ta.insert_newline();
                self.ta.move_cursor(CursorMove::Up);
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('x') => {
                let _ = self.ta.delete_next_char();
                Action::Changed
            }
            KeyCode::Char('D') => {
                let _ = self.ta.delete_line_by_end();
                Action::Changed
            }
            KeyCode::Char('d') => {
                self.pending = Some('d');
                Action::None
            }
            KeyCode::Char('y') => {
                self.pending = Some('y');
                Action::None
            }
            KeyCode::Char('p') => self.paste(false),
            KeyCode::Char('P') => self.paste(true),
            KeyCode::Char('u') => {
                let _ = self.ta.undo();
                Action::Changed
            }
            KeyCode::Char('/') => {
                self.cmdline = Some(CmdLine {
                    kind: '/',
                    input: String::new(),
                });
                Action::None
            }
            KeyCode::Char(':') => {
                self.cmdline = Some(CmdLine {
                    kind: ':',
                    input: String::new(),
                });
                Action::None
            }
            KeyCode::Char('n') => {
                let _ = self.ta.search_forward(false);
                Action::None
            }
            KeyCode::Char('N') => {
                let _ = self.ta.search_back(false);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_sequence(&mut self, leader: char, code: KeyCode) -> Action {
        match (leader, code) {
            ('g', KeyCode::Char('g')) => {
                self.ta.move_cursor(CursorMove::Jump(0, 0));
                Action::None
            }
            ('d', KeyCode::Char('d')) => {
                let (row, _) = self.ta.cursor();
                if let Some(line) = self.ta.lines().get(row) {
                    self.register = vec![line.to_string()];
                    let len = line.chars().count();
                    self.ta.move_cursor(CursorMove::Jump(row as u16, 0));
                    let _ = self.ta.delete_str(len + 1); // also eats the newline
                }
                Action::Changed
            }
            ('d', KeyCode::Char('w')) => {
                let _ = self.ta.delete_next_word();
                Action::Changed
            }
            ('y', KeyCode::Char('y')) => {
                let (row, _) = self.ta.cursor();
                if let Some(line) = self.ta.lines().get(row) {
                    self.register = vec![line.to_string()];
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn paste(&mut self, before: bool) -> Action {
        if self.register.is_empty() {
            return Action::None;
        }
        let (row, _) = self.ta.cursor();
        if before {
            self.ta.move_cursor(CursorMove::Jump(row as u16, 0));
        } else {
            self.ta.move_cursor(CursorMove::Jump(row as u16, u16::MAX));
            self.ta.move_cursor(CursorMove::End);
        }
        self.ta.insert_newline();
        for (i, line) in self.register.iter().enumerate() {
            if i > 0 {
                self.ta.insert_newline();
            }
            if !line.is_empty() {
                let _ = self.ta.insert_str(line);
            }
        }
        Action::Changed
    }

    /// Move the cursor to a (row, col) in buffer coordinates (mouse click).
    pub fn jump(&mut self, row: usize, col: usize) {
        let rows = self.ta.lines().len();
        let r = row.min(rows.saturating_sub(1));
        let line_len = self
            .ta
            .lines()
            .get(r)
            .map(|l| l.chars().count())
            .unwrap_or(0);
        let c = col.min(line_len);
        self.ta.move_cursor(CursorMove::Jump(r as u16, c as u16));
    }

    /// Mouse wheel scroll.
    pub fn scroll(&mut self, rows: i16) {
        self.ta.scroll((rows, 0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    fn editor(text: &str) -> Editor {
        Editor::new(text)
    }

    #[test]
    fn insert_mode_typing() {
        let mut e = editor("");
        e.handle_key(key('i'));
        assert_eq!(e.mode, Mode::Insert);
        e.handle_key(key('a'));
        e.handle_key(key('b'));
        assert_eq!(e.text(), "ab");
        e.handle_key(esc());
        assert_eq!(e.mode, Mode::Normal);
    }

    #[test]
    fn backspace_in_insert() {
        let mut e = editor("hello");
        e.handle_key(key('A'));
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(e.text(), "hel");
    }

    #[test]
    fn motions_move_cursor() {
        let mut e = editor("hello world\nsecond line\nthird");
        e.handle_key(key('l'));
        e.handle_key(key('l'));
        assert_eq!(e.cursor(), (0, 2));
        e.handle_key(key('j'));
        assert_eq!(e.cursor().0, 1);
        e.handle_key(key('0'));
        assert_eq!(e.cursor(), (1, 0));
        e.handle_key(key('$'));
        let (_, c) = e.cursor();
        assert_eq!(c, "second line".chars().count());
        e.handle_key(key('G'));
        assert_eq!(e.cursor().0, 2);
        e.handle_key(key('g'));
        e.handle_key(key('g'));
        assert_eq!(e.cursor(), (0, 0));
    }

    #[test]
    fn word_motions() {
        let mut e = editor("foo bar baz");
        e.handle_key(key('w'));
        assert_eq!(e.cursor(), (0, 4));
        e.handle_key(key('e'));
        assert_eq!(e.cursor(), (0, 6));
        e.handle_key(key('b'));
        assert_eq!(e.cursor(), (0, 4));
    }

    #[test]
    fn x_dd_yy_p() {
        let mut e = editor("one\ntwo\nthree");
        e.handle_key(key('j'));
        e.handle_key(key('x'));
        assert_eq!(e.text(), "one\nwo\nthree");
        e.handle_key(key('y'));
        e.handle_key(key('y'));
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one\nthree");
        // vim dd+p swaps the line below the cursor position
        e.handle_key(key('p'));
        assert_eq!(e.text(), "one\nthree\nwo");
    }

    #[test]
    fn undo_redo() {
        let mut e = editor("abc");
        e.handle_key(key('x'));
        assert_eq!(e.text(), "bc");
        e.handle_key(key('u'));
        assert_eq!(e.text(), "abc");
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "bc");
    }

    #[test]
    fn open_lines_below_and_above() {
        let mut e = editor("one");
        e.handle_key(key('o'));
        assert_eq!(e.mode, Mode::Insert);
        e.handle_key(key('t'));
        assert_eq!(e.text(), "one\nt");
        e.handle_key(esc());
        e.handle_key(key('O'));
        assert_eq!(e.text(), "one\n\nt");
        assert_eq!(e.cursor().0, 1);
    }

    #[test]
    fn search_jumps_to_match() {
        let mut e = editor("alpha\nbeta\ngamma");
        // 'e' only occurs in "beta"
        e.handle_key(key('/'));
        e.cmdline.as_mut().unwrap().input.push('e');
        e.handle_key(enter());
        assert_eq!(e.cursor(), (1, 1));
        // only match: n wraps back onto it
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (1, 1));
        e.handle_key(key('N'));
        assert_eq!(e.cursor(), (1, 1));
    }

    #[test]
    fn cmdline_enter_returns_command() {
        let mut e = editor("x");
        e.handle_key(key(':'));
        e.cmdline.as_mut().unwrap().input.push('w');
        let a = e.handle_key(enter());
        assert_eq!(a, Action::Command("w".to_string()));
        assert!(e.cmdline.is_none());
    }

    #[test]
    fn jump_clamps_to_bounds() {
        let mut e = editor("abc\n de");
        e.jump(99, 99);
        assert_eq!(e.cursor(), (1, 3));
        e.jump(0, 0);
        assert_eq!(e.cursor(), (0, 0));
    }
}
