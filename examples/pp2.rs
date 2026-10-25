fn main() {
    let src = std::fs::read_to_string(
        "tests/dbml_fixtures/exporter/oracle_exporter/input/special_schema.in.dbml",
    )
    .unwrap();
    let out = dbml_ui::dbml::parser::parse(&src);
    for e in &out.errors {
        println!("{}: {}", e.offset, e.message);
    }
    println!(
        "{}",
        &src[out.errors[0].offset.saturating_sub(40)..out.errors[0].offset + 40]
    );
}
