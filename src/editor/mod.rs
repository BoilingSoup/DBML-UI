//! Vim modal editing over the editor Buffer.

pub mod buffer;
pub mod completion;

use crate::editor::buffer::{Buffer, Move as CursorMove};

// Input shims mirroring the crossterm event types the engine was written
// against; the app maps iced keyboard events into these.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // full crossterm-compatible surface, used piecemeal
pub enum KeyCode {
    Backspace,
    Space,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Char(char),
    Esc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KeyModifiers(u8);

#[allow(dead_code)]
impl KeyModifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1);
    pub const CONTROL: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
    #[allow(dead_code)]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn new(control: bool, shift: bool, alt: bool) -> Self {
        Self((control as u8) * 2 + (shift as u8) + (alt as u8) * 4)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyEvent {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }
}

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
    /// `r` awaiting the replacement character
    R,
}

pub struct Editor {
    pub buf: Buffer,
    pub mode: Mode,
    /// Visual-line (V) selection: whole rows, regardless of column.
    pub visual_linewise: bool,
    /// Operator-pending / multi-key sequences in flight.
    pending: Option<Pending>,
    /// Count typed before the current key ([count]motion, [count]operator).
    count: Option<u32>,
    /// Count captured when the operator key itself was pressed; multiplies
    /// with `count` (vim: 2d3w == 6w).
    op_count: Option<u32>,
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
            .field("cursor", &self.buf.cursor())
            .field("lines", &self.buf.lines().len())
            .field("cmdline", &self.cmdline)
            .finish()
    }
}

#[allow(dead_code)] // full API kept for callers/tests
impl Editor {
    pub fn new(source: &str) -> Self {
        Self {
            buf: Buffer::from_lines(source.lines().map(|l| l.to_string())),
            mode: Mode::Normal,
            visual_linewise: false,
            pending: None,
            count: None,
            op_count: None,
            cmdline: None,
            register: Vec::new(),
            trailing_newline: source.ends_with('\n'),
        }
    }

    pub fn text(&self) -> String {
        let mut t = self.buf.lines().join("\n");
        if self.trailing_newline && !t.is_empty() {
            t.push('\n');
        }
        t
    }

    pub fn set_text(&mut self, source: &str) {
        self.buf = Buffer::from_lines(source.lines().map(|l| l.to_string()));
        self.trailing_newline = source.ends_with('\n');
    }

    pub fn cursor(&self) -> (usize, usize) {
        self.buf.cursor()
    }

    pub fn mode_str(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
        }
    }

    /// Vim-style showcmd: the in-progress count and pending operator, e.g.
    /// "2d3".
    pub fn showcmd(&self) -> Option<String> {
        let mut s = String::new();
        if let Some(c) = self.count {
            s.push_str(&c.to_string());
        }
        let p = match self.pending {
            Some(Pending::G) => "g",
            Some(Pending::D) => "d",
            Some(Pending::C) => "c",
            Some(Pending::Y) => "y",
            Some(Pending::Di) => "di",
            Some(Pending::Ci) => "ci",
            Some(Pending::Da) => "da",
            Some(Pending::Ca) => "ca",
            Some(Pending::Dg) => "dg",
            Some(Pending::Cg) => "cg",
            Some(Pending::R) => "r",
            None => "",
        };
        s.push_str(p);
        if s.is_empty() {
            None
        } else {
            Some(s)
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
        if self.buf.set_search_pattern(pattern) {
            self.buf.search_forward(true);
        }
    }

    fn handle_insert_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                self.buf.move_cursor(CursorMove::Back);
                Action::None
            }
            KeyCode::Char(c) => {
                self.buf.insert_char(c);
                Action::Changed
            }
            KeyCode::Enter => {
                self.buf.insert_newline();
                Action::Changed
            }
            KeyCode::Backspace => {
                self.buf.delete_char();
                Action::Changed
            }
            KeyCode::Delete => {
                self.buf.delete_next_char();
                Action::Changed
            }
            KeyCode::Left => {
                self.buf.move_cursor(CursorMove::Back);
                Action::None
            }
            KeyCode::Right => {
                self.buf.move_cursor(CursorMove::Forward);
                Action::None
            }
            KeyCode::Up => {
                self.buf.move_cursor(CursorMove::Up);
                Action::None
            }
            KeyCode::Down => {
                self.buf.move_cursor(CursorMove::Down);
                Action::None
            }
            KeyCode::Home => {
                self.buf.move_cursor(CursorMove::Head);
                Action::None
            }
            KeyCode::End => {
                self.buf.move_cursor(CursorMove::End);
                Action::None
            }
            // PageUp/PageDown are handled by the app (animated, page-size
            // aware) before keys reach here
            KeyCode::Tab => Action::None, // focus switching handled by app
            _ => Action::None,
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Action {
        if self.mode == Mode::Visual {
            return self.handle_visual_key(key);
        }
        if self.accumulate_count(key.code) {
            return Action::None;
        }
        if let Some(p) = self.pending.take() {
            let action = self.handle_sequence(p, key.code);
            if self.pending.is_none() {
                self.count = None;
                self.op_count = None;
            }
            return action;
        }

        let m = key.modifiers;
        if let Some(action) = self.apply_motion(key.code) {
            self.count = None;
            return action;
        }
        let action = self.dispatch_normal(key, m);
        if self.pending.is_none() {
            self.count = None;
            self.op_count = None;
        }
        action
    }

    /// Digits accumulate a count; a leading `0` stays the head-of-line
    /// motion. Returns true when the key was consumed as a count digit.
    fn accumulate_count(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char(d @ '1'..='9') => {
                let v = (d as u8 - b'0') as u32;
                self.count = Some((self.count.unwrap_or(0) * 10 + v).min(999_999));
                true
            }
            KeyCode::Char('0') if self.count.is_some() => {
                self.count = Some(self.count.unwrap_or(0) * 10);
                true
            }
            _ => false,
        }
    }

    fn dispatch_normal(&mut self, key: KeyEvent, m: KeyModifiers) -> Action {
        match key.code {
            KeyCode::Delete => self.x_delete(),
            KeyCode::Char('r') if m.contains(KeyModifiers::CONTROL) => {
                let n = self.count.take().unwrap_or(1).max(1);
                for _ in 0..n {
                    self.buf.redo();
                }
                Action::Changed
            }
            KeyCode::Char('g') => {
                self.op_count = self.count.take();
                self.pending = Some(Pending::G);
                Action::None
            }
            KeyCode::Char('v') => {
                self.mode = Mode::Visual;
                self.visual_linewise = false;
                self.buf.start_selection();
                Action::None
            }
            KeyCode::Char('V') => {
                self.mode = Mode::Visual;
                self.visual_linewise = true;
                self.buf.start_selection();
                Action::None
            }
            KeyCode::Char('i') => {
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('a') => {
                self.buf.move_cursor(CursorMove::Forward);
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('I') => {
                self.buf.move_cursor(CursorMove::Head);
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('A') => {
                self.buf.move_cursor(CursorMove::End);
                self.mode = Mode::Insert;
                Action::None
            }
            KeyCode::Char('o') => {
                self.buf.move_cursor(CursorMove::End);
                self.buf.insert_newline();
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('O') => {
                self.buf.move_cursor(CursorMove::Head);
                self.buf.insert_newline();
                self.buf.move_cursor(CursorMove::Up);
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('x') => self.x_delete(),
            KeyCode::Char('D') => {
                self.buf.delete_line_by_end();
                Action::Changed
            }
            KeyCode::Char('C') => {
                self.buf.delete_line_by_end();
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('c') => {
                self.op_count = self.count.take();
                self.pending = Some(Pending::C);
                Action::None
            }
            KeyCode::Char('d') => {
                self.op_count = self.count.take();
                self.pending = Some(Pending::D);
                Action::None
            }
            KeyCode::Char('y') => {
                self.op_count = self.count.take();
                self.pending = Some(Pending::Y);
                Action::None
            }
            KeyCode::Char('r') => {
                self.op_count = self.count.take();
                self.pending = Some(Pending::R);
                Action::None
            }
            KeyCode::Char('p') => {
                let n = self.count.take().unwrap_or(1).max(1);
                self.paste_n(false, n)
            }
            KeyCode::Char('P') => {
                let n = self.count.take().unwrap_or(1).max(1);
                self.paste_n(true, n)
            }
            KeyCode::Char('u') => {
                let n = self.count.take().unwrap_or(1).max(1);
                for _ in 0..n {
                    self.buf.undo();
                }
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
                self.buf.search_forward(false);
                Action::None
            }
            KeyCode::Char('N') => {
                self.buf.search_back(false);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_visual_key(&mut self, key: KeyEvent) -> Action {
        if self.accumulate_count(key.code) {
            return Action::None;
        }
        let action = self.dispatch_visual(key);
        if self.pending.is_none() {
            self.count = None;
        }
        action
    }

    fn dispatch_visual(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                self.visual_linewise = false;
                Action::None
            }
            // v/V switch selection kinds; same-kind toggles off (vim)
            KeyCode::Char('v') if self.visual_linewise => {
                self.visual_linewise = false;
                Action::None
            }
            KeyCode::Char('V') if !self.visual_linewise => {
                self.visual_linewise = true;
                Action::None
            }
            KeyCode::Char('V') => {
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                self.visual_linewise = false;
                Action::None
            }
            KeyCode::Char('y') if self.visual_linewise => {
                if let Some((from, n)) = self.selection_lines() {
                    self.buf.move_cursor(CursorMove::Jump(from, 0));
                    self.yank_lines(n);
                }
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Char('y') => {
                if let Some(t) = self.buf.copy_selection() {
                    self.register = t.split('\n').map(str::to_string).collect();
                }
                // copy() leaves selection_start set, which would make later
                // normal-mode motions extend an invisible selection
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Char('d') | KeyCode::Char('x') if self.visual_linewise => {
                // cancel FIRST: delete_lines -> delete_str would otherwise
                // consume the still-active selection instead
                let lines = self.selection_lines();
                self.buf.cancel_selection();
                if let Some((from, n)) = lines {
                    self.buf.move_cursor(CursorMove::Jump(from, 0));
                    self.yank_lines(n);
                    self.delete_lines(n);
                }
                self.mode = Mode::Normal;
                self.visual_linewise = false;
                Action::Changed
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                if let Some(t) = self.buf.cut_selection() {
                    self.register = t.split('\n').map(str::to_string).collect();
                }
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                Action::Changed
            }
            KeyCode::Char('o') => {
                // swap selection ends (vim vo); approximate by cancelling and
                // restarting from the current cursor
                self.buf.cancel_selection();
                self.buf.start_selection();
                Action::None
            }
            // operators conclude visual mode (vim): after u/U the session
            // returns to normal so motions extend nothing
            KeyCode::Char('u') if self.visual_linewise => {
                if let Some((from, n)) = self.selection_lines() {
                    if self.buf.apply_lines_case(from, n, false) {
                        self.buf.cancel_selection();
                        self.mode = Mode::Normal;
                        self.visual_linewise = false;
                        return Action::Changed;
                    }
                }
                Action::None
            }
            KeyCode::Char('u') => {
                if self.buf.apply_selection_case(false) {
                    self.buf.cancel_selection();
                    self.mode = Mode::Normal;
                    Action::Changed
                } else {
                    Action::None
                }
            }
            KeyCode::Char('U') if self.visual_linewise => {
                if let Some((from, n)) = self.selection_lines() {
                    if self.buf.apply_lines_case(from, n, true) {
                        self.buf.cancel_selection();
                        self.mode = Mode::Normal;
                        self.visual_linewise = false;
                        return Action::Changed;
                    }
                }
                Action::None
            }
            KeyCode::Char('U') => {
                if self.buf.apply_selection_case(true) {
                    self.buf.cancel_selection();
                    self.mode = Mode::Normal;
                    Action::Changed
                } else {
                    Action::None
                }
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
        let count = self.count.take();
        let n = count.unwrap_or(1).max(1) as usize;
        match code {
            // count is an absolute line number for G
            KeyCode::Char('G') => match count {
                Some(line) => {
                    let rows = self.buf.lines().len().max(1);
                    let row = (line as usize).saturating_sub(1).min(rows - 1);
                    self.buf.move_cursor(CursorMove::Jump(row, 0));
                }
                None => self.buf.move_cursor(CursorMove::Bottom),
            },
            // n$ moves to the end of the (n-1)th line below
            KeyCode::Char('$') | KeyCode::End => {
                for _ in 1..n {
                    self.buf.move_cursor(CursorMove::Down);
                }
                self.buf.move_cursor(CursorMove::End);
            }
            // counts don't repeat line-head motions
            KeyCode::Char('0') | KeyCode::Home | KeyCode::Char('^') => {
                self.buf.move_cursor(CursorMove::Head);
            }
            KeyCode::Char('h') | KeyCode::Left => self.repeat(CursorMove::Back, n),
            KeyCode::Char('l') | KeyCode::Right => self.repeat(CursorMove::Forward, n),
            KeyCode::Char('j') | KeyCode::Down => self.repeat(CursorMove::Down, n),
            KeyCode::Char('k') | KeyCode::Up => self.repeat(CursorMove::Up, n),
            KeyCode::Char('w') => self.repeat(CursorMove::WordForward, n),
            KeyCode::Char('b') => self.repeat(CursorMove::WordBack, n),
            KeyCode::Char('e') => self.repeat(CursorMove::WordEnd, n),
            KeyCode::Char('{') => self.repeat(CursorMove::ParagraphBack, n),
            KeyCode::Char('}') => self.repeat(CursorMove::ParagraphForward, n),
            _ => {
                self.count = count; // not a motion; keep the count
                return None;
            }
        }
        Some(Action::None)
    }

    fn repeat(&mut self, m: CursorMove, n: usize) {
        for _ in 0..n {
            self.buf.move_cursor(m);
        }
    }

    fn handle_sequence(&mut self, leader: Pending, code: KeyCode) -> Action {
        use Pending::*;
        // vim multiplies leading and trailing counts: 2d3w == 6w
        let rep = (self.op_count.unwrap_or(1).max(1) as usize)
            .saturating_mul(self.count.unwrap_or(1).max(1) as usize);
        match (leader, code) {
            (R, KeyCode::Esc) => Action::None,
            (R, KeyCode::Enter) => {
                if self.buf.replace_with_newline() {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            (R, KeyCode::Char(c)) => {
                if self.buf.replace_next(rep.max(1), c) {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            (R, _) => Action::None,
            (G, KeyCode::Char('g')) => {
                // [count]gg jumps to line N
                if let Some(line) = self.count.or(self.op_count) {
                    let rows = self.buf.lines().len().max(1);
                    let row = (line as usize).saturating_sub(1).min(rows - 1);
                    self.buf.move_cursor(CursorMove::Jump(row, 0));
                } else {
                    self.buf.move_cursor(CursorMove::Jump(0, 0));
                }
                Action::None
            }
            (Y, KeyCode::Char('y')) => {
                self.yank_lines(rep);
                Action::None
            }
            (D, KeyCode::Char('d')) => {
                self.yank_lines(rep);
                self.delete_lines(rep);
                Action::Changed
            }
            (C, KeyCode::Char('c')) => {
                // [count]cc deletes the lines and edits one empty line
                self.yank_lines(rep);
                self.delete_lines(rep);
                self.buf.insert_newline();
                self.buf.move_cursor(CursorMove::Up);
                self.mode = Mode::Insert;
                Action::Changed
            }
            // operator + motions; cw follows vim's cw == ce special case
            (D, KeyCode::Char('w')) => {
                self.operate(&vec![CursorMove::WordForward; rep], false, true, false)
            }
            (C, KeyCode::Char('w')) => {
                self.operate(&vec![CursorMove::WordEnd; rep], true, true, true)
            }
            (D, KeyCode::Char('e')) => {
                self.operate(&vec![CursorMove::WordEnd; rep], true, false, false)
            }
            (C, KeyCode::Char('e')) => {
                self.operate(&vec![CursorMove::WordEnd; rep], true, false, true)
            }
            (D, KeyCode::Char('b')) => {
                self.operate(&vec![CursorMove::WordBack; rep], false, false, false)
            }
            (C, KeyCode::Char('b')) => {
                self.operate(&vec![CursorMove::WordBack; rep], false, false, true)
            }
            (D, KeyCode::Char('0') | KeyCode::Home | KeyCode::Char('^')) => {
                self.operate(&[CursorMove::Head], false, false, false)
            }
            (C, KeyCode::Char('0') | KeyCode::Home | KeyCode::Char('^')) => {
                self.operate(&[CursorMove::Head], false, false, true)
            }
            // n$ operates to the end of the (n-1)th line below; the End
            // position already sits past the last char, so this is exclusive
            (D, KeyCode::Char('$') | KeyCode::End) => {
                let mut ms = vec![CursorMove::Down; rep.saturating_sub(1)];
                ms.push(CursorMove::End);
                self.operate(&ms, false, false, false)
            }
            (C, KeyCode::Char('$') | KeyCode::End) => {
                let mut ms = vec![CursorMove::Down; rep.saturating_sub(1)];
                ms.push(CursorMove::End);
                self.operate(&ms, false, false, true)
            }
            // [count]G targets an absolute line, linewise (like vim)
            (D, KeyCode::Char('G')) => {
                let target = self.line_target_row(false);
                self.operate_lines(target, false)
            }
            (C, KeyCode::Char('G')) => {
                let target = self.line_target_row(false);
                self.operate_lines(target, true)
            }
            (D, KeyCode::Char('{')) => {
                self.operate(&vec![CursorMove::ParagraphBack; rep], false, false, false)
            }
            (C, KeyCode::Char('{')) => {
                self.operate(&vec![CursorMove::ParagraphBack; rep], false, false, true)
            }
            (D, KeyCode::Char('}')) => self.operate(
                &vec![CursorMove::ParagraphForward; rep],
                false,
                false,
                false,
            ),
            (C, KeyCode::Char('}')) => {
                self.operate(&vec![CursorMove::ParagraphForward; rep], false, false, true)
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
                let target = self.line_target_row(true);
                self.operate_lines(target, false)
            }
            (Cg, KeyCode::Char('g')) => {
                let target = self.line_target_row(true);
                self.operate_lines(target, true)
            }
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

    /// Target row for G/gg operators: `[count]` is an absolute line number
    /// (1-based); without one, G means the last row and gg the first.
    fn line_target_row(&self, from_gg: bool) -> usize {
        if let Some(line) = self.count.or(self.op_count) {
            let rows = self.buf.lines().len().max(1);
            (line as usize).saturating_sub(1).min(rows - 1)
        } else if from_gg {
            0
        } else {
            self.buf.lines().len().saturating_sub(1)
        }
    }

    /// Linewise d/c to a target row (for G/gg): whole lines from the cursor
    /// row through the target row.
    fn operate_lines(&mut self, target: usize, insert: bool) -> Action {
        let (row, _) = self.buf.cursor();
        let from = row.min(target);
        let n = row.abs_diff(target) + 1;
        self.buf.move_cursor(CursorMove::Jump(from, 0));
        self.yank_lines(n);
        self.delete_lines(n);
        if insert {
            self.buf.insert_newline();
            self.buf.move_cursor(CursorMove::Up);
            self.mode = Mode::Insert;
        }
        Action::Changed
    }

    /// Rows covered by a visual-line selection: (first_row, count),
    /// anchor row through cursor row inclusive.
    fn selection_lines(&self) -> Option<(usize, usize)> {
        let anchor = self.buf.selection_anchor()?;
        let cursor = self.buf.pos();
        let from = anchor.row.min(cursor.row);
        let to = anchor.row.max(cursor.row);
        Some((from, to - from + 1))
    }

    /// `x`: delete up to [count] chars, never crossing the line end.
    fn x_delete(&mut self) -> Action {
        let n = self.count.take().unwrap_or(1).max(1) as usize;
        let (row, col) = self.buf.cursor();
        let remain = self
            .buf
            .lines()
            .get(row)
            .map(|l| l.chars().count().saturating_sub(col))
            .unwrap_or(0);
        let n = n.min(remain);
        if n > 0 {
            self.buf.delete_str(n);
            Action::Changed
        } else {
            Action::None
        }
    }

    /// Yank `n` lines from the cursor row into the register.
    fn yank_lines(&mut self, n: usize) {
        let (row, _) = self.buf.cursor();
        let lines = self.buf.lines();
        let end = (row + n.max(1)).min(lines.len());
        self.register = lines[row..end].to_vec();
    }

    /// Delete `n` lines starting at the cursor row.
    fn delete_lines(&mut self, n: usize) {
        let (row, _) = self.buf.cursor();
        let lines = self.buf.lines();
        let end = (row + n.max(1)).min(lines.len());
        let total: usize = lines[row..end].iter().map(|l| l.chars().count() + 1).sum();
        self.buf.move_cursor(CursorMove::Jump(row, 0));
        self.buf.delete_str(total);
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
        let start = self.buf.cursor();
        for m in motions {
            self.buf.move_cursor(*m);
        }
        let mut end = self.buf.cursor();
        if clamp_line && end.0 != start.0 {
            self.buf.move_cursor(CursorMove::Jump(start.0, 0));
            self.buf.move_cursor(CursorMove::End);
            end = self.buf.cursor();
        }
        let forward = end > start;
        let (from, to) = if forward { (start, end) } else { (end, start) };
        let mut n = self.chars_between(from, to);
        if inclusive && forward {
            n += 1;
        }
        if n > 0 {
            self.buf.move_cursor(CursorMove::Jump(from.0, from.1));
            self.buf.delete_str(n);
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
        let lines = self.buf.lines();
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
        let (row, _) = self.buf.cursor();
        self.buf.move_cursor(CursorMove::Jump(row, s));
        self.buf.delete_str(e - s);
        if insert {
            self.mode = Mode::Insert;
        }
        Action::Changed
    }

    /// Char-index range of the word at the cursor: the contiguous run with
    /// the same whitespace-ness. `a_word` extends over adjacent whitespace
    /// (trailing, else leading).
    fn word_span(&self, a_word: bool) -> Option<(usize, usize)> {
        let (row, col) = self.buf.cursor();
        let chars: Vec<char> = self.buf.lines().get(row)?.chars().collect();
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

    fn paste_n(&mut self, before: bool, times: u32) -> Action {
        if self.register.is_empty() {
            return Action::None;
        }
        let (row, _) = self.buf.cursor();
        if before {
            self.buf.move_cursor(CursorMove::Jump(row, 0));
        } else {
            self.buf.move_cursor(CursorMove::Jump(row, usize::MAX));
            self.buf.move_cursor(CursorMove::End);
        }
        self.buf.insert_newline();
        for c in 0..times {
            if c > 0 {
                self.buf.insert_newline();
            }
            for (i, line) in self.register.iter().enumerate() {
                if i > 0 {
                    self.buf.insert_newline();
                }
                if !line.is_empty() {
                    self.buf.insert_str(line);
                }
            }
        }
        Action::Changed
    }

    /// Move the cursor to a (row, col) in buffer coordinates (mouse click).
    pub fn jump(&mut self, row: usize, col: usize) {
        let rows = self.buf.lines().len();
        let r = row.min(rows.saturating_sub(1));
        let line_len = self
            .buf
            .lines()
            .get(r)
            .map(|l| l.chars().count())
            .unwrap_or(0);
        let c = col.min(line_len);
        self.buf.move_cursor(CursorMove::Jump(r, c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    pub(super) fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    }

    fn enter() -> KeyEvent {
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
    }

    pub(super) fn editor(text: &str) -> Editor {
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
        // $ lands ON the last char, so $vb yanks "three" inclusive
        assert_eq!(pasted, "three", "from {}", e.text());

        // visual d deletes the selection (cursor sits on the pasted line)
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(key('d'));
        // vim v+l selects TWO chars (inclusive of the cursor cell): "th"
        assert_eq!(e.text().lines().nth(1), Some("ree"), "{}", e.text());
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
        // dgg deletes whole lines up to and including the cursor line
        let (_, t) = typed("one\ntwo\nthree", "jdgg");
        assert_eq!(t, "three");
        // cG clears forward and leaves an empty line to edit
        let (e, t) = typed("one\ntwo", "jcG");
        assert_eq!(t, "one\n\n");
        assert_eq!(e.mode, Mode::Insert);
    }

    #[test]
    fn counts_with_motions() {
        // 3j
        let mut e = editor("a\nb\nc\nd");
        e.handle_key(key('3'));
        e.handle_key(key('j'));
        assert_eq!(e.cursor().0, 3);
        // 2w
        let mut e = editor("one two three");
        e.handle_key(key('2'));
        e.handle_key(key('w'));
        assert_eq!(e.cursor(), (0, 8));
        // 10l clamps at EOL
        let mut e = editor("hello");
        e.handle_key(key('1'));
        e.handle_key(key('0'));
        e.handle_key(key('l'));
        assert_eq!(e.cursor(), (0, 5));
        // 2$ goes to the end of the next line
        let mut e = editor("hello\nworld!");
        e.handle_key(key('2'));
        e.handle_key(key('$'));
        assert_eq!(e.cursor(), (1, 6));
        // 3gg / 4G absolute line jumps (clamped)
        let mut e = editor("a\nb\nc\nd\ne");
        e.handle_key(key('3'));
        e.handle_key(key('g'));
        e.handle_key(key('g'));
        assert_eq!(e.cursor().0, 2);
        e.handle_key(key('4'));
        e.handle_key(key('G'));
        assert_eq!(e.cursor().0, 3);
        e.handle_key(key('9'));
        e.handle_key(key('G'));
        assert_eq!(e.cursor().0, 4, "clamped to last line");
        // count does not leak into the next command
        e.handle_key(key('j'));
        assert_eq!(e.cursor().0, 4, "plain j moves one");
    }

    #[test]
    fn counts_with_operators() {
        // d3w deletes three words
        let (_, t) = typed("one two three four", "d3w");
        assert_eq!(t, "four");
        // 2dd deletes two lines
        let (_, t) = typed("a\nb\nc", "2dd");
        assert_eq!(t, "c");
        // 2d2w == d4w (counts multiply)
        let (_, t) = typed("one two three four five", "2d2w");
        assert_eq!(t, "five");
        // 3x deletes three chars as one undo unit
        let mut e = editor("abcdef");
        e.handle_key(key('3'));
        e.handle_key(key('x'));
        assert_eq!(e.text(), "def");
        e.handle_key(key('u'));
        assert_eq!(e.text(), "abcdef");
        // 3x clamps at EOL
        let (_, t) = typed("ab", "5x");
        assert_eq!(t, "");
        // d2$ deletes to end of next line
        let (_, t) = typed("hello\nworld\n!", "ld2$");
        assert_eq!(t, "h\n!");
        // 2yy then 3p pastes twice, thrice
        let mut e = editor("x\ny");
        e.handle_key(key('2'));
        e.handle_key(key('y'));
        e.handle_key(key('y'));
        e.handle_key(key('G'));
        e.handle_key(key('3'));
        e.handle_key(key('p'));
        assert_eq!(e.text(), "x\ny\nx\ny\nx\ny\nx\ny");
        // d2G from line 4 deletes lines 2-4
        let (_, t) = typed("l1\nl2\nl3\nl4\nl5", "3jd2G");
        assert_eq!(t, "l1\nl5");
        // 3cc clears 3 lines into one editable empty line
        let (e, t) = typed("a\nb\nc\nd", "3cc");
        assert_eq!(t, "\nd");
        assert_eq!(e.mode, Mode::Insert);
    }

    #[test]
    fn zero_is_motion_until_count_started() {
        // leading 0 = head of line (cursor at col 3)
        let (_, t) = typed("hello", "llld0");
        assert_eq!(t, "lo");
        // 0 after a count is a digit: 10l
        let mut e = editor("0123456789abc");
        e.handle_key(key('1'));
        e.handle_key(key('0'));
        e.handle_key(key('l'));
        assert_eq!(e.cursor(), (0, 10));
        // d0 keeps working with a fresh count: 2d0 deletes [head, cursor)
        let (_, t) = typed("hello", "lll2d0");
        assert_eq!(t, "lo", "count before d0 is ignored, not typed");
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

#[cfg(test)]
mod polish_tests {
    use super::tests::editor;
    use super::*;

    #[test]
    fn r_replaces_char_under_cursor() {
        let mut e = editor("hello\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(e.text(), "xello\n");
        // single undo unit
        e.buf.undo();
        assert_eq!(e.text(), "hello\n");
    }

    #[test]
    fn r_with_count_replaces_run_and_aborts_past_eol() {
        let mut e = editor("hello\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
        assert_eq!(e.text(), "!!!lo\n");
        // 10r? past end of line: vim aborts with no change
        e.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        assert_eq!(e.text(), "!!!lo\n");
    }

    #[test]
    fn esc_cancels_pending_r() {
        let mut e = editor("abc\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert_eq!(e.showcmd().as_deref(), Some("r"));
        e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.showcmd(), None);
        e.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(e.text(), "bc\n");
    }

    #[test]
    fn visual_u_lowercases_and_concludes() {
        let mut e = editor("Hello World\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        assert_eq!(e.text(), "hello World\n");
        assert_eq!(e.mode, Mode::Normal, "operator concludes visual");
        assert!(e.buf.selection_range().is_none(), "no lingering selection");
        // motions after the op extend nothing
        e.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE));
        assert!(e.buf.selection_range().is_none());
        // one undo unit restores both cases
        e.buf.undo();
        assert_eq!(e.text(), "Hello World\n");
    }

    #[test]
    fn visual_u_uppercase_multiline() {
        let mut e = editor("abc\ndef\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE));
        // extend to end of second line
        e.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
        e.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::NONE));
        assert_eq!(e.text(), "ABC\nDEF\n");
        assert_eq!(e.mode, Mode::Normal, "U concludes visual");
        assert!(e.buf.selection_range().is_none());
    }

    #[test]
    fn insert_mode_arrow_and_delete_keys() {
        let mut e = editor("ab\ncd\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(e.text(), "abcd\n", "Delete at EOL joins (GUI behavior)");
    }
}

#[cfg(test)]
mod vline_tests {
    use super::tests::{editor, key};
    use super::*;

    #[test]
    fn capital_v_selects_lines_and_d_deletes_them() {
        let mut e = editor("one\ntwo\nthree\n");
        e.handle_key(key('V'));
        assert_eq!(e.mode, Mode::Visual);
        assert!(e.visual_linewise, "linewise");
        e.handle_key(key('j'));
        e.handle_key(key('d'));
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.text(), "three\n");
        assert_eq!(e.register, vec!["one".to_string(), "two".to_string()]);
        // linewise p pastes below
        e.handle_key(key('p'));
        assert_eq!(e.text(), "three\none\ntwo\n");
    }

    #[test]
    fn v_line_y_yanks_whole_lines() {
        let mut e = editor("aaa\nbbb\nccc\n");
        e.handle_key(key('j')); // row 1
        e.handle_key(key('V'));
        e.handle_key(key('j')); // extend to row 2
        e.handle_key(key('y'));
        assert_eq!(e.text(), "aaa\nbbb\nccc\n", "yank only");
        assert_eq!(e.register, vec!["bbb".to_string(), "ccc".to_string()]);
    }

    #[test]
    fn v_line_case_ops_conclude_visual() {
        let mut e = editor("abc\ndef\nghi\n");
        e.handle_key(key('V'));
        e.handle_key(key('j'));
        e.handle_key(KeyEvent::new(KeyCode::Char('U'), KeyModifiers::SHIFT));
        assert_eq!(e.text(), "ABC\nDEF\nghi\n");
        assert_eq!(e.mode, Mode::Normal, "operator concludes visual");
        assert!(e.buf.selection_range().is_none(), "no lingering selection");
        // re-select the two rows and lowercase them again
        e.handle_key(key('V'));
        e.handle_key(key('k'));
        e.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        assert_eq!(e.text(), "abc\ndef\nghi\n", "back to lowercase");
        // each re-case is exactly one undo unit
        e.buf.undo();
        assert_eq!(e.text(), "ABC\nDEF\nghi\n", "first undo reverts the u");
        e.buf.undo();
        assert_eq!(e.text(), "abc\ndef\nghi\n", "second undo reverts the U");
    }

    #[test]
    fn v_and_capital_v_switch_kinds_and_toggle_off() {
        let mut e = editor("x\n");
        e.handle_key(key('v'));
        assert!(!e.visual_linewise, "charwise");
        e.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        assert!(e.visual_linewise, "switched to linewise");
        e.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        assert_eq!(e.mode, Mode::Normal, "V in linewise exits visual");
        assert!(!e.visual_linewise);
        // and the flag resets through Esc
        e.handle_key(KeyEvent::new(KeyCode::Char('V'), KeyModifiers::SHIFT));
        assert!(e.visual_linewise);
        e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Normal);
        assert!(!e.visual_linewise, "reset for the next v");
    }

    #[test]
    fn v_line_motions_ignore_columns() {
        let mut e = editor("hello world\nsecond\n");
        e.handle_key(key('V'));
        e.handle_key(key('e')); // word motion within the line
        e.handle_key(key('$')); // end of line
                                // still exactly one row selected
        e.handle_key(key('d'));
        assert_eq!(e.text(), "second\n", "whole first line gone");
    }
}
