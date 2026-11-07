fn main() {
    let src = std::fs::read_to_string("tests/fixtures/feature_tour.dbml").unwrap();
    let layout = dbml_ui::layout::layout(&dbml_ui::model::build(
        &dbml_ui::dbml::parser::parse(&src).doc,
    ));
    for g in &layout.groups {
        println!("group {:?} rect={:?}", g.name, g.rect);
    }
    for b in &layout.boxes {
        println!("box {:?} x={} y={} w={} h={}", b.title, b.x, b.y, b.w, b.h);
    }
}
