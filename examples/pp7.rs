fn main() {
    let src = std::fs::read_to_string(
        "tests/dbml_fixtures/importer/postgres_importer/input/checks.in.sql",
    )
    .unwrap();
    let out = dbml_ui::dbml::sqldump::import(&src, dbml_ui::dbml::sql::Dialect::Postgres).unwrap();
    println!("{}", &out[..700]);
}
