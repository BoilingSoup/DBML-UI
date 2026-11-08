//! Show diagnostics for a file (dev probe).
fn main() {
    let path = std::env::args().nth(1).expect("file");
    let src = std::fs::read_to_string(&path).unwrap();
    let out = dbml_ui::dbml::parser::parse(&src);
    println!("{} errors", out.errors.len());
    for d in dbml_ui::dbml::validate::validate(&out.doc, &src) {
        let (row, col) = dbml_ui::dbml::parser::line_col_at(&src, d.offset);
        let sev = match d.severity {
            dbml_ui::dbml::validate::Severity::Error => "error",
            dbml_ui::dbml::validate::Severity::Warning => "warn",
            dbml_ui::dbml::validate::Severity::Info => "info",
        };
        println!(
            "  [{}{}] {}:{} {}",
            sev,
            d.category.as_ref().map(|c| format!(" {c}")).unwrap_or_default(),
            row + 1,
            col + 1,
            d.message
        );
        for f in &d.quick_fixes {
            println!("    fix: {} -> {:?}", f.title, f.new_text);
        }
    }
}
