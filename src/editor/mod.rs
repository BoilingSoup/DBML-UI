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
    F1,
    F2,
    F12,
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
    /// `gd`: the app resolves the word under the caret to its definition
    /// (the engine has no schema access) and moves the cursor there.
    GotoDefinition,
}

/// The find popup's live query (the GUI find, shared by vim `/` and
/// Regular ctrl-f). Each keystroke re-sets the buffer's search pattern
/// and jumps to the first match at/after the caret; Enter cycles
/// forward, `n`/`N` keep working once the popup closes.
#[derive(Debug, Clone)]
pub struct FindState {
    pub input: String,
    /// The caret as a CHAR offset into `input` — the active end of the
    /// selection (where typed text lands).
    pub caret: usize,
    /// The selection anchor (char offset); None = no selection. The
    /// selected span spans between anchor and caret.
    pub anchor: Option<usize>,
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
    /// `y g…` awaiting `g` (ygg)
    Yg,
    /// `g c…` — the comment-toggle operator awaiting the second `c`
    /// (vim-commentary's gcc line toggle)
    Gc,
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

/// The yanked region, flashed briefly in the editor (vim's on_yank blink):
/// start/end in buffer positions, end exclusive. A linewise yank encodes
/// whole rows as (first row, 0) .. (end row, 0), which the renderer's band
/// helper draws full-width.
#[derive(Debug, Clone, Copy)]
pub struct YankFlash {
    pub start: (usize, usize),
    pub end: (usize, usize),
    pub started: std::time::Instant,
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
    pub find: Option<FindState>,
    /// The vim `/` prompt's live query — a vim-style bottom command line,
    /// NOT the gui find popup (ctrl+f / the toolbar button own that).
    pub vim_search: Option<String>,
    register: Vec<String>,
    /// The most recent YANK (the `y` family: y/yy/yiw/visual y…), queued
    /// for the app to push to the SYSTEM clipboard — the Editor has no
    /// arboard access, so the app drains this after every key
    /// (`take_system_yank`). Deletes fill the register but never queue a
    /// system copy (vim's unnamed-register semantics).
    system_yank: Option<String>,
    /// Last f/F/t/T (forward, till, char) for `;` and `,`
    last_find: Option<(bool, bool, char)>,
    /// The yanked region flashing after a yank (vim's on_yank blink).
    pub yank_flash: Option<YankFlash>,
    /// How the register was filled: linewise (yy/yj/visual-V y) pastes
    /// whole lines below/above; charwise (yw/visual y) pastes inline at
    /// the cursor (vim's put semantics).
    register_linewise: bool,
    /// Source ended with a newline; preserved by `text()` for byte-identical
    /// file round-trips.
    trailing_newline: bool,
    /// Vim's jumplist, two stacks: `jumps_past` holds jump ORIGINS (the
    /// position before each jump, most recent on top), `jumps_future` the
    /// positions left by Ctrl+O so Ctrl+I can return to them. A new jump
    /// discards the future, like vim's.
    jumps_past: Vec<Pos>,
    jumps_future: Vec<Pos>,
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
            .field("find", &self.find)
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
            find: None,
            vim_search: None,
            register: Vec::new(),
            system_yank: None,
            last_find: None,
            yank_flash: None,
            register_linewise: false,
            trailing_newline: source.ends_with('\n'),
            jumps_past: Vec::new(),
            jumps_future: Vec::new(),
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
        self.register_linewise = true;
    }

    /// Replace the yank register with an explicit linewise-ness: the
    /// external-clipboard adoption (vim `p` over copied-from-elsewhere
    /// text) classifies by the text's trailing newline.
    pub fn set_register_kind(&mut self, lines: Vec<String>, linewise: bool) {
        self.register = lines;
        self.register_linewise = linewise;
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

    /// The previous/next word boundary in a one-line input (char offsets):
/// whitespace/non-whitespace transitions — the standard text-field word
/// motion the find field's ctrl/alt+arrows walk.
fn find_word_boundary(input: &str, from: usize, forward: bool) -> usize {
    let chars: Vec<char> = input.chars().collect();
    let mut i = from.min(chars.len());
    let is_word = |c: char| !c.is_whitespace();
    if forward {
        while i < chars.len() && !is_word(chars[i]) {
            i += 1;
        }
        while i < chars.len() && is_word(chars[i]) {
            i += 1;
        }
    } else {
        while i > 0 && !is_word(chars[i - 1]) {
            i -= 1;
        }
        while i > 0 && is_word(chars[i - 1]) {
            i -= 1;
        }
    }
    i
}

/// Charwise yank: the text into the register AND queued for the
    /// system clipboard.
    fn yank_chars(&mut self, text: String) {
        self.register = text.split('\n').map(str::to_string).collect();
        self.register_linewise = false;
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
            Some(Pending::Yg) => "yg",
            Some(Pending::Gc) => "gc",
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
        // the vim `/` prompt swallows every key while open (typing
        // searches live, Enter commits, Esc cancels)
        if self.vim_search.is_some() {
            return self.handle_vim_search_key(key);
        }
        // the find popup swallows every key while open (typing edits the
        // query, Enter cycles, Esc closes) — vim and Regular alike
        if self.find.is_some() {
            return self.handle_find_key(key);
        }
        // vim normal/visual never REST on the past-the-last-char cell:
        // the block cursor would sit on a phantom column past EOL (which
        // reads as "a space at the end of the line"). The buffer cursor is
        // mode-agnostic — the insert caret legitimately lives at len — so
        // the resting clamp happens here, but ONLY when the key STAYED in
        // normal/visual: a motion that ENTERED insert (i/a/A/o/cc) must
        // keep its landing caret untouched.
        let was_vim = self.mode != Mode::Insert;
        let mode_before = self.mode;
        let action = match self.mode {
            Mode::Insert => self.handle_insert_key(key),
            Mode::Normal | Mode::Visual => self.handle_normal_key(key),
        };
        if was_vim && self.mode != Mode::Insert {
            self.buf.clamp_cursor_to_line_end();
        }
        // leaving a vim mode for another hides the search match bands
        // (vim's :noh on mode-changing actions) — the pattern stays for
        // `n`/`N`, and a fresh `/` or find query re-shows them
        if self.mode != mode_before {
            self.buf.hide_search_hl();
        }
        action
    }

    /// Keys while the find popup is open — always consumed. Typing edits
    /// the query LIVE (the pattern re-sets and the caret jumps to the
    /// first match at/after it per keystroke), Enter cycles forward, Esc
    /// closes: the match bands drop (:noh) while the pattern stays for
    /// `n`/`N`.
    pub fn handle_find_key(&mut self, key: KeyEvent) -> Action {
        let mods = key.modifiers;
        match key.code {
            KeyCode::Esc => {
                self.close_find();
                Action::None
            }
            KeyCode::Enter => {
                self.buf.search_forward(false);
                Action::None
            }
            KeyCode::Left | KeyCode::Right if !mods.contains(KeyModifiers::ALT) => {
                // caret motion: plain = char, ctrl = word, SHIFT =
                // selection. A plain arrow with a live selection
                // COLLAPSES to the pressed edge (left to the start,
                // right to the end — the standard text-field behavior),
                // then walks from there. ALT+arrows are inert (the user's
                // contract: no move, no select).
                let Some(f) = &mut self.find else {
                    return Action::None;
                };
                let forward = key.code == KeyCode::Right;
                let len = f.input.chars().count();
                let caret = f.caret.min(len);
                let anchor = f.anchor.unwrap_or(caret).min(len);
                let (lo, hi) = (caret.min(anchor), caret.max(anchor));
                let has_sel = hi > lo;
                let by_word = mods.contains(KeyModifiers::CONTROL)
                    || mods.contains(KeyModifiers::ALT);
                let select =
                    mods.contains(KeyModifiers::SHIFT) || mods.contains(KeyModifiers::ALT);
                let base = if !select && has_sel {
                    if forward {
                        hi
                    } else {
                        lo
                    }
                } else {
                    caret
                };
                let target = if by_word {
                    Self::find_word_boundary(&f.input, base, forward)
                } else if forward {
                    (base + 1).min(len)
                } else {
                    base.saturating_sub(1)
                };
                if select {
                    f.anchor.get_or_insert(base);
                    f.caret = target;
                } else {
                    f.caret = target;
                    f.anchor = None;
                }
                Action::None
            }
            KeyCode::Home | KeyCode::End => {
                let Some(f) = &mut self.find else {
                    return Action::None;
                };
                let target = if key.code == KeyCode::Home {
                    0
                } else {
                    f.input.chars().count()
                };
                if mods.contains(KeyModifiers::SHIFT) {
                    f.anchor.get_or_insert(f.caret.min(f.input.chars().count()));
                    f.caret = target;
                } else {
                    f.caret = target;
                    f.anchor = None;
                }
                Action::None
            }
            KeyCode::Backspace | KeyCode::Delete => {
                // with a selection the chord empties it; without, the
                // chord eats one char on the caret's side
                let len = self
                    .find
                    .as_ref()
                    .map(|f| f.input.chars().count())
                    .unwrap_or(0);
                let has_sel = self
                    .find
                    .as_ref()
                    .map(|f| f.anchor.map(|a| a != f.caret.min(len)).unwrap_or(false))
                    .unwrap_or(false);
                let forward = key.code == KeyCode::Delete;
                if has_sel {
                    self.find_delete_selection();
                } else if let Some(f) = &mut self.find {
                    let caret = f.caret.min(len);
                    let drop = if forward { caret } else { caret.saturating_sub(1) };
                    if (!forward && caret > 0) || (forward && caret < len) {
                        let b = f.input.char_indices().nth(drop).map(|(i, _)| i).unwrap_or(f.input.len());
                        f.input.remove(b);
                        if !forward {
                            f.caret = caret - 1;
                        }
                    }
                    f.anchor = None;
                    self.apply_find_query();
                }
                Action::None
            }
            KeyCode::Char(c)
                if !mods.contains(KeyModifiers::CONTROL)
                    && !mods.contains(KeyModifiers::ALT) =>
            {
                // typing replaces the selection or inserts at the caret
                if let Some(f) = &mut self.find {
                    let len = f.input.chars().count();
                    let caret = f.caret.min(len);
                    let anchor = f.anchor.unwrap_or(caret).min(len);
                    let (s, e) = (caret.min(anchor), caret.max(anchor));
                    let start_b = f
                        .input
                        .char_indices()
                        .nth(s)
                        .map(|(i, _)| i)
                        .unwrap_or(f.input.len());
                    let end_b = f
                        .input
                        .char_indices()
                        .nth(e)
                        .map(|(i, _)| i)
                        .unwrap_or(f.input.len());
                    if e > s {
                        f.input.replace_range(start_b..end_b, "");
                    }
                    let at = f
                        .input
                        .char_indices()
                        .nth(s)
                        .map(|(i, _)| i)
                        .unwrap_or(f.input.len());
                    f.input.insert(at, c);
                    f.caret = s + 1;
                    f.anchor = None;
                }
                self.apply_find_query();
                Action::None
            }
            // ctrl/alt chords are the app's business (select-all,
            // clipboard) or inert — never typed into the query
            _ => Action::None,
        }
    }

    /// The find field's selected span, normalized (start, end) in chars.
    fn find_sel_range(&self) -> (usize, usize) {
        let Some(f) = &self.find else {
            return (0, 0);
        };
        let len = f.input.chars().count();
        let caret = f.caret.min(len);
        let anchor = f.anchor.unwrap_or(caret).min(len);
        (caret.min(anchor), caret.max(anchor))
    }

    /// The selected text (None without a non-empty selection).
    pub fn find_selection_text(&self) -> Option<String> {
        let f = self.find.as_ref()?;
        let (s, e) = self.find_sel_range();
        if e <= s {
            return None;
        }
        let start = f
            .input
            .char_indices()
            .nth(s)
            .map(|(i, _)| i)
            .unwrap_or(f.input.len());
        let end = f
            .input
            .char_indices()
            .nth(e)
            .map(|(i, _)| i)
            .unwrap_or(f.input.len());
        Some(f.input[start..end].to_string())
    }

    /// Ctrl+A: the whole query selected (typing replaces it).
    pub fn find_select_all(&mut self) {
        if let Some(f) = &mut self.find {
            f.anchor = Some(0);
            f.caret = f.input.chars().count();
        }
    }

    /// Ctrl+X: the selection out — returned for the system clipboard.
    pub fn find_cut(&mut self) -> Option<String> {
        let text = self.find_selection_text()?;
        self.find_delete_selection();
        Some(text)
    }

    /// Ctrl+V: the clipboard text in, replacing the selection or
    /// inserting at the caret (newlines flatten — the field is one line).
    pub fn find_paste(&mut self, text: &str) {
        let flat = text.replace(['\n', '\r'], " ");
        {
            let Some(f) = &mut self.find else {
                return;
            };
            let len = f.input.chars().count();
            let caret = f.caret.min(len);
            let anchor = f.anchor.unwrap_or(caret).min(len);
            let (s, e) = (caret.min(anchor), caret.max(anchor));
            let start_b = f
                .input
                .char_indices()
                .nth(s)
                .map(|(i, _)| i)
                .unwrap_or(f.input.len());
            let end_b = f
                .input
                .char_indices()
                .nth(e)
                .map(|(i, _)| i)
                .unwrap_or(f.input.len());
            if e > s {
                f.input.replace_range(start_b..end_b, "");
            }
            let at = f
                .input
                .char_indices()
                .nth(s)
                .map(|(i, _)| i)
                .unwrap_or(f.input.len());
            f.input.insert_str(at, &flat);
            f.caret = s + flat.chars().count();
            f.anchor = None;
        }
        self.apply_find_query();
    }

    fn find_delete_selection(&mut self) {
        {
            let Some(f) = &mut self.find else {
                return;
            };
            let len = f.input.chars().count();
            let caret = f.caret.min(len);
            let anchor = f.anchor.unwrap_or(caret).min(len);
            let (s, e) = (caret.min(anchor), caret.max(anchor));
            if e > s {
                let start_b = f
                    .input
                    .char_indices()
                    .nth(s)
                    .map(|(i, _)| i)
                    .unwrap_or(f.input.len());
                let end_b = f
                    .input
                    .char_indices()
                    .nth(e)
                    .map(|(i, _)| i)
                    .unwrap_or(f.input.len());
                f.input.replace_range(start_b..end_b, "");
            }
            f.caret = s;
            f.anchor = None;
        }
        self.apply_find_query();
    }

    /// Push the popup's query into the buffer's search engine and jump to
    /// the first match at/after the caret. An empty query clears the
    /// pattern (an empty regex would match everywhere).
    fn apply_find_query(&mut self) {
        let Some(f) = self.find.as_ref() else { return };
        if f.input.is_empty() {
            self.buf.clear_search();
        } else if self.buf.set_search_pattern(&f.input) {
            self.buf.search_forward(true);
        }
    }

    /// Keys while the vim `/` prompt is open — always consumed. Typing
    /// searches live (the same engine the gui find drives: highlights
    /// appear and the caret rides the first match per keystroke), Enter
    /// LOCKS IN the pattern (no jump: cycling is `n`/`N`'s job), Esc
    /// cancels and clears.
    pub fn handle_vim_search_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                self.buf.clear_search();
                self.vim_search = None;
                Action::None
            }
            KeyCode::Enter => {
                // lock in: live typing already applied the pattern (an
                // empty one cleared it) — Enter closes the prompt and
                // leaves the caret alone; `n`/`N` do the cycling
                self.vim_search = None;
                Action::None
            }
            KeyCode::Backspace => {
                if let Some(q) = self.vim_search.as_mut() {
                    q.pop();
                }
                self.apply_vim_search();
                Action::None
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if let Some(q) = self.vim_search.as_mut() {
                    q.push(c);
                }
                self.apply_vim_search();
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Push the `/` prompt's query into the buffer's search engine (live:
    /// highlights + the caret ride per keystroke). An empty query clears
    /// the pattern (an empty regex would match everywhere).
    fn apply_vim_search(&mut self) {
        let Some(q) = self.vim_search.as_ref() else { return };
        if q.is_empty() {
            self.buf.clear_search();
        } else if self.buf.set_search_pattern(q) {
            self.buf.search_forward(true);
        }
    }

    /// Open/close the find popup. Closing hides the match bands (the
    /// pattern stays: `n`/`N` keep cycling) and keeps the cursor where the
    /// last match landed.
    pub fn toggle_find(&mut self) {
        match self.find.take() {
            None => {
                self.find = Some(FindState {
                    input: String::new(),
                    caret: 0,
                    anchor: None,
                })
            }
            Some(_) => self.close_find(),
        }
    }

    /// Close the find popup, dropping the match bands — a fresh query
    /// (find or `/`) re-shows them.
    pub fn close_find(&mut self) {
        self.find = None;
        self.buf.hide_search_hl();
    }

    pub fn find_open(&self) -> bool {
        self.find.is_some()
    }

    /// (current 1-based index, total) for the popup's "n of m" — the
    /// current match is the first at/after the caret, wrapping.
    pub fn find_position(&self) -> (usize, usize) {
        let spans = self.buf.search_match_spans();
        if spans.is_empty() {
            return (0, 0);
        }
        let cur = self.buf.pos();
        let idx = spans
            .iter()
            .position(|&(r, c, _)| (r, c) >= (cur.row, cur.col))
            .unwrap_or(0);
        (idx + 1, spans.len())
    }

    fn handle_insert_key(&mut self, key: KeyEvent) -> Action {
        // ctrl-f opens the find popup even in insert mode (IDE muscle
        // memory) — before the ctrl-chord swallow
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('f')
        {
            self.toggle_find();
            return Action::None;
        }
        // the app-wide select-all chord works in insert mode too
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('a')
        {
            self.buf.cancel_selection();
            self.buf.move_cursor(CursorMove::Jump(0, 0));
            self.buf.start_selection();
            let last = self.buf.line_count().saturating_sub(1);
            self.buf.move_cursor(CursorMove::Jump(last, usize::MAX));
            return Action::None;
        }
        // alt+arrows: word-wise SELECTION in insert mode too (the same
        // text-field semantics regular mode got) — before the chord
        // swallow, which would eat them
        if key.modifiers.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
        {
            if self.buf.selection_anchor().is_none() {
                self.buf.start_selection();
            }
            // the caret lands PAST the word's last char (the insertion
            // boundary — the band stops there)
            self.buf.move_cursor(CursorMove::WordEnd);
            if key.code == KeyCode::Right {
                self.buf.move_cursor(CursorMove::Forward);
            }
            return Action::None;
        }
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
                if self.buf.selection_anchor().is_some() {
                    // a highlight made in insert mode becomes a visual
                    // selection: escape lands in visual mode, a second
                    // escape clears it
                    self.mode = Mode::Visual;
                    self.visual_linewise = false;
                    return Action::None;
                }
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
            KeyCode::Char(c) if matches!(c, '(' | '[' | '{') => self.open_pair(c),
            KeyCode::Char(c) if matches!(c, ')' | ']' | '}') => self.type_over(c),
            KeyCode::Char(c) => {
                self.buf.insert_char(c);
                Action::Changed
            }
            KeyCode::Space => {
                self.buf.insert_char(' ');
                Action::Changed
            }
            KeyCode::Enter => self.smart_enter(),
            KeyCode::Backspace => {
                if !self.pair_backspace() {
                    self.buf.delete_char();
                }
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
            KeyCode::Home | KeyCode::End => {
                // shift+home/end selects to the line start/end (regular
                // mode's notepad behavior; shift+arrows do the same)
                if shift {
                    if self.buf.selection_anchor().is_none() {
                        self.buf.start_selection();
                    }
                } else {
                    self.buf.cancel_selection();
                }
                self.buf.move_cursor(match key.code {
                    KeyCode::Home => CursorMove::Head,
                    _ => CursorMove::End,
                });
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
        // escape is :noh — the search match bands drop (the pattern stays
        // for `n`/`N`) — and it also clears a mouse/shift-arrow highlight
        // made in NORMAL mode (visual mode has its own escape, which lands
        // in normal). A pending operator still gets the Esc first.
        if self.mode == Mode::Normal && key.code == KeyCode::Esc && self.pending.is_none() {
            self.buf.hide_search_hl();
            self.buf.cancel_selection();
            return Action::None;
        }
        // ctrl/alt-modified keys are never vim keys here — the app scrolls
        // on ctrl+d/u itself, and an operator armed from a modified letter
        // turned `ctrl+d … gg` into `dgg` (delete to the top). The lone
        // ctrl chord vim uses in normal mode is Ctrl+r (redo).
        // VISUAL mode runs no ctrl guard: every visual key acts immediately
        // (nothing arms an operator from one), and a ctrl letter with an
        // app-level binding (c/x/d/u/v…) never reaches this function — it
        // was intercepted upstream. Without this, a stale ctrl flag on `y`
        // (winit's tracked state after an eaten release, or the arbiter
        // restoring a stripped chord) killed visual yank while normal yy
        // survived.
        let m = key.modifiers;
        // Alt+k / Alt+j: move lines up/down (VS Code's alt+arrows) — the
        // cursor line in normal mode, every highlighted row (partial
        // highlights included) in visual, cursor/selection riding along.
        // The document edges are no-ops.
        if m.contains(KeyModifiers::ALT)
            && matches!(key.code, KeyCode::Char('k') | KeyCode::Char('j'))
        {
            return self.move_lines_by(key.code == KeyCode::Char('j'));
        }
        if m.contains(KeyModifiers::ALT)
            || (m.contains(KeyModifiers::CONTROL)
                && self.mode == Mode::Normal
                && !matches!(
                    key.code,
                    KeyCode::Char('r')
                        | KeyCode::Char('o')
                        | KeyCode::Char('i')
                        | KeyCode::Char('y')
                ))
        {
            return Action::None;
        }
        // pending sequences run in every mode, so viw/S… work from visual
        if let Some(p) = self.pending.take() {
            // a ctrl chord never completes a replace-pending ('r' waits for
            // a literal char). Desktop environments that re-inject a
            // stripped copy of ctrl-chord letters made slow left-ctrl+r arm
            // replace and then complete with 'r' — salvage the intended
            // chord instead.
            if p == Pending::R && key.modifiers.contains(KeyModifiers::CONTROL) {
                if key.code == KeyCode::Char('r') {
                    let n = self.count.take().unwrap_or(1).max(1);
                    for _ in 0..n {
                        self.buf.redo();
                    }
                    return Action::Changed;
                }
                return Action::None; // cancel the pending replace, swallow
            }
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
                // gvim-style: shift+arrows highlight in visual mode
                if self.buf.selection_anchor().is_none() {
                    self.buf.start_selection();
                }
                self.visual_linewise = false;
                self.mode = Mode::Visual;
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
        let action = self.dispatch_normal_match(key, m);
        // vim never rests the cursor on the document's EOL marker: the
        // empty last row a trailing newline stores is not a line, so a
        // motion that landed there (j past the end, a search, a jump)
        // clamps back onto the last content line — dd then operates on
        // the line the cursor visibly sits on, never the brace above an
        // empty-looking row.
        if self.mode == Mode::Normal {
            let (row, col) = self.buf.cursor();
            let marker_row = self.buf.line_count().saturating_sub(1);
            let has_marker =
                marker_row > 0 && self.buf.line(marker_row).is_empty();
            if has_marker && row == marker_row {
                let last = self.buf.last_content_row();
                let col = col.min(self.buf.line_len(last));
                self.buf.move_cursor(CursorMove::Jump(last, col));
            }
        }
        action
    }

    fn dispatch_normal_match(&mut self, key: KeyEvent, m: KeyModifiers) -> Action {
        match key.code {
            KeyCode::Delete => self.x_delete(),
            // jumplist navigation (vim: Ctrl+O back, Ctrl+I forward; Tab
            // doubles as Ctrl+I in terminals but is taken by pane logic)
            KeyCode::Char('o') if m.contains(KeyModifiers::CONTROL) => {
                self.jump_back();
                Action::None
            }
            KeyCode::Char('i') if m.contains(KeyModifiers::CONTROL) => {
                self.jump_forward();
                Action::None
            }
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
                self.buf.open_line_below();
                self.mode = Mode::Insert;
                Action::Changed
            }
            KeyCode::Char('O') => {
                self.buf.open_line_above();
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
            // vim S: substitute [count] lines — cc's linewise change
            KeyCode::Char('S') => {
                let rep = self.count.take().unwrap_or(1).max(1) as usize;
                self.yank_lines(rep, false);
                self.change_lines_seeded(rep);
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
            // Y (shift+y): vim's Y == yy — linewise yank of [count] lines,
            // executed immediately (not an operator)
            KeyCode::Char('Y') => {
                let n = self.count.take().unwrap_or(1).max(1) as usize;
                let (row, _) = self.buf.cursor();
                let n = n.min(self.buf.lines().len().saturating_sub(row)).max(1);
                self.flash_yank((row, 0), (row + n, 0));
                if std::env::var("DBML_UI_FLASH_DEBUG").is_ok() {
                    eprintln!("[flash] set by yy: rows {row}..{} n={n}", row + n);
                }
                self.yank_lines(n, true);
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
                // vim's /: the bottom command line (the gui find stays on
                // ctrl+f / the toolbar button)
                self.vim_search = Some(String::new());
                Action::None
            }
            KeyCode::Char('n') => {
                self.record_jump();
                self.buf.search_forward(false);
                Action::None
            }
            KeyCode::Char('N') => {
                self.record_jump();
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
                    self.flash_yank((from, 0), (from + n, 0));
                    self.buf.move_cursor(CursorMove::Jump(from, 0));
                    self.yank_lines(n, true);
                }
                self.buf.cancel_selection();
                self.mode = Mode::Normal;
                Action::None
            }
            KeyCode::Char('y') => {
                let range = self.buf.selection_range();
                if let Some(t) = self.buf.copy_selection() {
                    self.yank_chars(t);
                    if let Some((from, to)) = range {
                        self.flash_yank((from.row, from.col), (to.row, to.col));
                    }
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
                    self.change_lines_seeded(n);
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
            KeyCode::Char('g') => {
                // gc (toggle comments over the selection); gg lands on top
                self.pending = Some(Pending::G);
                Action::None
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                // vim: visual p/P replaces the selection with the register
                self.paste_over_selection()
            }
            KeyCode::Char('r') => {
                // vim: visual r{char} replaces every selected char — the
                // char completes through the shared Pending::R sequence,
                // which branches on the mode
                self.pending = Some(Pending::R);
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
            KeyCode::Char('G') => {
                self.record_jump();
                match count {
                    Some(line) => {
                        let row = (line as usize)
                            .saturating_sub(1)
                            .min(self.buf.last_content_row());
                        self.buf.move_cursor(CursorMove::Jump(row, 0));
                    }
                    // the last line is the last CONTENT line — the EOL
                    // marker row below it is not a cursor destination
                    None => self
                        .buf
                        .move_cursor(CursorMove::Jump(self.buf.last_content_row(), 0)),
                }
            }
            // n$ moves to the end of the (n-1)th line below; the aim goes
            // MAXCOL so following j/k ride every line's end (vim curswant)
            KeyCode::Char('$') | KeyCode::End => {
                for _ in 1..n {
                    self.buf.move_cursor(CursorMove::Down);
                }
                self.buf.move_cursor(CursorMove::EndMax);
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
            // paragraph motions are jumps (vim's jumplist includes them)
            KeyCode::Char('{') => {
                self.record_jump();
                self.repeat(CursorMove::ParagraphBack, n)
            }
            KeyCode::Char('}') => {
                self.record_jump();
                self.repeat(CursorMove::ParagraphForward, n)
            }
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
                // visual r{char} replaces the whole selection (vim);
                // normal r{char} replaces [count] chars at the cursor
                if self.mode == Mode::Visual {
                    if self.visual_replace_selection(c) {
                        Action::Changed
                    } else {
                        Action::None
                    }
                } else if self.buf.replace_next(rep.max(1), c) {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            (R, _) => Action::None,
            (G, KeyCode::Char('d')) => {
                // gd — the app resolves the definition (needs the schema)
                Action::GotoDefinition
            }
            (G, KeyCode::Char('g')) => {
                // [count]gg jumps to line N
                self.record_jump();
                if let Some(line) = self.count.or(self.op_count) {
                    let rows = self.buf.lines().len().max(1);
                    let row = (line as usize).saturating_sub(1).min(rows - 1);
                    self.buf.move_cursor(CursorMove::Jump(row, 0));
                } else {
                    self.buf.move_cursor(CursorMove::Jump(0, 0));
                }
                Action::None
            }
            (G, KeyCode::Char('c')) => {
                // gc: in visual, toggle EVERY highlighted line now —
                // uncomment only when all of them carry `//`, else comment
                // them all; in normal, await gcc's second `c`
                if self.mode == Mode::Visual {
                    if let Some((a, b)) = self.buf.selection_range() {
                        let first = a.row.min(b.row);
                        let last = a.row.max(b.row);
                        self.buf.toggle_comment_rows(first, last);
                    }
                    self.buf.cancel_selection();
                    self.mode = Mode::Normal;
                    self.visual_linewise = false;
                    Action::Changed
                } else {
                    self.pending = Some(Pending::Gc);
                    Action::None
                }
            }
            (Gc, KeyCode::Char('c')) => {
                // gcc: toggle the current line's // comment
                let row = self.buf.pos().row;
                self.buf.toggle_line_comment(row);
                Action::Changed
            }
            (Y, KeyCode::Char('y') | KeyCode::Char('Y')) => {
                // yy / Y: [count] whole lines, flash the rows
                let (row, _) = self.buf.cursor();
                let n = rep.min(self.buf.lines().len().saturating_sub(row));
                self.flash_yank((row, 0), (row + n, 0));
                self.yank_lines(n, true);
                Action::None
            }
            (Y, KeyCode::Char('g')) => {
                self.pending = Some(Yg);
                Action::None
            }
            // yj / yk: linewise to the adjacent line, cursor RESTORED
            (Y, KeyCode::Char('j') | KeyCode::Down) => {
                let (row, col) = self.buf.cursor();
                let target = (row + rep).min(self.buf.lines().len().saturating_sub(1));
                self.flash_yank((row, 0), (target + 1, 0));
                self.yank_lines(target - row + 1, true);
                self.buf.move_cursor(CursorMove::Jump(row, col));
                Action::None
            }
            (Y, KeyCode::Char('k') | KeyCode::Up) => {
                let (row, col) = self.buf.cursor();
                let target = row.saturating_sub(rep);
                self.flash_yank((target, 0), (row + 1, 0));
                self.buf.move_cursor(CursorMove::Jump(target, 0));
                self.yank_lines(row - target + 1, true);
                self.buf.move_cursor(CursorMove::Jump(row, col));
                Action::None
            }
            // yw / ye / yb / y0 / y$: charwise, cursor RESTORED (vim's y
            // does not move); same range math as the d counterparts
            (Y, KeyCode::Char('w')) => {
                self.yank_over(&vec![CursorMove::WordForward; rep], false, true)
            }
            (Y, KeyCode::Char('e')) => self.yank_over(&vec![CursorMove::WordEnd; rep], true, false),
            (Y, KeyCode::Char('b')) => self.yank_over(&vec![CursorMove::WordBack; rep], false, false),
            (Y, KeyCode::Char('0') | KeyCode::Home | KeyCode::Char('^')) => {
                self.yank_over(&[CursorMove::Head], false, false)
            }
            (Y, KeyCode::Char('$') | KeyCode::End) => {
                let mut ms = vec![CursorMove::Down; rep.saturating_sub(1)];
                ms.push(CursorMove::End);
                self.yank_over(&ms, false, false)
            }
            // yG: linewise to the last line — the range starts at the
            // cursor, so no jump is needed; y does not move the cursor
            (Y, KeyCode::Char('G')) => {
                let (row, _) = self.buf.cursor();
                let last = self.buf.lines().len().saturating_sub(1);
                self.flash_yank((row, 0), (last + 1, 0));
                self.yank_lines(last - row + 1, true);
                Action::None
            }
            (D, KeyCode::Char('d')) => {
                self.yank_lines(rep, false);
                self.delete_lines(rep);
                Action::Changed
            }
            (C, KeyCode::Char('c')) => {
                // [count]cc deletes the lines and edits one line seeded
                // with the formatter's indent (one undo unit: insert_str
                // snapshots before the whole change; delete_range is raw)
                self.yank_lines(rep, false);
                self.change_lines_seeded(rep);
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
            // dl / cl: [count] chars forward, never crossing the line end
            // (vim's l motion clamps there, so the newline is safe). cl
            // edits in place (cl ≈ s) even when there is nothing to delete.
            (D, KeyCode::Char('l')) => {
                if self.delete_chars_ahead(rep) {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            (C, KeyCode::Char('l')) => {
                let changed = self.delete_chars_ahead(rep);
                self.mode = Mode::Insert;
                if changed {
                    Action::Changed
                } else {
                    Action::None
                }
            }
            // dj / dk: linewise to the adjacent line — [count] multiplies
            // the motion's line distance (d3j deletes 4 lines), clamped at
            // the buffer ends (vim: dj on the last line deletes just the
            // current line)
            (D, KeyCode::Char('j') | KeyCode::Down) => {
                let (row, _) = self.buf.cursor();
                let target = (row + rep).min(self.buf.lines().len().saturating_sub(1));
                self.operate_lines(target, false)
            }
            (D, KeyCode::Char('k') | KeyCode::Up) => {
                let (row, _) = self.buf.cursor();
                self.operate_lines(row.saturating_sub(rep), false)
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
            // cj / ck: the change counterpart of dj/dk (one seeded line)
            (C, KeyCode::Char('j') | KeyCode::Down) => {
                let (row, _) = self.buf.cursor();
                let target = (row + rep).min(self.buf.lines().len().saturating_sub(1));
                self.operate_lines(target, true)
            }
            (C, KeyCode::Char('k') | KeyCode::Up) => {
                let (row, _) = self.buf.cursor();
                self.operate_lines(row.saturating_sub(rep), true)
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
            (Yg, KeyCode::Char('g')) => {
                // ygg: linewise to the first line, cursor restored
                let (row, col) = self.buf.cursor();
                let target = self.line_target_row(true);
                self.flash_yank((target, 0), (row + 1, 0));
                self.buf.move_cursor(CursorMove::Jump(target, 0));
                self.yank_lines(row - target + 1, true);
                self.buf.move_cursor(CursorMove::Jump(row, col));
                Action::None
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
                linewise: true,
                start: Pos { row: from, col: 0 },
                end: Pos {
                    row: end_row,
                    col: self.buf.line(end_row).chars().count().saturating_sub(1),
                },
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
            // cG/cgg: one editable line seeded with the formatter's
            // indent, one undo unit
            self.change_lines_seeded(n);
            self.mode = Mode::Insert;
        } else {
            self.delete_lines(n);
        }
        Action::Changed
    }

    /// The indentation a linewise change should seed the fresh line with:
    /// what the formatter would indent the line to (2-space unit, see
    /// src/dbml/fmt.rs) without running it. The nearest non-blank line
    /// above the replaced range decides: a block opener (`{` at end after
    /// stripping a `//` comment) means one level deeper (its indent + 2);
    /// otherwise that neighbor's indent carries over; with no neighbor
    /// above, the replaced line's own leading whitespace. Empty for a
    /// top-level line, so nothing is seeded.
    fn linewise_change_indent(&self, row: usize) -> String {
        let lines = self.buf.lines();
        let indent_of = |l: &str| l.chars().count() - l.trim_start().chars().count();
        let mut prev = None;
        let mut r = row;
        while r > 0 {
            r -= 1;
            if let Some(l) = lines.get(r) {
                if !l.trim().is_empty() {
                    prev = Some(l.as_str());
                    break;
                }
            }
        }
        match prev {
            Some(l) => {
                let code = l.split("//").next().unwrap_or("").trim_end();
                " ".repeat(indent_of(l) + usize::from(code.ends_with('{')) * 2)
            }
            None => match lines.get(row) {
                Some(l) => " ".repeat(indent_of(l)),
                None => String::new(),
            },
        }
    }

    /// Linewise change (`cc`, `S`, visual-line `c`, `cG`): clear `n` rows
    /// into one editable line seeded with `linewise_change_indent` (the
    /// formatter re-indents the line anyway), cursor after the seed. Still
    /// one undo unit without touching the Buffer's private history:
    /// `insert_str` takes the single pre-change snapshot and `delete_range`
    /// never pushes, so deleting from the seed's end through the last
    /// replaced row's EOL leaves exactly the seed on the fresh line — the
    /// same shape `change_lines` leaves. An empty seed falls back to the
    /// plain `change_lines` path.
    fn change_lines_seeded(&mut self, n: usize) {
        let row = self.buf.cursor().0;
        let indent = self.linewise_change_indent(row);
        if indent.is_empty() {
            self.buf.change_lines(n);
            return;
        }
        self.buf.cancel_selection();
        self.buf.move_cursor(CursorMove::Jump(row, 0));
        self.buf.insert_str(&indent);
        let end_row = (row + n.max(1) - 1).min(self.buf.lines().len() - 1);
        let end_col = self.buf.line(end_row).chars().count();
        self.buf.delete_range(
            Pos {
                row,
                col: indent.chars().count(),
            },
            Pos {
                row: end_row,
                col: end_col,
            },
        );
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
        if self.delete_chars_ahead(n) {
            Action::Changed
        } else {
            Action::None
        }
    }

    /// Delete up to `n` chars forward from the cursor, clamped to the line
    /// end (x/s semantics): true when anything was deleted. Shared by `x`
    /// and the `dl`/`cl` operator forms.
    fn delete_chars_ahead(&mut self, n: usize) -> bool {
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
            true
        } else {
            false
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
        self.register_linewise = true;
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
        // vim: deleting through the END of the buffer. A trailing-newline
        // document stores that newline as an empty last line — the file's
        // EOL marker, not a line: the range then stops short of it (dd on
        // the last line keeps the file's trailing newline; dd with the
        // cursor ON the marker deletes the last content line, the marker
        // surviving as its terminator; dG from the first line empties the
        // buffer). Without a marker, the newline ending the previous line
        // goes with the range so the last line is removed outright.
        let (start_row, start_col, total) = if end == lines.len()
            && lines.len() > 1
            && lines[lines.len() - 1].is_empty()
        {
            let first = row.min(lines.len() - 2);
            let sum: usize = lines[first..lines.len() - 1]
                .iter()
                .map(|l| l.chars().count() + 1)
                .sum();
            (first, 0, sum + usize::from(first == 0))
        } else if end == lines.len() && row > 0 {
            let sum: usize =
                lines[row..end].iter().map(|l| l.chars().count() + 1).sum();
            (row - 1, lines[row - 1].chars().count(), sum)
        } else {
            (row, 0, lines[row..end].iter().map(|l| l.chars().count() + 1).sum())
        };
        self.buf.move_cursor(CursorMove::Jump(start_row, start_col));
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
        let Some((from, n)) = self.operator_range(motions, inclusive, clamp_line) else {
            return Action::None;
        };
        self.buf.move_cursor(CursorMove::Jump(from.0, from.1));
        self.buf.delete_str(n);
        if insert {
            self.mode = Mode::Insert;
        }
        Action::Changed
    }

    /// Apply the operator's motions and normalize the covered range:
    /// (start position, character count). The cursor ends wherever the
    /// motions landed — the caller deletes, yanks, or restores.
    fn operator_range(
        &mut self,
        motions: &[CursorMove],
        inclusive: bool,
        clamp_line: bool,
    ) -> Option<((usize, usize), usize)> {
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
        (n > 0).then_some((from, n))
    }

    /// The `n` characters starting at `from` (newlines count as one, the
    /// same accounting as `chars_between`), plus the exclusive end position.
    fn text_from(&self, from: (usize, usize), n: usize) -> (String, (usize, usize)) {
        let lines = self.buf.lines();
        let mut out = String::new();
        let (mut r, mut c) = from;
        for _ in 0..n {
            match lines.get(r) {
                Some(line) if c < line.chars().count() => {
                    out.push(line.chars().nth(c).unwrap());
                    c += 1;
                }
                Some(_) => {
                    out.push('\n');
                    r += 1;
                    c = 0;
                }
                None => break,
            }
        }
        (out, (r, c))
    }

    /// Every yank hides the search match bands (an edit-class action reads
    /// as :noh) while flashing the yanked span.
    fn flash_yank(&mut self, start: (usize, usize), end: (usize, usize)) {
        self.buf.hide_search_hl();
        self.yank_flash = Some(YankFlash {
            start,
            end,
            started: std::time::Instant::now(),
        });
    }

    /// Yank (never delete) the range an operator covers: the text lands in
    /// the register + system clipboard, the cursor is RESTORED (vim's y
    /// does not move), and the yanked region flashes.
    fn yank_over(&mut self, motions: &[CursorMove], inclusive: bool, clamp_line: bool) -> Action {
        let start = self.buf.cursor();
        let Some((from, n)) = self.operator_range(motions, inclusive, clamp_line) else {
            self.buf.move_cursor(CursorMove::Jump(start.0, start.1));
            return Action::None;
        };
        let (text, end) = self.text_from(from, n);
        self.yank_chars(text);
        self.flash_yank(from, end);
        self.buf.move_cursor(CursorMove::Jump(start.0, start.1));
        Action::None
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

    /// Visual p/P: replace the selected text with the register's
    /// contents — one undo unit, vim's register swap (the replaced text
    /// becomes the unnamed register), and the pasted range is left
    /// selected with the cursor at its end.
    /// Alt+k/j: move the cursor line (normal) or every highlighted row
    /// (visual) up/down one line, cursor/selection riding along. None at
    /// the document edges.
    fn move_lines_by(&mut self, down: bool) -> Action {
        match self.mode {
            Mode::Visual => {
                let Some((a, b)) = self.buf.selection_range() else {
                    return Action::None;
                };
                let first = a.row.min(b.row);
                let last = a.row.max(b.row);
                if !self.buf.move_lines(first, last, down) {
                    return Action::None;
                }
                // the selection rides with the lines (the text of each
                // line is unchanged — only the order — so the columns hold)
                let (ar, ac) = match self.buf.selection_anchor() {
                    Some(p) => (p.row, p.col),
                    None => return Action::None,
                };
                let (cr, cc) = self.buf.cursor();
                let d = if down { 1 } else { -1 };
                self.buf.cancel_selection();
                self.buf.move_cursor(CursorMove::Jump(
                    (ar as i32 + d) as usize,
                    ac,
                ));
                self.buf.start_selection();
                self.buf.move_cursor(CursorMove::Jump(
                    (cr as i32 + d) as usize,
                    cc,
                ));
                Action::Changed
            }
            _ => {
                let row = self.buf.pos().row;
                if !self.buf.move_lines(row, row, down) {
                    return Action::None;
                }
                let col = self.buf.cursor().1;
                let to = (row as i32 + if down { 1 } else { -1 }) as usize;
                self.buf.move_cursor(CursorMove::Jump(to, col));
                Action::Changed
            }
        }
    }

    fn paste_over_selection(&mut self) -> Action {
        if self.register.is_empty() {
            return Action::None;
        }
        let Some((a, b)) = self.buf.selection_range() else {
            return Action::None;
        };
        let (start, end) = if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        let first = start.row.min(end.row);
        let last = start.row.max(end.row);
        // the PASTE keeps the register's own kind (linewise register ->
        // linewise rows); vim's swap then moves the replaced text into
        // the register with the visual mode's kind
        let pasted_linewise = self.register_linewise;
        let pasted_lines = std::mem::take(&mut self.register);
        let replaced = self.buf.selection_text().unwrap_or_default();
        self.register = if replaced.is_empty() {
            vec![String::new()]
        } else {
            replaced.lines().map(|l| l.to_string()).collect()
        };
        self.register_linewise = self.visual_linewise;
        // the raw ops delete any active selection first: clear it so the
        // v-highlight is not eaten mid-paste
        self.buf.cancel_selection();
        self.buf.begin_edit(); // one undo unit
        if pasted_linewise {
            // replace the WHOLE rows first..=last with the register lines
            if last + 1 < self.buf.lines().len() {
                self.buf.delete_range(
                    Pos { row: first, col: 0 },
                    Pos { row: last + 1, col: 0 },
                );
            } else {
                // the range ends the buffer: eat the separator before it
                let from = if first > 0 {
                    Pos {
                        row: first - 1,
                        col: self.buf.line_len(first - 1),
                    }
                } else {
                    Pos { row: 0, col: 0 }
                };
                self.buf
                    .delete_range(from, Pos { row: last, col: self.buf.line_len(last) });
            }
            self.buf.move_cursor(CursorMove::Jump(first, 0));
            for l in pasted_lines.iter() {
                // a linewise line carries its own newline: insert the text
                // then split the remainder onto the next row
                self.buf.insert_str_raw(l);
                self.buf.insert_newline_raw();
            }
            // vim: the cursor rests on the first pasted row, NORMAL mode —
            // no selection lingers, so motions never extend a highlight
            self.mode = Mode::Normal;
            self.visual_linewise = false;
        } else {
            // charwise: delete the selection, insert at the cursor, leave
            // the cursor ON the pasted text's last char (vim's resting
            // spot) in NORMAL mode — re-selecting the pasted range here
            // kept visual alive, so the next motion extended a highlight
            let pasted = pasted_lines.join("\n");
            self.buf.delete_range(start, end);
            self.buf.move_cursor(CursorMove::Jump(start.row, start.col));
            self.insert_multiline_raw(&pasted);
            let pl: Vec<&str> = pasted.split('\n').collect();
            let end_pos = if pl.len() == 1 {
                Pos {
                    row: start.row,
                    col: start.col + pl[0].chars().count().saturating_sub(1),
                }
            } else {
                Pos {
                    row: start.row + pl.len() - 1,
                    col: pl.last().map(|l| l.chars().count().saturating_sub(1)).unwrap_or(0),
                }
            };
            self.buf.move_cursor(CursorMove::Jump(end_pos.row, end_pos.col));
            self.mode = Mode::Normal;
            self.visual_linewise = false;
        }
        Action::Changed
    }

    /// Visual `r{char}` (vim): replace EVERY selected character with
    /// `{char}` — per row the selected span becomes that many copies; an
    /// empty selected row receives one `{char}` (vim). Linewise (V)
    /// selections replace the whole rows. One undo unit; the cursor
    /// rests on the last replaced character, normal mode.
    fn visual_replace_selection(&mut self, c: char) -> bool {
        let Some((a, b)) = self.buf.selection_range() else {
            return false;
        };
        let (start, end) = if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        };
        // linewise (V): the whole rows are the span (vim replaces every
        // character of every selected row)
        let (start, end) = if self.visual_linewise {
            let last = end.row.max(start.row);
            (
                Pos { row: start.row, col: 0 },
                Pos {
                    row: last,
                    col: self.buf.line_len(last),
                },
            )
        } else {
            (start, end)
        };
        let fill = c.to_string();
        self.buf.cancel_selection();
        self.mode = Mode::Normal;
        self.visual_linewise = false;
        self.buf.begin_edit(); // one undo unit
        let mut last = (start.row, 0usize);
        for (i, row) in (start.row..=end.row).enumerate() {
            // the selected span on this row: charwise selections start at
            // start.col, end at end.col on the LAST row and run to EOL in
            // between; linewise spans are whole rows
            let (a, b) = if start.row == end.row {
                (start.col, end.col)
            } else if i == 0 {
                (start.col, self.buf.line_len(row))
            } else if row == end.row {
                (0, end.col)
            } else {
                (0, self.buf.line_len(row))
            };
            // an empty selected row still receives one {char} (vim)
            let count = b.saturating_sub(a).max(1);
            self.buf.move_cursor(CursorMove::Jump(row, a));
            self.buf.delete_range(Pos { row, col: a }, Pos { row, col: b });
            self.buf.insert_str_raw(&fill.repeat(count));
            last = (row, count - 1);
        }
        self.buf.move_cursor(CursorMove::Jump(last.0, last.1));
        true
    }

    /// Insert `text` at the cursor as raw (history-free) ops, with
    /// newlines split into real line breaks — `insert_str_raw` only
    /// writes into the current line.
    fn insert_multiline_raw(&mut self, text: &str) {
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.buf.insert_newline_raw();
            }
            if !line.is_empty() {
                self.buf.insert_str_raw(line);
            }
        }
    }

    // ---- IDE editing (shared by vim insert mode and Regular typing) ----

    /// The closer for an auto-paired opener.
    fn pair_close(open: char) -> char {
        match open {
            '(' => ')',
            '[' => ']',
            _ => '}',
        }
    }

    /// Typing an opener (`(`, `[`, `{`): with a selection, WRAP it
    /// (`(text)`, cursor after the closer — VS Code's behavior); without
    /// one, insert the pair with the caret between. One undo unit.
    pub fn open_pair(&mut self, open: char) -> Action {
        let close = Self::pair_close(open);
        let (row, col) = (self.buf.pos().row, self.buf.pos().col);
        self.buf.insert_str(&format!("{open}{close}"));
        // absolute jump: Move::Back would wrap at column 0
        self.buf.move_cursor(CursorMove::Jump(row, col + 1));
        Action::Changed
    }

    /// Typing a closer (`)`, `]`, `}`): when the caret sits on the
    /// matching closer, step OVER it instead of inserting a twin; else
    /// insert normally.
    pub fn type_over(&mut self, close: char) -> Action {
        let (row, col) = (self.buf.pos().row, self.buf.pos().col);
        if self.buf.line(row).chars().nth(col) == Some(close) {
            self.buf.move_cursor(CursorMove::Forward);
            Action::None
        } else {
            self.buf.insert_char(close);
            Action::Changed
        }
    }

    /// Backspace between an empty pair deletes BOTH chars (IDE standard
    /// companion to auto-pairs); returns false for the caller to fall
    /// back to the plain backspace.
    pub fn pair_backspace(&mut self) -> bool {
        let (row, col) = (self.buf.pos().row, self.buf.pos().col);
        if col == 0 {
            return false;
        }
        let line = self.buf.line(row);
        let opens = |c: char| matches!(c, '(' | '[' | '{');
        let is_pair = line
            .chars()
            .nth(col - 1)
            .filter(|p| opens(*p))
            .and_then(|p| line.chars().nth(col).map(|n| n == Self::pair_close(p)))
            .unwrap_or(false);
        if !is_pair {
            return false;
        }
        self.buf.begin_edit(); // one undo unit restores both
        self.buf
            .delete_range(Pos { row, col: col - 1 }, Pos { row, col: col + 1 });
        self.buf.move_cursor(CursorMove::Jump(row, col - 1));
        true
    }

    /// IDE Enter: the new line inherits the cursor line's indentation; a
    /// `{` before the caret opens a block one level deeper (the 2-space
    /// unit `linewise_change_indent` seeds), and a `}` right after the
    /// caret EXPANDS the braces across three lines with the caret on the
    /// indented middle line. One undo unit.
    pub fn smart_enter(&mut self) -> Action {
        let (row, col) = (self.buf.pos().row, self.buf.pos().col);
        let line = self.buf.line(row).to_string();
        let before: String = line.chars().take(col).collect();
        let after: String = line.chars().skip(col).collect();
        let base: String = line.chars().take_while(|ch| ch.is_whitespace()).collect();
        // a `//` comment tail never opens a block (linewise_change_indent's
        // rule)
        let code = before.split("//").next().unwrap_or("").trim_end().to_string();
        let opens = code.ends_with('{');
        let closes_after = after.trim_start().starts_with('}');
        let unit = "  ";
        self.buf.begin_edit(); // one undo unit
        match (opens, closes_after) {
            (true, true) => {
                // {  |  }  →  { ⏎ <base+unit>| ⏎ <base>}
                self.buf.insert_newline_raw();
                self.buf.insert_str_raw(&format!("{base}{unit}"));
                self.buf.insert_newline_raw();
                self.buf.insert_str_raw(&base);
                self.buf.move_cursor(CursorMove::Jump(
                    row + 1,
                    base.chars().count() + unit.chars().count(),
                ));
            }
            (true, false) => {
                // {  |  …  →  { ⏎ <base+unit>|
                self.buf.insert_newline_raw();
                self.buf.insert_str_raw(&format!("{base}{unit}"));
            }
            _ => {
                // plain auto-indent: the new line inherits the indent
                self.buf.insert_newline_raw();
                if !base.is_empty() {
                    self.buf.insert_str_raw(&base);
                }
            }
        }
        Action::Changed
    }


    fn paste_n(&mut self, before: bool, times: u32) -> Action {
        if self.register.is_empty() {
            return Action::None;
        }
        let times = times.max(1);
        if self.register_linewise {
            // vim: a put is ONE undo unit — a single snapshot up front,
            // then raw (history-free) mutations; u undoes the whole paste
            // at once and restores the pre-put cursor
            self.buf.begin_edit();
            // linewise put: duplicate the block below (p) / above (P) the
            // cursor line; the cursor lands on the first pasted line's
            // first non-blank (verified against real nvim)
            let (row, _) = self.buf.cursor();
            let first = if before { row } else { row + 1 };
            // p opens the slot at the END of the cursor line; P at its start
            self.buf.move_cursor(CursorMove::Jump(row, 0));
            if !before {
                self.buf.move_cursor(CursorMove::End);
            }
            self.buf.insert_newline_raw();
            let mut at = first;
            for c in 0..times {
                if c > 0 {
                    at += self.register.len();
                    self.buf.move_cursor(CursorMove::Jump(at, 0));
                    self.buf.move_cursor(CursorMove::End);
                    self.buf.insert_newline_raw();
                }
                // insert_newline_raw leaves the cursor on the remainder
                // line — the block goes into the OPENED slot
                self.buf.move_cursor(CursorMove::Jump(at, 0));
                for (i, line) in self.register.iter().enumerate() {
                    if i > 0 {
                        self.buf.insert_newline_raw();
                    }
                    if !line.is_empty() {
                        self.buf.insert_str_raw(line);
                    }
                }
            }
            // cursor: the first pasted line, its first non-blank column
            let nb = self.register[0]
                .chars()
                .take_while(|c| c.is_whitespace())
                .count();
            self.buf.move_cursor(CursorMove::Jump(first, nb));
            Action::Changed
        } else {
            // charwise put: inline after (p) / before (P) the cursor char;
            // the cursor ends ON the last pasted character (nvim-verified)
            let text = self.register.join("\n").repeat(times as usize);
            if text.is_empty() {
                return Action::None;
            }
            self.buf.begin_edit();
            if !before {
                self.buf.move_cursor(CursorMove::Forward);
            }
            let mut last: Option<Pos> = None;
            for (i, part) in text.split('\n').enumerate() {
                if i > 0 {
                    self.buf.insert_newline_raw();
                }
                if !part.is_empty() {
                    self.buf.insert_str_raw(part);
                    let (r, c) = self.buf.cursor();
                    last = Some(Pos { row: r, col: c.saturating_sub(1) });
                }
            }
            if let Some(p) = last {
                self.buf.move_cursor(CursorMove::Jump(p.row, p.col));
            }
            Action::Changed
        }
    }

    /// Move the cursor to a (row, col) in buffer coordinates (mouse click).
    /// Disarm any pending operator/count (a flagged ctrl+d/u handled at
    /// the app level implies its stripped twin may have armed one).
    pub fn cancel_pending(&mut self) {
        self.pending = None;
        self.count = None;
        self.op_count = None;
    }

    /// Capture the cursor position as a jump origin (call BEFORE a jump
    /// motion moves the cursor). Dedupes consecutive records; discards the
    /// forward stack like vim's jumplist; capped at 100.
    pub fn record_jump(&mut self) {
        let pos = self.buf.pos();
        if self.jumps_past.last() == Some(&pos) {
            self.jumps_future.clear();
            return;
        }
        self.jumps_past.push(pos);
        if self.jumps_past.len() > 100 {
            self.jumps_past.remove(0);
        }
        self.jumps_future.clear();
    }

    /// Ctrl+O: back one jump — the current position goes to the future
    /// stack so Ctrl+I can return. True when the cursor moved.
    pub fn jump_back(&mut self) -> bool {
        let Some(pos) = self.jumps_past.pop() else {
            return false;
        };
        self.jumps_future.insert(0, self.buf.pos());
        self.buf.move_cursor(CursorMove::Jump(pos.row, pos.col));
        true
    }

    /// Ctrl+I: forward one jump (undo one Ctrl+O). True when moved.
    pub fn jump_forward(&mut self) -> bool {
        let Some(pos) = self.jumps_future.first().copied() else {
            return false;
        };
        self.jumps_future.remove(0);
        self.jumps_past.push(self.buf.pos());
        self.buf.move_cursor(CursorMove::Jump(pos.row, pos.col));
        true
    }

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
    fn auto_pairs_wrap_the_caret_and_type_over() {
        let mut e = editor("");
        e.handle_key(key('i'));
        e.handle_key(key('('));
        assert_eq!(e.buf.line(0), "()");
        assert_eq!(e.buf.pos(), Pos { row: 0, col: 1 }, "caret between");
        e.handle_key(key('['));
        assert_eq!(e.buf.line(0), "([])");
        assert_eq!(e.buf.pos(), Pos { row: 0, col: 2 }, "nested pair");
        e.handle_key(key(']'));
        e.handle_key(key(')'));
        assert_eq!(e.buf.line(0), "([])", "typed closers skip over");
        assert_eq!(e.buf.pos(), Pos { row: 0, col: 4 });
        // a closer with nothing to skip inserts normally
        e.handle_key(key(')'));
        assert_eq!(e.buf.line(0), "([]))");
    }

    #[test]
    fn pair_backspace_deletes_both() {
        let mut e = editor("");
        e.handle_key(key('i'));
        e.handle_key(key('('));
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(e.buf.line(0), "", "the empty pair collapses whole");
        // a non-pair backspace stays plain
        e.handle_key(key('('));
        e.handle_key(key('a'));
        e.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(e.buf.line(0), "()");
    }

    #[test]
    fn smart_enter_expands_braces() {
        let mut e = editor("Table users {}");
        e.buf.move_cursor(CursorMove::Jump(0, 13)); // between { }
        e.handle_key(key('i'));
        e.handle_key(enter());
        assert_eq!(
            e.buf.lines(),
            vec!["Table users {".to_string(), "  ".to_string(), "}".to_string()]
        );
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 2 }, "caret on the inner line");
        e.handle_key(esc());
        // one undo reverts the whole expansion
        e.handle_key(key('u'));
        assert_eq!(e.buf.lines(), vec!["Table users {}".to_string()]);
    }

    #[test]
    fn smart_enter_inherits_indent_and_opens_blocks() {
        // plain Enter: the new line inherits the cursor line's indent
        let mut e = editor("Table users {\n  id integer\n}");
        e.buf.move_cursor(CursorMove::Jump(1, 11)); // on the last char
        e.handle_key(key('a'));
        e.handle_key(enter());
        assert_eq!(e.buf.lines()[2], "  ");
        assert_eq!(e.buf.pos(), Pos { row: 2, col: 2 });
        // a `{` before the caret opens one level deeper
        let mut e = editor("Table users {");
        e.buf.move_cursor(CursorMove::Jump(0, 13));
        e.handle_key(key('i'));
        e.handle_key(enter());
        assert_eq!(
            e.buf.lines(),
            vec!["Table users {".to_string(), "  ".to_string()]
        );
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 2 });
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
        // $ lands ON the last char, never on the phantom cell past it
        let (_, c) = e.cursor();
        assert_eq!(c, "second line".chars().count() - 1);
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
    fn dd_removes_the_last_line_entirely() {
        // vim dd on the buffer's last line takes the line away — no
        // phantom empty line left behind
        let mut e = editor("one\ntwo\nthree");
        e.handle_key(key('j'));
        e.handle_key(key('j'));
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one\ntwo");

        // a [count]dd reaching the end eats every line to the end
        let mut e = editor("one\ntwo\nthree");
        e.handle_key(key('j'));
        e.handle_key(key('2'));
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one");

        // a lone line collapses to one empty line, never zero
        let mut e = editor("solo");
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "");

        // a trailing-newline buffer's empty last line is the file's EOL,
        // not a line: dd on it removes the last CONTENT line and the
        // trailing newline survives
        let mut e = editor("one\ntwo\nthree\n");
        e.handle_key(key('G'));
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one\ntwo\n");
    }

    #[test]
    fn motions_never_rest_on_the_eol_marker_row() {
        // a trailing newline stores an empty last row — the file's EOL
        // marker, not a line. G lands on the last CONTENT line (the
        // cursor visibly sits on the brace), j past the end stays put,
        // and dd deletes the line the cursor is on — never the marker
        // row's "contents"
        let mut e = editor("one\n}\n");
        e.handle_key(key('G'));
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 0 }, "G: last content line");
        e.handle_key(key('j'));
        e.handle_key(key('j'));
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 0 }, "j stays on the last line");
        e.handle_key(key('d'));
        e.handle_key(key('d'));
        assert_eq!(e.text(), "one\n");

        // a real empty line before the marker is still a line: G lands
        // on it, not on the content line above
        let mut e = editor("one\n\n\n");
        e.handle_key(key('G'));
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 0 });

        // no trailing newline: no marker, G is the plain last row
        let mut e = editor("one\n}");
        e.handle_key(key('G'));
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 0 });

        // [count]G clamps to the last content line too
        let mut e = editor("one\n}\n");
        e.handle_key(key('9'));
        e.handle_key(key('G'));
        assert_eq!(e.buf.pos(), Pos { row: 1, col: 0 });
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
    fn vim_o_and_o_open_lines_at_the_indented_position() {
        // `o` on a column row: insert mode ON the indented new line
        let mut e = editor("Table users {\n  id integer\n}");
        e.handle_key(key('j'));
        e.handle_key(key('o'));
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.buf.lines(), ["Table users {", "  id integer", "  ", "}"]);
        assert_eq!(e.buf.pos(), Pos { row: 2, col: 2 });
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.buf.lines(), ["Table users {", "  id integer", "}"], "one undo removes the opened line");
        // `o` on the opener sits INSIDE the block, one unit deeper
        let mut e = editor("Table users {\n}");
        e.handle_key(key('o'));
        assert_eq!(e.buf.lines(), ["Table users {", "  ", "}"]);
        // `O` above the opener stays outside the block
        let mut e = editor("Table users {\n}");
        e.handle_key(key('O'));
        assert_eq!(e.buf.lines(), ["", "Table users {", "}"]);
        assert_eq!(e.buf.pos(), Pos { row: 0, col: 0 });
    }

    #[test]
    fn dollar_rides_line_ends_without_the_phantom_column() {
        // vim normal mode never rests ON the past-the-last-char cell: a
        // block cursor at col == len reads as "a space at the end of the
        // line". $ lands ON the last char, and the MAXCOL aim survives
        // the resting clamp (the next j still rides every line's end).
        let mut e = editor("one two three\n\nfour five six\nseven eight");
        e.handle_key(key('$'));
        assert_eq!(e.cursor(), (0, 12), "$ lands ON the last char");
        e.handle_key(key('j'));
        assert_eq!(e.cursor(), (1, 0), "blank line: clamped to 0");
        e.handle_key(key('j'));
        assert_eq!(e.cursor(), (2, 12), "the aim rode past the blank line");
        e.handle_key(key('j'));
        assert_eq!(e.cursor(), (3, 10), "ON this line's last char too");
        e.handle_key(key('k'));
        assert_eq!(e.cursor(), (2, 12), "and back up, aim intact");
        // plain l also stops at the last char instead of stepping past
        let mut e = editor("abc");
        e.handle_key(key('$'));
        e.handle_key(key('l'));
        assert_eq!(e.cursor(), (0, 2), "l never steps past the last char");
    }

    #[test]
    fn visual_p_replaces_the_selection_with_the_register() {
        let mut e = editor("AB go\none two\nthree");
        // yank "AB" charwise (the visual selection includes the cursor
        // char: one l from col 0 covers both letters)
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(key('y'));
        // replace the whole second line with the register
        e.handle_key(key('j'));
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('$'));
        e.handle_key(key('p'));
        assert_eq!(e.text(), "AB go\nAB\nthree");
        // vim's swap: the replaced text became the register
        assert_eq!(e.register, vec!["one two".to_string()]);
        // vim: visual p lands in NORMAL mode on the pasted text's last
        // char — no selection lingers, so motions never extend a highlight
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.buf.selection_anchor().is_none());
        assert_eq!(e.cursor(), (1, 1), "on the last pasted char");
        // one undo reverts the whole replace
        e.handle_key(key('u'));
        assert_eq!(e.text(), "AB go\none two\nthree");
    }

    #[test]
    fn visual_p_then_motion_extends_nothing() {
        // the report: after visual p, moving still highlighted text
        let mut e = editor("AB go\none two\nthree");
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(key('y'));
        e.handle_key(key('j'));
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('$'));
        e.handle_key(key('p'));
        e.handle_key(key('j'));
        e.handle_key(key('j'));
        assert_eq!(e.mode, Mode::Normal, "p left visual");
        assert!(
            e.buf.selection_anchor().is_none(),
            "no selection after p + motions"
        );
        assert_eq!(e.text(), "AB go\nAB\nthree");
        assert_eq!(e.cursor(), (2, 1), "the motions moved the cursor");
    }

    #[test]
    fn visual_p_linewise_register_replaces_the_rows() {
        let mut e = editor("l1\nl2\nl3\nl4");
        e.handle_key(key('y'));
        e.handle_key(key('y')); // linewise "l1"
        e.handle_key(key('j'));
        e.handle_key(key('j'));
        e.handle_key(key('v'));
        e.handle_key(key('l')); // partial "l3"
        e.handle_key(key('p'));
        // yy yanks WITHOUT deleting: the rows are l1, l2, l1(new), l4
        assert_eq!(e.text(), "l1\nl2\nl1\nl4", "the selection's row is replaced by the register lines");
        // vim: normal mode on the first pasted row
        assert_eq!(e.mode, Mode::Normal);
        assert!(!e.visual_linewise);
        assert!(e.buf.selection_anchor().is_none());
    }

    #[test]
    fn visual_r_replaces_every_selected_char() {
        // vim's visual r: every selected char becomes the typed char
        let mut e = editor("foo bar baz\n");
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(key('l')); // "foo" selected
        e.handle_key(key('r'));
        e.handle_key(key('x'));
        assert_eq!(e.text(), "xxx bar baz\n", "the selection became x's");
        // normal mode, cursor on the LAST replaced char
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.cursor(), (0, 2));
        // one undo reverts the whole replace
        e.handle_key(key('u'));
        assert_eq!(e.text(), "foo bar baz\n");
    }

    #[test]
    fn visual_r_spans_rows_and_fills_empty_ones() {
        // charwise over rows: each row's selected span becomes z's; an
        // EMPTY selected row receives ONE z (vim), and the last row only
        // up to the inclusive cursor char
        let mut e = editor("abc\ndef\n\nghi");
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('2'));
        e.handle_key(key('j'));
        e.handle_key(key('G'));
        e.handle_key(key('r'));
        e.handle_key(key('z'));
        assert_eq!(e.text(), "zzz\nzzz\nz\nzhi");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.cursor(), (3, 0), "on the last replaced char");
    }

    #[test]
    fn visual_r_partial_columns_across_rows() {
        // v l j j from col 1: row0 cols 1.. → "bc", row1 whole, row2 up
        // to and including the cursor char (the selection includes it)
        let mut e = editor("abc\ndef\nghi");
        e.handle_key(key('l'));
        e.handle_key(key('v'));
        e.handle_key(key('j'));
        e.handle_key(key('j'));
        e.handle_key(key('r'));
        e.handle_key(key('z'));
        assert_eq!(e.text(), "azz\nzzz\nzzi");
        assert_eq!(e.cursor(), (2, 1), "on the last replaced char");
    }

    #[test]
    fn visual_line_r_replaces_whole_rows() {
        // linewise V r: every char of every selected row
        let mut e = editor("abc\ndef\nghi");
        e.handle_key(key('V'));
        e.handle_key(key('j'));
        e.handle_key(key('r'));
        e.handle_key(key('-'));
        assert_eq!(e.text(), "---\n---\nghi");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.cursor(), (1, 2), "on the last replaced char");
    }

    #[test]
    fn alt_kj_moves_the_cursor_line() {
        let alt = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        let mut e = editor("one\ntwo\nthree");
        e.handle_key(key('j')); // row 1
        e.handle_key(alt('k')); // move up, cursor rides
        assert_eq!(e.text(), "two\none\nthree");
        assert_eq!(e.cursor(), (0, 0));
        // at the top: no-op
        e.handle_key(alt('k'));
        assert_eq!(e.text(), "two\none\nthree");
        // down: "two" swaps with "one" beneath it, cursor rides
        e.handle_key(key('$'));
        e.handle_key(alt('j'));
        assert_eq!(e.text(), "one\ntwo\nthree");
        // down again: "two" swaps with "three"
        e.handle_key(alt('j'));
        assert_eq!(e.text(), "one\nthree\ntwo");
        assert_eq!(e.cursor(), (2, 2), "the column rides along");
        // at the bottom: no-op
        e.handle_key(alt('j'));
        assert_eq!(e.text(), "one\nthree\ntwo");
        // one undo reverts ONE move (the second down)
        e.handle_key(key('u'));
        assert_eq!(e.text(), "one\ntwo\nthree");
    }

    #[test]
    fn alt_kj_moves_the_highlighted_block() {
        let alt = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT);
        let mut e = editor("l0\nl1\nl2\nl3\nl4");
        // select rows 0..=1 partially (v, j, l) — partial highlights count
        e.handle_key(key('v'));
        e.handle_key(key('j'));
        e.handle_key(key('l'));
        // at the TOP: no-op
        e.handle_key(alt('k'));
        assert_eq!(e.text(), "l0\nl1\nl2\nl3\nl4");
        // down: the block swaps with l2, selection rides (rows 1..=2)
        e.handle_key(alt('j'));
        assert_eq!(e.text(), "l2\nl0\nl1\nl3\nl4");
        assert_eq!(e.mode, Mode::Visual);
        assert_eq!(
            e.buf.selection_range(),
            Some((Pos { row: 1, col: 0 }, Pos { row: 2, col: 2 }))
        );
        // down again, and again — the block parks at the bottom
        e.handle_key(alt('j'));
        e.handle_key(alt('j'));
        assert_eq!(e.text(), "l2\nl3\nl4\nl0\nl1");
        // at the BOTTOM: no-op
        e.handle_key(alt('j'));
        assert_eq!(e.text(), "l2\nl3\nl4\nl0\nl1");
        // all the way back up restores the original order
        for _ in 0..3 {
            e.handle_key(alt('k'));
        }
        assert_eq!(e.text(), "l0\nl1\nl2\nl3\nl4");
    }

    #[test]
    fn gcc_toggles_the_line_comment_both_ways() {
        let mut e = editor("id integer\nname text");
        e.handle_key(key('g'));
        e.handle_key(key('c'));
        e.handle_key(key('c'));
        assert_eq!(e.text(), "// id integer\nname text");
        e.handle_key(key('g'));
        e.handle_key(key('c'));
        e.handle_key(key('c'));
        assert_eq!(e.text(), "id integer\nname text");
        // one undo step per toggle
        e.handle_key(key('u'));
        assert_eq!(e.text(), "// id integer\nname text");
        // an indented line toggles at its first non-blank column
        let mut e = editor("Table t {\n  id integer\n}");
        e.handle_key(key('j'));
        for c in "gcc".chars() {
            e.handle_key(key(c));
        }
        assert!(e.text().contains("\n  // id integer\n"), "{:?}", e.text());
    }

    #[test]
    fn visual_gc_toggles_every_highlighted_line() {
        // partial highlights count: the selection touches rows 0..=1
        let mut e = editor("one\ntwo\nthree");
        e.handle_key(key('v'));
        e.handle_key(key('j'));
        e.handle_key(key('l')); // partial second line
        e.handle_key(key('g'));
        e.handle_key(key('c'));
        assert_eq!(e.text(), "// one\n// two\nthree");
        // all-to-none: gc over BOTH commented lines uncomments (the
        // cursor sits on row 1 after the first toggle: k back to row 0,
        // then v j re-covers rows 0..1)
        e.handle_key(key('k'));
        e.handle_key(key('v'));
        e.handle_key(key('j'));
        e.handle_key(key('g'));
        e.handle_key(key('c'));
        assert_eq!(e.text(), "one\ntwo\nthree");
        // mixed: any uncommented line makes the whole gc COMMENT
        let mut e = editor("// one\ntwo\nthree");
        e.handle_key(key('v'));
        for _ in 0..2 {
            e.handle_key(key('j'));
        }
        e.handle_key(key('g'));
        e.handle_key(key('c'));
        assert_eq!(e.text(), "// one\n// two\n// three");
        // the toggle exits visual mode
        assert_eq!(e.mode, Mode::Normal);
    }

    #[test]
    fn undo_after_o_returns_to_the_pre_command_cursor() {
        // `o` from mid-line: undo must land where the user WAS, not the
        // line-end column a move-first implementation snapshots (the
        // left-ctrl chord salvage turns that ghost into a jumplist entry)
        let mut e = editor("one two three\nfour");
        e.handle_key(key('l'));
        e.handle_key(key('l')); // (0, 2), mid-word
        e.handle_key(key('o')); // open below, insert
        assert_eq!(e.cursor(), (1, 0));
        e.handle_key(esc()); // leave insert: `u` is the undo motion
        e.handle_key(key('u'));
        assert_eq!(e.text(), "one two three\nfour");
        assert_eq!(e.cursor(), (0, 2), "back to the pre-`o` spot, col included");
        // `O` mirrors it: undo returns to the same line and column
        e.handle_key(key('O'));
        assert_eq!(e.cursor(), (0, 0), "O opens above at col 0");
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.cursor(), (0, 2), "undo restores the column again");
    }

    #[test]
    fn vim_slash_search_is_vim_style_not_the_gui_find() {
        let mut e = editor("alpha\nbeta\ngamma");
        // 'e' only occurs in "beta": '/' opens the vim prompt (NOT the
        // gui find popup), typing searches LIVE (the caret jumps per
        // keystroke)
        e.handle_key(key('/'));
        assert!(e.vim_search.is_some());
        assert!(!e.find_open(), "the gui find stays out of vim's /");
        e.handle_key(key('e'));
        assert_eq!(e.cursor(), (1, 1));
        assert_eq!(e.find_position(), (1, 1), "one match, on it");
        // Enter locks in: the prompt closes, the caret stays put
        // (cycling is n/N's job)
        e.handle_key(enter());
        assert!(e.vim_search.is_none());
        assert_eq!(e.cursor(), (1, 1));
        // the pattern stays: n/N keep cycling
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (1, 1));
        e.handle_key(key('N'));
        assert_eq!(e.cursor(), (1, 1));
        // a fresh search cancelled with Esc clears the pattern: n does
        // not cycle on a cancelled query
        e.handle_key(key('/'));
        e.handle_key(key('z'));
        e.handle_key(esc());
        assert!(e.vim_search.is_none());
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (1, 1), "no z match: the caret stays");
    }

    #[test]
    fn vim_slash_enter_locks_in_without_jumping() {
        // three matches: live typing parks the caret on the first, and
        // Enter must NOT advance — the caret stays parked while the
        // pattern stays set for n/N
        let mut e = editor("foo\nbar\nfoo\nfoo");
        e.handle_key(key('/'));
        for c in "foo".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 0), "live search rides the first match");
        e.handle_key(enter());
        assert!(e.vim_search.is_none(), "the prompt closed");
        assert_eq!(e.cursor(), (0, 0), "Enter does not cycle");
        // the pattern is locked in: n cycles from the parked caret
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (2, 0), "n moves to the second match");
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (3, 0), "then the third");
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (0, 0), "n wraps back to the first");
    }

    #[test]
    fn search_bands_drop_on_esc_insert_yank_visual_but_n_keeps_cycling() {
        let mut e = editor("foo\nbar\nfoo");
        // commit a / search: the caret rides the first match, bands drawn
        e.handle_key(key('/'));
        for c in "foo".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(enter());
        assert_eq!(e.cursor(), (0, 0), "Enter locks in: the caret stays");
        assert_eq!(e.buf.search_hl_spans().len(), 2, "bands on");
        // Esc is :noh: bands gone, the pattern alive
        e.handle_key(esc());
        assert!(e.buf.search_hl_spans().is_empty(), "Esc hides");
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (2, 0), "n steps to the other match (hidden bands)");
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (0, 0), "n wraps with the hidden bands");
        // a fresh search re-shows; entering insert hides again
        e.handle_key(key('/'));
        for c in "foo".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(enter());
        assert_eq!(e.buf.search_hl_spans().len(), 2, "re-shown");
        e.handle_key(key('i'));
        assert!(e.buf.search_hl_spans().is_empty(), "insert hides");
        e.handle_key(esc());
        // yank (yy) hides
        e.handle_key(key('y'));
        e.handle_key(key('y'));
        assert!(e.buf.search_hl_spans().is_empty(), "yank hides");
        // visual entry hides
        e.handle_key(key('v'));
        assert!(e.buf.search_hl_spans().is_empty(), "visual hides");
        e.handle_key(esc());
        // the gui find: a fresh query re-shows, closing hides
        e.toggle_find();
        e.handle_key(key('o'));
        assert!(!e.buf.search_hl_spans().is_empty(), "find query re-shows");
        e.handle_key(esc());
        assert!(!e.find_open(), "the find bar closed");
        assert!(e.buf.search_hl_spans().is_empty(), "closing hides");
        // and the pattern still cycles through all of it (the live query
        // parked the caret on the first o: (0,1))
        e.handle_key(key('n'));
        assert_eq!(e.cursor(), (0, 2), "n lands on the next o ahead");
    }

    #[test]
    fn ctrl_a_selects_all_in_vim_insert_mode() {
        let mut e = editor("one\ntwo");
        e.handle_key(key('i'));
        e.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(e.buf.selection_range().is_some(), "select-all engaged");
        assert_eq!(
            e.buf.selection_text().as_deref(),
            Some("one\ntwo"),
            "the whole buffer"
        );
        // the selection is live: typing over it replaces everything
        e.handle_key(key('x'));
        assert_eq!(e.text(), "x");
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

        // visual y then p pastes INLINE after the cursor char (vim put)
        e.handle_key(key('$'));
        e.handle_key(key('v'));
        e.handle_key(key('b'));
        e.handle_key(key('y'));
        e.handle_key(key('p'));
        // $ lands ON the last char, so $vb yanks "three" inclusive; visual
        // y leaves the cursor at the selection START (vim) — p inserts
        // inline after that char
        assert_eq!(e.text(), "one two tthreehree", "{}", e.text());

        // visual d deletes the selection
        e.handle_key(key('0'));
        e.handle_key(key('v'));
        e.handle_key(key('l'));
        e.handle_key(key('d'));
        // vim v+l selects TWO chars (inclusive of the cursor cell): "on"
        assert_eq!(e.text(), "e two tthreehree", "{}", e.text());
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
    fn plain_arrow_extends_in_visual_after_shift_arrows() {
        // shift+arrows land in charwise visual mode, so a plain arrow
        // extends the selection there (vim); escape clears the highlight
        let mut e = editor("one two");
        e.handle_key(shift(KeyCode::Right));
        e.handle_key(shift(KeyCode::Right));
        assert_eq!(e.mode, Mode::Visual);
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(e.buf.selection_anchor().is_some(), "visual arrows extend");
        assert_eq!(e.cursor(), (0, 3), "and moved");
        e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Normal, "escape clears the highlight");
        assert!(e.buf.selection_anchor().is_none());
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
    fn cl_and_dl_change_and_delete_chars() {
        // cl ≈ s: the char under the cursor goes, insert begins
        let (e, t) = typed("hello world", "cl");
        assert_eq!(t, "ello world");
        assert_eq!(e.mode, Mode::Insert);
        // counts: 3cl == 3s, leading or trailing
        let (_, t) = typed("hello world", "3cl");
        assert_eq!(t, "lo world");
        let (_, t) = typed("hello world", "c3l");
        assert_eq!(t, "lo world");
        // dl: the same delete without insert
        let (e, t) = typed("hello world", "dl");
        assert_eq!(t, "ello world");
        assert_eq!(e.mode, Mode::Normal);
        let (_, t) = typed("hello world", "3dl");
        assert_eq!(t, "lo world");
        // the cursor sits where the deletion happened
        let mut e = editor("hello world");
        for c in "llldl".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (0, 3));
        // never crosses the line end; cl still enters insert there (vim:
        // nothing to delete at EOL, but the edit starts)
        let (e, t) = typed("ab", "$cl");
        // $ lands ON the last char, so cl changes through it (vim's C)
        assert_eq!(t, "a");
        assert_eq!(e.mode, Mode::Insert);
        let (_, t) = typed("ab", "$dl");
        assert_eq!(t, "a");
        // a spent count does not multiply the typed text afterwards
        let (_, t) = typed("hello world", "3clx");
        assert_eq!(t, "xlo world");
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
        assert_eq!(t, "one");
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
        assert_eq!(e.cursor(), (0, 4), "l stops at the last char");
        // 2$ goes to the end of the next line
        let mut e = editor("hello\nworld!");
        e.handle_key(key('2'));
        e.handle_key(key('$'));
        assert_eq!(e.cursor(), (1, 5), "2$: ON the next line's last char");
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
mod highlight_escape_tests {
    use super::*;
    use super::tests::{editor, key};

    #[test]
    fn shift_arrows_enter_visual_mode_and_escape_clears() {
        let mut e = editor("hello world\n");
        e.handle_key(key('w'));
        // shift+right in normal mode: visual selection starts
        let _ = e.handle_key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ));
        assert_eq!(e.mode, Mode::Visual);
        assert!(e.buf.selection_anchor().is_some());
        // escape clears the highlight and lands in normal mode
        let _ = e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.buf.selection_anchor().is_none());
    }

    #[test]
    fn insert_mode_highlight_escape_lands_in_visual() {
        // highlight in insert mode, escape: visual keeps the selection
        let mut e = editor("hello world\n");
        e.handle_key(key('i'));
        let _ = e.handle_key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ));
        assert!(e.buf.selection_anchor().is_some());
        let _ = e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Visual);
        assert!(e.buf.selection_anchor().is_some());
        // a second escape clears it
        let _ = e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.buf.selection_anchor().is_none());
    }

    #[test]
    fn plain_arrows_collapse_the_normal_mode_highlight() {
        let mut e = editor("hello world\n");
        e.handle_key(key('w'));
        let _ = e.handle_key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        ));
        assert!(e.buf.selection_anchor().is_some());
        let _ = e.handle_key(key('w')); // plain motion in visual: extends
        let _ = e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(e.buf.selection_anchor().is_none(), "escape clears");
    }

    #[test]
    fn escape_without_a_highlight_is_a_noop() {
        let mut e = editor("hello world\n");
        let _ = e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.buf.selection_anchor().is_none());
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
    fn vim_insert_alt_arrows_select_a_word() {
        let mut e = editor("one two");
        e.mode = Mode::Insert;
        e.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::ALT));
        assert_eq!(
            e.buf.selection_draw_range_exclusive(),
            Some((
                crate::editor::buffer::Pos { row: 0, col: 0 },
                crate::editor::buffer::Pos { row: 0, col: 3 }
            )),
            "alt+right highlighted the word `one`"
        );
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

#[cfg(test)]
mod indent_seed_tests {
    use super::tests::{editor, esc, key};
    use super::*;

    // ---- linewise change seeds the formatter's indent (cc / S / V c) ----

    #[test]
    fn cc_on_a_body_line_seeds_the_formatter_indent() {
        // the formatter would re-indent the line to its body level, so the
        // fresh line starts pre-indented with the cursor after the seed
        let mut e = editor("Table users {\n  id integer\n  name varchar\n}\n");
        for c in "jjcc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.cursor(), (2, 2), "cursor after the seeded indent");
        assert_eq!(
            e.buf.lines(),
            vec![
                "Table users {".to_string(),
                "  id integer".to_string(),
                "  ".to_string(),
                "}".to_string()
            ]
        );
        // typing continues right after the indent
        e.handle_key(key('X'));
        assert_eq!(e.buf.line(2), "  X");
    }

    #[test]
    fn cc_seed_is_part_of_the_change_undo_step() {
        // one u after cc + Esc restores the original file (the indent seed
        // rides in the change's snapshot, it is no extra step)
        let mut e = editor("Table users {\n  id integer\n  name varchar\n}\n");
        for c in "jjcc".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.cursor(), (2, 1), "Esc backs up onto the indent line");
        e.handle_key(key('u'));
        assert_eq!(e.text(), "Table users {\n  id integer\n  name varchar\n}\n");
        assert_eq!(e.cursor().0, 2, "undo returns to the changed line");
        // redo brings the seeded line back
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "Table users {\n  id integer\n  \n}\n");

        // typed text stacks on top and enough undos still restore
        let mut e = editor("Table users {\n  id integer\n}\n");
        for c in "jccX".chars() {
            e.handle_key(key(c));
        }
        e.handle_key(esc());
        assert_eq!(e.text(), "Table users {\n  X\n}\n");
        for _ in 0..3 {
            e.handle_key(key('u'));
        }
        assert_eq!(e.text(), "Table users {\n  id integer\n}\n");
    }

    #[test]
    fn vline_c_seeds_the_indent_on_the_first_row() {
        // V+c over two body lines collapses them into one seeded line
        let mut e = editor("Table users {\n  a integer\n  b integer\n  c integer\n}\n");
        for c in "jjVjc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.cursor(), (2, 2));
        assert_eq!(
            e.buf.lines(),
            vec![
                "Table users {".to_string(),
                "  a integer".to_string(),
                "  ".to_string(),
                "}".to_string()
            ]
        );
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(
            e.text(),
            "Table users {\n  a integer\n  b integer\n  c integer\n}\n",
            "one undo unit"
        );
    }

    #[test]
    fn cc_on_a_top_level_line_stays_at_col_zero() {
        // no non-blank line above, own indent 0: nothing is seeded
        let mut e = editor("Ref: fk\nTable users {\n  id integer\n}\n");
        for c in "cc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.cursor(), (0, 0), "no indent at top level");
        assert_eq!(e.text(), "\nTable users {\n  id integer\n}\n");

        // below an indented body, the closing } is back at column 0
        // (cc at EOF appends the fresh line below — change_lines' shape,
        // pinned by the cG tests)
        let mut e = editor("Table users {\n  id integer\n}\nRef: fk");
        for c in "jjjcc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (3, 0), "the closer's level carries over");
        assert_eq!(e.text(), "Table users {\n  id integer\n}\n\n");
    }

    #[test]
    fn cc_after_a_block_opener_indents_one_level() {
        // the replaced line has no indent of its own, but the opener above
        // says the formatter would put this line one level in
        let mut e = editor("Table users {\nid integer\n}\n");
        for c in "jcc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (1, 2));
        assert_eq!(e.buf.lines(), vec!["Table users {", "  ", "}"]);

        // a trailing // comment does not hide the opener
        let mut e = editor("Table users { // login\nid integer\n}\n");
        for c in "jcc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (1, 2));
        assert_eq!(e.buf.lines(), vec!["Table users { // login", "  ", "}"]);
    }

    #[test]
    fn counts_seed_like_their_first_line() {
        // 2cc at the first body row: the opener above decides the level
        let mut e = editor("Table users {\n  a\n  b\n  c\n}\n");
        for c in "j2cc".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (1, 2));
        assert_eq!(e.text(), "Table users {\n  \n  c\n}\n");
    }

    #[test]
    fn s_substitutes_the_line_like_cc() {
        let mut e = editor("Table users {\n  id integer\n}\n");
        for c in "jS".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.cursor(), (1, 2), "S seeds the indent like cc");
        assert_eq!(e.buf.lines(), vec!["Table users {", "  ", "}"]);
        e.handle_key(esc());
        e.handle_key(key('u'));
        assert_eq!(e.text(), "Table users {\n  id integer\n}\n", "one undo unit");

        // [count]S clears count lines, seeded from the opener above
        let mut e = editor("T {\n  b\n  c\nd");
        for c in "j2S".chars() {
            e.handle_key(key(c));
        }
        assert_eq!(e.cursor(), (1, 2), "seeded one level under the opener");
        assert_eq!(e.text(), "T {\n  \nd");
    }
}

#[cfg(test)]
mod jumplist_tests {
    use super::*;

    fn editor() -> Editor {
        let src: String = (0..50)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        Editor::new(&src)
    }

    #[test]
    fn back_and_forward_walk_the_recorded_origins() {
        let mut e = editor();
        e.jump(10, 0);
        e.record_jump();
        e.jump(20, 0);
        e.record_jump();
        e.jump(30, 0);
        // present: (30,0). back -> (20,0), back -> (10,0)
        assert!(e.jump_back());
        assert_eq!(e.buf.cursor(), (20, 0));
        assert!(e.jump_back());
        assert_eq!(e.buf.cursor(), (10, 0));
        assert!(!e.jump_back(), "past is empty");
        // forward returns the way it came
        assert!(e.jump_forward());
        assert_eq!(e.buf.cursor(), (20, 0));
        assert!(e.jump_forward());
        assert_eq!(e.buf.cursor(), (30, 0));
        assert!(!e.jump_forward(), "future is empty");
    }

    #[test]
    fn a_new_jump_discards_the_forward_stack() {
        let mut e = editor();
        // A -> B -> C, then one step back to B; a fresh jump from there
        // must discard the old future (C): forward now returns to the
        // position just left (40), not the stale one
        e.record_jump(); // origin (0,0)
        e.jump(10, 0);
        e.record_jump(); // origin (10,0)
        e.jump(20, 0);
        e.jump_back();
        assert_eq!(e.buf.cursor(), (10, 0));
        e.record_jump();
        e.jump(40, 0);
        e.jump_back();
        assert_eq!(e.buf.cursor(), (10, 0));
        assert!(e.jump_forward());
        assert_eq!(e.buf.cursor(), (40, 0), "the stale (20,0) future is gone");
    }

    #[test]
    fn records_never_create_spurious_slots() {
        let mut e = editor();
        // walk 0 -> 5 -> 10 -> 15 recording before each hop, then walk the
        // whole list back: exactly three origins, no duplicates
        e.record_jump();
        e.jump(5, 0);
        e.record_jump();
        e.jump(10, 0);
        e.record_jump();
        e.jump(15, 0);
        assert!(e.jump_back());
        assert_eq!(e.buf.cursor(), (10, 0));
        assert!(e.jump_back());
        assert_eq!(e.buf.cursor(), (5, 0));
        assert!(e.jump_back());
        assert_eq!(e.buf.cursor(), (0, 0));
        assert!(!e.jump_back());
        // and recording repeatedly at one spot adds no slots
        let mut e2 = editor();
        e2.record_jump();
        e2.record_jump();
        e2.jump(5, 0);
        e2.record_jump();
        e2.jump(10, 0);
        e2.jump_back();
        assert_eq!(e2.buf.cursor(), (5, 0));
        e2.jump_back();
        assert_eq!(e2.buf.cursor(), (0, 0));
        assert!(!e2.jump_back(), "the duplicate created no extra slot");
    }
}

#[cfg(test)]
mod replace_ctrl_tests {
    use super::*;

    #[test]
    fn ctrl_r_never_completes_a_pending_replace() {
        // the DE re-injection: bare 'r' arms replace, then the flagged
        // ctrl+r arrives — it must REDO, not replace the char with 'r'
        let mut e = Editor::new("abc\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(e.text(), "bc\n", "'x' deleted the 'a'");
        e.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE));
        assert_eq!(e.text(), "abc\n", "undone");
        // bare r arms the replace-pending
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        // flagged ctrl+r: REDO (restores the delete), never replace-with-r
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "bc\n", "redo ran — not replace-with-r");
        assert_eq!(e.pending, None, "the pending was consumed");
    }

    #[test]
    fn other_ctrl_chars_cancel_the_pending_replace() {
        let mut e = Editor::new("abc\n");
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        // some other ctrl chord: the entry guard swallows it entirely —
        // the pending replace stays armed (vim semantics: ctrl chords
        // never complete a replace)
        e.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "abc\n");
        assert_eq!(e.pending, Some(Pending::R), "the guard preserves the pending");
        // Esc cancels the pending replace (vim), and the engine works on
        e.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(e.pending, None);
        e.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(e.buf.cursor(), (0, 1));
    }
}

#[cfg(test)]
mod linewise_operator_tests {
    use super::*;

    fn editor(src: &str) -> Editor {
        let mut e = Editor::new(src);
        e.mode = Mode::Normal;
        e
    }

    fn keys(e: &mut Editor, k: &[(&str, char)]) {
        for (mods, c) in k {
            let m = match *mods {
                "ctrl" => KeyModifiers::CONTROL,
                _ => KeyModifiers::NONE,
            };
            e.handle_key(KeyEvent::new(KeyCode::Char(*c), m));
        }
    }

    #[test]
    fn dj_deletes_the_current_and_next_line() {
        let mut e = editor("one\ntwo\nthree\n");
        keys(&mut e, &[("", 'd'), ("", 'j')]);
        assert_eq!(e.text(), "three\n");
        assert_eq!(e.buf.cursor(), (0, 0));
    }

    #[test]
    fn dk_deletes_the_current_and_previous_line() {
        let mut e = editor("one\ntwo\nthree\n");
        e.buf.move_cursor(CursorMove::Down);
        keys(&mut e, &[("", 'd'), ("", 'k')]);
        assert_eq!(e.text(), "three\n");
        assert_eq!(e.buf.cursor(), (0, 0));
    }

    #[test]
    fn d3j_deletes_four_lines() {
        let src = "l0\nl1\nl2\nl3\nl4\n";
        let mut e = editor(src);
        keys(&mut e, &[("", 'd'), ("3", '3'), ("", 'j')]);
        assert_eq!(e.text(), "l4\n");
    }

    #[test]
    fn two_dj_deletes_three_lines() {
        let src = "l0\nl1\nl2\nl3\nl4\n";
        let mut e = editor(src);
        keys(&mut e, &[("2", '2'), ("", 'd'), ("", 'j')]);
        assert_eq!(e.text(), "l3\nl4\n");
    }

    #[test]
    fn dj_on_the_last_line_deletes_just_the_current_line() {
        // j runs out at the trailing EOL marker: dj deletes just the
        // current (last content) line, the file's trailing newline stays
        let mut e = editor("one\ntwo\n");
        e.buf.move_cursor(CursorMove::Down);
        keys(&mut e, &[("", 'd'), ("", 'j')]);
        assert_eq!(e.text(), "one\n");
    }

    #[test]
    fn dj_yanks_into_the_register_for_p() {
        let mut e = editor("one\ntwo\nthree\n");
        keys(&mut e, &[("", 'd'), ("", 'j')]);
        e.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(e.text(), "three\none\ntwo\n");
    }

    #[test]
    fn cj_deletes_and_enters_insert_with_the_seed() {
        let mut e = editor("Table t {\n  a integer\n  b integer\n}\n");
        e.buf.move_cursor(CursorMove::Down);
        keys(&mut e, &[("", 'c'), ("", 'j')]);
        assert_eq!(e.mode, Mode::Insert);
        assert_eq!(e.text(), "Table t {\n  \n}\n", "one seeded line replaces both");
    }
}

#[cfg(test)]
mod yank_motion_tests {
    use super::*;

    fn editor(src: &str) -> Editor {
        let mut e = Editor::new(src);
        e.mode = Mode::Normal;
        e
    }

    fn keys(e: &mut Editor, k: &[char]) {
        for c in k {
            e.handle_key(KeyEvent::new(KeyCode::Char(*c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn yw_yanks_the_word_and_keeps_the_cursor() {
        let mut e = editor("foo bar baz\n");
        keys(&mut e, &[('y'), ('w')]);
        assert_eq!(e.register, vec!["foo ".to_string()]);
        assert_eq!(e.buf.cursor(), (0, 0), "y does not move");
        // p pastes INLINE after the cursor char (vim); yw takes the
        // trailing space (dw semantics); the cursor lands on the last
        // pasted character
        e.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE));
        assert_eq!(e.text(), "ffoo oo bar baz\n");
        assert_eq!(e.buf.cursor(), (0, 4), "on the last pasted char");
    }

    #[test]
    fn ye_yanks_inclusive_of_the_last_char() {
        let mut e = editor("foo bar baz\n");
        keys(&mut e, &[('y'), ('e')]);
        assert_eq!(e.register, vec!["foo".to_string()]);
    }

    #[test]
    fn y0_and_yy_span_the_line() {
        let mut e = editor("foo bar baz\n");
        e.buf.move_cursor(CursorMove::Forward);
        e.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::Char('0'), KeyModifiers::NONE));
        assert_eq!(e.register, vec!["f".to_string()]);
        let mut e = editor("foo bar baz\n");
        keys(&mut e, &[('y'), ('y')]);
        assert_eq!(e.register, vec!["foo bar baz".to_string()]);
        assert_eq!(e.buf.cursor(), (0, 0));
    }

    #[test]
    fn y_dollar_yanks_to_the_line_end() {
        let mut e = editor("foo bar baz\n");
        e.buf.move_cursor(CursorMove::Forward);
        e.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        e.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(e.register, vec!["oo bar baz".to_string()]);
    }

    #[test]
    fn yj_yanks_two_lines_and_restores_the_cursor() {
        let mut e = editor("l0\nl1\nl2\n");
        e.buf.move_cursor(CursorMove::Down);
        e.buf.move_cursor(CursorMove::Forward);
        keys(&mut e, &[('y'), ('j')]);
        assert_eq!(e.register, vec!["l1".to_string(), "l2".to_string()]);
        assert_eq!(e.buf.cursor(), (1, 1), "cursor restored");
    }

    #[test]
    fn ygg_and_y_g_yank_to_the_ends() {
        let mut e = editor("l0\nl1\nl2\n");
        e.buf.move_cursor(CursorMove::Down);
        keys(&mut e, &[('y'), ('g'), ('g')]);
        assert_eq!(e.register, vec!["l0".to_string(), "l1".to_string()]);
        assert_eq!(e.buf.cursor(), (1, 0), "restored");
        keys(&mut e, &[('y'), ('G')]);
        assert_eq!(e.register, vec!["l1".to_string(), "l2".to_string()]);
        assert_eq!(e.buf.cursor(), (1, 0), "restored");
    }

    #[test]
    fn capital_y_yanks_the_whole_line() {
        let mut e = editor("l0\nl1\n");
        e.buf.move_cursor(CursorMove::Forward);
        e.handle_key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT));
        assert_eq!(e.register, vec!["l0".to_string()]);
        assert_eq!(e.buf.cursor(), (0, 1), "cursor restored");
    }

    #[test]
    fn every_yank_flashes_the_region() {
        let mut e = editor("l0\nl1\nl2\n");
        keys(&mut e, &[('y'), ('y')]);
        let f = e.yank_flash.expect("yy flashes");
        assert!(f.start == (0, 0) && f.end == (1, 0), "whole-row encoding");
        keys(&mut e, &[('y'), ('w')]);
        let f = e.yank_flash.expect("yw flashes");
        assert!(f.start == (0, 0) && f.end == (0, 2), "charwise span of l0");
    }
}

#[cfg(test)]
mod debug_yg {
    use super::*;

    #[test]
    fn debug_ygg() {
        let mut e = Editor::new("l0\nl1\nl2\n");
        e.mode = Mode::Normal;
        e.buf.move_cursor(CursorMove::Down);
        println!("after down: {:?}", e.buf.cursor());
        e.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        println!("after y: pending={:?} cursor={:?}", e.pending, e.buf.cursor());
        e.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        println!("after g: pending={:?} cursor={:?}", e.pending, e.buf.cursor());
        e.handle_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE));
        println!("after g2: register={:?} cursor={:?}", e.register, e.buf.cursor());
    }
}

#[cfg(test)]
mod paste_semantics_tests {
    use super::*;

    fn editor(src: &str) -> Editor {
        let mut e = Editor::new(src);
        e.mode = Mode::Normal;
        e
    }

    fn keys(e: &mut Editor, k: &[char]) {
        for c in k {
            e.handle_key(KeyEvent::new(KeyCode::Char(*c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn linewise_p_duplicates_below_cursor_on_first_non_blank() {
        // nvim-verified: yy p duplicates the line below; the cursor lands
        // on the first pasted line's first non-blank column
        let mut e = editor("alpha beta\ngamma\n");
        keys(&mut e, &[('y'), ('y'), ('p')]);
        assert_eq!(
            e.text(),
            "alpha beta\nalpha beta\ngamma\n",
            "duplicated below"
        );
        assert_eq!(e.buf.cursor(), (1, 0), "first pasted line, first non-blank");
    }

    #[test]
    fn linewise_p_put_duplicates_above() {
        let mut e = editor("alpha beta\ngamma\n");
        keys(&mut e, &[('y'), ('y')]);
        e.buf.move_cursor(CursorMove::Down);
        keys(&mut e, &[('P')]);
        assert_eq!(
            e.text(),
            "alpha beta\nalpha beta\ngamma\n",
            "duplicated above the gamma line"
        );
        assert_eq!(e.buf.cursor(), (1, 0));
    }

    #[test]
    fn charwise_p_inserts_after_the_cursor_char() {
        // nvim-verified: yiw at "alpha", w to "beta", p inserts "alpha"
        // after the 'b' of beta; the cursor ends ON the last pasted char
        let mut e = editor("alpha beta gamma\n");
        keys(&mut e, &[('y'), ('i'), ('w')]);
        e.buf.move_cursor(CursorMove::WordForward);
        keys(&mut e, &[('p')]);
        assert_eq!(e.text(), "alpha balphaeta gamma\n");
        // pasted "alpha" occupies cols 7..12; the cursor is on col 11
        assert_eq!(e.buf.cursor(), (0, 11), "on the last pasted char");
    }

    #[test]
    fn charwise_p_put_inserts_before_the_cursor_char() {
        let mut e = editor("alpha beta gamma\n");
        e.buf.move_cursor(CursorMove::WordForward); // cursor on 'b' of beta
        keys(&mut e, &[('y'), ('i'), ('w')]);
        keys(&mut e, &[('P')]);
        assert_eq!(e.text(), "alpha betabeta gamma\n");
        // pasted "beta" occupies cols 6..10; the cursor is on col 9
        assert_eq!(e.buf.cursor(), (0, 9), "on the last pasted char");
    }

    #[test]
    fn counts_repeat_the_put() {
        let mut e = editor("alpha beta\ngamma\n");
        keys(&mut e, &[('y'), ('y'), ('3'), ('p')]);
        assert_eq!(e.buf.line_count(), 5, "1 + 3 pasted copies");
    }
}

#[cfg(test)]
mod paste_undo_tests {
    use super::*;
    use crate::editor::tests::key as engine_key;

    fn editor(src: &str) -> Editor {
        let mut e = Editor::new(src);
        e.mode = Mode::Normal;
        e
    }

    fn keys(e: &mut Editor, k: &[char]) {
        for c in k {
            e.handle_key(KeyEvent::new(KeyCode::Char(*c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn one_u_undoes_a_whole_linewise_paste() {
        let mut e = editor("alpha beta\ngamma\n");
        keys(&mut e, &[('y'), ('y'), ('p')]);
        assert_eq!(e.text(), "alpha beta\nalpha beta\ngamma\n", "pasted");
        keys(&mut e, &[('u')]);
        assert_eq!(e.text(), "alpha beta\ngamma\n", "ONE u undoes the paste");
        // the cursor returns to the pre-paste position
        assert_eq!(e.buf.cursor(), (0, 0));
        // and ctrl+r redoes the whole paste
        e.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "alpha beta\nalpha beta\ngamma\n");
    }

    #[test]
    fn two_pastes_undo_in_two_presses() {
        let mut e = editor("alpha beta\ngamma\n");
        keys(&mut e, &[('y'), ('y'), ('p'), ('p')]);
        assert_eq!(
            e.text(),
            "alpha beta\nalpha beta\nalpha beta\ngamma\n",
            "two pastes"
        );
        keys(&mut e, &[('u')]);
        assert_eq!(e.text(), "alpha beta\nalpha beta\ngamma\n", "first u");
        keys(&mut e, &[('u')]);
        assert_eq!(e.text(), "alpha beta\ngamma\n", "second u");
    }

    #[test]
    fn a_counted_paste_is_still_one_unit() {
        let mut e = editor("l0\nl1\nl2\n");
        keys(&mut e, &[('y'), ('y'), ('3'), ('p')]);
        assert_eq!(e.buf.line_count(), 6, "1 + 3 copies");
        keys(&mut e, &[('u')]);
        assert_eq!(e.buf.line_count(), 3, "one u undoes all four");
    }

    #[test]
    fn charwise_multiline_paste_is_one_unit() {
        // visual across two lines: charwise register with an embedded
        // newline — the paste splits lines but is still ONE undo unit
        let mut e = editor("l0\nl1\n");
        e.handle_key(engine_key('v'));
        e.handle_key(engine_key('j'));
        e.handle_key(engine_key('$'));
        e.handle_key(engine_key('y'));
        keys(&mut e, &[('p')]);
        assert_eq!(e.buf.line_count(), 3, "pasted inline, splitting lines");
        keys(&mut e, &[('u')]);
        assert_eq!(e.buf.line_count(), 2, "one u undoes the whole put");
    }
}
