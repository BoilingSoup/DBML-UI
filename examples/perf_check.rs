fn main() {
    let mut src = String::new();
    for i in 0usize..200 {
        src.push_str(&format!("Table t{i} {{\n  id integer [pk]\n  name varchar(100) [not null, unique]\n  owner_id integer [ref: > t{}.id]\n}}\n", i.saturating_sub(1)));
        if i % 5 == 0 {
            src.push_str(&format!(
                "Ref: t{i}.owner_id > t{}.id\n",
                i.saturating_sub(1)
            ));
        }
    }
    let t0 = std::time::Instant::now();
    let n = 50;
    for _ in 0..n {
        let out = tui_db::dbml::parser::parse(&src);
        assert_eq!(out.doc.tables.len(), 200);
    }
    let dt = t0.elapsed() / n;
    println!("parse of 200-table doc: {dt:?}");
}
