//! Parity harness (our side): for each .dbml path on argv, emit the
//! normalized diagnostics JSON (see parity/PARITY_SPEC.md). One process
//! dumps every file: [{path, diagnostics: [...]}].
use serde::Serialize;

#[derive(Serialize)]
struct QuickFix {
    short_title: String,
    new_text: String,
}

#[derive(Serialize)]
struct Diagnostic {
    r#type: &'static str,
    text: String,
    row: usize,
    quick_fixes: Vec<QuickFix>,
}

#[derive(Serialize)]
struct Case {
    path: String,
    diagnostics: Vec<Diagnostic>,
}

/// The 0-based line of a byte offset in the source.
fn row_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count()
}

fn dump_path(path: &str) -> Case {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let out = dbml_ui::dbml::parser::parse(&src);
    let ds = dbml_ui::dbml::validate::validate(&out.doc, &src);
    let mut diagnostics: Vec<Diagnostic> = out
        .errors
        .iter()
        .map(|e| Diagnostic {
            r#type: "error",
            text: e.message.clone(),
            row: row_of(&src, e.offset),
            quick_fixes: Vec::new(),
        })
        .collect();
    for d in &ds {
        let ty = match d.severity {
            dbml_ui::dbml::validate::Severity::Error => "error",
            dbml_ui::dbml::validate::Severity::Warning => "warning",
        };
        diagnostics.push(Diagnostic {
            r#type: ty,
            text: d.message.clone(),
            row: row_of(&src, d.offset),
            quick_fixes: d
                .quick_fixes
                .iter()
                .map(|q| QuickFix {
                    short_title: q.short_title.clone(),
                    new_text: q.new_text.clone(),
                })
                .collect(),
        });
    }
    Case { path: path.to_string(), diagnostics }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let cases: Vec<Case> = paths.iter().map(|p| dump_path(p)).collect();
    println!("{}", serde_json::to_string_pretty(&cases).expect("serialize"));
}
