fn main() {
    let src = std::fs::read_to_string(
        "tests/dbml_fixtures/importer/postgres_importer/input/general_schema.in.sql",
    )
    .unwrap();
    let out = dbml_ui::dbml::sqldump::import(&src, dbml_ui::dbml::sql::Dialect::Postgres).unwrap();
    let fmt = dbml_ui::dbml::fmt::format_source(&out);
    if fmt.is_none() {
        let p = dbml_ui::dbml::parser::parse(&out);
        println!("errors: {}", p.errors.len());
        for e in p.errors.iter().take(2) {
            println!("{}: {}", e.offset, e.message);
            println!("{:?}", &out[e.offset.saturating_sub(35)..e.offset + 35]);
        }
    } else {
        println!("parses fine");
    }
}
