// Probe: run the SQL->DBML importer on one file and print the result.
// usage: cargo run --example import_probe -- <dialect> <file> [--raw]
//   dialect: postgres | mysql | mssql | oracle
//   --raw    print the importer's raw output instead of the
//            canonicalized (re-formatted) form used by fixture tests
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: import_probe <dialect> <file> [--raw]");
        std::process::exit(2);
    }
    let dialect = dbml_ui::dbml::sql::Dialect::from_name(&args[1])
        .unwrap_or_else(|| panic!("unknown dialect {}", args[1]));
    let src = std::fs::read_to_string(&args[2]).unwrap();
    match dbml_ui::dbml::sqldump::import(&src, dialect) {
        Ok(out) => {
            if args.iter().any(|a| a == "--raw") {
                println!("{out}");
            } else {
                match dbml_ui::dbml::fmt::format_source(&out) {
                    Some(f) => println!("{f}"),
                    None => {
                        eprintln!("!! output does not re-parse; raw follows");
                        println!("{out}");
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("ERR {e}");
            std::process::exit(1);
        }
    }
}
