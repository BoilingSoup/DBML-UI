//! Schema-parity harness (our side): for each .dbml path on argv, emit
//! the normalized Database JSON (see parity/PARITY_SCHEMA_SPEC.md) —
//! the same shape their Compiler.interpretFile produces with token
//! positions stripped. A document with parse errors or any error-
//! severity diagnostic yields null (their interpretFile refuses too).
use serde_json::Value;

fn dump_path(path: &str) -> Value {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let out = dbml_ui::dbml::parser::parse(&src);
    let blocked = !out.errors.is_empty()
        || dbml_ui::dbml::validate::validate_with_parse(
            &out.doc,
            &src,
            &out.binder_errors,
            &out.binder_chains,
            &out.record_count_errors,
            &out.errors,
        )
        .iter()
        .any(|d| d.severity == dbml_ui::dbml::validate::Severity::Error);
    let database = if blocked {
        Value::Null
    } else {
        dbml_ui::dbml::schema_json::to_database_json(&out.doc, &src)
    };
    serde_json::json!({
        "path": path,
        "database": database,
        "errors": out.errors.iter().map(|e| &e.message).collect::<Vec<_>>(),
    })
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let cases: Vec<Value> = paths.iter().map(|p| dump_path(p)).collect();
    println!("{}", serde_json::to_string_pretty(&cases).expect("serialize"));
}
