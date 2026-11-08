//! The editor canvas: monospace code text with a gutter, a mode-styled
//! fake cursor, and visual-mode selection spans. Scrolling is a float row
//! offset kept in `App`; the canvas just draws the visible window.

use crate::app::{App, Focus, Maximized, Message};
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
/// room for the overlay scrollbar (dbdiagram's `scrollBeyondLastColumn`).
/// Part of `App::max_hscroll`.
pub const H_RIGHT_MARGIN: f32 = 16.0;
/// Overlay scrollbar band width (Monaco-style: no layout reservation, the
/// thumb floats over the text; the margin above keeps line ends clear).
pub const SCROLLBAR_W: f32 = 10.0;

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

/// The thin caret's x for the current cursor + selection: with a live
/// selection whose cursor is the HIGH side, the caret sits after the last
/// covered char — at the band's right edge (selection_draw_range's end
/// column, clamped at the insert column) — not one char before it. A
/// reversed selection (cursor on the low side) keeps the caret at the
/// band's left edge, and no selection at all puts it on the cursor cell.
fn caret_x(app: &App, scroll: f32, hs: f32) -> f32 {
    let cursor = app.editor().buf.pos();
    let x = pos_to_px(cursor, scroll, hs).x;
    // the band convention follows the caret: vim visual's inclusive end
    // for the block cursor, the exclusive insertion boundary for the
    // thin caret — either way the caret sits at the band's right edge
    // when the cursor is the HIGH side
    let range = if is_block_cursor(app) {
        app.editor().buf.selection_draw_range()
    } else {
        app.editor().buf.selection_draw_range_exclusive()
    };
    match (
        app.editor().buf.selection_anchor(),
        range,
    ) {
        (Some(a), Some((_, end)))
            if cursor.row > a.row || (cursor.row == a.row && cursor.col > a.col) =>
        {
            pos_to_px(end, scroll, hs).x
        }
        _ => x,
    }
}

/// Buffer position under a local point (clamped into the text).
pub fn px_to_pos(app: &App, p: Point) -> Pos {
    let lines = app.editor().buf.lines();
    // floor AFTER adding the scroll: the bottom clamp leaves editor_scroll
    // fractional (line_count - fractional page_lines), and flooring the
    // click line and the scroll separately lost a line whenever the two
    // fractions summed past 1 — clicks landed one row above
    let row = (((p.y / LINE_HEIGHT) + app.cur().editor_scroll).floor() as usize)
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

/// Rectangles of the charwise selection band, for a range whose end
/// `Buffer::selection_draw_range` already clamped to the end line's text
/// (so an insert-column end stops at the last character cell instead of
/// carrying an empty trailing cell or wrapping to the next row). The start
/// row and mid rows band full-width — their newline is selected, like
/// vim's charwise visual — while the end row bands only over its covered
/// cells: never past its last character, and nothing at all when no cell
/// of it is covered (an empty end line, or an empty same-row span — a
/// plain click on the insert column must not highlight the empty space).
fn selection_band_rects(
    start: Pos,
    end: Pos,
    scroll: f32,
    hs: f32,
    view_width: f32,
) -> Vec<Rectangle> {
    let top = pos_to_px(start, scroll, hs);
    let bottom = pos_to_px(end, scroll, hs);
    // FULL line height: a 0.9 factor left a 10% gap between the start
    // band and the full-row block (and between every V-linewise row) —
    // the bands must tile seamlessly. Adjacent, never overlapping, so
    // the translucent fill never double-blends at a seam.
    let band_h = LINE_HEIGHT;
    let mut rects = Vec::new();
    if top.y == bottom.y {
        // exactly the span's cells; a zero-width span (anchor == cursor on
        // the insert column) draws nothing
        let w = bottom.x - top.x;
        if w > 0.0 {
            rects.push(Rectangle::new(top, Size::new(w, band_h)));
        }
    } else {
        let start_w = view_width - top.x;
        if start_w > 0.0 {
            rects.push(Rectangle::new(top, Size::new(start_w, band_h)));
        }
        let full_rows = end.row.saturating_sub(start.row + 1);
        if full_rows > 0 {
            rects.push(Rectangle::new(
                Point::new(GUTTER, top.y + LINE_HEIGHT),
                Size::new(view_width - GUTTER, full_rows as f32 * LINE_HEIGHT),
            ));
        }
        let tail = bottom.x - GUTTER;
        if tail > 0.0 {
            rects.push(Rectangle::new(
                Point::new(GUTTER, bottom.y),
                Size::new(tail, band_h),
            ));
        }
    }
    rects
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
    /// Eased overlay-scrollbar opacity, stepped toward its target on every
    /// draw (draws at the 16ms tick while a fade is in flight).
    scrollbar_alpha: Cell<f32>,
}

impl Default for BlinkState {
    fn default() -> Self {
        Self {
            last: Cell::default(),
            // placeholder: `last` starts unset, so the first block_visible
            // call always resets this before it is ever subtracted from
            cycle_start: Cell::new(Instant::now()),
            scrollbar_alpha: Cell::new(0.0),
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
/// editor pane is actually on screen: maximizing the diagram removes the
/// canvas from the widget tree, and the start screen has no editor pane.
pub fn block_cursor_blinking(app: &App) -> bool {
    is_block_cursor(app)
        && app.focus == Focus::Editor
        && app.maximized != Maximized::Diagram
        && app.doc_open()
}

/// Whether the THIN caret (insert mode / regular editor) is on screen and
/// should blink: the same conditions as the block cursor minus the mode
/// gate (the caret is every non-block state).
pub fn caret_blinking(app: &App) -> bool {
    app.focus == Focus::Editor
        && app.maximized != Maximized::Diagram
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
        font: theme::mono(),
        horizontal_alignment: iced::alignment::Horizontal::Right,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Default::default()
    });
}

/// Track/thumb geometry of one overlay scrollbar, pane-local. Shared by
/// the draw pass and the press/drag hit-testing so both agree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarGeom {
    /// Track length along the bar's axis (px).
    pub track: f32,
    /// Thumb length along the track, and its offset from the track start.
    pub thumb_len: f32,
    pub thumb_pos: f32,
    /// Scroll offset this bar maps (v_max / h_max): the drag converts a
    /// thumb fraction back into a scroll target through it.
    pub max: f32,
    /// The bar's own band rectangle: vertical bars anchor at the pane's
    /// right edge, horizontal at the bottom.
    pub band: Rectangle,
}

impl ScrollbarGeom {
    /// Whether a pane-local point is inside the thumb.
    pub fn thumb_contains(&self, p: Point) -> bool {
        if !self.band.contains(p) {
            return false;
        }
        if self.band.width >= self.band.height {
            p.x >= self.band.x + self.thumb_pos
                && p.x <= self.band.x + self.thumb_pos + self.thumb_len
        } else {
            p.y >= self.band.y + self.thumb_pos
                && p.y <= self.band.y + self.thumb_pos + self.thumb_len
        }
    }

    /// Whether the point is on the bar at all (band area, e.g. track click).
    pub fn band_contains(&self, p: Point) -> bool {
        self.band.contains(p)
    }
}

/// Geometry of both overlay scrollbars for the current buffer + scroll.
/// `None` on an axis with no overflow (never drawn, never hit-tested).
pub fn scrollbar_layout(app: &App, bounds: Rectangle) -> (Option<ScrollbarGeom>, Option<ScrollbarGeom>) {
    let lines = app.editor().buf.lines();
    let total_rows = lines.len() as f32;
    let max_cols = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as f32;
    let h_max = (max_cols * char_width() - (bounds.width - GUTTER - H_RIGHT_MARGIN)).max(0.0);
    // mirror the app's page_lines: the horizontal scrollbar's reserved
    // line at the bottom shrinks the vertical page — the drag math and
    // the scroll clamp must agree or the thumb thrashes
    let reserved = if h_max > 0.0 { LINE_HEIGHT } else { 0.0 };
    let page_rows = ((bounds.height - reserved) / LINE_HEIGHT).max(1.0);
    let v_max = (total_rows - page_rows).max(0.0);
    let scroll = app.cur().editor_scroll;
    let hs = app.cur().editor_hscroll;
    // the drag math must agree with the app's clamp EXACTLY: bounds-based
    // math drifts a few px from the window-chrome-based max_scroll(), and
    // a target past the clamp flips the delta sign — the scroll bounces
    // back while dragging (the thrash)
    let app_max = app.max_scroll();
    let app_hmax = app.max_hscroll();
    let v = if v_max > 0.0 {
        // the horizontal bar shortens the vertical track at the corner
        let track = bounds.height - if h_max > 0.0 { SCROLLBAR_W } else { 0.0 };
        // max-then-min (not clamp): never panics on a tiny canvas; 24px
        // floor so huge documents keep a grabbable-looking thumb
        let thumb_len = (track * page_rows / total_rows).max(24.0).min(track);
        let frac = (scroll / v_max).clamp(0.0, 1.0);
        Some(ScrollbarGeom {
            track,
            thumb_len,
            thumb_pos: (track - thumb_len) * frac,
            max: app_max,
            band: Rectangle::new(
                Point::new(bounds.width - SCROLLBAR_W, 0.0),
                Size::new(SCROLLBAR_W, track),
            ),
        })
    } else {
        None
    };
    let h = if h_max > 0.0 {
        let track = bounds.width - if v_max > 0.0 { SCROLLBAR_W } else { 0.0 };
        let thumb_len = (track * (bounds.width - GUTTER) / (max_cols * char_width()))
            .max(24.0)
            .min(track);
        let frac = (hs / h_max).clamp(0.0, 1.0);
        Some(ScrollbarGeom {
            track,
            thumb_len,
            thumb_pos: (track - thumb_len) * frac,
            max: app_hmax,
            band: Rectangle::new(
                Point::new(0.0, bounds.height - SCROLLBAR_W),
                Size::new(track, SCROLLBAR_W),
            ),
        })
    } else {
        None
    };
    (v, h)
}

pub struct EditorView<'a> {
    pub app: &'a App,
}

impl canvas::Program<Message> for EditorView<'_> {
    type State = BlinkState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        let _ = state; // the scrollbar drag lives at the app level
        use iced::mouse;
        // The scrollbar drag itself lives at the APP level (see
        // `Message::ScrollbarDragStart` + `MouseMovedWindow`): widget event
        // delivery proved flaky once the pointer left the pane — the app's
        // subscription provably arrives window-wide (the divider and tab
        // drags already rely on it).
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
                // both axes are PX under the EditorScrolled contract: the
                // wheel's notch deltas convert here (one notch = one
                // character cell horizontally, one line vertically)
                let (v, h) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => {
                        (-y * LINE_HEIGHT, -x * char_width())
                    }
                    mouse::ScrollDelta::Pixels { x, y } => (-y, -x),
                };
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorScrolled { v, h }),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // a press on the popup picks that row
                if let Some((rect, first_row, _)) = popup_layout(self.app, bounds) {
                    if rect.contains(local) {
                        let row = ((local.y - rect.y) / ROW_H) as usize;
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::CompletionPicked(first_row + row)),
                        );
                    }
                }
                // the scrollbar bands own their pane edges before the text
                // does. ANY band press arms a drag (GTK model): a thumb
                // press grabs where you caught it; a track press first
                // centers the thumb under the pointer, then drags — one
                // gesture, no track-vs-thumb mode to get wrong. (The old
                // split — page-jump on the track, drag only on the thumb —
                // thrashed: a press a pixel outside the thumb paged a whole
                // viewport and armed nothing.)
                let (vbar, hbar) = scrollbar_layout(self.app, bounds);
                if let Some(bar) = vbar.filter(|b| b.band_contains(local)) {
                    let (grab, jump_to) = if bar.thumb_contains(local) {
                        // grab where you caught it; no jump
                        (local.y - bar.band.y - bar.thumb_pos, None)
                    } else {
                        // track press: center the thumb under the pointer,
                        // then the drag continues from there
                        let span = (bar.track - bar.thumb_len).max(1.0);
                        let frac = ((local.y - bar.thumb_len / 2.0) / span).clamp(0.0, 1.0);
                        (bar.thumb_len / 2.0, Some(frac * bar.max))
                    };
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::ScrollbarDragStart {
                            vertical: true,
                            grab,
                            band_origin: bounds.y + bar.band.y,
                            span: bar.track - bar.thumb_len,
                            max: bar.max,
                            jump_to,
                        }),
                    );
                }
                if let Some(bar) = hbar.filter(|b| b.band_contains(local)) {
                    let (grab, jump_to) = if bar.thumb_contains(local) {
                        (local.x - bar.band.x - bar.thumb_pos, None)
                    } else {
                        let span = (bar.track - bar.thumb_len).max(1.0);
                        let frac = ((local.x - bar.thumb_len / 2.0) / span).clamp(0.0, 1.0);
                        (bar.thumb_len / 2.0, Some(frac * bar.max))
                    };
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::ScrollbarDragStart {
                            vertical: false,
                            grab,
                            band_origin: bounds.x + bar.band.x,
                            span: bar.track - bar.thumb_len,
                            max: bar.max,
                            jump_to,
                        }),
                    );
                }
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorPressed(local)),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // a scrollbar drag started here ends at the app level
                // (Message::MouseReleased clears it)
                (canvas::event::Status::Ignored, None)
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
        cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::PANEL);

        let app = self.app;
        // pane-local pointer position, captured before the buffer's cursor
        // position shadows the name below
        let pointer = cursor
            .position_over(bounds)
            .map(|p| Point::new(p.x - bounds.x, p.y - bounds.y));
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
                        // full line height: 0.9 left a gap after every
                        // highlighted row
                        frame.fill_rectangle(
                            Point::new(GUTTER, y),
                            Size::new(bounds.width - GUTTER, LINE_HEIGHT),
                            Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                        );
                    }
                }
            }
        } else if let Some((start, end)) = if is_block_cursor(app) {
            // vim visual: the cursor sits ON the last selected char — the
            // band counts it as covered (inclusive)
            app.editor().buf.selection_draw_range()
        } else {
            // the thin-caret modes: the caret is the insertion boundary —
            // the band stops AT it. The inclusive +1 highlighted one char
            // past the caret on every shift/alt+arrow press
            app.editor().buf.selection_draw_range_exclusive()
        } {
            // both variants clamp the end to the end line's text, so a
            // selection ending on the insert column never bands into the
            // empty space past it; `selection_band_rects` lays out the
            // per-row geometry
            for r in selection_band_rects(start, end, scroll, hs, bounds.width) {
                frame.fill_rectangle(
                    Point::new(r.x, r.y),
                    Size::new(r.width, r.height),
                    Color::from_rgba(0.3, 0.5, 0.9, 0.35),
                );
            }
        }

        // search match highlights (the vim / search and the find popup
        // share the buffer's pattern): a soft yellow band per match,
        // under the text like the selection. `search_hl_spans` is empty
        // while the bands are hidden (:noh — Esc / insert / yank / visual
        // / closing the find bar), the pattern then still cycles via n/N.
        if app.editor().buf.has_search_pattern() {
            let ink = Color::from_rgba(0.95, 0.85, 0.30, 0.28);
            for (r, c0, c1) in app.editor().buf.search_hl_spans() {
                let y = (r as f32 - scroll) * LINE_HEIGHT;
                if y <= -LINE_HEIGHT || y >= bounds.height {
                    continue;
                }
                let x0 = GUTTER + c0 as f32 * char_width() - hs;
                let w = (c1 - c0) as f32 * char_width();
                frame.fill_rectangle(Point::new(x0, y), Size::new(w, LINE_HEIGHT * 0.9), ink);
            }
        }

        // error marks (squiggles + gutter dots + hover tooltip)
        let marks = error_marks(app);

        // yank flash: vim's on_yank blink — the yanked region highlights
        // and fades over ~250ms (fills land under the text pass)
        if let Some(f) = app.editor().yank_flash {
            let el = f.started.elapsed().as_millis() as f32;
            let debug = std::env::var("DBML_UI_FLASH_DEBUG").is_ok();
            let dur = if debug { 5000.0 } else { 250.0 };
            if debug {
                eprintln!(
                    "[flash] draw: el={el:.0}ms start={:?} end={:?}",
                    f.start, f.end
                );
            }
            if el < dur {
                let alpha = if debug {
                    0.6
                } else {
                    0.28 * (1.0 - el / dur)
                };
                let color = Color { a: alpha, ..theme::PK };
                // linewise yanks span whole rows: encode the range as
                // (first row, col 0) .. (end row, col 0) — the band helper
                // covers full rows for that shape
                let start = crate::editor::buffer::Pos { row: f.start.0, col: f.start.1 };
                let end = crate::editor::buffer::Pos { row: f.end.0, col: f.end.1 };
                for band in selection_band_rects(start, end, scroll, hs, bounds.width) {
                    frame.fill_rectangle(band.position(), band.size(), color);
                }
            }
        }

        // gutter + text, token-colored
        let src = app.source();
        // the EOL-marker row (the empty last row a trailing newline
        // stores) is not a line: it gets no gutter number, so the
        // blank-looking space below the document — scrollbar reserve
        // included — never reads as a line the file contains
        let marker_row = if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
            Some(lines.len() - 1)
        } else {
            None
        };
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
            if let Some(m) = marks.iter().find(|m| m.row == i) {
                frame.fill(
                    &canvas::Path::circle(Point::new(10.0, text_y), 2.5),
                    if m.warn { theme::WARN } else { theme::ERR },
                );
            }
            if Some(i) != marker_row {
                gutter_number(&mut frame, i, text_y);
            }
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
                                font: theme::mono(),
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
                                font: theme::mono(),
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                        }
                    }
                }
            } else {
                // insert mode and regular mode: the thin caret, blinking
                // on the same cycle as the block (the blink restarts on
                // every cursor move). With a live selection whose cursor
                // is the HIGH side, the caret sits AFTER the last covered
                // char (the band counts the cursor-side char as covered —
                // draw_range's +1): at the band's right edge, not one
                // char before it.
                if state.block_visible(
                    cursor,
                    app.editor().mode,
                    caret_blinking(app),
                    Instant::now(),
                ) {
                    frame.fill_rectangle(
                        Point::new(caret_x(app, scroll, hs), p.y),
                        Size::new(2.0, LINE_HEIGHT * 0.9),
                        theme::TEXT,
                    );
                }
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
                    style: (if m.warn { theme::WARN } else { theme::ERR }).into(),
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
                if let Some(m) = marks.iter().find(|m| m.row == i) {
                    frame.fill(
                        &canvas::Path::circle(Point::new(10.0, text_y), 2.5),
                        if m.warn { theme::WARN } else { theme::ERR },
                    );
                }
                gutter_number(&mut frame, i, text_y);
                gy += LINE_HEIGHT;
            }
        }

        // Overlay scrollbars (Monaco-style, dbdiagram's editor): rounded
        // translucent thumbs on invisible tracks floating over the text.
        // They ease in while the pointer is over the pane or the user
        // scrolled/dragged recently, and back out when idle — the layout
        // never shifts, and H_RIGHT_MARGIN keeps line ends clear of the
        // band when fully scrolled right.
        let (vbar, hbar) = scrollbar_layout(app, bounds);
        if vbar.is_some() || hbar.is_some() {
            let recent = app
                .last_editor_scroll
                .is_some_and(|t| t.elapsed().as_millis() < 700);
            let drag = app.scrollbar_drag;
            let shown = app.editor_hover.is_some() || recent || drag.is_some();
            // step the eased opacity toward the target; the 16ms tick keeps
            // redrawing while a fade is in flight (see `subscription`)
            let cur = state.scrollbar_alpha.get();
            let alpha = if shown {
                (cur + 0.12).min(1.0)
            } else {
                (cur - 0.08).max(0.0)
            };
            state.scrollbar_alpha.set(alpha);
            if alpha > 0.0 {
                let hover_local = pointer;
                let thumb_color = |bar: &ScrollbarGeom, dragged: bool| {
                    let active = dragged
                        || hover_local.is_some_and(|p| bar.thumb_contains(p));
                    Color {
                        a: if active {
                            theme::SCROLLBAR_ACTIVE_ALPHA
                        } else {
                            theme::SCROLLBAR_IDLE_ALPHA
                        } * alpha,
                        ..theme::DIM
                    }
                };
                if let Some(bar) = vbar {
                    frame.fill(
                        &canvas::Path::rounded_rectangle(
                            Point::new(bar.band.x, bar.band.y + bar.thumb_pos),
                            Size::new(SCROLLBAR_W, bar.thumb_len),
                            iced::border::Radius::from(SCROLLBAR_W / 2.0),
                        ),
                        thumb_color(&bar, drag.is_some_and(|d| d.vertical)),
                    );
                }
                if let Some(bar) = hbar {
                    frame.fill(
                        &canvas::Path::rounded_rectangle(
                            Point::new(bar.band.x + bar.thumb_pos, bar.band.y),
                            Size::new(bar.thumb_len, SCROLLBAR_W),
                            iced::border::Radius::from(SCROLLBAR_W / 2.0),
                        ),
                        thumb_color(&bar, drag.is_some_and(|d| !d.vertical)),
                    );
                }
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        let _ = state; // drag state lives at the app level
        // canvas mouse_interaction is not bounds-gated by iced, so check
        // hover here: I-beam while typing or in Regular mode (where the
        // mouse selects text), plain arrow otherwise (normal/visual
        // motions are keyboard-driven). Drags never change it — a
        // scrollbar or text drag keeps the cursor it had at the click.
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
/// Warnings (dbdiagram's soft-blue class) draw in their own color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub message: String,
    /// Soft-blue warning (strict-grammar / semantic lint) vs red error.
    pub warn: bool,
}

/// One row of a definition tooltip: the column's text plus the
/// embellishments the diagram shows for it (pk/fk/unique icons, not-null,
/// enum type, default).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocRow {
    pub text: String,
    pub pk: bool,
    pub fk: bool,
    pub unique: bool,
    pub not_null: Option<bool>,
    pub enum_ty: bool,
    pub default: Option<String>,
}

impl DocRow {
    /// A plain row with no embellishments (enum value lists).
    pub fn plain(text: String) -> Self {
        Self {
            text,
            pk: false,
            fk: false,
            unique: false,
            not_null: None,
            enum_ty: false,
            default: None,
        }
    }

    /// Whether any embellishment slot is occupied (drives the icon glyph).
    pub fn has_icon(&self) -> bool {
        self.pk || self.fk || self.unique
    }
}

/// A definition tooltip: title line plus per-column rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocHover {
    pub title: String,
    pub rows: Vec<DocRow>,
}

/// Parse errors and diagnostics as per-line underline ranges: the token
/// at the finding's offset, one char minimum (point findings still get a
/// visible mark). Errors first so hover resolution prefers them.
pub fn error_marks(app: &App) -> Vec<Mark> {
    let src = app.source();
    let mut marks = Vec::new();
    let mut push = |offset: usize, message: &str, warn: bool| {
        let (row, byte_col) = dbml_ui::dbml::parser::line_col_at(src, offset);
        let Some(line) = src.split('\n').nth(row) else {
            return;
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
            message: message.to_string(),
            warn,
        });
    };
    for e in &app.cur().parse_errors {
        push(e.offset, &e.message, false);
    }
    for d in &app.cur().diagnostics {
        push(
                        d.offset,
                        &d.message,
                        d.severity != dbml_ui::dbml::validate::Severity::Error,
                    );
    }
    marks
}

/// Whether a hover point sits on a real buffer row: `px_to_pos` clamps
/// its row onto the last line, so without this check the empty space
/// under the buffer would resolve onto that line's words and marks.
fn hover_on_text(app: &App, p: Point) -> bool {
    let row = (p.y / LINE_HEIGHT + app.cur().editor_scroll).floor();
    row >= 0.0 && row < app.editor().buf.lines().len() as f32
}

/// The mark under a hover point, if any: the squiggle's own cells (its
/// char range, end exclusive) on its row. Points below the buffer miss.
pub fn mark_hover_at<'a>(app: &App, marks: &'a [Mark], p: Point) -> Option<&'a Mark> {
    if !hover_on_text(app, p) {
        return None;
    }
    let pos = px_to_pos(app, p);
    marks
        .iter()
        .find(|m| m.row == pos.row && pos.col >= m.start && pos.col < m.end)
}

/// The word under a hover point, if the pointer is on a word character.
pub(crate) fn word_at(app: &App, p: Point) -> Option<String> {
    if !hover_on_text(app, p) {
        return None;
    }
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

/// Definition doc for the hovered word: a table by name or alias, or an
/// enum by name (enums are not diagram boxes, so their docs come from the
/// layout's enum list).
pub fn doc_hover_at(app: &App, p: Point) -> Option<DocHover> {
    let word = word_at(app, p)?;
    let pos = px_to_pos(app, p);
    let chars: Vec<char> = app.editor().buf.line(pos.row).chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let diagram = app.diagram();
    // a column reference `table.column` / `alias.column`: the hovered
    // word is the column and the dot-bound qualifier names the table —
    // hovering ANYWHERE in `id` of `Ref: posts.user_id > users.id` shows
    // THAT column's definition (the word alone is ambiguous: many tables
    // have an `id`). The qualifier is the word left of the DOT — not the
    // char before the caret, which only matched the column's first char.
    let word_start = {
        let mut s = pos.col;
        while s > 0 && chars.get(s - 1).copied().is_some_and(is_word) {
            s -= 1;
        }
        s
    };
    if word_start > 0 && chars.get(word_start - 1) == Some(&'.') {
        let mut qs = word_start - 1;
        while qs > 0 && (chars[qs - 1].is_alphanumeric() || chars[qs - 1] == '_') {
            qs -= 1;
        }
        let qualifier: String = chars[qs..word_start - 1].iter().collect();
        if !qualifier.is_empty() {
            let table = diagram
                .boxes
                .iter()
                .find(|b| b.title == qualifier)
                .or_else(|| {
                    diagram
                        .boxes
                        .iter()
                        .find(|b| b.alias.as_deref() == Some(qualifier.as_str()))
                });
            if let Some(b) = table {
                if let Some(r) = b.rows.iter().find(|r| r.name == word) {
                    let text = if r.ty.is_empty() {
                        r.name.clone()
                    } else {
                        format!("{}  {}", r.name, r.ty)
                    };
                    return Some(DocHover {
                        title: format!("{}.{}", b.title, r.name),
                        rows: vec![DocRow {
                            text,
                            pk: r.pk,
                            fk: r.fk,
                            unique: r.unique,
                            not_null: r.not_null,
                            enum_ty: r.ty_enum,
                            default: r.default.clone(),
                        }],
                    });
                }
            }
        }
    }
    // exact name first, then alias (`Table users as U`); two tables could
    // share an alias — the first in document order wins
    let boxes = &diagram.boxes;
    if let Some(b) = boxes.iter().find(|b| b.title == word).or_else(|| {
        boxes
            .iter()
            .find(|b| b.alias.as_deref() == Some(word.as_str()))
    }) {
        let rows = b
            .rows
            .iter()
            .map(|r| {
                let text = if r.ty.is_empty() {
                    r.name.clone()
                } else {
                    format!("{}  {}", r.name, r.ty)
                };
                DocRow {
                    text,
                    pk: r.pk,
                    fk: r.fk,
                    unique: r.unique,
                    not_null: r.not_null,
                    enum_ty: r.ty_enum,
                    default: r.default.clone(),
                }
            })
            .collect();
        return Some(DocHover {
            title: format!("Table {}", b.title),
            rows,
        });
    }
    // an enum type name: its members, no embellishments
    let e = diagram.enums.iter().find(|e| e.name == word)?;
    Some(DocHover {
        title: format!("Enum {}", e.name),
        rows: e.values.iter().map(|v| DocRow::plain(v.clone())).collect(),
    })
}

/// The doc tooltip's title with dbml syntax coloring: the leading
/// keyword (`Table`/`Enum`) in the keyword red, the name in the name
/// purple, a `table.column` ref with the column in the completion-column
/// blue — the same palette the buffer highlights these words with.
fn draw_doc_title(frame: &mut canvas::Frame, title: &str, at: Point) {
    let size = (FONT_SIZE - 1.0).into();
    let font = Font {
        weight: iced::font::Weight::Bold,
        ..theme::mono()
    };
    let cw = (FONT_SIZE - 1.0) * CHAR_RATIO;
    let mut x = at.x;
    let put = |frame: &mut canvas::Frame, s: &str, c: Color, x: &mut f32| {
        frame.fill_text(Text {
            content: s.to_string(),
            position: Point::new(*x, at.y),
            color: c,
            size,
            font,
            horizontal_alignment: iced::alignment::Horizontal::Left,
            vertical_alignment: iced::alignment::Vertical::Top,
            ..Default::default()
        });
        *x += s.chars().count() as f32 * cw;
    };
    if let Some(name) = title.strip_prefix("Table ") {
        put(frame, "Table ", theme::SYN_KEYWORD, &mut x);
        put(frame, name, theme::SYN_NAME, &mut x);
    } else if let Some(name) = title.strip_prefix("Enum ") {
        put(frame, "Enum ", theme::SYN_KEYWORD, &mut x);
        put(frame, name, theme::SYN_NAME, &mut x);
    } else if let Some((t, c)) = title.split_once('.') {
        put(frame, t, theme::SYN_NAME, &mut x);
        put(frame, ".", theme::DIM, &mut x);
        put(frame, c, theme::SYN_NUMBER, &mut x);
    } else {
        put(frame, title, theme::TEXT, &mut x);
    }
}

/// A hover-doc row's embellishment pills (the diagram's row icons and
/// setting pills, in the doc's compact size): (label, present). Nullable
/// gets no pill — the absence of "Not null" says it.
fn doc_row_pills(row: &DocRow) -> Vec<(&'static str, bool)> {
    vec![
        ("Enum", row.enum_ty),
        ("Not null", row.not_null == Some(true)),
    ]
}

/// The width of one hover-doc row's content past the icon slot.
fn doc_row_content_w(row: &DocRow, text_w: f32) -> f32 {
    let pill_w: f32 = doc_row_pills(row)
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(label, _)| label.chars().count() as f32 * 5.4 + 10.0 + 4.0)
        .sum();
    let def_w = row
        .default
        .as_ref()
        .map(|d| (d.chars().count() + 10) as f32 * text_w + 8.0)
        .unwrap_or(0.0);
    (row.text.chars().count() as f32 * text_w) + pill_w + def_w
}

/// Tooltip placement: below-right of the pointer preferred; an axis that
/// would leave the pane flips to the other side of the pointer (above /
/// left, still anchored to it) when the card fits there, and a card that
/// fits nowhere clamps into the pane — pinned at its top/left edge.
fn place_rect(hover: Point, w: f32, h: f32, bounds: Rectangle) -> Point {
    let left = bounds.x + 4.0;
    let top = bounds.y + 4.0;
    let right = bounds.x + bounds.width - 4.0;
    let bottom = bounds.y + bounds.height - 4.0;
    let mut x = hover.x + 14.0;
    if x + w > right && hover.x - 14.0 - w >= left {
        x = hover.x - 14.0 - w;
    }
    x = x.min(right - w).max(left);
    let mut y = hover.y + LINE_HEIGHT + 4.0;
    if y + h > bottom && hover.y - h - 6.0 >= top {
        y = hover.y - h - 6.0;
    }
    y = y.min(bottom - h).max(top);
    Point::new(x, y)
}

/// Doc tooltip geometry: same placement rules as the error tooltip.
fn doc_rect(hover: Point, doc: &DocHover, bounds: Rectangle) -> Rectangle {
    // the REAL mono advance the doc text renders with (the editor grid
    // width) — the old 13px*0.6 estimate ran ~7% narrow, so the widest
    // rows' pills drifted into the card's right edge
    let text_w = char_width();
    let widest = doc
        .rows
        .iter()
        .map(|r| 16.0 + doc_row_content_w(r, text_w))
        .chain(
            std::iter::once(doc.title.chars().count() as f32 * text_w + 16.0),
        )
        .fold(140.0f32, f32::max)
        + 24.0;
    // title zone + its divider + the rows + the bottom pad
    let h = (doc.rows.len() as f32 * 16.0) + 38.0;
    Rectangle::new(place_rect(hover, widest, h, bounds), Size::new(widest, h))
}

/// Error tooltip: geometry plus the message wrapped to fit the pane,
/// placed by `place_rect` like the doc card (flip above/left, clamp as
/// a last resort).
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
    let p = place_rect(hover, w, h, bounds);
    (Rectangle::new(p, Size::new(w, h)), lines)
}

/// Geometry of the open completion popup: rect, first visible row, visible
/// count. Positioned under the caret, clamped/flipped to stay inside the
/// pane. None when the popup is closed.
fn popup_layout(app: &App, bounds: Rectangle) -> Option<(Rectangle, usize, usize)> {
    let c = app.completion.as_ref()?;
    let visible = c.items.len().min(COMPLETION_ROWS);
    // nvim-cmp shape: label left, kind name right-aligned in its own column
    let label_w = c
        .items
        .iter()
        .map(|i| i.label.chars().count())
        .max()
        .unwrap_or(8) as f32
        * char_width();
    let kind_w = c
        .items
        .iter()
        .map(|i| item_kind_label(i.kind).chars().count())
        .max()
        .unwrap_or(0) as f32
        * char_width();
    let width = label_w + kind_w + 26.0;
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
    // the scroll window is persisted state on the completion (the
    // selection works its way to an edge before the window moves);
    // clamp defensively against list changes between frames
    let first_row = c.first_row.min(c.items.len().saturating_sub(visible));
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
        // these fixtures exercise insert-mode popups/carets: opt into vim
        // (the shipped default is Regular) so the 'i' enters insert
        app.config.vim = true;
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
    fn alt_right_selects_the_word_at_or_after_the_caret() {
        // the alt+arrow word-select semantics: the caret lands ON the
        // word's last char (vim `e`), and the band counts that char as
        // covered — the band shows exactly the word at/after the caret.
        let mut app = app();
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(0, 0));
        app.editor_mut().buf.start_selection();
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::WordEnd);
        let (start, end) = app
            .editor()
            .buf
            .selection_draw_range()
            .expect("a selection exists");
        let line = app.editor().buf.line(0);
        let covered: String = line[start.col..end.col].to_string();
        assert_eq!(covered, "Project", "the word at the caret, whole");
    }

    #[test]
    fn the_caret_sits_after_the_last_covered_char() {
        // the regression: with a selection the caret rendered at the
        // cursor cell's LEFT edge while the band counted that char as
        // covered — the caret sat one char before the last highlighted
        // char. It must sit at the band's right edge.
        let mut app = app();
        // anchor at (0,3), then select forward: the thin-caret band is
        // EXCLUSIVE up to the caret — the caret sits ON the cursor cell
        // (the insertion boundary) and the band's right edge meets it
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(0, 3));
        app.editor_mut().buf.start_selection();
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(0, 5));
        let cursor = app.editor().buf.pos();
        let x_caret = caret_x(&app, 0.0, 0.0);
        assert_eq!(
            x_caret,
            pos_to_px(cursor, 0.0, 0.0).x,
            "the caret rides the cursor cell (the insertion boundary)"
        );
        // the band's right edge meets the caret: cols 3..5 exclusive
        let (_, end) = app.editor().buf.selection_draw_range_exclusive().unwrap();
        assert_eq!(end.col, 5);
        assert_eq!(pos_to_px(end, 0.0, 0.0).x, x_caret);
        // a reversed selection (cursor on the LOW side of the anchor):
        // the caret rides the band's left edge
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(0, 1));
        let cursor = app.editor().buf.pos();
        assert_eq!(
            caret_x(&app, 0.0, 0.0),
            pos_to_px(cursor, 0.0, 0.0).x,
            "the low-side caret rides the band's left edge"
        );
        // at the insert column the band clamps to the line's text: no
        // overshoot into the empty space past it
        let len = app.editor().buf.line_len(0);
        app.editor_mut()
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(0, len));
        let cursor = app.editor().buf.pos();
        assert_eq!(
            caret_x(&app, 0.0, 0.0),
            pos_to_px(cursor, 0.0, 0.0).x,
            "the insert column's caret is already at the band's edge"
        );
    }

    #[test]
    fn selection_bands_tile_without_gaps() {
        // the regression: the per-line bands were 0.9 x LINE_HEIGHT — a
        // 10% gap after the start line (and after every V-linewise row).
        // Multi-line bands must tile seamlessly: each band's bottom is
        // the next band's top.
        let rects = selection_band_rects(
            Pos { row: 3, col: 4 },
            Pos { row: 7, col: 9 },
            0.0,
            0.0,
            800.0,
        );
        // start band + full-row block + tail
        assert_eq!(rects.len(), 3);
        let (head, mid, tail) = (rects[0], rects[1], rects[2]);
        // the start band is FULL line height and meets the block
        assert_eq!(head.height, LINE_HEIGHT);
        assert_eq!(head.y + head.height, mid.y, "no gap after the start line");
        // the block covers the middle rows at full height
        assert_eq!(mid.height, 3.0 * LINE_HEIGHT);
        assert_eq!(mid.y + mid.height, tail.y, "no gap before the tail");
        // and the tail is full height too
        assert_eq!(tail.height, LINE_HEIGHT);
    }

    #[test]
    fn hovering_a_table_name_shows_its_columns() {
        let app = app();
        // last line: "Ref fk_post_comment: posts.id - comments.id" —
        // hover "posts"
        let row = app.editor().buf.line_count() - 1;
        let line = app.editor().buf.line(row);
        let col = line.find("posts").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("doc");
        assert_eq!(doc.title, "Table posts");
        assert!(doc.rows.iter().any(|r| r.text.contains("user_id")));
        assert!(
            doc.rows.iter().any(|r| r.pk && r.has_icon()),
            "pk marker"
        );
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
    fn hovering_a_referenced_column_shows_its_definition() {
        // both sides of a Ref: hovering the column token shows THAT
        // column's definition, resolved through the dot-bound qualifier
        let app = App::with_source(
            "Table users as U {\n  id integer [pk]\n  name varchar\n}\n\nTable posts {\n  id integer [pk]\n  user_id integer [not null]\n}\n\nRef: posts.user_id > U.id\n"
                .to_string(),
        );
        let row = app.editor().buf.line_count() - 1;
        let line = app.editor().buf.line(row);
        // the referenced side: U.id -> users.id (by alias) — hovering
        // ANY char of the column name, not just the first
        let name_start = line.find("U.").unwrap() + 2; // the `id` after the dot
        for col in [name_start, name_start + 1] {
            let p = pos_to_px(
                crate::editor::buffer::Pos { row, col },
                app.cur().editor_scroll,
                app.cur().editor_hscroll,
            );
            let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("column doc");
            assert_eq!(doc.title, "users.id");
            assert_eq!(doc.rows.len(), 1);
            assert!(doc.rows[0].pk, "users.id is the pk");
        }
        // the fk side: posts.user_id -> posts's own column
        let col = line.find("user_id").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("column doc");
        assert_eq!(doc.title, "posts.user_id");
        assert_eq!(
            doc.rows[0].not_null,
            Some(true),
            "user_id carries its own [not null]"
        );
    }

    #[test]
    fn hovering_a_bare_table_token_still_shows_the_table_doc() {
        // the column-reference branch must not hijack tokens without a
        // dot-bound qualifier (the alias `U` keeps its table doc)
        let app = App::with_source(
            "Table users as U {\n  id integer [pk]\n}\n\nTable posts {\n  id integer [pk]\n  user_id integer\n}\n\nRef: posts.user_id > U.id\n"
                .to_string(),
        );
        let row = app.editor().buf.line_count() - 1;
        let line = app.editor().buf.line(row);
        let col = line.find("U.").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("alias doc");
        assert_eq!(doc.title, "Table users");
    }

    #[test]
    fn hovering_an_alias_in_a_standalone_ref_shows_the_table_doc() {
        // the reported case: `Ref: posts.user_id > U.id` — U is users' alias
        let app = App::with_source(
            "Table users as U {\n  id integer [pk]\n}\n\nTable posts {\n  id integer [pk]\n  user_id integer\n}\n\nRef: posts.user_id > U.id\n"
                .to_string(),
        );
        let row = app.editor().buf.line_count() - 1;
        let line = app.editor().buf.line(row);
        let col = line.find("U.").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("alias doc in a Ref");
        assert_eq!(doc.title, "Table users");
        assert!(doc.rows.iter().any(|r| r.text.starts_with("id ")));
    }

    #[test]
    fn a_name_outranks_a_matching_alias_in_hover() {
        // `Table t1 as X` + a table literally named X: hovering X shows the
        // named table (exact name first, then alias)
        let app = App::with_source(
            "Table t1 as X {\n  id integer [pk]\n}\n\nTable X {\n  x integer [pk]\n}\n".to_string(),
        );
        let line = app.editor().buf.line(4);
        let col = line.find("X").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 4, col },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("doc");
        assert_eq!(doc.title, "Table X");
        assert!(
            doc.rows.iter().any(|r| r.text.starts_with("x ")),
            "the named table's column, not t1's"
        );
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
        assert_eq!(doc.rows[0].text, "draft");
        // enum members carry no embellishments
        assert!(
            doc.rows.iter().all(|r| !r.has_icon() && !r.enum_ty),
            "plain rows: {:?}",
            doc.rows
        );
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
                .map(|i| DocRow::plain(format!("col_{i} varchar(255)")))
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
    fn diagnostics_reach_the_marks_with_severity() {
        // dangling ref: parses fine (non-blocking) but validates into a
        // RED mark carrying dbdiagram's message — their parseError class
        // (pk keeps the nullable-FK lint out of this one)
        let app = App::with_source("Table a { id integer [pk] }\nRef: a.id > b.x\n".into());
        assert!(app.cur().parse_errors.is_empty());
        let marks = error_marks(&app);
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(!marks[0].warn);
        assert_eq!(marks[0].message, "Table 'b' does not exist in Schema 'public'");

        // strict-grammar recovery (comma index settings): their
        // validateNode class — a red mark, not soft-blue
        let app = App::with_source(
            "Table t {\n  id integer\n  indexes {\n    (id), unique\n  }\n}\n".into(),
        );
        let marks = error_marks(&app);
        assert_eq!(marks.len(), 1, "{marks:?}");
        assert!(!marks[0].warn);
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
        let hit = mark_hover_at(&app, &marks, p);
        assert!(
            hit.is_some(),
            "hover at {p:?} hits none of {marks:?}"
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
        assert!(mark_hover_at(&app, &marks, p).is_none(), "{pos:?}");
    }

    #[test]
    fn hovering_below_the_last_line_misses() {
        // gibberish after the last block: the error mark rides the final
        // line, and empty space under the buffer must not resolve onto
        // that line — px_to_pos clamps its row, so the mark hit-test, the
        // word lookup and the I-beam def cursor all used to fire there
        let app = App::with_source("Table users {\n  id integer [pk]\n}\ngibberish\n".into());
        assert!(!app.cur().parse_errors.is_empty(), "fixture is broken");
        let marks = error_marks(&app);
        assert_eq!(marks[0].row, 3, "the gibberish mark is on the last line");
        // two rows under the buffer, aligned with the marked token
        let p = pos_to_px(
            crate::editor::buffer::Pos {
                row: 6,
                col: marks[0].start,
            },
            app.cur().editor_scroll,
            app.cur().editor_hscroll,
        );
        assert!(mark_hover_at(&app, &marks, p).is_none());
        // the clamped word used to resolve too (the last line's token)
        assert!(word_at(&app, p).is_none());
        assert!(doc_hover_at(&app, p).is_none());
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

    #[test]
    fn doc_card_tries_another_spot_to_stay_in_the_pane() {
        let bounds = Rectangle::new(Point::new(0.0, 0.0), Size::new(500.0, 700.0));
        let doc = DocHover {
            title: "Table users".into(),
            rows: vec![DocRow::plain("id integer".into())],
        };
        // roomy pane: below-right of the pointer wins
        let r = doc_rect(Point::new(100.0, 100.0), &doc, bounds);
        assert_eq!(r.x, 114.0);
        assert_eq!(r.y, 100.0 + LINE_HEIGHT + 4.0);
        // near the bottom: flips above, anchored just over the pointer line
        let r = doc_rect(Point::new(100.0, 660.0), &doc, bounds);
        assert!(r.y + r.height <= 700.0 - 4.0, "fits vertically: {r:?}");
        assert_eq!(r.y, 660.0 - r.height - 6.0);
        // near the right edge: flips left, right edge 14px from the pointer
        let r = doc_rect(Point::new(470.0, 100.0), &doc, bounds);
        assert!(r.x + r.width <= 500.0 - 4.0, "fits horizontally: {r:?}");
        assert_eq!(r.x + r.width, 470.0 - 14.0);
        // near both edges: both axes flip
        let r = doc_rect(Point::new(470.0, 660.0), &doc, bounds);
        assert!(r.x + r.width <= 500.0 - 4.0 && r.y + r.height <= 700.0 - 4.0, "{r:?}");
        // a card taller than the pane fits nowhere: pinned at the top
        let big = DocHover {
            title: "Table users".into(),
            rows: (0..60)
                .map(|i| DocRow::plain(format!("col{i} integer")))
                .collect(),
        };
        let r = doc_rect(Point::new(100.0, 350.0), &big, bounds);
        assert_eq!(r.y, 4.0, "pinned at the pane top: {r:?}");
        assert!(r.x >= 4.0 && r.x + r.width <= 500.0 - 4.0, "{r:?}");
    }
}

#[cfg(test)]
mod popup_tests {
    use super::*;
    use crate::app::{update, App};

    const PANE: Rectangle = Rectangle::new(Point::new(0.0, 0.0), Size::new(400.0, 300.0));

    fn app_with_caret(row: usize, col: usize) -> App {
        let mut app = App::default();
        app.config.vim = true;
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
        app.config.vim = true;
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
        // long labels to force a wide popup (label + kind column)
        if let Some(c) = app.completion.as_mut() {
            c.items.insert(
                0,
                crate::editor::completion::Item {
                    label: "x".repeat(70),
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
        // document words read as plain text, like Monaco's Word kind
        ItemKind::Word => theme::DIM,
    }
}

/// The kind column's label, nvim-cmp style (kind name right of the label).
fn item_kind_label(kind: crate::editor::completion::ItemKind) -> &'static str {
    use crate::editor::completion::ItemKind;
    match kind {
        ItemKind::Keyword => "Keyword",
        ItemKind::Table => "Table",
        ItemKind::Enum => "Enum",
        ItemKind::Column => "Column",
        ItemKind::EnumValue => "Value",
        ItemKind::Word => "Word",
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
    fn px_to_pos_at_a_fractional_scroll_hits_the_clicked_row() {
        // regression: at the bottom clamp editor_scroll is FRACTIONAL
        // (line_count - fractional page_lines); flooring the click line and
        // the scroll separately then lost a line whenever the fractions
        // summed past 1 — the cursor landed one row ABOVE the click
        let mut app = App::with_source(
            (0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"),
        );
        app.cur_mut().editor_scroll = 5.4; // fractional, like max_scroll
        // with scroll=5.4 the on-screen cells sit at [row-5.4, row-4.4):
        // row 9's cell spans [3.6, 4.6)*LINE_HEIGHT. Its TOP part (screen
        // line 3, fraction >= 0.6) is the zone that used to lose a line —
        // a click on row 9's text landed on row 8.
        let pos = px_to_pos(&app, Point::new(GUTTER + 2.0 * char_width(), 3.7 * LINE_HEIGHT));
        assert_eq!(pos.row, 9, "top of row 9's cell");
        // a mid-cell click was already fine
        let pos = px_to_pos(&app, Point::new(GUTTER + 2.0 * char_width(), 4.4 * LINE_HEIGHT));
        assert_eq!(pos.row, 9, "middle of row 9's cell");
        // and row 10's cell ([4.6, 5.6)) near its top — same lost line
        let pos = px_to_pos(&app, Point::new(GUTTER + 2.0 * char_width(), 4.65 * LINE_HEIGHT));
        assert_eq!(pos.row, 10, "top of row 10's cell");
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
    fn click_past_eol_maps_to_the_insert_column() {
        // "hello" (len 5): the last char's own cell floors to col 4; every
        // x past it — one cell or far beyond — clamps to col 5, so the
        // cursor visually snaps to the line's last available position
        let app = App::with_source("hello\n\nworld".into());
        let y = 10.0; // inside row 0
        let last_char = px_to_pos(&app, Point::new(GUTTER + 4.5 * char_width(), y));
        assert_eq!(last_char, Pos { row: 0, col: 4 });
        for x in [5.01, 5.5, 100.0, 1000.0] {
            let pos = px_to_pos(&app, Point::new(GUTTER + x * char_width(), y));
            assert_eq!(pos, Pos { row: 0, col: 5 }, "click {x} cells past the gutter");
        }
    }

    #[test]
    fn selection_bands_never_cover_cells_past_eol() {
        let w = char_width();
        // same row, ending on the insert column (draw end col 5 = the
        // clamped exclusive end on "hello"): exactly cols 2..5 — through
        // the last char, no empty trailing cell
        let rects = selection_band_rects(
            Pos { row: 0, col: 2 },
            Pos { row: 0, col: 5 },
            0.0,
            0.0,
            400.0,
        );
        assert_eq!(rects.len(), 1);
        assert!((rects[0].x - (GUTTER + 2.0 * w)).abs() < 1e-4);
        assert!((rects[0].width - 3.0 * w).abs() < 1e-4, "3 cells wide");

        // press-hold ON the insert column: an empty span paints nothing
        let rects = selection_band_rects(
            Pos { row: 0, col: 5 },
            Pos { row: 0, col: 5 },
            0.0,
            0.0,
            400.0,
        );
        assert!(rects.is_empty(), "a click past EOL must not highlight");

        // multi-line: start row and the one mid row band full-width, the
        // end row bands only its covered cells (cols 0..4)
        let rects = selection_band_rects(
            Pos { row: 0, col: 1 },
            Pos { row: 2, col: 4 },
            0.0,
            0.0,
            400.0,
        );
        assert_eq!(rects.len(), 3, "start row, mid rows, end row");
        assert!((rects[0].y).abs() < 1e-4);
        assert!(
            (rects[0].width - (400.0 - (GUTTER + w))).abs() < 1e-4,
            "start row: from col 1 to the pane edge"
        );
        assert!(
            (rects[1].y - LINE_HEIGHT).abs() < 1e-4 && (rects[1].height - LINE_HEIGHT).abs() < 1e-4,
            "exactly one full mid row"
        );
        assert!((rects[2].width - 4.0 * w).abs() < 1e-4, "end row: cols 0..4");

        // an empty end line (draw end col 0): nothing bands on it
        let rects = selection_band_rects(
            Pos { row: 0, col: 1 },
            Pos { row: 1, col: 0 },
            0.0,
            0.0,
            400.0,
        );
        assert_eq!(rects.len(), 1, "only the start row bands");

        // an end-row tail scrolled left of the gutter paints nothing (the
        // old .max(w) forced a cell at the gutter's edge)
        let hs = GUTTER + 2.0 * w;
        let rects = selection_band_rects(
            Pos { row: 0, col: 0 },
            Pos { row: 1, col: 1 },
            0.0,
            hs,
            400.0,
        );
        assert_eq!(rects.len(), 1, "end-row cells are scrolled off-view");
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
            // Lines are notches: 2 notches x 0.6em char_width in px
            (mouse::ScrollDelta::Lines { x: 2.0, y: 0.0 }, -2.0 * char_width()),
            // Pixels deltas pass through 1:1
            (mouse::ScrollDelta::Pixels { x: 80.0, y: 0.0 }, -80.0),
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
        app.config.vim = true;
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
        // the shipped default is Regular: a thin caret in every mode
        assert!(!is_block_cursor(&app), "regular default: thin caret");
        app.config.vim = true;
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
        app.config.vim = true;
        assert!(block_cursor_blinking(&app), "vim normal, focused, open");

        app.editor_mut().mode = Mode::Insert;
        assert!(!block_cursor_blinking(&app));
        app.editor_mut().mode = Mode::Normal;

        app.focus = Focus::Canvas;
        assert!(!block_cursor_blinking(&app));
        app.focus = Focus::Editor;

        app.maximized = Maximized::Diagram;
        assert!(!block_cursor_blinking(&app), "pane is off screen");
        app.maximized = Maximized::None;

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
                if let Some(m) = mark_hover_at(app, &marks, hover) {
                    let (rect, lines) = tooltip_layout(hover, &m.message, bounds);
                    frame.fill(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(4.0),
                        ),
                        theme::FLOAT_BG,
                    );
                    frame.stroke(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(4.0),
                        ),
                        canvas::Stroke {
                            style: (if m.warn { theme::WARN } else { theme::ERR }).into(),
                            width: 1.0,
                            ..Default::default()
                        },
                    );
                    // each line centers in its 18px slot: the old
                    // Top-aligned +12 offset put the single-line message's
                    // ink ~14px from the top and ~3px from the bottom
                    for (i, line) in lines.iter().enumerate() {
                        frame.fill_text(Text {
                            content: line.clone(),
                            position: Point::new(
                                rect.x + 12.0,
                                rect.y + 6.0 + (i as f32 + 0.5) * 18.0,
                            ),
                            color: theme::TEXT,
                            size: (FONT_SIZE - 1.0).into(),
                            font: theme::mono(),
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Center,
                            ..Default::default()
                        });
                    }
                } else if let Some(doc) = doc_hover_at(app, hover) {
                    let rect = doc_rect(hover, &doc, bounds);
                    // the diagram popups' card family: #303137, hairline
                    // border, rounded 8, a two-fill fake shadow (the canvas
                    // has no blur) — and dbml syntax coloring on the
                    // content, the same palette the buffer highlights with
                    for (off, alpha) in [(3.0, 0.16), (6.0, 0.10)] {
                        frame.fill(
                            &canvas::Path::rounded_rectangle(
                                Point::new(rect.x - 2.0, rect.y - 2.0 + off),
                                Size::new(rect.width + 4.0, rect.height + 4.0),
                                iced::border::Radius::from(8.0),
                            ),
                            Color::from_rgba(0.0, 0.0, 0.0, alpha),
                        );
                    }
                    frame.fill(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(8.0),
                        ),
                        Color::from_rgba8(0x30, 0x31, 0x37, 0.98),
                    );
                    frame.stroke(
                        &canvas::Path::rounded_rectangle(
                            rect.position(),
                            rect.size(),
                            iced::border::Radius::from(8.0),
                        ),
                        canvas::Stroke {
                            style: Color::from_rgba(1.0, 1.0, 1.0, 0.1).into(),
                            width: 1.0,
                            ..Default::default()
                        },
                    );
                    draw_doc_title(
                        &mut frame,
                        &doc.title,
                        Point::new(rect.x + 12.0, rect.y + 8.0),
                    );
                    // the card's hairline under the title
                    frame.fill_rectangle(
                        Point::new(rect.x + 1.0, rect.y + 26.0),
                        Size::new(rect.width - 2.0, 1.0),
                        Color::from_rgba(1.0, 1.0, 1.0, 0.1),
                    );
                    // per-row: icon slot (pk/fk/unique glyphs from the
                    // diagram), name + type, setting pills, default value
                    let icon_x = rect.x + 12.0;
                    let col_x = icon_x + 16.0;
                    let cw = (FONT_SIZE - 1.0) * CHAR_RATIO;
                    // an enum doc's values read in the enum blue, the
                    // popups' semantic
                    let row_ink = if doc.title.starts_with("Enum ") {
                        crate::canvas::diagram::ENUM_VALUE_BLUE
                    } else {
                        theme::TEXT
                    };
                    for (i, row) in doc.rows.iter().enumerate() {
                        let ry = rect.y + 32.0 + i as f32 * 16.0;
                        // the icon slot: the diagram's pk/fk/unique glyphs,
                        // blank when the column has none
                        if row.has_icon() {
                            let icon = if row.pk {
                                dbml_ui::layout::RowIcon::Pk
                            } else if row.fk {
                                dbml_ui::layout::RowIcon::Fk
                            } else {
                                dbml_ui::layout::RowIcon::Unique
                            };
                            crate::canvas::diagram::svg_icon(
                                &mut frame,
                                Point::new(icon_x + 6.0, ry + 6.0),
                                10.0,
                                10.5,
                                theme::MARKER,
                                dbml_ui::layout::row_icon_path(icon),
                            );
                        }
                        // "name  type": name in the text color, type dimmed
                        let (name, ty) = match row.text.split_once("  ") {
                            Some((n, t)) => (n, Some(t)),
                            None => (row.text.as_str(), None),
                        };
                        frame.fill_text(Text {
                            content: name.to_string(),
                            position: Point::new(col_x, ry),
                            color: row_ink,
                            size: (FONT_SIZE - 1.0).into(),
                            font: theme::mono(),
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            ..Default::default()
                        });
                        let mut px = col_x + (name.chars().count() as f32 + 2.0) * cw;
                        if let Some(ty) = ty {
                            // enum types read in the enum blue (the diagram
                            // + popups' semantic), plain types in the
                            // editor's type orange
                            let ty_color = if row.enum_ty {
                                crate::canvas::diagram::ENUM_VALUE_BLUE
                            } else {
                                theme::SYN_TYPE
                            };
                            frame.fill_text(Text {
                                content: ty.to_string(),
                                position: Point::new(px, ry),
                                color: ty_color,
                                size: (FONT_SIZE - 1.0).into(),
                                font: theme::mono(),
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Top,
                                ..Default::default()
                            });
                            px += (ty.chars().count() as f32 + 2.0) * cw;
                        }
                        // setting pills: bg pill + centered label, like the
                        // diagram's Not null / Enum badges
                        for (label, on) in doc_row_pills(row) {
                            if !on {
                                continue;
                            }
                            let pw = label.chars().count() as f32 * 5.4 + 10.0;
                            // the pill carries the optical centering, not
                            // the label: Vertical::Center centers the
                            // LINE-HEIGHT box, which for these descender-
                            // less labels puts the ink ~0.1-0.2px above the
                            // pill's geometric center (pixel-measured via
                            // AA edge-crossings on the rendered doc) — but
                            // the crisp solid-row gaps read MORE lopsided
                            // (1px above the glyph band vs 2px below).
                            // The label itself can't take the correction:
                            // glyph rasterization snaps vertically to
                            // whole pixels (a 0.35px text nudge rendered
                            // bit-identical), while this fill is a mesh
                            // that rasterizes at full precision. A 0.2px
                            // pill shift balances both measures.
                            let pill = canvas::Path::rounded_rectangle(
                                Point::new(px, ry + 1.30),
                                Size::new(pw, 12.0),
                                iced::border::Radius::from(3.0),
                            );
                            frame.fill(&pill, theme::FIELD_SETTING_BG);
                            frame.fill_text(Text {
                                content: label.to_string(),
                                position: Point::new(px + pw / 2.0, ry + 7.5),
                                color: theme::FIELD_SETTING_TEXT,
                                size: 9.0.into(),
                                horizontal_alignment: iced::alignment::Horizontal::Center,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                            px += pw + 4.0;
                        }
                        // the default value after the pills, colored as
                        // its dbml tokens: the keyword label, the string
                        if let Some(d) = &row.default {
                            frame.fill_text(Text {
                                content: "default:".to_string(),
                                position: Point::new(px, ry),
                                color: theme::SYN_KEYWORD,
                                size: (FONT_SIZE - 2.0).into(),
                                font: theme::mono(),
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Top,
                                ..Default::default()
                            });
                            frame.fill_text(Text {
                                content: format!(" {d}"),
                                position: Point::new(
                                    px + 8.0 * ((FONT_SIZE - 2.0) * CHAR_RATIO),
                                    ry,
                                ),
                                color: theme::SYN_STRING,
                                size: (FONT_SIZE - 2.0).into(),
                                font: theme::mono(),
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Top,
                                ..Default::default()
                            });
                        }
                    }
                }
            }
        }

        if let Some((prect, first_row, visible)) = popup_layout(app, bounds) {
            let c = app.completion.as_ref().unwrap();
            // nvim-cmp menu: float surface + quiet border, a square
            // selection band, the kind name in a right-hand column, and a
            // thin scrollbar when the list overflows
            frame.fill(
                &canvas::Path::rounded_rectangle(
                    prect.position(),
                    prect.size(),
                    iced::border::Radius::from(4.0),
                ),
                theme::FLOAT_BG,
            );
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    prect.position(),
                    prect.size(),
                    iced::border::Radius::from(4.0),
                ),
                canvas::Stroke {
                    style: theme::FLOAT_BORDER.into(),
                    width: 1.0,
                    ..Default::default()
                },
            );
            for (i, item) in c.items[first_row..first_row + visible].iter().enumerate() {
                let sel = first_row + i == c.selected;
                let ry = prect.y + i as f32 * ROW_H;
                if sel {
                    frame.fill_rectangle(
                        Point::new(prect.x + 1.0, ry),
                        Size::new(prect.width - 2.0, ROW_H),
                        Color {
                            a: 0.32,
                            ..theme::EDGE_HOVER
                        },
                    );
                }
                frame.fill_text(Text {
                    content: item.label.clone(),
                    position: Point::new(prect.x + 8.0, ry + ROW_H * 0.5),
                    color: theme::TEXT,
                    size: (FONT_SIZE - 1.0).into(),
                    font: theme::mono(),
                    horizontal_alignment: iced::alignment::Horizontal::Left,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
                frame.fill_text(Text {
                    content: item_kind_label(item.kind).to_string(),
                    position: Point::new(prect.x + prect.width - 8.0, ry + ROW_H * 0.5),
                    color: item_kind_color(item.kind),
                    size: (FONT_SIZE - 1.0).into(),
                    font: theme::mono(),
                    horizontal_alignment: iced::alignment::Horizontal::Right,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
            }
            // 2px thumb (dbdiagram's own webkit scrollbar width) tracking
            // the visible window into the filtered list
            if c.items.len() > visible {
                let list_h = visible as f32 * ROW_H;
                let thumb_h = (list_h * visible as f32 / c.items.len() as f32).max(8.0);
                let frac = first_row as f32 / (c.items.len() - visible) as f32;
                let y = prect.y + 3.0 + (list_h - 6.0 - thumb_h) * frac;
                frame.fill_rectangle(
                    Point::new(prect.x + prect.width - 3.0, y),
                    Size::new(2.0, thumb_h),
                    Color { a: 0.5, ..theme::DIM },
                );
            }
        }
        vec![frame.into_geometry()]
}}
