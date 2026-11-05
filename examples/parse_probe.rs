
use dbml_ui::dbml::parser::parse;
fn main() {
    let src = "Table a { id integer [pk] }\nTable c { name varchar }\nTable b { a_id integer }\nRef: b.a_id > a.name\n";
    let out = parse(src);
    println!("errors: {:?}", out.errors);
    for d in dbml_ui::dbml::validate::validate(&out.doc, src) {
        println!("diag: {}", d.message);
    }
    for r in &out.doc.refs {
        println!("ref: {:?} -> {:?}", r.from, r.to);
    }
}
