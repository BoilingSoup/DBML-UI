fn main() {
    let path = std::env::args().nth(1).expect("file");
    let src = std::fs::read_to_string(&path).unwrap();
    let errs = dbml_ui::dbml::parser::parse(&src).errors;
    println!("{} parse errors", errs.len());
    for e in errs.iter().take(10) {
        println!("  @{}: {}", e.offset, e.message);
    }
    // idempotence
    let f1 = dbml_ui::dbml::fmt::format_source(&src).unwrap();
    let f2 = dbml_ui::dbml::fmt::format_source(&f1).unwrap();
    println!("idempotent: {}", f1 == f2);
}
