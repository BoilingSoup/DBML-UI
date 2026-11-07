//! The diagram canvas: draws the laid-out schema as dbdiagram.io-style
//! table boxes — monochrome charcoal with soft drop shadows, near-black
//! header bands, gray outline key/link markers after each column name —
//! and gray curved relationship lines (side-anchored beziers) with
//! markers. Mirrors the SVG exporter's geometry, but sized by `zoom`
//! (pixels per diagram cell) so text stays crisp at every scale.
//!
//! Hover highlight — ported from dbdiagram's renderer (their Konva
//! sceneFunc, extracted from their JS bundle): the line eases to their
//! dark-theme highlight blue and doubles in width; a dashed overlay
//! (dash 3, gap 40 in 1:1 px) rides the SAME filleted path with an
//! animated dash offset (~33px/s, flowing referenced→foreign-key), over
//! a static glow; endpoint markers (tick at the "one" end, chevron +
//! circle at the "many" end) sit in a trimmed gap the dashes never
//! enter. No text. The highlight amount per edge is animated by the app
//! (`edge_anim`), so transitions ease in and out instead of popping.

use crate::app::RaisedItem;
use crate::theme;
use dbml_ui::layout::{many_ends, row_icon_path, wrap_chars, EdgeRoute, Layout, NoteBox, Row, RowIcon, NOTE_ICON, TableBox};
use iced::widget::canvas::{self, Path, Stroke, Text};
use iced::{font, Color, Font, Point, Rectangle, Size, Vector};

/// Diagram cells around the content (matches the SVG renderer's margin).
pub const MARGIN_CELLS: f32 = 1.0;

// Bold text runs wider than the advance classes `Row::est_width` was
// calibrated on — the factor lives in layout (`layout::BOLD_TEXT`) where
// the box width reserves the header's glyph room with the same measure
// the painter places glyphs with.
pub use dbml_ui::layout::BOLD_TEXT;

/// A diagram cell is half as wide as it is tall (mirrors the SVG unit
/// ratio 10x20); `zoom` is the cell height in pixels.
pub const CELL_ASPECT: f32 = 0.5;

/// One badge-font char, in cell widths (`Row::BADGE_CELL` is the same
/// figure on the layout side).
const BADGE_CHAR: f32 = 0.42;
/// The badge pill's horizontal padding, in cell widths.
const BADGE_PAD: f32 = 0.24;

/// Cell size in pixels at `zoom` (pixels per cell height).
pub fn cell_size(zoom: f32) -> Size {
    Size::new(zoom * CELL_ASPECT, zoom)
}

/// Top-left of a diagram cell in canvas pixels.
pub fn to_px(x: i32, y: i32, zoom: f32, pan: Vector) -> Point {
    let s = cell_size(zoom);
    Point::new(
        (x as f32 + MARGIN_CELLS) * s.width + pan.x,
        (y as f32 + MARGIN_CELLS) * s.height + pan.y,
    )
}

/// Inverse of `to_px`: diagram cell under a canvas point.
#[allow(dead_code)] // kept for callers that want cell granularity
pub fn to_cell(p: Point, zoom: f32, pan: Vector) -> (i32, i32) {
    let (fx, fy) = to_cell_f(p, zoom, pan);
    (fx.floor() as i32, fy.floor() as i32)
}

/// Fractional diagram cell under a canvas point (for grab offsets).
pub fn to_cell_f(p: Point, zoom: f32, pan: Vector) -> (f32, f32) {
    let s = cell_size(zoom);
    (
        (p.x - pan.x) / s.width - MARGIN_CELLS,
        (p.y - pan.y) / s.height - MARGIN_CELLS,
    )
}

/// Topmost table box containing the canvas point, if any.
pub fn hit_test(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<&TableBox> {
    let (fx, fy) = to_cell_f(p, zoom, pan);
    layout.boxes.iter().rev().find(|b| {
        fx >= b.x as f32 && fx < (b.x + b.w) as f32 && fy >= b.y as f32 && fy < (b.y + b.h) as f32
    })
}

/// The TOPMOST item at `p` in paint order: a raised table over a sticky
/// must win the press (the click belongs to what you see), and a sticky
/// over a table likewise. Mirrors `paint_items`' ranking — the raise
/// order decides, the static layering (stickies above untouched tables)
/// beneath it — so the cursor and the press routes agree with the render.
/// A sticky's hit area includes its corner resize envelope.
pub fn hit_topmost<'a>(
    layout: &'a Layout,
    zoom: f32,
    pan: Vector,
    p: Point,
    item_z: &[RaisedItem],
) -> Option<PaintItem<'a>> {
    paint_items(layout, item_z, None, None, None)
        .into_iter()
        .rev()
        .find(|item| match item {
            PaintItem::Table(b) => {
                // rect containment (not hit_test: that is doc-order and
                // would disagree with the raise order on overlaps)
                let tl = to_px(b.x, b.y, zoom, pan);
                let sz = cell_size(zoom);
                p.x >= tl.x
                    && p.x < tl.x + b.w as f32 * sz.width
                    && p.y >= tl.y
                    && p.y < tl.y + b.h as f32 * sz.height
            }
            PaintItem::Note(n) => {
                // per-note containment: a doc-order helper could return a
                // DIFFERENT note at the same point and make the true
                // topmost one skip itself (two stacked stickies clicked
                // through to the lower one exactly that way)
                super::notes::note_body_contains(n, zoom, pan, p)
                    || super::notes::note_corner_zone_contains(n, zoom, pan, p)
            }
        })
}

/// Monospace character advance for row text at `zoom` (mono glyphs are
/// ~0.6 em wide), used to find where a column name ends.
/// The name's end x in frame px (the row text is drawn at this size).
fn row_name_end_px(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> f32 {
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    // the shared per-character estimator (layout::Row::est_width): the old
    // uniform 0.58/char factor overshot narrow names like "author_id" and
    // left a dead gap before the row's first icon.
    // pk names draw BOLD (dbdiagram parity) — bold runs past the regular
    // estimate (the popup chrome carries the same 1.18 semibold factor),
    // and the estimate must clear the ink: "channel" put the pk glyph ON
    // the name. Overshooting only widens the gap, so 1.2 with headroom.
    let ems = Row::est_width(&b.rows[row].name) * if b.rows[row].pk { 1.2 } else { 1.0 };
    top_left.x + 1.0 * s.width + ems * s.width
}

/// The box's frame-px rect (the paint bounds, shadows excluded).
fn box_rect(b: &TableBox, zoom: f32, pan: Vector) -> Rectangle {
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    Rectangle::new(
        top_left,
        Size::new(b.w as f32 * s.width, b.h as f32 * s.height),
    )
}

/// A sticky's frame-px rect (the paint bounds, shadow excluded) — the
/// same bounds `draw_notes` fills.
fn note_rect_px(note: &NoteBox, zoom: f32, pan: Vector) -> Rectangle {
    let (x, y, w, h) = note.rect;
    let top_left = to_px(x, y, zoom, pan);
    let s = cell_size(zoom);
    Rectangle::new(
        top_left,
        Size::new(w as f32 * s.width, h as f32 * s.height),
    )
}

/// `rect` minus ALL `occluders`: the region a covered table still
/// draws its text in. Subtracts each occluder from the surviving set
/// (`visible_slivers` removes one rect) — with three stacked tables the
/// middle one must hide the bottom table's text too, not just the top
/// occluder, or the bottom text leaks through it. Capped: a
/// pathological stack stops subtracting rather than exploding.
pub(crate) fn visible_region(rect: Rectangle, occluders: &[Rectangle]) -> Vec<Rectangle> {
    let mut visible = vec![rect];
    for occ in occluders {
        if visible.len() >= 48 {
            break;
        }
        let mut next = Vec::new();
        for v in &visible {
            if v.intersects(occ) {
                next.extend(visible_slivers(*v, *occ));
            } else {
                next.push(*v);
            }
        }
        visible = next;
    }
    visible
}

/// The parts of `rect` NOT covered by `occ` (the occluder), as up to
/// four rectangles: left, right, top and bottom slivers around the
/// occluder — the visible region a covered table still draws in.
pub(crate) fn visible_slivers(rect: Rectangle, occ: Rectangle) -> Vec<Rectangle> {
    let rr = rect.x + rect.width;
    let rb = rect.y + rect.height;
    let or = occ.x + occ.width;
    let ob = occ.y + occ.height;
    let x_lo = rect.x.max(occ.x);
    let x_hi = rr.min(or);
    vec![
        // left of the occluder (full height, clamped into the rect)
        Rectangle::new(
            Point::new(rect.x, rect.y),
            Size::new((occ.x - rect.x).max(0.0).min(rect.width), rect.height),
        ),
        // right of the occluder (full height, clamped into the rect)
        Rectangle::new(
            Point::new(or.max(rect.x), rect.y),
            Size::new((rr - or).max(0.0).min(rect.width), rect.height),
        ),
        // above the occluder (the horizontal overlap band)
        Rectangle::new(
            Point::new(x_lo, rect.y),
            Size::new((x_hi - x_lo).max(0.0), (occ.y - rect.y).max(0.0)),
        ),
        // below the occluder (the horizontal overlap band)
        Rectangle::new(
            Point::new(x_lo, ob),
            Size::new((x_hi - x_lo).max(0.0), (rb - ob).max(0.0)),
        ),
    ]
    .into_iter()
    .filter(|s| s.width > 0.0 && s.height > 0.0)
    .collect()
}

/// The vertical center of a column row in frame px.
fn y_of(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> f32 {
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    top_left.y + (row as f32 + 1.0) * s.height + s.height * 0.5
}

fn row_icon_layout(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> Vec<(RowIcon, f32)> {
    let rowd = &b.rows[row];
    let k = zoom / 20.0;
    let mut u = row_name_end_px(b, row, zoom, pan) + 8.0 * k;
    let mut out = Vec::new();
    // every slot is a 10.5px glyph plus its 4px gap
    let push = |icon: RowIcon, u: &mut f32, out: &mut Vec<(RowIcon, f32)>| {
        out.push((icon, *u));
        *u += 14.5 * k;
    };
    if rowd.pk {
        push(RowIcon::Pk, &mut u, &mut out);
    }
    if rowd.fk {
        push(RowIcon::Fk, &mut u, &mut out);
    }
    if rowd.unique {
        push(RowIcon::Unique, &mut u, &mut out);
    }
    // the note icon is the "extra info on hover" affordance: ANY custom
    // property triggers it (audit_log.event's `classification` too) —
    // `note` is not a special custom-property key
    if rowd.note.is_some() || !rowd.custom.is_empty() {
        push(RowIcon::Note, &mut u, &mut out);
    }
    out
}

/// The EXACT icon of a row's left cluster under a canvas point, if any —
/// pk, fk, unique, or the note affordance (whatever `row_icon_layout`
/// draws). Reporting the glyph itself (not just its row) is what lets a
/// pk+fk row tell its two markers apart when hovered.
pub fn marker_icon_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: Point,
) -> Option<(&TableBox, usize, RowIcon)> {
    let (b, row) = hovered_row_of(layout, zoom, pan, p)?;
    let k = zoom / 20.0;
    for (icon, slot) in row_icon_layout(b, row, zoom, pan) {
        let center = Point::new(slot + 5.25 * k, y_of(b, row, zoom, pan));
        let r = 5.25 * k;
        if (p.x - center.x).abs() <= r && (p.y - center.y).abs() <= r {
            return Some((b, row, icon));
        }
    }
    None
}

/// The pk/fk marker under a canvas point, if any: the pointer must be
/// inside one of the row's pk/fk icon slots (the unique/note glyphs are
/// not markers).
#[allow(dead_code)] // the overlay now resolves the exact glyph; kept as the pk/fk-level hit test
pub fn marker_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    marker_icon_hit(layout, zoom, pan, p)
        .filter(|(_, _, icon)| matches!(icon, RowIcon::Pk | RowIcon::Fk))
        .map(|(b, row, _)| (b, row))
}

/// The note icon under a canvas point, if any.
#[allow(dead_code)] // the overlay now resolves the exact glyph; kept as the note-level hit test
pub fn note_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    marker_icon_hit(layout, zoom, pan, p)
        .filter(|(_, _, icon)| *icon == RowIcon::Note)
        .map(|(b, row, _)| (b, row))
}

/// X range (frame px) of an enum-typed row's "Enum" pill: the pill right
/// of the type text, before the "Not Null" pill's slot — the same rect
/// `badge_pill` draws for it (see `paint_table`).
fn enum_pill_band(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> Option<(f32, f32)> {
    if !b.rows.get(row)?.ty_enum {
        return None;
    }
    let s = cell_size(zoom);
    let top_left = to_px(b.x, b.y, zoom, pan);
    let nn_w = if b.rows[row].not_null == Some(true) {
        Row::badge_cell("Not Null") * s.width
    } else {
        0.0
    };
    let en_w = Row::badge_cell("Enum") * s.width;
    let type_right = top_left.x + b.w as f32 * s.width - 0.5 * s.width - nn_w - en_w;
    let x0 = type_right + BADGE_PAD * s.width;
    let w = "Enum".chars().count() as f32 * BADGE_CHAR * s.width + 2.0 * BADGE_PAD * s.width;
    Some((x0, x0 + w))
}

/// The "Enum" pill under a canvas point, if any: (box, row) of the
/// enum-typed column whose pill band contains the pointer. The overlay
/// turns it into the enum's definition tooltip.
pub fn enum_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let (b, row) = hovered_row_of(layout, zoom, pan, p)?;
    let (x0, x1) = enum_pill_band(b, row, zoom, pan)?;
    let s = cell_size(zoom);
    let cy = y_of(b, row, zoom, pan);
    let half = 0.30 * s.height; // the pill's half-height, as badge_pill draws it
    (p.x >= x0 && p.x < x1 && (p.y - cy).abs() <= half).then_some((b, row))
}

/// The header-color palette glyph under a canvas point, if any — the
/// same geometry `paint_table` draws (header right end, shifted left a
/// band when the records glyph shares the header). The glyph only shows
/// while the header is hovered, and a click on it is what opens the
/// color menu.
pub fn palette_icon_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: Point,
) -> Option<&TableBox> {
    let b = hit_test(layout, zoom, pan, p)?;
    let s = cell_size(zoom);
    let top_left = to_px(b.x, b.y, zoom, pan);
    // header band only
    if (p.y - top_left.y) / s.height >= 1.0 {
        return None;
    }
    let (c, half_w, half_h) = palette_icon_geometry(b, top_left, s);
    ((p.x - c.x).abs() <= half_w && (p.y - c.y).abs() <= half_h).then_some(b)
}

/// The circles glyph's geometry, shared by the draw pass and the hit
/// test: the icon is RIGHT-ALIGNED to the header's inset edge (0.6 cell
/// in, shifted left a band when the records glyph shares the header) and
/// centered vertically on the band. The viewBox is 578x531 — wider than
/// tall, which is why the center and the two half-extents are computed
/// together (a square hit zone around the band center let the drawn icon
/// hang past the header's right edge).
fn palette_icon_geometry(
    b: &TableBox,
    top_left: Point,
    s: Size,
) -> (Point, f32, f32) {
    let right = top_left.x + b.w as f32 * s.width - 0.6 * s.width;
    // the wheel ALWAYS owns the header's rightmost slot (records sits to
    // its LEFT — see records_band), so the color glyph never jumps when
    // a records-bearing table is hovered
    let icon_h = 0.72 * s.height;
    let icon_w = CIRCLES_VIEWBOX.0 / CIRCLES_VIEWBOX.1 * icon_h;
    let c = Point::new(right - icon_w / 2.0, top_left.y + s.height * 0.5);
    // grabbable: never smaller than 7px half-extents
    (c, (icon_w / 2.0).max(7.0), (icon_h / 2.0).max(7.0))
}

/// The (box, row) the pointer is over, if it is on a column row.
pub(crate) fn hovered_row_of(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: Point,
) -> Option<(&TableBox, usize)> {
    let b = hit_test(layout, zoom, pan, p)?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let row = (((p.y - top_left.y) / s.height) as usize).saturating_sub(1);
    (row < b.rows.len()).then_some((b, row))
}

/// X range (frame px) of the records affordance on a table that has
/// records: the rows glyph at the right end of the header band (mirrors
/// the note band, one row up).
/// The header's title right edge in frame px: the 0.6-cell left inset
/// plus the bold-measured title — the anchor every title-adjacent glyph
/// (note, records) hangs from, and the same measure the width
/// computation reserves (Row::BOLD_TEXT on both sides keeps them in step).
fn title_end_px(b: &TableBox, top_left: Point, s: Size) -> f32 {
    top_left.x + 0.6 * s.width + Row::est_width(&b.title) * BOLD_TEXT * s.width
}

/// The note affordance's (center x, size) in frame px, right of the
/// title — shared by the painter and the records placement, which sits
/// on the note's right when the table carries one.
fn note_icon_geometry(b: &TableBox, top_left: Point, s: Size) -> Option<(f32, f32)> {
    let has_info = b.note.is_some() || !b.custom.is_empty();
    if !has_info {
        return None;
    }
    let size = 0.5 * s.height;
    let cx = title_end_px(b, top_left, s) + 0.6 * s.width + size * 0.5;
    Some((cx, size))
}

pub(crate) fn records_band(b: &TableBox, zoom: f32, pan: Vector) -> Option<(f32, f32)> {
    b.records.as_ref()?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    // the records glyph ALWAYS sits in the title-adjacent glyph row:
    // right of the note glyph when the table carries one, else right of
    // the title — the header's right edge stays the wheel's (hover) slot
    let records_size = 0.55 * s.height;
    let left = match note_icon_geometry(b, top_left, s) {
        Some((note_cx, note_size)) => note_cx + note_size * 0.5,
        None => title_end_px(b, top_left, s),
    };
    let center = left + 0.45 * s.width + records_size * 0.5;
    Some((center - 0.5 * s.width, center + 0.5 * s.width))
}

/// The records affordance under a canvas point: the rows glyph at the
/// right end of a records-bearing table's HEADER band. Pressing it opens
/// the record view (it wins over the generic table press).
pub fn records_icon_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<&TableBox> {
    let b = hit_test(layout, zoom, pan, p)?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let y_cell = (p.y - top_left.y) / s.height;
    if !(0.0..1.0).contains(&y_cell) {
        return None; // the header band only
    }
    let (x0, x1) = records_band(b, zoom, pan)?;
    (p.x >= x0 && p.x < x1).then_some(b)
}

/// The hover label for a pk/fk marker row.
pub fn marker_label(pk: bool) -> &'static str {
    if pk {
        "Primary Key"
    } else {
        "Foreign Key"
    }
}

/// The tooltip text for a hovered cluster icon: pk/fk/unique name their
/// constraint; the note affordance shows the row's note text (60-char
/// capped, the same pill the note icon has always popped).
pub fn icon_tooltip(icon: RowIcon) -> Option<String> {
    match icon {
        RowIcon::Pk => Some(marker_label(true).to_string()),
        RowIcon::Fk => Some(marker_label(false).to_string()),
        RowIcon::Unique => Some("Unique".to_string()),
        // the note glyph pops NO pill in any case: a real note is already
        // duplicated by the field card's Note section, and custom
        // properties by its Custom Properties section — a pill here would
        // read as a third copy of the same information
        RowIcon::Note => None,
    }
}

/// The icon tooltip the overlay must pop for the pointer, if it sits on
/// a cluster glyph: the EXACT glyph's label. The note glyph never pops a
/// pill — whatever raised it (a note, custom properties) is already
/// shown by the field card, whose Note / Custom Properties sections
/// would just be duplicated by a pill. Pk/fk/unique glyphs pop their
/// pill alongside the field card when the row also qualifies for one:
/// the overlay draws the card first, the pill on top.
fn icon_tooltip_line(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<String> {
    marker_icon_hit(layout, zoom, pan, p)
        .and_then(|(_, _, icon)| icon_tooltip(icon))
}

fn header_color(b: &TableBox) -> Color {
    b.header_color
        .as_deref()
        .and_then(dbml_ui::render::svg::parse_hex)
        .map(|(r, g, bl)| Color::from_rgb8(r, g, bl))
        // dbdiagram's dark theme renders table and enum headers the same
        // near-black band; a DBML headercolor override still wins
        .unwrap_or(theme::HEADER_BG)
}

/// Everything the painter needs; rebuilt each frame in `view`.
pub struct Diagram<'a> {
    pub layout: &'a Layout,
    /// Pixels per diagram cell height.
    pub zoom: f32,
    pub pan: Vector,
    /// Cursor position over the canvas (local px), for hover highlight.
    pub hover: Option<Point>,
    /// The selected sticky note (its name): draw_notes paints its
    /// selection border. Note text is edited in the source editor only.
    pub selected_note: Option<String>,
    /// A drag is in flight (table or pan) — show the grab cursor.
    pub dragging: bool,
    /// Raise order across tables AND stickies (most recently grabbed
    /// last): a raised item paints after every unraised one, so the
    /// last-grabbed item keeps priority over whatever it crossed.
    pub item_z: &'a [RaisedItem],
    /// The TableGroup being dragged (its name): member tables paint
    /// after every non-member for the duration of the drag.
    pub drag_group: Option<String>,
    /// The sticky being dragged or resized (its name): it paints after
    /// every other note (an item dragged over another rides on top).
    pub drag_note: Option<String>,

    /// Indices of the dragged table's relationship lines: while the drag
    /// is live those lines paint ABOVE the tables they cross (back
    /// beneath on release).
    pub drag_edges: Option<Vec<usize>>,
    /// Per-edge highlight amount in 0..1 (animated by the app's tick);
    /// parallel to `layout.edges`.
    pub edge_anim: &'a [f32],
    /// Animation clock in seconds; advances the dash-flow phase.
    pub clock: f32,
}

impl canvas::Program<crate::app::Message> for Diagram<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<crate::app::Message>) {
        use crate::app::Message;
        use iced::mouse;

        // The canvas widget forwards every window event with window-level
        // coordinates; only react when the pointer is over this canvas.
        // Exception: while a drag is in flight, keep tracking moves and the
        // release outside the canvas so pans continue past the edges.
        let local = |p: Point| Point::new(p.x - bounds.x, p.y - bounds.y);
        let over = cursor.position_over(bounds).map(local);
        let tracked = if self.dragging {
            over.or_else(|| cursor.position().map(local))
        } else {
            over
        };

        let (status, message) = match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => (
                canvas::event::Status::Captured,
                over.map(Message::CanvasPressed),
            ),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => (
                canvas::event::Status::Captured,
                tracked.map(Message::CanvasReleased),
            ),
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => (
                canvas::event::Status::Ignored,
                if self.dragging {
                    // a diagram drag keeps tracking past the pane edges
                    tracked.map(Message::CanvasMoved)
                } else {
                    // hover tracks enter/leave like the editor's tooltip:
                    // a move over another pane (or out of the window)
                    // drops every hover effect — a quick hop to the
                    // editor used to leave the border, cards, handle and
                    // edge glow painted
                    Some(match tracked {
                        Some(p) => Message::CanvasMoved(p),
                        None => Message::CanvasLeft,
                    })
                },
            ),
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let notches = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                (
                    canvas::event::Status::Captured,
                    over.map(|_| Message::Wheel(notches)),
                )
            }
            _ => (canvas::event::Status::Ignored, None),
        };
        (status, message)
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
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::CANVAS);
        // NOTE: canvas geometry IS clipped to this widget's bounds at
        // render time — the geometry's frame clip bounds intersect the
        // scissor/mask in both backends (iced_wgpu scissors to
        // `mesh.clip_bounds() * transformation`; the vendored tiny-skia
        // compositor masks the same way) — so boxes dragged past the pane
        // seam never paint over the editor pane. Don't use
        // Frame::with_clip here — its draft/paste misplaces mesh
        // geometry on the wgpu backend in 0.13.
        paint(
            &mut frame,
            self.layout,
            self.zoom,
            self.pan,
            &HoverState {
                anim: self.edge_anim,
                pointer: self.hover,
                clock: self.clock,
                selected_note: self.selected_note.as_deref(),
                item_z: self.item_z,
                drag_group: self.drag_group.as_deref(),
                drag_note: self.drag_note.as_deref(),
                drag_edges: self.drag_edges.as_deref(),
            },
        );

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> iced::mouse::Interaction {
        use iced::mouse::Interaction;
        // the fist only over the diagram pane: canvas mouse_interaction is
        // consulted window-wide, and an unbounded fist leaked over the
        // editor while a table was dragged across the seam
        if self.dragging && cursor.position_over(bounds).is_some() {
            return Interaction::Grabbing;
        }
        if let Some(p) = cursor.position_over(bounds) {
            let local = Point::new(p.x - bounds.x, p.y - bounds.y);
            // the cursor mirrors the PRESS: the TOPMOST item wins (the
            // raise order decides, stickies included) — a raised table
            // over a sticky says table, a sticky over a table says grab.
            // Band and line cursors only count in open space.
            match hit_topmost(self.layout, self.zoom, self.pan, local, self.item_z) {
                Some(PaintItem::Note(top)) => {
                    // a corner press resizes (cursor = the corner's
                    // resize axis); a body press drags the sticky. The
                    // corner must belong to the TOPMOST note: a doc-order
                    // scan could return a lower sticky's corner.
                    if let Some((_, corner)) = super::notes::note_corner_hit(
                        self.layout,
                        self.zoom,
                        self.pan,
                        local,
                    )
                    .filter(|(h, _)| h.name == top.name)
                    {
                        // NW/SE corners take the nwse cursor (iced's
                        // DiagonallyDown), NE/SW the nesw one
                        return match corner {
                            crate::app::StickyCorner::TopLeft
                            | crate::app::StickyCorner::BottomRight => {
                                Interaction::ResizingDiagonallyDown
                            }
                            crate::app::StickyCorner::TopRight
                            | crate::app::StickyCorner::BottomLeft => {
                                Interaction::ResizingDiagonallyUp
                            }
                        };
                    }
                    return Interaction::Grab;
                }
                Some(PaintItem::Table(_)) => {
                    if records_icon_hit(self.layout, self.zoom, self.pan, local).is_some() {
                        return Interaction::Pointer; // opens the record view
                    }
                    // the color-picker glyph opens its menu: the link cursor
                    if palette_icon_hit(self.layout, self.zoom, self.pan, local).is_some() {
                        return Interaction::Pointer;
                    }
                    // dbdiagram's cursors: a column row grabs (drag =
                    // re-order affordance in their editor), the rest of
                    // the table moves. The note icon keeps the surrounding
                    // cursor: it sits over a column row.
                    if row_at(self.layout, self.zoom, self.pan, local).is_some() {
                        return Interaction::Grab;
                    }
                    return Interaction::Move;
                }
                None => {
                    // a group's header band reads as movable (it drags the
                    // whole container) — except its picker icon, a link
                    if super::groups::group_icon_hit(self.layout, self.zoom, self.pan, local)
                        .is_some()
                    {
                        return Interaction::Pointer;
                    }
                    if super::groups::group_label_hit(self.layout, self.zoom, self.pan, local)
                        .is_some()
                    {
                        return Interaction::Move;
                    }
                    // a relationship line reads as interactive, like
                    // dbdiagram's
                    if edge_hit(self.layout, self.zoom, self.pan, local, 7.0).is_some() {
                        return Interaction::Pointer;
                    }
                    // dbdiagram's empty canvas stays the plain arrow
                    return Interaction::default();
                }
            }
        }
        Interaction::default()
    }
}

pub fn row_at(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let b = hit_test(layout, zoom, pan, p)?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let y_cell = (p.y - top_left.y) / s.height;
    if y_cell < 1.0 {
        return None; // the header band is not a column row
    }
    let row = (y_cell as usize).saturating_sub(1);
    (row < b.rows.len()).then_some((b, row))
}

/// The edge whose polyline passes within `threshold` px of the point
/// (nearest wins), for line-hover highlighting.
pub fn edge_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: Point,
    threshold: f32,
) -> Option<usize> {
    let s = cell_size(zoom);
    let to_px = |(x, y): (f32, f32)| {
        Point::new(
            (x + MARGIN_CELLS) * s.width + pan.x,
            (y + MARGIN_CELLS) * s.height + pan.y,
        )
    };
    let mut best: Option<(f32, usize)> = None;
    for r in &layout.edges {
        let mut dmin = f32::INFINITY;
        for w in r.pts.windows(2) {
            let (a, b) = (to_px(w[0]), to_px(w[1]));
            let (vx, vy) = (b.x - a.x, b.y - a.y);
            let len2 = vx * vx + vy * vy;
            let t = if len2 < f32::EPSILON {
                0.0
            } else {
                (((p.x - a.x) * vx + (p.y - a.y) * vy) / len2).clamp(0.0, 1.0)
            };
            let dx = a.x + t * vx - p.x;
            let dy = a.y + t * vy - p.y;
            dmin = dmin.min((dx * dx + dy * dy).sqrt());
        }
        if dmin <= threshold && best.is_none_or(|(d, _)| dmin < d) {
            best = Some((dmin, r.edge));
        }
    }
    best.map(|(_, e)| e)
}

/// Hover-effect state the painter needs, threaded through `paint`.
struct HoverState<'a> {
    /// Per-edge highlight amount in 0..1, parallel to `layout.edges`.
    anim: &'a [f32],
    /// Pointer position over the canvas (local px).
    pointer: Option<Point>,
    /// Animation clock in seconds; advances the dash-flow phase.
    clock: f32,
    /// The selected sticky note (its name), for the border.
    selected_note: Option<&'a str>,
    /// Raise order across tables AND stickies: raised items paint last.
    item_z: &'a [RaisedItem],
    /// The TableGroup being dragged (its name): member tables paint
    /// after every non-member for the duration of the drag.
    drag_group: Option<&'a str>,
    /// The sticky being dragged/resized (its name): it paints last.
    drag_note: Option<&'a str>,
    /// Indices of the dragged table's lines: painted above the tables.
    drag_edges: Option<&'a [usize]>,
}

/// One paintable diagram item: a table box or a sticky note. Tables and
/// notes interleave in ONE paint sequence so the raise order can rank
/// them against each other.
pub enum PaintItem<'a> {
    Table(&'a TableBox),
    Note(&'a NoteBox),
}

/// The unified paint sequence. Rank rules:
/// - unraised tables in doc order, then unraised notes in schema order
///   (the static layering: stickies float above untouched tables)
/// - raised items in grab order — tables and notes interleave, so the
///   last-grabbed item outranks everything it crossed, whatever kind
/// - a dragged group's members outrank every non-member (and every
///   note) for the drag's duration
/// - the dragged table / dragged sticky go LAST — the drag owns its own
///   body and paints after its raised lines
///
/// Text occlusion then follows this sequence directly: every item's
/// text clips around everything painted after it.
pub(crate) fn paint_items<'a>(
    layout: &'a Layout,
    item_z: &[RaisedItem],
    drag_group: Option<&str>,
    drag_note: Option<&str>,
    drag_table: Option<&str>,
) -> Vec<PaintItem<'a>> {
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum Rank {
        BaseTable(usize),
        BaseNote(usize),
        Raised(usize),
        GroupMember(usize),
        DragNote,
    }
    let group_members: &[usize] = drag_group
        .and_then(|name| layout.groups.iter().find(|g| g.name == name))
        .map(|g| g.members.as_slice())
        .unwrap_or(&[]);
    let rank_of = |t: &TableBox| {
        item_z.iter().position(|r| match r {
            RaisedItem::Table(title) => title == &t.title,
            _ => false,
        })
    };
    let note_rank_of = |n: &NoteBox| {
        item_z.iter().position(|r| match r {
            RaisedItem::Note(name) => name == &n.name,
            _ => false,
        })
    };
    let mut seq: Vec<(Rank, PaintItem)> = Vec::new();
    for (i, b) in layout.boxes.iter().enumerate() {
        let rank = match rank_of(b) {
            None => Rank::BaseTable(i),
            Some(pos) if group_members.contains(&b.node) => Rank::GroupMember(pos),
            Some(pos) => Rank::Raised(pos),
        };
        seq.push((rank, PaintItem::Table(b)));
    }
    for (i, n) in layout.notes.iter().enumerate() {
        let rank = match (note_rank_of(n), drag_note == Some(n.name.as_str())) {
            (_, true) => Rank::DragNote,
            (None, false) => Rank::BaseNote(i),
            (Some(pos), false) => Rank::Raised(pos),
        };
        seq.push((rank, PaintItem::Note(n)));
    }
    seq.sort_by_key(|(rank, _)| match rank {
        Rank::BaseTable(i) => (0, *i),
        Rank::BaseNote(i) => (1, *i),
        Rank::Raised(pos) => (2, *pos),
        Rank::GroupMember(pos) => (3, *pos),
        Rank::DragNote => (4, 0),
    });
    let mut seq: Vec<PaintItem> = seq.into_iter().map(|(_, item)| item).collect();
    // the dragged table paints after its raised lines AND after every
    // note: pull it to the very end
    if let Some(title) = drag_table {
        if let Some(i) = seq.iter().position(
            |item| matches!(item, PaintItem::Table(b) if b.title == title),
        ) {
            let dragged = seq.remove(i);
            seq.push(dragged);
        }
    }
    seq
}

fn paint(
    frame: &mut canvas::Frame,
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    hs: &HoverState<'_>,
) {
    // the dragged table's own lines stay beneath while idle; while its
    // drag is live they are pulled out and re-painted ABOVE the tables
    // (back beneath on release — dbdiagram's drag behavior)
    let raised = hs.drag_edges.unwrap_or(&[]);
    // the hover border belongs to the TOPMOST table: a table under a
    // sticky (or under a raised table) must not light up because the
    // pointer crossed its hidden area
    let hovered = hs.pointer.and_then(|p| {
        match hit_topmost(layout, zoom, pan, p, hs.item_z) {
            Some(PaintItem::Table(b)) => Some(b),
            _ => None,
        }
    });
    // the hovered column row (dbdiagram tints it); only when its box is
    // the hovered one
    // rows to tint: the hovered row, plus BOTH endpoint rows of every
    // lit edge — dbdiagram tints the far end of a hovered relationship
    // with the same hover background
    let mut tinted: Vec<(usize, usize)> = hs
        .pointer
        .and_then(|p| row_at(layout, zoom, pan, p))
        .filter(|(b, _)| hovered.is_some_and(|h| h.title == b.title))
        .map(|(b, r)| vec![(b.node, r)])
        .unwrap_or_default();
    for edge in &layout.edges {
        if hs.anim.get(edge.edge).copied().unwrap_or(0.0) > 0.0 {
            tinted.extend(edge.ends);
        }
    }
    // while a connected-table drag is live, drag_edges is Some and the
    // grab pushed the dragged table to item_z's end — that title is the
    // dragged table, painted after its raised lines below
    let dragging_table = if hs.drag_edges.is_some() {
        hs.item_z.iter().rev().find_map(|r| match r {
            RaisedItem::Table(title) => Some(title.as_str()),
            _ => None,
        })
    } else {
        None
    };
    // while a TableGroup is dragged, its member tables paint after every
    // non-member (and every note) for the drag's duration — the group
    // takes priority over the canvas
    let drag_group = hs
        .drag_group
        .and_then(|name| layout.groups.iter().find(|g| g.name == name));
    // ONE paint sequence for tables and stickies: unraised tables (doc
    // order), unraised notes (schema order), then raised items in grab
    // order — the last-grabbed item outranks everything it crossed,
    // whatever kind. The dragged table/note rank last.
    let seq = paint_items(
        layout,
        hs.item_z,
        drag_group.map(|g| g.name.as_str()),
        hs.drag_note,
        dragging_table,
    );
    let rect_of = |item: &PaintItem| match item {
        PaintItem::Table(b) => box_rect(b, zoom, pan),
        PaintItem::Note(n) => note_rect_px(n, zoom, pan),
    };
    let rects: Vec<Rectangle> = seq.iter().map(rect_of).collect();
    // the groups paint first (behind everything), so EVERY table and
    // sticky is above them — their label text clips around all of it
    // group regions sit behind everything (dbdiagram's labeled frames)
    super::groups::draw_groups(frame, layout, zoom, pan, hs.pointer, &rects);
    // the highlight's glow pass FIRST (under every core): all highlighted
    // edges' soft bands paint before any line core, so overlapping
    // highlighted lines share one glow level instead of compounding
    for (ei, edge) in layout.edges.iter().enumerate() {
        if raised.contains(&ei) {
            continue;
        }
        let t = hs.anim.get(edge.edge).copied().unwrap_or(0.0);
        paint_edge(frame, edge, zoom, pan, t, hs.clock, true);
    }
    for (ei, edge) in layout.edges.iter().enumerate() {
        if raised.contains(&ei) {
            continue;
        }
        let t = hs.anim.get(edge.edge).copied().unwrap_or(0.0);
        paint_edge(frame, edge, zoom, pan, t, hs.clock, false);
    }
    // the topmost note at the pointer: only IT draws its drag handles
    // (a buried sticky's envelope must not sprout handles under the
    // item covering it)
    let hover_note = hs.pointer.and_then(|p| {
        match hit_topmost(layout, zoom, pan, p, hs.item_z) {
            Some(PaintItem::Note(n)) => Some(n.name.as_str()),
            _ => None,
        }
    });
    // iced draws ALL of a canvas's text above ALL of its geometry (the
    // renderer emits meshes first, then the text stack), so paint order
    // alone cannot hide an overlapped table's text — the covered table's
    // text pass is therefore drawn CLIPPED to the region that sticks out
    // around EVERY object above it (with_clip stamps clip_bounds into
    // every text primitive — per-pixel). Fills always paint unclipped at
    // draw order: the occluder's opaque body covers them in the overlap.
    // ONE occlusion rule for tables and stickies alike: an item's text
    // clips around EVERY object painted after it — the raise order
    // decides who covers whom, and with three stacked items the middle
    // one hides the bottom one's text too, not just the top occluder.
    // The dragged table/note stay in `seq` (their rects occlude what
    // they cover) but skip painting here — the drag owns its own body
    // and paints after the raised lines below.
    for (i, item) in seq.iter().enumerate() {
        let rect = rects[i];
        let occluders: Vec<Rectangle> = rects[i + 1..]
            .iter()
            .filter(|r| r.intersects(&rect))
            .copied()
            .collect();
        match item {
            PaintItem::Table(b) => {
                if Some(b.title.as_str()) == dragging_table {
                    continue;
                }
                let is_hovered = hovered.map(|h| h.title == b.title).unwrap_or(false);
                let rows: Vec<usize> = tinted
                    .iter()
                    .filter(|(n, _)| *n == b.node)
                    .map(|(_, r)| *r)
                    .collect();
                paint_table(frame, b, zoom, pan, is_hovered, &rows, rect.position(), false);
                if occluders.is_empty() {
                    paint_table(frame, b, zoom, pan, is_hovered, &rows, rect.position(), true);
                } else {
                    let visible = visible_region(rect, &occluders);
                    for sliver in &visible {
                        if sliver.width < 2.0 || sliver.height < 2.0 {
                            continue;
                        }
                        frame.with_clip(*sliver, |f| {
                            // the draft frame's origin is the sliver's:
                            // paint the table shifted back under it
                            paint_table(
                                f,
                                b,
                                zoom,
                                pan,
                                is_hovered,
                                &rows,
                                Point::new(rect.x - sliver.x, rect.y - sliver.y),
                                true,
                            );
                        });
                    }
                }
            }
            PaintItem::Note(n) => {
                if hs.drag_note == Some(n.name.as_str()) {
                    continue;
                }
                super::notes::draw_note(
                    frame,
                    n,
                    zoom,
                    pan,
                    super::notes::NoteChrome {
                        pointer: hs.pointer,
                        selected: hs.selected_note,
                        top_note: hover_note,
                    },
                    &occluders,
                );
            }
        }
    }
    // the dragged table's lines: ABOVE every table they cross until the
    // drag is released (the exception to tables-over-lines)
    for ei in raised {
        if let Some(edge) = layout.edges.get(*ei) {
            let t = hs.anim.get(edge.edge).copied().unwrap_or(0.0);
            paint_edge(frame, edge, zoom, pan, t, hs.clock, true);
            paint_edge(frame, edge, zoom, pan, t, hs.clock, false);
        }
    }
    // the DRAGGED items paint after the raised lines — the drag owns its
    // own body: an item dragged over anything covers it fully (fill in
    // the mesh pass, text later in the text pass), while its raised
    // lines stay over every OTHER table. Known iced artifact: ALL canvas
    // text renders above ALL meshes, so a dragged box's text layer still
    // rides above a line crossing it — its fill, pills and borders do
    // not.
    if let Some(name) = hs.drag_note {
        if let Some(n) = layout.notes.iter().find(|n| n.name == name) {
            // the dragged note draws its own handles: it is topmost by
            // construction
            super::notes::draw_note(
                frame,
                n,
                zoom,
                pan,
                super::notes::NoteChrome {
                    pointer: hs.pointer,
                    selected: hs.selected_note,
                    top_note: Some(n.name.as_str()),
                },
                &[],
            );
        }
    }
    if let Some(title) = dragging_table {
        if let Some(b) = layout.boxes.iter().find(|b| b.title == title) {
            let rect = box_rect(b, zoom, pan);
            let is_hovered = hovered.map(|h| h.title == b.title).unwrap_or(false);
            let rows: Vec<usize> = tinted
                .iter()
                .filter(|(n, _)| *n == b.node)
                .map(|(_, r)| *r)
                .collect();
            paint_table(frame, b, zoom, pan, is_hovered, &rows, rect.position(), false);
            paint_table(frame, b, zoom, pan, is_hovered, &rows, rect.position(), true);
        }
    }
}

/// A line's base color: an explicit `[color: …]` wins, else the
/// referenced table's resolved header color (its TableGroup cascade
/// included), else the default gray.
fn edge_base_color(edge: &EdgeRoute) -> Color {
    edge.color
        .as_deref()
        .and_then(theme::hex_color)
        .or_else(|| edge.inherited.as_deref().and_then(theme::hex_color))
        .unwrap_or(theme::LINE)
}

/// A line's hover color (dbdiagram's highlight): a LIGHTER tone of the
/// color the line already wears when it HAS one — an explicit
/// `[color: …]` or a non-dark inherited header (their likes.post_id →
/// posts purple line highlights in a lifted purple) — else the highlight
/// blue. A line resolving to the dark header option (no inherited color
/// at all — the default cascade — or the dark swatch/hex picked for the
/// referenced table) has no visible highlight IN that color, so it eases
/// to the blue.
fn edge_hover_color(edge: &EdgeRoute) -> Color {
    let colored = edge.color.is_some()
        || edge
            .inherited
            .as_deref()
            .is_some_and(|h| !h.eq_ignore_ascii_case(theme::DARK_HEADER_HEX));
    if colored {
        // a touch lighter than the line, through BRIGHTENING (their
        // colorFactor move) — whitening reads as a washed pastel
        brighten(edge_base_color(edge), 1.15)
    } else {
        theme::EDGE_HOVER
    }
}

fn paint_edge(
    frame: &mut canvas::Frame,
    edge: &EdgeRoute,
    zoom: f32,
    pan: Vector,
    t: f32,
    clock: f32,
    glow_pass: bool,
) {
    let s = cell_size(zoom);
    // waypoints are f32 cell coords; rows anchor at their vertical center
    let pts: Vec<Point> = edge
        .pts
        .iter()
        .map(|&(x, y)| {
            Point::new(
                (x + MARGIN_CELLS) * s.width + pan.x,
                (y + MARGIN_CELLS) * s.height + pan.y,
            )
        })
        .collect();
    if pts.len() < 2 {
        return;
    }
    // dbdiagram caps the corner fillet at 10px in 1:1 pixels
    let r = (0.9 * s.height).min(10.0 * zoom / 20.0);
    let path = rounded_edge_path(&pts, r);

    // dbdiagram's hovered route: the core eases to the line's highlight
    // color and their LINE_WIDTH_NORMAL 1 -> HIGHLIGHT 2 — exactly 2px at
    // 1:1 (a 2x multiplier over our wider 1.5px base read as unusually
    // thick under the highlight color). A `[color: ...]` ref replaces
    // the gray base, and a line WITHOUT one renders in the referenced
    // table's header color (`inherited`) — the whole relationship (line
    // + markers) renders in it (live-verified).
    let k = zoom / 20.0;
    let mut core = edge_stroke(zoom);
    let base = edge_base_color(edge);
    let hover_color = edge_hover_color(edge);
    if glow_pass {
        // static glow hugging the path (their shape shadow: blur 10,
        // opacity .5 in the dark theme — no pulsing). The nested rings
        // ride in the line's own highlight color; MSAA blends the bands
        // into a smooth falloff. Restored verbatim from f529342 — every
        // replacement (opaque bands, dashed rings, disc ladders) read
        // worse than this.
        if t > 0.0 {
            paint_glow(frame, &path, zoom, t, hover_color);
        }
        return;
    }
    core.style = lerp_color(base, hover_color, t).into();
    core.width = core.width + (2.0 * zoom / 20.0 - core.width) * t;

    // end markers: the "many" end gets a chevron + open circle, the "one"
    // end a perpendicular tick. Each end also trims the flow path by the
    // marker gap (their LGt: 10px at a one end, 16px at a many end), so
    // the dashes stop at the circle instead of painting over the markers.
    let (many_fk, many_ref) = many_ends(edge.card);
    let n = pts.len();
    let trim_fk = if many_fk { 0.8 * zoom } else { 0.5 * zoom };
    let trim_ref = if many_ref { 0.8 * zoom } else { 0.5 * zoom };

    if t > 0.0 {
        frame.stroke(&path, core);
        // the flow: a dashed stroke on the SAME filleted path (their
        // setLineDash([3, 40]) with lineDashOffset = time/30), so dashes
        // ride the corners instead of cutting them. Increasing the offset
        // shifts the pattern toward the path start; pts[0] is the fk end,
        // so the flow reads referenced key → foreign key. Their dash
        // color is the line's brightened 1.2x.
        let phase = hs_clock(clock) * zoom / 20.0;
        paint_flow(frame, &pts, r, phase, t, (trim_fk, trim_ref), zoom, hover_color, 4.0 * k);
    } else {
        frame.stroke(&path, core);
    }

    if many_fk {
        many_marker(frame, pts[0], pts[1], zoom, core);
    } else {
        one_tick(frame, pts[0], pts[1], zoom, core);
    }
    if many_ref {
        many_marker(frame, pts[n - 1], pts[n - 2], zoom, core);
    } else {
        one_tick(frame, pts[n - 1], pts[n - 2], zoom, core);
    }

}
fn hs_clock(clock: f32) -> f32 {
    // their createAnimation: dashOffset = time_ms / 30 → ~33.3 px/s at 1:1
    clock * 1000.0 / 30.0
}

/// Pull the first/last points inward along their segments by `first`/`last`
/// px (their LGt trims the hover overlay by the marker gap). The glow must
/// stop in the marker gap too: the rings are half as wide as the trim is
/// deep, so a trimmed halo ends short of the anchor instead of stacking
/// with every other route that ends on the same column.
/// Static halo around a highlighted route: the canvas has no blur, so
/// three nested strokes fake the falloff of dbdiagram's shape shadow.
/// The flowing dashes of a highlighted route: 3px dashes with 40px gaps
/// (1:1 px), 4px wide, gliding along the filleted path. Rendered by
/// stroking each dash's sub-range of the flattened path — a dash pattern
/// can never cut a corner the way bead chords did.
/// The highlighted route's gaussian glow: nested translucent rings in
/// the line's brightened highlight color, MSAA blends the bands into a
/// smooth falloff (f529342's paint_glow, restored verbatim).
fn paint_glow(
    frame: &mut canvas::Frame,
    path: &Path,
    zoom: f32,
    t: f32,
    hover_color: Color,
) {
    let k = zoom / 20.0;
    for (w, a) in [
        (2.0, 0.16),
        (3.5, 0.13),
        (5.0, 0.10),
        (6.5, 0.075),
        (8.5, 0.05),
        (10.5, 0.03),
        (12.5, 0.015),
    ] {
        frame.stroke(
            path,
            Stroke {
                style: with_alpha(brighten(hover_color, 1.4), a * t).into(),
                width: w * k,
                ..Default::default()
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_flow(
    frame: &mut canvas::Frame,
    pts: &[Point],
    radius: f32,
    phase: f32,
    t: f32,
    trims: (f32, f32),
    zoom: f32,
    hover_color: Color,
    width: f32,
) {
    let flat = flatten_pieces(&rounded_pieces(pts, radius));
    if flat.total <= 0.0 {
        return;
    }
    // dash cycle/length/width are authored in 1:1 px like dbdiagram's
    // setLineDash([3,40]) and scale with zoom (their stage transform)
    let k = zoom / 20.0;
    let lo = trims.0.min(flat.total);
    let hi = (flat.total - trims.1).max(lo);
    for (a, b) in dash_windows(phase, 43.0 * k, 3.0 * k, lo, hi) {
        stroke_arc_range(frame, &flat, a, b, width, hover_color, t);
    }
}

/// Where the [dash, cycle] pattern lands on the path for a given phase:
/// increasing phase shifts windows toward the path start (the canvas
/// lineDashOffset semantics), each clipped to the drawable [lo, hi].
fn dash_windows(phase: f32, cycle: f32, dash: f32, lo: f32, hi: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    if (hi - lo).abs() < f32::EPSILON {
        return out;
    }
    // first window start at or before lo
    let shift = phase.rem_euclid(cycle);
    let mut start = lo - shift;
    while start < hi {
        let a = start.max(lo);
        let b = (start + dash).min(hi);
        if b > a {
            out.push((a, b));
        }
        start += cycle;
    }
    out
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    Color::new(
        a.r + (b.r - a.r) * t,
        a.g + (b.g - a.g) * t,
        a.b + (b.b - a.b) * t,
        a.a,
    )
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a: a.clamp(0.0, 1.0), ..c }
}

/// One segment of the filleted orthogonal path: a straight run or a
/// quadratic corner bend (a quadratic B(c;t) equals a cubic with both
/// controls at c, which iced's builder has).
enum Piece {
    Line(Point, Point),
    Quad(Point, Point, Point),
}

/// The filleted path as geometric pieces: straight runs with each corner
/// replaced by `cut`-length entry/exit runs joined by a quadratic through
/// the corner point (dbdiagram's RGt, cut capped at `r`).
fn rounded_pieces(pts: &[Point], r: f32) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let Some(first) = pts.first() else {
        return pieces;
    };
    let mut cursor = *first;
    for i in 1..pts.len().saturating_sub(1) {
        let (p, v, n) = (pts[i - 1], pts[i], pts[i + 1]);
        let seg_in = ((v.x - p.x).powi(2) + (v.y - p.y).powi(2)).sqrt();
        let seg_out = ((n.x - v.x).powi(2) + (n.y - v.y).powi(2)).sqrt();
        if seg_in < f32::EPSILON || seg_out < f32::EPSILON {
            continue;
        }
        let cut = r.min(seg_in / 2.0).min(seg_out / 2.0);
        let din = ((v.x - p.x) / seg_in, (v.y - p.y) / seg_in);
        let dout = ((n.x - v.x) / seg_out, (n.y - v.y) / seg_out);
        let entry = Point::new(v.x - din.0 * cut, v.y - din.1 * cut);
        let exit = Point::new(v.x + dout.0 * cut, v.y + dout.1 * cut);
        pieces.push(Piece::Line(cursor, entry));
        pieces.push(Piece::Quad(entry, v, exit));
        cursor = exit;
    }
    if let Some(last) = pts.last() {
        pieces.push(Piece::Line(cursor, *last));
    }
    pieces
}

/// Orthogonal waypoint path with filleted corners.
fn rounded_edge_path(pts: &[Point], r: f32) -> Path {
    Path::new(|builder| {
        let pieces = rounded_pieces(pts, r);
        let Some(first) = pieces.first() else {
            return;
        };
        let start = match first {
            Piece::Line(a, _) | Piece::Quad(a, _, _) => *a,
        };
        builder.move_to(start);
        for piece in &pieces {
            match piece {
                Piece::Line(_, b) => builder.line_to(*b),
                Piece::Quad(_, c, b) => builder.quadratic_curve_to(*c, *b),
            }
        }
    })
}

/// A flat arc-length parameterization of the path: sampled points with
/// cumulative distance, plus the total length.
struct Flat {
    pts: Vec<(Point, f32)>,
    total: f32,
}

/// Flatten the pieces into a dense polyline (corners at 8 samples) so any
/// sub-range of the path can be stroked following its curves exactly.
fn flatten_pieces(pieces: &[Piece]) -> Flat {
    let mut pts: Vec<(Point, f32)> = Vec::new();
    let mut total = 0.0f32;
    for piece in pieces {
        let (a, b, quad) = match *piece {
            Piece::Line(a, b) => (a, b, None),
            Piece::Quad(a, c, b) => (a, b, Some(c)),
        };
        if pts.is_empty() {
            pts.push((a, 0.0));
        }
        // sample lines in one step, corners at 8 quadratic steps
        let steps = if quad.is_some() { 8 } else { 1 };
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            let p = match quad {
                None => Point::new(
                    a.x + (b.x - a.x) * t,
                    a.y + (b.y - a.y) * t,
                ),
                Some(c) => {
                    let mt = 1.0 - t;
                    Point::new(
                        mt * mt * a.x + 2.0 * mt * t * c.x + t * t * b.x,
                        mt * mt * a.y + 2.0 * mt * t * c.y + t * t * b.y,
                    )
                }
            };
            let (last, _) = *pts.last().unwrap();
            let d = ((p.x - last.x).powi(2) + (p.y - last.y).powi(2)).sqrt();
            if d < f32::EPSILON {
                continue;
            }
            total += d;
            pts.push((p, total));
        }
    }
    Flat { pts, total }
}

/// The path point at arc distance `s` (linear within the sampled pieces).
fn flat_point(flat: &Flat, s: f32) -> Point {
    let s = s.clamp(0.0, flat.total);
    let idx = match flat.pts.binary_search_by(|(_, d)| d.partial_cmp(&s).unwrap()) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let (a, sa) = flat.pts[idx];
    match flat.pts.get(idx + 1) {
        Some((b, sb)) if *sb > sa => {
            let t = (s - sa) / (*sb - sa);
            Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
        }
        _ => a,
    }
}

/// Stroke the path sub-range `[a, b]` (arc distances) as a polyline.
fn stroke_arc_range(
    frame: &mut canvas::Frame,
    flat: &Flat,
    a: f32,
    b: f32,
    width: f32,
    color: Color,
    t: f32,
) {
    if b <= a {
        return;
    }
    let path = Path::new(|builder| {
        builder.move_to(flat_point(flat, a));
        // sample every few px so straight runs stay cheap and corners curve
        let mut s = a;
        while s < b {
            s = (s + 2.0).min(b);
            builder.line_to(flat_point(flat, s));
        }
    });
    frame.stroke(
        &path,
        Stroke {
            // the dashes brighten the line's own hover color — the flow
            // dots match the line they ride (never a hardcoded blue)
            style: with_alpha(brighten(color, 1.2), t).into(),
            width,
            ..Default::default()
        },
    );
}

/// Multiply an RGB color's brightness by `f` (their colorFactor brighten).
fn brighten(c: Color, f: f32) -> Color {
    Color::new((c.r * f).min(1.0), (c.g * f).min(1.0), (c.b * f).min(1.0), c.a)
}

fn edge_stroke(zoom: f32) -> Stroke<'static> {
    Stroke {
        style: theme::LINE.into(),
        width: edge_stroke_width(zoom),
        ..Default::default()
    }
}

/// Unit direction from `a` to `b`, and its perpendicular.
fn dir_and_normal(a: Point, b: Point) -> (Vector, Vector) {
    let d = Vector::new(b.x - a.x, b.y - a.y);
    let len = (d.x * d.x + d.y * d.y).sqrt();
    if len < f32::EPSILON {
        (Vector::new(1.0, 0.0), Vector::new(0.0, 1.0))
    } else {
        let d = Vector::new(d.x / len, d.y / len);
        (d, Vector::new(-d.y, d.x))
    }
}

/// The "one" end: a perpendicular tick across the line, sitting in the
/// 10px marker gap before the table edge (their PGt, ±4px at 10px out).
fn one_tick(frame: &mut canvas::Frame, end: Point, inner: Point, zoom: f32, stroke: Stroke) {
    let (d, n) = dir_and_normal(end, inner);
    let half = 4.0 * zoom / 20.0;
    let out = end + d * (0.5 * zoom);
    frame.stroke(
        &Path::new(|b| {
            b.move_to(out - n * half);
            b.line_to(out + n * half);
        }),
        stroke,
    );
}

/// The "many" end: a chevron whose wings sit on the table edge and whose
/// tip points back along the line (9px out), plus an open circle (r=3px)
/// just past the tip at 13px out — their NGt + FGt; the flow's dashes
/// stop at this circle.
fn many_marker(frame: &mut canvas::Frame, end: Point, inner: Point, zoom: f32, stroke: Stroke) {
    let k = zoom / 20.0;
    let (d, n) = dir_and_normal(end, inner);
    let spread = 4.0 * k;
    let tip = end + d * (9.0 * k);
    frame.stroke(
        &Path::new(|b| {
            b.move_to(end - n * spread);
            b.line_to(tip);
            b.line_to(end + n * spread);
        }),
        canvas::Stroke {
            line_join: iced::widget::canvas::LineJoin::Round,
            ..stroke
        },
    );
    frame.stroke(
        &Path::circle(end + d * (13.0 * k), 3.0 * k),
        stroke,
    );
}

/// Edge stroke thickness at `zoom`: proportional above 1:1, but never
/// thinner than 1px on screen — sub-pixel strokes rasterize depending on
/// their subpixel phase, so edges visibly flickered in and out while
/// panning a zoomed-out diagram (the classic hairline-SVG problem).
fn edge_stroke_width(zoom: f32) -> f32 {
    (1.5 * zoom / 20.0).clamp(1.0, 12.0)
}

/// dbdiagram's row icons, as SVG path data lifted from their renderer
/// (Path2D strings), authored on a 10.5px grid.
/// The header-color affordance (shared with group headers): three
/// overlapping circles (green/red/blue) on a 578x531 viewBox, the
/// classic color-wheel mark. Drawn directly (the source IS three
/// circles) at the header's right end while hovered.
pub(crate) const CIRCLES_ICON: [(f32, f32, f32, Color); 3] = [
    // (cx, cy, r, fill) in viewBox units
    (
        166.0,
        366.0,
        166.0,
        Color::from_rgb(0x6D as f32 / 255.0, 0xBB as f32 / 255.0, 0x68 as f32 / 255.0),
    ),
    (
        412.0,
        366.0,
        166.0,
        Color::from_rgb(0xFF as f32 / 255.0, 0x41 as f32 / 255.0, 0x46 as f32 / 255.0),
    ),
    (
        277.0,
        165.0,
        166.0,
        Color::from_rgb(0x00 as f32 / 255.0, 0x9F as f32 / 255.0, 0xF3 as f32 / 255.0),
    ),
];
pub(crate) const CIRCLES_VIEWBOX: (f32, f32) = (578.0, 531.0);

/// The SVG path data for one row-icon glyph (shared with the hover docs).
/// dbdiagram's unique fingerprint icon (their QTn), lifted verbatim
/// from their bundle (minified: implicit command repetition).
///
/// dbdiagram's note page icon (their Exn).
/// The records affordance glyph (a table with rows), used at the right
/// end of the header of a records-bearing table on hover, on an 18px grid.
const TABLE_RING_ICON: &str = "M18 2c0-1.103-.897-2-2-2H2c-1.103 0-2 .897-2 2v14c0 1.103.897 2 2 2h14c1.103 0 2-.897 2-2zM2 16V2h14l.002 14z";
const TABLE_ROWS_ICON: &str = "M4 4h1.998v2H4zm4 0h6v2h-6zm-4 4h1.998v2H4zm4 0h6v2h-6zm-4 4h1.998v2H4zm4 0h6v2h-6z";
/// Tokenize SVG path data into (command, params) pairs.
fn svg_path_cmds(data: &str) -> Vec<(u8, Vec<f32>)> {
    let b = data.as_bytes();
    let (mut i, mut out) = (0usize, Vec::new());
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() {
            out.push((c, Vec::new()));
            i += 1;
        } else if c == b' ' || c == b',' {
            i += 1;
        } else {
            let start = i;
            if matches!(b[i], b'-' | b'+') {
                i += 1;
            }
            // compact SVG notation glues the next number's dot to the
            // previous one ("1.103.897" = 1.103, .897) — one dot per number
            let mut dot = false;
            while i < b.len()
                && (b[i].is_ascii_digit() || (b[i] == b'.' && !dot))
            {
                dot |= b[i] == b'.';
                i += 1;
            }
            if let Some(last) = out.last_mut() {
                last.1
                    .push(std::str::from_utf8(&b[start..i]).unwrap().parse().unwrap());
            }
        }
    }
    out
}

/// Parameter values per SVG command group (the implicit-repetition unit:
/// a command's groups repeat until its data runs out). 0 = no params.
fn svg_group(cmd: u8) -> usize {
    match cmd.to_ascii_uppercase() {
        b'M' | b'L' => 2,
        b'H' | b'V' => 1,
        b'C' => 6,
        b'S' => 4,
        _ => 0,
    }
}

/// Fill an icon authored as SVG path data (dbdiagram's Path2D strings),
/// scaled from its `grid` to `size` px and centered on `center`.
pub(crate) fn svg_icon(frame: &mut canvas::Frame, center: Point, size: f32, grid: f32, color: Color, data: &str) {
    let k = size / grid;
    let px = |x: f32, y: f32| Point::new(center.x - size / 2.0 + x * k, center.y - size / 2.0 + y * k);
    let cmds = svg_path_cmds(data);
    let path = Path::new(|b| {
        let (mut x, mut y, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        // previous cubic's second control, for the S/s smooth shorthand
        let (mut qx, mut qy) = (0.0f32, 0.0f32);
        for &(cmd, ref v) in &cmds {
            let upper = cmd.to_ascii_uppercase();
            let rel = cmd.is_ascii_lowercase();
            if svg_group(cmd) == 0 {
                if matches!(cmd, b'Z' | b'z') {
                    b.close();
                    x = sx;
                    y = sy;
                    // for non-cubics the smooth-reflection anchor is the
                    // point itself
                    qx = x;
                    qy = y;
                }
                continue;
            }
            let group = svg_group(cmd);
            let mut idx = 0usize;
            // implicit repetition: the command's parameter groups repeat
            // until its data runs out (dbdiagram's minified icons rely on
            // it); an M/m's later groups are plain lines (SVG grammar)
            while idx + group <= v.len() {
                let take = |idx: &mut usize| -> f32 {
                    let val = v.get(*idx).copied().unwrap_or(0.0);
                    *idx += 1;
                    val
                };
                match upper {
                    b'M' if idx == 0 => {
                        if rel {
                            x += take(&mut idx);
                            y += take(&mut idx);
                        } else {
                            x = take(&mut idx);
                            y = take(&mut idx);
                        }
                        sx = x;
                        sy = y;
                        b.move_to(px(x, y));
                    }
                    b'M' | b'L' => {
                        if rel {
                            x += take(&mut idx);
                            y += take(&mut idx);
                        } else {
                            x = take(&mut idx);
                            y = take(&mut idx);
                        }
                        b.line_to(px(x, y));
                    }
                    b'H' => {
                        let nx = take(&mut idx);
                        x = if rel { x + nx } else { nx };
                        b.line_to(px(x, y));
                    }
                    b'V' => {
                        let ny = take(&mut idx);
                        y = if rel { y + ny } else { ny };
                        b.line_to(px(x, y));
                    }
                    b'C' => {
                        let (mut x1, mut y1) = (take(&mut idx), take(&mut idx));
                        let (mut x2, mut y2) = (take(&mut idx), take(&mut idx));
                        if rel {
                            x1 += x;
                            y1 += y;
                            x2 += x;
                            y2 += y;
                        }
                        if rel {
                            x += take(&mut idx);
                            y += take(&mut idx);
                        } else {
                            x = take(&mut idx);
                            y = take(&mut idx);
                        }
                        b.bezier_curve_to(px(x1, y1), px(x2, y2), px(x, y));
                        qx = x2;
                        qy = y2;
                    }
                    b'S' => {
                        let (mut x2, mut y2) = (take(&mut idx), take(&mut idx));
                        let (mut ex, mut ey) = (take(&mut idx), take(&mut idx));
                        if rel {
                            x2 += x;
                            y2 += y;
                            ex += x;
                            ey += y;
                        }
                        b.bezier_curve_to(
                            px(2.0 * x - qx, 2.0 * y - qy),
                            px(x2, y2),
                            px(ex, ey),
                        );
                        x = ex;
                        y = ey;
                        qx = x2;
                        qy = y2;
                    }
                    _ => {}
                }
                // for non-cubics the smooth-reflection anchor is the point itself
                if !matches!(upper, b'C' | b'S') {
                    qx = x;
                    qy = y;
                }
            }
        }
    });
    frame.fill(&path, color);
}

#[allow(clippy::too_many_arguments)]
fn paint_table(
    frame: &mut canvas::Frame,
    b: &TableBox,
    zoom: f32,
    pan: Vector,
    hovered: bool,
    tinted: &[usize],
    at: Point,
    text_pass: bool,
) {
    let s = cell_size(zoom);
    // `at` is the box's frame-px origin in the target frame: to_px(b.x,
    // b.y) normally, shifted when painting under a clip sliver.
    // `text_pass`: iced draws ALL canvas text above ALL canvas geometry
    // (Frame::fill_text's warning), so a table paints in TWO passes —
    // shapes (body, band, tints, icons: correct draw-order z, an
    // occluder's body covers them) and text (title, names, pills: bound
    // to whatever clip the caller set). A full table = shapes pass +
    // text pass back to back.
    let top_left = at;
    let size = Size::new(b.w as f32 * s.width, b.h as f32 * s.height);

    // ALL shared chrome — shadow, body, band, borders, hover glyphs — is
    // SHAPES-PASS ONLY. The text pass must draw strictly text: it runs a
    // second time after the shapes pass (and, when covered, once per clip
    // sliver), and an opaque body repaint there would blank the shapes
    // pass's icons, tints and pill backgrounds (exactly the regression
    // where uncovered tables lost their icons until covered).
    if !text_pass {
    // soft drop shadow: the canvas has no blur, so two stacked offset
    // fills fake the falloff (dbdiagram boxes float on the canvas)
    let sh = 0.22 * s.height;
    frame.fill_rectangle(
        top_left + Vector::new(0.45 * sh, 0.8 * sh),
        size,
        Color::from_rgba(0.0, 0.0, 0.0, 0.30),
    );
    frame.fill_rectangle(
        top_left + Vector::new(0.9 * sh, 1.6 * sh),
        size,
        Color::from_rgba(0.0, 0.0, 0.0, 0.13),
    );

    // body: sharp corners, thin darker border (dbdiagram's boxes are square)
    let body = Path::rectangle(top_left, size);
    frame.fill(&body, theme::TABLE_BG);
    let border_w = if hovered {
        2.0
    } else {
        (zoom / 20.0).clamp(1.0, 2.5)
    };
    // header band first: solid near-black, full-width, with a hairline
    // below it — the body border strokes LAST so the band never thins it
    let band = Size::new(size.width, s.height);
    frame.fill_rectangle(top_left, band, theme::readable_band(header_color(b)));
    frame.stroke(
        &body,
        Stroke {
            style: if hovered {
                theme::TABLE_HOVER_BORDER
            } else {
                theme::TABLE_BORDER
            }
            .into(),
            width: border_w,
            ..Default::default()
        },
    );
    frame.stroke(
        &Path::new(|bd| {
            bd.move_to(Point::new(top_left.x, top_left.y + s.height));
            bd.line_to(Point::new(top_left.x + size.width, top_left.y + s.height));
        }),
        Stroke {
            style: theme::TABLE_BORDER.into(),
            width: 1.0,
            ..Default::default()
        },
    );
    } // !text_pass (shared chrome)

    // records affordance: tables with sample rows carry the rows glyph
    // ALWAYS visible, in the title-adjacent glyph row — right of the
    // note glyph when the table has one, else right of the title (press
    // opens the record view). Geometry: shapes pass; records_band is the
    // single source of the placement. The band under the chrome is
    // readability-clamped (readable_band), so the near-white ink reads
    // on ANY user hex.
    if b.records.is_some() && !text_pass {
        if let Some((rx0, rx1)) = records_band(b, zoom, pan) {
            let center = Point::new((rx0 + rx1) / 2.0, top_left.y + s.height * 0.5);
            let size = 0.55 * s.height;
            svg_icon(frame, center, size, 18.0, theme::TEXT, TABLE_RING_ICON);
            svg_icon(frame, center, size, 18.0, theme::TEXT, TABLE_ROWS_ICON);
        }
    }

    // header-color affordance (dbdiagram's gif): the palette glyph at the
    // header's right end while hovered — CLICKING it opens the color menu
    if hovered && !text_pass {
        let s1 = cell_size(zoom);
        let (c, _, _) = palette_icon_geometry(b, top_left, s1);
        // the circle wheel, right-aligned to the header's inset edge
        let icon_h = 0.72 * s1.height;
        let k = icon_h / CIRCLES_VIEWBOX.1;
        let x0 = c.x - CIRCLES_VIEWBOX.0 * k * 0.5;
        let y0 = c.y - icon_h * 0.5;
        for (cx, cy, r, color) in CIRCLES_ICON {
            frame.fill(
                &Path::circle(Point::new(x0 + cx * k, y0 + cy * k), r * k),
                color,
            );
        }
    }

    // note affordance (dbdiagram): tables with a note or custom
    // properties carry the paper glyph right AFTER THE TITLE — the
    // affordance belongs to the name. Always visible (hovering the
    // header pops the card); the records glyph rides on its right. The
    // ink tracks the band color.
    if let Some((note_icon_cx, note_icon_size)) = note_icon_geometry(b, top_left, s) {
        if !text_pass {
            svg_icon(
                frame,
                Point::new(note_icon_cx, top_left.y + s.height * 0.5),
                note_icon_size,
                10.5,
                theme::TEXT,
                NOTE_ICON,
            );
        }
    }
    // title, bold white, inset from the left edge by the same 0.6-cell
    // inset the group-header labels use (the pictogram that occupied
    // 1.4 cells here is gone — the wide indent read as an accidental
    // tab). Text draws in the text pass (the canvas's top text layer).
    let title_size = 0.55 * s.height;
    if text_pass {
    frame.fill_text(Text {
        content: b.title.clone(),
        position: Point::new(top_left.x + 0.6 * s.width, top_left.y + s.height * 0.5),
        // the name stays WHITE: the band itself is readability-clamped
        // (readable_band) so white chrome reads on any user hex
        color: Color::WHITE,
        size: title_size.into(),
        font: Font {
            weight: font::Weight::Bold,
            ..Font::DEFAULT
        },
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Default::default()
    });
    }

    // column rows: name at the left, gray outline marker right AFTER the
    // name (dbdiagram's placement), type right-aligned in muted gray
    let row_size = 0.55 * s.height;
    for (i, row) in b.rows.iter().enumerate() {
        let y = top_left.y + (i as f32 + 1.0) * s.height + s.height * 0.5;
        if tinted.contains(&i) && !text_pass {
            // dbdiagram tints hovered column rows — and the far end of a
            // hovered relationship — edge-to-edge inside the border
            frame.fill_rectangle(
                Point::new(top_left.x + 1.0, top_left.y + (i as f32 + 1.0) * s.height),
                Size::new(size.width - 2.0, s.height),
                theme::ROW_HOVER_BG,
            );
        }
        let baseline = |x: f32| Point::new(x, y);
        // dbdiagram renders pk column names bold
        let name_font = if row.pk {
            Font {
                weight: font::Weight::Bold,
                ..Font::DEFAULT
            }
        } else {
            Font::DEFAULT
        };
        if text_pass {
        frame.fill_text(Text {
            content: row.name.clone(),
            position: baseline(top_left.x + 1.0 * s.width),
            color: theme::ROW_TEXT,
            size: row_size.into(),
            font: name_font,
            horizontal_alignment: iced::alignment::Horizontal::Left,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
        }
        // dbdiagram's left cluster: pk, fk, unique, note in order — 10.5px
        // glyphs (the unique one their QTn fingerprint), each with a 4px
        // gap
        if !text_pass {
        for (icon, slot) in row_icon_layout(b, i, zoom, pan) {
            let path = row_icon_path(icon);
            svg_icon(
                frame,
                Point::new(slot + 5.25 * zoom / 20.0, y),
                10.5 * zoom / 20.0,
                10.5,
                theme::MARKER,
                path,
            );
        }
        }
        // right side: type text, then the "Enum" / "Not Null" pills flush
        // right; the type's right edge shifts left of both pills (their
        // renderType plus the enum pill)
        let nn_w = if row.not_null == Some(true) {
            Row::badge_cell("Not Null") * s.width
        } else {
            0.0
        };
        let en_w = if row.ty_enum {
            Row::badge_cell("Enum") * s.width
        } else {
            0.0
        };
        let right = top_left.x + size.width - 0.5 * s.width - nn_w - en_w;
        if !row.ty.is_empty() && text_pass {
            frame.fill_text(Text {
                content: row.ty.clone(),
                position: baseline(right),
                color: theme::TYPE_TEXT,
                size: row_size.into(),
                font: Font::DEFAULT,
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        if row.ty_enum {
            badge_pill(frame, "Enum", right + 0.3 * s.width, y, s, text_pass);
        }
        if row.not_null == Some(true) {
            badge_pill(
                frame,
                "Not Null",
                right + en_w + 0.3 * s.width,
                y,
                s,
                text_pass,
            );
        }
    }
}

/// A subtle text badge pill ("Enum" / "Not Null"), left edge at `x`,
/// vertically centered on the row line.
fn badge_pill(
    frame: &mut canvas::Frame,
    label: &str,
    x: f32,
    y: f32,
    s: Size,
    text_pass: bool,
) {
    let tw = label.chars().count() as f32 * BADGE_CHAR * s.width;
    let pad = BADGE_PAD * s.width;
    let rect = Rectangle::new(
        Point::new(x, y - 0.275 * s.height),
        Size::new(tw + 2.0 * pad, 0.55 * s.height),
    );
    if !text_pass {
    frame.fill(
        &Path::rounded_rectangle(
            rect.position(),
            rect.size(),
            iced::border::Radius::from(0.12 * s.height),
        ),
        Color::from_rgba(0.545, 0.565, 0.612, 0.14),
    );
    }
    // the label is CENTERED in the pill: the width reservation is an
    // estimate, and a left-aligned label drifted right when it ran narrow.
    // Vertical: Vertical::Center centers the LINE-HEIGHT box, whose
    // baseline puts the label ink ~1.6px above the pill's center at 1:1 —
    // the label reads pinned to the top. The bias is a fixed raster
    // metric (verified ~-1.5px at zoom 20 and ~-1.75px at zoom 56), so
    // the correction is a CONSTANT px nudge, not zoom-scaled — scaling it
    // over-corrected at high zoom and pushed the label below center.
    if text_pass {
    frame.fill_text(Text {
        content: label.to_string(),
        position: Point::new(rect.center().x, y + 1.6),
        color: theme::TYPE_TEXT,
        size: (0.35 * s.height).into(),
        font: Font::DEFAULT,
        horizontal_alignment: iced::alignment::Horizontal::Center,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Default::default()
    });
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse("Table a { id integer [pk] }").doc,
        ))
    }

    #[test]
    fn dash_windows_tile_and_clip() {
        // the dash pattern tiles the drawable window; increasing the phase
        // shifts windows toward the path start (canvas lineDashOffset)
        let w = dash_windows(0.0, 43.0, 3.0, 0.0, 200.0);
        assert_eq!(w[0], (0.0, 3.0));
        assert_eq!(w[1], (43.0, 46.0));
        // phase shifts the tiling back
        let w = dash_windows(2.0, 43.0, 3.0, 0.0, 200.0);
        assert_eq!(w[0], (0.0, 1.0));
        assert_eq!(w[1], (41.0, 44.0));
        // windows clip at both ends of the drawable range — the marker
        // gap trims mean a dash at the boundary is cut, never wrapped
        let w = dash_windows(0.0, 43.0, 3.0, 16.0, 18.0);
        assert_eq!(w, vec![(16.0, 18.0)]);
        // an empty window yields nothing
        assert!(dash_windows(0.0, 43.0, 3.0, 10.0, 10.0).is_empty());
    }

    #[test]
    fn flattened_path_measures_and_follows_corners() {
        // an L-shaped route with a fillet: two 40px straight runs joined
        // by the corner quadratic (~16px), total ≈ 96
        let pts = [
            Point::new(0.0, 0.0),
            Point::new(50.0, 0.0),
            Point::new(50.0, 50.0),
        ];
        let flat = flatten_pieces(&rounded_pieces(&pts, 10.0));
        assert!(
            flat.total > 94.0 && flat.total < 98.0,
            "total: {}",
            flat.total
        );
        // a point 20px along sits on the horizontal run, before the corner
        let p = flat_point(&flat, 20.0);
        assert_eq!(p, Point::new(20.0, 0.0));
        // a point inside the corner bend sits on the quadratic, which hugs
        // the corner through ~(47.5, 2.5) at its midpoint: the flow's
        // dashes ride this curve instead of cutting it
        let bend = flat_point(&flat, 48.0);
        assert!(
            (bend.x - 50.0).abs() < 4.0 && bend.y > 1.0 && bend.y < 12.0,
            "bend point: {bend:?}"
        );
        // endpoints survive
        assert_eq!(flat_point(&flat, 0.0), pts[0]);
        assert_eq!(flat_point(&flat, flat.total), pts[2]);
    }

    #[test]
    fn marker_hit_tests_the_icon_band() {
        // fixture: Table a { id integer [pk] } — one pk row
        let l = layout();
        let zoom = 20.0;
        let pan = Vector::new(50.0, 50.0);
        let b = &l.boxes[0];
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        let row_top = top_left.y + s.height;
        // the pk icon is the first slot of the cluster, right after the
        // name "id": its center must hit
        let (icon, slot) = row_icon_layout(b, 0, zoom, pan)
            .into_iter()
            .next()
            .expect("pk icon present");
        assert_eq!(icon, RowIcon::Pk);
        let p = Point::new(slot + 5.25 * zoom / 20.0, y_of(b, 0, zoom, pan));
        let hit = marker_hit(&l, zoom, pan, p).expect("pk marker hit");
        assert_eq!(hit.0.title, "a");
        assert!(hit.0.rows[hit.1].pk);
        assert_eq!(marker_label(true), "Primary Key");
        // the name text (left of the cluster) is NOT a marker
        let name_p = Point::new(top_left.x + 1.2 * s.width, row_top + s.height * 0.5);
        assert!(marker_hit(&l, zoom, pan, name_p).is_none());
        // left of the box padding misses too
        let left_p = Point::new(top_left.x + 0.3 * s.width, row_top + s.height * 0.5);
        assert!(marker_hit(&l, zoom, pan, left_p).is_none());
        // and the cluster tracks the name width: "id" spans 1.0..~2.16
        // cells (2 chars at 0.58), so the icon starts past that
        assert!(
            slot > top_left.x + 2.16 * s.width,
            "cluster after the name: {slot}"
        );
    }

    #[test]
    fn unique_rows_carry_the_fingerprint_in_the_cluster() {
        // fixture: Table a { id integer [pk, unique] email varchar }
        let src = "Table a { id integer [pk, unique] email varchar }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = &l.boxes[0];
        let icons = row_icon_layout(b, 0, zoom, pan);
        // pk first, then the unique fingerprint, each slot advancing by
        // the standard glyph step (10.5px glyph + 4px gap) — no text-pill
        // width in the cluster anymore
        assert_eq!(icons.len(), 2, "pk + unique on row 0");
        assert_eq!(icons[0].0, RowIcon::Pk);
        assert_eq!(icons[1].0, RowIcon::Unique);
        let step = (icons[1].1 - icons[0].1) / (zoom / 20.0);
        assert!((step - 14.5).abs() < 0.01, "standard glyph advance: {step}");
        // the fingerprint has no marker tooltip of its own (markers stay
        // pk/fk), and row 1 carries no cluster at all
        let p = Point::new(icons[1].1 + 5.25 * zoom / 20.0, y_of(b, 0, zoom, pan));
        assert!(marker_hit(&l, zoom, pan, p).is_none());
        assert!(row_icon_layout(b, 1, zoom, pan).is_empty());
    }

    #[test]
    fn icon_tooltips_name_the_exact_glyph_under_the_pointer() {
        // fixture: id is pk AND fk (inline ref), email is unique with a
        // note — a pk+fk row must label its two markers separately (the
        // old row-level label showed "Primary Key" for both), and the
        // unique fingerprint gets its own tooltip
        let src = "Table a { id integer [pk, ref: > b.id] email varchar [unique, note: 'hi'] }
\
                   Table b { id integer }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = l
            .boxes
            .iter()
            .find(|b| b.title == "a")
            .expect("table a laid out");
        assert!(b.rows[0].pk && b.rows[0].fk, "id is pk + fk");
        assert!(b.rows[1].unique && b.rows[1].note.is_some());
        let slot_of = |row: usize, icon: RowIcon| {
            row_icon_layout(b, row, zoom, pan)
                .into_iter()
                .find(|(ic, _)| *ic == icon)
                .map(|(_, slot)| slot)
                .expect("icon present")
        };
        let hover = |row: usize, slot: f32| {
            icon_tooltip_line(
                &l,
                zoom,
                pan,
                Point::new(slot + 5.25 * zoom / 20.0, y_of(b, row, zoom, pan)),
            )
        };
        // the fk slot (second) says "Foreign Key" even though the row is pk
        assert_eq!(
            hover(0, slot_of(0, RowIcon::Fk)).as_deref(),
            Some("Foreign Key")
        );
        // the pk slot (first) says "Primary Key"
        assert_eq!(
            hover(0, slot_of(0, RowIcon::Pk)).as_deref(),
            Some("Primary Key")
        );
        // the unique fingerprint has its own tooltip now
        assert_eq!(hover(1, slot_of(1, RowIcon::Unique)).as_deref(), Some("Unique"));
        // the note glyph never pops a pill — whatever raised it (note /
        // custom properties) is already shown by the field card
        assert_eq!(hover(1, slot_of(1, RowIcon::Note)), None);
        // and the hit contract is unchanged: unique/note slots are no
        // marker (marker_hit), the pk/fk slots are
        let uniq_p = Point::new(
            slot_of(1, RowIcon::Unique) + 5.25 * zoom / 20.0,
            y_of(b, 1, zoom, pan),
        );
        assert!(marker_hit(&l, zoom, pan, uniq_p).is_none());
        let pk_p = Point::new(
            slot_of(0, RowIcon::Pk) + 5.25 * zoom / 20.0,
            y_of(b, 0, zoom, pan),
        );
        assert!(marker_hit(&l, zoom, pan, pk_p).is_some());
    }

    #[test]
    fn glyph_tooltips_compose_with_the_field_card() {
        // The overlay draws the field card first, the hovered glyph's
        // tooltip pill on top: on a custom-properties row the pk glyph
        // must pop BOTH (the pill no longer hides the card), while the
        // note glyph — raised by the custom property, no `note` key —
        // pops NO pill (the card's Custom Properties section is the
        // affordance there). A plain pk row keeps the pill alone: no
        // card ever qualifies for it.
        let src = "Table a { id integer [pk, owner: \"x\"] email integer [pk] }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = &l.boxes[0];
        assert!(b.rows[0].note.is_none() && !b.rows[0].custom.is_empty());
        assert!(b.rows[1].pk && b.rows[1].custom.is_empty());
        let pill_at = |row: usize, icon: RowIcon| {
            let slot = row_icon_layout(b, row, zoom, pan)
                .into_iter()
                .find(|(ic, _)| *ic == icon)
                .expect("icon present")
                .1;
            icon_tooltip_line(
                &l,
                zoom,
                pan,
                Point::new(slot + 5.25 * zoom / 20.0, y_of(b, row, zoom, pan)),
            )
        };
        // pk glyph on the custom-props row: pill AND card together
        assert_eq!(
            pill_at(0, RowIcon::Pk).as_deref(),
            Some("Primary Key"),
            "the glyph keeps its pill where the card also pops"
        );
        assert!(row_has_field_card(&b.rows[0]), "the card stays up too");
        // the note glyph on the SAME row pops no pill — the field card
        // is the custom-properties affordance
        assert!(pill_at(0, RowIcon::Note).is_none());
        assert!(row_has_field_card(&b.rows[0]));
        // a plain pk row keeps today's behavior: the pill alone, no card
        assert_eq!(pill_at(1, RowIcon::Pk).as_deref(), Some("Primary Key"));
        assert!(!row_has_field_card(&b.rows[1]));
        // away from the glyphs the helper yields — the rest of the row
        // owns the card
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        let name_p = Point::new(top_left.x + 1.2 * s.width, top_left.y + 1.5 * s.height);
        assert!(icon_tooltip_line(&l, zoom, pan, name_p).is_none());
        assert!(row_has_field_card(&b.rows[0]));
    }

    #[test]
    fn box_rect_matches_the_paint_bounds() {
        let src = "Table a { id int }
Table b { x int }";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(0.0, 0.0);
        let a = box_rect(&l.boxes[0], zoom, pan);
        let s = cell_size(zoom);
        assert_eq!(
            a.size(),
            Size::new(l.boxes[0].w as f32 * s.width, l.boxes[0].h as f32 * s.height)
        );
    }

    #[test]
    fn visible_slivers_decompose_the_covered_region() {
        // a 20x4 table fully overlapped in x by an occluder spanning the
        // middle: left + right slivers carry the full height, top/bottom
        // slivers span only the horizontal overlap band
        let rect = Rectangle::new(Point::new(0.0, 0.0), Size::new(20.0, 8.0));
        let occ = Rectangle::new(Point::new(5.0, 2.0), Size::new(6.0, 3.0));
        let sl = visible_slivers(rect, occ);
        // left: 5 wide, full height; right: 9 wide, full height
        assert!(sl.iter().any(|s| s.x == 0.0 && s.width == 5.0 && s.height == 8.0));
        assert!(sl.iter().any(|s| s.x == 11.0 && s.width == 9.0 && s.height == 8.0));
        // top: 6 wide, 2 tall; bottom: 6 wide, 3 tall
        assert!(sl.iter().any(|s| s.x == 5.0 && s.y == 0.0 && s.width == 6.0 && s.height == 2.0));
        assert!(sl.iter().any(|s| s.x == 5.0 && s.y == 5.0 && s.width == 6.0 && s.height == 3.0));
        // the slivers never intersect the occluder
        for sl in &sl {
            assert!(!sl.intersects(&occ), "{sl:?} overlaps the occluder");
        }
        // a disjoint occluder leaves the whole rect visible (the paint
        // loop never calls it without an intersection)
        let sl = visible_slivers(rect, Rectangle::new(Point::new(50.0, 50.0), Size::new(5.0, 5.0)));
        assert_eq!(sl.len(), 1);
        assert_eq!(sl[0], rect);
    }

    #[test]
    fn the_topmost_item_wins_the_hit() {
        // the raise order decides who a click belongs to: a sticky over
        // a table hits the sticky; raise the table and the TABLE wins —
        // the click must not fall through to the sticky beneath. Corner
        // envelopes belong to the topmost note too.
        let src = "Table a { id integer [pk] }
Note n { '''x''' }";
        // pin the table and the sticky overlapping (note_pos pins the
        // sticky, table_pos the table — the same maps drags fill), so
        // neither moves under the other
        let mut tpos = std::collections::HashMap::new();
        tpos.insert("a".to_string(), (22, 22));
        let mut npos = std::collections::HashMap::new();
        npos.insert("n".to_string(), (20, 22, 14, 5));
        let l = dbml_ui::layout::layout_with_all_overrides(
            &dbml_ui::model::build(&dbml_ui::dbml::parser::parse(src).doc),
            &tpos,
            &npos,
        );
        let (zoom, pan) = (20.0, Vector::default());
        // the press point: inside BOTH the sticky (20..34, 22..27) and
        // the table (22..32, 22..24 — header + its one row)
        let p = to_px(23, 23, zoom, pan);
        // static layering: the sticky floats above the untouched table
        let hit = hit_topmost(&l, zoom, pan, p, &[]);
        assert!(
            matches!(hit, Some(PaintItem::Note(_))),
            "the sticky floats above an unraised table"
        );
        // the table raised: it wins the same press
        let z = vec![RaisedItem::Table("a".into())];
        let hit = hit_topmost(&l, zoom, pan, p, &z);
        assert!(
            matches!(hit, Some(PaintItem::Table(_))),
            "the raised table keeps its press priority"
        );
        // and the sticky re-grabbed takes it back
        let z = vec![RaisedItem::Table("a".into()), RaisedItem::Note("n".into())];
        let hit = hit_topmost(&l, zoom, pan, p, &z);
        assert!(matches!(hit, Some(PaintItem::Note(_))), "the last grab wins");
    }

    #[test]
    fn stacked_stickies_hit_the_topmost_one() {
        // two stickies piled on the same spot: the LAST-raised one wins
        // the press — a doc-order note scan returned the lower one and
        // the click fell straight through the top sticky
        let src = "Note a { '''x''' }\nNote b { '''y''' }\n";
        let build = || {
            dbml_ui::model::build(&dbml_ui::dbml::parser::parse(src).doc)
        };
        let mut npos = std::collections::HashMap::new();
        npos.insert("a".to_string(), (5, 5, 14, 5));
        npos.insert("b".to_string(), (6, 6, 14, 5)); // overlapping a
        let l = dbml_ui::layout::layout_with_all_overrides(
            &build(),
            &Default::default(),
            &npos,
        );
        let (zoom, pan) = (20.0, Vector::default());
        // inside BOTH bodies
        let p = to_px(10, 8, zoom, pan);
        fn name_of<'a>(hit: &'a Option<PaintItem<'a>>) -> &'a str {
            match hit {
                Some(PaintItem::Note(n)) => n.name.as_str(),
                _ => "?",
            }
        }
        // no raises: schema order puts b on top
        assert_eq!(name_of(&hit_topmost(&l, zoom, pan, p, &[])), "b");
        // a raised takes the top...
        let z = vec![RaisedItem::Note("a".into())];
        assert_eq!(name_of(&hit_topmost(&l, zoom, pan, p, &z)), "a");
        // ...and hands it back when the other re-grabs
        let z = vec![RaisedItem::Note("a".into()), RaisedItem::Note("b".into())];
        assert_eq!(name_of(&hit_topmost(&l, zoom, pan, p, &z)), "b");
    }

    #[test]
    fn visible_region_subtracts_every_occluder() {
        // three stacked tables: the bottom table's text must hide under
        // the middle one AND the top one — one-occluder slivers leaked
        // the bottom text through the middle table's body
        let rect = Rectangle::new(Point::new(0.0, 0.0), Size::new(100.0, 100.0));
        let middle = Rectangle::new(Point::new(0.0, 0.0), Size::new(60.0, 100.0));
        let top = Rectangle::new(Point::new(50.0, 0.0), Size::new(50.0, 50.0));
        let vis = visible_region(rect, &[middle, top]);
        // everything is covered except the bottom-right quadrant
        assert_eq!(vis.len(), 1, "{vis:?}");
        assert_eq!(vis[0], Rectangle::new(Point::new(60.0, 50.0), Size::new(40.0, 50.0)));
        // no visible piece ever intersects any occluder
        for v in &vis {
            assert!(!v.intersects(&middle) || !v.intersects(&top) || {
                // a piece may touch both only if it exits both — it cannot
                false
            });
        }
        // disjoint occluders leave the whole rect
        let vis = visible_region(rect, &[Rectangle::new(Point::new(200.0, 200.0), Size::new(5.0, 5.0))]);
        assert_eq!(vis, vec![rect]);
    }

    #[test]
    fn raised_tables_paint_after_every_other_box() {
        // doc order: a before b; raising b (a grab) moves it last —
        // unraised tables keep doc order, a stale raise (renamed table)
        // is harmless
        let src = "Table a { id int }
Table b { x int }";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        // b raised paints after a; raising a flips it; a raised NOTE
        // outranks raised tables grabbed BEFORE it, and vice versa
        let z = vec![RaisedItem::Table("b".into())];
        let seq = paint_items(&l, &z, None, None, None);
        let names: Vec<&str> = seq
            .iter()
            .map(|item| match item {
                PaintItem::Table(b) => b.title.as_str(),
                PaintItem::Note(n) => n.name.as_str(),
            })
            .collect();
        assert_eq!(names, vec!["a", "b"], "b raised paints after a");
        let z = vec![RaisedItem::Table("a".into())];
        let seq = paint_items(&l, &z, None, None, None);
        let names: Vec<&str> = seq
            .iter()
            .map(|item| match item {
                PaintItem::Table(b) => b.title.as_str(),
                PaintItem::Note(n) => n.name.as_str(),
            })
            .collect();
        assert_eq!(names, vec!["b", "a"], "a raised paints after b");
    }

    #[test]
    fn the_shared_raise_order_ranks_tables_and_stickies() {
        // the static base: tables in doc order, then stickies floating
        // above them; a grabbed TABLE then paints above every unraised
        // sticky; a sticky grabbed AFTER it takes the top back — the
        // last grab wins in both directions
        let src = "Table a { id int }
Note n { '''x''' }";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        fn names<'a>(seq: &'a [PaintItem<'_>]) -> Vec<&'a str> {
            seq.iter()
                .map(|item| match item {
                    PaintItem::Table(b) => b.title.as_str(),
                    PaintItem::Note(n) => n.name.as_str(),
                })
                .collect()
        }
        // no raises: sticky above the table
        assert_eq!(names(&paint_items(&l, &[], None, None, None)), vec!["a", "n"]);
        // the table grabbed: it outranks the unraised sticky
        let z = vec![RaisedItem::Table("a".into())];
        assert_eq!(names(&paint_items(&l, &z, None, None, None)), vec!["n", "a"]);
        // then the sticky grabbed: the last grab wins
        let z = vec![RaisedItem::Table("a".into()), RaisedItem::Note("n".into())];
        assert_eq!(names(&paint_items(&l, &z, None, None, None)), vec!["a", "n"]);
        // and a second table grab flips it again
        let z = vec![RaisedItem::Note("n".into()), RaisedItem::Table("a".into())];
        assert_eq!(names(&paint_items(&l, &z, None, None, None)), vec!["n", "a"]);
        // a raised NOTE paints before a table grabbed later, above one
        // grabbed earlier
        let src2 = "Table a { id int }
Table b { x int }
Note n { '''x''' }";
        let l2 = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src2).doc,
        ));
        let z = vec![
            RaisedItem::Note("n".into()),
            RaisedItem::Table("a".into()),
            RaisedItem::Table("b".into()),
        ];
        assert_eq!(names(&paint_items(&l2, &z, None, None, None)), vec!["n", "a", "b"]);
    }

    #[test]
    fn icon_paths_parse_into_whole_command_groups() {
        // every icon's path data must divide into whole per-command
        // parameter groups and use only supported commands — dbdiagram's
        // minified unique fingerprint relies on implicit repetition, which
        // the renderer expands
        for (name, data) in [
            ("pk", row_icon_path(RowIcon::Pk)),
            ("fk", row_icon_path(RowIcon::Fk)),
            ("unique", row_icon_path(RowIcon::Unique)),
            ("note", row_icon_path(RowIcon::Note)),
            ("table ring", TABLE_RING_ICON),
            ("table rows", TABLE_ROWS_ICON),
        ] {
            for (cmd, v) in svg_path_cmds(data) {
                let group = svg_group(cmd);
                if group == 0 {
                    assert!(
                        matches!(cmd, b'Z' | b'z') && v.is_empty(),
                        "{name}: unsupported command {} with params {v:?}",
                        cmd as char
                    );
                } else {
                    assert_eq!(
                        v.len() % group,
                        0,
                        "{name}: cmd {} params {v:?}",
                        cmd as char
                    );
                }
            }
        }
    }

    #[test]
    fn enum_pill_hit_tests_the_band_right_of_the_type() {
        let src = "Enum st { draft published archived }
Table t { s st [not null] n integer }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = &l.boxes[0];
        assert!(b.rows[0].ty_enum, "the enum-typed row carries the flag");
        assert!(!b.rows[1].ty_enum);
        assert_eq!(b.rows[0].ty, "st");
        let s = cell_size(zoom);
        let y = y_of(b, 0, zoom, pan);
        let (x0, x1) = enum_pill_band(b, 0, zoom, pan).expect("enum band");
        let hit = enum_hit(&l, zoom, pan, Point::new((x0 + x1) / 2.0, y))
            .expect("enum pill hit");
        assert_eq!(hit.0.title, "t");
        assert_eq!(hit.1, 0);
        // the band hugs the pill's drawn height: above/below the row line
        // misses
        let half = 0.30 * s.height;
        assert!(
            enum_hit(&l, zoom, pan, Point::new((x0 + x1) / 2.0, y - half - 2.0)).is_none(),
            "outside the pill's vertical band"
        );
        // left of the band (the type text) and right of it (the Not Null
        // pill's slot) miss too
        assert!(enum_hit(&l, zoom, pan, Point::new(x0 - 3.0, y)).is_none());
        assert!(
            enum_hit(&l, zoom, pan, Point::new(x1 + 3.0, y)).is_none(),
            "past the band's right edge"
        );
        // a row without the flag has no band
        assert!(enum_pill_band(b, 1, zoom, pan).is_none());
        // and the pill shifts the type text left of both pills: the type's
        // right edge is the enum band's left edge minus its padding
        assert!(x0 > to_px(b.x, b.y, zoom, pan).x, "band inside the box");
    }

    #[test]
    fn edge_strokes_never_drop_below_one_pixel() {
        assert_eq!(edge_stroke_width(160.0), 12.0, "proportional at 8x");
        assert_eq!(edge_stroke_width(20.0), 1.5, "proportional at 1:1");
        for zoom in [5.0, 8.0, 10.0, 13.0] {
            assert!(
                edge_stroke_width(zoom) >= 1.0,
                "zoom {zoom}: {}",
                edge_stroke_width(zoom)
            );
        }
        assert_eq!(edge_stroke_width(5.0), 1.0, "clamped at 25% zoom");
    }

    #[test]
    fn note_hit_tests_the_note_icon_slot() {
        let src = "Table a { id integer [pk, note: 'the identifier'] user_id integer }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = &l.boxes[0];
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        // row 0 has the note icon; it is the LAST slot of the cluster
        let icons = row_icon_layout(b, 0, zoom, pan);
        let (icon, slot) = icons.last().expect("note icon present");
        assert_eq!(*icon, RowIcon::Note);
        let p = Point::new(slot + 5.25 * zoom / 20.0, y_of(b, 0, zoom, pan));
        let hit = note_hit(&l, zoom, pan, p).expect("note icon hit");
        assert_eq!(hit.0.title, "a");
        assert_eq!(hit.1, 0);
        // row 1 has no note: no cluster slot, no hit
        assert!(row_icon_layout(b, 1, zoom, pan)
            .iter()
            .all(|(ic, _)| *ic != RowIcon::Note));
        let p2 = Point::new(*slot, top_left.y + s.height * 2.5);
        assert!(note_hit(&l, zoom, pan, p2).is_none());
        // the pk icon (first slot) does not collide with the note slot
        assert!(icons.len() >= 2, "pk + note on row 0");
    }

    #[test]
    fn records_icon_hit_tests_the_header_band() {
        let src = "Table a { id integer [pk] name varchar
  records (id, name) {
    1, 'x'
  }
}
Table b { c integer }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = l.boxes.iter().find(|b| b.title == "a").unwrap();
        assert!(b.records.is_some(), "fixture carries records");
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        // the rows glyph LEFT of the wheel's fixed rightmost slot
        let (x0, x1) = records_band(b, zoom, pan).expect("records band");
        let p = Point::new((x0 + x1) / 2.0, top_left.y + s.height * 0.5);
        let hit = records_icon_hit(&l, zoom, pan, p).expect("records affordance hit");
        assert_eq!(hit.title, "a");
        // the records slot is LEFT of the color wheel's slot
        let (wc, _, _) = palette_icon_geometry(b, top_left, s);
        assert!(
            (x0 + x1) / 2.0 < wc.x,
            "records ({}) left of the wheel ({})",
            (x0 + x1) / 2.0,
            wc.x
        );
        // body rows and the title area are not the affordance
        let body = Point::new((x0 + x1) / 2.0, top_left.y + s.height * 1.5);
        assert!(records_icon_hit(&l, zoom, pan, body).is_none());
        let title_area = Point::new(top_left.x + 3.0 * s.width, top_left.y + s.height * 0.5);
        assert!(records_icon_hit(&l, zoom, pan, title_area).is_none());
        // a table without records has no affordance anywhere
        let b2 = l.boxes.iter().find(|b| b.title == "b").unwrap();
        let t2 = to_px(b2.x, b2.y, zoom, pan);
        let r2 = t2.x + b2.w as f32 * s.width;
        let p2 = Point::new(r2 - 1.0 * s.width, t2.y + s.height * 0.5);
        assert!(records_icon_hit(&l, zoom, pan, p2).is_none());
        // and the affordance band does not swallow a row-0 note band (the
        // note sits a row below the header)
        assert!(note_hit(&l, zoom, pan, p).is_none());
    }

    #[test]
    fn badge_flags_follow_settings() {
        let src = "Table a { id integer [pk, not null, unique] email varchar(80) }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let r = &l.boxes[0].rows[0];
        assert_eq!(r.not_null, Some(true));
        assert!(r.unique);
        assert_eq!(l.boxes[0].rows[1].not_null, None);
        // null (explicit) is NOT the NN pill
        let src2 = "Table b { x integer [null] }
";
        let l2 = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src2).doc,
        ));
        assert_eq!(l2.boxes[0].rows[0].not_null, Some(false));
        // the NN pill widens an otherwise identical row
        let w_plain = {
            let src3 = "Table c { email varchar(80) }
";
            dbml_ui::layout::layout(&dbml_ui::model::build(
                &dbml_ui::dbml::parser::parse(src3).doc,
            ))
            .boxes[0]
                .w
        };
        let w_nn = {
            let src4 = "Table c { email varchar(80) [not null] }
";
            dbml_ui::layout::layout(&dbml_ui::model::build(
                &dbml_ui::dbml::parser::parse(src4).doc,
            ))
            .boxes[0]
                .w
        };
        assert!(
            w_nn > w_plain,
            "the NN pill adds ~1.4 cells (the ceil may absorb part): {w_nn} vs {w_plain}"
        );
    }

    #[test]
    fn a_line_highlights_in_the_referenced_tables_header_color() {
        // dbdiagram's highlight rule: a line with a color of its own —
        // an explicit `[color:]` or a non-dark inherited header —
        // highlights IN that color; a line resolving to the dark header
        // option (no inherited color at all, or the dark swatch) eases
        // to the highlight blue.
        let layout_of = |src: &str| {
            dbml_ui::layout::layout(&dbml_ui::model::build(
                &dbml_ui::dbml::parser::parse(src).doc,
            ))
        };
        let purple = theme::hex_color("#7a4f9e").unwrap();
        // likes.post_id -> posts (#7a4f9e): the purple case
        let l = layout_of(
            "Table posts [headercolor: #7a4f9e] { id integer [pk] }
Table likes { post_id integer }
Ref: likes.post_id > posts.id
",
        );
        assert_eq!(l.edges[0].inherited.as_deref(), Some("#7a4f9e"));
        assert_eq!(edge_hover_color(&l.edges[0]), brighten(purple, 1.15));
        // an explicit `[color:]` still owns the highlight
        let l = layout_of(
            "Table posts [headercolor: #7a4f9e] { id integer [pk] }
Table likes { post_id integer }
Ref: likes.post_id > posts.id [color: #123456]
",
        );
        assert_eq!(
            edge_hover_color(&l.edges[0]),
            brighten(theme::hex_color("#123456").unwrap(), 1.15)
        );
        // the dark option THROUGH THE DEFAULT CASCADE (no headercolor at
        // all): the highlight blue
        let l = layout_of(
            "Table posts { id integer [pk] }
Table likes { post_id integer }
Ref: likes.post_id > posts.id
",
        );
        assert_eq!(l.edges[0].inherited, None);
        assert_eq!(edge_hover_color(&l.edges[0]), theme::EDGE_HOVER);
        // the dark option THROUGH AN EXPLICIT PICK (the picker's dark
        // swatch): still the highlight blue
        let l = layout_of(
            "Table posts [headercolor: #1f2128] { id integer [pk] }
Table likes { post_id integer }
Ref: likes.post_id > posts.id
",
        );
        assert_eq!(l.edges[0].inherited.as_deref(), Some("#1f2128"));
        assert_eq!(edge_hover_color(&l.edges[0]), theme::EDGE_HOVER);
        // …case-insensitively (a hand-typed hex)
        let l = layout_of(
            "Table posts [headercolor: #1F2128] { id integer [pk] }
Table likes { post_id integer }
Ref: likes.post_id > posts.id
",
        );
        assert_eq!(edge_hover_color(&l.edges[0]), theme::EDGE_HOVER);
    }

    #[test]
    fn edge_hit_finds_the_line_under_the_pointer() {
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(
                "Table a { id integer [pk] }
Table b { a_id integer }
Ref: b.a_id > a.id
",
            )
            .doc,
        ));
        assert!(!l.edges.is_empty());
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let r = &l.edges[0];
        // midpoint of the first segment, converted to px
        let (x, y) = r.pts[0];
        let (x2, y2) = r.pts[1];
        let mx = (x + x2) / 2.0;
        let my = (y + y2) / 2.0;
        let s = cell_size(zoom);
        let p = Point::new(
            (mx + MARGIN_CELLS) * s.width + pan.x,
            (my + MARGIN_CELLS) * s.height + pan.y,
        );
        assert_eq!(edge_hit(&l, zoom, pan, p, 7.0), Some(0));
        // far away: nothing
        let far = Point::new(5000.0, 5000.0);
        assert_eq!(edge_hit(&l, zoom, pan, far, 7.0), None);
    }

    #[test]
    fn row_at_finds_rows_not_borders() {
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(
                "Table a { id integer [pk] }
Table b { a_id integer }
Ref: b.a_id > a.id
",
            )
            .doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = &l.boxes[0];
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        // middle of row 0 (id)
        let p = Point::new(top_left.x + 3.0 * s.width, top_left.y + s.height * 1.5);
        let (hit, row) = row_at(&l, zoom, pan, p).unwrap();
        assert_eq!(hit.title, "a");
        assert_eq!(row, 0);
        // header band is not a row
        let header = Point::new(top_left.x + 3.0 * s.width, top_left.y + s.height * 0.5);
        assert!(row_at(&l, zoom, pan, header).is_none());
    }

    #[test]
    fn transform_roundtrips() {
        let zoom = 24.0;
        let pan = Vector::new(13.0, -7.0);
        let p = to_px(4, 9, zoom, pan);
        assert_eq!(to_cell(p, zoom, pan), (4, 9));
        let inside = Point::new(p.x + 3.0, p.y + 11.0);
        assert_eq!(to_cell(inside, zoom, pan), (4, 9));
    }

    #[test]
    fn hit_test_finds_box() {
        let l = layout();
        let zoom = 20.0;
        let p = to_px(0, 0, zoom, Vector::new(50.0, 50.0)) + Vector::new(2.0, 2.0);
        let hit = hit_test(&l, zoom, Vector::new(50.0, 50.0), p);
        assert_eq!(hit.map(|b| b.title.as_str()), Some("a"));
        let miss = Point::new(5000.0, 5000.0);
        assert!(hit_test(&l, zoom, Vector::new(50.0, 50.0), miss).is_none());
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::app::Message;
    use iced::mouse::{self, Cursor};
    use iced::widget::canvas::Program as _;

    const BOUNDS: Rectangle = Rectangle::new(Point::new(500.0, 0.0), Size::new(600.0, 400.0));

    fn diagram() -> Diagram<'static> {
        let layout: &'static dbml_ui::layout::Layout =
            Box::leak(Box::new(dbml_ui::layout::layout(&dbml_ui::model::build(
                &dbml_ui::dbml::parser::parse(dbml_ui::SAMPLE_DBML).doc,
            ))));
        Diagram {
            layout,
            zoom: 20.0,
            pan: Vector::default(),
            hover: None,
            dragging: false,
            edge_anim: &[],
            clock: 0.0,
            selected_note: None,
            // the test constructor leaks an empty raise list (a Diagram
            // borrows its item_z for its lifetime)
            item_z: Box::leak(Vec::new().into_boxed_slice()),
            drag_group: None,
            drag_note: None,
            drag_edges: None,
        }
    }

    #[test]
    fn press_and_wheel_outside_bounds_are_ignored() {
        // regression: presses/wheels over the editor pane leaked in here
        let d = diagram();
        let outside = Point::new(100.0, 100.0);
        for event in [
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            canvas::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 2.0 },
            }),
        ] {
            let (_, message) = d.update(&mut (), event, BOUNDS, Cursor::Available(outside));
            assert!(message.is_none(), "{message:?}");
        }
    }

    #[test]
    fn events_inside_are_localized() {
        let d = diagram();
        let (_, message) = d.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            BOUNDS,
            Cursor::Available(Point::new(550.0, 30.0)),
        );
        match message {
            Some(Message::CanvasPressed(p)) => {
                assert_eq!(p, Point::new(50.0, 30.0), "bounds-relative");
            }
            other => panic!("expected CanvasPressed, got {other:?}"),
        }
    }

    #[test]
    fn drag_keeps_tracking_outside_bounds() {
        // pan continues past the canvas edge; release outside still lands
        let mut d = diagram();
        d.dragging = true;
        let outside = Point::new(100.0, 100.0);
        let (_, moved) = d.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::CursorMoved { position: outside }),
            BOUNDS,
            Cursor::Available(outside),
        );
        assert!(moved.is_some(), "drag tracks outside");
        let (_, released) = d.update(
            &mut (),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            BOUNDS,
            Cursor::Available(outside),
        );
        assert!(released.is_some(), "release outside ends the drag");
    }

    #[test]
    fn cursor_shape_matches_context() {
        use iced::mouse::Interaction;
        let mut d = diagram();
        // over a table box: the move cursor (drag = reposition)
        let (bx, by) = first_box_point();
        let over_box = Cursor::Available(Point::new(BOUNDS.x + bx, BOUNDS.y + by));
        assert_eq!(
            d.mouse_interaction(&(), BOUNDS, over_box),
            Interaction::Move
        );
        // empty canvas (past all content): the plain arrow, like dbdiagram
        let empty = Cursor::Available(Point::new(BOUNDS.x + 550.0, BOUNDS.y + 380.0));
        assert_eq!(d.mouse_interaction(&(), BOUNDS, empty), Interaction::default());
        // mid-drag: grabbing
        d.dragging = true;
        assert_eq!(
            d.mouse_interaction(&(), BOUNDS, empty),
            Interaction::Grabbing
        );
        // outside the canvas: no opinion (arrow). Grabbing DOES persist
        // outside mid-drag (pan continues past the edge), so un-drag first.
        d.dragging = false;
        assert_eq!(
            d.mouse_interaction(&(), BOUNDS, Cursor::Available(Point::new(10.0, 10.0))),
            Interaction::default()
        );
    }

    /// A bounds-relative point inside the first table box of SAMPLE_DBML.
    fn first_box_point() -> (f32, f32) {
        let layout = diagram().layout;
        let b = &layout.boxes[0];
        let tl = to_px(b.x, b.y, 20.0, Vector::default());
        (tl.x + 5.0, tl.y + 5.0)
    }
}

/// True when hovering the row shows the field card (dbdiagram's trigger:
/// note OR enum type OR default OR custom properties — verified against
/// their bundle's `t.enumId || t.note || t.dbdefault ...` condition and
/// live against the Custom Properties popup).
pub(crate) fn row_has_field_card(row: &Row) -> bool {
    row.note.is_some() || row.ty_enum || row.default.is_some() || !row.custom.is_empty()
}

// ---- shared popup model (field/table/group popups)

/// dbdiagram's canvas popovers (FieldTooltipPopup / TableTooltipPopup /
/// TableGroupTooltipPopup over their CanvasPopover), dark theme — ported
/// from their bundle + CSS 1:1: a content-hugging card (min-w 260,
/// max-w 360, r 8, bg #303137, hairline border, `0 8px 24px` shadow) with
/// a speech-bubble tail on the edge facing the anchor, a px-3 py-2.5
/// header band (#3a3c42: glyph + semibold name + mono type), then
/// sections: Note (pt-1 pb-2 label, pb-2.5 body), Enum ("Enum" + blue
/// name inline, tab-indented blue values, leading-relaxed), Default (one
/// py-2 row: label + orange value inline), Custom Properties (py-1
/// label, pl-4 disc bullets, pb-1.5). Hairline dividers after the
/// header, after the Note, and before Custom Properties — never between
/// Enum and Default (their template's exact divider placement). The body
/// caps at the pane height and clips (the wheel stays with the canvas
/// zoom, even over an open card).
enum PopupEntry {
    /// A text line (wrapped note lines, enum values).
    Plain(String, Color),
    /// A `• key: value` bullet — key gray-300, value orange-300 (their
    /// li spans), truncating instead of wrapping.
    Bullet(String, Color, String, Color),
}

struct PopupSection {
    label: &'static str,
    /// inline colored run after the label (the enum name, the default
    /// value — their `Enum {name}` header pair and Default row)
    label_sub: Option<(String, Color)>,
    entries: Vec<PopupEntry>,
    /// entry-line pitch, 1:1 px at zoom 20 (default vs leading-relaxed)
    pitch: f32,
    pad_top: f32,
    label_pad_bottom: f32,
    pad_bottom: f32,
    /// hairline divider above this section (their TooltipSectionDivider)
    divider_before: bool,
}

/// The Note section (their TooltipNoteSection: pt-1 pb-2 label + the
/// wrapped note at pb-2.5).
fn note_section(note: &str, wrap_cols: usize, divider_before: bool) -> PopupSection {
    PopupSection {
        label: "Note",
        label_sub: None,
        entries: wrap_chars(note, wrap_cols)
            .into_iter()
            .map(|l| PopupEntry::Plain(l, theme::TEXT))
            .collect(),
        pitch: POPUP_LINE_H,
        pad_top: 4.0,
        label_pad_bottom: 8.0,
        pad_bottom: 10.0,
        divider_before,
    }
}

/// The Custom Properties section (their TooltipCustomPropertySection:
/// py-1 label + pl-4 disc bullets + pb-1.5).
fn custom_props_section(
    custom: &[(String, String)],
    divider_before: bool,
) -> PopupSection {
    PopupSection {
        label: "Custom Properties",
        label_sub: None,
        entries: custom
            .iter()
            .map(|(k, v)| {
                PopupEntry::Bullet(
                    k.clone(),
                    Color::from_rgb8(0xd1, 0xd5, 0xdb), // gray-300
                    v.clone(),
                    TYPE_ORANGE,
                )
            })
            .collect(),
        pitch: POPUP_RELAXED_H,
        pad_top: 4.0,
        label_pad_bottom: 4.0,
        pad_bottom: 6.0,
        divider_before,
    }
}

const POPUP_PAD_X: f32 = 12.0; // their px-3
const POPUP_MIN_W: f32 = 260.0; // their min-width, 1:1 px
const POPUP_MAX_W: f32 = 360.0; // their max-width, 1:1 px
const POPUP_LINE_H: f32 = 18.0; // default 1.5 pitch on 12px text
const POPUP_RELAXED_H: f32 = 19.5; // leading-relaxed pitch on 12px text
const POPUP_HEADER_H: f32 = 40.0; // py-2.5 + the 20px text-sm line
const POPUP_TAB_INDENT: f32 = 13.2; // tab-size 4 ≈ 4 spaces on 12px text

/// Width of `text` at font size `size` px: the shared per-character
/// advance classes (Row::est_width) are calibrated on the row font
/// (0.55×zoom, `est × 0.5×zoom` px), so at any size the advance scales
/// linearly: `est × size / 1.1`.
fn est_px(text: &str, size: f32) -> f32 {
    Row::est_width(text) * size / 1.1
}

/// Width of `text` at font size `size` px in the SEMIBOLD header font:
/// semibold runs past the regular-calibrated classes (live-measured
/// "created_at" renders 1.13×, "referrer_id" 1.18× its regular estimate
/// — undershooting puts the type ON the name), so a 1.18 factor keeps
/// the type clear of every letter mix.
fn semibold_px(text: &str, size: f32) -> f32 {
    est_px(text, size) * 1.18
}

/// The bullet key's run width: the regular-weight classes undershoot a
/// little (live: "sla_hours" rendered past its estimate, putting the
/// value ON the colon), so the key carries a 1.08 factor — the value
/// keeps its colon+nbsp clearance on every key mix.
fn bullet_key_px(key: &str, k: f32) -> f32 {
    est_px(key, 12.0 * k) * 1.08
}

/// Width of `text` at font size `size` px in the monospace font —
/// live-measured advance ~0.545em (the old 0.6em guess overshot the
/// type's width by 13%).
fn mono_px(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * 0.545 * size
}

/// The hovered shape's rect the popover anchors to (their popper's
/// `rect` prop — the row's / header band's / group band's frame rect):
/// the card sits right-start of it (left edge = rect.right + 5), the
/// tail points at its vertical center.
struct PopupAnchor {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

struct PopupGeom {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    header_h: f32,
    /// the tail rides the card's LEFT edge (false: flipped to the right
    /// edge when there is no room on the anchor's side)
    tail_left: bool,
    /// the tail tip's clamped y in frame px (the anchor's center)
    tail_y: f32,
}

/// Size + place a popup card: content-hugging (their width:max-content)
/// between the 260/360 clamps, the body capped by the canvas height
/// (their max-height calc(100vh - 24px)) with the passed scroll clamped,
/// right-start of the anchor (top = rect.top - 12, their skidding) and
/// clamped — flipping to the anchor's left when the right overflows
/// (popper's flip).
fn popup_layout(
    sections: &[PopupSection],
    header_w: f32,
    anchor: PopupAnchor,
    zoom: f32,
    bounds: Rectangle,
) -> PopupGeom {
    let k = zoom / 20.0;
    let wrap_cols = ((POPUP_MAX_W - 2.0 * POPUP_PAD_X) / (6.6 * k)) as usize;
    let mut content_w = header_w;
    // body top pad (their first section's flow starts under the divider)
    let mut content_h = 0.0;
    for sec in sections {
        if sec.divider_before {
            content_h += 1.0; // h-px divider in flow
        }
        content_h += sec.pad_top * k;
        content_w = content_w
            .max(2.0 * POPUP_PAD_X * k + semibold_px(sec.label, 12.0 * k));
        if let Some((sub, _)) = &sec.label_sub {
            content_w = content_w.max(
                2.0 * POPUP_PAD_X * k
                    + semibold_px(sec.label, 12.0 * k)
                    + 6.0 * k
                    + est_px(sub, 12.0 * k),
            );
        }
        content_h += POPUP_LINE_H * k + sec.label_pad_bottom * k;
        for e in &sec.entries {
            match e {
                PopupEntry::Plain(text, _) => {
                    for l in wrap_chars(text, wrap_cols) {
                        content_w = content_w.max(
                            2.0 * POPUP_PAD_X * k
                                + l.chars().count() as f32 * 6.6 * k,
                        );
                        content_h += sec.pitch * k;
                    }
                }
                PopupEntry::Bullet(key, _, value, _) => {
                    content_w = content_w.max(
                        2.0 * POPUP_PAD_X * k
                            + 16.0 * k
                            + bullet_key_px(key, k)
                            + 6.6 * k
                            + est_px(value, 12.0 * k),
                    );
                    content_h += sec.pitch * k;
                }
            }
        }
        content_h += sec.pad_bottom * k;
    }
    // the whole card never outgrows the canvas (their max-height
    // calc(100vh - 24px) on canvas-popover__scroll)
    let header_h = POPUP_HEADER_H * k;
    let body_cap = (bounds.height - 24.0 * k - header_h).max(60.0 * k);
    let body_visible = content_h.min(body_cap);
    let h = header_h + body_visible;
    let w = content_w
        .max(POPUP_MIN_W * k)
        .min(POPUP_MAX_W * k)
        .min((bounds.width - 8.0).max(60.0));
    // right-start (distance 5): the tail tip lands on the anchor's edge
    let mut tail_left = true;
    let mut x = anchor.right + 5.0 * k;
    if x + w > bounds.width - 4.0 {
        // popper's flip: the card moves to the anchor's left, the tail
        // to the card's right edge
        x = anchor.left - 5.0 * k - w;
        tail_left = false;
        if x < 2.0 {
            x = (bounds.width - 4.0 - w).max(2.0);
            tail_left = true;
        }
    }
    let mut y = anchor.top - 12.0 * k;
    if y + h > bounds.height - 4.0 {
        y = (bounds.height - 4.0 - h).max(2.0);
    }
    y = y.max(2.0);
    // the tail points at the anchor's vertical center, kept clear of the
    // rounded corners (their arrowPadding)
    let tail_y = ((anchor.top + anchor.bottom) * 0.5).clamp(y + 14.0 * k, y + h - 14.0 * k);
    PopupGeom { x, y, w, h, header_h, tail_left, tail_y }
}

/// The popup header's little glyph (their w-3 svg before the title): the
/// field card's table-with-columns glyph, the table popup's table glyph,
/// the group popup's overlapping-panels glyph.
#[derive(Clone, Copy, PartialEq)]
enum PopupIcon {
    Field,
    Table,
    Group,
}

/// The card: speech-bubble tail toward the anchor, soft shadow, rounded
/// body, hairline border, rounded-TOP header band with the glyph + title
/// (+ optional colored type after a 6px gap — their items-baseline
/// gap-1.5 header pair), hairline under the band.
fn draw_popup_chrome(
    frame: &mut canvas::Frame,
    g: &PopupGeom,
    icon: PopupIcon,
    title: &str,
    sub: Option<(&str, Color)>,
    zoom: f32,
) {
    let k = zoom / 20.0;
    let radius = 8.0 * k;
    let border = Color::from_rgba(1.0, 1.0, 1.0, 0.1);
    let bg = Color::from_rgba8(0x30, 0x31, 0x37, 1.0); // opaque: even 2% alpha ghosts underlying table text through the card
    // soft shadow (their box-shadow 0 8px 24px #0006): two stacked
    // offset fills fake the falloff — the canvas has no blur
    for (off, alpha) in [(3.0, 0.16), (6.0, 0.10)] {
        frame.fill(
            &Path::rounded_rectangle(
                Point::new(g.x - radius * 0.4, g.y - radius * 0.4 + off * k),
                Size::new(g.w + radius * 0.8, g.h + radius * 0.8),
                radius.into(),
            ),
            Color::from_rgba(0.0, 0.0, 0.0, alpha),
        );
    }
    // the speech-bubble tail: a triangle poking out of the card edge
    // toward the anchor (their v-popper arrow — a 6px-deep, 12px-tall
    // CSS triangle with the border color outlining it), drawn BEFORE the
    // card so the body covers its base
    let depth = 6.0 * k;
    let half = 6.0 * k;
    let (tip_x, base_x) = if g.tail_left {
        (g.x - depth, g.x)
    } else {
        (g.x + g.w + depth, g.x + g.w)
    };
    let tail = Path::new(|p| {
        p.move_to(Point::new(tip_x, g.tail_y));
        p.line_to(Point::new(base_x, g.tail_y - half));
        p.line_to(Point::new(base_x, g.tail_y + half));
        p.close();
    });
    frame.fill(&tail, bg);
    frame.stroke(
        &Path::new(|p| {
            p.move_to(Point::new(base_x, g.tail_y - half));
            p.line_to(Point::new(tip_x, g.tail_y));
            p.line_to(Point::new(base_x, g.tail_y + half));
        }),
        canvas::Stroke {
            style: border.into(),
            width: 1.0,
            ..Default::default()
        },
    );
    frame.fill(
        &Path::rounded_rectangle(
            Point::new(g.x, g.y),
            Size::new(g.w, g.h),
            iced::border::Radius::from(radius),
        ),
        bg,
    );
    frame.stroke(
        &Path::rounded_rectangle(
            Point::new(g.x, g.y),
            Size::new(g.w, g.h),
            iced::border::Radius::from(radius),
        ),
        canvas::Stroke {
            style: border.into(),
            width: 1.0,
            ..Default::default()
        },
    );
    frame.fill(
        &Path::rounded_rectangle(
            Point::new(g.x, g.y),
            Size::new(g.w, g.header_h),
            iced::border::Radius {
                top_left: radius,
                top_right: radius,
                bottom_right: 0.0,
                bottom_left: 0.0,
            },
        ),
        Color::from_rgba8(0x3a, 0x3c, 0x42, 0.98),
    );
    frame.fill(
        &Path::rectangle(
            Point::new(g.x + 0.5, g.y + g.header_h),
            Size::new(g.w - 1.0, 1.0),
        ),
        border,
    );
    let icon_size = if icon == PopupIcon::Group { 13.0 } else { 12.0 } * k;
    let icon_cx = g.x + POPUP_PAD_X * k + icon_size * 0.5;
    let icon_cy = g.y + g.header_h * 0.5;
    let ink = Color::from_rgb8(0xf9, 0xfa, 0xfb); // gray-50
    draw_popup_icon(frame, icon, icon_cx, icon_cy, icon_size, ink);
    let title_x = g.x + POPUP_PAD_X * k + icon_size + 6.0 * k;
    frame.fill_text(Text {
        content: title.to_string(),
        position: Point::new(title_x, icon_cy),
        color: Color::WHITE,
        size: (0.7 * zoom).into(),
        font: Font {
            weight: font::Weight::Semibold,
            ..Font::DEFAULT
        },
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Default::default()
    });
    if let Some((text, color)) = sub {
        let name_w = semibold_px(title, 0.7 * zoom);
        frame.fill_text(Text {
            content: text.to_string(),
            position: Point::new(title_x + name_w + 6.0 * k, icon_cy),
            color,
            size: (0.7 * zoom).into(),
            font: theme::mono(),
            horizontal_alignment: iced::alignment::Horizontal::Left,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
    }
}

/// The header glyph, on a `size`-square box centered at (cx, cy): thin
/// currentColor strokes, matching their svgs (field: 12×12 table with a
/// header line + first-column divider; table: 12×10 rounded rect with a
/// filled top band; group: 13×12 overlapping panels).
fn draw_popup_icon(
    frame: &mut canvas::Frame,
    icon: PopupIcon,
    cx: f32,
    cy: f32,
    size: f32,
    ink: Color,
) {
    let stroke = |frame: &mut canvas::Frame, p: Path| frame.stroke(
        &p,
        canvas::Stroke {
            style: ink.into(),
            width: 1.0,
            ..Default::default()
        },
    );
    match icon {
        PopupIcon::Field => {
            // rect(0.5,0.5,11,11 rx1.5), h-line at y=4, v-line at x=4
            let s = size / 12.0;
            let x0 = cx - 6.0 * s;
            let y0 = cy - 6.0 * s;
            stroke(
                frame,
                Path::rounded_rectangle(
                    Point::new(x0 + 0.5 * s, y0 + 0.5 * s),
                    Size::new(11.0 * s, 11.0 * s),
                    iced::border::Radius::from(1.5 * s),
                ),
            );
            stroke(
                frame,
                Path::new(|p| {
                    p.move_to(Point::new(x0 + 0.5 * s, y0 + 4.0 * s));
                    p.line_to(Point::new(x0 + 11.5 * s, y0 + 4.0 * s));
                }),
            );
            stroke(
                frame,
                Path::new(|p| {
                    p.move_to(Point::new(x0 + 4.0 * s, y0 + 4.0 * s));
                    p.line_to(Point::new(x0 + 4.0 * s, y0 + 11.5 * s));
                }),
            );
        }
        PopupIcon::Table => {
            // rect(0.5,0.5,11,9 rx~2) outline + filled top band to y=2.5
            let s = size / 12.0;
            let x0 = cx - 6.0 * s;
            let y0 = cy - 5.0 * s;
            stroke(
                frame,
                Path::rounded_rectangle(
                    Point::new(x0 + 0.5 * s, y0 + 0.5 * s),
                    Size::new(11.0 * s, 9.0 * s),
                    iced::border::Radius::from(2.0 * s),
                ),
            );
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(x0 + 0.9 * s, y0 + 0.9 * s),
                    Size::new(10.2 * s, 2.0 * s),
                    iced::border::Radius {
                        top_left: 1.6 * s,
                        top_right: 1.6 * s,
                        bottom_right: 0.0,
                        bottom_left: 0.0,
                    },
                ),
                ink,
            );
        }
        PopupIcon::Group => {
            // two overlapping rounded panels (their 13×12 TableGroupIcon)
            let s = size / 13.0;
            let x0 = cx - 6.5 * s;
            let y0 = cy - 6.0 * s;
            stroke(
                frame,
                Path::rounded_rectangle(
                    Point::new(x0 + 0.7 * s, y0 + 2.2 * s),
                    Size::new(7.6 * s, 8.1 * s),
                    iced::border::Radius::from(1.5 * s),
                ),
            );
            stroke(
                frame,
                Path::rounded_rectangle(
                    Point::new(x0 + 4.0 * s, y0 + 3.7 * s),
                    Size::new(8.3 * s, 3.9 * s),
                    iced::border::Radius::from(1.5 * s),
                ),
            );
        }
    }
}

/// The body: sections inside a CLIPPED card area translated up by the
/// scroll offset (text primitives honor the clip bounds) — hairline
/// dividers between sections, semibold labels (with an optional inline
/// colored run), wrapped plain lines, truncating bullets. Clip-relative
/// coordinates (with_clip drafts at the clip origin).
fn draw_popup_body(
    frame: &mut canvas::Frame,
    g: &PopupGeom,
    sections: &[PopupSection],
    zoom: f32,
) {
    let k = zoom / 20.0;
    let wrap_cols = ((POPUP_MAX_W - 2.0 * POPUP_PAD_X) / (6.6 * k)) as usize;
    let body_top = g.y + g.header_h;
    let body_clip_h = g.h - g.header_h;
    frame.with_clip(
        Rectangle::new(Point::new(g.x, body_top), Size::new(g.w, body_clip_h)),
        |f| {
            let mut cy = 0.0;
            for sec in sections {
                if sec.divider_before {
                    // TooltipSectionDivider: h-px bg-white/10
                    f.fill_rectangle(
                        Point::new(0.0, cy),
                        Size::new(g.w, 1.0),
                        Color::from_rgba(1.0, 1.0, 1.0, 0.1),
                    );
                    cy += 1.0;
                }
                cy += sec.pad_top * k;
                // the section label (12px semibold, gray-200), with the
                // inline colored run after it (the enum name rides the
                // label line; the Default row IS this line)
                let label_cy = cy + POPUP_LINE_H * k * 0.5;
                f.fill_text(Text {
                    content: sec.label.to_string(),
                    position: Point::new(POPUP_PAD_X * k, label_cy),
                    color: Color::from_rgb8(0xe5, 0xe7, 0xeb), // gray-200
                    size: (0.6 * zoom).into(),
                    font: Font {
                        weight: font::Weight::Semibold,
                        ..Font::DEFAULT
                    },
                    horizontal_alignment: iced::alignment::Horizontal::Left,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
                if let Some((sub, color)) = &sec.label_sub {
                    // "Enum " carries a trailing space in their markup,
                    // the Default row uses their gap-1.5
                    let gap = if sec.label == "Enum" { 4.0 } else { 6.0 } * k;
                    f.fill_text(Text {
                        content: sub.clone(),
                        position: Point::new(
                            POPUP_PAD_X * k + semibold_px(sec.label, 12.0 * k) + gap,
                            label_cy,
                        ),
                        color: *color,
                        size: (0.6 * zoom).into(),
                        font: Font::DEFAULT,
                        horizontal_alignment: iced::alignment::Horizontal::Left,
                        vertical_alignment: iced::alignment::Vertical::Center,
                        ..Default::default()
                    });
                }
                cy += POPUP_LINE_H * k + sec.label_pad_bottom * k;
                // the enum values hang one tab stop in (their
                // whitespace-pre-wrap block leads every value with \t)
                let entry_indent = if sec.label == "Enum" { POPUP_TAB_INDENT * k } else { 0.0 };
                for e in &sec.entries {
                    match e {
                        PopupEntry::Plain(text, color) => {
                            for l in wrap_chars(text, wrap_cols) {
                                f.fill_text(Text {
                                    content: l,
                                    position: Point::new(
                                        POPUP_PAD_X * k + entry_indent,
                                        cy + sec.pitch * k * 0.5,
                                    ),
                                    color: *color,
                                    size: (0.6 * zoom).into(),
                                    font: Font::DEFAULT,
                                    horizontal_alignment: iced::alignment::Horizontal::Left,
                                    vertical_alignment: iced::alignment::Vertical::Center,
                                    ..Default::default()
                                });
                                cy += sec.pitch * k;
                            }
                        }
                        PopupEntry::Bullet(key, key_color, value, value_color) => {
                            f.fill_text(Text {
                                content: "\u{2022}".to_string(),
                                position: Point::new(
                                    POPUP_PAD_X * k + 4.0 * k,
                                    cy + sec.pitch * k * 0.5,
                                ),
                                color: *key_color,
                                size: (0.6 * zoom).into(),
                                font: Font::DEFAULT,
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                            let text_x = POPUP_PAD_X * k + 16.0 * k; // their pl-4
                            let usable = g.w - 2.0 * POPUP_PAD_X * k - 16.0 * k;
                            // natural first: truncation (their 40% key
                            // cap) only when the pair genuinely overflows
                            // the card (huge properties at the max width)
                            let key_nat = bullet_key_px(key, k);
                            let val_nat = est_px(value, 12.0 * k);
                            let colon_w = est_px(":", 12.0 * k) + 3.3 * k;
                            let (key_text, key_w, val_text) =
                                if key_nat + colon_w + val_nat <= usable {
                                    (key.clone(), key_nat, value.clone())
                                } else {
                                    let (kt, kw) =
                                        truncate_to(key, usable * 0.4, 6.6 * k);
                                    let (vt, _) = truncate_to(
                                        value,
                                        (usable - kw - colon_w).max(6.6 * k),
                                        6.6 * k,
                                    );
                                    (kt, kw, vt)
                                };
                            // the key run carries its colon+nbsp (their
                            // `:` span between the two truncate spans)
                            let key_w_px = key_w + colon_w;
                            f.fill_text(Text {
                                content: format!("{key_text}:"),
                                position: Point::new(text_x, cy + sec.pitch * k * 0.5),
                                color: *key_color,
                                size: (0.6 * zoom).into(),
                                font: Font::DEFAULT,
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                            f.fill_text(Text {
                                content: val_text,
                                position: Point::new(
                                    text_x + key_w_px,
                                    cy + sec.pitch * k * 0.5,
                                ),
                                color: *value_color,
                                size: (0.6 * zoom).into(),
                                font: Font::DEFAULT,
                                horizontal_alignment: iced::alignment::Horizontal::Left,
                                vertical_alignment: iced::alignment::Vertical::Center,
                                ..Default::default()
                            });
                            cy += sec.pitch * k;
                        }
                    }
                }
                cy += sec.pad_bottom * k;
            }
        },
    );
}

/// Truncate `text` to at most `max_w` px at the given per-char advance,
/// ending with an ellipsis when cut. Returns (text, drawn width).
fn truncate_to(text: &str, max_w: f32, per_char: f32) -> (String, f32) {
    let count = text.chars().count();
    let full = count as f32 * per_char;
    if full <= max_w {
        return (text.to_string(), full);
    }
    let fit = ((max_w - per_char) / per_char).max(0.0) as usize; // room for …
    let cut: String = text.chars().take(fit).collect();
    let w = (fit + 1) as f32 * per_char;
    (format!("{cut}\u{2026}"), w)
}

#[allow(clippy::too_many_arguments)]
fn draw_field_popup(
    frame: &mut canvas::Frame,
    layout: &Layout,
    b: &TableBox,
    r: usize,
    view_zoom: f32,
    pan: Vector,
    bounds: Rectangle,
) {
    // static size: dbdiagram's popovers are DOM nodes OUTSIDE the zoomed
    // canvas (their CanvasPopover anchors a position:fixed reference div
    // to the hovered element's screen rect, so zoom never scales them) —
    // drawn in-canvas, that translates to computing every size at 1:1 px
    // (zoom pinned to 20) and letting the view zoom move only the anchor
    let zoom = 20.0;
    let k = zoom / 20.0;
    let row = &b.rows[r];
    let wrap_cols = ((POPUP_MAX_W - 2.0 * POPUP_PAD_X) / (6.6 * k)) as usize;

    // sections stack in their order (Note, Enum, Default, Custom
    // Properties); dividers follow their template exactly: after the
    // Note when anything follows, never between Enum and Default, and
    // before Custom Properties when anything precedes
    let mut sections: Vec<PopupSection> = Vec::new();
    if let Some(note) = &row.note {
        sections.push(note_section(note, wrap_cols, false));
    }
    let mut enum_found = false;
    if row.ty_enum {
        if let Some(e) = layout.enums.iter().find(|e| e.name == row.ty) {
            enum_found = true;
            // every value, tab-indented (their whitespace-pre-wrap
            // block); the body scrolls when it overflows
            let entries = e
                .values
                .iter()
                .cloned()
                .map(|v| PopupEntry::Plain(v, ENUM_VALUE_BLUE))
                .collect();
            sections.push(PopupSection {
                label: "Enum",
                label_sub: Some((e.name.clone(), ENUM_VALUE_BLUE)),
                entries,
                pitch: POPUP_RELAXED_H,
                pad_top: 4.0,
                label_pad_bottom: 0.0,
                pad_bottom: 6.0,
                divider_before: row.note.is_some(),
            });
        }
    }
    if let Some(d) = &row.default {
        // theirs shows the raw expression: quotes AND backticks stripped
        let raw = d.trim_matches(|c| c == '\'' || c == '"' || c == '`');
        sections.push(PopupSection {
            label: "Default",
            label_sub: Some((raw.to_string(), TYPE_ORANGE)),
            entries: Vec::new(),
            pitch: POPUP_LINE_H,
            pad_top: 8.0,
            label_pad_bottom: 0.0,
            pad_bottom: 8.0,
            // their divider after the Note skips the Enum section (which
            // carries its own) — a Note followed directly by the Default
            // row still gets one
            divider_before: row.note.is_some() && !enum_found,
        });
    }
    if !row.custom.is_empty() {
        let preceded = row.note.is_some() || enum_found || row.default.is_some();
        sections.push(custom_props_section(&row.custom, preceded));
    }

    let header_w = 12.0 * k
        + 6.0 * k
        + semibold_px(&row.name, 14.0 * k)
        + 6.0 * k
        + mono_px(&row.ty, 14.0 * k)
        + 2.0 * POPUP_PAD_X * k;
    let s = cell_size(view_zoom);
    let box_left = to_px(b.x, b.y, view_zoom, pan).x;
    let anchor = PopupAnchor {
        left: box_left,
        right: to_px(b.x + b.w, b.y, view_zoom, pan).x,
        top: to_px(b.x, b.y + r as i32 + 1, view_zoom, pan).y,
        bottom: to_px(b.x, b.y + r as i32 + 1, view_zoom, pan).y + s.height,
    };
    let g = popup_layout(&sections, header_w, anchor, zoom, bounds);
    let type_color = if row.ty_enum { ENUM_VALUE_BLUE } else { TYPE_ORANGE };
    draw_popup_chrome(frame, &g, PopupIcon::Field, &row.name, Some((row.ty.as_str(), type_color)), zoom);
    draw_popup_body(frame, &g, &sections, zoom);
}

fn draw_table_popup(
    frame: &mut canvas::Frame,
    b: &TableBox,
    view_zoom: f32,
    pan: Vector,
    bounds: Rectangle,
) {
    // static size (see draw_field_popup)
    let zoom = 20.0;
    let k = zoom / 20.0;
    let wrap_cols = ((POPUP_MAX_W - 2.0 * POPUP_PAD_X) / (6.6 * k)) as usize;
    let mut sections: Vec<PopupSection> = Vec::new();
    if let Some(note) = &b.note {
        sections.push(note_section(note, wrap_cols, false));
    }
    if !b.custom.is_empty() {
        sections.push(custom_props_section(&b.custom, b.note.is_some()));
    }
    let header_w = 12.0 * k
        + 6.0 * k
        + semibold_px(&b.title, 14.0 * k)
        + 2.0 * POPUP_PAD_X * k;
    let s = cell_size(view_zoom);
    let box_left = to_px(b.x, b.y, view_zoom, pan).x;
    let header_top = to_px(b.x, b.y, view_zoom, pan).y;
    let anchor = PopupAnchor {
        left: box_left,
        right: to_px(b.x + b.w, b.y, view_zoom, pan).x,
        top: header_top,
        bottom: header_top + s.height,
    };
    let g = popup_layout(&sections, header_w, anchor, zoom, bounds);
    draw_popup_chrome(frame, &g, PopupIcon::Table, &b.title, None, zoom);
    draw_popup_body(frame, &g, &sections, zoom);
}

fn draw_group_popup(
    frame: &mut canvas::Frame,
    g: &dbml_ui::layout::GroupRegion,
    view_zoom: f32,
    pan: Vector,
    bounds: Rectangle,
) {
    // static size (see draw_field_popup)
    let zoom = 20.0;
    let k = zoom / 20.0;
    let wrap_cols = ((POPUP_MAX_W - 2.0 * POPUP_PAD_X) / (6.6 * k)) as usize;
    let mut sections: Vec<PopupSection> = Vec::new();
    if let Some(note) = &g.note {
        sections.push(note_section(note, wrap_cols, false));
    }
    if !g.custom.is_empty() {
        sections.push(custom_props_section(&g.custom, g.note.is_some()));
    }
    let header_w = 13.0 * k
        + 6.0 * k
        + semibold_px(&g.name, 14.0 * k)
        + 2.0 * POPUP_PAD_X * k;
    let s = cell_size(zoom);
    let band_left = to_px(g.rect.0, g.rect.1, view_zoom, pan).x;
    let band_top = to_px(g.rect.0, g.rect.1, view_zoom, pan).y;
    let anchor = PopupAnchor {
        left: band_left,
        right: band_left + g.rect.2 as f32 * s.width,
        top: band_top,
        bottom: band_top + s.height,
    };
    let geom = popup_layout(&sections, header_w, anchor, zoom, bounds);
    draw_popup_chrome(frame, &geom, PopupIcon::Group, &g.name, None, zoom);
    draw_popup_body(frame, &geom, &sections, zoom);
}

pub(crate) const ENUM_VALUE_BLUE: iced::Color = iced::Color::from_rgb(0.376, 0.647, 0.980); // #60A5FA
const TYPE_ORANGE: iced::Color = iced::Color::from_rgb(0.992, 0.729, 0.455); // #FDBA74

/// dbdiagram's pointer-anchored tooltip pill (pointer + 10,10, clamped,
/// dark bg) — it follows the cursor. Draws one line per entry; the first
/// line reads as the title (bold) when there are several (the enum
/// definition tooltip).
fn draw_tooltip_pill(
    frame: &mut canvas::Frame,
    lines: &[String],
    p: Point,
    _view_zoom: f32,
    bounds: Rectangle,
) {
    // static size (see draw_field_popup)
    let zoom = 20.0;
    let k = zoom / 20.0;
    let widest = lines
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0) as f32;
    // the codebase's standard 0.58em advance for the 10px tooltip font —
    // the old 6.2 + 16px padding sized the pill ~25% past its text
    let w = widest * 5.8 * k + 12.0 * k;
    // 15k per line + 4k padding: a single line is an 18k pill, snug
    // around its 10k text
    let h = (lines.len() as f32 * 15.0 + 4.0) * k;
    let x = (p.x + 10.0 * k)
        .clamp(2.0, (bounds.width - w - 2.0).max(2.0));
    let y = (p.y + 10.0 * k)
        .clamp(2.0, (bounds.height - h - 2.0).max(2.0));
    frame.fill(
        &Path::rounded_rectangle(
            Point::new(x, y),
            Size::new(w, h),
            iced::border::Radius::from(4.0),
        ),
        theme::BG,
    );
    for (i, line) in lines.iter().enumerate() {
        // the enum's name line reads as the tooltip's title
        let title = i == 0 && lines.len() > 1;
        frame.fill_text(Text {
            content: line.clone(),
            // horizontally CENTERED in the pill (each line, so shorter
            // enum-definition lines stay balanced under the bold title)
            position: Point::new(x + w / 2.0, y + (2.0 + (i as f32 + 0.5) * 15.0) * k),
            color: theme::TEXT,
            size: (0.5 * zoom).into(),
            font: Font {
                weight: if title {
                    font::Weight::Bold
                } else {
                    font::Weight::Normal
                },
                ..Font::DEFAULT
            },
            horizontal_alignment: iced::alignment::Horizontal::Center,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
    }
}

/// True when the pointer sits on a header's color-picker glyph: the
/// overlay suppresses the popup/glyph chrome there (the picker owns the
/// press).
fn picker_glyph_hovered(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> bool {
    palette_icon_hit(layout, zoom, pan, p).is_some()
        || super::groups::group_icon_hit(layout, zoom, pan, p).is_some()
}

/// The diagram pane's event capture: the wheel over an open popup card
/// scrolls the CARD, not the canvas (dbdiagram's popup scrolls too). The
/// popups themselves draw in the Diagram widget — canvas text inside a
/// `with_layer` layer renders nothing on iced 0.13's WebGL2 backend, and
/// this widget IS layered (the pane stack layers children 1+), so its
/// draw is intentionally empty.
pub struct DiagramOverlay<'a> {
    pub layout: &'a Layout,
    pub zoom: f32,
    pub pan: Vector,
    /// Cursor position over the canvas (local px), as `Diagram::hover`.
    pub hover: Option<Point>,
}

impl canvas::Program<crate::app::Message> for DiagramOverlay<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        // the popups draw in THIS stacked widget, above the Diagram
        // widget's chrome: one canvas geometry flattens into a mesh pass
        // (everything) + a text pass (everything), so popup chrome drawn
        // into the base widget's frame would land under every table's
        // text. A second widget composites wholly after the first — the
        // popup (chrome + text) rides on top of everything again.
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        if let Some(p) = self.hover {
            draw_active_popup(
                self.layout,
                self.zoom,
                self.pan,
                p,
                bounds,
                &mut frame,
            );
        }
        vec![frame.into_geometry()]
    }
}

/// Draws the popup under the pointer (chrome + text) into the given
/// canvas frame. Called from the DiagramOverlay widget's own draw: the
/// overlay stacks above the Diagram widget, so the popup's chrome and
/// text composite wholly after the base canvas — one geometry flattens
/// into a mesh pass + a text pass, which would sink popup chrome under
/// every table's text inside the base widget's frame.
fn draw_active_popup(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: Point,
    bounds: Rectangle,
    frame: &mut canvas::Frame,
) {
    // the picker glyphs sit on the same header/band hover targets the
    // popups below use: while one is under the pointer nothing pops
    if picker_glyph_hovered(layout, zoom, pan, p) {
        return;
    }
    // The left icon cluster and the field card compose: hovering a
    // pk/fk/unique GLYPH pops that glyph's tooltip pill AND, when the
    // row also qualifies, the field card — the card draws first, the
    // pill on top. The note glyph raised by custom properties alone
    // pops no pill (icon_tooltip_line already filtered it): the card
    // below is that row's affordance. With a real `note` the pill
    // shows the note text, again beside the card.
    let icon_pill = icon_tooltip_line(layout, zoom, pan, p);
    // the field card (dbdiagram's FieldTooltipPopup): hovering a
    // column row shows a popup IFF the column has a note, an enum
    // type, or a default — plain/nn/unique/pk/fk rows never pop.
    // Content sections stack in their order: Note, Enum, Default.
    let field_card: Option<(&TableBox, usize)> = {
        let hovered = hit_test(layout, zoom, pan, p);
        hovered.and_then(|b| {
            row_at(layout, zoom, pan, p)
                .filter(|(hb, r)| hb.title == b.title && row_has_field_card(&b.rows[*r]))
        })
    };
    if let Some((b, r)) = field_card {
        draw_field_popup(frame, layout, b, r, zoom, pan, bounds);
        // the hovered glyph's tooltip pill rides on top of the card
        if let Some(line) = icon_pill {
            draw_tooltip_pill(frame, &[line], p, zoom, bounds);
        }
        return;
    }
    // a glyph on a row the card never pops (a plain pk row, say)
    // keeps today's behavior: its tooltip pill alone
    if let Some(line) = icon_pill {
        draw_tooltip_pill(frame, &[line], p, zoom, bounds);
        return;
    }
    // dbdiagram's table popup: hovering a table's HEADER band pops a
    // card with the title and its Custom Properties (live-verified:
    // `users` + owner/sla_hours/pii). Tables without custom properties
    // never pop.
    if let Some(b) = hit_test(layout, zoom, pan, p) {
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        let rowf = (p.y - top_left.y) / s.height;
        if rowf < 1.0 && (!b.custom.is_empty() || b.note.is_some()) {
            draw_table_popup(frame, b, zoom, pan, bounds);
            return;
        }
    }
    // a group's label hover pops the same card a column does — the
    // Note section and the Custom Properties bullet list (the old
    // pointer note pill duplicated the card's Note section)
    if let Some(g) = super::groups::group_label_hit(layout, zoom, pan, p) {
        if g.note.is_some() || !g.custom.is_empty() {
            draw_group_popup(frame, g, zoom, pan, bounds);
            return;
        }
    }
    // the "Enum" pill shows the enum's definition (its name, then the
    // values one per line)
    let lines: Option<Vec<String>> =
        if let Some((b, row)) = enum_hit(layout, zoom, pan, p) {
            let ty = &b.rows[row].ty;
            layout.enums.iter().find(|e| &e.name == ty).map(|e| {
                let mut lines = vec![e.name.clone()];
                let cap = 12; // keep the pill readable on huge enums
                lines.extend(e.values.iter().take(cap).cloned());
                if e.values.len() > cap {
                    lines.push("\u{2026}".to_string());
                }
                lines
            })
        } else {
            None
        };
    if let Some(lines) = lines {
        draw_tooltip_pill(frame, &lines, p, zoom, bounds);
    }
}

#[cfg(test)]
mod picker_suppression_tests {
    use super::*;

    /// The overlay's suppression contract: wherever a picker glyph
    /// accepts the pointer the overlay draws nothing; the same header
    /// away from the glyph still pops.
    #[test]
    fn the_overlay_yields_wherever_a_picker_glyph_accepts_the_pointer() {
        let src = "Table users [owner: \"data-team\"] { id int }
Table t { x int }
TableGroup g { users t }
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(0.0, 0.0);
        let s = cell_size(zoom);

        // the TABLE glyph's center: the draw pass's own geometry
        let b = &l.boxes[0];
        let tl = to_px(b.x, b.y, zoom, pan);
        let (c, _, _) = palette_icon_geometry(b, tl, s);
        assert!(palette_icon_hit(&l, zoom, pan, c).is_some());
        assert!(picker_glyph_hovered(&l, zoom, pan, c));

        // the GROUP glyph's center: the band's right end (the zone
        // groups.rs draws)
        let g = &l.groups[0];
        let gtl = to_px(g.rect.0, g.rect.1, zoom, pan);
        let icon_h = 0.72 * s.height;
        let icon_w = CIRCLES_VIEWBOX.0 / CIRCLES_VIEWBOX.1 * icon_h;
        let gc = Point::new(
            gtl.x + g.rect.2 as f32 * s.width - 0.6 * s.width - icon_w / 2.0,
            gtl.y + s.height * 0.5,
        );
        assert!(super::super::groups::group_icon_hit(&l, zoom, pan, gc).is_some());
        assert!(picker_glyph_hovered(&l, zoom, pan, gc));

        // the same header away from the glyph: the custom-properties
        // card still shows (users carries [owner: ...])
        let header_left = Point::new(tl.x + 1.0 * s.width, tl.y + 0.5 * s.height);
        assert!(!picker_glyph_hovered(&l, zoom, pan, header_left));
    }

    #[test]
    fn the_records_slot_follows_the_note_glyph() {
        // the records glyph rides in the title-adjacent row: right of the
        // note glyph when the table carries one, else right of the title
        // — and always clear of the wheel's hover slot at the right edge
        // (the width computation reserves that room, Row::BOLD_TEXT on
        // both sides keeps the measures in step)
        let src = "Table plain { id int

  records (id) {
    1
  }
}
Table noted { id int
  Note: 'hi'

  records (id) {
    1
  }
}
";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(0.0, 0.0);
        let s = cell_size(zoom);
        let band_center = |b: &TableBox| {
            let (x0, x1) = records_band(b, zoom, pan).unwrap();
            (x0 + x1) / 2.0
        };
        let plain = l.boxes.iter().find(|b| b.title == "plain").unwrap();
        let noted = l.boxes.iter().find(|b| b.title == "noted").unwrap();
        // plain: right of the title; noted: right of the note glyph
        let ptl = to_px(plain.x, plain.y, zoom, pan);
        assert!(band_center(plain) > title_end_px(plain, ptl, s));
        let ntl = to_px(noted.x, noted.y, zoom, pan);
        let (note_cx, note_size) = note_icon_geometry(noted, ntl, s).unwrap();
        assert!(band_center(noted) > note_cx + note_size * 0.5);
        // and the note pushes the records slot further right than the
        // plain table's title-side slot
        assert!(band_center(noted) > band_center(plain));
        // both stay left of the wheel's hover zone
        for b in [&plain, &noted] {
            let tl = to_px(b.x, b.y, zoom, pan);
            let (wc, half_w, _) = palette_icon_geometry(b, tl, s);
            assert!(
                band_center(b) + 0.5 * s.width < wc.x - half_w,
                "the records band must clear the wheel's slot"
            );
        }
    }
}

#[cfg(test)]
mod drag_priority_tests {
    use super::*;

    fn layout_of(src: &str) -> Layout {
        dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ))
    }

    #[test]
    fn the_dragged_table_paints_after_its_raised_lines() {
        let src = "Table a { id integer [pk] }
Table b { a_id integer }
Ref: b.a_id > a.id
";
        let l = layout_of(src);
        fn titles<'a>(seq: &'a [PaintItem<'_>]) -> Vec<&'a str> {
            seq.iter()
                .filter_map(|item| match item {
                    PaintItem::Table(b) => Some(b.title.as_str()),
                    _ => None,
                })
                .collect()
        }
        // mid-drag: a's grab raised it to item_z's end and its line is
        // raised — the table must come AFTER the raised edges, i.e. last
        let z = vec![RaisedItem::Table("a".into())];
        let seq = paint_items(&l, &z, None, None, Some("a"));
        assert_eq!(titles(&seq).last(), Some(&"a"));
        // at rest: the persistent raise keeps a last either way
        let seq = paint_items(&l, &z, None, None, None);
        assert_eq!(titles(&seq).last(), Some(&"a"));
        // no raise at all: doc order is preserved
        let seq = paint_items(&l, &[], None, None, None);
        assert_eq!(titles(&seq), vec!["a", "b"]);
    }
}
