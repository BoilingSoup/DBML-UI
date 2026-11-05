//! The editor's text buffer: lines + cursor + selection + undo/redo +
//! regex search, with the exact motion semantics the vim engine was built
//! against (ported from tui-textarea's documented behavior: char columns,
//! sticky column on Up/Down, line-wrap behaviors of Head/End, vim-style
//! word motions, newline-counting `delete_str`).

use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // full motion surface kept for the engine
pub enum Move {
    Forward,
    Back,
    Up,
    Down,
    Head,
    End,
    Top,
    Bottom,
    WordForward,
    WordEnd,
    WordBack,
    ParagraphForward,
    ParagraphBack,
    /// Jump to (row, col), clamped into the text.
    Jump(usize, usize),
    /// Vim's `$`: end of line, and the desired column goes MAXCOL —
    /// vertical moves land on EVERY line's end until a horizontal motion
    /// re-aims (vim's curswant semantics).
    EndMax,
    /// Land on an absolute row at the DESIRED column, preserving the aim
    /// — the animated scroll's per-tick cursor move (a `Jump` would clamp
    /// the column to each crossed line and feed the collapse forward:
    /// one blank line during ctrl+u used to park the cursor at col 0 for
    /// the rest of the scroll).
    GotoRow(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone)]
struct Snapshot {
    lines: Vec<String>,
    cursor: Pos,
    selection: Option<Pos>,
}

#[derive(Default)]
#[allow(dead_code)] // full editing surface kept for the engine
pub struct Buffer {
    lines: Vec<String>,
    cursor: Pos,
    /// Selection anchor; when set, moving the cursor extends the selection.
    selection: Option<Pos>,
    /// Preferred column for Up/Down across short lines.
    desired_col: usize,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    search: Option<Regex>,
    /// `:noh` — the pattern stays alive for `n`/`N`, but the match bands
    /// draw hidden until a fresh search re-shows them.
    search_hl_hidden: bool,
}

#[allow(dead_code)] // full editing surface kept for the engine
/// Inclusive char span of a text object, or whole rows when `linewise`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextObj {
    pub start: Pos,
    pub end: Pos,
    pub linewise: bool,
}

/// The object selector after `i`/`a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjKind {
    Word,
    Paragraph,
    /// `'` `"` or `` ` ``
    Quote(char),
    /// any member of the pair; stored as its opening char
    Bracket(char),
}

/// Normalize an object key press to a kind: `w` `p` quotes, brackets
/// (either member, plus vim's `b`/`B` aliases).
pub fn parse_obj_kind(c: char) -> Option<ObjKind> {
    match c {
        'w' => Some(ObjKind::Word),
        'p' => Some(ObjKind::Paragraph),
        '\'' | '"' | '`' => Some(ObjKind::Quote(c)),
        '(' | ')' | 'b' => Some(ObjKind::Bracket('(')),
        '[' | ']' => Some(ObjKind::Bracket('[')),
        '{' | '}' | 'B' => Some(ObjKind::Bracket('{')),
        '<' | '>' => Some(ObjKind::Bracket('<')),
        _ => None,
    }
}

pub fn close_of(open: char) -> char {
    match open {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => '>',
    }
}

impl Buffer {
    pub fn from_lines(lines: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let lines: Vec<String> = lines.into_iter().map(Into::into).collect();
        Buffer {
            lines: if lines.is_empty() {
                vec![String::new()]
            } else {
                lines
            },
            ..Default::default()
        }
    }

    #[allow(dead_code)] // kept tui-textarea API surface
    pub fn from_text(text: &str) -> Self {
        Self::from_lines(text.split('\n'))
    }

    /// Full text; a document always ends with a trailing newline when the
    /// source had one (lines.len() > 1 implies separators between them).
    #[allow(dead_code)] // kept tui-textarea API surface
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn line(&self, row: usize) -> &str {
        self.lines.get(row).map(String::as_str).unwrap_or("")
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col)
    }

    pub fn pos(&self) -> Pos {
        self.cursor
    }

    pub fn selection_anchor(&self) -> Option<Pos> {
        self.selection
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The last CONTENT line's row: a document stored with a trailing
    /// newline keeps that newline as an empty last row (the file's EOL
    /// marker), which vim never shows as a line nor rests the cursor on.
    /// Motions and click targeting clamp here — the empty row below the
    /// document (and the horizontal scrollbar's reserved space under it)
    /// is not a cursor destination.
    pub fn last_content_row(&self) -> usize {
        if self.lines.len() > 1 && self.lines.last().is_some_and(|l| l.is_empty()) {
            self.lines.len() - 2
        } else {
            self.lines.len() - 1
        }
    }

    /// Normalized selection range (start inclusive, end exclusive), if a
    /// selection is active.
    ///
    /// Vim selects the cells between anchor and cursor inclusive on both
    /// ends (an `e` or `$` landing on a char includes it; at EOL the
    /// newline). Normalized here by advancing the cursor-side end by one.
    pub fn selection_range(&self) -> Option<(Pos, Pos)> {
        let a = self.selection?;
        let (start, end) = if self.before(self.cursor, a) {
            (self.cursor, self.advance(a))
        } else {
            (a, self.advance(self.cursor))
        };
        Some((start, end))
    }

    /// The charwise selection as a DRAWABLE exclusive range: like
    /// `selection_range`, but the end never leaves the end line's text.
    /// The cursor may legitimately sit on the insert column one past the
    /// last char (a click past EOL clamps there via px_to_pos/Jump), and
    /// `advance`'s wrap to the next row start is right for text semantics
    /// (the newline counts as selected) but wrong for pixels: the band
    /// must stop at the last character cell instead of painting the empty
    /// space past EOL. Pure; the canvas maps it to rectangles.
    pub fn selection_draw_range(&self) -> Option<(Pos, Pos)> {
        let a = self.selection?;
        let c = self.cursor;
        let (start, end) = if self.before(c, a) { (c, a) } else { (a, c) };
        // end.col + 1: the char under the cursor side counts as covered
        // (same convention as selection_range's advance), clamped so an
        // insert-column end draws through the last char, not past it
        let end_col = (end.col + 1).min(self.line_len(end.row));
        Some((
            start,
            Pos {
                row: end.row,
                col: end_col,
            },
        ))
    }

    /// The charwise selection as a DRAWABLE exclusive range for the
    /// THIN-CARET modes (insert / regular): the caret is the insertion
    /// boundary, so the band stops AT it — no `+1`. (The inclusive
    /// `selection_draw_range` is the vim-visual convention, where the
    /// cursor sits ON the last selected char.) The end clamps to the end
    /// line's text the same way.
    pub fn selection_draw_range_exclusive(&self) -> Option<(Pos, Pos)> {
        let a = self.selection?;
        let c = self.cursor;
        let (start, end) = if self.before(c, a) { (c, a) } else { (a, c) };
        let end_col = end.col.min(self.line_len(end.row));
        Some((
            start,
            Pos {
                row: end.row,
                col: end_col,
            },
        ))
    }

    /// One cell past `p`, wrapping to the next line start after EOL.
    pub fn advance(&self, p: Pos) -> Pos {
        if p.col < self.line_len(p.row) {
            Pos {
                row: p.row,
                col: p.col + 1,
            }
        } else if p.row + 1 < self.lines.len() {
            Pos {
                row: p.row + 1,
                col: 0,
            }
        } else {
            p
        }
    }

    pub fn selection_text(&self) -> Option<String> {
        let ((sr, sc), (er, ec)) = {
            let (s, e) = self.selection_range()?;
            ((s.row, s.col), (e.row, e.col))
        };
        let mut out = String::new();
        if sr == er {
            out.extend(self.line(sr).chars().skip(sc).take(ec.saturating_sub(sc)));
        } else {
            out.push_str(&self.line(sr)[char_off(self.line(sr), sc)..]);
            out.push('\n');
            for l in &self.lines[sr + 1..er] {
                out.push_str(l);
                out.push('\n');
            }
            let l = self.line(er);
            out.push_str(&l[..char_off(l, ec)]);
        }
        Some(out)
    }

    // ---- selection control ----

    pub fn start_selection(&mut self) {
        self.selection = Some(self.cursor);
    }

    pub fn cancel_selection(&mut self) {
        self.selection = None;
    }

    /// Selected text, clearing the selection (vim visual `y`).
    pub fn copy_selection(&mut self) -> Option<String> {
        let t = self.selection_text();
        self.selection = None;
        t
    }

    /// Selected text, deleting it (vim visual `d`).
    pub fn cut_selection(&mut self) -> Option<String> {
        let t = self.selection_text()?;
        let (start, end) = self.selection_range().unwrap();
        self.selection = None;
        self.push_history();
        self.delete_range(start, end);
        t.into()
    }

    // ---- vim text objects (i… / a…) ----

    /// iw/aw: the whitespace-separated word run under the cursor (whitespace
    /// is the only separator, matching the w/e/b motions); `a` extends over
    /// trailing whitespace, else leading.
    pub fn word_object(&self, around: bool) -> Option<TextObj> {
        use CharKind::*;
        let (row, col) = (self.cursor.row, self.cursor.col);
        let chars: Vec<char> = self.line(row).chars().collect();
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
        if around && kind != Space {
            // trailing whitespace run, else leading (vim aw)
            let mut ne = e + 1;
            while ne < chars.len() && CharKind::new(chars[ne]) == Space {
                ne += 1;
            }
            if ne > e + 1 {
                return Some(TextObj {
                    start: Pos { row, col: s },
                    end: Pos { row, col: ne - 1 },
                    linewise: false,
                });
            }
            let mut ns = s;
            while ns > 0 && CharKind::new(chars[ns - 1]) == Space {
                ns -= 1;
            }
            if ns < s {
                return Some(TextObj {
                    start: Pos { row, col: ns },
                    end: Pos { row, col: e },
                    linewise: false,
                });
            }
        }
        Some(TextObj {
            start: Pos { row, col: s },
            end: Pos { row, col: e },
            linewise: false,
        })
    }

    /// ip/ap: the block of lines sharing the cursor line's blankness; `a`
    /// adds the trailing blank run (or the leading one at EOF, or the
    /// following paragraph when the cursor is on a blank line).
    pub fn paragraph_object(&self, around: bool) -> Option<TextObj> {
        let n = self.lines.len();
        if n == 0 {
            return None;
        }
        let blank = |i: usize| self.lines[i].trim().is_empty();
        let r = self.cursor.row.min(n - 1);
        let is_b = blank(r);
        let mut from = r;
        while from > 0 && blank(from - 1) == is_b {
            from -= 1;
        }
        let mut to = r;
        while to + 1 < n && blank(to + 1) == is_b {
            to += 1;
        }
        if around {
            if is_b {
                if to + 1 < n {
                    to += 1;
                    while to + 1 < n && !blank(to + 1) {
                        to += 1;
                    }
                }
            } else if to + 1 < n && blank(to + 1) {
                to += 1;
                while to + 1 < n && blank(to + 1) {
                    to += 1;
                }
            } else if from > 0 && blank(from - 1) {
                from -= 1;
                while from > 0 && blank(from - 1) {
                    from -= 1;
                }
            }
        }
        Some(TextObj {
            start: Pos { row: from, col: 0 },
            end: Pos {
                row: to,
                col: self.lines[to].chars().count().saturating_sub(1),
            },
            linewise: true,
        })
    }

    /// i"/a" (also `'` and `` ` ``): the quote pair enclosing the cursor on
    /// the current line. Inner excludes the quotes.
    pub fn quote_object(&self, q: char, around: bool) -> Option<TextObj> {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let chars: Vec<char> = self.line(row).chars().collect();
        let marks: Vec<usize> = chars
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == q)
            .map(|(i, _)| i)
            .collect();
        for w in marks.windows(2) {
            let (o, c) = (w[0], w[1]);
            if col >= o && col <= c {
                let (s, e) = if around { (o, c) } else { (o + 1, c - 1) };
                if s > e {
                    return None; // empty quoted string has no inner span
                }
                return Some(TextObj {
                    start: Pos { row, col: s },
                    end: Pos { row, col: e },
                    linewise: false,
                });
            }
        }
        None
    }

    /// i(/a{ (multiline, balanced): the innermost bracket pair enclosing the
    /// cursor. The cursor may sit on either bracket.
    pub fn bracket_object(&self, open: char, around: bool) -> Option<TextObj> {
        let close = close_of(open);
        let cur = self.cursor;
        let at =
            |p: Pos| -> Option<char> { self.lines.get(p.row).and_then(|l| l.chars().nth(p.col)) };
        let close_pos;
        if at(cur) == Some(open) {
            close_pos = self.match_forward(cur, open, close);
        } else {
            // scan forward for the first unmatched close
            let mut depth = 0i32;
            let mut p = cur;
            loop {
                match at(p) {
                    Some(c) if c == open => depth += 1,
                    Some(c) if c == close => {
                        if depth == 0 {
                            close_pos = Some(p);
                            break;
                        }
                        depth -= 1;
                    }
                    _ => {}
                }
                p = self.next_pos(p)?;
            }
        }
        let cp = close_pos?;
        // scan backward from the cursor for the open matching that close
        let mut open_pos = None;
        let mut depth = 0i32;
        let mut p = if at(cur) == Some(open) {
            cur
        } else {
            self.prev_pos(cur)?
        };
        loop {
            match at(p) {
                Some(c) if c == close => depth += 1,
                Some(c) if c == open => {
                    if depth == 0 {
                        open_pos = Some(p);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            if p.row == 0 && p.col == 0 {
                break;
            }
            p = self.prev_pos(p)?;
        }
        let op = open_pos?;
        let (s, e) = if around {
            (op, cp)
        } else if cp.row > op.row {
            // multi-line block (nvim): the content between the braces goes
            // (indentation included), but the first line's leading
            // indentation stays — the blank line that remains is indented
            // like the formatter would leave it. TextObj end is inclusive:
            // the last content char of the line above the closer.
            let first = op.row + 1;
            let indent = self.lines[first].chars().count()
                - self.lines[first].trim_start().chars().count();
            let start = Pos {
                row: first,
                col: indent,
            };
            let last = cp.row - 1;
            if self.lines[last].is_empty() || self.lines[first].trim().is_empty() {
                return None; // nothing between the braces
            }
            let end = Pos {
                row: last,
                col: self.lines[last].chars().count() - 1,
            };
            (start, end)
        } else {
            let s = self.next_pos(op)?;
            let e = self.prev_pos(cp)?;
            if self.before(e, s) {
                return None; // empty body: no inner span
            }
            (s, e)
        };
        Some(TextObj {
            start: s,
            end: e,
            linewise: false,
        })
    }

    pub fn text_object(&self, around: bool, kind: ObjKind) -> Option<TextObj> {
        match kind {
            ObjKind::Word => self.word_object(around),
            ObjKind::Paragraph => self.paragraph_object(around),
            ObjKind::Quote(q) => self.quote_object(q, around),
            ObjKind::Bracket(open) => self.bracket_object(open, around),
        }
    }

    /// Char after `p` in buffer order (row-major).
    fn next_pos(&self, p: Pos) -> Option<Pos> {
        let len = self.lines.get(p.row)?.chars().count();
        if p.col + 1 < len {
            Some(Pos {
                row: p.row,
                col: p.col + 1,
            })
        } else if p.row + 1 < self.lines.len() {
            Some(Pos {
                row: p.row + 1,
                col: 0,
            })
        } else {
            None
        }
    }

    /// Char before `p` in buffer order.
    fn prev_pos(&self, p: Pos) -> Option<Pos> {
        if p.col > 0 {
            Some(Pos {
                row: p.row,
                col: p.col - 1,
            })
        } else if p.row > 0 {
            let row = p.row - 1;
            Some(Pos {
                row,
                col: self.lines[row].chars().count().saturating_sub(1),
            })
        } else {
            None
        }
    }

    /// Buffer position of the `close` matching the `open` AT `from`.
    fn match_forward(&self, from: Pos, open: char, close: char) -> Option<Pos> {
        let at =
            |p: Pos| -> Option<char> { self.lines.get(p.row).and_then(|l| l.chars().nth(p.col)) };
        let mut depth = 0i32;
        let mut p = from;
        loop {
            if let Some(c) = at(p) {
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(p);
                    }
                }
            }
            p = self.next_pos(p)?;
        }
    }

    /// Text of an inclusive char span (or whole rows when linewise).
    pub fn range_text(&self, obj: TextObj) -> String {
        if obj.linewise {
            self.lines[obj.start.row..=obj.end.row].join("\n")
        } else if obj.start.row == obj.end.row {
            let line = self.line(obj.start.row);
            let s = char_off(line, obj.start.col);
            let e = char_off(line, obj.end.col + 1);
            line[s..e].to_string()
        } else {
            let mut out = String::new();
            let first = self.line(obj.start.row);
            out.push_str(&first[char_off(first, obj.start.col)..]);
            out.push('\n');
            for row in obj.start.row + 1..obj.end.row {
                out.push_str(&self.lines[row]);
                out.push('\n');
            }
            let last = self.line(obj.end.row);
            out.push_str(&last[..char_off(last, obj.end.col + 1)]);
            out
        }
    }

    /// Column of the `count`-th `ch` on the cursor's line for the f/F/t/T
    /// motions (till stops one short of the match, always making progress).
    pub fn find_char_col(
        &self,
        forward: bool,
        till: bool,
        ch: char,
        count: usize,
    ) -> Option<usize> {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let chars: Vec<char> = self.line(row).chars().collect();
        let mut hits = 0usize;
        if forward {
            for (c, x) in chars.iter().enumerate().skip(col + 1) {
                if *x == ch {
                    hits += 1;
                    if hits == count {
                        return if till {
                            (c > col + 1).then_some(c - 1)
                        } else {
                            Some(c)
                        };
                    }
                }
            }
        } else {
            for (c, x) in chars.iter().enumerate().take(col).rev() {
                if *x == ch {
                    hits += 1;
                    if hits == count {
                        return if till {
                            (c + 1 < col).then_some(c + 1)
                        } else {
                            Some(c)
                        };
                    }
                }
            }
        }
        None
    }

    // ---- vim-surround ----

    /// The (open, close) glyphs a surround target char stands for; quotes
    /// map to themselves, and vim-surround's alias chars resolve to their
    /// pair (`b` parens, `B` braces, `r` brackets, `a` angle brackets).
    pub fn surround_pair(c: char) -> Option<(char, char)> {
        match c {
            '\'' => Some(('\'', '\'')),
            '"' => Some(('"', '"')),
            '`' => Some(('`', '`')),
            '(' | ')' | 'b' => Some(('(', ')')),
            '[' | ']' | 'r' => Some(('[', ']')),
            '{' | '}' | 'B' => Some(('{', '}')),
            '<' | '>' | 'a' => Some(('<', '>')),
            _ => None,
        }
    }

    /// Delete the surrounding pair of kind `target` around the cursor.
    /// Returns false when no such enclosing pair exists.
    pub fn delete_surround(&mut self, target: char) -> bool {
        let obj = self.surround_span(target);
        let Some(o) = obj else { return false };
        self.push_history();
        self.delete_range(o.end, self.advance(o.end));
        self.delete_range(o.start, self.advance(o.start));
        self.move_cursor(Move::Jump(o.start.row, o.start.col));
        true
    }

    /// Change the surrounding pair `target` to `replacement`.
    pub fn change_surround(&mut self, target: char, replacement: char) -> bool {
        let Some(o) = self.surround_span(target) else {
            return false;
        };
        let Some((no, nc)) = Self::surround_pair(replacement) else {
            return false;
        };
        self.push_history();
        let mut line = self.lines[o.end.row].clone();
        replace_char_at(&mut line, o.end.col, nc);
        self.lines[o.end.row] = line;
        let mut line = self.lines[o.start.row].clone();
        replace_char_at(&mut line, o.start.col, no);
        self.lines[o.start.row] = line;
        self.move_cursor(Move::Jump(o.start.row, o.start.col));
        true
    }

    /// Insert `pair` around the inclusive span `obj` as one undo unit.
    pub fn surround_insert(&mut self, obj: TextObj, pair: (char, char)) {
        self.push_history();
        self.insert_at(
            Pos {
                row: obj.end.row,
                col: obj.end.col + 1,
            },
            pair.1,
        );
        self.insert_at(obj.start, pair.0);
        self.move_cursor(Move::Jump(obj.start.row, obj.start.col));
    }

    /// Raw single-char insert at a position: no history push (the caller's
    /// push_history covers the whole pair), no selection (cleared first).
    fn insert_at(&mut self, p: Pos, c: char) {
        self.move_cursor(Move::Jump(p.row, p.col));
        self.delete_selection();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let line = &mut self.lines[r];
        let off = char_off(line, cpos);
        line.insert(off, c);
        self.cursor.col = cpos + 1;
    }

    /// The enclosing span (quotes or brackets, both glyphs included) around
    /// the cursor for a surround target char.
    fn surround_span(&self, target: char) -> Option<TextObj> {
        match target {
            '\'' | '"' | '`' => self.quote_object(target, true),
            c => {
                let open = match c {
                    // vim-surround aliases and closers normalize to opens
                    ')' | 'b' => '(',
                    ']' | 'r' => '[',
                    '}' | 'B' => '{',
                    '>' | 'a' => '<',
                    _ => c,
                };
                self.bracket_object(open, true)
            }
        }
    }

    /// Replace the whole text as ONE undo step (used by the formatter).
    pub fn replace_all(&mut self, text: &str) {
        self.push_history();
        // from_text builds a fresh buffer; keep this buffer's history and
        // cursor bookkeeping so the replace is one undo step
        let mut next = Self::from_text(text);
        std::mem::swap(&mut next.undo, &mut self.undo);
        std::mem::swap(&mut next.redo, &mut self.redo);
        let search = self.search.clone();
        let hl_hidden = self.search_hl_hidden;
        *self = next;
        self.search = search;
        self.search_hl_hidden = hl_hidden;
    }

    // ---- cursor movement ----

    /// Port of tui-textarea's `next_cursor` (no viewport moves): computes
    /// the raw target for a move, or None when the cursor cannot move.
    fn next_cursor(&self, (row, col): (usize, usize), m: Move) -> Option<(usize, usize)> {
        let lines = &self.lines;
        let fit_col = |col: usize, line: &str| col.min(line.chars().count());
        match m {
            Move::Forward => {
                if col >= lines[row].chars().count() {
                    (row + 1 < lines.len()).then(|| (row + 1, 0))
                } else {
                    Some((row, col + 1))
                }
            }
            Move::Back => {
                if col == 0 {
                    let row = row.checked_sub(1)?;
                    Some((row, lines[row].chars().count()))
                } else {
                    Some((row, col - 1))
                }
            }
            Move::Up => {
                let row = row.checked_sub(1)?;
                // aim at the REMEMBERED column, not the current one: a
                // short/blank line clamps the landing col only, so the
                // aim survives the crossing (vim's curswant)
                Some((row, fit_col(self.desired_col, &lines[row])))
            }
            Move::Down => Some((
                row + 1,
                fit_col(self.desired_col, lines.get(row + 1)?),
            )),
            Move::Head => Some((row, 0)),
            Move::End => Some((row, lines[row].chars().count())),
            Move::EndMax => Some((row, lines[row].chars().count())),
            Move::Top => Some((0, fit_col(col, &lines[0]))),
            Move::Bottom => {
                let row = lines.len() - 1;
                Some((row, fit_col(col, &lines[row])))
            }
            Move::WordEnd => {
                // `+ 1` for not accepting the current cursor position
                if let Some(col) = find_word_inclusive_end_forward(&lines[row], col + 1) {
                    Some((row, col))
                } else {
                    let mut row = row;
                    loop {
                        if row == lines.len() - 1 {
                            break Some((row, lines[row].chars().count()));
                        }
                        row += 1;
                        if let Some(col) = find_word_inclusive_end_forward(&lines[row], 0) {
                            break Some((row, col));
                        }
                    }
                }
            }
            Move::WordForward => {
                if let Some(col) = find_word_start_forward(&lines[row], col) {
                    Some((row, col))
                } else if row + 1 < lines.len() {
                    Some((row + 1, 0))
                } else {
                    Some((row, lines[row].chars().count()))
                }
            }
            Move::WordBack => {
                if let Some(col) = find_word_start_backward(&lines[row], col) {
                    Some((row, col))
                } else if row > 0 {
                    Some((row - 1, lines[row - 1].chars().count()))
                } else {
                    Some((row, 0))
                }
            }
            Move::ParagraphForward => {
                let mut prev_is_empty = lines[row].is_empty();
                #[allow(clippy::needless_range_loop)] // verbatim tui-textarea port
                for row in row + 1..lines.len() {
                    let is_empty = lines[row].is_empty();
                    if !is_empty && prev_is_empty {
                        return Some((row, fit_col(col, &lines[row])));
                    }
                    prev_is_empty = is_empty;
                }
                let row = lines.len() - 1;
                Some((row, fit_col(col, &lines[row])))
            }
            Move::ParagraphBack => {
                let row = row.checked_sub(1)?;
                let mut prev_is_empty = lines[row].is_empty();
                for row in (0..row).rev() {
                    let is_empty = lines[row].is_empty();
                    if is_empty && !prev_is_empty {
                        return Some((row + 1, fit_col(col, &lines[row + 1])));
                    }
                    prev_is_empty = is_empty;
                }
                Some((0, fit_col(col, &lines[0])))
            }
            Move::Jump(row, col) => {
                let row = row.min(lines.len() - 1);
                let col = fit_col(col, &lines[row]);
                Some((row, col))
            }
            Move::GotoRow(row) => {
                // curswant semantics: aim at the remembered column, keep
                // the aim across short/blank lines (move_cursor preserves
                // it for this move)
                lines.get(row.min(lines.len() - 1))
                    .map(|l| (row.min(lines.len() - 1), fit_col(self.desired_col, l)))
            }
        }
    }

    pub fn move_cursor(&mut self, m: Move) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        if let Some(next) = self.next_cursor((row, col), m) {
            self.cursor.row = next.0;
            self.cursor.col = next.1;
            // vim's curswant: vertical moves AIM at the remembered column
            // without consuming it (short lines clamp the landing only);
            // every other motion re-aims, and `$` aims at every line's
            // end until a horizontal motion re-aims.
            match m {
                Move::Up | Move::Down | Move::GotoRow(_) => {}
                Move::EndMax => self.desired_col = usize::MAX,
                _ => self.desired_col = next.1,
            }
        }
    }

    fn set_col(&mut self, col: usize) {
        self.cursor.col = col;
        // edits move the cursor horizontally: the vertical aim follows
        // (typing then `j` aims from the new column, like vim)
        self.desired_col = col;
    }

    /// Clamp the cursor column onto the line's last char (col len-1;
    /// empty lines: 0) WITHOUT touching the vertical-move aim — vim
    /// normal/visual never rest on the past-EOL insert cell, but `$`'s
    /// MAXCOL aim must survive the clamp so the next `j` still rides the
    /// line ends. Insert mode skips this: its caret lives past the last
    /// char by design.
    pub fn clamp_cursor_to_line_end(&mut self) {
        let max = self.lines[self.cursor.row].chars().count().saturating_sub(1);
        if self.cursor.col > max {
            self.cursor.col = max;
        }
    }

    fn before(&self, a: Pos, b: Pos) -> bool {
        (a.row, a.col) < (b.row, b.col)
    }

    /// Chars in a row (cursor clamping when restoring a session).
    pub fn line_len(&self, row: usize) -> usize {
        self.line(row).chars().count()
    }

    // ---- editing ----

    pub fn insert_char(&mut self, c: char) {
        self.push_history();
        self.delete_selection();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let line = &mut self.lines[r];
        let off = char_off(line, cpos);
        line.insert(off, c);
        self.set_col(cpos + 1);
    }

    pub fn insert_str(&mut self, s: &str) {
        self.push_history();
        self.delete_selection();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let line = &mut self.lines[r];
        let off = char_off(line, cpos);
        line.insert_str(off, s);
        self.set_col(cpos + s.chars().count());
    }

    pub fn insert_newline(&mut self) {
        self.push_history();
        self.delete_selection();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let mut line = self.lines[r].clone();
        let off = char_off(&line, cpos);
        let rest = line.split_off(off);
        self.lines[r] = line;
        self.lines.insert(r + 1, rest);
        self.cursor.row += 1;
        self.set_col(0);
    }

    /// `o`: open a line below the cursor's line, wearing the indent the
    /// position calls for (see `opened_indent`). Atomic with the cursor
    /// the user actually occupied — the undo snapshot must hold the
    /// PRE-command position, not the line-end spot a move-first
    /// implementation would record (undoing `o` used to land on a
    /// fabricated end-of-line column, and the left-ctrl chord salvage
    /// then pushed that ghost into the jumplist).
    pub fn open_line_below(&mut self) {
        self.push_history();
        let r = self.cursor.row;
        let indent = opened_indent(&self.lines[r], true);
        self.lines.insert(r + 1, indent.clone());
        self.cursor.row += 1;
        self.set_col(indent.chars().count());
    }

    /// `O`: open a line above, the cursor riding up onto it (the old
    /// line shifts down untouched). The new line sits where the cursor
    /// line's own position dictates: the same indent for a body line or
    /// a block closer, the opener's own (shallower) indent when the
    /// cursor is ON the opener — above it is outside the block. Same
    /// snapshot contract as `open_line_below`.
    pub fn open_line_above(&mut self) {
        self.push_history();
        let r = self.cursor.row;
        let indent = opened_indent(&self.lines[r], false);
        self.lines.insert(r, indent.clone());
        self.set_col(indent.chars().count());
    }

    /// Backspace.
    pub fn delete_char(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.push_history();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        if cpos > 0 {
            let line = &mut self.lines[r];
            let start = char_off(line, cpos - 1);
            let end = char_off(line, cpos);
            line.replace_range(start..end, "");
            self.set_col(cpos - 1);
        } else if r > 0 {
            let line = self.lines.remove(r);
            let prev_len = self.lines[r - 1].chars().count();
            self.lines[r - 1].push_str(&line);
            self.cursor.row -= 1;
            self.set_col(prev_len);
        }
    }

    /// Delete at cursor (`x`).
    pub fn delete_next_char(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.push_history();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let len = self.line_len(r);
        if cpos < len {
            let line = &mut self.lines[r];
            let start = char_off(line, cpos);
            let end = char_off(line, cpos + 1);
            line.replace_range(start..end, "");
        } else if r + 1 < self.lines.len() {
            let next = self.lines.remove(r + 1);
            self.lines[r].push_str(&next);
        }
    }

    /// Delete `chars` characters forward from the cursor; newlines count as
    /// one character and crossing lines joins them. Deletes an active
    /// selection first.
    /// Replace the `chars` chars at (`row`, `col`) with `text` as ONE
    /// undo step — the quick-fix splice: a plain delete+insert splits into
    /// two history entries, and undo then walks the fix back in halves
    /// with the caret teleporting between them. The snapshot's caret is
    /// the CALLER's position (pre-move), so undo returns there.
    pub fn replace_span(&mut self, row: usize, col: usize, chars: usize, text: &str) {
        self.push_history();
        let _ = self.delete_selection();
        self.move_cursor(Move::Jump(row, col));
        if chars > 0 {
            self.delete_str_raw(chars);
        }
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let line = &mut self.lines[r];
        let off = char_off(line, cpos);
        line.insert_str(off, text);
        self.set_col(cpos + text.chars().count());
    }

    pub fn delete_str(&mut self, chars: usize) {
        if self.delete_selection() {
            return;
        }
        if chars == 0 {
            return;
        }
        self.push_history();
        self.delete_str_raw(chars);
    }

    /// The (char column, commented?) state of a line's `//` opener at its
    /// first non-blank char.
    fn comment_state(line: &str) -> (usize, bool) {
        let col = line.chars().take_while(|c| c.is_whitespace()).count();
        let commented = line.chars().skip(col).take(2).eq(['/', '/']);
        (col, commented)
    }

    /// Move the inclusive row range up/down one line (a rotation: the
    /// neighbor line swaps places with the block). One undo unit. False
    /// at the document edges (no move).
    pub fn move_lines(&mut self, first: usize, last: usize, down: bool) -> bool {
        if down {
            if last + 1 >= self.lines.len() {
                return false;
            }
            self.push_history();
            let below = self.lines.remove(last + 1);
            self.lines.insert(first, below);
        } else {
            if first == 0 {
                return false;
            }
            self.push_history();
            let above = self.lines.remove(first - 1);
            self.lines.insert(last, above);
        }
        true
    }

    /// Toggle a `//` line comment on `row` at its first non-blank column
    /// (one undo unit): `// text` becomes `text`, `text` becomes `// text`.
    pub fn toggle_line_comment(&mut self, row: usize) {
        // the raw ops delete the active selection first: clear it so a
        // live visual highlight is not eaten by the edit
        self.cancel_selection();
        let line = self.lines[row].clone();
        let (col, commented) = Self::comment_state(&line);
        self.push_history();
        self.move_cursor(Move::Jump(row, col));
        if commented {
            let remove = if line.chars().nth(col + 2) == Some(' ') { 3 } else { 2 };
            self.delete_str_raw(remove);
        } else {
            let ins = if line.trim().is_empty() { "//" } else { "// " };
            self.insert_str_raw(ins);
        }
    }

    /// Toggle `//` over the inclusive row range as ONE undo unit:
    /// uncomment only when EVERY line is commented, otherwise comment
    /// them all (vim-commentary's visual gc).
    pub fn toggle_comment_rows(&mut self, first: usize, last: usize) {
        // clear the visual highlight before editing (the raw ops would
        // otherwise delete the selection text on the first insert)
        self.cancel_selection();
        let all_commented = (first..=last).all(|r| Self::comment_state(&self.lines[r]).1);
        self.push_history();
        for r in first..=last {
            let line = self.lines[r].clone();
            let (col, commented) = Self::comment_state(&line);
            self.move_cursor(Move::Jump(r, col));
            if all_commented {
                let remove = if line.chars().nth(col + 2) == Some(' ') { 3 } else { 2 };
                self.delete_str_raw(remove);
            } else if !commented {
                // mixed: comment the bare lines only — a line already
                // carrying // never gains a second //
                let ins = if line.trim().is_empty() { "//" } else { "// " };
                self.insert_str_raw(ins);
            }
        }
    }

    /// `delete_str` without the undo snapshot: compose with `push_history`
    /// and other raw ops to make a multi-step edit ONE undo unit.
    fn delete_str_raw(&mut self, chars: usize) {
        let start = self.cursor;
        // walk forward `chars` chars across lines
        let mut remaining = chars;
        let mut r = start.row;
        // first line
        let line_len = self.line_len(r);
        if start.col + remaining <= line_len {
            self.delete_span(
                start,
                Pos {
                    row: r,
                    col: start.col + remaining,
                },
            );
            return;
        }
        remaining -= line_len - start.col; // rest of line, plus...
        remaining -= 1; // ...the newline itself
        r += 1;
        while r < self.lines.len() {
            let len = self.line_len(r);
            if remaining <= len {
                self.delete_span(
                    start,
                    Pos {
                        row: r,
                        col: remaining,
                    },
                );
                return;
            }
            remaining -= len + 1;
            r += 1;
        }
        // ran off the end: delete to end of buffer
        self.delete_span(
            start,
            Pos {
                row: self.lines.len() - 1,
                col: self.line_len(self.lines.len() - 1),
            },
        );
    }

    /// vim [count]cc / linewise c: replace the `n` lines at the cursor with
    /// one empty editable line as a SINGLE undo unit (the delete-line step
    /// and the re-inserted line break share one snapshot, so one `u`
    /// restores the original lines). The cursor lands on the empty line,
    /// column 0.
    pub fn change_lines(&mut self, n: usize) {
        let row = self.cursor.row.min(self.lines.len().saturating_sub(1));
        self.selection = None;
        self.cursor = Pos { row, col: 0 };
        let end = (row + n.max(1)).min(self.lines.len());
        let total: usize = self.lines[row..end]
            .iter()
            .map(|l| l.chars().count() + 1)
            .sum();
        self.push_history();
        if total > 0 {
            self.delete_str_raw(total);
        }
        self.insert_newline_raw();
        self.move_cursor(Move::Up);
    }

    /// vim `r`: replace `n` chars starting at the cursor with `c` as a
    /// single undo unit. Returns false (unchanged) if fewer than `n` chars
    /// remain on the line.
    pub fn replace_next(&mut self, n: usize, c: char) -> bool {
        if n == 0 {
            return false;
        }
        let (r, col) = (self.cursor.row, self.cursor.col);
        let len = self.line(r).chars().count();
        if col + n > len {
            return false;
        }
        self.push_history();
        let line = &mut self.lines[r];
        let start = char_off(line, col);
        let end = char_off(line, col + n);
        let replacement: String = std::iter::repeat(c).take(n).collect();
        line.replace_range(start..end, &replacement);
        true
    }

    /// vim V-u / V-U: re-case `n` whole lines starting at `from` as one
    /// undo unit; the cursor lands on the first changed line.
    pub fn apply_lines_case(&mut self, from: usize, n: usize, upper: bool) -> bool {
        if n == 0 || from >= self.lines.len() {
            return false;
        }
        self.push_history();
        let end = (from + n).min(self.lines.len());
        for line in &mut self.lines[from..end] {
            *line = if upper {
                line.chars().flat_map(|c| c.to_uppercase()).collect()
            } else {
                line.chars().flat_map(|c| c.to_lowercase()).collect()
            };
        }
        // the cursor stays put so a live visual-line selection keeps its
        // extent across repeated u/U
        self.cursor.col = self.cursor.col.min(self.line_len(self.cursor.row));
        true
    }

    /// vim `r<CR>`: replace the char under the cursor with a newline
    /// (single undo unit).
    pub fn replace_with_newline(&mut self) -> bool {
        let len = self.line_len(self.cursor.row);
        if self.cursor.col >= len {
            return false;
        }
        self.selection = None;
        self.push_history();
        self.delete_next_char_raw();
        self.insert_newline_raw();
        true
    }

    /// vim visual `u`/`U`: re-case the selection in place as one undo
    /// unit; keeps visual mode by re-selecting the transformed range.
    /// Returns false when nothing is selected.
    pub fn apply_selection_case(&mut self, upper: bool) -> bool {
        let Some((start, end)) = self.selection_range() else {
            return false;
        };
        let Some(text) = self.selection_text() else {
            return false;
        };
        // clear before the raw ops: delete/insert re-check the (stale)
        // selection internally
        self.selection = None;
        self.push_history();
        let transformed: String = if upper {
            text.chars().flat_map(|c| c.to_uppercase()).collect()
        } else {
            text.chars().flat_map(|c| c.to_lowercase()).collect()
        };
        self.delete_range(start, end);
        self.cursor = start;
        self.insert_str_raw(&transformed);
        // keep the selection over the (possibly re-sized) result: the
        // cursor sits ON the last transformed char (selections are
        // inclusive of the cursor cell)
        self.selection = Some(start);
        let new_len = transformed.chars().count();
        self.cursor.col = start.col + new_len.saturating_sub(1);
        true
    }

    /// Delete from the cursor to the end of the line (newline stays).
    pub fn delete_line_by_end(&mut self) {
        if self.delete_selection() {
            return;
        }
        self.push_history();
        let r = self.cursor.row;
        let off = char_off(&self.lines[r], self.cursor.col);
        self.lines[r].truncate(off);
    }

    fn delete_next_char_raw(&mut self) {
        if self.delete_selection() {
            return;
        }
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let len = self.line_len(r);
        if cpos < len {
            let line = &mut self.lines[r];
            let start = char_off(line, cpos);
            let end = char_off(line, cpos + 1);
            line.replace_range(start..end, "");
        } else if r + 1 < self.lines.len() {
            let next = self.lines.remove(r + 1);
            self.lines[r].push_str(&next);
        }
    }

    pub(crate) fn insert_newline_raw(&mut self) {
        self.delete_selection();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let mut line = self.lines[r].clone();
        let off = char_off(&line, cpos);
        let rest = line.split_off(off);
        self.lines[r] = line;
        self.lines.insert(r + 1, rest);
        self.cursor.row += 1;
        self.cursor.col = 0;
    }

    pub(crate) fn insert_str_raw(&mut self, s: &str) {
        self.delete_selection();
        let (r, cpos) = (self.cursor.row, self.cursor.col);
        let line = &mut self.lines[r];
        let off = char_off(line, cpos);
        line.insert_str(off, s);
        self.cursor.col = cpos + s.chars().count();
    }

    fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.selection_range() {
            self.selection = None;
            self.delete_range(start, end);
            true
        } else {
            false
        }
    }

    pub fn delete_range(&mut self, start: Pos, end: Pos) {
        // an exclusive `end` at or past a row's end with the span starting
        // at a row start covers whole rows: drop them (vim-surround's ds/cs
        // join full-line spans instead of leaving empty shells). Span-based
        // deletions that must keep the trailing newline call `delete_span`
        // directly — see delete_str_raw's walk.
        if start.row < end.row
            && start.col == 0
            // an exclusive end at col 0 starts a row, it never covers one
            // (an empty row's "one past the end" is col 0, but so is its
            // start — ambiguous, so never join those)
            && end.col > 0
            && end.col >= self.line_len(end.row)
            && end.row + 1 < self.lines.len()
        {
            self.lines.drain(start.row..=end.row);
            if self.lines.is_empty() {
                self.lines.push(String::new());
            }
            self.cursor = Pos {
                row: start.row.min(self.lines.len() - 1),
                col: 0,
            };
            return;
        }
        self.delete_span(start, end);
    }

    /// Plain exclusive-range deletion: never joins whole rows — an end at
    /// a row's newline slot keeps that newline (an empty row remains).
    fn delete_span(&mut self, start: Pos, end: Pos) {
        self.cursor = start;
        if start.row == end.row {
            let line = &mut self.lines[start.row];
            let s = char_off(line, start.col);
            let e = char_off(line, end.col);
            line.replace_range(s..e, "");
        } else {
            let head_off = char_off(&self.lines[start.row], start.col);
            self.lines[start.row].truncate(head_off);
            let mut rest = self.lines[end.row].clone();
            let cut = char_off(&rest, end.col);
            let tail = rest.split_off(cut);
            self.lines[start.row].push_str(&tail);
            self.lines.drain(start.row + 1..=end.row);
        }
    }

    // ---- undo/redo ----

    /// An undo step is available (gates the toolbar Undo button).
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Entries on the undo stack: the left-ctrl pair salvage compares this
    /// across a bare letter to learn whether that copy edited the buffer.
    pub fn history_len(&self) -> usize {
        self.undo.len()
    }

    /// A redo step is available (gates the toolbar Redo button).
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// One history snapshot for a composite edit (vim's put): subsequent
    /// raw mutations share this single undo unit.
    pub fn begin_edit(&mut self) {
        self.push_history();
    }

    fn push_history(&mut self) {
        let snap = Snapshot {
            lines: self.lines.clone(),
            cursor: self.cursor,
            selection: self.selection,
        };
        self.undo.push(snap);
        self.redo.clear();
    }

    pub fn undo(&mut self) -> bool {
        if let Some(snap) = self.undo.pop() {
            self.redo.push(Snapshot {
                lines: std::mem::replace(&mut self.lines, snap.lines),
                cursor: self.cursor,
                selection: self.selection,
            });
            self.cursor = snap.cursor;
            self.selection = snap.selection;
            self.desired_col = snap.cursor.col;
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some(snap) = self.redo.pop() {
            self.undo.push(Snapshot {
                lines: std::mem::replace(&mut self.lines, snap.lines),
                cursor: self.cursor,
                selection: self.selection,
            });
            self.cursor = snap.cursor;
            self.selection = snap.selection;
            self.desired_col = snap.cursor.col;
            true
        } else {
            false
        }
    }

    // ---- search ----

    /// Compile a regex pattern; returns false on invalid pattern (keeps
    /// the previous one). A fresh pattern shows its match bands again.
    pub fn set_search_pattern(&mut self, pattern: &str) -> bool {
        match Regex::new(pattern) {
            Ok(re) => {
                self.search = Some(re);
                self.search_hl_hidden = false;
                true
            }
            Err(_) => false,
        }
    }

    #[allow(dead_code)] // kept tui-textarea API surface
    pub fn has_search_pattern(&self) -> bool {
        self.search.is_some()
    }

    /// Move the cursor to the next match at-or-after the cursor (wrapping).
    /// When `from_cursor` is false, a match starting exactly at the cursor
    /// is skipped (used for `n`).
    /// Drop the search pattern (the GUI find's × / empty query).
    pub fn clear_search(&mut self) {
        self.search = None;
        self.search_hl_hidden = false;
    }

    /// `:noh`: hide the match bands while keeping the pattern — `n`/`N`
    /// keep cycling (vim's hlsearch-off).
    pub fn hide_search_hl(&mut self) {
        self.search_hl_hidden = true;
    }

    /// The match spans as they DRAW: empty while the highlights are
    /// hidden (`hide_search_hl`), the real spans otherwise. Cycling and
    /// the find bar's "n of m" read `search_match_spans` instead.
    pub fn search_hl_spans(&self) -> Vec<(usize, usize, usize)> {
        if self.search_hl_hidden {
            Vec::new()
        } else {
            self.search_match_spans()
        }
    }

    /// Every search match as (row, start col, end col) spans — the
    /// engine's matches never span lines — in document order.
    pub fn search_match_spans(&self) -> Vec<(usize, usize, usize)> {
        let Some(re) = &self.search else { return Vec::new() };
        let mut out = Vec::new();
        for (r, line) in self.lines.iter().enumerate() {
            for m in re.find_iter(line) {
                let start = line[..m.start()].chars().count();
                let end = line[..m.end()].chars().count();
                if end > start {
                    out.push((r, start, end));
                }
            }
        }
        out
    }

    pub fn search_forward(&mut self, from_cursor: bool) -> bool {
        self.search_next(true, from_cursor)
    }

    pub fn search_back(&mut self, from_cursor: bool) -> bool {
        self.search_next(false, from_cursor)
    }

    fn search_next(&mut self, forward: bool, from_cursor: bool) -> bool {
        let Some(re) = &self.search else { return false };
        let mut found: Option<Pos> = None;
        for r in 0..self.lines.len() {
            let line = self.line(r);
            for m in re.find_iter(line) {
                let col = line[..m.start()].chars().count();
                let pos = Pos { row: r, col };
                let hits = if from_cursor {
                    // at-or-after the cursor
                    if forward {
                        !self.before(pos, self.cursor)
                    } else {
                        !self.before(self.cursor, pos)
                    }
                } else if forward {
                    self.before(self.cursor, pos)
                } else {
                    self.before(pos, self.cursor)
                };
                if hits {
                    if forward {
                        found = Some(pos);
                        break;
                    }
                    found = Some(pos); // keep last (closest above)
                }
            }
            if found.is_some() && forward {
                break;
            }
        }
        // wrap: nothing ahead — take the first (forward) / last (back)
        if found.is_none() {
            let rows: Vec<usize> = if forward {
                (0..self.lines.len()).collect()
            } else {
                (0..self.lines.len()).rev().collect()
            };
            'outer: for r in rows {
                let line = self.line(r);
                let ms: Vec<_> = re.find_iter(line).collect();
                let first = if forward { ms.first() } else { ms.last() };
                if let Some(m) = first {
                    let col = line[..m.start()].chars().count();
                    found = Some(Pos { row: r, col });
                    break 'outer;
                }
            }
        }
        if let Some(p) = found {
            self.cursor = p;
            self.desired_col = p.col;
            true
        } else {
            false
        }
    }
}

fn replace_char_at(s: &mut String, col: usize, c: char) {
    let off = char_off(s, col);
    s.replace_range(off..off + c.len_utf8(), &c.to_string());
}

fn char_off(s: &str, col: usize) -> usize {
    s.char_indices().nth(col).map(|(i, _)| i).unwrap_or(s.len())
}

/// The indent a vim `o`/`O` line should wear, given the cursor line:
/// vim's autoindent (the new line inherits the reference line's leading
/// whitespace), plus — for the line BELOW — the smart-enter block rule
/// (`Editor::smart_enter`'s same unit): code that ends in `{` opens a
/// block, so the line under it sits INSIDE the block, one unit deeper.
/// A `//` comment tail never opens a block (the smart-enter rule).
fn opened_indent(reference: &str, below: bool) -> String {
    const UNIT: &str = "  ";
    let base: String = reference.chars().take_while(|ch| ch.is_whitespace()).collect();
    if !below {
        return base;
    }
    let code = reference.split("//").next().unwrap_or("").trim_end();
    if code.ends_with('{') {
        format!("{base}{UNIT}")
    } else {
        base
    }
}

// ---- vim-style word scanning ----

/// Word-motion character class. Whitespace is the ONLY separator: runs of
/// non-whitespace form one word, so `snake_case` and `[pk]` each move as a
/// single `w`/`e`/`b` unit (vim's `iskeyword` with `_`, tuned for DBML
/// identifiers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharKind {
    Space,
    Punct,
    Other,
}

impl CharKind {
    pub fn new(c: char) -> Self {
        if c.is_whitespace() {
            Self::Space
        } else if c.is_ascii_punctuation() && c != '_' {
            // symbols (, ; : brackets …) separate words; `_` does not —
            // snake_case identifiers move as one unit (vim iskeyword)
            Self::Punct
        } else {
            Self::Other
        }
    }
}

/// Next word start after `start_col` (first char of the next word run).
fn find_word_start_forward(line: &str, start_col: usize) -> Option<usize> {
    let mut it = line.chars().enumerate().skip(start_col);
    let mut prev = CharKind::new(it.next()?.1);
    for (col, c) in it {
        let cur = CharKind::new(c);
        if cur != CharKind::Space && prev != cur {
            return Some(col);
        }
        prev = cur;
    }
    None
}

/// End of the current/next word run (vim `e`), inclusive semantics: the
/// caller advances the start column to skip the current position.
fn find_word_inclusive_end_forward(line: &str, start_col: usize) -> Option<usize> {
    let mut it = line.chars().enumerate().skip(start_col);
    let (mut last_col, c) = it.next()?;
    let mut prev = CharKind::new(c);
    for (col, c) in it {
        let cur = CharKind::new(c);
        if prev != CharKind::Space && cur != prev {
            return Some(col.saturating_sub(1));
        }
        prev = cur;
        last_col = col;
    }
    if prev != CharKind::Space {
        Some(last_col)
    } else {
        None
    }
}

/// Start of the word run at-or-before `start_col` (vim `b`).
fn find_word_start_backward(line: &str, start_col: usize) -> Option<usize> {
    let idx = line
        .char_indices()
        .nth(start_col)
        .map(|(i, _)| i)
        .unwrap_or(line.len());
    let mut it = line[..idx].chars().rev().enumerate();
    let mut cur = CharKind::new(it.next()?.1);
    for (i, c) in it {
        let next = CharKind::new(c);
        if cur != CharKind::Space && next != cur {
            return Some(start_col - i);
        }
        cur = next;
    }
    (cur != CharKind::Space).then_some(0)
}

#[cfg(test)]
mod tests {

    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::from_text(s)
    }

    #[test]
    fn basic_motions_and_wraps() {
        // End/Head stay on the line (tui-textarea semantics the engine was
        // tuned against); Forward/Back wrap across lines.
        let mut b = buf("abc\ndef");
        b.move_cursor(Move::End);
        assert_eq!(b.pos(), Pos { row: 0, col: 3 });
        b.move_cursor(Move::End);
        assert_eq!(b.pos(), Pos { row: 0, col: 3 }, "End does not wrap");
        b.move_cursor(Move::Forward);
        assert_eq!(b.pos(), Pos { row: 1, col: 0 }, "Forward wraps at EOL");
        b.move_cursor(Move::Jump(1, 2));
        b.move_cursor(Move::Head);
        assert_eq!(b.pos(), Pos { row: 1, col: 0 }, "Head does not wrap");
        b.move_cursor(Move::Back);
        assert_eq!(b.pos(), Pos { row: 0, col: 3 }, "Back wraps at col 0");
    }

    #[test]
    fn up_down_clamp_and_keep_the_aim() {
        // short lines clamp the LANDING column but never consume the aim
        // (the old contract — "no stickiness" — was vim-divergent: crossing
        // a short line used to collapse the column for good)
        let mut b = buf("hello\nab\nlonger line");
        b.move_cursor(Move::Jump(0, 4));
        b.move_cursor(Move::Down); // "ab" (len 2) clamps to 2
        assert_eq!(b.cursor().1, 2);
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor().1, 4, "the aim (4) survived the short line");
    }

    #[test]
    fn word_motions_vim_semantics() {
        let mut b = buf("aaa bbb ccc");
        b.move_cursor(Move::WordForward);
        assert_eq!(b.cursor().1, 4);
        b.move_cursor(Move::WordForward);
        assert_eq!(b.cursor().1, 8);
        b.move_cursor(Move::End);
        b.move_cursor(Move::WordBack);
        assert_eq!(b.cursor().1, 8);
        b.move_cursor(Move::WordBack);
        assert_eq!(b.cursor().1, 4);

        // symbols separate words; `_` does not (snake_case is one word)
        let mut b = buf("aaa bbb [[[ccc]]]\n\n ddd");
        for expect in [2, 6, 10, 13, 16] {
            b.move_cursor(Move::WordEnd);
            assert_eq!(
                b.pos(),
                Pos {
                    row: 0,
                    col: expect
                }
            );
        }
        b.move_cursor(Move::WordEnd);
        assert_eq!(b.pos(), Pos { row: 2, col: 3 }, "crosses lines");
    }

    #[test]
    fn symbols_separate_but_underscores_do_not() {
        // symbol runs separate words (a run of consecutive symbols is one
        // stop, like vim); user_id stays a single word
        let mut b = buf("user_id,[x]:;y z");
        for expect in [7, 9, 10, 13, 15] {
            b.move_cursor(Move::WordForward);
            assert_eq!(b.cursor().1, expect, "w stop {expect}");
        }
        // e crosses user_id in one press, stopping before the comma
        let mut b = buf("user_id, tail");
        b.move_cursor(Move::WordEnd);
        assert_eq!(b.cursor().1, 6);
        // and objects agree: iw on user_id spans just the identifier
        let mut b2 = buf("a user_id, b");
        b2.move_cursor(Move::WordForward); // onto 'u' of user_id
        let obj = b2.word_object(false).unwrap();
        assert_eq!(
            (obj.start.row, obj.start.col, obj.end.row, obj.end.col),
            (0, 2, 0, 8),
            "user_id without the comma"
        );
    }

    #[test]
    fn underscore_is_a_word_character() {
        // one press of e/w/b spans a whole snake_case identifier
        let mut b = buf("user_id created_at tx");
        b.move_cursor(Move::WordEnd);
        assert_eq!(b.cursor().1, 6, "e lands on the final _ of user_id");
        b.move_cursor(Move::WordEnd);
        assert_eq!(b.cursor().1, 17, "e spans created_at in one hop");
        b.move_cursor(Move::WordForward);
        assert_eq!(b.cursor().1, 19, "w starts tx (space at 18)");
        b.move_cursor(Move::WordEnd);
        assert_eq!(b.cursor().1, 20);
        b.move_cursor(Move::WordBack);
        assert_eq!(b.cursor().1, 19, "cursor was on x: b goes to word start");
        b.move_cursor(Move::WordBack);
        assert_eq!(b.cursor().1, 8, "b spans created_at in one hop");
        b.move_cursor(Move::WordBack);
        assert_eq!(b.cursor().1, 0, "b spans user_id in one hop");
    }

    #[test]
    fn paragraph_motions() {
        let mut b = buf("aaa\n\nbbb\n\nccc\nddd");
        b.move_cursor(Move::ParagraphForward);
        assert_eq!(b.cursor().0, 2);
        b.move_cursor(Move::ParagraphForward);
        assert_eq!(b.cursor().0, 4);
        b.move_cursor(Move::Bottom);
        b.move_cursor(Move::ParagraphBack);
        assert_eq!(b.cursor().0, 4);
        b.move_cursor(Move::ParagraphBack);
        assert_eq!(b.cursor().0, 2);
        b.move_cursor(Move::ParagraphBack);
        assert_eq!(b.cursor().0, 0);
    }

    #[test]
    fn delete_str_counts_newlines() {
        let mut b = buf("🐱🐶🐰🐮");
        b.move_cursor(Move::Forward);
        b.delete_str(2);
        assert_eq!(b.lines(), ["🐱🐮"]);

        let mut b = buf("🐱\n🐶\n🐰\n🐮");
        b.move_cursor(Move::Down);
        b.delete_str(4); // 🐶, \n, 🐰, \n
        assert_eq!(b.lines(), ["🐱", "🐮"]);
    }

    #[test]
    fn delete_line_by_end_truncates() {
        let mut b = buf("hello world");
        b.move_cursor(Move::Jump(0, 5));
        b.delete_line_by_end();
        assert_eq!(b.lines(), ["hello"]);
        assert_eq!(b.cursor().1, 5);
    }

    #[test]
    fn selection_copy_cut_and_replace() {
        let mut b = buf("hello world");
        b.move_cursor(Move::Jump(0, 6));
        b.start_selection();
        b.move_cursor(Move::End);
        assert_eq!(b.selection_text().as_deref(), Some("world"));
        let t = b.cut_selection();
        assert_eq!(t.as_deref(), Some("world"));
        assert_eq!(b.lines(), ["hello "]);
        assert_eq!(b.cursor().1, 6);

        // typing replaces the selection
        let mut b = buf("abc");
        b.move_cursor(Move::Jump(0, 1));
        b.start_selection();
        b.move_cursor(Move::End);
        b.insert_char('X');
        assert_eq!(b.lines(), ["aX"]);
    }

    #[test]
    fn selection_draw_range_stops_at_the_lines_text() {
        // A click past EOL clamps (px_to_pos / Jump) to the insert column,
        // one past the last char: the drawable range must never extend past
        // the last character cell, while selection_range keeps its wrapped
        // text semantics (the newline counts as selected)
        let mut b = buf("hello\nab");
        b.move_cursor(Move::Jump(0, 5));
        b.start_selection();
        assert_eq!(b.pos(), Pos { row: 0, col: 5 });
        assert_eq!(
            b.selection_range(),
            Some((Pos { row: 0, col: 5 }, Pos { row: 1, col: 0 })),
            "text semantics still wrap past EOL"
        );
        assert_eq!(
            b.selection_draw_range(),
            Some((Pos { row: 0, col: 5 }, Pos { row: 0, col: 5 })),
            "press-hold at the insert column: empty span, nothing to band"
        );

        // dragging left: the band covers cols 2..5 (through the last char)
        // on the SAME row — it must not wrap onto the next line
        b.move_cursor(Move::Jump(0, 2));
        assert_eq!(
            b.selection_draw_range(),
            Some((Pos { row: 0, col: 2 }, Pos { row: 0, col: 5 }))
        );

        // multi-line: the end row covers only its own cells; an
        // insert-column end draws through its last char, on its own row
        // (trailing newline so the wrap target row exists)
        let mut b = buf("hello\nmid\nab\n");
        b.move_cursor(Move::Jump(0, 2));
        b.start_selection();
        b.move_cursor(Move::Jump(2, 2));
        assert_eq!(
            b.selection_range(),
            Some((Pos { row: 0, col: 2 }, Pos { row: 3, col: 0 })),
        );
        assert_eq!(
            b.selection_draw_range(),
            Some((Pos { row: 0, col: 2 }, Pos { row: 2, col: 2 })),
            "end row banded through its last char, no wrap to row 3"
        );

        // an empty END line has no last char cell: nothing draws on it
        let mut b = buf("hello\n\nab");
        b.move_cursor(Move::Jump(0, 1));
        b.start_selection();
        b.move_cursor(Move::Jump(1, 0));
        assert_eq!(
            b.selection_draw_range(),
            Some((Pos { row: 0, col: 1 }, Pos { row: 1, col: 0 })),
            "end col clamps to the empty line's zero cells"
        );
    }

    #[test]
    fn drag_past_eol_selects_like_ending_exactly_at_line_end() {
        // however far past EOL the drag ends, the clamp lands on the
        // insert column, so the selection matches a drag ending exactly at
        // the line's last available position
        let at_end = |end_col| {
            let mut b = buf("hello\nworld");
            b.move_cursor(Move::Jump(0, 1));
            b.start_selection();
            b.move_cursor(Move::Jump(0, end_col));
            (b.pos(), b.selection_range(), b.selection_draw_range())
        };
        let (pos, range, draw_range) = at_end(5);
        assert_eq!(pos, Pos { row: 0, col: 5 }, "line end is the insert column");
        for past in [6, 50, 500] {
            assert_eq!(
                at_end(past),
                (pos, range, draw_range),
                "a drag ending {past} cols past EOL selects the same span"
            );
        }

        // anchor on the insert column, dragging left: identical band to a
        // selection whose cursor side ends exactly at line end
        let mut anchored_past = buf("hello\nworld");
        anchored_past.move_cursor(Move::Jump(0, 5));
        anchored_past.start_selection();
        anchored_past.move_cursor(Move::Jump(0, 2));
        let mut anchored_at = buf("hello\nworld");
        anchored_at.move_cursor(Move::Jump(0, 5));
        anchored_at.start_selection();
        anchored_at.move_cursor(Move::Jump(0, 2));
        assert_eq!(
            anchored_past.selection_range(),
            anchored_at.selection_range()
        );
        assert_eq!(
            anchored_past.selection_draw_range(),
            Some((Pos { row: 0, col: 2 }, Pos { row: 0, col: 5 })),
            "the band stops at the last char cell"
        );
    }

    #[test]
    fn vertical_moves_keep_the_desired_column_across_short_lines() {
        // the user's repro: `$` on line 1, then `j` past a BLANK line —
        // the aim must survive the crossing (vim curswant), not collapse
        // to the blank line's column 0
        let mut b = buf("one two three\n\nfour five six\nseven eight");
        b.move_cursor(Move::End);
        assert_eq!(b.cursor(), (0, 13));
        b.move_cursor(Move::Down); // the blank line: landing clamps to 0
        assert_eq!(b.cursor(), (1, 0));
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor(), (2, 13), "the aim survived the blank line");
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor(), (3, 11), "clamped to this line's length");
        b.move_cursor(Move::Up);
        assert_eq!(b.cursor(), (2, 13), "aiming back up re-lands the column");
        // a horizontal motion re-aims
        b.move_cursor(Move::Back);
        assert_eq!(b.cursor(), (2, 12));
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor(), (3, 11), "the new aim (12) clamped");
    }

    #[test]
    fn dollar_aims_at_every_lines_end_until_re_aimed() {
        // vim's $: curswant = MAXCOL — j/k ride each line's end, even
        // through lines that end BEFORE the original column
        let mut b = buf("one two three\nshort\n\nlonger line here");
        b.move_cursor(Move::EndMax);
        assert_eq!(b.cursor(), (0, 13));
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor(), (1, 5), "short line: its end");
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor(), (2, 0), "blank line: its end");
        b.move_cursor(Move::Down);
        assert_eq!(b.cursor(), (3, 16), "long line: still riding the end");
        // typing re-aims via set_col
        b.set_col(10);
        b.move_cursor(Move::Up);
        assert_eq!(b.cursor(), (2, 0), "clamped to the blank line");
        b.move_cursor(Move::Up);
        assert_eq!(b.cursor(), (1, 5), "clamped to the short line");
        b.move_cursor(Move::Up);
        assert_eq!(b.cursor(), (0, 10), "the re-aimed column, not MAXCOL");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut b = buf("abc");
        b.move_cursor(Move::End);
        b.insert_char('d');
        b.insert_char('e');
        assert_eq!(b.lines(), ["abcde"]);
        b.undo();
        assert_eq!(b.lines(), ["abcd"]);
        b.undo();
        assert_eq!(b.lines(), ["abc"]);
        b.redo();
        assert_eq!(b.lines(), ["abcd"]);
        b.redo();
        assert_eq!(b.lines(), ["abcde"]);
    }

    #[test]
    fn search_wraps_and_respects_from_cursor() {
        let mut b = buf("foo bar\nfoo baz");
        assert!(b.set_search_pattern("foo"));
        assert!(b.search_forward(true));
        assert_eq!(b.pos(), Pos { row: 0, col: 0 });
        assert!(b.search_forward(false)); // n
        assert_eq!(b.pos(), Pos { row: 1, col: 0 });
        assert!(b.search_forward(false)); // wraps
        assert_eq!(b.pos(), Pos { row: 0, col: 0 });
        assert!(b.search_back(false)); // N
        assert_eq!(b.pos(), Pos { row: 1, col: 0 });
        assert!(!b.set_search_pattern("("));
        assert!(b.has_search_pattern());
    }

    #[test]
    fn hidden_hl_keeps_the_pattern_and_fresh_search_re_shows() {
        let mut b = buf("foo bar\nfoo baz");
        assert!(b.set_search_pattern("foo"));
        assert_eq!(b.search_hl_spans().len(), 2);
        // :noh — the bands drop but the engine keeps cycling
        b.hide_search_hl();
        assert!(b.search_hl_spans().is_empty(), "hidden");
        assert_eq!(b.search_match_spans().len(), 2, "the pattern lives");
        assert!(b.search_forward(false), "n works while hidden");
        // an invalid pattern keeps the previous one (still hidden)
        assert!(!b.set_search_pattern("("));
        assert!(b.search_hl_spans().is_empty());
        // a valid fresh pattern re-shows the bands
        assert!(b.set_search_pattern("bar"));
        assert_eq!(b.search_hl_spans().len(), 1, "re-shown");
        // clear_search drops pattern and flag together
        b.hide_search_hl();
        b.clear_search();
        assert!(b.search_match_spans().is_empty());
        assert!(b.search_hl_spans().is_empty());
        assert!(b.set_search_pattern("foo"));
        assert_eq!(b.search_hl_spans().len(), 2);
    }

    #[test]
    fn text_roundtrip() {
        let src = "a\nb\nc\n";
        let b = Buffer::from_text(src);
        assert_eq!(b.text(), src);
    }

    #[test]
    fn opened_lines_wear_the_indent_their_position_calls_for() {
        // a body line: `o`/`O` inherit its indent (vim autoindent)
        let mut b = buf("Table users {\n  id integer\n}");
        b.move_cursor(Move::Jump(1, 4));
        b.open_line_below();
        assert_eq!(b.lines(), ["Table users {", "  id integer", "  ", "}"]);
        assert_eq!(b.pos(), Pos { row: 2, col: 2 }, "cursor after the indent");
        b.move_cursor(Move::Jump(1, 4));
        b.open_line_above();
        assert_eq!(b.lines(), ["Table users {", "  ", "  id integer", "  ", "}"]);
        assert_eq!(b.pos(), Pos { row: 1, col: 2 });

        // the opener: the line BELOW sits inside the block, one unit
        // deeper; the line ABOVE is outside it, at the opener's indent
        let mut b = buf("Table users {\n  id integer\n}");
        b.move_cursor(Move::Jump(0, 6));
        b.open_line_below();
        assert_eq!(b.lines(), ["Table users {", "  ", "  id integer", "}"]);
        b.undo();
        assert_eq!(b.lines(), ["Table users {", "  id integer", "}"]);
        b.move_cursor(Move::Jump(0, 6));
        b.open_line_above();
        assert_eq!(b.lines(), ["", "Table users {", "  id integer", "}"]);

        // the closer: `o` continues at the closer's indent (still the
        // block's level for an indented closer), `O` lands inside
        let mut b = buf("Table users {\n  records (id) {\n    1\n  }\n}");
        b.move_cursor(Move::Jump(3, 2));
        b.open_line_below();
        assert_eq!(b.lines()[4], "  ", "below the inner closer: body level");
        b.move_cursor(Move::Jump(3, 2));
        b.open_line_above();
        assert_eq!(b.lines()[3], "  ", "above the inner closer: same level");

        // a comment tail never MAKES a block open — but the `{` before
        // the `//` still does
        let mut b = buf("Table users { // many");
        b.open_line_below();
        assert_eq!(b.lines()[1], "  ", "the {{ before the // still opens");
        let mut b = buf("// TODO {");
        b.open_line_below();
        assert_eq!(b.lines()[1], "", "a comment's {{ opens nothing");
    }
}
