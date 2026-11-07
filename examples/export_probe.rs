//! Export feature_tour.dbml to /tmp/export_probe.png (2x) for pixel checks.
fn main() {
    let src = std::fs::read_to_string("tests/fixtures/feature_tour.dbml").unwrap();
    let schema = dbml_ui::model::build(&dbml_ui::dbml::parser::parse(&src).doc);
    let l = dbml_ui::layout::layout(&schema);
    let doc = dbml_ui::render::svg::build_svg(&l);
    let (sw, sh) = dbml_ui::render::svg::svg_size(&l);
    let img = dbml_ui::render::raster::rasterize(&doc, (sw * 2.0) as u32, (sh * 2.0) as u32, 2.0, 2.0).unwrap();
    img.save("/tmp/export_probe.png").unwrap();
    println!("exported {}x{}", img.width(), img.height());
}
