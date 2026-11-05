fn main() {
    let src = std::fs::read_to_string("/tmp/records_demo.dbml").unwrap();
    let out = dbml_ui::dbml::parser::parse(&src);
    println!("errors={} tables={} refs={}", out.errors.len(), out.doc.tables.len(), out.doc.refs.len());
    for e in &out.errors {
        let (l, c) = dbml_ui::dbml::parser::line_col_at(&src, e.offset);
        println!("  {}:{} {}", l + 1, c + 1, e.message);
    }
    for t in &out.doc.tables {
        println!("table {:?} records={:?}", t.name, t.records.as_ref().map(|r| r.rows.len()));
    }
    let l = dbml_ui::layout::layout(&dbml_ui::model::build(&out.doc));
    println!("boxes={}", l.boxes.len());
    for b in &l.boxes {
        println!("  box {:?} x={} y={} w={} h={} records={:?}", b.title, b.x, b.y, b.w, b.h, b.records.as_ref().map(|r| r.rows.len()));
    }
}
