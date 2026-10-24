//! Headless render: parse a .dbml file and write the diagram to a PNG.
//!
//! Usage: cargo run --example dump_png -- [input.dbml] [out.png] [scale] [mode]
//!
//! `scale` multiplies the native SVG size. `mode`:
//! - `text` (default): full diagram with labels
//! - `outline`: label-free, rasterized at 1x2 px per diagram cell — exactly
//!   what the halfblocks minimap builds for text-mode zoom-out

use tui_db::dbml::parser;
use tui_db::layout::layout;
use tui_db::model::build;
use tui_db::render::raster::rasterize;
use tui_db::render::svg::{build_svg_with, svg_size, MARGIN_CELLS};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .unwrap_or_else(|| "tests/fixtures/basic.dbml".into());
    let out_path = args
        .next()
        .unwrap_or_else(|| "/tmp/tui-db-diagram.png".into());
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2.0);
    let outline = args.next().as_deref() == Some("outline");

    let src = std::fs::read_to_string(&input)?;
    let parsed = parser::parse(&src);
    for e in &parsed.errors {
        eprintln!("warning: {e:?}");
    }
    let lay = layout(&build(&parsed.doc));
    let svg = build_svg_with(&lay, !outline);
    let (sw, sh) = svg_size(&lay);

    let (w, h, sx, sy) = if outline {
        // halfblocks raster units: 1 px per cell wide, 2 px per cell tall
        let w = ((lay.extent.0 as f32 + 2.0 * MARGIN_CELLS) * scale) as u32;
        let h = ((lay.extent.1 as f32 + 2.0 * MARGIN_CELLS) * scale * 2.0) as u32;
        (w.max(1), h.max(1), w as f32 / sw, h as f32 / sh)
    } else {
        ((sw * scale) as u32, (sh * scale) as u32, scale, scale)
    };
    let img = rasterize(&svg, w, h, sx, sy)?;
    img.save(&out_path)?;
    println!("wrote {out_path} ({}x{})", img.width(), img.height());
    Ok(())
}
