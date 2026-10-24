//! Headless render: parse a .dbml file and write the diagram to a PNG.
//!
//! Usage: cargo run --example dump_png -- [input.dbml] [out.png] [scale]

use tui_db::dbml::parser;
use tui_db::layout::layout;
use tui_db::model::build;
use tui_db::render::raster::rasterize;
use tui_db::render::svg::{build_svg, svg_size};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .unwrap_or_else(|| "tests/fixtures/basic.dbml".into());
    let out_path = args
        .next()
        .unwrap_or_else(|| "/tmp/tui-db-diagram.png".into());
    let scale: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2.0);

    let src = std::fs::read_to_string(&input)?;
    let parsed = parser::parse(&src);
    for e in &parsed.errors {
        eprintln!("warning: {e:?}");
    }
    let lay = layout(&build(&parsed.doc));
    let svg = build_svg(&lay);
    let (sw, sh) = svg_size(&lay);
    let img = rasterize(&svg, (sw * scale) as u32, (sh * scale) as u32, scale, scale)?;
    img.save(&out_path)?;
    println!("wrote {out_path} ({}x{})", img.width(), img.height());
    Ok(())
}
