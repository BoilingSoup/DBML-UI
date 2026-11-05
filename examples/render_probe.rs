//! Render a diagram to PNG for visual inspection (dev probe).
fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let out = std::env::args().nth(2).unwrap();
    let layout = dbml_ui::layout::layout(&dbml_ui::model::build(
        &dbml_ui::dbml::parser::parse(&src).doc,
    ));
    let svg = dbml_ui::render::svg::build_svg(&layout);
    let img = dbml_ui::render::raster::rasterize(&svg, 1400, 900, 1.0, 1.0).unwrap();
    img.save(out).unwrap();
}
