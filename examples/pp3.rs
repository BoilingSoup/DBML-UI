fn main() {
    let src = "Ref: \"users\".\"id\" < \"orders\".\"user_id\" [delete: cascade]\n";
    let out = dbml_ui::dbml::parser::parse(src);
    println!("errors: {}", out.errors.len());
    for e in &out.errors {
        println!("{}: {}", e.offset, e.message);
    }
    println!("refs: {}", out.doc.refs.len());
    if let Some(r) = out.doc.refs.first() {
        println!("{:?} del={:?} upd={:?}", r.card, r.on_delete, r.on_update);
    }
}
