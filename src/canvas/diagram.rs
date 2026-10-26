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

use crate::theme;
use dbml_ui::layout::{many_ends, EdgeRoute, Layout, Row, TableBox};
use iced::widget::canvas::{self, Path, Stroke, Text};
use iced::{font, Color, Font, Point, Rectangle, Size, Vector};
use unicode_width::UnicodeWidthStr;

/// Diagram cells around the content (matches the SVG renderer's margin).
pub const MARGIN_CELLS: f32 = 1.0;
/// A diagram cell is half as wide as it is tall (mirrors the SVG unit
/// ratio 10x20); `zoom` is the cell height in pixels.
pub const CELL_ASPECT: f32 = 0.5;

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

/// Monospace character advance for row text at `zoom` (mono glyphs are
/// ~0.6 em wide), used to find where a column name ends.
fn char_px(zoom: f32) -> f32 {
    // keep in sync with layout::Row::CHAR_CELL (0.58 cell of 10px)
    0.58 * 0.5 * zoom
}

/// The embellishment icons of one column row, in dbdiagram's order:
/// pk, fk, unique, note — a left cluster after the name, each 10.5px
/// (1:1) with a 4px gap, vertically centered. Returns (kind, center).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowIcon {
    Pk,
    Fk,
    Unique,
    Note,
}

/// The name's end x in frame px (the row text is drawn at this size).
fn row_name_end_px(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> f32 {
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    top_left.x + 1.0 * s.width + b.rows[row].name.width() as f32 * char_px(zoom)
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
    let cell_w = 0.5 * zoom;
    let push = |icon: RowIcon, u: &mut f32, out: &mut Vec<(RowIcon, f32)>| {
        out.push((icon, *u));
        *u += match icon {
            // "Unique" is a text pill: its slot is the pill width + gap
            RowIcon::Unique => Row::badge_cell("Unique") * cell_w + 4.0 * k,
            _ => 14.5 * k,
        };
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
    if rowd.note.is_some() {
        push(RowIcon::Note, &mut u, &mut out);
    }
    out
}

/// The pk/fk marker under a canvas point, if any: the pointer must be
/// inside one of the row's pk/fk icon slots.
pub fn marker_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let (b, row) = hovered_row_of(layout, zoom, pan, p)?;
    let k = zoom / 20.0;
    for (icon, slot) in row_icon_layout(b, row, zoom, pan) {
        if !matches!(icon, RowIcon::Pk | RowIcon::Fk) {
            continue;
        }
        let center = Point::new(slot + 5.25 * k, y_of(b, row, zoom, pan));
        let r = 5.25 * k;
        if (p.x - center.x).abs() <= r && (p.y - center.y).abs() <= r {
            return Some((b, row));
        }
    }
    None
}

/// The note icon under a canvas point, if any.
pub fn note_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let (b, row) = hovered_row_of(layout, zoom, pan, p)?;
    let k = zoom / 20.0;
    for (icon, slot) in row_icon_layout(b, row, zoom, pan) {
        if icon != RowIcon::Note {
            continue;
        }
        let center = Point::new(slot + 5.25 * k, y_of(b, row, zoom, pan));
        let r = 5.25 * k;
        if (p.x - center.x).abs() <= r && (p.y - center.y).abs() <= r {
            return Some((b, row));
        }
    }
    None
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
fn records_band(b: &TableBox, zoom: f32, pan: Vector) -> Option<(f32, f32)> {
    b.records.as_ref()?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let right = top_left.x + b.w as f32 * s.width;
    Some((right - 1.5 * s.width, right - 0.5 * s.width))
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
    /// A drag is in flight (table or pan) — show the grab cursor.
    pub dragging: bool,
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
                tracked.map(Message::CanvasMoved),
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
        // NOTE: canvas geometry is not clipped to the widget by default, but
        // the pane container uses .clip(true) (view.rs) so boxes dragged to
        // negative diagram coords never paint over the editor pane. Don't
        // use Frame::with_clip here — its draft/paste misplaces mesh
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
        if self.dragging {
            return Interaction::Grabbing;
        }
        if let Some(p) = cursor.position_over(bounds) {
            let local = Point::new(p.x - bounds.x, p.y - bounds.y);
            if hit_test(self.layout, self.zoom, self.pan, local).is_some() {
                if records_icon_hit(self.layout, self.zoom, self.pan, local).is_some() {
                    return Interaction::Pointer; // opens the record view
                }
                if note_hit(self.layout, self.zoom, self.pan, local).is_some() {
                    return Interaction::Help;
                }
                return Interaction::Pointer;
            }
            // empty canvas: the open hand, signalling pan
            return Interaction::Grab;
        }
        Interaction::default()
    }
}

/// The (box, row) under a canvas point, if any row of that box.
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
}

fn paint(
    frame: &mut canvas::Frame,
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    hs: &HoverState<'_>,
) {
    for edge in &layout.edges {
        let t = hs.anim.get(edge.edge).copied().unwrap_or(0.0);
        paint_edge(frame, edge, zoom, pan, t, hs.clock);
    }
    let hovered = hs.pointer.and_then(|p| hit_test(layout, zoom, pan, p));
    // the hovered column row (dbdiagram tints it); only when its box is
    // the hovered one
    let hovered_row = hs
        .pointer
        .and_then(|p| row_at(layout, zoom, pan, p))
        .filter(|(b, _)| hovered.is_some_and(|h| h.title == b.title))
        .map(|(_, r)| r);
    for b in &layout.boxes {
        let is_hovered = hovered.map(|h| h.title == b.title).unwrap_or(false);
        paint_table(frame, b, zoom, pan, is_hovered, hovered_row);
    }
}

fn paint_edge(
    frame: &mut canvas::Frame,
    edge: &EdgeRoute,
    zoom: f32,
    pan: Vector,
    t: f32,
    clock: f32,
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

    // dbdiagram's hovered route: the core eases gray -> their highlight
    // blue and doubles in width (their LINE_WIDTH_NORMAL 1 -> HIGHLIGHT 2)
    let mut core = edge_stroke(zoom);
    core.style = lerp_color(theme::LINE, theme::EDGE_HOVER, t).into();
    core.width *= 1.0 + t;

    // end markers: the "many" end gets a chevron + open circle, the "one"
    // end a perpendicular tick. Each end also trims the flow path by the
    // marker gap (their LGt: 10px at a one end, 16px at a many end), so
    // the dashes stop at the circle instead of painting over the markers.
    let (many_fk, many_ref) = many_ends(edge.card);
    let n = pts.len();
    let trim_fk = if many_fk { 0.8 * zoom } else { 0.5 * zoom };
    let trim_ref = if many_ref { 0.8 * zoom } else { 0.5 * zoom };

    if t > 0.0 {
        // static glow hugging the path (their shape shadow: blur 10,
        // opacity .5 in the dark theme — no pulsing)
        paint_glow(frame, &path, zoom, t);
        frame.stroke(&path, core);
        // the flow: a dashed stroke on the SAME filleted path (their
        // setLineDash([3, 40]) with lineDashOffset = time/30), so dashes
        // ride the corners instead of cutting them. Increasing the offset
        // shifts the pattern toward the path start; pts[0] is the fk end,
        // so the flow reads referenced key → foreign key.
        let phase = hs_clock(clock) * zoom / 20.0;
        paint_flow(frame, &pts, r, phase, t, (trim_fk, trim_ref), zoom);
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

/// Static halo around a highlighted route: the canvas has no blur, so
/// three nested strokes fake the falloff of dbdiagram's shape shadow.
fn paint_glow(frame: &mut canvas::Frame, path: &Path, zoom: f32, t: f32) {
    let k = zoom / 20.0;
    // approximate dbdiagram's gaussian shadow (blur 10, opacity .5) with
    // nested rings; with MSAA the bands blend into a smooth falloff. The
    // profile stays clear of the core so the highlight does not read as a
    // thicker line (their shadow sits behind the line, not around it).
    for (w, a) in [
        (4.0, 0.06),
        (5.5, 0.05),
        (7.0, 0.04),
        (9.0, 0.03),
        (11.0, 0.02),
        (13.0, 0.012),
    ] {
        frame.stroke(
            path,
            Stroke {
                style: with_alpha(theme::EDGE_HOVER, a * t).into(),
                width: w * k,
                ..Default::default()
            },
        );
    }
}

/// The flowing dashes of a highlighted route: 3px dashes with 40px gaps
/// (1:1 px), 4px wide, gliding along the filleted path. Rendered by
/// stroking each dash's sub-range of the flattened path — a dash pattern
/// can never cut a corner the way bead chords did.
fn paint_flow(
    frame: &mut canvas::Frame,
    pts: &[Point],
    radius: f32,
    phase: f32,
    t: f32,
    trims: (f32, f32),
    zoom: f32,
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
        stroke_arc_range(frame, &flat, a, b, 4.0 * k, t);
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
fn stroke_arc_range(frame: &mut canvas::Frame, flat: &Flat, a: f32, b: f32, width: f32, t: f32) {
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
            style: with_alpha(brighten(theme::EDGE_HOVER, 1.2), t).into(),
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
const PK_ICON: &str = "M 6.5625 0.9844 C 8.1935 0.9844 9.5156 2.3065 9.5156 3.9375 C 9.5156 5.5685 8.1935 6.8906 6.5625 6.8906 C 6.1735 6.8906 5.8022 6.8153 5.462 6.6786 L 4.5938 7.5469 H 3.9375 V 8.5313 H 2.9531 V 9.5156 H 0.9844 V 7.5469 L 3.736 4.7952 C 3.6518 4.5171 3.6092 4.2281 3.6094 3.9375 C 3.6094 2.3065 4.9315 0.9844 6.5625 0.9844 Z M 6.5625 0 C 4.388 0 2.625 1.7627 2.625 3.9375 C 2.625 4.1186 2.6373 4.2989 2.6619 4.4772 L 0.1441 6.995 C 0.0519 7.0873 0 7.2125 0 7.343 L 0 10.0078 C 0 10.2797 0.2204 10.5 0.4922 10.5 H 3.4453 C 3.7171 10.5 3.9375 10.2797 3.9375 10.0078 V 9.5156 H 4.4297 C 4.7015 9.5156 4.9219 9.2953 4.9219 9.0234 V 8.6133 L 5.7432 7.7895 C 6.011 7.8464 6.2848 7.875 6.5625 7.875 C 8.737 7.875 10.5 6.1123 10.5 3.9375 C 10.5 1.763 8.7373 0 6.5625 0 Z M 6.5625 2.9531 C 6.5625 3.4968 7.0032 3.9375 7.5469 3.9375 C 8.0905 3.9375 8.5313 3.4968 8.5313 2.9531 C 8.5313 2.4095 8.0905 1.9688 7.5469 1.9688 C 7.0032 1.9688 6.5625 2.4095 6.5625 2.9531 Z";
const FK_ICON: &str = "M 4.6244 6.6258 c -0.2002 0 -0.3885 -0.078 -0.5304 -0.2197 c -1.1204 -1.1205 -1.1204 -2.9437 0 -4.0641 l 1.5016 -1.5016 c 0.5419 -0.542 1.2635 -0.8405 2.032 -0.8405 s 1.4901 0.2985 2.032 0.8405 c 1.1204 1.1204 1.1204 2.9436 0 4.0641 l -1.001 1.001 c -0.1416 0.1417 -0.3299 0.2197 -0.5304 0.2198 c -0.2002 0 -0.3885 -0.078 -0.5304 -0.2197 c -0.1416 -0.1417 -0.2198 -0.3301 -0.2198 -0.5305 c 0 -0.2004 0.0781 -0.3888 0.2198 -0.5305 l 1.001 -1.001 c 0.5355 -0.5355 0.5355 -1.4068 0 -1.9423 c -0.2588 -0.2588 -0.6039 -0.4015 -0.9712 -0.4015 s -0.7123 0.1426 -0.9712 0.4015 l -1.5016 1.5016 c -0.5355 0.5355 -0.5355 1.4068 0 1.9422 c 0.2925 0.2925 0.2925 0.7684 0 1.0609 c -0.1419 0.1417 -0.3302 0.2197 -0.5304 0.2197 z M 2.8725 10.5 c -0.7685 0 -1.4901 -0.2985 -2.032 -0.8405 c -1.1204 -1.1204 -1.1204 -2.9436 0 -4.0641 l 0.7509 -0.7509 c 0.1416 -0.1416 0.3302 -0.2197 0.5304 -0.2197 c 0.2002 0 0.3888 0.0781 0.5304 0.2198 c 0.1416 0.1416 0.2198 0.33 0.2198 0.5304 c 0 0.2004 -0.0781 0.3888 -0.2198 0.5305 l -0.7509 0.7508 c -0.5355 0.5354 -0.5355 1.4067 0 1.9422 c 0.2588 0.2588 0.6039 0.4015 0.9712 0.4015 s 0.7123 -0.1425 0.9712 -0.4015 l 1.5016 -1.5016 c 0.5355 -0.5355 0.5355 -1.4068 0 -1.9422 c -0.1416 -0.1417 -0.2198 -0.3301 -0.2198 -0.5305 s 0.0781 -0.3888 0.2198 -0.5305 c 0.1416 -0.1417 0.3302 -0.2197 0.5304 -0.2197 s 0.3888 0.078 0.5304 0.2196 c 1.1204 1.1205 1.1204 2.9437 0 4.0641 l -1.5016 1.5016 c -0.5419 0.542 -1.2635 0.8405 -2.032 0.8405 z";
/// dbdiagram's unique fingerprint icon (their QTn).
const NOTE_ICON: &str = "M 9 1.125 C 9.2063 1.125 9.375 1.2938 9.375 1.5 V 6.75 H 7.5 C 7.0852 6.75 6.75 7.0852 6.75 7.5 V 9.375 H 1.5 C 1.2938 9.375 1.125 9.2063 1.125 9 V 1.5 C 1.125 1.2938 1.2938 1.125 1.5 1.125 H 9 Z M 1.5 10.5 H 6.75 H 6.8789 C 7.2773 10.5 7.6594 10.343 7.9406 10.0617 L 10.0617 7.9406 C 10.343 7.6594 10.5 7.2773 10.5 6.8789 V 6.75 V 1.5 C 10.5 0.6727 9.8273 0 9 0 H 1.5 C 0.6727 0 0 0.6727 0 1.5 V 9 C 0 9.8273 0.6727 10.5 1.5 10.5 Z M 3 7.6875 C 3 7.5383 2.9407 7.3952 2.8352 7.2897 C 2.7298 7.1843 2.5867 7.125 2.4375 7.125 C 2.2883 7.125 2.1452 7.1843 2.0398 7.2897 C 1.9343 7.3952 1.875 7.5383 1.875 7.6875 C 1.875 7.8367 1.9343 7.9798 2.0398 8.0853 C 2.1452 8.1907 2.2883 8.25 2.4375 8.25 C 2.5867 8.25 2.7298 8.1907 2.8352 8.0853 C 2.9407 7.9798 3 7.8367 3 7.6875 Z M 2.4375 2.25 C 2.2883 2.25 2.1452 2.3093 2.0398 2.4148 C 1.9343 2.5202 1.875 2.6633 1.875 2.8125 C 1.875 2.9617 1.9343 3.1048 2.0398 3.2102 C 2.1452 3.3157 2.2883 3.375 2.4375 3.375 C 2.5867 3.375 2.7298 3.3157 2.8352 3.2102 C 2.9407 3.1048 3 2.9617 3 2.8125 C 3 2.6633 2.9407 2.5202 2.8352 2.4148 C 2.7298 2.3093 2.5867 2.25 2.4375 2.25 Z M 3 5.25 C 3 5.1008 2.9407 4.9577 2.8352 4.8522 C 2.7298 4.7468 2.5867 4.6875 2.4375 4.6875 C 2.2883 4.6875 2.1452 4.7468 2.0398 4.8522 C 1.9343 4.9577 1.875 5.1008 1.875 5.25 C 1.875 5.3992 1.9343 5.5423 2.0398 5.6478 C 2.1452 5.7532 2.2883 5.8125 2.4375 5.8125 C 2.5867 5.8125 2.7298 5.7532 2.8352 5.6478 C 2.9407 5.5423 3 5.3992 3 5.25 Z";
/// The records affordance glyph (a table with rows), used at the right
/// end of the header of a records-bearing table on hover, on an 18px grid.
const TABLE_RING_ICON: &str = "M18 2c0-1.103-.897-2-2-2H2c-1.103 0-2 .897-2 2v14c0 1.103.897 2 2 2h14c1.103 0 2-.897 2-2zM2 16V2h14l.002 14z";
const TABLE_ROWS_ICON: &str = "M4 4h1.998v2H4zm4 0h6v2h-6zm-4 4h1.998v2H4zm4 0h6v2h-6zm-4 4h1.998v2H4zm4 0h6v2h-6z";
/// dbdiagram's table pictogram, lifted from their renderer (Path2D
/// strings): the rounded-rect ring and its header band, on an 11.25x10
/// grid. The ring's two subpaths wind oppositely, so a plain fill punches
/// the window; the band is filled on top.
const TABLE_PICTO_RING: &str = "M9.93182 0.909091H2.31818C1.8161 0.909091 1.40909 1.3161 1.40909 1.81818V8.18182C1.40909 8.6839 1.8161 9.09091 2.31818 9.09091H9.93182C10.4339 9.09091 10.8409 8.6839 10.8409 8.18182V1.81818C10.8409 1.3161 10.4339 0.909091 9.93182 0.909091ZM2.31818 0C1.31403 0 0.5 0.814028 0.5 1.81818V8.18182C0.5 9.18597 1.31403 10 2.31818 10H9.93182C10.936 10 11.7 9.18597 11.7 8.18182V1.81818C11.7 0.814028 10.936 0 9.93182 0Z";
const TABLE_PICTO_BAND: &str = "M0.5 1.81818C0.5 0.814028 1.31403 0 2.31818 0H9.93182C10.936 0 11.75 0.814028 11.75 1.81818V2.5H0.5V1.81818Z";

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

/// Fill an icon authored as SVG path data (dbdiagram's Path2D strings),
/// scaled from its `grid` to `size` px and centered on `center`.
fn svg_icon(frame: &mut canvas::Frame, center: Point, size: f32, grid: f32, color: Color, data: &str) {
    let k = size / grid;
    let px = |x: f32, y: f32| Point::new(center.x - size / 2.0 + x * k, center.y - size / 2.0 + y * k);
    let cmds = svg_path_cmds(data);
    let path = Path::new(|b| {
        let (mut x, mut y, mut sx, mut sy) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        // previous cubic's second control, for the S/s smooth shorthand
        let (mut qx, mut qy) = (0.0f32, 0.0f32);
        for &(cmd, ref v) in &cmds {
            let mut idx = 0usize;
            loop {
                let take = |idx: &mut usize| -> f32 {
                    let val = v.get(*idx).copied().unwrap_or(0.0);
                    *idx += 1;
                    val
                };
                match cmd {
                    b'M' => {
                        x = take(&mut idx); y = take(&mut idx);
                        sx = x; sy = y;
                        b.move_to(px(x, y));
                    }
                    b'm' => {
                        x += take(&mut idx); y += take(&mut idx);
                        sx = x; sy = y;
                        b.move_to(px(x, y));
                    }
                    b'L' => { x = take(&mut idx); y = take(&mut idx); b.line_to(px(x, y)); }
                    b'l' => { x += take(&mut idx); y += take(&mut idx); b.line_to(px(x, y)); }
                    b'H' => { x = take(&mut idx); b.line_to(px(x, y)); }
                    b'h' => { x += take(&mut idx); b.line_to(px(x, y)); }
                    b'V' => { y = take(&mut idx); b.line_to(px(x, y)); }
                    b'v' => { y += take(&mut idx); b.line_to(px(x, y)); }
                    b'C' => {
                        let (x1, y1) = (take(&mut idx), take(&mut idx));
                        let (x2, y2) = (take(&mut idx), take(&mut idx));
                        x = take(&mut idx); y = take(&mut idx);
                        b.bezier_curve_to(px(x1, y1), px(x2, y2), px(x, y));
                        qx = x2; qy = y2;
                    }
                    b'c' => {
                        let (x1, y1) = (x + take(&mut idx), y + take(&mut idx));
                        let (x2, y2) = (x + take(&mut idx), y + take(&mut idx));
                        x += take(&mut idx); y += take(&mut idx);
                        b.bezier_curve_to(px(x1, y1), px(x2, y2), px(x, y));
                        qx = x2; qy = y2;
                    }
                    b'S' | b's' => {
                        let rel = cmd == b's';
                        let (mut x2, mut y2) = (take(&mut idx), take(&mut idx));
                        let (mut ex, mut ey) = (take(&mut idx), take(&mut idx));
                        if rel { x2 += x; y2 += y; ex += x; ey += y; }
                        b.bezier_curve_to(px(2.0 * x - qx, 2.0 * y - qy), px(x2, y2), px(x, y));
                        x = ex; y = ey;
                        qx = x2; qy = y2;
                    }
                    b'Z' | b'z' => { b.close(); x = sx; y = sy; }
                    _ => {}
                }
                // for non-cubics the smooth-reflection anchor is the point itself
                if !matches!(cmd, b'C' | b'c' | b'S' | b's') {
                    qx = x; qy = y;
                }
                // implicit repetition: only these take repeated groups
                let group = match cmd.to_ascii_uppercase() {
                    b'M' | b'L' => 2,
                    b'H' | b'V' => 1,
                    _ => 0,
                };
                if group == 0 || idx >= v.len() {
                    break;
                }
                if cmd == b'M' || cmd == b'm' {
                    // subsequent pairs after moveto are lines
                    let (letter, v) = if cmd == b'M' { (b'L', v.clone()) } else { (b'l', v.clone()) };
                    let _ = (letter, v);
                }
            }
        }
    });
    frame.fill(&path, color);
}

/// dbdiagram's table pictogram: the rounded-rect ring with its header
/// band filled on top, scaled to `size` px and centered on `center`.
/// Shared by the table header and the record view's title row.
pub(crate) fn draw_table_pictogram(frame: &mut canvas::Frame, center: Point, size: f32, color: Color) {
    svg_icon(frame, center, size, 11.25, color, TABLE_PICTO_RING);
    svg_icon(frame, center, size, 11.25, color, TABLE_PICTO_BAND);
}

fn paint_table(
    frame: &mut canvas::Frame,
    b: &TableBox,
    zoom: f32,
    pan: Vector,
    hovered: bool,
    hovered_row: Option<usize>,
) {
    let s = cell_size(zoom);
    let top_left = to_px(b.x, b.y, zoom, pan);
    let size = Size::new(b.w as f32 * s.width, b.h as f32 * s.height);

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
    frame.fill_rectangle(top_left, band, header_color(b));
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

    // the table pictogram before the title (dbdiagram's own table icon:
    // rounded-rect ring with its header band filled on top)
    let glyph = 0.55 * s.height;
    let gx = top_left.x + 0.9 * s.width;
    let gy = top_left.y + (s.height - glyph) / 2.0;
    let gcolor = Color::from_rgba(0.91, 0.92, 0.93, 0.55);
    draw_table_pictogram(
        frame,
        Point::new(gx + glyph / 2.0, gy + glyph / 2.0),
        glyph,
        gcolor,
    );

    // records affordance: tables with sample rows grow a rows glyph at
    // the right end of the header while hovered (press opens the record
    // view)
    if b.records.is_some() && hovered {
        if let Some((rx0, rx1)) = records_band(b, zoom, pan) {
            let center = Point::new((rx0 + rx1) / 2.0, top_left.y + s.height * 0.5);
            let size = 0.55 * s.height;
            svg_icon(frame, center, size, 18.0, theme::MARKER, TABLE_RING_ICON);
            svg_icon(frame, center, size, 18.0, theme::MARKER, TABLE_ROWS_ICON);
        }
    }

    // title, bold white, after the glyph
    let title_size = 0.55 * s.height;
    frame.fill_text(Text {
        content: b.title.clone(),
        position: Point::new(gx + glyph + 0.5 * s.width, top_left.y + s.height * 0.5),
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

    // column rows: name at the left, gray outline marker right AFTER the
    // name (dbdiagram's placement), type right-aligned in muted gray
    let row_size = 0.55 * s.height;
    for (i, row) in b.rows.iter().enumerate() {
        let y = top_left.y + (i as f32 + 1.0) * s.height + s.height * 0.5;
        if hovered && hovered_row == Some(i) {
            // dbdiagram tints the hovered column row, edge-to-edge inside
            // the border
            frame.fill_rectangle(
                Point::new(top_left.x + 1.0, top_left.y + (i as f32 + 1.0) * s.height),
                Size::new(size.width - 2.0, s.height),
                theme::ROW_HOVER_BG,
            );
        }
        let baseline = |x: f32| Point::new(x, y);
        frame.fill_text(Text {
            content: row.name.clone(),
            position: baseline(top_left.x + 1.0 * s.width),
            color: theme::ROW_TEXT,
            size: row_size.into(),
            font: Font::DEFAULT,
            horizontal_alignment: iced::alignment::Horizontal::Left,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
        // dbdiagram's left cluster: pk, fk, unique, note in order — the
        // first three 10.5px glyphs ("Unique" keeps its text pill in its
        // slot), each with a 4px gap, at 40% opacity
        for (icon, slot) in row_icon_layout(b, i, zoom, pan) {
            match icon {
                RowIcon::Unique => {
                    // "Unique" keeps its text pill in its cluster slot
                    badge_pill(frame, "Unique", slot, y, s);
                }
                other => {
                    let path = match other {
                        RowIcon::Pk => PK_ICON,
                        RowIcon::Fk => FK_ICON,
                        RowIcon::Note => NOTE_ICON,
                        RowIcon::Unique => unreachable!(),
                    };
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
        }
        // right side: type text, then the "Not Null" pill flush right; the
        // type's right edge shifts left of the pill (their renderType)
        let nn_w = if row.not_null == Some(true) {
            Row::badge_cell("Not Null") * s.width
        } else {
            0.0
        };
        if !row.ty.is_empty() {
            let right = top_left.x + size.width - 0.5 * s.width - nn_w;
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
        if row.not_null == Some(true) {
            badge_pill(
                frame,
                "Not Null",
                top_left.x + size.width - 0.5 * s.width - nn_w + 0.3 * s.width,
                y,
                s,
            );
        }
    }
}

/// A subtle text badge pill ("Unique" / "Not Null"), left edge at `x`,
/// vertically centered on the row line.
fn badge_pill(frame: &mut canvas::Frame, label: &str, x: f32, y: f32, s: Size) {
    let tw = label.chars().count() as f32 * 0.456 * s.width;
    let pad = 0.30 * s.width;
    let rect = Rectangle::new(
        Point::new(x, y - 0.30 * s.height),
        Size::new(tw + 2.0 * pad, 0.60 * s.height),
    );
    frame.fill(
        &Path::rounded_rectangle(
            rect.position(),
            rect.size(),
            iced::border::Radius::from(0.12 * s.height),
        ),
        Color::from_rgba(0.545, 0.565, 0.612, 0.14),
    );
    frame.fill_text(Text {
        content: label.to_string(),
        position: Point::new(rect.x + pad, y),
        color: theme::TYPE_TEXT,
        size: (0.38 * s.height).into(),
        font: Font::DEFAULT,
        horizontal_alignment: iced::alignment::Horizontal::Left,
        vertical_alignment: iced::alignment::Vertical::Center,
        ..Default::default()
    });
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
        let src = "Table a { id integer [pk, note: 'the identifier'] user_id integer }\n";
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
        let src = "Table a { id integer [pk] name varchar\n  records (id, name) {\n    1, 'x'\n  }\n}\nTable b { c integer }\n";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = l.boxes.iter().find(|b| b.title == "a").unwrap();
        assert!(b.records.is_some(), "fixture carries records");
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        let right = top_left.x + b.w as f32 * s.width;
        // the rows glyph at the right end of the header band
        let p = Point::new(right - 1.0 * s.width, top_left.y + s.height * 0.5);
        let hit = records_icon_hit(&l, zoom, pan, p).expect("records affordance hit");
        assert_eq!(hit.title, "a");
        // body rows and the title area are not the affordance
        let body = Point::new(right - 1.0 * s.width, top_left.y + s.height * 1.5);
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
        let src = "Table a { id integer [pk, not null, unique] email varchar(80) }\n";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let r = &l.boxes[0].rows[0];
        assert_eq!(r.not_null, Some(true));
        assert!(r.unique);
        assert_eq!(l.boxes[0].rows[1].not_null, None);
        // null (explicit) is NOT the NN pill
        let src2 = "Table b { x integer [null] }\n";
        let l2 = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src2).doc,
        ));
        assert_eq!(l2.boxes[0].rows[0].not_null, Some(false));
        // the NN pill widens an otherwise identical row
        let w_plain = {
            let src3 = "Table c { email varchar(80) }\n";
            dbml_ui::layout::layout(&dbml_ui::model::build(
                &dbml_ui::dbml::parser::parse(src3).doc,
            ))
            .boxes[0]
                .w
        };
        let w_nn = {
            let src4 = "Table c { email varchar(80) [not null] }\n";
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
    fn edge_hit_finds_the_line_under_the_pointer() {
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(
                "Table a { id integer [pk] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n",
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
                "Table a { id integer [pk] }\nTable b { a_id integer }\nRef: b.a_id > a.id\n",
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
        // over a table box: the pointer (click-to-jump)
        let (bx, by) = first_box_point();
        let over_box = Cursor::Available(Point::new(BOUNDS.x + bx, BOUNDS.y + by));
        assert_eq!(
            d.mouse_interaction(&(), BOUNDS, over_box),
            Interaction::Pointer
        );
        // empty canvas (past all content): the open grab hand
        let empty = Cursor::Available(Point::new(BOUNDS.x + 550.0, BOUNDS.y + 380.0));
        assert_eq!(d.mouse_interaction(&(), BOUNDS, empty), Interaction::Grab);
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

/// The diagram's marker/note tooltip as its own canvas widget, STACKED
/// above the diagram canvas: iced flattens one widget's geometries into a
/// mesh pass + a text pass, so a second-geometry tooltip pill painted
/// behind the box row text. Separate widgets composite fully in order.
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
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let Some(p) = self.hover else {
            return vec![frame.into_geometry()];
        };
        // note icons: hover shows the note text; then pk/fk markers.
        // dbdiagram's tooltip is pointer-anchored (pointer + 10,10, clamped)
        // with a dark #222 pill — it follows the cursor.
        let text = if let Some((b, row)) = note_hit(self.layout, self.zoom, self.pan, p) {
            // 60-char cap keeps the pill readable
            let mut text: String = b.rows[row].note.clone().unwrap_or_default();
            if text.chars().count() > 60 {
                text = text.chars().take(60).collect::<String>() + "\u{2026}";
            }
            Some(text)
        } else {
            marker_hit(self.layout, self.zoom, self.pan, p)
                .map(|(b, row)| marker_label(b.rows[row].pk).to_string())
        };
        if let Some(text) = text {
            let k = self.zoom / 20.0;
            let w = text.chars().count() as f32 * 6.2 * k + 16.0 * k;
            let h = 20.0 * k;
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
            frame.fill_text(Text {
                content: text,
                position: Point::new(x + 8.0 * k, y + h * 0.5),
                color: theme::TEXT,
                size: (0.5 * self.zoom).into(),
                font: Font::DEFAULT,
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }
}
