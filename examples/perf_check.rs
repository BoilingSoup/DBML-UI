use std::time::Instant;
use tui_db::dbml::parser;
use tui_db::layout::layout;
use tui_db::model::build;
use tui_db::render::{raster, svg};

fn main() {
    let mut src = String::new();
    for i in 0usize..200 {
        src.push_str(&format!(
            "Table t{i} {{\n  id integer [pk]\n  name varchar(100) [not null, unique]\n  owner_id integer [ref: > t{}.id]\n}}\n",
            i.saturating_sub(1)
        ));
    }
    let n = 20;
    let t0 = Instant::now();
    for _ in 0..n {
        let out = parser::parse(&src);
        assert_eq!(out.doc.tables.len(), 200);
    }
    println!("parse 200 tables: {:?}", t0.elapsed() / n);

    let t0 = Instant::now();
    let out = parser::parse(&src);
    println!("parse+build+layout 200 tables: {:?}", t0.elapsed());

    let t0 = Instant::now();
    let l = layout(&build(&out.doc));
    let doc = svg::build_svg(&l);
    let (sw, sh) = svg::svg_size(&l);
    println!(
        "svg build: {:?} ({}x{} units, {} bytes)",
        t0.elapsed(),
        sw,
        sh,
        doc.len()
    );

    // rasterize at a typical viewport-ish target
    let (w, h) = (1600u32, 900u32);
    let t0 = Instant::now();
    let img = raster::rasterize(&doc, w, h, w as f32 / sw, h as f32 / sh).unwrap();
    println!(
        "rasterize {w}x{h}: {:?} ({} px)",
        t0.elapsed(),
        img.pixels().len()
    );
}
