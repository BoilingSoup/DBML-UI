//! The diagram canvas: draws the laid-out schema as rounded table boxes,
//! header bands, column rows, and relationship edges — mirroring the SVG
//! exporter's geometry, but sized by `zoom` (pixels per diagram cell) so
//! text stays crisp at every scale.

use crate::theme;
use dbml_ui::layout::{EdgeRoute, Layout, TableBox};
use dbml_ui::model::NodeKind;
use iced::widget::canvas::{self, Path, Stroke, Text};
use iced::{font, Color, Font, Point, Rectangle, Size, Vector};

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

/// The pk/fk marker under a canvas point, if any: the icon band sits
/// between the box edge and the column name, on a row that has a marker.
pub fn marker_hit(layout: &Layout, zoom: f32, pan: Vector, p: Point) -> Option<(&TableBox, usize)> {
    let b = hit_test(layout, zoom, pan, p)?;
    let top_left = to_px(b.x, b.y, zoom, pan);
    let s = cell_size(zoom);
    let row = (((p.y - top_left.y) / s.height) as usize).saturating_sub(1);
    if row >= b.rows.len() {
        return None;
    }
    // the glyph starts at 1.6*sw (advance ~1.1*sw wide); the band hugs it
    // and stops before the column name at 3.2*sw
    let x0 = top_left.x + 0.8 * s.width;
    let x1 = top_left.x + 2.9 * s.width;
    (p.x >= x0 && p.x < x1 && (b.rows[row].pk || b.rows[row].fk)).then_some((b, row))
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
        .unwrap_or(match b.kind {
            NodeKind::Table => theme::TABLE_HEADER,
            NodeKind::Enum => theme::ENUM_HEADER,
        })
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
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), theme::BG);
        // NOTE: canvas geometry is not clipped to the widget by default, but
        // the pane container uses .clip(true) (view.rs) so boxes dragged to
        // negative diagram coords never paint over the editor pane. Don't
        // use Frame::with_clip here — its draft/paste misplaces mesh
        // geometry on the wgpu backend in 0.13.
        paint(&mut frame, self.layout, self.zoom, self.pan, self.hover);

        // pk/fk marker tooltip (dbdiagram-style), as a SECOND geometry:
        // within one geometry, text renders after meshes, so a tooltip
        // mesh could never cover the row text beneath it
        let mut tooltip_frame = None;
        if let Some(p) = self.hover {
            if let Some((b, row)) = marker_hit(self.layout, self.zoom, self.pan, p) {
                let rowd = &b.rows[row];
                let label = marker_label(rowd.pk);
                let top_left = to_px(b.x, b.y, self.zoom, self.pan);
                let s = cell_size(self.zoom);
                // the tooltip rect is frame-local: clamp against the pane
                // size, not the window-space widget bounds
                let local = Rectangle::new(Point::ORIGIN, bounds.size());
                let rect = marker_tooltip_rect(
                    top_left.x + 1.6 * s.width,
                    top_left.y,
                    top_left.y + b.h as f32 * s.height,
                    label,
                    self.zoom,
                    local,
                );
                let accent = if rowd.pk { theme::PK } else { theme::FK };
                let mut tframe = canvas::Frame::new(renderer, bounds.size());
                tframe.fill(
                    &Path::rounded_rectangle(
                        rect.position(),
                        rect.size(),
                        iced::border::Radius::from(4.0),
                    ),
                    theme::TOOLTIP,
                );
                tframe.stroke(
                    &Path::rounded_rectangle(
                        rect.position(),
                        rect.size(),
                        iced::border::Radius::from(4.0),
                    ),
                    Stroke {
                        style: accent.into(),
                        width: 1.0,
                        ..Default::default()
                    },
                );
                tframe.fill_text(Text {
                    content: label.to_string(),
                    position: Point::new(rect.x + 8.0, rect.y + rect.height * 0.5),
                    color: theme::TEXT,
                    size: (0.5 * self.zoom).into(),
                    font: Font::MONOSPACE,
                    horizontal_alignment: iced::alignment::Horizontal::Left,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    ..Default::default()
                });
                tooltip_frame = Some(tframe);
            }
        }
        match tooltip_frame {
            Some(t) => vec![frame.into_geometry(), t.into_geometry()],
            None => vec![frame.into_geometry()],
        }
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
                return Interaction::Pointer;
            }
        }
        Interaction::default()
    }
}

fn paint(frame: &mut canvas::Frame, layout: &Layout, zoom: f32, pan: Vector, hover: Option<Point>) {
    for edge in &layout.edges {
        paint_edge(frame, edge, zoom, pan);
    }
    let hovered = hover.and_then(|p| hit_test(layout, zoom, pan, p));
    for b in &layout.boxes {
        let is_hovered = hovered.map(|h| h.title == b.title).unwrap_or(false);
        paint_table(frame, b, zoom, pan, is_hovered);
    }
}

fn paint_edge(frame: &mut canvas::Frame, edge: &EdgeRoute, zoom: f32, pan: Vector) {
    let path = Path::new(|builder| {
        let mut it = edge.pts.iter();
        if let Some(&(x, y)) = it.next() {
            builder.move_to(to_px(x, y, zoom, pan));
        }
        for &(x, y) in it {
            builder.line_to(to_px(x, y, zoom, pan));
        }
    });
    frame.stroke(
        &path,
        Stroke {
            style: theme::EDGE.into(),
            width: edge_stroke_width(zoom),
            ..Default::default()
        },
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
    let corner = (0.35 * s.height).min(size.width / 2.0);
    let radius = iced::border::Radius::from(corner);

    // body
    frame.fill(
        &Path::rounded_rectangle(top_left, size, radius),
        theme::PANEL,
    );
    frame.stroke(
        &Path::rounded_rectangle(top_left, size, radius),
        Stroke {
            style: if hovered { theme::TEXT } else { theme::BORDER }.into(),
            width: if hovered { 2.0 } else { 1.0 },
            ..Default::default()
        },
    );

    // header band, rounded on top only: a rounded band plus a square strip
    // covering its bottom corners
    let band = Size::new(size.width, s.height);
    frame.fill(
        &Path::rounded_rectangle(top_left, band, radius),
        header_color(b),
    );
    frame.fill_rectangle(
        top_left + Vector::new(0.0, s.height - corner),
        Size::new(size.width, corner),
        header_color(b),
    );

    // title, centered in the header band
    let title_size = 0.55 * s.height;
    frame.fill_text(Text {
        content: b.title.clone(),
        position: Point::new(top_left.x + 2.0 * s.width, top_left.y + s.height * 0.5),
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

    // column rows
    let row_size = 0.55 * s.height;
    for (i, row) in b.rows.iter().enumerate() {
        let y = top_left.y + (i as f32 + 1.0) * s.height + s.height * 0.5;
        let baseline = |x: f32| Point::new(x, y);
        // dbdiagram-style markers: a key icon for primary keys, a chain
        // link for foreign keys (stroke-drawn so they stay crisp at any
        // zoom; the old diamond glyphs were text and scaled poorly)
        let center = baseline(top_left.x + 1.6 * s.width);
        if row.pk {
            draw_key_icon(frame, center, 0.62 * s.height, theme::PK);
        } else if row.fk {
            draw_link_icon(frame, center, 0.58 * s.height, theme::FK);
        }
        frame.fill_text(Text {
            content: row.name.clone(),
            position: baseline(top_left.x + 3.2 * s.width),
            color: theme::TEXT,
            size: row_size.into(),
            font: Font::MONOSPACE,
            horizontal_alignment: iced::alignment::Horizontal::Left,
            vertical_alignment: iced::alignment::Vertical::Center,
            ..Default::default()
        });
        if !row.ty.is_empty() {
            frame.fill_text(Text {
                content: row.ty.clone(),
                position: baseline(top_left.x + size.width - 0.8 * s.width),
                color: theme::DIM,
                size: row_size.into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Right,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        }
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
    fn marker_hit_tests_the_icon_band() {
        // fixture: Table a { id integer [pk] } — one pk row
        let l = layout();
        let zoom = 20.0;
        let pan = Vector::new(50.0, 50.0);
        let b = &l.boxes[0];
        let top_left = to_px(b.x, b.y, zoom, pan);
        let s = cell_size(zoom);
        let row_top = top_left.y + s.height;
        // dead center of the marker band on that row (glyph at 1.6*sw)
        let p = Point::new(top_left.x + 1.9 * s.width, row_top + s.height * 0.5);
        let hit = marker_hit(&l, zoom, pan, p).expect("pk marker hit");
        assert_eq!(hit.0.title, "a");
        assert!(hit.0.rows[hit.1].pk);
        assert_eq!(marker_label(true), "Primary Key");
        // the name column is NOT the marker band
        let name_p = Point::new(top_left.x + 3.5 * s.width, row_top + s.height * 0.5);
        assert!(marker_hit(&l, zoom, pan, name_p).is_none());
        // left of the band (before the glyph) misses too
        let left_p = Point::new(top_left.x + 0.3 * s.width, row_top + s.height * 0.5);
        assert!(marker_hit(&l, zoom, pan, left_p).is_none());
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
}
