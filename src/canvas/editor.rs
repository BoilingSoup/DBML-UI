//! The editor canvas: monospace code text with a gutter, a mode-styled
//! fake cursor, and visual-mode selection spans. Scrolling is a float row
//! offset kept in `App`; the canvas just draws the visible window.

use crate::app::{App, Focus, Message};
use crate::editor::buffer::Pos;
use crate::editor::Mode;
use crate::theme;
use std::cell::Cell;
use std::time::Instant;
use iced::widget::canvas::{self, Text};
use iced::{Color, Font, Point, Rectangle, Size};

/// iced's default monospace (Fira Mono) advances exactly 0.6 em.
pub const CHAR_RATIO: f32 = 0.6;
pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 21.0;
pub const GUTTER: f32 = 56.0;
/// Right margin kept clear when the view is scrolled fully right: the
/// longest line's last cell never touches the pane's right edge, leaving
/// room for the horizontal scrollbar. Part of `App::max_hscroll`.
pub const H_RIGHT_MARGIN: f32 = 16.0;

pub fn char_width() -> f32 {
    FONT_SIZE * CHAR_RATIO
}

/// Screen-space origin of a buffer position (local to the editor pane).
pub fn pos_to_px(p: Pos, scroll: f32, hscroll: f32) -> Point {
    Point::new(
        GUTTER + p.col as f32 * char_width() - hscroll,
        // pixel-snapped so glyphs, cursor and marks always share one grid
        ((p.row as f32 - scroll) * LINE_HEIGHT).round(),
    )
}

/// Buffer position under a local point (clamped into the text).
pub fn px_to_pos(app: &App, p: Point) -> Pos {
    let lines = app.editor().buf.lines();
    let row = ((p.y / LINE_HEIGHT).floor() as usize)
        .saturating_add(app.cur().editor_scroll as usize)
        .min(lines.len().saturating_sub(1));
    let max_col = lines.get(row).map(|l| l.chars().count()).unwrap_or(0);
    let col = if p.x <= GUTTER {
        0
    } else {
        // floor: the char under the pointer, also used for hit-testing
        // (+ hscroll: the view may be scrolled sideways)
        (((p.x - GUTTER + app.cur().editor_hscroll) / char_width()).floor() as usize).min(max_col)
    };
    Pos { row, col }
}

// ---- block cursor (vim normal/visual) ----
//
// Neovim renders its normal-mode cursor as a solid block that INVERTS the
// character under it, and blinks it (painted ~600ms, hidden ~350ms,
// restarting on every cursor movement). Here that is an opaque cell in the
// caret color with the glyph redrawn on top in the pane background color,
// so the character stays readable instead of washing out behind a
// translucent fill.

/// How long the block cursor stays painted per blink cycle.
pub const BLINK_ON_MS: u64 = 600;
/// How long the block cursor stays hidden per blink cycle.
pub const BLINK_OFF_MS: u64 = 350;

/// Blink state of the normal-mode block cursor, stored as the editor
/// canvas's per-widget `Program::State` (`type State = BlinkState`) so no
/// App state is involved. iced keeps a widget's state across frames; the
/// draw pass advances it through `Cell` (draw only gets `&State`).
///
/// Draw is the blink's chokepoint on purpose: it sees every cursor
/// position the app can reach — motions, edits, clicks, the scroll
/// animation — so a changed position restarts the cycle without hunting
/// down each cursor-move call site.
#[derive(Debug)]
pub struct BlinkState {
    /// What the cursor looked like on the previous draw: buffer position,
    /// vim mode, and whether the blink was live (pane focused and on
    /// screen). Any difference restarts the cycle in the visible phase,
    /// like neovim restarting its blink on cursor movement.
    last: Cell<Option<(Pos, Mode, bool)>>,
    /// When the current blink cycle began.
    cycle_start: Cell<Instant>,
}

impl Default for BlinkState {
    fn default() -> Self {
        Self {
            last: Cell::default(),
            // placeholder: `last` starts unset, so the first block_visible
            // call always resets this before it is ever subtracted from
            cycle_start: Cell::new(Instant::now()),
        }
    }
}

impl BlinkState {
    /// Whether the block cursor paints on this draw: visible for
    /// `BLINK_ON_MS`, hidden for `BLINK_OFF_MS`, repeating. With `active`
    /// false (pane unfocused) it stays solid — redraws stop then, so a
    /// running cycle could otherwise freeze in the hidden phase.
    fn block_visible(&self, pos: Pos, mode: Mode, active: bool, now: Instant) -> bool {
        if self.last.get() != Some((pos, mode, active)) {
            self.last.set(Some((pos, mode, active)));
            self.cycle_start.set(now);
        }
        if !active {
            return true;
        }
        let cycle = (BLINK_ON_MS + BLINK_OFF_MS) as u128;
        now.duration_since(self.cycle_start.get()).as_millis() % cycle < BLINK_ON_MS as u128
    }
}

/// Whether the current mode paints the vim block cursor rather than the
/// thin caret: normal/visual in vim mode (regular mode always gets the
/// caret, insert mode too).
fn is_block_cursor(app: &App) -> bool {
    !app.regular_mode() && matches!(app.editor().mode, Mode::Normal | Mode::Visual)
}

/// Whether the block cursor is blinking right now. The app's tick
/// subscription stays hot while this is true (see `subscription` in
/// app.rs) so successive redraws can toggle the block. Only while the
/// editor pane is actually on screen: collapsing the pane removes the
/// canvas from the widget tree, and the start screen has no editor pane.
pub fn block_cursor_blinking(app: &App) -> bool {
    is_block_cursor(app)
        && app.focus == Focus::Editor
        && !app.editor_collapsed
        && app.doc_open()
}

/// The glyph to repaint inverted under the block cursor: the character at
/// the cursor cell when it actually draws ink. Whitespace (the text pass
/// never paints it), control and zero-width characters get the bare
/// block, and past end-of-line (an empty row included) there is no glyph
/// at all — neovim shows a plain cell there as well. Wide characters keep
/// their inverted glyph; the block itself stays one cell.
fn inverted_glyph(app: &App, pos: Pos) -> Option<char> {
    let ch = app.editor().buf.line(pos.row).chars().nth(pos.col)?;
    if !invertible_char(ch) {
        return None;
    }
    Some(ch)
}

/// Whether a character paints ink of its own in a cell: whitespace is
/// never painted by the text pass, and control and zero-width characters
/// (combining marks) have no cell of their own. Wide characters do, so
/// they keep their inverted glyph.
fn invertible_char(ch: char) -> bool {
    use unicode_width::UnicodeWidthChar;
    !(ch.is_control() || ch.is_whitespace() || ch.width() == Some(0))
}

/// One gutter line number at its row's vertical center. Shared by the main
/// row loop and the gutter repaint that runs when the view is scrolled
/// sideways (keeping the two verbatim in sync).
fn gutter_number(frame: &mut canvas::Frame, row: usize, text_y: f32) {
    frame.fill_text(Text {
        content: format!("{:>4}", row + 1),
        position: Point::new(GUTTER - 8.0, text_y),
        color: theme::DIM,
        size: (FONT_SIZE - 2.0).into(),
        font: Font::MONOSPACE,
        horizontal_alignment: iced::alignment::Horizontal::Right,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Default::default()
    });
}

pub struct EditorView<'a> {
    pub app: &'a App,
}

impl canvas::Program<Message> for EditorView<'_> {
    type State = BlinkState;

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use iced::mouse;
        // The canvas widget forwards every window event with window-level
        // coordinates; hover tracks enter/leave, the rest only fire inside.
        if matches!(
            event,
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. })
        ) {
            let over = cursor
                .position_over(bounds)
                .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y));
            return (
                canvas::event::Status::Ignored,
                Some(Message::EditorHover(over)),
            );
        }
        let Some(p) = cursor.position_over(bounds) else {
            return (canvas::event::Status::Ignored, None);
        };
        let local = Point::new(p.x - bounds.x, p.y - bounds.y);
        match event {
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                // winit's y is positive when the content moves down (wheel
                // up); EditorScrolled is app-space — positive v scrolls the
                // view down, positive h the view right — so flip both
                // device signs here (two-finger right pans the view right)
                let (v, h) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (-y, -x),
                    mouse::ScrollDelta::Pixels { x, y } => (-y / 40.0, -x / 40.0),
                };
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorScrolled { v, h }),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // a press on the popup picks that row; anywhere else places
                // the caret (and dismisses the popup)
                if let Some((rect, first_row, _)) = popup_layout(self.app, bounds) {
                    if rect.contains(local) {
                        let row = ((local.y - rect.y) / ROW_H) as usize;
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::CompletionPicked(first_row + row)),
                        );
                    }
                }
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorPressed(local)),
                )
            }
            // right-press opens the editor context menu (Copy / Cut /
            // Paste / Format); the app anchors it via the window cursor
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => (
                canvas::event::Status::Captured,
                Some(Message::EditorMenu(local)),
            ),
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::PANEL);

        let app = self.app;
        let lines = app.editor().buf.lines();
        let scroll = app.cur().editor_scroll;
        let hs = app.cur().editor_hscroll;
        let visible = (bounds.height / LINE_HEIGHT).ceil() as usize + 1;
        let first = scroll.floor() as usize;
        let sub = scroll - first as f32;

        // selection spans (visual mode); V paints whole rows regardless
        // of column
        if app.editor().visual_linewise && app.editor().mode == crate::editor::Mode::Visual {
            if let Some(anchor) = app.editor().buf.selection_anchor() {
                let cursor = app.editor().buf.pos();
                let from = anchor.row.min(cursor.row);
                let to = anchor.row.max(cursor.row);
                for r in from..=to {
                    let y = (r as f32 - scroll) * LINE_HEIGHT;
                    if y > -LINE_HEIGHT && y < bounds.height {
                        frame.fill_rectangle(
                            Point::new(GUTTER, y),
                            Size::new(bounds.width - GUTTER, LINE_HEIGHT * 0.9),
                            Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                        );
                    }
                }
            }
        } else if let Some((start, end)) = app.editor().buf.selection_range() {
            let top = pos_to_px(start, scroll, hs);
            let bottom = pos_to_px(end, scroll, hs);
            let full_rows = end.row.saturating_sub(start.row + 1);
            let w = char_width();
            if top.y == bottom.y {
                frame.fill_rectangle(
                    top,
                    Size::new((bottom.x - top.x).max(w), LINE_HEIGHT * 0.9),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
            } else {
                frame.fill_rectangle(
                    top,
                    Size::new(bounds.width - top.x, LINE_HEIGHT * 0.9),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
                if full_rows > 0 {
                    frame.fill_rectangle(
                        Point::new(GUTTER, top.y + LINE_HEIGHT),
                        Size::new(bounds.width - GUTTER, full_rows as f32 * LINE_HEIGHT),
                        Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                    );
                }
                frame.fill_rectangle(
                    Point::new(GUTTER, bottom.y),
                    Size::new((bottom.x - GUTTER).max(w), LINE_HEIGHT * 0.9),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
            }
        }

        // error marks (squiggles + gutter dots + hover tooltip)
        let marks = error_marks(app);

        // gutter + text, token-colored
        let src = app.source();
        // byte offset of each line start in `source` (highlight spans index it)
        let mut line_start = 0usize;
        let mut line_starts = Vec::with_capacity(lines.len());
        for l in lines {
            line_starts.push(line_start);
            line_start += l.len() + 1;
        }
        let mut y = -(sub * LINE_HEIGHT).round();
        for (i, line) in lines.iter().enumerate().skip(first).take(visible.max(1)) {
            let text_y = y + LINE_HEIGHT * 0.5;
            if marks.iter().any(|m| m.row == i) {
                frame.fill(
                    &canvas::Path::circle(Point::new(10.0, text_y), 2.5),
                    theme::ERR,
                );
            }
            gutter_number(&mut frame, i, text_y);
            if !line.is_empty() {
                let ls = line_starts[i];
                let le = ls + line.len();
                // token chunks intersecting this line, drawn at their char
                // column on the same monospace grid the cursor math uses
                let first_tok = app.cur().highlights.partition_point(|t| t.end <= ls);
                for t in &app.cur().highlights[first_tok..] {
                    if t.start >= le {
                        break;
                    }
                    let s = t.start.max(ls);
                    let e = t.end.min(le);
                    let content = &src[s..e];
                    if content.chars().any(|c| !c.is_whitespace()) {
                        let col = src[ls..s].chars().count();
                        // hscroll clips at the gutter's right edge: cells
                        // left of it are dropped here, because a fill drawn
                        // later cannot cover text (iced paints a widget's
                        // whole mesh pass before its text pass)
                        let col_min = (hs / char_width()).ceil() as usize;
                        let skip = col_min.saturating_sub(col);
                        let content: String = content.chars().skip(skip).collect();
                        if !content.is_empty() {
                            frame.fill_text(Text {
                                content,
                                position: Point::new(
                                    GUTTER + (col + skip) as f32 * char_width() - hs,
                                    text_y,
                                ),
                                color: token_color(t.kind),
                                size: FONT_SIZE.into(),
                                font: Font::MONOSPACE,
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
            y += LINE_HEIGHT;
        }

        // fake cursor, styled by mode. Deliberately NOT hidden while a
        // scroll animation runs: neoscroll hides only the terminal's
        // hardware caret (the text still visibly scrolls), but here the
        // gliding block IS the animation — hiding it made fitting-document
        // scrolls look like an instant jump. (The blink restarts on every
        // cursor move anyway, so a gliding block stays solid for free.)
        let cursor = app.editor().buf.pos();
        let p = pos_to_px(cursor, scroll, hs);
        if p.y > -LINE_HEIGHT && p.y < bounds.height + LINE_HEIGHT {
            if is_block_cursor(app) {
                // Neovim-style block: an OPAQUE cell in the caret color
                // with the glyph under it redrawn on top in the pane
                // background, so the character reads as inverted instead
                // of showing through a translucent fill. Text renders
                // after meshes within one geometry, so the inverted glyph
                // lands over the block. In the blink's hidden phase
                // nothing is drawn at all, like neovim.
                if state.block_visible(
                    cursor,
                    app.editor().mode,
                    block_cursor_blinking(app),
                    Instant::now(),
                ) {
                    frame.fill_rectangle(
                        p,
                        Size::new(char_width(), LINE_HEIGHT * 0.9),
                        theme::TEXT,
                    );
                    if let Some(ch) = inverted_glyph(app, cursor) {
                        if p.x >= GUTTER {
                            // exactly the text pass's cell geometry, so the
                            // inverted glyph sits pixel-perfect on the
                            // original; skipped when hscroll pushed the cell
                            // under the gutter (the repaint below hides the
                            // block — the glyph must not float there alone)
                            frame.fill_text(Text {
                                content: ch.to_string(),
                                position: Point::new(p.x, p.y + LINE_HEIGHT * 0.5),
                                color: theme::PANEL,
                                size: FONT_SIZE.into(),
                                font: Font::MONOSPACE,
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                        }
                    }
                }
            } else {
                // insert mode and regular mode: the thin caret
                frame.fill_rectangle(p, Size::new(2.0, LINE_HEIGHT * 0.9), theme::TEXT);
            }
        }

        // squiggly underlines under error tokens
        for m in &marks {
            let top_y = (m.row as f32 - scroll) * LINE_HEIGHT;
            if top_y < -LINE_HEIGHT || top_y > bounds.height {
                continue;
            }
            let y = top_y + LINE_HEIGHT * 0.85;
            let x0 = GUTTER + m.start as f32 * char_width() - hs;
            let x1 = (GUTTER + m.end as f32 * char_width() - hs).max(x0 + 3.0);
            let path = canvas::Path::new(|b| {
                b.move_to(Point::new(x0, y + 1.5));
                let mut x = x0 + 2.5;
                let mut down = false;
                while x < x1 - 2.5 {
                    b.line_to(Point::new(x, y + if down { 1.5 } else { -1.5 }));
                    down = !down;
                    x += 2.5;
                }
                b.line_to(Point::new(x1, y + 1.5));
            });
            frame.stroke(
                &path,
                canvas::Stroke {
                    style: theme::ERR.into(),
                    width: 1.2,
                    ..Default::default()
                },
            );
        }

        // Sideways scroll bleeds the mesh layer (selection spans, the
        // cursor block, squiggles) into the gutter: repaint the gutter band
        // over it and re-draw its numbers and error dots on top. Token
        // GLYPHS are text — a later fill cannot cover them — so the row
        // loop already drops the cells left of the gutter edge.
        if hs > 0.0 {
            frame.fill_rectangle(Point::ORIGIN, Size::new(GUTTER, bounds.height), theme::PANEL);
            let mut gy = -(sub * LINE_HEIGHT).round();
            for (i, _) in lines.iter().enumerate().skip(first).take(visible.max(1)) {
                let text_y = gy + LINE_HEIGHT * 0.5;
                if marks.iter().any(|m| m.row == i) {
                    frame.fill(
                        &canvas::Path::circle(Point::new(10.0, text_y), 2.5),
                        theme::ERR,
                    );
                }
                gutter_number(&mut frame, i, text_y);
                gy += LINE_HEIGHT;
            }
        }

        // scrollbars: draw-only indicators, on top of everything. Both
        // compare the buffer against what one page can show.
        let total_rows = lines.len() as f32;
        let page_rows = (bounds.height / LINE_HEIGHT).max(1.0);
        let v_max = (total_rows - page_rows).max(0.0);
        let max_cols = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0) as f32;
        let h_max = (max_cols * char_width() - (bounds.width - GUTTER - H_RIGHT_MARGIN)).max(0.0);
        if v_max > 0.0 || h_max > 0.0 {
            let track = Color {
                a: 0.3,
                ..theme::BORDER
            };
            let thumb = theme::DIM;
            // vertical: 4px track on the right edge, thumb sized by the
            // visible fraction and offset by the scroll fraction (24px
            // floor so huge documents keep a grabbable-looking thumb)
            if v_max > 0.0 {
                let track_h = bounds.height - if h_max > 0.0 { 4.0 } else { 0.0 };
                // max-then-min (not clamp): never panics on a tiny canvas
                let thumb_h = (track_h * page_rows / total_rows).max(24.0).min(track_h);
                let frac = (scroll / v_max).clamp(0.0, 1.0);
                frame.fill_rectangle(
                    Point::new(bounds.width - 4.0, 0.0),
                    Size::new(4.0, track_h),
                    track,
                );
                frame.fill_rectangle(
                    Point::new(bounds.width - 4.0, (track_h - thumb_h) * frac),
                    Size::new(4.0, thumb_h),
                    thumb,
                );
            }
            // horizontal: mirrored, along the bottom edge
            if h_max > 0.0 {
                let track_w = bounds.width - if v_max > 0.0 { 4.0 } else { 0.0 };
                let thumb_w = (track_w * (bounds.width - GUTTER) / (max_cols * char_width()))
                    .max(24.0)
                    .min(track_w);
                let frac = (hs / h_max).clamp(0.0, 1.0);
                frame.fill_rectangle(
                    Point::new(0.0, bounds.height - 4.0),
                    Size::new(track_w, 4.0),
                    track,
                );
                frame.fill_rectangle(
                    Point::new((track_w - thumb_w) * frac, bounds.height - 4.0),
                    Size::new(thumb_w, 4.0),
                    thumb,
                );
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        // canvas mouse_interaction is not bounds-gated by iced, so check
        // hover here: I-beam while typing or in Regular mode (where the
        // mouse selects text), plain arrow otherwise (normal/visual
        // motions are keyboard-driven)
        if cursor.position_over(bounds).is_some()
            && (self.app.editor().mode == Mode::Insert || self.app.regular_mode())
        {
            return iced::mouse::Interaction::Text;
        }
        iced::mouse::Interaction::default()
    }
}

/// Popup rows per page and row height.
pub const COMPLETION_ROWS: usize = 8;
pub const ROW_H: f32 = 20.0;

/// A squiggle underline: buffer row plus char-column range (end exclusive),
/// with the parser's message. Feeds both drawing and hover hit-testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub message: String,
}

/// A definition tooltip: title line plus (marker, text) rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocHover {
    pub title: String,
    pub rows: Vec<(String, String)>,
}

/// Parse errors as per-line underline ranges: the token at the error
/// offset, one char minimum (point errors still get a visible mark).
pub fn error_marks(app: &App) -> Vec<Mark> {
    let src = app.source();
    let mut marks = Vec::new();
    for e in &app.cur().parse_errors {
        let (row, byte_col) = dbml_ui::dbml::parser::line_col_at(src, e.offset);
        let Some(line) = src.split('\n').nth(row) else {
            continue;
        };
        let at = byte_col.min(line.len());
        let start = line[..at].chars().count();
        let end_byte = line[at..]
            .char_indices()
            .take_while(|(_, c)| c.is_alphanumeric() || *c == '_')
            .last()
            .map(|(i, c)| at + i + c.len_utf8())
            .unwrap_or(at);
        let mut end = line[..end_byte.min(line.len())].chars().count();
        if end <= start {
            end = start + 1;
        }
        marks.push(Mark {
            row,
            start,
            end,
            message: e.message.clone(),
        });
    }
    marks
}

/// The word under a hover point, if the pointer is on a word character.
fn word_at(app: &App, p: Point) -> Option<String> {
    let pos = px_to_pos(app, p);
    let chars: Vec<char> = app.editor().buf.line(pos.row).chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    if !chars.get(pos.col).copied().is_some_and(is_word) {
        return None;
    }
    let mut s = pos.col;
    while s > 0 && is_word(chars[s - 1]) {
        s -= 1;
    }
    let mut e = pos.col + 1;
    while e < chars.len() && is_word(chars[e]) {
        e += 1;
    }
    Some(chars[s..e].iter().collect())
}

/// Definition doc for the hovered word: a table/enum by name or alias.
pub fn doc_hover_at(app: &App, p: Point) -> Option<DocHover> {
    use dbml_ui::model::NodeKind;
    let word = word_at(app, p)?;
    let b = app
        .diagram()
        .boxes
        .iter()
        .find(|b| b.title == word || b.alias.as_deref() == Some(word.as_str()))?;
    let title = format!(
        "{} {}",
        match b.kind {
            NodeKind::Enum => "Enum",
            _ => "Table",
        },
        b.title
    );
    let rows = b
        .rows
        .iter()
        .map(|r| {
            let marker = if r.pk {
                "\u{25c6}"
            } else if r.fk {
                "\u{25c7}"
            } else {
                " "
            };
            let text = if r.ty.is_empty() {
                r.name.clone()
            } else {
                format!("{}  {}", r.name, r.ty)
            };
            (marker.to_string(), text)
        })
        .collect();
    Some(DocHover { title, rows })
}

/// Doc tooltip geometry: same placement rules as the error tooltip.
fn doc_rect(hover: Point, doc: &DocHover, bounds: Rectangle) -> Rectangle {
    let text_w = (FONT_SIZE - 1.0) * CHAR_RATIO;
    let widest = doc
        .rows
        .iter()
        .map(|(m, t)| m.chars().count() + 2 + t.chars().count())
        .chain(std::iter::once(doc.title.chars().count()))
        .max()
        .unwrap_or(10) as f32
        * text_w
        + 24.0;
    let h = (doc.rows.len() + 1) as f32 * 16.0 + 12.0;
    let mut x = hover.x + 14.0;
    if x + widest > bounds.x + bounds.width - 4.0 {
        x = bounds.x + bounds.width - 4.0 - widest;
    }
    x = x.max(bounds.x + 4.0);
    let mut y = hover.y + LINE_HEIGHT + 4.0;
    if y + h > bounds.y + bounds.height - 4.0 {
        y = hover.y - h - 6.0;
    }
    y = y.max(bounds.y + 4.0);
    Rectangle::new(Point::new(x, y), Size::new(widest, h))
}

/// Error tooltip: geometry plus the message wrapped to fit the pane,
/// clamped inside and flipped above when it would overflow the bottom.
fn tooltip_layout(hover: Point, message: &str, bounds: Rectangle) -> (Rectangle, Vec<String>) {
    let text_w = (FONT_SIZE - 1.0) * CHAR_RATIO;
    let max_chars = (((bounds.width - 32.0) / text_w) as usize).max(10);
    let mut lines = Vec::new();
    let mut current = String::new();
    // hard-break words longer than a line so a single token can't escape
    let mut words = Vec::new();
    for word in message.split_whitespace() {
        if word.chars().count() > max_chars {
            let mut chunk = String::new();
            for c in word.chars() {
                chunk.push(c);
                if chunk.chars().count() == max_chars {
                    words.push(std::mem::take(&mut chunk));
                }
            }
            if !chunk.is_empty() {
                words.push(chunk);
            }
        } else {
            words.push(word.to_string());
        }
    }
    for word in words {
        let candidate_len = current.chars().count() + word.chars().count() + 1;
        if !current.is_empty() && candidate_len > max_chars {
            lines.push(std::mem::take(&mut current));
        }
        if current.is_empty() {
            current.push_str(&word);
        } else {
            current.push(' ');
            current.push_str(&word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > 8 {
        lines.truncate(8);
    }
    let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(10) as f32 * text_w + 24.0;
    let h = lines.len() as f32 * 18.0 + 12.0;
    let mut x = hover.x + 14.0;
    if x + w > bounds.x + bounds.width - 4.0 {
        x = bounds.x + bounds.width - 4.0 - w;
    }
    x = x.max(bounds.x + 4.0);
    let mut y = hover.y + LINE_HEIGHT + 4.0;
    if y + h > bounds.y + bounds.height - 4.0 {
        y = hover.y - h - 6.0;
    }
    y = y.max(bounds.y + 4.0);
    (Rectangle::new(Point::new(x, y), Size::new(w, h)), lines)
}

/// Geometry of the open completion popup: rect, first visible row, visible
/// count. Positioned under the caret, clamped/flipped to stay inside the
/// pane. None when the popup is closed.
fn popup_layout(app: &App, bounds: Rectangle) -> Option<(Rectangle, usize, usize)> {
    let c = app.completion.as_ref()?;
    let visible = c.items.len().min(COMPLETION_ROWS);
    let width = c
        .items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(8) as f32
        * char_width()
        + 20.0;
    let height = visible as f32 * ROW_H + 6.0;
    let caret = pos_to_px(app.editor().buf.pos(), app.cur().editor_scroll, app.cur().editor_hscroll);
    let mut x = caret.x + char_width() + 2.0;
    let mut y = caret.y + LINE_HEIGHT + 2.0;
    if y + height > bounds.y + bounds.height - 2.0 {
        y = caret.y - height - 2.0;
    }
    if x + width > bounds.x + bounds.width - 2.0 {
        x = bounds.x + bounds.width - 2.0 - width;
    }
    x = x.max(bounds.x + 2.0);
    y = y.max(bounds.y + 2.0);
    // keep the selection visible when scrolled
    let first_row = if c.selected + 1 > visible {
        c.selected + 1 - visible
    } else {
        0
    };
    Some((
        Rectangle::new(Point::new(x, y), Size::new(width, height)),
        first_row,
        visible,
    ))
}

#[cfg(test)]
mod doc_tests {
    use super::*;
    use crate::app::{update, App};
    use iced::Vector;

    fn app() -> App {
        let mut app = App::default();
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Char('i'),
                crate::editor::KeyModifiers::NONE,
            ),
        );
        app
    }

    #[test]
    fn hovering_a_table_name_shows_its_columns() {
        let app = app();
        // last line: "Ref: comments.author_id > users.id" — hover "users"
        let row = app.editor().buf.line_count() - 1;
        let line = app.editor().buf.line(row);
        let col = line.find("users").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("doc");
        assert_eq!(doc.title, "Table users");
        assert!(doc.rows.iter().any(|(_, t)| t.contains("username")));
        assert!(doc.rows.iter().any(|(m, _)| m == "\u{25c6}"), "pk marker");
    }

    #[test]
    fn hovering_an_alias_shows_the_same_doc() {
        let app = app();
        // 0-based line 19: "user_id integer [not null, ref: > U.id]"
        let line = app.editor().buf.line(19);
        let col = line.find("U.").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 19, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("alias doc");
        assert_eq!(doc.title, "Table users");
    }

    #[test]
    fn hovering_an_enum_name_shows_values() {
        let app = app();
        // line 34: "Enum post_status {"
        let line = app.editor().buf.line(33);
        let col = line.find("post_status").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 33, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("enum doc");
        assert_eq!(doc.title, "Enum post_status");
        assert_eq!(doc.rows.len(), 3);
        assert_eq!(doc.rows[0].1, "draft");
    }

    #[test]
    fn non_name_words_get_no_doc() {
        let app = app();
        // hover "integer" on line 7 — not a definition name
        let line = app.editor().buf.line(6);
        let col = line.find("integer").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 6, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        assert!(doc_hover_at(&app, p + Vector::new(3.0, 10.0)).is_none());
        // hovering whitespace: no word at all
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 6, col: 0 },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        assert!(word_at(&app, p + Vector::new(1.0, 10.0)).is_none());
    }

    #[test]
    fn doc_rect_stays_inside_pane() {
        let doc = DocHover {
            title: "Table users".into(),
            rows: (0..30)
                .map(|i| (" ".into(), format!("col_{i} varchar(255)")))
                .collect(),
        };
        let bounds = Rectangle::new(Point::new(0.0, 0.0), Size::new(400.0, 300.0));
        let r = doc_rect(Point::new(390.0, 285.0), &doc, bounds);
        assert!(r.x + r.width <= bounds.width + 0.5, "{r:?}");
        assert!(r.y >= bounds.y, "{r:?}");
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use crate::app::{update, App};

    fn broken_app() -> App {
        // 'Table' with no name: error points at the '{'
        let mut app = App::with_source("Table {\n  id integer [pk]\n}\n".into());
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Char('i'),
                crate::editor::KeyModifiers::NONE,
            ),
        );
        app
    }

    #[test]
    fn parse_errors_become_line_marks() {
        let app = broken_app();
        assert!(!app.cur().parse_errors.is_empty(), "fixture is broken");
        let marks = error_marks(&app);
        assert!(!marks.is_empty());
        for m in &marks {
            assert!(m.end > m.start, "non-empty: {m:?}");
            let line_len = app.editor().buf.line(m.row).chars().count();
            assert!(m.start <= line_len && m.end <= line_len + 1, "{m:?}");
        }
        // the sample's error sits on the first line
        assert_eq!(marks[0].row, 0);
    }

    #[test]
    fn hovering_a_mark_finds_its_message() {
        let app = broken_app();
        let marks = error_marks(&app);
        let m = &marks[0];
        // hover the squiggled char (left edge of its column)
        let p = pos_to_px(
            crate::editor::buffer::Pos {
                row: m.row,
                col: m.start,
            },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let pos = px_to_pos(&app, p);
        let hit = marks
            .iter()
            .find(|m| m.row == pos.row && pos.col >= m.start && pos.col <= m.end);
        assert!(
            hit.is_some(),
            "hover at {p:?} -> {pos:?} hits none of {marks:?}"
        );
        assert!(!hit.unwrap().message.is_empty());
    }

    #[test]
    fn hovering_elsewhere_misses() {
        let app = broken_app();
        let marks = error_marks(&app);
        // hover a different line than any mark
        let other = (0..app.editor().buf.line_count())
            .find(|r| !marks.iter().any(|m| m.row == *r))
            .expect("fixture has non-error lines");
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: other, col: 0 },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let pos = px_to_pos(&app, p);
        assert!(!marks
            .iter()
            .any(|m| m.row == pos.row && pos.col >= m.start && pos.col <= m.end));
    }

    #[test]
    fn tooltip_wraps_and_clamps_into_pane() {
        let bounds = Rectangle::new(Point::new(0.0, 0.0), Size::new(400.0, 300.0));
        let long = "x".repeat(120);
        let (r, lines) = tooltip_layout(Point::new(390.0, 280.0), &long, bounds);
        assert!(lines.len() > 1, "long message wraps: {lines:?}");
        assert!(
            r.x >= bounds.x && r.x + r.width <= bounds.x + bounds.width + 0.5,
            "{r:?}"
        );
        assert!(
            r.y >= bounds.y && r.y + r.height <= bounds.y + bounds.height + 0.5,
            "{r:?}"
        );
        // short message stays a single line
        let (r, lines) = tooltip_layout(Point::new(20.0, 20.0), "expected name", bounds);
        assert_eq!(lines, vec!["expected name".to_string()]);
        assert!(r.y > 20.0, "below the cursor: {r:?}");
    }
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use crate::app::{update, App};

    const PANE: Rectangle = Rectangle::new(Point::new(0.0, 0.0), Size::new(400.0, 300.0));

    fn app_with_caret(row: usize, col: usize) -> App {
        let mut app = App::default();
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Char('i'),
                crate::editor::KeyModifiers::NONE,
            ),
        );
        app.editor_mut().buf.move_cursor(crate::editor::buffer::Move::Jump(row, col));
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Space,
                crate::editor::KeyModifiers::CONTROL,
            ),
        );
        app
    }

    #[test]
    fn popup_flips_above_when_near_bottom() {
        // caret on the last visible line of a tall buffer
        let mut app = App::default();
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Char('i'),
                crate::editor::KeyModifiers::NONE,
            ),
        );
        app.editor_mut().buf.move_cursor(crate::editor::buffer::Move::Bottom);
        app.cur_mut().editor_scroll = app.editor().buf.line_count() as f32 - 20.0;
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Space,
                crate::editor::KeyModifiers::CONTROL,
            ),
        );
        assert!(app.completion.is_some(), "popup open");
        let (rect, _, _) = popup_layout(&app, PANE).unwrap();
        let caret = pos_to_px(app.editor().buf.pos(), app.cur().editor_scroll, app.cur().editor_hscroll);
        assert!(
            rect.y + rect.height <= caret.y,
            "popup should sit above the caret: rect={rect:?} caret_y={}",
            caret.y
        );
    }

    #[test]
    fn popup_stays_inside_right_edge() {
        // caret near the right edge: popup shifts left, stays inside
        let mut app = app_with_caret(0, 0);
        // long labels to force a wide popup
        if let Some(c) = app.completion.as_mut() {
            c.items.insert(
                0,
                crate::editor::completion::Item {
                    label: "x".repeat(80),
                    kind: crate::editor::completion::ItemKind::Table,
                },
            );
        }
        app.cur_mut().editor_scroll = 0.0;
        // a pane the popup can fit in but not at the caret's default spot
        let narrow = Rectangle::new(Point::new(0.0, 0.0), Size::new(720.0, 300.0));
        let (rect, _, _) = popup_layout(&app, narrow).unwrap();
        assert!(rect.width < narrow.width, "precondition: {rect:?}");
        assert!(
            rect.x + rect.width <= narrow.x + narrow.width + 0.5,
            "{rect:?}"
        );
    }

    #[test]
    fn no_popup_geometry_when_closed() {
        let app = App::default();
        assert!(popup_layout(&app, PANE).is_none());
    }
}

fn item_kind_color(kind: crate::editor::completion::ItemKind) -> Color {
    use crate::editor::completion::ItemKind;
    match kind {
        ItemKind::Keyword => theme::SYN_KEYWORD,
        ItemKind::Table | ItemKind::Enum => theme::SYN_NAME,
        ItemKind::Column | ItemKind::EnumValue => theme::SYN_NUMBER,
    }
}

fn token_color(kind: dbml_ui::dbml::highlight::Kind) -> Color {
    use dbml_ui::dbml::highlight::Kind;
    match kind {
        Kind::Keyword => theme::SYN_KEYWORD,
        Kind::Name => theme::SYN_NAME,
        Kind::Type => theme::SYN_TYPE,
        Kind::String => theme::SYN_STRING,
        Kind::Number => theme::SYN_NUMBER,
        Kind::Comment => theme::DIM,
        // punctuation (brackets, braces, parens, commas...) reads as body
        // text, not as a muted comment
        Kind::Punct => theme::TEXT,
        Kind::Ident | Kind::Plain => theme::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use iced::mouse::{self, Cursor};
    use iced::widget::canvas::Program as _;

    const BOUNDS: Rectangle = Rectangle::new(Point::new(100.0, 50.0), Size::new(400.0, 300.0));

    #[test]
    fn press_outside_editor_does_not_move_cursor() {
        // regression: presses in the diagram pane used to leak into the
        // editor canvas and jump the caret
        let app = App::default();
        let view = EditorView { app: &app };
        let outside = Point::new(900.0, 100.0); // diagram-pane territory
        let (status, message) = view.update(
            &mut BlinkState::default(),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            BOUNDS,
            Cursor::Available(outside),
        );
        assert!(message.is_none(), "{message:?}");
        assert_eq!(status, canvas::event::Status::Ignored);
    }

    #[test]
    fn press_inside_places_cursor_at_local_position() {
        let app = App::default();
        let view = EditorView { app: &app };
        let (_, message) = view.update(
            &mut BlinkState::default(),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            BOUNDS,
            Cursor::Available(Point::new(100.0 + GUTTER + 30.0, 50.0 + 40.0)),
        );
        match message {
            Some(Message::EditorPressed(p)) => {
                assert!((p.x - (GUTTER + 30.0)).abs() < 0.01);
                assert!((p.y - 40.0).abs() < 0.01);
            }
            other => panic!("expected EditorPressed, got {other:?}"),
        }
    }

    #[test]
    fn right_press_inside_opens_the_context_menu() {
        let app = App::default();
        let view = EditorView { app: &app };
        let (status, message) = view.update(
            &mut BlinkState::default(),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            BOUNDS,
            Cursor::Available(Point::new(100.0 + GUTTER + 30.0, 50.0 + 40.0)),
        );
        assert_eq!(status, canvas::event::Status::Captured);
        match message {
            Some(Message::EditorMenu(p)) => {
                // canvas-LOCAL coords; the app anchors the overlay
                assert!((p.x - (GUTTER + 30.0)).abs() < 0.01);
                assert!((p.y - 40.0).abs() < 0.01);
            }
            other => panic!("expected EditorMenu, got {other:?}"),
        }
    }

    #[test]
    fn right_press_outside_the_pane_is_ignored() {
        let app = App::default();
        let view = EditorView { app: &app };
        let (status, message) = view.update(
            &mut BlinkState::default(),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
            BOUNDS,
            Cursor::Available(Point::new(900.0, 100.0)),
        );
        assert_eq!(status, canvas::event::Status::Ignored);
        assert!(message.is_none());
    }

    #[test]
    fn wheel_outside_editor_does_not_scroll() {
        // regression: zooming the diagram scrolled the editor pane
        let app = App::default();
        let view = EditorView { app: &app };
        let (_, message) = view.update(
            &mut BlinkState::default(),
            canvas::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
            }),
            BOUNDS,
            Cursor::Available(Point::new(900.0, 100.0)),
        );
        assert!(message.is_none(), "{message:?}");
    }

    #[test]
    fn wheel_sign_scrolls_the_view_the_right_way() {
        // regression: the wheel sign was flipped, so plain wheel-down
        // scrolled the view up. winit's y is positive when the content
        // moves down (wheel up); the handler adds the payload to
        // editor_scroll, and larger editor_scroll shows lines further down
        let app = App::default();
        let view = EditorView { app: &app };
        let inside = Cursor::Available(Point::new(150.0, 100.0));
        for (y, expected_down) in [(1.0, false), (-1.0, true)] {
            let (status, message) = view.update(
                &mut BlinkState::default(),
                canvas::Event::Mouse(mouse::Event::WheelScrolled {
                    delta: mouse::ScrollDelta::Lines { x: 0.0, y },
                }),
                BOUNDS,
                inside,
            );
            assert_eq!(status, canvas::event::Status::Captured);
            match message {
                Some(Message::EditorScrolled { v, h }) => {
                    assert_eq!(h, 0.0, "y-only wheel carries no x payload");
                    assert_eq!(
                        v > 0.0,
                        expected_down,
                        "y={y}: view should move {}",
                        if expected_down { "down" } else { "up" }
                    );
                }
                other => panic!("y={y}: expected EditorScrolled, got {other:?}"),
            }
        }
    }

    #[test]
    fn horizontal_wheel_flips_the_sign_like_the_vertical() {
        // two-finger right (winit x > 0) must move the view right, i.e. a
        // NEGATIVE app-space payload — the same flip the vertical axis uses
        let app = App::default();
        let view = EditorView { app: &app };
        let inside = Cursor::Available(Point::new(150.0, 100.0));
        for (delta, expected_h) in [
            (mouse::ScrollDelta::Lines { x: 2.0, y: 0.0 }, -2.0),
            (mouse::ScrollDelta::Pixels { x: 80.0, y: 0.0 }, -2.0),
        ] {
            let (_, message) = view.update(
                &mut BlinkState::default(),
                canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }),
                BOUNDS,
                inside,
            );
            match message {
                Some(Message::EditorScrolled { v, h }) => {
                    assert_eq!(v, 0.0, "x-only wheel carries no y payload");
                    assert!(
                        (h - expected_h).abs() < 1e-4,
                        "expected h={expected_h}, got {h}"
                    );
                }
                other => panic!("expected EditorScrolled, got {other:?}"),
            }
        }
    }

    #[test]
    fn cursor_shape_follows_mode() {
        use iced::mouse::Interaction;
        let inside = Cursor::Available(Point::new(150.0, 100.0));
        let mut app = App::default();
        // insert mode: the I-beam; normal/visual: plain arrow
        for (mode, expected) in [
            (Mode::Insert, Interaction::Text),
            (Mode::Normal, Interaction::default()),
            (Mode::Visual, Interaction::default()),
        ] {
            app.editor_mut().mode = mode;
            let view = EditorView { app: &app };
            assert_eq!(
                view.mouse_interaction(&BlinkState::default(), BOUNDS, inside),
                expected,
                "{mode:?}"
            );
        }
        // outside the pane: no opinion even in insert mode
        app.editor_mut().mode = Mode::Insert;
        let view = EditorView { app: &app };
        assert_eq!(
            view.mouse_interaction(
                &BlinkState::default(),
                BOUNDS,
                Cursor::Available(Point::new(900.0, 100.0))
            ),
            Interaction::default()
        );
        // Regular mode: the mouse selects text, so it gets the I-beam in
        // every vim mode
        app.config.vim = false;
        for mode in [Mode::Normal, Mode::Visual, Mode::Insert] {
            app.editor_mut().mode = mode;
            let view = EditorView { app: &app };
            assert_eq!(
                view.mouse_interaction(&BlinkState::default(), BOUNDS, inside),
                Interaction::Text,
                "regular {mode:?}"
            );
        }
    }
}

#[cfg(test)]
mod blink_tests {
    use super::*;
    use crate::app::App;
    use crate::editor::buffer::Move;
    use std::time::Duration;

    /// A draw instant `ms` after the cycle began (monotonic: always after
    /// every instant previously fed to the state).
    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn block_blinks_600ms_on_350ms_off() {
        let s = BlinkState::default();
        let t0 = Instant::now();
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 0)));
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 599)));
        assert!(!s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 600)));
        assert!(!s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 949)));
        // the cycle repeats
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 950)));
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 1549)));
        assert!(!s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 1550)));
    }

    #[test]
    fn moving_the_cursor_restarts_the_blink_visible() {
        let s = BlinkState::default();
        let t0 = Instant::now();
        // establish a cycle, then run into the hidden phase ...
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 0)));
        assert!(!s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 700)));
        // ... a cursor move brings the block straight back
        let moved = Pos { row: 2, col: 7 };
        assert!(s.block_visible(moved, Mode::Normal, true, at(t0, 700)));
        assert!(s.block_visible(moved, Mode::Normal, true, at(t0, 701)));
    }

    #[test]
    fn mode_change_restarts_the_blink_visible() {
        let s = BlinkState::default();
        let t0 = Instant::now();
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 0)));
        assert!(!s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 700)));
        // e.g. v entered while the block was hidden: fresh 600ms phase
        assert!(s.block_visible(Pos::default(), Mode::Visual, true, at(t0, 1298)));
        assert!(!s.block_visible(Pos::default(), Mode::Visual, true, at(t0, 1899)));
    }

    #[test]
    fn unfocused_draws_stay_solid_and_restart_the_cycle() {
        let s = BlinkState::default();
        let t0 = Instant::now();
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 0)));
        assert!(!s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 700)));
        // focus moved to the diagram: the block holds solid (redraws stop
        // while unfocused, so a running cycle must not freeze it hidden)
        assert!(s.block_visible(Pos::default(), Mode::Normal, false, at(t0, 701)));
        assert!(s.block_visible(Pos::default(), Mode::Normal, false, at(t0, 10_000)));
        // back in focus: a fresh visible phase
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 10_001)));
        assert!(s.block_visible(Pos::default(), Mode::Normal, true, at(t0, 10_500)));
    }

    #[test]
    fn block_cursor_tracks_mode_and_regular_mode() {
        let mut app = App::default();
        assert!(is_block_cursor(&app), "vim normal mode");
        app.editor_mut().mode = Mode::Visual;
        assert!(is_block_cursor(&app));
        app.editor_mut().mode = Mode::Insert;
        assert!(!is_block_cursor(&app));
        // regular mode: thin caret in every vim mode
        app.config.vim = false;
        for mode in [Mode::Normal, Mode::Visual, Mode::Insert] {
            app.editor_mut().mode = mode;
            assert!(!is_block_cursor(&app), "{mode:?}");
        }
    }

    #[test]
    fn blink_gate_tracks_focus_pane_and_document() {
        let mut app = App::default();
        assert!(block_cursor_blinking(&app), "vim normal, focused, open");

        app.editor_mut().mode = Mode::Insert;
        assert!(!block_cursor_blinking(&app));
        app.editor_mut().mode = Mode::Normal;

        app.focus = Focus::Canvas;
        assert!(!block_cursor_blinking(&app));
        app.focus = Focus::Editor;

        app.editor_collapsed = true;
        assert!(!block_cursor_blinking(&app), "pane is off screen");
        app.editor_collapsed = false;

        app.config.vim = false;
        assert!(!block_cursor_blinking(&app), "regular mode caret is static");

        // a start-screen tab has no editor pane in the widget tree
        assert!(!block_cursor_blinking(&App::blank_start()));
    }

    #[test]
    fn inverted_glyph_picks_the_char_under_the_cursor() {
        let app = App::with_source("ab\n\n c".into());
        assert_eq!(inverted_glyph(&app, Pos::default()), Some('a'));
        assert_eq!(inverted_glyph(&app, Pos { row: 0, col: 1 }), Some('b'));
        // past end of line: bare block
        assert_eq!(inverted_glyph(&app, Pos { row: 0, col: 2 }), None);
        // empty line: bare block
        assert_eq!(inverted_glyph(&app, Pos { row: 1, col: 0 }), None);
        // whitespace: bare block (the text pass never paints it)
        assert_eq!(inverted_glyph(&app, Pos { row: 2, col: 0 }), None);
        assert_eq!(inverted_glyph(&app, Pos { row: 2, col: 1 }), Some('c'));
        // a row index past the buffer must not panic
        assert_eq!(inverted_glyph(&app, Pos { row: 9, col: 0 }), None);
    }

    #[test]
    fn zero_width_chars_get_a_bare_block_wide_chars_keep_their_glyph() {
        // char-level rule (fed combining marks through the app's
        // highlighter panics its byte slicing, so this stays pure):
        assert!(invertible_char('e'));
        assert!(invertible_char('世'), "wide char keeps its inverted glyph");
        assert!(!invertible_char('\u{301}'), "combining mark: bare block");
        assert!(!invertible_char('\u{7f}'), "control: bare block");
        assert!(!invertible_char('\t'), "whitespace: bare block");
    }

    #[test]
    fn jump_reaches_end_of_line_where_the_block_is_bare() {
        // cursor positions the draw pass actually sees: Jump clamps to
        // line length (the insert column), one past the last char
        let mut app = App::with_source("ab".into());
        app.editor_mut().buf.move_cursor(Move::Jump(0, 2));
        assert_eq!(app.editor().buf.pos(), Pos { row: 0, col: 2 });
        assert_eq!(inverted_glyph(&app, app.editor().buf.pos()), None);
    }
}

/// The editor's floating overlays (error tooltip, hover docs, completion
/// popup) as their own canvas widget, STACKED above the editor canvas:
/// iced flattens one widget's geometries into a single mesh pass followed
/// by a single text pass, so a tooltip background drawn in a second
/// geometry landed behind the editor text (geometry 1's text pass) and
/// the tooltip read as transparent. Separate widgets composite fully in
/// order, which restores bg-under-text within the overlay.
pub struct EditorOverlay<'a> {
    pub app: &'a App,
}

impl canvas::Program<Message> for EditorOverlay<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let app = self.app;
        let marks = error_marks(app);
        // tooltips on hover: errors win, then definition docs; suppressed
        // while the completion popup is open
        if let Some(hover) = app.editor_hover {
            if app.completion.is_none() {
                let pos = px_to_pos(app, hover);
                if let Some(m) = marks
                    .iter()
                    .find(|m| m.row == pos.row && pos.col >= m.start && pos.col <= m.end)
                {
                    let (rect, lines) = tooltip_layout(hover, &m.message, bounds);
                    frame.fill(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(6.0),
                        ),
                        theme::TOOLTIP,
                    );
                    frame.stroke(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(6.0),
                        ),
                        canvas::Stroke {
                            style: theme::ERR.into(),
                            width: 1.0,
                            ..Default::default()
                        },
                    );
                    for (i, line) in lines.iter().enumerate() {
                        frame.fill_text(Text {
                            content: line.clone(),
                            position: Point::new(rect.x + 12.0, rect.y + 12.0 + i as f32 * 18.0),
                            color: theme::TEXT,
                            size: (FONT_SIZE - 1.0).into(),
                            font: Font::MONOSPACE,
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            ..Default::default()
                        });
                    }
                } else if let Some(doc) = doc_hover_at(app, hover) {
                    let rect = doc_rect(hover, &doc, bounds);
                    frame.fill(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(6.0),
                        ),
                        theme::TOOLTIP,
                    );
                    frame.stroke(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(6.0),
                        ),
                        canvas::Stroke {
                            style: theme::BORDER.into(),
                            width: 1.0,
                            ..Default::default()
                        },
                    );
                    frame.fill_text(Text {
                        content: doc.title.clone(),
                        position: Point::new(rect.x + 12.0, rect.y + 10.0),
                        color: theme::TEXT,
                        size: (FONT_SIZE - 1.0).into(),
                        font: Font {
                            weight: iced::font::Weight::Bold,
                            ..Font::MONOSPACE
                        },
                        horizontal_alignment: iced::alignment::Horizontal::Left,
                        vertical_alignment: iced::alignment::Vertical::Top,
                        ..Default::default()
                    });
                    for (i, (marker, text)) in doc.rows.iter().enumerate() {
                        let ry = rect.y + 28.0 + i as f32 * 16.0;
                        frame.fill_text(Text {
                            content: marker.clone(),
                            position: Point::new(rect.x + 12.0, ry),
                            color: if marker == "\u{25c6}" {
                                theme::PK
                            } else if marker == "\u{25c7}" {
                                theme::FK
                            } else {
                                theme::TEXT
                            },
                            size: (FONT_SIZE - 1.0).into(),
                            font: Font::MONOSPACE,
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            ..Default::default()
                        });
                        frame.fill_text(Text {
                            content: text.clone(),
                            position: Point::new(rect.x + 12.0 + 2.0 * char_width(), ry),
                            color: theme::TEXT,
                            size: (FONT_SIZE - 1.0).into(),
                            font: Font::MONOSPACE,
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            ..Default::default()
                        });
                    }
                }
            }
        }

        if let Some((prect, first_row, visible)) = popup_layout(app, bounds) {
            let c = app.completion.as_ref().unwrap();
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    prect.position(),
                    prect.size(),
                    iced::border::Radius::from(6.0),
                ),
                theme::TOOLTIP,
            );
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    prect.position(),
                    prect.size(),
                    iced::border::Radius::from(6.0),
                ),
                canvas::Stroke {
                    style: theme::BORDER.into(),
                    width: 1.0,
                    ..Default::default()
                },
            );
            for (i, item) in c.items[first_row..first_row + visible].iter().enumerate() {
                let sel = first_row + i == c.selected;
                let ry = prect.y + i as f32 * ROW_H;
                if sel {
                    frame.fill_rectangle(
                        Point::new(prect.x + 2.0, ry),
                        Size::new(prect.width - 4.0, ROW_H),
                        Color::from_rgba(0.267, 0.576, 0.973, 0.25),
                    );
                }
                frame.fill_text(Text {
                    content: item.label.clone(),
                    position: Point::new(prect.x + 10.0, ry + ROW_H * 0.5),
                    color: item_kind_color(item.kind),
                    size: (FONT_SIZE - 1.0).into(),
                    font: Font::MONOSPACE,
                    horizontal_alignment: iced::alignment::Horizontal::Left,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
            }
        }
        vec![frame.into_geometry()]
    }
}
