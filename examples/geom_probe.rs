//! Print group/table/note geometry in px for the CURRENT rig layout
//! (default zoom/pan of a fresh instance) — for deterministic drags.
fn main() {
    let src = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    let schema = dbml_ui::model::build(&dbml_ui::dbml::parser::parse(&src).doc);
    let l = dbml_ui::layout::layout_with_all_overrides(&schema, &Default::default(), &Default::default());
    let zoom: f32 = 20.0;
    // cell_size = (zoom*CELL_ASPECT, zoom); to_px adds MARGIN_CELLS —
    // both consts are bin-crate, mirror them here
    let (aspect, margin) = (0.5f32, 1.0f32);
    let cw = zoom * aspect;
    let ch = zoom;
    let px = |x: i32, y: i32| (margin * cw + x as f32 * cw, margin * ch + y as f32 * ch);
    println!("cell {}x{}", cw, ch);
    for g in &l.groups {
        println!("group {:?} cells {:?} px tl {:?}", g.name, g.rect,
            px(g.rect.0, g.rect.1));
    }
    for b in &l.boxes {
        println!("table {:?} cells ({},{},{},{}) px tl {:?}", b.title, b.x, b.y, b.w, b.h,
            px(b.x, b.y));
    }
    for n in &l.notes {
        println!("note {:?} cells {:?} px tl {:?}", n.name, n.rect,
            px(n.rect.0, n.rect.1));
    }
}
