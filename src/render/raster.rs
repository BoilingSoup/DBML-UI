//! SVG -> RGBA rasterization via resvg, at viewport scale.

use anyhow::{Context, Result};
use std::sync::{Arc, OnceLock};

/// System font database for text rendering. `usvg::Options::default()` ships
/// an empty fontdb, and fontdb's default `monospace` family ("Courier New")
/// matches nothing on a typical Linux box — without this, every `<text>`
/// node is silently dropped from the raster.
fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        // The web build has no system fonts: the embedded editor mono face
        // is the whole font set (and the generic-family target).
        #[cfg(target_arch = "wasm32")]
        db.load_font_data(crate::MONO_TTF.to_vec());
        #[cfg(not(target_arch = "wasm32"))]
        db.load_system_fonts();
        // Point the generic `monospace` family at a font that actually exists.
        const PREFERRED: [&str; 6] = [
            "DejaVu Sans Mono",
            "Liberation Mono",
            "Noto Sans Mono",
            "Menlo",
            "Consolas",
            "Courier New",
        ];
        let mono = PREFERRED
            .iter()
            .find(|name| {
                db.faces()
                    .any(|f| f.families.iter().any(|(n, _)| n == *name))
            })
            .map(|n| n.to_string())
            .or_else(|| {
                db.faces()
                    .flat_map(|f| f.families.iter().map(|(n, _)| n.clone()))
                    .find(|n| n.ends_with("Mono"))
            });
        if let Some(name) = mono {
            db.set_monospace_family(&name);
            // usvg's last-resort fallback queries the generic serif family.
            db.set_serif_family(&name);
        }
        Arc::new(db)
    })
    .clone()
}

/// Rasterize `svg` (whose native size is `svg_size`) into a `out_w x out_h`
/// RGBA image, scaling x/y independently by (`sx`, `sy`).
pub fn rasterize(svg: &str, out_w: u32, out_h: u32, sx: f32, sy: f32) -> Result<image::RgbaImage> {
    let opts = usvg::Options {
        fontdb: fontdb(),
        ..Default::default()
    };
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

    #[test]
    fn text_is_rasterized() {
        // Regression guard: an empty/default fontdb makes usvg silently drop
        // every <text> node, leaving readable boxes but no labels.
        let out = parser::parse("Table users { id integer [pk]\n name varchar }");
        let l = layout(&build(&out.doc));
        let doc = svg::build_svg(&l);
        let (sw, sh) = svg::svg_size(&l);
        let img = rasterize(&doc, 600, 400, 600.0 / sw, 400.0 / sh).unwrap();
        // Dark-theme label text is near-white (#e6edf3); count antialiased
        // pixels brighter than the header/body bands.
        let text_px = img
            .pixels()
            .filter(|p| p.0[0] > 220 && p.0[1] > 220 && p.0[2] > 220)
            .count();
        assert!(
            text_px > 200,
            "expected rasterized text, got only {text_px} bright px (fontdb resolved no font?)"
        );
    }
}
