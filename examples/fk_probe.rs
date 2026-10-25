fn main() {
    let src = std::fs::read_to_string(
        "tests/dbml_fixtures/exporter/postgres_exporter/input/general_schema.in.dbml",
    )
    .unwrap();
    let out = dbml_ui::dbml::sql::export(&src, dbml_ui::dbml::sql::Dialect::Postgres).unwrap();
    println!(
        "{}",
        out.lines()
            .filter(|l| l.starts_with("ALTER"))
            .take(2)
            .collect::<Vec<_>>()
            .join("\n")
    );
    let e = std::fs::read_to_string(
        "tests/dbml_fixtures/exporter/postgres_exporter/output/enum.out.sql",
    )
    .unwrap();
    println!("enum tail: {:?}", &e[e.len() - 60..]);
    let nd = std::fs::read_to_string(
        "tests/dbml_fixtures/exporter/postgres_exporter/input/null_defaults.in.dbml",
    )
    .unwrap();
    println!(
        "null_defaults parse: {:?}",
        dbml_ui::dbml::parser::parse(&nd).errors.len()
    );
}
