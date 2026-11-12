//! dbdiagram's sticky notes: `Note name { '''text''' }` rendered as the
//! classic yellow stickies (soft shadow, rounded corners, dark gray
//! wrapped text), floating above the tables in z-order and auto-placed
//! in a row below the diagram content. Agent-owned file.

use dbml_ui::layout::wrap_chars;
use crate::canvas::diagram::{cell_size, to_px, visible_region};
use dbml_ui::layout::{Layout, NoteBox};
use iced::widget::canvas::{self, Path, Text};
use iced::{alignment, Color, Font, Point, Rectangle, Size, Vector};

/// The classic yellow sticky fill.
const STICKY: Color = Color::from_rgb(1.0, 0.953, 0.690);
/// The note text: dark gray on yellow (#333333).
const NOTE_TEXT: Color = Color::from_rgb(0.2, 0.2, 0.2);
/// Corner radius, in 1:1 px (scaled by zoom / 20).
const RADIUS_PX: f32 = 6.0;
/// The soft shadow's down-right offset, in 1:1 px (scaled by zoom / 20).
const SHADOW_PX: f32 = 2.0;

/// The wrap width (chars per display row) a sticky `w` cell-widths wide
/// lays its text out with: 0.6-cell chars between two 0.4-cell pads. The
/// draw pass wraps through this so a resized sticky's rows break at the
/// same columns everywhere.
pub fn wrap_cols_for_width(w_cells: f32) -> usize {
    (((w_cells - 2.0 * 0.4) / 0.6).floor() as usize).max(4)
}

/// The sticky's corner-resize handle: a SQUARE of `2 * HANDLE_HALF` px
/// (at 1:1 zoom; scaled by k) — cell units would render a narrow
/// rectangle (a cell is twice as tall as wide) that is hard to aim at.
const HANDLE_HALF: f32 = 6.0;
/// The grab zone's margin beyond the drawn square, same units: the hit
/// test is the drawn handle plus this, so landing "on" the visible
/// square always catches. Kept at a hair's width — the cursor must not
/// advertise a handle that is not on screen.
const HANDLE_HIT_MARGIN: f32 = 1.0;

/// The grab envelope of a note's corners in frame px: how far the
/// resize zones reach beyond the drawn handle square (half its side,
/// plus the hair of margin). The hover chrome (handles) uses the same
/// envelope, so the handles are on screen exactly when the cursor can
/// be in one of their zones.
fn corner_hit_px(zoom: f32) -> f32 {
    (HANDLE_HALF + HANDLE_HIT_MARGIN) * (zoom / 20.0)
}

/// The sticky note under a canvas point, if any: stickies float above
/// the tables, so their press drags the note (the app routes before the
/// table hit-test).
pub fn note_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: iced::Point,
) -> Option<&NoteBox> {
    // doc order reversed: the schema-last sticky containing the point —
    // NOT raise order (hit_topmost layers that on top)
    layout
        .notes
        .iter()
        .rev()
        .find(|n| note_body_contains(n, zoom, pan, p))
}

/// Whether `p` sits inside THIS sticky's body rect — a per-note test, so
/// topmost hit resolution can't be hijacked by a different note that a
/// doc-order scan would return instead (two stacked stickies clicked
/// through to the lower one exactly that way).
pub fn note_body_contains(note: &NoteBox, zoom: f32, pan: Vector, p: iced::Point) -> bool {
    let s = crate::canvas::diagram::cell_size(zoom);
    let tl = crate::canvas::diagram::to_px(note.rect.0, note.rect.1, zoom, pan);
    p.x >= tl.x
        && p.x < tl.x + note.rect.2 as f32 * s.width
        && p.y >= tl.y
        && p.y < tl.y + note.rect.3 as f32 * s.height
}

/// Whether `p` sits in THIS sticky's corner-resize envelope: within the
/// grab square of one of its four corners (the zone straddles the
/// corner — the handle square is drawn half OUTSIDE the sticky, so the
/// exact edge pixels grab too).
pub fn note_corner_zone_contains(note: &NoteBox, zoom: f32, pan: Vector, p: iced::Point) -> bool {
    let s = crate::canvas::diagram::cell_size(zoom);
    let hit = corner_hit_px(zoom);
    let (x, y, w, h) = note.rect;
    let tl = crate::canvas::diagram::to_px(x, y, zoom, pan);
    let px = |cx: f32, cy: f32| iced::Point::new(tl.x + cx * s.width, tl.y + cy * s.height);
    [
        (0.0, 0.0),
        (w as f32, 0.0),
        (0.0, h as f32),
        (w as f32, h as f32),
    ]
    .into_iter()
    .any(|(cx, cy)| {
        let c = px(cx, cy);
        (p.x - c.x).abs() <= hit && (p.y - c.y).abs() <= hit
    })
}

/// The resize corner under a canvas point, if any: within the grab
/// square (the drawn handle plus its margin) of one of the note's four
/// corners — the zone straddles the corner, so it extends slightly
/// OUTSIDE the sticky too, like every resize handle. The corner enum is
/// the app's drag state.
pub fn note_corner_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: iced::Point,
) -> Option<(&NoteBox, crate::app::StickyCorner)> {
    let s = cell_size(zoom);
    let hit = corner_hit_px(zoom);
    // the grab zone straddles each corner (see note_corner_zone_contains):
    // handle square is drawn half OUTSIDE the sticky, and the exact edge
    // pixels must grab too
    let note = layout
        .notes
        .iter()
        .rev()
        .find(|n| note_corner_zone_contains(n, zoom, pan, p))?;
    let (x, y, w, h) = note.rect;
    let tl = crate::canvas::diagram::to_px(x, y, zoom, pan);
    let px = |cell_x: f32, cell_y: f32| {
        iced::Point::new(tl.x + cell_x * s.width, tl.y + cell_y * s.height)
    };
    let corners = [
        (px(0.0, 0.0), crate::app::StickyCorner::TopLeft),
        (px(w as f32, 0.0), crate::app::StickyCorner::TopRight),
        (px(0.0, h as f32), crate::app::StickyCorner::BottomLeft),
        (
            px(w as f32, h as f32),
            crate::app::StickyCorner::BottomRight,
        ),
    ];
    let (pt, corner) = corners
        .into_iter()
        .find(|(pt, _)| (p.x - pt.x).abs() <= hit && (p.y - pt.y).abs() <= hit)?;
    let _ = pt;
    Some((note, corner))
}

/// Whether THIS note may show its hover chrome (the corner drag
/// handles): the pointer must sit in its grab envelope AND the note must
/// be the TOPMOST item at the pointer — a sticky buried under a table or
/// under another sticky stays chrome-less while the pointer crosses its
/// hidden area, exactly like every other hover effect.
pub fn note_hover_chrome(
    note: &NoteBox,
    top_note: Option<&str>,
    pointer: Option<iced::Point>,
    zoom: f32,
    pan: Vector,
) -> bool {
    if top_note != Some(note.name.as_str()) {
        return false;
    }
    let s = crate::canvas::diagram::cell_size(zoom);
    let hit = corner_hit_px(zoom);
    pointer.is_some_and(|p| {
        let tl = crate::canvas::diagram::to_px(note.rect.0, note.rect.1, zoom, pan);
        let size = Size::new(
            note.rect.2 as f32 * s.width,
            note.rect.3 as f32 * s.height,
        );
        p.x >= tl.x - hit
            && p.x <= tl.x + size.width + hit
            && p.y >= tl.y - hit
            && p.y <= tl.y + size.height + hit
    })
}

/// Draw ONE sticky note: soft shadow, plain yellow body, wrapped text
/// and the hover/selection chrome. `occluders` is every object painted
/// after it (tables and stickies alike, in paint order) — the note's
/// TEXT clips to the visible slivers around all of them, because iced
/// draws all canvas text above all geometry and the fill covering alone
/// would leave the covered note's words visible. The mesh phase (body,
/// shadow, handles) never clips: whatever paints later covers it.
/// `top_note` names the topmost item at the pointer (None when a table
/// or open space is topmost): only that note draws its drag handles.
pub struct NoteChrome<'a> {
    /// The pointer, for hover chrome (None while a drag/modal owns it).
    pub pointer: Option<iced::Point>,
    /// The selected sticky's name (its selection border).
    pub selected: Option<&'a str>,
    /// The TOPMOST note at the pointer: only it draws drag handles.
    pub top_note: Option<&'a str>,
}

pub fn draw_note(
    frame: &mut canvas::Frame,
    note: &NoteBox,
    zoom: f32,
    pan: Vector,
    chrome: NoteChrome<'_>,
    occluders: &[Rectangle],
) {
    let k = zoom / 20.0;
    let s = cell_size(zoom);
    let line_h = 16.0 * zoom / 20.0;
    let (x, y, w, h) = note.rect;
    let top_left = to_px(x, y, zoom, pan);
    let size = Size::new(w as f32 * s.width, h as f32 * s.height);
    let radius = iced::border::Radius::from(RADIUS_PX * k);

    // soft drop shadow: a translucent rounded rect offset down-right,
    // drawn BEHIND the sticky
    frame.fill(
        &Path::rounded_rectangle(
            Point::new(top_left.x + SHADOW_PX * k, top_left.y + SHADOW_PX * k),
            size,
            radius,
        ),
        Color::from_rgba(0.0, 0.0, 0.0, 0.35),
    );
    // the sticky itself: plain yellow, no border
    frame.fill(&Path::rounded_rectangle(top_left, size, radius), STICKY);

    // the text only (dbdiagram never draws the note's name): left
    // aligned, starting near the top, one fill_text per wrapped line
    // vertically centered in its line slot (like the hover popups)
    let full = Rectangle::new(top_left, size);
    let occl: Vec<Rectangle> = occluders
        .iter()
        .filter(|r| r.intersects(&full))
        .copied()
        .collect();
    let wrap_cols = wrap_cols_for_width(w as f32);
    let draw_texts = |f: &mut canvas::Frame, at: Point| {
        let pad = 0.4 * s.width;
        let max_lines = ((size.height / line_h).floor() as usize).max(1);
        for (i, line) in note_lines(&note.text, wrap_cols, max_lines).iter().enumerate() {
            f.fill_text(Text {
                content: line.clone(),
                position: Point::new(at.x + pad, at.y + pad + (i as f32 + 0.5) * line_h),
                color: NOTE_TEXT,
                size: (0.6 * zoom).into(),
                font: Font::DEFAULT,
                horizontal_alignment: alignment::Horizontal::Left,
                vertical_alignment: alignment::Vertical::Center,
                ..Default::default()
            });
        }
    };
    if occl.is_empty() {
        draw_texts(frame, top_left);
    } else {
        for v in visible_region(full, &occl) {
            if v.width < 2.0 || v.height < 2.0 {
                continue;
            }
            frame.with_clip(v, |f| {
                // the draft frame's origin is the sliver's: draw the
                // note shifted back under it
                draw_texts(f, Point::new(top_left.x - v.x, top_left.y - v.y));
            });
        }
    }

    // resize handles at the four corners while hovered: small white
    // squares with a dark-gold outline, drawn last so they sit on top.
    // Hovered = "within the grab envelope" (the corner zones reach
    // `corner_hit_px` beyond the rect, and a resize cursor out there
    // must never point at chrome that isn't drawn) ON THE TOPMOST note
    // — a buried sticky draws no handles while the pointer crosses it
    let NoteChrome {
        pointer,
        selected,
        top_note,
    } = chrome;
    let hovered = note_hover_chrome(note, top_note, pointer, zoom, pan);
    // the edit chrome: a selection border (textbox look) on the
    // selected sticky — the editor caret lives in the source editor,
    // so nothing blinks here
    if selected.is_some_and(|name| name == note.name) {
        let sel = Path::rounded_rectangle(
            Point::new(top_left.x - 1.5, top_left.y - 1.5),
            Size::new(size.width + 3.0, size.height + 3.0),
            iced::border::Radius::from(RADIUS_PX * k + 1.5),
        );
        frame.stroke(
            &sel,
            canvas::Stroke {
                style: Color::from_rgb(0.95, 0.62, 0.10).into(),
                width: 2.0,
                ..Default::default()
            },
        );
    }
    if hovered {
        for (cx, cy) in [
            (0.0, 0.0),
            (w as f32, 0.0),
            (0.0, h as f32),
            (w as f32, h as f32),
        ] {
            let c = Point::new(top_left.x + cx * s.width, top_left.y + cy * s.height);
            // a SQUARE in px (the same geometry note_corner_hit grabs)
            let handle = Path::rectangle(
                Point::new(c.x - HANDLE_HALF * k, c.y - HANDLE_HALF * k),
                Size::new(2.0 * HANDLE_HALF * k, 2.0 * HANDLE_HALF * k),
            );
            frame.fill(&handle, Color::WHITE);
            frame.stroke(
                &handle,
                canvas::Stroke {
                    style: Color::from_rgb(0.55, 0.42, 0.05).into(),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }
    }
}

/// The note's text as the lines to draw: word-wrapped at `wrap` chars
/// (`wrap_chars`, which keeps explicit newlines as paragraph breaks),
/// capped at `max_lines` lines with a final `…` when anything was cut.
fn note_lines(text: &str, wrap: usize, max_lines: usize) -> Vec<String> {
    let mut lines = wrap_chars(text, wrap);
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        if let Some(last) = lines.last_mut() {
            last.push('…');
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_line() {
        assert_eq!(note_lines("hello", 22, 6), vec!["hello"]);
    }

    #[test]
    fn long_text_wraps_at_the_wrap_width() {
        // wrap_chars semantics: word-wrap at `wrap` chars
        assert_eq!(
            note_lines("TODO 1: fix the login flow now", 10, 10),
            vec!["TODO 1:", "fix the", "login flow", "now"]
        );
    }

    #[test]
    fn explicit_newlines_are_honored() {
        assert_eq!(
            note_lines("TODO 1: x\nTODO 2: y", 22, 6),
            vec!["TODO 1: x", "TODO 2: y"]
        );
        // empty paragraphs survive as blank lines
        assert_eq!(note_lines("a\n\nb", 22, 6), vec!["a", "", "b"]);
    }

    #[test]
    fn overflow_truncates_with_an_ellipsis() {
        // wrap 8 -> ["one two", "three", "four", "five six", "seven"]
        let lines = note_lines("one two three four five six seven", 8, 3);
        assert_eq!(lines.len(), 3, "capped at max_lines");
        assert!(lines.last().unwrap().ends_with('…'), "final …: {lines:?}");
        // exactly-fitting text is not truncated
        assert_eq!(note_lines("one two three", 8, 3), vec!["one two", "three"]);
    }

    #[test]
    fn the_corner_grab_zone_straddles_the_drawn_handle() {
        let src = "Note todo { '''x'''\nmore'''\n}\nTable t { id integer }\n";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(0.0, 0.0);
        let (x, y, w, h) = l.notes[0].rect;
        let tl = to_px(x, y, zoom, pan);
        let s = cell_size(zoom);
        let br = Point::new(tl.x + w as f32 * s.width, tl.y + h as f32 * s.height);
        // the EXACT corner pixel grabs (note_hit's interior test used to
        // exclude the right/bottom edges, leaving the handle half dead)
        let grabbed = note_corner_hit(&l, zoom, pan, br).expect("the exact corner grabs");
        assert!(matches!(
            grabbed.1,
            crate::app::StickyCorner::BottomRight
        ));
        // the drawn square reaches 6px out; the zone, 9px: just outside
        // the sticky still grabs
        let outside = Point::new(br.x + 5.0, br.y + 5.0);
        assert!(note_corner_hit(&l, zoom, pan, outside).is_some());
        // past the zone: nothing (the note hit area ends 9px out)
        let far = Point::new(br.x + 20.0, br.y + 20.0);
        assert!(note_corner_hit(&l, zoom, pan, far).is_none());
        // the sticky's middle is a drag, not a resize
        let mid = Point::new(
            tl.x + w as f32 * s.width * 0.5,
            tl.y + h as f32 * s.height * 0.5,
        );
        assert!(note_corner_hit(&l, zoom, pan, mid).is_none());
    }

    #[test]
    fn only_the_topmost_sticky_grows_drag_handles() {
        // two piled stickies: hovering the pile shows handles on the
        // TOP one only — the buried one's envelope must not sprout
        // handles under the item covering it
        let mk = |name: &str, x: i32, y: i32| NoteBox {
            name: name.into(),
            text: "t".into(),
            rect: (x, y, 14, 5),
        };
        let notes = vec![mk("a", 5, 5), mk("b", 6, 6)];
        let layout = Layout {
            notes,
            ..Default::default()
        };
        let (zoom, pan) = (20.0, Vector::default());
        // on the pile, inside both envelopes: b is topmost (schema order)
        let p = crate::canvas::diagram::to_px(10, 8, zoom, pan);
        assert!(note_hover_chrome(
            &layout.notes[1],
            Some("b"),
            Some(p),
            zoom,
            pan
        ));
        assert!(!note_hover_chrome(
            &layout.notes[0],
            Some("b"),
            Some(p),
            zoom,
            pan
        ));
        // a raise flips it
        assert!(note_hover_chrome(
            &layout.notes[0],
            Some("a"),
            Some(p),
            zoom,
            pan
        ));
        // a table (or open space) topmost: no sticky gets handles
        assert!(!note_hover_chrome(
            &layout.notes[1],
            None,
            Some(p),
            zoom,
            pan
        ));
        // no pointer at all: nothing
        assert!(!note_hover_chrome(
            &layout.notes[1],
            Some("b"),
            None,
            zoom,
            pan
        ));
    }

    #[test]
    fn note_paint_follows_the_shared_raise_order() {
        // the raise order lives on the app side (item_z); the canvas
        // just consumes it — a raised sticky paints after every unraised
        // item, the dragged one last, stale entries skipped
        let mk = |name: &str| NoteBox {
            name: name.into(),
            text: "t".into(),
            rect: (0, 0, 14, 5),
        };
        let notes = vec![mk("a"), mk("b"), mk("c")];
        let layout = Layout {
            notes,
            ..Default::default()
        };
        use crate::app::RaisedItem;
        // grab c then a: the unraised one first, then the raised in
        // grab order (most recent wins)
        let z = vec![RaisedItem::Note("c".into()), RaisedItem::Note("a".into())];
        let ordered = crate::canvas::diagram::paint_items(&layout, &z, None, None);
        let names: Vec<&str> = ordered
            .iter()
            .filter_map(|item| match item {
                crate::canvas::diagram::PaintItem::Note(n) => Some(n.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["b", "c", "a"]);
        // the dragged note rides above the raised ones
        let ordered = crate::canvas::diagram::paint_items(&layout, &z, Some("b"), None);
        let names: Vec<&str> = ordered
            .iter()
            .filter_map(|item| match item {
                crate::canvas::diagram::PaintItem::Note(n) => Some(n.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["c", "a", "b"]);
        // a stale entry is skipped, no duplicates
        let z = vec![
            RaisedItem::Note("gone".into()),
            RaisedItem::Note("b".into()),
            RaisedItem::Note("b".into()),
        ];
        let ordered = crate::canvas::diagram::paint_items(&layout, &z, None, None);
        let names: Vec<&str> = ordered
            .iter()
            .filter_map(|item| match item {
                crate::canvas::diagram::PaintItem::Note(n) => Some(n.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["a", "c", "b"]);
    }

    #[test]
    fn layout_pipeline_places_notes_below_the_content() {
        let src = "Note todo { '''TODO 1: x\nTODO 2: y''' }\nTable t { id integer }\n";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        assert_eq!(l.notes.len(), 1);
        let n = &l.notes[0];
        assert_eq!(n.name, "todo");
        assert_eq!(n.text, "TODO 1: x\nTODO 2: y");
        assert_eq!(n.rect.2, 14, "the 14x5 sticky footprint");
    }
}
