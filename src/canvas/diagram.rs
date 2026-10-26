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
use dbml_ui::layout::{many_ends, EdgeRoute, Layout, TableBox};
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
    0.6 * 0.55 * zoom
}

/// X range (frame px) of the pk/fk icon on a row: dbdiagram puts the
/// marker right AFTER the column name.
fn marker_band(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> (f32, f32) {
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let name_end = top_left.x + 1.0 * s.width + b.rows[row].name.width() as f32 * char_px(zoom);
    (name_end + 0.35 * s.width, name_end + 1.85 * s.width)
}

/// The pk/fk marker under a canvas point, if any: the icon band sits just
/// after the column name, on a row that has a marker.
pub fn marker_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let b = hit_test(layout, zoom, pan, p)?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let row = (((p.y - top_left.y) / s.height) as usize).saturating_sub(1);
    if row >= b.rows.len() {
        return None;
    }
    let (x0, x1) = marker_band(b, row, zoom, pan);
    (p.x >= x0 && p.x < x1 && (b.rows[row].pk || b.rows[row].fk)).then_some((b, row))
}

/// X range (frame px) of the note icon on a row that has one: the small
/// paper glyph at the far right edge of the box.
fn note_band(b: &TableBox, row: usize, zoom: f32, pan: Vector) -> Option<(f32, f32)> {
    b.rows[row].note.as_ref()?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let right = top_left.x + b.w as f32 * s.width;
    Some((right - 1.5 * s.width, right - 0.5 * s.width))
}

/// The note under a canvas point, if the pointer is on its paper icon.
pub fn note_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let b = hit_test(layout, zoom, pan, p)?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let row = (((p.y - top_left.y) / s.height) as usize).saturating_sub(1);
    if row >= b.rows.len() {
        return None;
    }
    let (x0, x1) = note_band(b, row, zoom, pan)?;
    (p.x >= x0 && p.x < x1).then_some((b, row))
}

/// The hover label for a pk/fk marker row.
pub fn marker_label(pk: bool) -> &'static str {
    if pk {
        "Primary Key"
    } else {
        "Foreign Key"
    }
}

/// Tooltip geometry for a marker: just below the hovered box, aligned
/// with the marker column — the route channel below a box carries only
/// edge lines (mesh over mesh), never row text, because iced defers all
/// text after all meshes within a layer and a tooltip could never hide
/// row text drawn over it. Flips above the box when the pane bottom is
/// close. Clamped inside the frame-local pane rect.
fn marker_tooltip_rect(
    marker_x: f32,
    box_top: f32,
    box_bottom: f32,
    text: &str,
    zoom: f32,
    local: Rectangle,
) -> Rectangle {
    let text_w = 0.5 * zoom * 0.6;
    let w = text.chars().count() as f32 * text_w + 16.0;
    let h = 0.5 * zoom + 10.0;
    let mut x = marker_x - 0.2 * zoom;
    x = x.max(local.x + 2.0);
    x = x.min(local.x + local.width - w - 2.0);
    let mut y = box_bottom + 4.0;
    if y + h > local.y + local.height - 2.0 {
        y = box_top - h - 4.0;
    }
    y = y.max(local.y + 2.0);
    Rectangle::new(Point::new(x, y), Size::new(w, h))
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
    for b in &layout.boxes {
        let is_hovered = hovered.map(|h| h.title == b.title).unwrap_or(false);
        paint_table(frame, b, zoom, pan, is_hovered);
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
        paint_flow(frame, &pts, r, phase, t, (trim_fk, trim_ref));
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
    for (w, a) in [(11.0, 0.05), (6.5, 0.09), (3.0, 0.15)] {
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
) {
    let flat = flatten_pieces(&rounded_pieces(pts, radius));
    if flat.total <= 0.0 {
        return;
    }
    let lo = trims.0;
    let hi = (flat.total - trims.1).max(lo);
    for (a, b) in dash_windows(phase, 43.0, 3.0, lo, hi) {
        stroke_arc_range(frame, &flat, a, b, 4.0, t);
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
            style: with_alpha(brighten(theme::EDGE_HOVER, 1.2), 0.9 * t).into(),
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
        stroke,
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

/// Key icon (dbdiagram pk): circular bow, shaft, two teeth — stroked.
fn draw_key_icon(frame: &mut canvas::Frame, center: Point, size: f32, color: Color) {
    let w = (0.085 * size).max(1.0);
    let stroke = canvas::Stroke {
        style: color.into(),
        width: w,
        ..Default::default()
    };
    let r = 0.30 * size;
    let bow = Path::circle(Point::new(center.x - 0.18 * size, center.y), r);
    frame.stroke(&bow, stroke);
    let shaft_y = center.y;
    let x0 = center.x - 0.18 * size + r * 0.72;
    let x1 = center.x + 0.45 * size;
    frame.stroke(
        &Path::new(|b| {
            b.move_to(Point::new(x0, shaft_y));
            b.line_to(Point::new(x1, shaft_y));
            // two teeth
            let t1 = x1 - 0.16 * size;
            let t2 = x1 - 0.02 * size;
            b.move_to(Point::new(t1, shaft_y));
            b.line_to(Point::new(t1, shaft_y + 0.26 * size));
            b.move_to(Point::new(t2, shaft_y));
            b.line_to(Point::new(t2, shaft_y + 0.26 * size));
        }),
        stroke,
    );
}

/// Chain-link icon (dbdiagram fk): two interlocking rings — stroked.
fn draw_link_icon(frame: &mut canvas::Frame, center: Point, size: f32, color: Color) {
    let w = (0.085 * size).max(1.0);
    let stroke = canvas::Stroke {
        style: color.into(),
        width: w,
        ..Default::default()
    };
    let r = 0.26 * size;
    let off = 0.30 * size;
    frame.stroke(
        &Path::circle(Point::new(center.x - off, center.y), r),
        stroke,
    );
    frame.stroke(
        &Path::circle(Point::new(center.x + off, center.y), r),
        stroke,
    );
}

fn paint_table(frame: &mut canvas::Frame, b: &TableBox, zoom: f32, pan: Vector, hovered: bool) {
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
    frame.stroke(
        &body,
        Stroke {
            style: if hovered {
                theme::TABLE_HOVER
            } else {
                theme::TABLE_BORDER
            }
            .into(),
            width: border_w,
            ..Default::default()
        },
    );

    // header band: solid near-black, full-width, with a hairline below it
    let band = Size::new(size.width, s.height);
    frame.fill_rectangle(top_left, band, header_color(b));
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

    // a tiny table glyph before the title, like dbdiagram's header icon
    let glyph = 0.5 * s.height;
    let gx = top_left.x + 0.9 * s.width;
    let gy = top_left.y + (s.height - glyph) / 2.0;
    let gstroke = Stroke {
        style: Color::from_rgba(0.91, 0.92, 0.93, 0.9).into(),
        width: 1.0,
        ..Default::default()
    };
    frame.stroke(
        &Path::new(|bd| {
            let r = glyph * 0.2;
            bd.move_to(Point::new(gx + r, gy));
            bd.line_to(Point::new(gx + glyph - r, gy));
            bd.line_to(Point::new(gx + glyph, gy + r));
            bd.line_to(Point::new(gx + glyph, gy + glyph));
            bd.line_to(Point::new(gx, gy + glyph));
            bd.line_to(Point::new(gx, gy + r));
            bd.close();
            // header divider + one body line
            let h1 = gy + glyph * 0.42;
            bd.move_to(Point::new(gx, h1));
            bd.line_to(Point::new(gx + glyph, h1));
            let h2 = gy + glyph * 0.71;
            bd.move_to(Point::new(gx, h2));
            bd.line_to(Point::new(gx + glyph, h2));
        }),
        gstroke,
    );

    // title, bold white, after the glyph
    let title_size = 0.55 * s.height;
    frame.fill_text(Text {
        content: b.title.clone(),
        position: Point::new(gx + glyph + 0.5 * s.width, top_left.y + s.height * 0.5),
        color: Color::WHITE,
        size: title_size.into(),
        font: Font {
            weight: font::Weight::Bold,
            ..Font::MONOSPACE
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
        let baseline = |x: f32| Point::new(x, y);
        frame.fill_text(Text {
            content: row.name.clone(),
            position: baseline(top_left.x + 1.0 * s.width),
            color: theme::ROW_TEXT,
            size: row_size.into(),
            font: Font::MONOSPACE,
            horizontal_alignment: iced::alignment::Horizontal::Left,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
        let (band_x0, band_x1) = marker_band(b, i, zoom, pan);
        let center = baseline((band_x0 + band_x1) / 2.0);
        if row.pk {
            draw_key_icon(frame, center, 0.55 * s.height, theme::MARKER);
        } else if row.fk {
            draw_link_icon(frame, center, 0.50 * s.height, theme::MARKER);
        }
        // dbdiagram-style badges: small pills for not null / unique
        let badge_font = 0.38 * s.height;
        let badge_char = 0.6 * badge_font;
        let mut bx = band_x1 + 0.35 * s.width;
        for label in row.badges() {
            let tw = label.chars().count() as f32 * badge_char;
            let pad = 0.30 * s.width;
            let rect = Rectangle::new(
                Point::new(bx, y - 0.30 * s.height),
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
                size: badge_font.into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
            bx = rect.x + rect.width + 0.30 * s.width;
        }
        if !row.ty.is_empty() {
            let right = if row.note.is_some() {
                top_left.x + size.width - 1.8 * s.width
            } else {
                top_left.x + size.width - 1.2 * s.width
            };
            frame.fill_text(Text {
                content: row.ty.clone(),
                position: baseline(right),
                color: theme::TYPE_TEXT,
                size: row_size.into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        // note paper icon at the far right; hover shows the note
        if let Some((nx0, nx1)) = note_band(b, i, zoom, pan) {
            let ncenter = baseline((nx0 + nx1) / 2.0);
            draw_note_icon(frame, ncenter, 0.55 * s.height, theme::MARKER);
        }
    }
}

/// Tiny page glyph (dbdiagram's note marker): outlined sheet with a
/// folded corner and two text lines.
fn draw_note_icon(frame: &mut canvas::Frame, center: Point, size: f32, color: Color) {
    let w = (0.085 * size).max(1.0);
    let stroke = canvas::Stroke {
        style: color.into(),
        width: w,
        ..Default::default()
    };
    let x0 = center.x - 0.40 * size;
    let x1 = center.x + 0.40 * size;
    let y0 = center.y - 0.50 * size;
    let y1 = center.y + 0.50 * size;
    let fold = 0.22 * size;
    frame.stroke(
        &Path::new(|b| {
            b.move_to(Point::new(x0, y0));
            b.line_to(Point::new(x1 - fold, y0));
            b.line_to(Point::new(x1, y0 + fold));
            b.line_to(Point::new(x1, y1));
            b.line_to(Point::new(x0, y1));
            b.close();
            // fold
            b.move_to(Point::new(x1 - fold, y0));
            b.line_to(Point::new(x1 - fold, y0 + fold));
            b.line_to(Point::new(x1, y0 + fold));
            // text lines
            let ly = y0 + 0.42 * size;
            b.move_to(Point::new(x0 + 0.14 * size, ly));
            b.line_to(Point::new(x1 - 0.14 * size, ly));
            b.move_to(Point::new(x0 + 0.14 * size, ly + 0.22 * size));
            b.line_to(Point::new(x1 - 0.14 * size, ly + 0.22 * size));
        }),
        stroke,
    );
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
        // the band sits right after the name "id": marker_band tells us
        // exactly where, and its middle must hit
        let (x0, x1) = marker_band(b, 0, zoom, pan);
        let p = Point::new((x0 + x1) / 2.0, row_top + s.height * 0.5);
        let hit = marker_hit(&l, zoom, pan, p).expect("pk marker hit");
        assert_eq!(hit.0.title, "a");
        assert!(hit.0.rows[hit.1].pk);
        assert_eq!(marker_label(true), "Primary Key");
        // the name text (left of the band) is NOT the marker band
        let name_p = Point::new(top_left.x + 1.2 * s.width, row_top + s.height * 0.5);
        assert!(marker_hit(&l, zoom, pan, name_p).is_none());
        // left of the box padding misses too
        let left_p = Point::new(top_left.x + 0.3 * s.width, row_top + s.height * 0.5);
        assert!(marker_hit(&l, zoom, pan, left_p).is_none());
        // and the band tracks the name width: "id" spans 1.0..2.32 cells
        // (2 chars at 0.66 cells advance), so the band starts past that
        assert!(
            x0 > top_left.x + 2.32 * s.width,
            "band after the name: {x0}"
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
    fn note_hit_tests_the_paper_icon_band() {
        let src = "Table a { id integer [pk, note: 'the identifier'] user_id integer }\n";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let zoom = 20.0;
        let pan = Vector::new(40.0, 40.0);
        let b = &l.boxes[0];
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        // row 0 (id) has the note; its icon sits at the far right
        let (x0, x1) = note_band(b, 0, zoom, pan).expect("row 0 has a note");
        let p = Point::new((x0 + x1) / 2.0, top_left.y + s.height * 1.5);
        let hit = note_hit(&l, zoom, pan, p).expect("note icon hit");
        assert_eq!(hit.0.title, "a");
        assert_eq!(hit.1, 0);
        // row 1 has no note: no band, no hit
        assert!(note_band(b, 1, zoom, pan).is_none());
        let p2 = Point::new((x0 + x1) / 2.0, top_left.y + s.height * 2.5);
        assert!(note_hit(&l, zoom, pan, p2).is_none());
        // and the marker band on row 0 does not overlap the note band
        let (_, mx1) = marker_band(b, 0, zoom, pan);
        assert!(mx1 < x0, "marker {mx1} vs note {x0}");
    }

    #[test]
    fn badge_badges_follow_settings() {
        let src = "Table a { id integer [pk, not null, unique] email varchar(80) }\n";
        let l = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ));
        let r = &l.boxes[0].rows[0];
        assert_eq!(r.badges(), vec!["Not Null", "Unique"]);
        assert_eq!(l.boxes[0].rows[1].badges(), Vec::<&str>::new());
        // null (explicit) is NOT the Not Null badge
        let src2 = "Table b { x integer [null] }\n";
        let l2 = dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src2).doc,
        ));
        assert!(l2.boxes[0].rows[0].badges().is_empty());
        // widths account for badges + note icon
        let w_plain = {
            let src3 = "Table c { email varchar(80) }\n";
            dbml_ui::layout::layout(&dbml_ui::model::build(
                &dbml_ui::dbml::parser::parse(src3).doc,
            ))
            .boxes[0]
                .w
        };
        assert!(l.boxes[0].w > w_plain, "badges widen the box");
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
        // note icons: hover shows the note text; then pk/fk markers
            if let Some((b, row)) = note_hit(self.layout, self.zoom, self.pan, p) {
                let top_left = to_px(b.x, b.y, self.zoom, self.pan);
                let s = cell_size(self.zoom);
                let local = Rectangle::new(Point::ORIGIN, bounds.size());
                let (nx0, nx1) = note_band(b, row, self.zoom, self.pan).unwrap();
                // 60-char cap keeps the pill readable
                let mut text: String = b.rows[row].note.clone().unwrap_or_default();
                if text.chars().count() > 60 {
                    text = text.chars().take(60).collect::<String>() + "…";
                }
                let rect = marker_tooltip_rect(
                    (nx0 + nx1) / 2.0,
                    top_left.y,
                    top_left.y + b.h as f32 * s.height,
                    &text,
                    self.zoom,
                    local,
                );
                            frame.fill(
                &Path::rounded_rectangle(
                    rect.position(),
                    rect.size(),
                    iced::border::Radius::from(4.0),
                ),
                theme::TIP_BG,
            );
            frame.fill_text(Text {
                content: text,
                position: Point::new(rect.x + 8.0, rect.y + rect.height * 0.5),
                color: theme::TIP_TEXT,
                size: (0.42 * self.zoom).into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        } else if let Some((b, row)) = marker_hit(self.layout, self.zoom, self.pan, p) {
                let rowd = &b.rows[row];
                let label = marker_label(rowd.pk);
                let top_left = to_px(b.x, b.y, self.zoom, self.pan);
                let s = cell_size(self.zoom);
                // the tooltip rect is frame-local: clamp against the pane
                // size, not the window-space widget bounds
                let local = Rectangle::new(Point::ORIGIN, bounds.size());
                let (band_x0, band_x1) = marker_band(b, row, self.zoom, self.pan);
                let rect = marker_tooltip_rect(
                    (band_x0 + band_x1) / 2.0,
                    top_left.y,
                    top_left.y + b.h as f32 * s.height,
                    label,
                    self.zoom,
                    local,
                );
                // dbdiagram's tooltip: a light pill with dark text
                            frame.fill(
                &Path::rounded_rectangle(
                    rect.position(),
                    rect.size(),
                    iced::border::Radius::from(4.0),
                ),
                theme::TIP_BG,
            );
            frame.fill_text(Text {
                content: label.to_string(),
                position: Point::new(rect.x + 8.0, rect.y + rect.height * 0.5),
                color: theme::TIP_TEXT,
                size: (0.5 * self.zoom).into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }
}
