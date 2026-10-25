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
        let (marker, mcolor) = if row.pk {
            (Some("\u{25c6}"), theme::PK) // ◆
        } else if row.fk {
            (Some("\u{25c7}"), theme::FK) // ◇
        } else {
            (None, Color::WHITE)
        };
        if let Some(m) = marker {
            frame.fill_text(Text {
                content: m.to_string(),
                position: baseline(top_left.x + 1.6 * s.width),
                color: mcolor,
                size: row_size.into(),
                font: Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
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
