//! The formatter's model-preservation property: for every cleanly
//! parsing document, `format_source` must not change the model — the
//! re-parsed formatted source renders the SAME Database JSON as the
//! original (parity/PARITY_SCHEMA_SPEC.md contract, our side on both
//! ends). This is the formatter's oracle: dbdiagram has no formatter to
//! diff against, so the property stands in for upstream.
//!
//! Idempotency (format(format(x)) == format(x)) lives in the fuzz tier.

use dbml_ui::dbml::{self, parser, schema_json};

/// Canonical form for model comparison: keys sorted, token positions
/// stripped (they are source-relative and shift under formatting).
fn canonical(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::Array(a) => {
            serde_json::Value::Array(a.iter().map(canonical).collect())
        }
        serde_json::Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if k == "token" {
                    continue;
                }
                out.insert(k.clone(), canonical(val));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

fn clean_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut push_dir = |dir: &str| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "dbml") {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        out.push((p.display().to_string(), s));
                    }
                }
            }
        }
    };
    push_dir("parity/corpus");
    push_dir("tests/fixtures");
    out.push(("SAMPLE_DBML".into(), dbml_ui::SAMPLE_DBML.to_string()));
    // the SQL importer's outputs are clean DBML by construction — round-
    // trip them too (importer + formatter interaction)
    for entry in std::fs::read_dir("tests/dbml_fixtures/importer").unwrap().flatten() {
        let input = entry.path().join("input");
        let Ok(rd) = std::fs::read_dir(&input) else { continue };
        for f in rd.flatten() {
            let p = f.path();
            if p.extension().is_some_and(|x| x == "sql") {
                if let Ok(sql) = std::fs::read_to_string(&p) {
                    let dialect = match p.parent()
                        .and_then(|d| d.parent())
                        .and_then(|d| d.file_name())
                        .and_then(|d| d.to_str())
                    {
                        Some("mysql_importer") => Some(dbml::sql::Dialect::MySQL),
                        Some("mssql_importer") => Some(dbml::sql::Dialect::MSSQL),
                        Some("oracle_importer") => Some(dbml::sql::Dialect::Oracle),
                        Some("postgres_importer") => Some(dbml::sql::Dialect::Postgres),
                        _ => None,
                    };
                    if let Some(dialect) = dialect {
                        if let Ok(dbml_text) = dbml::sqldump::import(&sql, dialect) {
                            // the raw import can carry dump-text quirks;
                            // the formatted form is the canonical one the
                            // golden harness already re-parses
                            let canonical = dbml::fmt::format_source(&dbml_text)
                                .unwrap_or(dbml_text);
                            out.push((p.display().to_string(), canonical));
                        }
                    }
                }
            }
        }
    }
    out
}

#[test]
fn formatting_preserves_the_model() {
    let mut checked = 0usize;
    for (label, src) in clean_sources() {
        let parsed = parser::parse(&src);
        if !parsed.errors.is_empty() {
            continue;
        }
        let Some(formatted) = dbml::fmt::format_source(&src) else {
            panic!("{label}: clean document does not format");
        };
        let reparsed = parser::parse(&formatted);
        assert!(
            reparsed.errors.is_empty(),
            "{label}: formatted output does not re-parse: {:?}\n--- formatted ---\n{formatted}",
            reparsed.errors
        );
        let before = canonical(&schema_json::to_database_json(&parsed.doc, &src));
        let after = canonical(&schema_json::to_database_json(&reparsed.doc, &formatted));
        assert_eq!(
            before, after,
            "{label}: formatting changed the model\n--- formatted ---\n{formatted}"
        );
        checked += 1;
    }
    assert!(checked > 120, "property ran on too few inputs: {checked}");
}
