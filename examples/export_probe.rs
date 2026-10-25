// Probe: export a DBML file to DDL.
// usage: cargo run --example export_probe -- <dialect> <file.dbml>
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: export_probe <dialect> <file.dbml>");
        std::process::exit(2);
    }
    let dialect = dbml_ui::dbml::sql::Dialect::from_name(&args[1])
        .unwrap_or_else(|| panic!("unknown dialect {}", args[1]));
    let src = std::fs::read_to_string(&args[2]).unwrap();
    match dbml_ui::dbml::sql::export(&src, dialect) {
        Ok(out) => println!("{out}"),
        Err(e) => {
            eprintln!("ERR {e}");
            std::process::exit(1);
        }
    }
}
