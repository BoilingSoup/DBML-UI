//! Render two deliberately overlapping tables to inspect drag-priority
//! rendering (dev probe).
fn main() {
    let src = "Table alpha {\n  id integer [pk]\n  name varchar(120)\n  created_at timestamp\n}\n\nTable beta {\n  beta_id integer [pk]\n  alpha_ref integer\n  note text\n}\n";
    let mut layout = dbml_ui::layout::layout(&dbml_ui::model::build(
        &dbml_ui::dbml::parser::parse(src).doc,
    ));
    // beta dragged onto alpha: beta (doc order 2nd) overlaps alpha
    layout.boxes[1].x = 3;
    layout.boxes[1].y = 1;
    let svg = dbml_ui::render::svg::build_svg(&layout);
    let img = dbml_ui::render::raster::rasterize(&svg, 900, 500, 1.0, 1.0).unwrap();
    img.save("/tmp/overlap.png").unwrap();
}
