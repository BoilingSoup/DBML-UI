fn main() {
    for dia in ["postgres", "mysql"] {
        let root = format!("tests/dbml_fixtures/importer/{dia}_importer/output");
        for entry in std::fs::read_dir(&root).unwrap() {
            let p = entry.unwrap().path();
            let src = std::fs::read_to_string(&p).unwrap();
            let out = dbml_ui::dbml::parser::parse(&src);
            if !out.errors.is_empty() {
                let e = &out.errors[0];
                println!(
                    "{} {}: {} @ {:?}",
                    dia,
                    p.file_name().unwrap().to_str().unwrap(),
                    e.message,
                    &src[e.offset.saturating_sub(25)..e.offset + 25].replace('\n', "\\n")
                );
            }
        }
    }
}
