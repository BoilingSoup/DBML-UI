//! Vim modal editing layer over tui-textarea.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tui_textarea::{CursorMove, TextArea};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
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

/// In-flight key sequences: `g`…, and the `d`/`c`/`y` operators awaiting a
/// motion or text object (`w b e 0 ^ $ G gg { }`, `iw`/`aw`, line doubling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    G,
    D,
    C,
    Y,
    /// `d i…` / `c i…` awaiting `w`
    Di,
    Ci,
    /// `d a…` / `c a…` awaiting `w`
    Da,
    Ca,
    /// `d g…` / `c g…` awaiting `g`
    Dg,
    Cg,
}

pub struct Editor {
    pub ta: TextArea<'static>,
    pub mode: Mode,
    /// Operator-pending / multi-key sequences in flight.
    pending: Option<Pending>,
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
            Mode::Visual => "VISUAL",
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
            Mode::Normal | Mode::Visual => self.handle_normal_key(key),
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
        if self.mode == Mode::Visual {
            return self.handle_visual_key(key);
        }
        if let Some(p) = self.pending.take() {
            return self.handle_sequence(p, key.code);
        }

        let m = key.modifiers;
        if let Some(action) = self.apply_motion(key.code) {
            return action;
        }
        match key.code {
            KeyCode::Char('r') if m.contains(KeyModifiers::CONTROL) => {
                let _ = self.ta.redo();
                Action::Changed
            }
            KeyCode::Char('g') => {
                self.pending = Some(Pending::G);
                Action::None
            }
            KeyCode::Char('v') => {
                self.mode = Mode::Visual;
                self.ta.start_selection();
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
            KeyCode::Char('C') => {
                let _ = self.ta.delete_line_by_end();
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('c') => {
                self.pending = Some(Pending::C);
                Action::None
            }
            KeyCode::Char('d') => {
                self.pending = Some(Pending::D);
                Action::None
            }
            KeyCode::Char('y') => {
                self.pending = Some(Pending::Y);
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

    fn handle_visual_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.ta.cancel_selection();
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Char('y') => {
                self.ta.copy();
                self.register = self
                    .ta
                    .yank_text()
                    .split('\n')
                    .map(str::to_string)
                    .collect();
                // copy() leaves selection_start set, which would make later
                // normal-mode motions extend an invisible selection
                self.ta.cancel_selection();
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.ta.cut();
                self.register = self
                    .ta
                    .yank_text()
                    .split('\n')
                    .map(str::to_string)
                    .collect();
                self.ta.cancel_selection();
                self.mode = Mode::Normal;
                Action::Changed
            }
            KeyCode::Char('o') => {
                // swap selection ends (vim vo); approximate by cancelling and
                // restarting from the current cursor
                self.ta.cancel_selection();
                self.ta.start_selection();
                Action::None
            }
            _ => {
                if let Some(action) = self.apply_motion(key.code) {
                    return action;
                }
                Action::None
            }
        }
    }

    /// Motions shared by normal and visual mode; Some(action) when handled.
    /// Selections extend automatically while `is_selecting`.
    fn apply_motion(&mut self, code: KeyCode) -> Option<Action> {
        let m = match code {
            KeyCode::Char('h') | KeyCode::Left => CursorMove::Back,
            KeyCode::Char('l') | KeyCode::Right => CursorMove::Forward,
            KeyCode::Char('j') | KeyCode::Down => CursorMove::Down,
            KeyCode::Char('k') | KeyCode::Up => CursorMove::Up,
            KeyCode::Char('w') => CursorMove::WordForward,
            KeyCode::Char('b') => CursorMove::WordBack,
            KeyCode::Char('e') => CursorMove::WordEnd,
            KeyCode::Char('0') | KeyCode::Home => CursorMove::Head,
            KeyCode::Char('$') | KeyCode::End => CursorMove::End,
            KeyCode::Char('^') => CursorMove::Head,
            KeyCode::Char('G') => CursorMove::Bottom,
            KeyCode::Char('{') => CursorMove::ParagraphBack,
            KeyCode::Char('}') => CursorMove::ParagraphForward,
            _ => return None,
        };
        self.ta.move_cursor(m);
        Some(Action::None)
    }

    fn handle_sequence(&mut self, leader: Pending, code: KeyCode) -> Action {
        use Pending::*;
        match (leader, code) {
            (G, KeyCode::Char('g')) => {
                self.ta.move_cursor(CursorMove::Jump(0, 0));
                Action::None
            }
            (Y, KeyCode::Char('y')) => {
                let (row, _) = self.ta.cursor();
                if let Some(line) = self.ta.lines().get(row) {
                    self.register = vec![line.to_string()];
                }
                Action::None
            }
            (D, KeyCode::Char('d')) => {
                let (row, _) = self.ta.cursor();
                if let Some(line) = self.ta.lines().get(row) {
                    self.register = vec![line.to_string()];
                    let len = line.chars().count();
                    self.ta.move_cursor(CursorMove::Jump(row as u16, 0));
                    let _ = self.ta.delete_str(len + 1); // also eats the newline
                }
                Action::Changed
            }
            (C, KeyCode::Char('c')) => {
                // cc: clear the line, keep it, and edit
                let (row, _) = self.ta.cursor();
                self.ta.move_cursor(CursorMove::Jump(row as u16, 0));
                let _ = self.ta.delete_line_by_end();
                self.mode = Mode::Insert;
                Action::Changed
            }
            // operator + motions; cw follows vim's cw == ce special case
            (D, KeyCode::Char('w')) => self.operate(&[CursorMove::WordForward], false, true, false),
            (C, KeyCode::Char('w')) => self.operate(&[CursorMove::WordEnd], true, true, true),
            (D, KeyCode::Char('e')) => self.operate(&[CursorMove::WordEnd], true, false, false),
            (C, KeyCode::Char('e')) => self.operate(&[CursorMove::WordEnd], true, false, true),
            (D, KeyCode::Char('b')) => self.operate(&[CursorMove::WordBack], false, false, false),
            (C, KeyCode::Char('b')) => self.operate(&[CursorMove::WordBack], false, false, true),
            (D, KeyCode::Char('0') | KeyCode::Home | KeyCode::Char('^')) => {
                self.operate(&[CursorMove::Head], false, false, false)
            }
            (C, KeyCode::Char('0') | KeyCode::Home | KeyCode::Char('^')) => {
                self.operate(&[CursorMove::Head], false, false, true)
            }
            (D, KeyCode::Char('$') | KeyCode::End) => {
                self.operate(&[CursorMove::End], true, false, false)
            }
            (C, KeyCode::Char('$') | KeyCode::End) => {
                self.operate(&[CursorMove::End], true, false, true)
            }
            (D, KeyCode::Char('G')) => {
                self.operate(&[CursorMove::Bottom, CursorMove::End], false, false, false)
            }
            (C, KeyCode::Char('G')) => {
                self.operate(&[CursorMove::Bottom, CursorMove::End], false, false, true)
            }
            (D, KeyCode::Char('{')) => {
                self.operate(&[CursorMove::ParagraphBack], false, false, false)
            }
            (C, KeyCode::Char('{')) => {
                self.operate(&[CursorMove::ParagraphBack], false, false, true)
            }
            (D, KeyCode::Char('}')) => {
                self.operate(&[CursorMove::ParagraphForward], false, false, false)
            }
            (C, KeyCode::Char('}')) => {
                self.operate(&[CursorMove::ParagraphForward], false, false, true)
            }
            (D, KeyCode::Char('g')) => {
                self.pending = Some(Dg);
                Action::None
            }
            (C, KeyCode::Char('g')) => {
                self.pending = Some(Cg);
                Action::None
            }
            (Dg, KeyCode::Char('g')) => {
                self.operate(&[CursorMove::Jump(0, 0)], false, false, false)
            }
            (Cg, KeyCode::Char('g')) => self.operate(&[CursorMove::Jump(0, 0)], false, false, true),
            // text objects
            (D, KeyCode::Char('i')) => {
                self.pending = Some(Di);
                Action::None
            }
            (C, KeyCode::Char('i')) => {
                self.pending = Some(Ci);
                Action::None
            }
            (D, KeyCode::Char('a')) => {
                self.pending = Some(Da);
                Action::None
            }
            (C, KeyCode::Char('a')) => {
                self.pending = Some(Ca);
                Action::None
            }
            (Di, KeyCode::Char('w')) => self.operate_word(false, false),
            (Ci, KeyCode::Char('w')) => self.operate_word(false, true),
            (Da, KeyCode::Char('w')) => self.operate_word(true, false),
            (Ca, KeyCode::Char('w')) => self.operate_word(true, true),
            _ => Action::None,
        }
    }

    /// Apply the pending `d`/`c` operator over a cursor motion: delete the
    /// characters between the cursor and wherever the motions land, then
    /// optionally enter insert mode. `inclusive` covers the landing char
    /// (for `e`/`$`); `clamp_line` keeps `dw`/`cw` from eating the newline
    /// (vim's last-word special case).
    fn operate(
        &mut self,
        motions: &[CursorMove],
        inclusive: bool,
        clamp_line: bool,
        insert: bool,
    ) -> Action {
        let start = self.ta.cursor();
        for m in motions {
            self.ta.move_cursor(*m);
        }
        let mut end = self.ta.cursor();
        if clamp_line && end.0 != start.0 {
            self.ta.move_cursor(CursorMove::Jump(start.0 as u16, 0));
            self.ta.move_cursor(CursorMove::End);
            end = self.ta.cursor();
        }
        let forward = end > start;
        let (from, to) = if forward { (start, end) } else { (end, start) };
        let mut n = self.chars_between(from, to);
        if inclusive && forward {
            n += 1;
        }
        if n > 0 {
            self.ta
                .move_cursor(CursorMove::Jump(from.0 as u16, from.1 as u16));
            let _ = self.ta.delete_str(n);
        }
        if insert {
            self.mode = Mode::Insert;
        }
        if n > 0 {
            Action::Changed
        } else {
            Action::None
        }
    }

    /// Number of characters strictly between two positions, `to > from`
    /// (newlines count as one character).
    fn chars_between(&self, from: (usize, usize), to: (usize, usize)) -> usize {
        let lines = self.ta.lines();
        if from == to || to < from {
            return 0;
        }
        if from.0 == to.0 {
            return to.1.saturating_sub(from.1);
        }
        let mut n = 0;
        if let Some(l) = lines.get(from.0) {
            n += l.chars().count().saturating_sub(from.1) + 1; // to EOL + newline
        }
        for r in from.0 + 1..to.0 {
            if let Some(l) = lines.get(r) {
                n += l.chars().count() + 1;
            }
        }
        n += to.1;
        n
    }

    /// `diw`/`ciw`/`daw`/`caw` over the word span at the cursor.
    fn operate_word(&mut self, a_word: bool, insert: bool) -> Action {
        let Some((s, e)) = self.word_span(a_word) else {
            return Action::None;
        };
        let (row, _) = self.ta.cursor();
        self.ta.move_cursor(CursorMove::Jump(row as u16, s as u16));
        let _ = self.ta.delete_str(e - s);
        if insert {
            self.mode = Mode::Insert;
        }
        Action::Changed
    }

    /// Char-index range of the word at the cursor: the contiguous run with
    /// the same whitespace-ness. `a_word` extends over adjacent whitespace
    /// (trailing, else leading).
    fn word_span(&self, a_word: bool) -> Option<(usize, usize)> {
        let (row, col) = self.ta.cursor();
        let chars: Vec<char> = self.ta.lines().get(row)?.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let probe = col.min(chars.len() - 1);
        let ws = chars[probe].is_whitespace();
        let mut s = probe;
        while s > 0 && chars[s - 1].is_whitespace() == ws {
            s -= 1;
        }
        let mut e = probe;
        while e + 1 < chars.len() && chars[e + 1].is_whitespace() == ws {
            e += 1;
        }
        let e = e + 1; // exclusive
        if a_word && !ws {
            let mut ne = e;
            while ne < chars.len() && chars[ne].is_whitespace() {
                ne += 1;
            }
            if ne > e {
                return Some((s, ne));
            }
            let mut ns = s;
            while ns > 0 && chars[ns - 1].is_whitespace() {
                ns -= 1;
            }
            if ns < s {
                return Some((ns, e));
            }
        }
        Some((s, e))
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
    fn visual_mode_yank_and_delete() {
        let mut e = editor("one two three");
        for _ in 0..4 {
            e.handle_key(key('l'));
        } // cursor at 't' of two (col 4)
        e.handle_key(key('v'));
        assert_eq!(e.mode, Mode::Visual);
        e.handle_key(key('e')); // extend to end of "two"
        e.handle_key(key('y'));
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.text(), "one two three"); // yank only

        // visual y then p pastes the yanked text below
        e.handle_key(key('$'));
        e.handle_key(key('v'));
        e.handle_key(key('b'));
        e.handle_key(key('y'));
        e.handle_key(key('p'));
        let pasted = e.text().lines().nth(1).unwrap_or("").to_string();
        assert!(
            pasted.len() >= 3 && "three".contains(&pasted),
            "pasted {pasted:?} from {}",
            e.text()
        );

        // visual d deletes the selection (cursor sits on the pasted line)
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(key('d'));
        assert_eq!(e.text().lines().nth(1), Some("hree"), "{}", e.text());
        assert!(e.text().starts_with("one two"));
    }

    #[test]
    fn visual_esc_cancels() {
        let mut e = editor("abcdef");
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(esc());
        assert_eq!(e.mode, Mode::Normal);
        // x deletes the char under the cursor (col 1 after the motion)
        e.handle_key(key('x'));
        assert_eq!(e.text(), "acdef");
    }

    fn typed(text: &str, keys: &str) -> (Editor, String) {
        let mut e = editor(text);
        for c in keys.chars() {
            e.handle_key(key(c));
        }
        let t = e.text();
        (e, t)
    }

    #[test]
    fn word_object_ops() {
        // diw from the middle of a word
        let (_, t) = typed("one two three", "lllllldiw");
        assert_eq!(t, "one  three");
        // diw from the word start
        let (_, t) = typed("one two three", "llllldiw");
        assert_eq!(t, "one  three");
        // diw on a whitespace run selects the blanks
        let (_, t) = typed("a   b", "ldiw");
        assert_eq!(t, "ab");
        // ciw deletes and enters insert
        let (e, t) = typed("one two three", "lllllciw");
        assert_eq!(t, "one  three");
        assert_eq!(e.mode, Mode::Insert);
        // daw eats trailing whitespace
        let (_, t) = typed("one two three", "daw");
        assert_eq!(t, "two three");
        // daw with no trailing ws falls back to leading ws
        let (_, t) = typed("one two", "lllldaw");
        assert_eq!(t, "one");
    }

    #[test]
    fn change_word_ops() {
        // cw == ce: delete to end of word, insert
        let (e, t) = typed("hello world", "cw");
        assert_eq!(t, " world");
        assert_eq!(e.mode, Mode::Insert);
        // cw from the middle of a word changes to its end
        let (_, t) = typed("hello world", "llcw");
        assert_eq!(t, "he world");
        // ce from the middle
        let (_, t) = typed("hello world", "llce");
        assert_eq!(t, "he world");
        // de deletes to end of word
        let (_, t) = typed("hello world", "de");
        assert_eq!(t, " world");
        // de from whitespace eats blanks + the word (inclusive motion)
        let (_, t) = typed("a   b", "lde");
        assert_eq!(t, "a");
        // cb changes back to word start (cursor on 'o', col 4)
        let (e, t) = typed("hello world", "llllcb");
        assert_eq!(t, "o world");
        assert_eq!(e.mode, Mode::Insert);
    }

    #[test]
    fn line_and_range_ops() {
        // d0 / c0 (cursor at col 3; the char under the cursor stays)
        let (_, t) = typed("hello", "llld0");
        assert_eq!(t, "lo");
        let (e, t) = typed("hello", "lllc0");
        assert_eq!(t, "lo");
        assert_eq!(e.mode, Mode::Insert);
        // d$ / c$ / C
        let (_, t) = typed("hello world", "lld$");
        assert_eq!(t, "he");
        let (e, t) = typed("hello world", "llc$");
        assert_eq!(t, "he");
        assert_eq!(e.mode, Mode::Insert);
        let (e, t) = typed("hello world", "llC");
        assert_eq!(t, "he");
        assert_eq!(e.mode, Mode::Insert);
        // cc clears the line in place
        let (e, t) = typed("one\ntwo\nthree", "jcc");
        assert_eq!(t, "one\n\nthree");
        assert_eq!(e.mode, Mode::Insert);
        // dw on the last word must not eat the newline (cursor on 'b')
        let (_, t) = typed("a b\nc", "lldw");
        assert_eq!(t, "a \nc");
    }

    #[test]
    fn buffer_range_ops() {
        // ggdG wipes the document
        let (_, t) = typed("one\ntwo\nthree", "ggdG");
        assert_eq!(t, "");
        // dG from the middle keeps everything before the cursor
        let (_, t) = typed("one\ntwo\nthree", "jdG");
        assert_eq!(t, "one\n");
        // dgg deletes everything before the cursor
        let (_, t) = typed("one\ntwo\nthree", "jlldgg");
        assert_eq!(t, "o\nthree");
        // cG clears forward and inserts
        let (e, t) = typed("one\ntwo", "jcG");
        assert_eq!(t, "one\n");
        assert_eq!(e.mode, Mode::Insert);
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
