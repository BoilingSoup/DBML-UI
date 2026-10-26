//! Vim modal editing over the editor Buffer.

pub mod buffer;
pub mod completion;

use crate::editor::buffer::{Buffer, Move as CursorMove, ObjKind, Pos, TextObj};

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
    /// `d g…` / `c g…` awaiting `g`
    Dg,
    Cg,
    /// `r` awaiting the replacement character
    R,
    /// operator + `i`/`a` awaiting the object char (diw, ci(, ya"…)
    Obj {
        op: ObjOp,
        around: bool,
    },
    /// visual-mode `i`/`a` awaiting the object char (viw, va(…)
    VisualObj {
        around: bool,
    },
    /// vim-surround `ds…` awaiting the target pair
    SurroundDelete,
    /// vim-surround `cs…`: target, then replacement
    SurroundChange {
        target: Option<char>,
    },
    /// vim-surround `ys…`: motion/object, then the pair to add
    SurroundAdd {
        stage: AddStage,
    },
    /// visual-mode `S…` awaiting the pair to wrap the selection in
    SurroundVisual,
    /// f/F/t/T awaiting the target char
    Find {
        forward: bool,
        till: bool,
    },
    /// [cdy]f/F/t/T awaiting the target char
    FindOp {
        op: ObjOp,
        forward: bool,
        till: bool,
    },
}

/// Which operator preceded a text object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjOp {
    Delete,
    Change,
    Yank,
}

/// Stage of an `ys` sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddStage {
    Motion,
    Obj { around: bool },
    Range(buffer::TextObj),
    /// `yss`: the cursor line (count-extended) minus its leading indent,
    /// awaiting the pair
    Line,
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
    /// The most recent YANK (the `y` family: y/yy/yiw/visual y…), queued
    /// for the app to push to the SYSTEM clipboard — the Editor has no
    /// arboard access, so the app drains this after every key
    /// (`take_system_yank`). Deletes fill the register but never queue a
    /// system copy (vim's unnamed-register semantics).
    system_yank: Option<String>,
    /// Last f/F/t/T (forward, till, char) for `;` and `,`
    last_find: Option<(bool, bool, char)>,
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
            system_yank: None,
            last_find: None,
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

    /// Replace the yank register (used by Regular-mode copy/cut).
    pub fn set_register(&mut self, lines: Vec<String>) {
        self.register = lines;
    }

    /// Snapshot of the yank register (used by Regular-mode paste).
    pub fn register_lines(&self) -> Vec<String> {
        self.register.clone()
    }

    /// The queued system-clipboard copy of the last vim yank, if any —
    /// drained by the app after each key (see the `system_yank` field).
    pub fn take_system_yank(&mut self) -> Option<String> {
        self.system_yank.take()
    }

    /// Charwise yank: the text into the register AND queued for the
    /// system clipboard.
    fn yank_chars(&mut self, text: String) {
        self.register = text.split('\n').map(str::to_string).collect();
        self.system_yank = Some(text);
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
            Some(Pending::Dg) => "dg",
            Some(Pending::Cg) => "cg",
            Some(Pending::R) => "r",
            Some(Pending::Obj {
                op: ObjOp::Delete,
                around: false,
            }) => "di",
            Some(Pending::Obj {
                op: ObjOp::Change,
                around: false,
            }) => "ci",
            Some(Pending::Obj {
                op: ObjOp::Yank,
                around: false,
            }) => "yi",
            Some(Pending::Obj {
                op: ObjOp::Delete,
                around: true,
            }) => "da",
            Some(Pending::Obj {
                op: ObjOp::Change,
                around: true,
            }) => "ca",
            Some(Pending::Obj {
                op: ObjOp::Yank,
                around: true,
            }) => "ya",
            Some(Pending::VisualObj { around: false }) => "vi",
            Some(Pending::VisualObj { around: true }) => "va",
            Some(Pending::SurroundDelete) => "ds",
            Some(Pending::SurroundChange { .. }) => "cs",
            Some(Pending::SurroundAdd { .. }) => "ys",
            Some(Pending::SurroundVisual) => "S",
            Some(Pending::Find {
                forward: true,
                till: false,
            }) => "f",
            Some(Pending::Find {
                forward: false,
                till: false,
            }) => "F",
            Some(Pending::Find {
                forward: true,
                till: true,
            }) => "t",
            Some(Pending::Find {
                forward: false,
                till: true,
            }) => "T",
            Some(Pending::FindOp {
                op: ObjOp::Delete,
                forward: true,
                till: false,
            }) => "df",
            Some(Pending::FindOp {
                op: ObjOp::Delete,
                forward: false,
                till: false,
            }) => "dF",
            Some(Pending::FindOp {
                op: ObjOp::Delete,
                forward: true,
                till: true,
            }) => "dt",
            Some(Pending::FindOp {
                op: ObjOp::Delete,
                forward: false,
                till: true,
            }) => "dT",
            Some(Pending::FindOp {
                op: ObjOp::Change,
                forward: true,
                till: false,
            }) => "cf",
            Some(Pending::FindOp {
                op: ObjOp::Change,
                forward: false,
                till: false,
            }) => "cF",
            Some(Pending::FindOp {
                op: ObjOp::Change,
                forward: true,
                till: true,
            }) => "ct",
            Some(Pending::FindOp {
                op: ObjOp::Change,
                forward: false,
                till: true,
            }) => "cT",
            Some(Pending::FindOp {
                op: ObjOp::Yank,
                forward: true,
                till: false,
            }) => "yf",
            Some(Pending::FindOp {
                op: ObjOp::Yank,
                forward: false,
                till: false,
            }) => "yF",
            Some(Pending::FindOp {
                op: ObjOp::Yank,
                forward: true,
                till: true,
            }) => "yt",
            Some(Pending::FindOp {
                op: ObjOp::Yank,
                forward: false,
                till: true,
            }) => "yT",
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
        // ctrl/alt chords never type (vim's insert-mode ctrl+d/u/k have no
        // port here, and a flubbed modifier must not insert text)
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || key.modifiers.contains(KeyModifiers::ALT)
        {
            return Action::None;
        }
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Normal;
                // vim: leaving insert backs up one column, but never off
                // the line — at column 0 (e.g. the empty line cc leaves)
                // the cursor stays put; Move::Back would wrap to the END
                // of the line above
                if self.buf.pos().col > 0 {
                    self.buf.move_cursor(CursorMove::Back);
                }
                Action::None
            }
            KeyCode::Char(c) => {
                self.buf.insert_char(c);
                Action::Changed
            }
            KeyCode::Space => {
                self.buf.insert_char(' ');
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
                self.insert_arrow(CursorMove::Back, shift);
                Action::None
            }
            KeyCode::Right => {
                self.insert_arrow(CursorMove::Forward, shift);
                Action::None
            }
            KeyCode::Up => {
                self.insert_arrow(CursorMove::Up, shift);
                Action::None
            }
            KeyCode::Down => {
                self.insert_arrow(CursorMove::Down, shift);
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

    /// An arrow in insert mode: Shift+arrow starts a selection at the
    /// caret (if none) and extends it, like regular mode's notepad
    /// behavior; a plain arrow collapses an active selection and moves.
    fn insert_arrow(&mut self, m: CursorMove, shift: bool) {
        if shift {
            if self.buf.selection_anchor().is_none() {
                self.buf.start_selection();
            }
        } else {
            self.buf.cancel_selection();
        }
        self.buf.move_cursor(m);
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Action {
        // ctrl/alt-modified keys are never vim keys here — the app scrolls
        // on ctrl+d/u itself, and an operator armed from a modified letter
        // turned `ctrl+d … gg` into `dgg` (delete to the top). The lone
        // ctrl chord vim uses in normal mode is Ctrl+r (redo).
        let m = key.modifiers;
        if m.contains(KeyModifiers::ALT)
            || (m.contains(KeyModifiers::CONTROL)
                && !(key.code == KeyCode::Char('r') && self.mode == Mode::Normal))
        {
            return Action::None;
        }
        // pending sequences run in every mode, so viw/S… work from visual
        if let Some(p) = self.pending.take() {
            let action = self.handle_sequence(p, key.code);
            if self.pending.is_none() {
                self.count = None;
                self.op_count = None;
            }
            return action;
        }
        if self.mode == Mode::Visual {
            return self.handle_visual_key(key);
        }
        if self.accumulate_count(key.code) {
            return Action::None;
        }

        let m = key.modifiers;
        // Shift+arrows extend a selection from the cursor (the notepad
        // behavior, vim too): start one if none, then repeat the arrow's
        // motion by the pending count like the plain arrow. A plain arrow
        // collapses an active selection and moves (regular mode's
        // collapse-on-unshifted-move); visual mode handled its own keys
        // above and keeps extending.
        if m.contains(KeyModifiers::SHIFT) {
            if let Some(motion) = Self::arrow_motion(key.code) {
                if self.buf.selection_anchor().is_none() {
                    self.buf.start_selection();
                }
                let n = self.count.take().unwrap_or(1).max(1) as usize;
                self.repeat(motion, n);
                return Action::None;
            }
        } else if Self::arrow_motion(key.code).is_some()
            && self.buf.selection_anchor().is_some()
        {
            self.buf.cancel_selection();
        }
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
            // f/F/t/T line motions await a target char
            KeyCode::Char(c) if matches!(c, 'f' | 'F' | 't' | 'T') => {
                self.pending = Some(Pending::Find {
                    forward: c == 'f' || c == 't',
                    till: c == 't' || c == 'T',
                });
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
                    self.yank_lines(n, true);
                }
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Char('y') => {
                if let Some(t) = self.buf.copy_selection() {
                    self.yank_chars(t);
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
                    self.yank_lines(n, false);
                    self.delete_lines(n);
                }
                self.mode = Mode::Normal;
                self.visual_linewise = false;
                Action::Changed
            }
            // linewise c: replace the rows with one empty line to edit (cc)
            KeyCode::Char('c') if self.visual_linewise => {
                let lines = self.selection_lines();
                self.buf.cancel_selection();
                if let Some((from, n)) = lines {
                    self.buf.move_cursor(CursorMove::Jump(from, 0));
                    self.yank_lines(n, false);
                    self.buf.change_lines(n);
                }
                self.mode = Mode::Insert;
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
            // c (and s, vim's alias): delete the selection and edit
            KeyCode::Char('c') | KeyCode::Char('s') => {
                if let Some(t) = self.buf.cut_selection() {
                    self.register = t.split('\n').map(str::to_string).collect();
                }
                self.buf.cancel_selection();
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('o') => {
                // swap selection ends (vim vo); approximate by cancelling and
                // restarting from the current cursor
                self.buf.cancel_selection();
                self.buf.start_selection();
                Action::None
            }
            // f/F/t/T extend the selection to a line char
            KeyCode::Char(c) if matches!(c, 'f' | 'F' | 't' | 'T') => {
                self.pending = Some(Pending::Find {
                    forward: c == 'f' || c == 't',
                    till: c == 't' || c == 'T',
                });
                Action::None
            }
            // text objects and vim-surround start sequences in visual too
            KeyCode::Char('i') => {
                self.pending = Some(Pending::VisualObj { around: false });
                Action::None
            }
            KeyCode::Char('a') => {
                self.pending = Some(Pending::VisualObj { around: true });
                Action::None
            }
            KeyCode::Char('S') => {
                self.pending = Some(Pending::SurroundVisual);
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
            // ; repeats the last f/F/t/T; , reverses its direction
            KeyCode::Char(';') | KeyCode::Char(',') => {
                if let Some((forward, till, ch)) = self.last_find {
                    let flip = code == KeyCode::Char(',');
                    if let Some(col) = self.buf.find_char_col(forward != flip, till, ch, n) {
                        let row = self.buf.pos().row;
                        self.buf.move_cursor(CursorMove::Jump(row, col));
                    }
                }
            }
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

    /// [cdy]f/F/t/T (or d;…) operator over a find target.
    fn apply_find_op(
        &mut self,
        op: ObjOp,
        forward: bool,
        till: bool,
        ch: char,
        count: usize,
    ) -> Action {
        let row = self.buf.pos().row;
        let Some(col) = self.buf.find_char_col(forward, till, ch, count.max(1)) else {
            return Action::None;
        };
        match op {
            ObjOp::Yank => {
                let anchor = self.buf.pos();
                self.buf.start_selection();
                self.buf.move_cursor(CursorMove::Jump(row, col));
                if let Some(t) = self.buf.copy_selection() {
                    self.yank_chars(t);
                }
                self.buf.cancel_selection();
                self.buf
                    .move_cursor(CursorMove::Jump(anchor.row, anchor.col));
                Action::None
            }
            ObjOp::Delete | ObjOp::Change => {
                // f deletes THROUGH its match; t lands one short of the
                // match, so inclusive-through-cursor equals vim's
                // exclusive-of-match. Backward motions stay exclusive.
                let inclusive = forward;
                let insert = matches!(op, ObjOp::Change);
                self.operate(&[CursorMove::Jump(row, col)], inclusive, false, insert);
                Action::Changed
            }
        }
    }

    /// Run an f/F/t/T motion to its target (plain or via ;/,).
    fn do_find(&mut self, forward: bool, till: bool, ch: char, count: usize) {
        if let Some(col) = self.buf.find_char_col(forward, till, ch, count.max(1)) {
            let row = self.buf.pos().row;
            self.buf.move_cursor(CursorMove::Jump(row, col));
        }
    }

    fn repeat(&mut self, m: CursorMove, n: usize) {
        for _ in 0..n {
            self.buf.move_cursor(m);
        }
    }

    /// The motion an arrow key stands for (vim's h/j/k/l aliases).
    fn arrow_motion(code: KeyCode) -> Option<CursorMove> {
        match code {
            KeyCode::Left => Some(CursorMove::Back),
            KeyCode::Right => Some(CursorMove::Forward),
            KeyCode::Up => Some(CursorMove::Up),
            KeyCode::Down => Some(CursorMove::Down),
            _ => None,
        }
    }

    fn handle_sequence(&mut self, leader: Pending, code: KeyCode) -> Action {
        use Pending::*;
        // trailing count after an operator (d3w): accumulate and keep the
        // operator pending (the count check used to run before this
        // dispatcher; the pending check moved first for visual sequences)
        if matches!(leader, D | C | Y | G | Dg | Cg) && self.accumulate_count(code) {
            self.pending = Some(leader);
            return Action::None;
        }
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
                self.yank_lines(rep, true);
                Action::None
            }
            (D, KeyCode::Char('d')) => {
                self.yank_lines(rep, false);
                self.delete_lines(rep);
                Action::Changed
            }
            (C, KeyCode::Char('c')) => {
                // [count]cc deletes the lines and edits one empty line
                // (one undo unit: change_lines snapshots before both steps)
                self.yank_lines(rep, false);
                self.buf.change_lines(rep);
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
            // f/F/t/T motions and their operator forms
            (Pending::Find { .. }, KeyCode::Esc) => Action::None,
            (Pending::Find { forward, till }, KeyCode::Char(c)) => {
                self.last_find = Some((forward, till, c));
                self.do_find(forward, till, c, rep);
                Action::None
            }
            (Pending::FindOp { .. }, KeyCode::Esc) => Action::None,
            (Pending::FindOp { op, forward, till }, KeyCode::Char(c)) => {
                self.last_find = Some((forward, till, c));
                self.apply_find_op(op, forward, till, c, rep)
            }
            // d;/c;/y; (and ,) repeat the last find as an operator motion
            (D, KeyCode::Char(';') | KeyCode::Char(','))
            | (C, KeyCode::Char(';') | KeyCode::Char(','))
            | (Y, KeyCode::Char(';') | KeyCode::Char(',')) => {
                let Some((forward, till, ch)) = self.last_find else {
                    return Action::None;
                };
                let flip = code == KeyCode::Char(',');
                let op = match leader {
                    D => ObjOp::Delete,
                    C => ObjOp::Change,
                    _ => ObjOp::Yank,
                };
                self.apply_find_op(op, forward != flip, till, ch, rep)
            }
            (D, KeyCode::Char(c)) if matches!(c, 'f' | 'F' | 't' | 'T') => {
                self.pending = Some(Pending::FindOp {
                    op: ObjOp::Delete,
                    forward: c == 'f' || c == 't',
                    till: c == 't' || c == 'T',
                });
                Action::None
            }
            (C, KeyCode::Char(c)) if matches!(c, 'f' | 'F' | 't' | 'T') => {
                self.pending = Some(Pending::FindOp {
                    op: ObjOp::Change,
                    forward: c == 'f' || c == 't',
                    till: c == 't' || c == 'T',
                });
                Action::None
            }
            (Y, KeyCode::Char(c)) if matches!(c, 'f' | 'F' | 't' | 'T') => {
                self.pending = Some(Pending::FindOp {
                    op: ObjOp::Yank,
                    forward: c == 'f' || c == 't',
                    till: c == 't' || c == 'T',
                });
                Action::None
            }
            // text objects: [cdy]i/a + w p quotes brackets
            (D, KeyCode::Char('i')) => {
                self.pending = Some(Pending::Obj {
                    op: ObjOp::Delete,
                    around: false,
                });
                Action::None
            }
            (C, KeyCode::Char('i')) => {
                self.pending = Some(Pending::Obj {
                    op: ObjOp::Change,
                    around: false,
                });
                Action::None
            }
            (Y, KeyCode::Char('i')) => {
                self.pending = Some(Pending::Obj {
                    op: ObjOp::Yank,
                    around: false,
                });
                Action::None
            }
            (D, KeyCode::Char('a')) => {
                self.pending = Some(Pending::Obj {
                    op: ObjOp::Delete,
                    around: true,
                });
                Action::None
            }
            (C, KeyCode::Char('a')) => {
                self.pending = Some(Pending::Obj {
                    op: ObjOp::Change,
                    around: true,
                });
                Action::None
            }
            (Y, KeyCode::Char('a')) => {
                self.pending = Some(Pending::Obj {
                    op: ObjOp::Yank,
                    around: true,
                });
                Action::None
            }
            (Pending::Obj { .. }, KeyCode::Esc) => Action::None,
            (Pending::Obj { op, around }, KeyCode::Char(c)) => {
                match crate::editor::buffer::parse_obj_kind(c) {
                    Some(kind) => self.apply_object(op, around, kind),
                    None => Action::None,
                }
            }
            // viw / va( … select the object, stay visual (vim)
            (Pending::VisualObj { .. }, KeyCode::Esc) => Action::None,
            (Pending::VisualObj { around }, KeyCode::Char(c)) => {
                match crate::editor::buffer::parse_obj_kind(c) {
                    Some(kind) => self.visual_select_object(around, kind),
                    None => Action::None,
                }
            }
            // vim-surround: ds pair / cs pair new / ys motion pair
            (D, KeyCode::Char('s')) => {
                self.pending = Some(Pending::SurroundDelete);
                Action::None
            }
            (C, KeyCode::Char('s')) => {
                self.pending = Some(Pending::SurroundChange { target: None });
                Action::None
            }
            (Y, KeyCode::Char('s')) => {
                self.pending = Some(Pending::SurroundAdd {
                    stage: AddStage::Motion,
                });
                Action::None
            }
            (Pending::SurroundDelete, KeyCode::Esc) => Action::None,
            (Pending::SurroundDelete, KeyCode::Char(c)) => {
                if Buffer::surround_pair(c).is_some() && self.buf.delete_surround(c) {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            (Pending::SurroundChange { .. }, KeyCode::Esc) => Action::None,
            (Pending::SurroundChange { target: None }, KeyCode::Char(t)) => {
                if Buffer::surround_pair(t).is_some() {
                    self.pending = Some(Pending::SurroundChange { target: Some(t) });
                }
                Action::None
            }
            (Pending::SurroundChange { target: Some(t) }, KeyCode::Char(r)) => {
                if Buffer::surround_pair(r).is_some() && self.buf.change_surround(t, r) {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            (Pending::SurroundAdd { .. }, KeyCode::Esc) => Action::None,
            (
                Pending::SurroundAdd {
                    stage: AddStage::Motion,
                },
                KeyCode::Char(c),
            ) => match c {
                'i' | 'a' => {
                    self.pending = Some(Pending::SurroundAdd {
                        stage: AddStage::Obj { around: c == 'a' },
                    });
                    Action::None
                }
                // yss: surround the (count-extended) line, minus its indent
                's' => {
                    self.pending = Some(Pending::SurroundAdd {
                        stage: AddStage::Line,
                    });
                    Action::None
                }
                'w' | 'e' | 'b' | '$' | '0' | '^' => match self.motion_object(c) {
                    Some(obj) => {
                        self.pending = Some(Pending::SurroundAdd {
                            stage: AddStage::Range(obj),
                        });
                        Action::None
                    }
                    None => Action::None,
                },
                _ => Action::None,
            },
            (
                Pending::SurroundAdd {
                    stage: AddStage::Obj { around },
                },
                KeyCode::Char(c),
            ) => {
                match crate::editor::buffer::parse_obj_kind(c)
                    .and_then(|k| self.buf.text_object(around, k))
                {
                    Some(obj) => {
                        self.pending = Some(Pending::SurroundAdd {
                            stage: AddStage::Range(obj),
                        });
                        Action::None
                    }
                    None => Action::None,
                }
            }
            (
                Pending::SurroundAdd {
                    stage: AddStage::Range(obj),
                },
                KeyCode::Char(t),
            ) => match Buffer::surround_pair(t) {
                Some(pair) => {
                    self.buf.surround_insert(obj, pair);
                    Action::Changed
                }
                None => Action::None,
            },
            // yss<pair>: wrap the line span (leading indent stays outside)
            (
                Pending::SurroundAdd {
                    stage: AddStage::Line,
                },
                KeyCode::Char(t),
            ) => {
                if let (Some(pair), Some(obj)) =
                    (Buffer::surround_pair(t), self.line_surround_span(rep))
                {
                    self.buf.surround_insert(obj, pair);
                    Action::Changed
                } else {
                    Action::None
                }
            }
            // visual S…: wrap the selection
            (Pending::SurroundVisual, KeyCode::Esc) => Action::None,
            (Pending::SurroundVisual, KeyCode::Char(c)) => self.surround_visual(c),
            _ => Action::None,
        }
    }

    /// Apply a text object operator (di/ci/yi/da/ca/ya).
    fn apply_object(&mut self, op: ObjOp, around: bool, kind: ObjKind) -> Action {
        let Some(obj) = self.buf.text_object(around, kind) else {
            return Action::None;
        };
        match op {
            ObjOp::Yank => {
                self.yank_chars(self.buf.range_text(obj));
                self.buf
                    .move_cursor(CursorMove::Jump(obj.start.row, obj.start.col));
                Action::None
            }
            ObjOp::Delete | ObjOp::Change => {
                // a delete fills the register but never the system clipboard
                self.register = self
                    .buf
                    .range_text(obj)
                    .split('\n')
                    .map(str::to_string)
                    .collect();
                match op {
                    ObjOp::Delete => {
                        self.delete_object(obj);
                        Action::Changed
                    }
                    _ => {
                        self.delete_object(obj);
                        self.mode = Mode::Insert;
                        Action::Changed
                    }
                }
            }
        }
    }

    fn delete_object(&mut self, obj: TextObj) {
        self.buf.cancel_selection();
        if obj.linewise {
            let n = obj.end.row - obj.start.row + 1;
            self.buf.move_cursor(CursorMove::Jump(obj.start.row, 0));
            self.delete_lines(n);
        } else {
            self.buf
                .move_cursor(CursorMove::Jump(obj.start.row, obj.start.col));
            // delete_str is one undo unit (raw delete_range never pushes);
            // TextObj.end is inclusive, advance makes the end exclusive
            let to = self.buf.advance(obj.end);
            let n = self.chars_between((obj.start.row, obj.start.col), (to.row, to.col));
            self.buf.delete_str(n);
        }
    }

    /// Select a text object from visual mode (viw/va…): stays visual, with
    /// the kind's linewise-ness (ip selects whole rows).
    fn visual_select_object(&mut self, around: bool, kind: ObjKind) -> Action {
        let Some(obj) = self.buf.text_object(around, kind) else {
            return Action::None;
        };
        self.buf.cancel_selection();
        self.visual_linewise = obj.linewise;
        self.buf
            .move_cursor(CursorMove::Jump(obj.start.row, obj.start.col));
        self.buf.start_selection();
        self.buf
            .move_cursor(CursorMove::Jump(obj.end.row, obj.end.col));
        Action::None
    }

    /// Span for the simple `ys` motions: w/e to the word end, b to the word
    /// start, $ to the line end, 0/^ to the line start.
    fn motion_object(&self, c: char) -> Option<TextObj> {
        let (row, col) = self.buf.cursor();
        let line: Vec<char> = self.buf.line(row).chars().collect();
        let cur = self.buf.pos();
        let at = |col: usize| line.get(col).copied();
        match c {
            '$' => {
                if col + 1 >= line.len() {
                    None
                } else {
                    Some(TextObj {
                        start: cur,
                        end: Pos {
                            row,
                            col: line.len() - 1,
                        },
                        linewise: false,
                    })
                }
            }
            '0' | '^' => {
                if col == 0 {
                    None
                } else {
                    Some(TextObj {
                        start: Pos { row, col: 0 },
                        end: Pos { row, col: col - 1 },
                        linewise: false,
                    })
                }
            }
            'w' | 'e' => {
                let is_word = |c: char| buffer::CharKind::new(c) == buffer::CharKind::Other;
                if !at(col).is_some_and(is_word) {
                    return None;
                }
                let mut e = col;
                while e + 1 < line.len() && is_word(line[e + 1]) {
                    e += 1;
                }
                Some(TextObj {
                    start: cur,
                    end: Pos { row, col: e },
                    linewise: false,
                })
            }
            'b' => {
                let is_word = |c: char| buffer::CharKind::new(c) == buffer::CharKind::Other;
                if col == 0 || !at(col).is_some_and(is_word) {
                    return None;
                }
                let mut st = col;
                while st > 0 && is_word(line[st - 1]) {
                    st -= 1;
                }
                if st == col {
                    None
                } else {
                    Some(TextObj {
                        start: Pos { row, col: st },
                        end: cur,
                        linewise: false,
                    })
                }
            }
            _ => None,
        }
    }

    /// Span for `yss`: the cursor row's first non-blank char through the
    /// last char of the count-extended end row — leading whitespace stays
    /// outside the pair, like vim-surround. Blank spans are a no-op.
    fn line_surround_span(&self, rep: usize) -> Option<TextObj> {
        let row = self.buf.cursor().0;
        let rows = self.buf.lines().len();
        let end_row = (row + rep.saturating_sub(1)).min(rows.saturating_sub(1));
        let indent = self
            .buf
            .line(row)
            .chars()
            .take_while(|c| c.is_whitespace())
            .count();
        let end_len = self.buf.line(end_row).chars().count();
        if row == end_row && indent >= end_len {
            return None; // blank line: nothing to surround
        }
        Some(TextObj {
            start: Pos { row, col: indent },
            end: Pos {
                row: end_row,
                col: end_len.saturating_sub(1),
            },
            linewise: false,
        })
    }

    /// Visual `S{pair}`: wrap the selection and return to normal mode.
    fn surround_visual(&mut self, c: char) -> Action {
        let Some(pair) = Buffer::surround_pair(c) else {
            return Action::None;
        };
        let obj = if self.visual_linewise {
            let Some((from, n)) = self.selection_lines() else {
                return Action::None;
            };
            let end_row = from + n - 1;
            TextObj {
                start: Pos { row: from, col: 0 },
                end: Pos {
                    row: end_row,
                    col: self.buf.line(end_row).chars().count().saturating_sub(1),
                },
                linewise: false,
            }
        } else {
            let Some((s, e)) = self.buf.selection_range() else {
                return Action::None;
            };
            TextObj {
                start: s,
                end: e,
                linewise: false,
            }
        };
        self.buf.cancel_selection();
        self.buf.surround_insert(obj, pair);
        self.mode = Mode::Normal;
        self.visual_linewise = false;
        Action::Changed
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
        self.yank_lines(n, false);
        if insert {
            // cG/cgg: one empty editable line, one undo unit
            self.buf.change_lines(n);
            self.mode = Mode::Insert;
        } else {
            self.delete_lines(n);
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

    /// Yank `n` lines from the cursor row into the register; `to_system`
    /// also queues a system-clipboard copy (linewise, the lines end with
    /// a trailing newline like nvim's clipboard=unnamedplus yy). Deletes
    /// reuse this to fill the register with `to_system = false`: a `d`
    /// never touches the system clipboard.
    fn yank_lines(&mut self, n: usize, to_system: bool) {
        let (row, _) = self.buf.cursor();
        let lines = self.buf.lines();
        let end = (row + n.max(1)).min(lines.len());
        self.register = lines[row..end].to_vec();
        if to_system {
            let mut text = self.register.join("\n");
            text.push('\n');
            self.system_yank = Some(text);
        }
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
        use buffer::CharKind;
        let (row, col) = self.buf.cursor();
        let chars: Vec<char> = self.buf.lines().get(row)?.chars().collect();
        if chars.is_empty() {
            return None;
        }
        let probe = col.min(chars.len() - 1);
        let kind = CharKind::new(chars[probe]);
        let same = |c: char| CharKind::new(c) == kind;
        let mut s = probe;
        while s > 0 && same(chars[s - 1]) {
            s -= 1;
        }
        let mut e = probe;
        while e + 1 < chars.len() && same(chars[e + 1]) {
            e += 1;
        }
        let e = e + 1; // exclusive
        if a_word && kind != CharKind::Space {
            let mut ne = e;
            while ne < chars.len() && CharKind::new(chars[ne]) == CharKind::Space {
                ne += 1;
            }
            if ne > e {
                return Some((s, ne));
            }
            let mut ns = s;
            while ns > 0 && CharKind::new(chars[ns - 1]) == CharKind::Space {
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

    // ---- Shift+arrows extend a selection (regular-mode behavior) ----

    fn shift(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::SHIFT)
    }

    #[test]
    fn shift_arrows_select_in_normal_mode() {
        let mut e = editor("one two\nsecond");
        e.handle_key(shift(KeyCode::Right));
        e.handle_key(shift(KeyCode::Right));
        assert_eq!(e.cursor(), (0, 2), "cursor is the moving end");
        assert_eq!(e.buf.selection_anchor(), Some(Pos { row: 0, col: 0 }));
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 0));
        assert_eq!((x.row, x.col), (0, 3), "cursor side is exclusive");

        // extending the other way crosses the anchor; back to the row head
        // (row 0: Back has nowhere to wrap) leaves one walked-over cell
        e.handle_key(shift(KeyCode::Left));
        e.handle_key(shift(KeyCode::Left));
        e.handle_key(shift(KeyCode::Left));
        assert_eq!(e.cursor(), (0, 0), "capped at the line head");
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 0));
        assert_eq!((x.row, x.col), (0, 1), "the walked-over cell");
    }

    #[test]
    fn plain_arrow_collapses_a_selection_in_normal_mode() {
        let mut e = editor("one two");
        e.handle_key(shift(KeyCode::Right));
        e.handle_key(shift(KeyCode::Right));
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(e.buf.selection_anchor(), None, "collapsed");
        assert_eq!(e.cursor(), (0, 3), "and moved");
        // motions from here extend nothing
        e.handle_key(key('l'));
        assert_eq!(e.buf.selection_anchor(), None);
        assert_eq!(e.cursor(), (0, 4));
    }

    #[test]
    fn count_applies_to_shift_arrows_in_normal_mode() {
        let mut e = editor("abcdef");
        e.handle_key(key('3'));
        e.handle_key(shift(KeyCode::Right));
        assert_eq!(e.cursor(), (0, 3), "3<Shift+Right> moves 3");
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 0));
        assert_eq!((x.row, x.col), (0, 4), "cursor side is exclusive");
        assert_eq!(e.count, None, "the count is consumed");
    }

    #[test]
    fn shift_arrows_select_in_insert_mode() {
        let mut e = editor("one\ntwo");
        e.handle_key(key('i'));
        e.handle_key(shift(KeyCode::Down));
        e.handle_key(shift(KeyCode::Right));
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.cursor(), (1, 1), "cursor is the moving end");
        assert_eq!(e.buf.selection_anchor(), Some(Pos { row: 0, col: 0 }));
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 0));
        assert_eq!((x.row, x.col), (1, 2), "cursor side is exclusive");

        // a plain arrow collapses and keeps moving; typing then inserts
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(e.buf.selection_anchor(), None);
        assert_eq!(e.cursor(), (1, 2));
        e.handle_key(key('!'));
        assert_eq!(e.text(), "one\ntw!o");
    }

    #[test]
    fn visual_mode_arrows_still_extend() {
        let mut e = editor("one two three");
        e.handle_key(key('v'));
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Visual);
        assert_eq!(e.buf.selection_anchor(), Some(Pos { row: 0, col: 0 }));
        assert_eq!(e.cursor(), (0, 2));
        e.handle_key(key('y'));
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.buf.selection_range().is_none());
        assert_eq!(e.register, vec!["one".to_string()]);
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
    fn cc_escape_stays_on_the_cleared_line() {
        // vim: Esc from insert at column 0 stays put; it must not wrap to
        // the end of the line above (the cleared line is empty, col 0)
        let mut e = editor("one\ntwo\nthree");
        for c in "jcc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.cursor(), (1, 0), "cursor stays on the cleared line");
        assert_eq!(e.text(), "one\n\nthree");

        // first line
        let mut e = editor("one\ntwo\nthree");
        for c in "cc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.cursor(), (0, 0));
        assert_eq!(e.text(), "\ntwo\nthree");

        // last line
        let mut e = editor("one\ntwo");
        for c in "jcc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.cursor(), (1, 0));

        // the only line of the document
        let mut e = editor("only");
        for c in "cc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.cursor(), (0, 0));

        // typed text keeps the cursor on that line after Esc
        let mut e = editor("one\ntwo\nthree");
        for c in "jccX".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.cursor(), (1, 0));
        assert_eq!(e.text(), "one\nX\nthree");
    }

    #[test]
    fn cc_undo_restores_the_line_in_one_step() {
        // one u restores the original line in place (was: the first u
        // deleted the cleared line, a second was needed for the text)
        let mut e = editor("one\ntwo\nthree");
        for c in "jcc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.text(), "one\ntwo\nthree");
        assert_eq!(e.cursor().0, 1, "undo returns to the changed line");
        // redo brings the cleared line back
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "one\n\nthree");

        // [count]cc is one undo unit too
        let mut e = editor("a\nb\nc\nd");
        for c in "j3cc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.text(), "a\nb\nc\nd");

        // so is the cG form
        let mut e = editor("one\ntwo\nthree");
        for c in "jcG".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.text(), "one\ntwo\nthree");
    }

    #[test]
    fn visual_linewise_c_undo_restores_rows() {
        let mut e = editor("a\nb\nc\n");
        for c in "Vjc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "\nc\n");
        assert_eq!(e.mode, Mode::Insert);
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.text(), "a\nb\nc\n");
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
mod space_tests {
    use super::*;

    fn ed(text: &str) -> Editor {
        let mut e = Editor::default();
        e.set_text(text);
        e.mode = Mode::Insert;
        e
    }

    #[test]
    fn space_types_in_insert_mode() {
        // regression: Space fell through the insert match and typed nothing
        let mut e = ed("ab");
        e.buf.move_cursor(CursorMove::Jump(0, 1));
        let _ = e.handle_key(KeyEvent::new(KeyCode::Space, KeyModifiers::NONE));
        assert_eq!(e.buf.line(0), "a b");
    }

    #[test]
    fn space_then_e_types_in_insert_mode() {
        // the app-level Space+e leader is gated to non-insert modes; pin
        // that insert-mode space is data, not a leader
        let mut e = ed("x");
        e.buf.move_cursor(CursorMove::Jump(0, 1));
        let _ = e.handle_key(KeyEvent::new(KeyCode::Space, KeyModifiers::NONE));
        let _ = e.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        assert_eq!(e.buf.line(0), "x e");
    }
}

#[cfg(test)]
mod bracket_inner_tests {
    use super::*;
    use super::tests::{editor, key};

    #[test]
    fn ci_brace_multiline_leaves_an_empty_line_and_inserts() {
        // nvim: ci{ on a multi-line block clears the lines between the
        // braces but keeps an empty line, cursor on it, insert mode
        let mut e = editor("Table users {\n  id integer\n  name varchar\n}\n");
        e.buf
            .move_cursor(CursorMove::Jump(1, 2)); // inside the block
        let _ = e.handle_key(key('c'));
        let _ = e.handle_key(key('i'));
        let _ = e.handle_key(key('{'));
        assert_eq!(
            e.buf.lines(),
            vec![
                "Table users {".to_string(),
                "  ".to_string(),
                "}".to_string()
            ]
        );
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 2 }, "cursor after the indent");
    }

    #[test]
    fn di_brace_multiline_matches_ci_deletion() {
        let mut e = editor("Table users {\n  id integer\n}\n");
        e.buf.move_cursor(CursorMove::Jump(1, 2));
        let _ = e.handle_key(key('d'));
        let _ = e.handle_key(key('i'));
        let _ = e.handle_key(key('{'));
        assert_eq!(
            e.buf.lines(),
            vec![
                "Table users {".to_string(),
                "  ".to_string(),
                "}".to_string()
            ]
        );
        assert_eq!(e.mode, Mode::Normal);
    }

    #[test]
    fn ci_paren_single_line_keeps_the_pair_glyphs() {
        let mut e = editor("f ( foo ) g");
        e.buf.move_cursor(CursorMove::Jump(0, 4));
        let _ = e.handle_key(key('c'));
        let _ = e.handle_key(key('i'));
        let _ = e.handle_key(key('('));
        assert_eq!(e.buf.line(0), "f () g");
        assert_eq!(e.mode, Mode::Insert);
    }

    #[test]
    fn ci_brace_on_adjacent_braces_is_a_noop() {
        let mut e = editor("Table users {\n}\n");
        e.buf.move_cursor(CursorMove::Jump(0, 5));
        let _ = e.handle_key(key('c'));
        let _ = e.handle_key(key('i'));
        let _ = e.handle_key(key('{'));
        assert_eq!(
            e.buf.lines(),
            vec!["Table users {".to_string(), "}".to_string()]
        );
    }
}

#[cfg(test)]
mod polish_tests {
    use super::tests::{editor, key};
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

    // ---- text objects + vim-surround ----

    #[test]
    fn visual_inner_word_selects() {
        let mut e = editor("one two three\n");
        for c in "llllllviw".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.mode, Mode::Visual, "stays visual (vim)");
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 4));
        assert_eq!((x.row, x.col), (0, 7), "two, exclusive end");
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one  three\n");
    }

    #[test]
    fn visual_c_deletes_and_inserts() {
        let mut e = editor("one two three\n");
        for c in "vllc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), " two three\n", "only \"one\" is deleted");
        assert_eq!(e.mode, Mode::Insert, "c concludes visual into insert");
        e.handle_key(key('X'));
        assert_eq!(e.text(), "X two three\n");
        // s behaves the same (vim alias)
        let mut e = editor("one two three\n");
        for c in "lves".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "o two three\n");
        assert_eq!(e.mode, Mode::Insert);
        // linewise: rows collapse to one editable empty line (cc-style)
        let mut e = editor("a\nb\nc\n");
        for c in "Vjc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "\nc\n", "rows a+b collapse to one empty line");
        assert_eq!(e.mode, Mode::Insert);
        e.handle_key(key('Z'));
        assert_eq!(e.text(), "Z\nc\n");
    }

    #[test]
    fn find_motions_and_repeats() {
        // f, jumps to the next comma; ; advances; , goes back
        let mut e = editor("a,b,c,d\n");
        for c in "f,".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 1));
        e.handle_key(key(';'));
        assert_eq!(e.cursor(), (0, 3));
        e.handle_key(key(';'));
        assert_eq!(e.cursor(), (0, 5));
        e.handle_key(key(','));
        assert_eq!(e.cursor(), (0, 3));
        // F scans backward
        let mut e = editor("a,b,c,d\n");
        for c in "$F,".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 5));
        // counts: 2f,
        let mut e = editor("a,b,c,d\n");
        for c in "2f,".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 3));
        // t stops one short of the target
        let mut e = editor("(foo)\n");
        for c in "t)".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 3), "t) lands before the paren");
        // T stops one after the previous match
        let mut e = editor("(x)y\n");
        for c in "$T)".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 3), "after the ), before the y");
        // visual f extends the selection
        let mut e = editor("a,b,c\n");
        for c in "vf,d".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "b,c\n");
        // no match on the line: cursor stays
        let mut e = editor("abc\n");
        for c in "fz".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 0));
    }

    #[test]
    fn find_operators_df_cf_yf() {
        // df( deletes through the paren (inclusive)
        let mut e = editor("foo(bar)\n");
        for c in "df(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "bar)\n");
        // dt) deletes up to but excluding the paren
        let mut e = editor("(foo)\n");
        for c in "ldt)".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "()\n");
        // cf( changes through the paren
        let mut e = editor("foo(bar)\n");
        for c in "cf(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "bar)\n");
        assert_eq!(e.mode, Mode::Insert);
        // yf, yanks through the comma, cursor stays put
        let mut e = editor("a,b,c\n");
        for c in "yf,".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.register, vec!["a,".to_string()]);
        assert_eq!(e.cursor(), (0, 0), "yank leaves the cursor");
        // d; repeats the find under the operator
        let mut e = editor("a,b,c,d\n");
        for c in "f,".chars() {
            e.handle_key(key(c));
        }
        for c in "d;".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "ac,d\n", "d; deletes through the next comma");
    }

    #[test]
    fn visual_around_word_takes_trailing_space() {
        let mut e = editor("one two three\n");
        for c in "lllllvaw".chars() {
            e.handle_key(key(c));
        }
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 4));
        assert_eq!((x.row, x.col), (0, 8), "two + trailing blank");
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one three\n");
    }

    #[test]
    fn visual_paragraph_selects_linewise() {
        let mut e = editor("a\nb\n\nc\n");
        for c in "vip".chars() {
            e.handle_key(key(c));
        }
        assert!(e.visual_linewise, "ip switches visual to linewise");
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, x.row), (0, 1), "the non-blank block");
        e.handle_key(key('d'));
        assert_eq!(e.text(), "\nc\n");
        // ap includes the trailing blank row, so d leaves only "c"
        let mut e = editor("a\nb\n\nc\n");
        for c in "vapd".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "c\n");
    }

    #[test]
    fn visual_quote_objects() {
        let mut e = editor("say \"hello\" ok\n");
        for c in "llllll".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(key('v'));
        e.handle_key(key('i'));
        e.handle_key(key('"'));
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 5));
        assert_eq!((x.row, x.col), (0, 10), "hello, exclusive");
        let mut e = editor("say \"hello\" ok\n");
        for c in "llllll".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(key('v'));
        e.handle_key(key('a'));
        e.handle_key(key('"'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "say  ok\n");
    }

    #[test]
    fn visual_bracket_objects() {
        let mut e = editor("f(a, (b) c)g\n");
        for c in "llllll".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(key('v'));
        e.handle_key(key('i'));
        e.handle_key(key('('));
        let (s, x) = e.buf.selection_range().unwrap();
        assert_eq!((s.row, s.col), (0, 6), "innermost pair's inside");
        assert_eq!((x.row, x.col), (0, 7), "b, exclusive");
        let mut e = editor("f(a, (b) c)g\n");
        for c in "llllllva(d".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "f(a,  c)g\n");
        // multiline inner body deletes cleanly (whole inner rows drop)
        let mut e = editor("f(\n  a\n  b\n)g\n");
        for c in "jvi(d".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "f(\n  \n)g\n", "inner rows drop, the blank stays indented");
    }

    #[test]
    fn change_and_delete_bracket_objects() {
        let mut e = editor("f(a, b)g\n");
        for c in "llci(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "f()g\n");
        assert_eq!(e.mode, Mode::Insert, "ci( enters insert");
        let mut e = editor("f(a, b)g\n");
        for c in "llda(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fg\n");
        let mut e = editor("say \"hi\" x\n");
        for c in "lllllldi\"".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "say \"\" x\n");
        // aliases: vib == vi(, diB == di{
        let mut e = editor("f(a)b\n");
        for c in "llvibd".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "f()b\n");
        let mut e = editor("f{a}b\n");
        for c in "lldiB".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "f{}b\n");
    }

    #[test]
    fn object_ops_are_undoable() {
        // di' then u restores the deleted inner text
        let mut e = editor("say 'hello' ok\n");
        for c in "lllllldi'".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "say '' ok\n");
        e.handle_key(key('u'));
        assert_eq!(e.text(), "say 'hello' ok\n");

        // ci' + typing: the delete sits on the undo stack below the typed
        // chars, so enough undos restore the original
        let mut e = editor("say 'hello' ok\n");
        for c in "llllllci'".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "say '' ok\n");
        assert_eq!(e.mode, Mode::Insert);
        for c in "hey".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "say 'hey' ok\n");
        e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        for _ in 0..4 {
            e.handle_key(key('u'));
        }
        assert_eq!(e.text(), "say 'hello' ok\n", "the ci' delete is undoable");
    }

    #[test]
    fn yank_objects_fill_the_register() {
        let mut e = editor("f(a, b)g\n");
        for c in "llyi(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.register, vec!["a, b".to_string()]);
        assert_eq!(e.text(), "f(a, b)g\n", "yank changes nothing");
        let mut e = editor("one \"xx\" y\n");
        for c in "lllllya\"".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(
            e.register,
            vec!["\"xx\"".to_string()],
            "a\" includes quotes"
        );
    }

    #[test]
    fn surround_delete_and_change() {
        let mut e = editor("f(w)g\n");
        for c in "llds(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fwg\n", "ds( strips parens");
        let mut e = editor("\"q\"\n");
        for c in "ds\"".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "q\n", "ds\" strips quotes");
        let mut e = editor("'q'\n");
        for c in "cs'\"".chars() {
            e.handle_key(key(c));
        }
        // cs<target><new>: ' -> "
        assert_eq!(e.text(), "\"q\"\n", "cs swaps quote styles");
        let mut e = editor("f[w]g\n");
        for c in "llcs[{".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "f{w}g\n", "cs grows delimiters");
        let mut e = editor("abc\n");
        for c in "ds(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "abc\n", "no enclosing pair: no-op");
    }

    #[test]
    fn surround_add() {
        let mut e = editor("one two\n");
        for c in "lllllysiw)".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one (two)\n", "ysiw) wraps the word");
        let mut e = editor("one two\n");
        for c in "llys$'".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "on'e two'\n", "ys$ wraps to line end");
        let mut e = editor("a\nb\n");
        for c in "ysip{".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "{a\nb}\n", "ysip wraps the paragraph");
    }

    #[test]
    fn visual_s_wraps_the_selection() {
        let mut e = editor("one two\n");
        for c in "llllvleS)".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one (two)\n");
        assert_eq!(e.mode, Mode::Normal, "S concludes visual");
    }

    // ---- vim-surround aliases (b B r a) + yss ----

    #[test]
    fn surround_aliases_resolve_as_replacements_and_targets() {
        // cs'b: the b alias stands for ( )
        let mut e = editor("'q'\n");
        for c in "cs'b".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "(q)\n", "cs'b swaps quotes for parens");
        // cs'B: braces
        let mut e = editor("'q'\n");
        for c in "cs'B".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "{q}\n", "cs'B swaps quotes for braces");
        // csbr round-trips parens -> brackets; csb( is an identity
        let mut e = editor("(q)\n");
        for c in "csbr".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "[q]\n");
        let mut e = editor("(q)\n");
        for c in "csb(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "(q)\n", "csb( round-trips unchanged");
        // ds targets: b/B/r/a delete their bracket pair
        let mut e = editor("f(w)g\n");
        for c in "lldsb".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fwg\n", "dsb strips parens");
        let mut e = editor("f[w]g\n");
        for c in "lldsr".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fwg\n", "dsr strips brackets");
        let mut e = editor("f{w}g\n");
        for c in "lldsB".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fwg\n", "dsB strips braces");
        let mut e = editor("f<w>g\n");
        for c in "lldsa".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fwg\n", "dsa strips angle brackets");
        // the literal glyphs still work beside the aliases
        let mut e = editor("f(w)g\n");
        for c in "llds(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "fwg\n");
    }

    #[test]
    fn yss_surrounds_the_line_excluding_indent() {
        let mut e = editor("one two\n");
        for c in "yss(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "(one two)\n", "yss wraps the whole line");
        // the open glyph sits after the leading whitespace
        let mut e = editor("    indented text\n");
        for c in "yss[".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "    [indented text]\n");
        // alias pair: yssb == yss(
        let mut e = editor("word\n");
        for c in "yssb".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "(word)\n");
        // blank line: nothing to surround
        let mut e = editor("   \n");
        for c in "yss(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "   \n", "blank line is a no-op");
        // [count]yss spans count lines
        let mut e = editor("a\nb\nc\n");
        for c in "2yss{".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "{a\nb}\nc\n");
        e.buf.undo();
        assert_eq!(e.text(), "a\nb\nc\n", "the pair insert is one undo unit");
        // yss( then dsb is an identity round-trip
        let mut e = editor("one two\n");
        for c in "yss(".chars() {
            e.handle_key(key(c));
        }
        for c in "dsb".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one two\n", "yss( + dsb restores the line");
    }

    #[test]
    fn ysiw_wraps_words_with_all_pair_kinds() {
        let mut e = editor("one two\n");
        for c in "lllllysiw(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one (two)\n", "ysiw( wraps the word");
        let mut e = editor("one two\n");
        for c in "lllllysiw'".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one 'two'\n", "ysiw' wraps in quotes");
        let mut e = editor("one two\n");
        for c in "lllllysiwB".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one {two}\n", "ysiwB wraps in braces via alias");
        let mut e = editor("one two three\n");
        for c in "lllllysiwb".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(
            e.text(),
            "one (two) three\n",
            "ysiwb wraps in parens via alias"
        );
        // the plain motion stage: ys w wraps from the cursor to the word end
        let mut e = editor("one two\n");
        for c in "ysw(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "(one) two\n");
        // aw keeps vim's trailing space (leading when there is none)
        let mut e = editor("one two three\n");
        for c in "lllllysaw(".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(
            e.text(),
            "one (two )three\n",
            "ysaw( keeps the trailing space"
        );
    }

    #[test]
    fn visual_s_accepts_alias_chars() {
        let mut e = editor("one two\n");
        for c in "llllvleSB".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one {two}\n", "S B wraps in braces via alias");
        assert_eq!(e.mode, Mode::Normal, "S concludes visual");
        let mut e = editor("one two\n");
        for c in "llllvleSr".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.text(), "one [two]\n", "S r wraps in brackets via alias");
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

    #[test]
    fn yanks_queue_a_system_copy_deletes_do_not() {
        // yy: linewise — the system text ends with a trailing newline
        // (nvim's clipboard=unnamedplus), the register keeps bare lines
        let mut e = editor("hello world\nsecond\n");
        e.handle_key(key('y'));
        e.handle_key(key('y'));
        assert_eq!(e.take_system_yank().as_deref(), Some("hello world\n"));
        assert_eq!(e.register_lines(), vec!["hello world".to_string()]);

        // yiw: charwise — the object text, no trailing newline
        e.handle_key(key('y'));
        e.handle_key(key('i'));
        e.handle_key(key('w'));
        assert_eq!(e.take_system_yank().as_deref(), Some("hello"));

        // visual y: the selection
        e.handle_key(key('v'));
        e.handle_key(key('e'));
        e.handle_key(key('y'));
        assert_eq!(e.take_system_yank().as_deref(), Some("hello"));
        assert_eq!(e.mode, Mode::Normal);

        // dd fills the register but NEVER queues a system copy
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert!(e.take_system_yank().is_none(), "delete is not a yank");
        assert_eq!(e.register_lines(), vec!["hello world".to_string()]);
        assert_eq!(e.text(), "second\n");

        // and the queue drains: a second take is empty
        assert!(e.take_system_yank().is_none());
    }

    #[test]
    fn y_operator_motions_queue_a_system_copy() {
        // yf through the found char
        let mut e = editor("hello world");
        e.handle_key(key('y'));
        e.handle_key(key('f'));
        e.handle_key(key('o'));
        assert_eq!(e.take_system_yank().as_deref(), Some("hello"));
    }
}
