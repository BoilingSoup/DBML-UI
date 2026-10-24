//! SVG -> RGBA rasterization via resvg, at viewport scale.

use anyhow::{Context, Result};

/// Rasterize `svg` (whose native size is `svg_size`) into a `out_w x out_h`
/// RGBA image, scaling x/y independently by (`sx`, `sy`).
pub fn rasterize(svg: &str, out_w: u32, out_h: u32, sx: f32, sy: f32) -> Result<image::RgbaImage> {
    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_str(svg, &opts).context("failed to parse generated svg")?;
    let mut pixmap =
        tiny_skia::Pixmap::new(out_w.max(1), out_h.max(1)).context("failed to allocate pixmap")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(sx, sy),
        &mut pixmap.as_mut(),
    );
    image::RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.take())
        .context("pixmap conversion")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbml::parser;
    use crate::layout::layout;
    use crate::model::build;
    use crate::render::svg;

    #[test]
    fn rasterizes_nonempty() {
        let out = parser::parse("Table a { id integer [pk] }");
        let l = layout(&build(&out.doc));
        let doc = svg::build_svg(&l);
        let (sw, sh) = svg::svg_size(&l);
        let img = rasterize(&doc, 400, 300, 400.0 / sw, 300.0 / sh).unwrap();
        assert_eq!(img.dimensions(), (400, 300));
        let nonzero = img.pixels().filter(|p| p.0[3] != 0).count();
        assert!(nonzero > 1000, "expected drawn content, got {nonzero} px");
    }
}
