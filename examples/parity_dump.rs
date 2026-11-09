//! Parity harness (our side): for each .dbml path on argv, emit the
//! normalized diagnostics JSON (see parity/PARITY_SPEC.md). One process
//! dumps every file: [{path, diagnostics: [...]}].
use serde::Serialize;

#[derive(Serialize)]
struct QuickFix {
    title: String,
    shortTitle: String,
    isPreferred: Option<bool>,
    newText: String,
}

#[derive(Serialize)]
struct Diagnostic {
    r#type: &'static str,
    text: String,
    row: usize,
    column: usize,
    endRow: usize,
    endColumn: usize,
    category: Option<String>,
    explanation: String,
    code: Option<String>,
    quickFixes: Vec<QuickFix>,
}

#[derive(Serialize)]
struct Case {
    path: String,
    diagnostics: Vec<Diagnostic>,
}

/// The 1-based line of a byte offset in the source — their provider
/// serves Monaco, whose rows are 1-based; the contract follows.
fn row_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

/// The byte column within the line for a byte offset.
fn col_of(src: &str, offset: usize) -> usize {
    let off = offset.min(src.len());
    match src[..off].rfind('\n') {
        Some(nl) => off - nl - 1,
        None => off,
    }
}

/// (row, column) for a byte offset.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    (row_of(src, offset), col_of(src, offset))
}

fn dump_path(path: &str) -> Case {
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let out = dbml_ui::dbml::parser::parse(&src);
    // their pipeline (services/diagnostics/provider.ts): provideErrors is
    // interpretFile().getErrors() — a chained Report carrying parse +
    // binder + interpret CompileErrors, ALL of them, always. The infos
    // vanish only because broken constructs never produce them (their
    // interpretation skips what failed to bind) — no suppression gate.
    let ds = dbml_ui::dbml::validate::validate_with_parse(
        &out.doc,
        &src,
        &out.binder_errors,
        &out.binder_chains,
        &out.record_count_errors,
    );
    let mut diagnostics: Vec<Diagnostic> = out
        .errors
        .iter()
        .map(|e| Diagnostic {
            r#type: "error",
            text: e.message.clone(),
            row: row_of(&src, e.offset),
            column: col_of(&src, e.offset),
            endRow: row_of(&src, e.offset),
            endColumn: col_of(&src, e.offset),
            category: None,
            explanation: String::new(),
            code: None,
            quickFixes: Vec::new(),
        })
        .collect();
    for d in &ds {
        let ty = match d.severity {
            dbml_ui::dbml::validate::Severity::Error => "error",
            dbml_ui::dbml::validate::Severity::Warning => "warning",
            dbml_ui::dbml::validate::Severity::Info => "info",
        };
        let (r, c) = line_col(&src, d.offset);
        diagnostics.push(Diagnostic {
            r#type: ty,
            text: d.message.clone(),
            row: r,
            column: c,
            endRow: r,
            endColumn: c,
            category: d.category.clone(),
            explanation: d.explanation.join("\n"),
            code: None,
            quickFixes: d
                .quick_fixes
                .iter()
                .map(|q| QuickFix {
                    title: q.title.clone(),
                    shortTitle: q.short_title.clone(),
                    // their non-preferred fixes carry no isPreferred at
                    // all — only the operator change sets it
                    isPreferred: if q.is_preferred { Some(true) } else { None },
                    newText: q.new_text.clone(),
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
