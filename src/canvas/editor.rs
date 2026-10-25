//! The editor canvas: monospace code text with a gutter, a mode-styled
//! fake cursor, and visual-mode selection spans. Scrolling is a float row
//! offset kept in `App`; the canvas just draws the visible window.

use crate::app::{App, Message};
use crate::editor::buffer::Pos;
use crate::editor::Mode;
use crate::theme;
use iced::widget::canvas::{self, Text};
use iced::{Color, Font, Point, Rectangle, Size};

/// iced's default monospace (Fira Mono) advances exactly 0.6 em.
pub const CHAR_RATIO: f32 = 0.6;
pub const FONT_SIZE: f32 = 14.0;
pub const LINE_HEIGHT: f32 = 21.0;
pub const GUTTER: f32 = 56.0;

pub fn char_width() -> f32 {
    FONT_SIZE * CHAR_RATIO
}

/// Screen-space origin of a buffer position (local to the editor pane).
pub fn pos_to_px(p: Pos, scroll: f32) -> Point {
    Point::new(
        GUTTER + p.col as f32 * char_width(),
        // pixel-snapped so glyphs, cursor and marks always share one grid
        ((p.row as f32 - scroll) * LINE_HEIGHT).round(),
    )
}

/// Buffer position under a local point (clamped into the text).
pub fn px_to_pos(app: &App, p: Point) -> Pos {
    let lines = app.editor.buf.lines();
    let row = ((p.y / LINE_HEIGHT).floor() as usize)
        .saturating_add(app.editor_scroll as usize)
        .min(lines.len().saturating_sub(1));
    let max_col = lines.get(row).map(|l| l.chars().count()).unwrap_or(0);
    let col = if p.x <= GUTTER {
        0
    } else {
        // floor: the char under the pointer, also used for hit-testing
        (((p.x - GUTTER) / char_width()).floor() as usize).min(max_col)
    };
    Pos { row, col }
}

pub struct EditorView<'a> {
    pub app: &'a App,
}

impl canvas::Program<Message> for EditorView<'_> {
    type State = ();

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
                let notches = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                (
                    canvas::event::Status::Captured,
                    Some(Message::EditorScrolled(notches)),
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
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::PANEL);

        let app = self.app;
        let lines = app.editor.buf.lines();
        let scroll = app.editor_scroll;
        let visible = (bounds.height / LINE_HEIGHT).ceil() as usize + 1;
        let first = scroll.floor() as usize;
        let sub = scroll - first as f32;

        // selection spans (visual mode); V paints whole rows regardless
        // of column
        if app.editor.visual_linewise && app.editor.mode == crate::editor::Mode::Visual {
            if let Some(anchor) = app.editor.buf.selection_anchor() {
                let cursor = app.editor.buf.pos();
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
        } else if let Some((start, end)) = app.editor.buf.selection_range() {
            let top = pos_to_px(start, scroll);
            let bottom = pos_to_px(end, scroll);
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
        let src = app.source.as_str();
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
            frame.fill_text(Text {
                content: format!("{:>4}", i + 1),
                position: Point::new(GUTTER - 8.0, text_y),
                color: theme::DIM,
                size: (FONT_SIZE - 2.0).into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
            if !line.is_empty() {
                let ls = line_starts[i];
                let le = ls + line.len();
                // token chunks intersecting this line, drawn at their char
                // column on the same monospace grid the cursor math uses
                let first_tok = app.highlights.partition_point(|t| t.end <= ls);
                for t in &app.highlights[first_tok..] {
                    if t.start >= le {
                        break;
                    }
                    let s = t.start.max(ls);
                    let e = t.end.min(le);
                    let content = &src[s..e];
                    if content.chars().any(|c| !c.is_whitespace()) {
                        let col = src[ls..s].chars().count();
                        frame.fill_text(Text {
                            content: content.to_string(),
                            position: Point::new(GUTTER + col as f32 * char_width(), text_y),
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
            y += LINE_HEIGHT;
        }

        // fake cursor, styled by mode. Deliberately NOT hidden while a
        // scroll animation runs: neoscroll hides only the terminal's
        // hardware caret (the text still visibly scrolls), but here the
        // gliding block IS the animation — hiding it made fitting-document
        // scrolls look like an instant jump.
        let cursor = app.editor.buf.pos();
        let p = pos_to_px(cursor, scroll);
        if p.y > -LINE_HEIGHT && p.y < bounds.height + LINE_HEIGHT {
            let (color, filled) = match app.editor.mode {
                crate::editor::Mode::Insert => (theme::TEXT, false),
                _ => (theme::TEXT, true),
            };
            let rect = if filled {
                // block cursor
                frame.fill_rectangle(
                    p,
                    Size::new(char_width(), LINE_HEIGHT * 0.9),
                    Color::from_rgba(color.r, color.g, color.b, 0.45),
                );
                Size::new(2.0, LINE_HEIGHT * 0.9)
            } else {
                Size::new(2.0, LINE_HEIGHT * 0.9)
            };
            frame.fill_rectangle(p, rect, color);
        }

        // squiggly underlines under error tokens
        for m in &marks {
            let top_y = (m.row as f32 - scroll) * LINE_HEIGHT;
            if top_y < -LINE_HEIGHT || top_y > bounds.height {
                continue;
            }
            let y = top_y + LINE_HEIGHT * 0.85;
            let x0 = GUTTER + m.start as f32 * char_width();
            let x1 = (GUTTER + m.end as f32 * char_width()).max(x0 + 3.0);
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

        // overlays (error tooltip, hover docs, completion popup) render in
        // a SECOND geometry: within one geometry iced draws all meshes
        // before all text, so a popup in the main geometry left the code
        // text beneath it showing through its background — it read as a
        // transparent popup
        let mut overlay = canvas::Frame::new(renderer, bounds.size());
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
                    overlay.fill(
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
                        overlay.fill_text(Text {
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
                    overlay.fill(
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
                            style: theme::SYN_NAME.into(),
                            width: 1.0,
                            ..Default::default()
                        },
                    );
                    overlay.fill_text(Text {
                        content: doc.title.clone(),
                        position: Point::new(rect.x + 12.0, rect.y + 10.0),
                        color: theme::SYN_NAME,
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
                        overlay.fill_text(Text {
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
                        overlay.fill_text(Text {
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
            overlay.fill(
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
                overlay.fill_text(Text {
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
        vec![frame.into_geometry(), overlay.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        // canvas mouse_interaction is not bounds-gated by iced, so check
        // hover here: I-beam only while typing, plain arrow otherwise
        // (normal/visual motions are keyboard-driven)
        if cursor.position_over(bounds).is_some() && self.app.editor.mode == Mode::Insert {
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
    let src = app.source.as_str();
    let mut marks = Vec::new();
    for e in &app.parse_errors {
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
    let chars: Vec<char> = app.editor.buf.line(pos.row).chars().collect();
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
fn doc_hover_at(app: &App, p: Point) -> Option<DocHover> {
    use dbml_ui::model::NodeKind;
    let word = word_at(app, p)?;
    let b = app
        .diagram
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
    let caret = pos_to_px(app.editor.buf.pos(), app.editor_scroll);
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
        let row = app.editor.buf.line_count() - 1;
        let line = app.editor.buf.line(row);
        let col = line.find("users").unwrap();
        let p = pos_to_px(crate::editor::buffer::Pos { row, col }, app.editor_scroll);
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("doc");
        assert_eq!(doc.title, "Table users");
        assert!(doc.rows.iter().any(|(_, t)| t.contains("username")));
        assert!(doc.rows.iter().any(|(m, _)| m == "\u{25c6}"), "pk marker");
    }

    #[test]
    fn hovering_an_alias_shows_the_same_doc() {
        let app = app();
        // 0-based line 19: "user_id integer [not null, ref: > U.id]"
        let line = app.editor.buf.line(19);
        let col = line.find("U.").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 19, col },
            app.editor_scroll,
        );
        let doc = doc_hover_at(&app, p + Vector::new(3.0, 10.0)).expect("alias doc");
        assert_eq!(doc.title, "Table users");
    }

    #[test]
    fn hovering_an_enum_name_shows_values() {
        let app = app();
        // line 34: "Enum post_status {"
        let line = app.editor.buf.line(33);
        let col = line.find("post_status").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 33, col },
            app.editor_scroll,
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
        let line = app.editor.buf.line(6);
        let col = line.find("integer").unwrap();
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 6, col },
            app.editor_scroll,
        );
        assert!(doc_hover_at(&app, p + Vector::new(3.0, 10.0)).is_none());
        // hovering whitespace: no word at all
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: 6, col: 0 },
            app.editor_scroll,
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
        assert!(!app.parse_errors.is_empty(), "fixture is broken");
        let marks = error_marks(&app);
        assert!(!marks.is_empty());
        for m in &marks {
            assert!(m.end > m.start, "non-empty: {m:?}");
            let line_len = app.editor.buf.line(m.row).chars().count();
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
            app.editor_scroll,
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
        let other = (0..app.editor.buf.line_count())
            .find(|r| !marks.iter().any(|m| m.row == *r))
            .expect("fixture has non-error lines");
        let p = pos_to_px(
            crate::editor::buffer::Pos { row: other, col: 0 },
            app.editor_scroll,
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
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Jump(row, col));
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
        app.editor
            .buf
            .move_cursor(crate::editor::buffer::Move::Bottom);
        app.editor_scroll = app.editor.buf.line_count() as f32 - 20.0;
        let _ = update(
            &mut app,
            crate::app::Message::KeyPressed(
                crate::editor::KeyCode::Space,
                crate::editor::KeyModifiers::CONTROL,
            ),
        );
        assert!(app.completion.is_some(), "popup open");
        let (rect, _, _) = popup_layout(&app, PANE).unwrap();
        let caret = pos_to_px(app.editor.buf.pos(), app.editor_scroll);
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
        app.editor_scroll = 0.0;
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
        Kind::Comment | Kind::Punct => theme::DIM,
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
            &mut (),
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
            &mut (),
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
    fn wheel_outside_editor_does_not_scroll() {
        // regression: zooming the diagram scrolled the editor pane
        let app = App::default();
        let view = EditorView { app: &app };
        let (_, message) = view.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
            }),
            BOUNDS,
            Cursor::Available(Point::new(900.0, 100.0)),
        );
        assert!(message.is_none(), "{message:?}");
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
            app.editor.mode = mode;
            let view = EditorView { app: &app };
            assert_eq!(
                view.mouse_interaction(&(), BOUNDS, inside),
                expected,
                "{mode:?}"
            );
        }
        // outside the pane: no opinion even in insert mode
        app.editor.mode = Mode::Insert;
        let view = EditorView { app: &app };
        assert_eq!(
            view.mouse_interaction(&(), BOUNDS, Cursor::Available(Point::new(900.0, 100.0))),
            Interaction::default()
        );
    }
}
