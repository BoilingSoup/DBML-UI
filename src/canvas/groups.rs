//! dbdiagram's TableGroup regions: a labeled rounded frame around each
//! group's member tables (drawn behind everything). Agent-owned file.

use crate::canvas::diagram::{cell_size, svg_icon, to_px, visible_region};
use dbml_ui::layout::NOTE_ICON;
use crate::theme;
use dbml_ui::layout::{GroupRegion, Layout};
use iced::widget::canvas::{self, Path, Stroke, Text};
use iced::{border, font, Color, Font, Point, Rectangle, Size, Vector};

/// A region's default: the SAME near-black a table's header band uses
/// (`theme::HEADER_BG`) — an uncolored group reads as part of the
/// diagram, not dbdiagram's old group blue. A region without (or with
/// an unparseable) `[color: ...]` renders in it.
const DEFAULT_GROUP: Color = theme::HEADER_BG;

/// The label's inset from the region's left edge, in cells.
const LABEL_INSET: f32 = 0.6;

/// Stroke + label color of a region: its `[color: ...]` when it parses,
/// the table-header near-black otherwise (see `DEFAULT_GROUP`).
fn region_color(g: &GroupRegion) -> Color {
    g.color
        .as_deref()
        .and_then(theme::hex_color)
        .unwrap_or(DEFAULT_GROUP)
}


/// The group note glyph's center x: the label's TRUE shaped advance
/// (the table-title convention, `measured_name_px`) plus the shared
/// 0.6-cell gap — the gap is identical for every group, which the
/// est_width classes could never guarantee.
fn group_note_cx(name: &str, label_x: f32, s: Size) -> f32 {
    label_x + crate::canvas::diagram::measured_name_px(name, true, 0.55 * s.height)
        + 0.6 * s.width
}

/// Draw every group region (called from `diagram::paint` before edges):
/// dbdiagram's CONTAINER look — translucent tinted body, a solid
/// group-color header band carrying the name (like a table header), and
/// a 2px border around the whole region. `occluders` is every table and
/// sticky rect in paint order — the group paints below them all, and
/// since iced draws all text above all meshes, the label clips to the
/// band's visible slivers around them (a table parked over the band
/// must not show the name through its body).
pub fn draw_groups(
    frame: &mut canvas::Frame,
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    pointer: Option<iced::Point>,
    occluders: &[Rectangle],
) {
    // authored in 1:1 px (dbdiagram's stage transform)
    let k = zoom / 20.0;
    for g in &layout.groups {
        let s = cell_size(zoom);
        let top_left = to_px(g.rect.0, g.rect.1, zoom, pan);
        let size = Size::new(g.rect.2 as f32 * s.width, g.rect.3 as f32 * s.height);
        let color = region_color(g);
        let radius = border::Radius::from(10.0 * k);

        // translucent tinted body: the group color washed over the canvas
        // behind the member tables (dbdiagram's container fill)
        frame.fill(
            &Path::rounded_rectangle(top_left, size, radius),
            Color { a: 0.12, ..color },
        );

        // solid header band: the layout reserves a full row above the
        // members for exactly this (rect.y +1 is where members start).
        // The fill is readability-clamped (readable_band) so the white
        // label + glyph read on any group color; the region's body tint
        // and outline keep the authored color
        let band_h = s.height;
        let band = Size::new(size.width, band_h);
        // square bottom corners on the band; the top matches the region
        frame.fill(
            &Path::rounded_rectangle(top_left, band, border::Radius {
                top_left: 10.0 * k,
                top_right: 10.0 * k,
                bottom_right: 0.0,
                bottom_left: 0.0,
            }),
            crate::theme::readable_band(color),
        );

        // the name, bold white on the clamped band — white chrome stays
        // white everywhere. The TEXT pass clips to the band's visible
        // slivers around every table/sticky above the group (iced draws
        // all text above all meshes): a table parked over the band hides
        // the name with its body, the same occlusion a header band gets.
        let label_x = top_left.x + LABEL_INSET * s.width;
        let band_rect = Rectangle::new(top_left, band);
        let band_occluders: Vec<Rectangle> = occluders
            .iter()
            .filter(|r| r.intersects(&band_rect))
            .copied()
            .collect();
        let draw_label = |f: &mut canvas::Frame, at: Point| {
            f.fill_text(Text {
                content: g.name.clone(),
                position: Point::new(at.x, at.y + band_h * 0.5),
                color: Color::WHITE,
                size: (0.55 * s.height).into(),
                font: Font {
                    weight: font::Weight::Bold,
                    ..Font::DEFAULT
                },
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Center,
                ..Default::default()
            });
        };
        if band_occluders.is_empty() {
            draw_label(frame, Point::new(label_x, top_left.y));
        } else {
            for v in visible_region(band_rect, &band_occluders) {
                if v.width < 2.0 || v.height < 2.0 {
                    continue;
                }
                frame.with_clip(v, |f| {
                    // the draft frame's origin is the sliver's: draw the
                    // label shifted back under it
                    draw_label(f, Point::new(label_x - v.x, top_left.y - v.y));
                });
            }
        }

        // note affordance (dbdiagram): groups with a note or custom
        // properties carry the paper glyph right AFTER THE NAME — the
        // affordance belongs to the label, matching the table headers.
        // Always visible (hovering the band pops the card). The
        // color-picker wheel keeps the band's right end while hovered.
        // White ink on the clamped band, like the label.
        let has_info = g.note.is_some() || !g.custom.is_empty();
        let note_size = 0.5 * s.height;
        if has_info {
            let n_cx = group_note_cx(&g.name, label_x, s) + 0.5 * note_size;
            svg_icon(
                frame,
                Point::new(n_cx, top_left.y + band_h * 0.5),
                note_size,
                10.5,
                Color::WHITE,
                NOTE_ICON,
            );
        }
        // the color-picker affordance (the circle wheel in its own
        // colors): at the BAND'S RIGHT END, right-aligned to the same
        // 0.6-cell inset edge the table glyph uses, shown while the band
        // is hovered. Clicking it opens the picker, which cascades to
        // the member tables (their headercolor inheritance).
        let band_hovered = pointer.is_some_and(|p| {
            let band = label_band(g, zoom, pan);
            band.contains(p)
        });
        if band_hovered {
            let (c, _, _) = group_icon_geometry(g, top_left, s);
            // drawn from the raw icon box (the hit zone's 7px floor
            // would overdraw at far zoom-out — see palette_icon_geometry)
            let icon_h = 0.72 * s.height;
            let ik = icon_h / crate::canvas::diagram::CIRCLES_VIEWBOX.1;
            let x0 = c.x - crate::canvas::diagram::CIRCLES_VIEWBOX.0 * ik * 0.5;
            let y0 = c.y - icon_h * 0.5;
            for (cx, cy, r, color) in crate::canvas::diagram::CIRCLES_ICON {
                frame.fill(
                    &Path::circle(Point::new(x0 + cx * ik, y0 + cy * ik), r * ik),
                    color,
                );
            }
        }

        // 2px border over everything: the frame reads crisply against the
        // tint and the band
        frame.stroke(
            &Path::rounded_rectangle(top_left, size, radius),
            Stroke {
                style: color.into(),
                width: 2.0 * k,
                ..Default::default()
            },
        );
    }
}

/// The group whose HEADER BAND contains the point, if any — the note
/// tooltip in the overlay AND the group-drag press target (full width,
/// one cell tall: it IS the container's header row).
pub fn group_label_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: iced::Point,
) -> Option<&dbml_ui::layout::GroupRegion> {
    layout
        .groups
        .iter()
        .find(|g| label_band(g, zoom, pan).contains(p))
}

/// A region's header band in frame px: the full-width, one-cell-tall
/// strip at the region's top — the solid color band `draw_groups` paints.
/// The region's BODY is not a header target: member boxes keep their own
/// hit tests over it (empty padding areas fall through to body-press
/// handling at the app layer).
fn label_band(g: &GroupRegion, zoom: f32, pan: Vector) -> Rectangle {
    let s = cell_size(zoom);
    let top_left = to_px(g.rect.0, g.rect.1, zoom, pan);
    Rectangle::new(top_left, Size::new(g.rect.2 as f32 * s.width, s.height))
}

/// The group's color-picker icon hit zone — exactly where `draw_groups`
/// paints the circles (the band's right end). Clicking it opens the
/// picker for the GROUP's color, which cascades to its members.
pub fn group_icon_hit(
    layout: &Layout,
    zoom: f32,
    pan: Vector,
    p: iced::Point,
) -> Option<&GroupRegion> {
    let s = cell_size(zoom);
    layout.groups.iter().rev().find(|g| {
        let top_left = to_px(g.rect.0, g.rect.1, zoom, pan);
        let (c, half_w, half_h) = group_icon_geometry(g, top_left, s);
        (p.x - c.x).abs() <= half_w && (p.y - c.y).abs() <= half_h
    })
}

/// The picker icon's geometry, shared by the draw pass and the hit test
/// so they can never drift: right-aligned to the band's inset edge
/// (0.6 cells in — the table glyph's exact convention, see
/// `palette_icon_geometry`) and vertically centered on the band. The
/// viewBox is 578x531, so the width derives from the height; the hit
/// half-extents never drop below 7px (a far-zoomed-out icon stays
/// clickable).
fn group_icon_geometry(g: &GroupRegion, top_left: Point, s: Size) -> (Point, f32, f32) {
    let right = top_left.x + g.rect.2 as f32 * s.width - 0.6 * s.width;
    let icon_h = 0.72 * s.height;
    let icon_w =
        crate::canvas::diagram::CIRCLES_VIEWBOX.0 / crate::canvas::diagram::CIRCLES_VIEWBOX.1
            * icon_h;
    let c = Point::new(right - icon_w / 2.0, top_left.y + s.height * 0.5);
    (c, (icon_w / 2.0).max(7.0), (icon_h / 2.0).max(7.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The group note glyph's gap is IDENTICAL for every group: the
    /// label end is its TRUE shaped advance (the table-title
    /// convention), not the est_width estimate.
    #[test]
    fn the_group_glyph_gap_is_identical_across_labels() {
        let zoom = 20.0;
        let s = cell_size(zoom);
        let mut gap: Option<f32> = None;
        for name in ["users", "launch_windows", "payloads", "WWW_aaaa"] {
            let label_x = 100.0;
            let end = label_x
                + crate::canvas::diagram::measured_name_px(name, true, 0.55 * s.height);
            let cx = group_note_cx(name, label_x, s);
            // the convention itself: inset-relative end + the shared gap
            assert_eq!(
                cx,
                label_x
                    + crate::canvas::diagram::measured_name_px(name, true, 0.55 * s.height)
                    + 0.6 * s.width,
                "the glyph gap is measured for {name:?}"
            );
            let g = cx - end - 0.6 * s.width;
            let _ = g;
            match gap {
                None => gap = Some(cx - end),
                Some(prev) => assert!(
                    (cx - end - prev).abs() < 0.01,
                    "the glyph gap differs for {name:?}"
                ),
            }
        }
    }

    /// Real parser+layout pipeline with two groups: "one" wraps tables
    /// a+b (explicit color), "two" wraps c (no color).
    fn group_layout() -> Layout {
        let src = "Table a { id integer }\nTable b { x integer }\nTable c { y integer }\n\
                   TableGroup one [color: #1E69FD] { a b }\nTableGroup two { c }\n";
        dbml_ui::layout::layout(&dbml_ui::model::build(
            &dbml_ui::dbml::parser::parse(src).doc,
        ))
    }

    #[test]
    fn region_color_parses_or_falls_back_to_the_table_default() {
        let g = GroupRegion {
            name: "g".into(),
            color: Some("#1E69FD".into()),
            note: None,
            custom: vec![],
            rect: (0, 0, 10, 10),
            members: vec![],
        };
        let c = region_color(&g);
        assert!((c.r - 0x1E as f32 / 255.0).abs() < 1e-6);
        assert!((c.b - 0xFD as f32 / 255.0).abs() < 1e-6);
        // no color and a garbage color both take the TABLE header default:
        // an uncolored group renders the same near-black band a table does
        // (theme::HEADER_BG, not dbdiagram's old group blue)
        let no_color = region_color(&GroupRegion { color: None, ..g.clone() });
        assert_eq!(no_color, theme::HEADER_BG);
        assert!((no_color.r - 0x1f as f32 / 255.0).abs() < 1e-3);
        assert!((no_color.g - 0x21 as f32 / 255.0).abs() < 1e-3);
        assert!((no_color.b - 0x28 as f32 / 255.0).abs() < 1e-3);
        assert_eq!(
            region_color(&GroupRegion {
                color: Some("nope".into()),
                ..g
            }),
            DEFAULT_GROUP
        );
    }

    #[test]
    fn label_band_hits_and_the_region_body_does_not() {
        let l = group_layout();
        assert_eq!(l.groups.len(), 2);
        let zoom = 20.0;
        let pan = Vector::new(50.0, 30.0);
        let s = cell_size(zoom);

        let one = &l.groups[0];
        let tl = to_px(one.rect.0, one.rect.1, zoom, pan);
        // "one" is ~1.74 cells wide (3 chars at Row::CHAR_CELL): a point
        // inside the band's first cell hits the region
        let in_band = Point::new(tl.x + 0.8 * s.width, tl.y + 0.5 * s.height);
        assert_eq!(group_label_hit(&l, zoom, pan, in_band).unwrap().name, "one");

        // the region's middle sits well below the one-cell band: the frame
        // itself is not a hit target (boxes/edges own their own hits)
        let mid = Point::new(
            tl.x + one.rect.2 as f32 * s.width * 0.5,
            tl.y + one.rect.3 as f32 * s.height * 0.5,
        );
        assert!(group_label_hit(&l, zoom, pan, mid).is_none());

        // and the second region's own band returns that one
        let two = &l.groups[1];
        let tl2 = to_px(two.rect.0, two.rect.1, zoom, pan);
        let in_band2 = Point::new(tl2.x + 0.8 * s.width, tl2.y + 0.5 * s.height);
        assert_eq!(group_label_hit(&l, zoom, pan, in_band2).unwrap().name, "two");
    }

    #[test]
    fn the_header_band_spans_the_region_and_one_row() {
        // the container's header: full region width, one cell tall — the
        // drag target for the whole group (the note glyph no longer
        // narrows/widens it; the band IS the header row)
        let l = group_layout();
        let zoom = 20.0;
        let pan = Vector::new(0.0, 0.0);
        let s = cell_size(zoom);
        let one = &l.groups[0];
        let tl = to_px(one.rect.0, one.rect.1, zoom, pan);
        // far right end of the band (inside the region, past any label)
        let right_end = Point::new(
            tl.x + one.rect.2 as f32 * s.width - 0.3 * s.width,
            tl.y + 0.5 * s.height,
        );
        assert_eq!(group_label_hit(&l, zoom, pan, right_end).unwrap().name, "one");
        // one row down: member territory, not the band
        let below = Point::new(
            tl.x + 0.8 * s.width,
            tl.y + 1.5 * s.height,
        );
        assert!(group_label_hit(&l, zoom, pan, below).is_none());
        // above the region: nothing
        let above = Point::new(tl.x + 0.8 * s.width, tl.y - 0.5 * s.height);
        assert!(group_label_hit(&l, zoom, pan, above).is_none());
    }

    #[test]
    fn the_picker_icon_sits_at_the_band_s_right_end() {
        let l = group_layout();
        let zoom = 20.0;
        let pan = Vector::new(0.0, 0.0);
        let s = cell_size(zoom);
        let one = &l.groups[0];
        let tl = to_px(one.rect.0, one.rect.1, zoom, pan);
        // the icon's center: right-aligned to the band's 0.6-cell inset
        // edge, vertically centered on the band (the draw pass's geometry)
        let icon_h = 0.72 * s.height;
        let icon_w = crate::canvas::diagram::CIRCLES_VIEWBOX.0
            / crate::canvas::diagram::CIRCLES_VIEWBOX.1
            * icon_h;
        let c = Point::new(
            tl.x + one.rect.2 as f32 * s.width - 0.6 * s.width - icon_w / 2.0,
            tl.y + s.height * 0.5,
        );
        assert!(group_icon_hit(&l, zoom, pan, c).is_some());
        // the band's LEFT end is not the icon (the old next-to-the-label
        // slot is long gone)
        assert!(group_icon_hit(&l, zoom, pan, Point::new(tl.x + 1.0 * s.width, c.y)).is_none());
        // below the band: member territory
        assert!(group_icon_hit(&l, zoom, pan, Point::new(c.x, tl.y + 1.5 * s.height)).is_none());
    }
}
