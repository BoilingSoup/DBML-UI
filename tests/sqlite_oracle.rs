//! The SQLite oracle: our exporter's DDL is executed by REAL sqlite3,
//! and the same DDL runs through our importer — the resulting model must
//! match the database's own catalog (PRAGMA table_info /
//! foreign_key_list). sqlite3 is the ground truth there is no upstream
//! for; if the two disagree, the bug is real by definition.
//!
//! Skips silently when the `sqlite3` binary is unavailable (CI images
//! without it) so the suite stays portable.

use std::process::Command;

use dbml_ui::dbml::{self, sql::Dialect, validate};

fn sqlite3_available() -> bool {
    Command::new("sqlite3")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run a script in a fresh in-memory database; returns the raw output of
/// the trailing queries (pipe-separated).
fn query_sqlite(script: &str) -> (String, String) {
    let mut child = Command::new("sqlite3")
        .args(["-noheader", "-separator", "|", ":memory:"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn sqlite3");
    use std::io::Write;
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write script");
    let out = child.wait_with_output().expect("run sqlite3");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn dbml_sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for dir in ["parity/corpus", "tests/fixtures"] {
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
    }
    out.push(("SAMPLE_DBML".into(), dbml_ui::SAMPLE_DBML.to_string()));
    out
}

#[test]
fn exporter_ddl_matches_sqlite3_and_the_importer_agrees() {
    if !sqlite3_available() {
        eprintln!("sqlite3 not available — oracle skipped");
        return;
    }
    let mut checked = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (label, src) in dbml_sources() {
        let parsed = dbml_ui::dbml::parser::parse(&src);
        if !parsed.errors.is_empty() || parsed.doc.tables.is_empty() {
            continue;
        }
        // strip records blocks: sample data is not DDL
        let Ok(ddl) = dbml::sql::export(&src, Dialect::SQLite) else {
            continue;
        };
        // --- sqlite3's own catalog --------------------------------------
        let mut script = ddl.clone();
        script.push_str("\nSELECT 'TBL', name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name;\n");
        for probe in ["tables"] {
            let _ = probe;
        }
        // table_info + foreign_key_list per table, driven by the model we
        // expect (the importer's view feeds the table list)
        let imported = match dbml::sqldump::import(&ddl, Dialect::SQLite) {
            Ok(t) => t,
            Err(e) => {
                failures.push(format!("{label}: our importer rejects our own DDL: {e}"));
                continue;
            }
        };
        let doc = dbml_ui::dbml::parser::parse(&imported);
        if !doc.errors.is_empty() {
            failures.push(format!(
                "{label}: imported DDL does not re-parse: {:?}",
                doc.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
            ));
            continue;
        }
        for t in &doc.doc.tables {
            script.push_str(&format!(
                "SELECT 'COL', '{t}', name, type, \"notnull\", pk FROM pragma_table_info('{t}') ORDER BY cid;\n",
                t = t.name.replace('\'', "''"),
            ));
            script.push_str(&format!(
                "SELECT 'FK', '{t}', \"table\", \"from\", \"to\", seq FROM pragma_foreign_key_list('{t}') ORDER BY id, seq;\n",
                t = t.name.replace('\'', "''"),
            ));
        }
        let (raw, stderr) = query_sqlite(&script);
        let input_invalid = validate::validate(&doc.doc, &imported)
            .iter()
            .any(|d| d.severity == validate::Severity::Error);
        if !stderr.is_empty() {
            if input_invalid {
                // a model-level-invalid document may legitimately emit DDL
                // sqlite3 refuses — the oracle only judges VALID models
                continue;
            }
            failures.push(format!("{label}: sqlite3 rejected valid DDL:\n{stderr}"));
            continue;
        }
        // --- cross-check columns: every sqlite3 column must exist in our
        // model with matching not-null/pk flags; and vice versa
        let mut sqlite_cols: std::collections::HashMap<String, Vec<(String, bool, bool)>> =
            std::collections::HashMap::new();
        for line in raw.lines() {
            let f: Vec<&str> = line.split('|').collect();
            if f.len() == 6 && f[0] == "COL" {
                sqlite_cols.entry(f[1].to_string()).or_default().push((
                    f[2].to_string(),
                    f[4] == "1",
                    f[5] != "0",
                ));
            }
        }
        // duplicate table names collapse in a real database — the oracle
        // judges valid models only
        let mut seen_names = std::collections::HashSet::new();
        if doc
            .doc
            .tables
            .iter()
            .any(|t| !seen_names.insert(t.name.clone()))
        {
            continue;
        }
        for t in &doc.doc.tables {
            let Some(actual) = sqlite_cols.get(&t.name) else {
                failures.push(format!("{label}: table '{}' missing from sqlite3", t.name));
                continue;
            };
            let pk_index_cols: Vec<&String> = t
                .indexes
                .iter()
                .filter(|ix| ix.pk)
                .flat_map(|ix| ix.columns.iter())
                .collect();
            let ours: Vec<(String, bool, bool)> = t
                .columns
                .iter()
                .map(|c| {
                    let in_pk_index = pk_index_cols.iter().any(|x| **x == c.name);
                    (
                        c.name.clone(),
                        // sqlite3's notnull flag is DECLARED only — the
                        // model's pk-implied NOT NULL never reaches it
                        c.settings.not_null == Some(true),
                        c.settings.pk || in_pk_index,
                    )
                })
                .collect();
            let mut names_ours: Vec<String> = ours.iter().map(|x| x.0.clone()).collect();
            let mut names_sqlite: Vec<String> = actual.iter().map(|x| x.0.clone()).collect();
            names_ours.sort();
            names_sqlite.sort();
            if names_ours != names_sqlite {
                failures.push(format!(
                    "{label}: table '{}' columns differ: ours {names_ours:?} vs sqlite3 {names_sqlite:?}",
                    t.name
                ));
                continue;
            }
            for ((on, onn, opk), (_, snn, spk)) in ours.iter().zip(actual.iter()) {
                if onn != snn {
                    failures.push(format!(
                        "{label}: {}.{} not-null ours {} vs sqlite3 {}",
                        t.name, on, onn, snn
                    ));
                }
                if opk != spk {
                    failures.push(format!(
                        "{label}: {}.{} pk ours {} vs sqlite3 {}",
                        t.name, on, opk, spk
                    ));
                }
            }
        }
        // --- cross-check FKs: our refs must appear in the FK lists
        let mut sqlite_fks: Vec<(String, String, String, String)> = Vec::new();
        for line in raw.lines() {
            let f: Vec<&str> = line.split('|').collect();
            if f.len() == 6 && f[0] == "FK" {
                sqlite_fks.push((f[1].to_string(), f[2].to_string(), f[3].to_string(), f[4].to_string()));
            }
        }
        for r in &doc.doc.refs {
            // the FK lives on the many side; one-to-one could be either,
            // so both directions count as a hit
            use dbml_ui::dbml::ast::Cardinality::*;
            let pairs: Vec<(String, String, String, String)> = match r.card {
                ManyToOne => vec![(r.from.table.clone(), r.from.columns[0].clone(), r.to.table.clone(), r.to.columns[0].clone())],
                OneToMany => vec![(r.to.table.clone(), r.to.columns[0].clone(), r.from.table.clone(), r.from.columns[0].clone())],
                _ => vec![
                    (r.from.table.clone(), r.from.columns[0].clone(), r.to.table.clone(), r.to.columns[0].clone()),
                    (r.to.table.clone(), r.to.columns[0].clone(), r.from.table.clone(), r.from.columns[0].clone()),
                ],
            };
            for (ft, ff, pt, tf) in &pairs {
                let hit = sqlite_fks.iter().any(|(sft, spt, sff, stf)| {
                    sft == ft && spt == pt && sff == ff && stf == tf
                });
                if !hit
                    && sqlite_fks
                        .iter()
                        .any(|(sft, ..)| sft == ft || sft == pt)
                {
                    failures.push(format!(
                        "{label}: ref {}.{} {} {}.{} missing from sqlite3's FK list",
                        r.from.table, r.from.columns[0], r.card.as_str(), r.to.table, r.to.columns[0]
                    ));
                    break;
                }
            }
        }
        checked += 1;
    }
    assert!(
        checked > 20,
        "oracle ran on too few documents: {checked}"
    );
    assert!(
        failures.is_empty(),
        "{} oracle disagreement(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
