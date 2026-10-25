fn main() {
    let src = std::fs::read_to_string(
        "tests/dbml_fixtures/importer/postgres_importer/output/checks.out.dbml",
    )
    .unwrap();
    let out = dbml_ui::dbml::parser::parse(&src);
    println!("errors: {}", out.errors.len());
    for e in &out.errors {
        println!("{}: {}", e.offset, e.message);
    }
    println!(
        "{}",
        &src[out.errors[0].offset.saturating_sub(30)..out.errors[0].offset + 40]
    );
}
