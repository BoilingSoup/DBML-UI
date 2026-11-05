//! Format a file and print the result (dev probe).
fn main() {
    let path = std::env::args().nth(1).expect("usage: fmt_probe <file>");
    let src = std::fs::read_to_string(&path).expect("read");
    match dbml_ui::dbml::fmt::format_source(&src) {
        Some(f) => print!("{f}"),
        None => {
            eprintln!("REFUSED (format_source returned None)");
            std::process::exit(1);
        }
    }
}
