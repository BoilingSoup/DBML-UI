//! Robustness fuzzing: deterministic, seeded, structured DBML generation
//! plus corpus mutation, run through parse → validate → format with the
//! editor's invariants asserted. No oracle here (that's the differential
//! tier in parity/fuzz.py + parity/run.mjs) — this tier guards the
//! product properties that must hold for ANY input:
//!
//!   - parse/validate/format never panic
//!   - every diagnostic and quick-fix span is in bounds on a char boundary
//!   - diagnostics come out sorted by offset (the panel's merge assumes it)
//!   - formatting is idempotent: format(format(x)) == format(x)
//!
//! Failures print the seed and the offending source; re-run with
//! FUZZ_SEED to reproduce a single case.

use dbml_ui::dbml::{self, parser};
use dbml_ui::dbml::validate::validate;

const ITERS: usize = 1200;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

const NAMES: &[&str] = &[
    "users", "posts", "a", "B", "my_table", "\"quoted name\"", "üñí", "_x", "t1", "orders",
    "dbo.users", "\"Schema B\".items",
];
const COLS: &[&str] = &["id", "name", "x", "\"odd col\"", "user_id", "code", " created_at "];
const TYPES: &[&str] = &[
    "integer",
    "varchar(255)",
    "decimal(10, 2)",
    "\"character varying\"",
    "dbo.status",
    "text",
    "timestamp",
    "bool",
    "int[]",
    "``",
];
const OPS: &[&str] = &[
    ">", "<", "-", "<>", ">?", "<?", "?>", "?<", "-?", "?-", "<>?", "?<>", "?<>?", "?>?", "?<?",
    "?-?",
];
const WORDS: &[&str] = &["pk", "unique", "not null", "null", "increment", "foo", "indexes"];

fn setting(rng: &mut Rng, names: &[String], cols: &[String]) -> String {
    match rng.below(10) {
        0 => "pk".into(),
        1 => "unique".into(),
        2 => "not null".into(),
        3 => "null".into(),
        4 => "increment".into(),
        5 => format!("default: {}", rng.pick(&["'x'", "1", "0.5", "true", "`now()`", "null"])),
        6 => format!("note: {}", rng.pick(&["'n'", "\"n\"", "'''m'''"])),
        7 => format!(
            "ref: {} {}.{}",
            rng.pick(OPS),
            rng.pick(names).trim_matches('"'),
            rng.pick(cols).trim_matches('"')
        ),
        8 => format!("check: {}", rng.pick(&["`id > 0`", "`x <> ''`"])),
        _ => format!("custom_{}: \"v\"", rng.below(3)),
    }
}

fn gen_dbml(rng: &mut Rng) -> String {
    let mut out = String::new();
    let table_n = 1 + rng.below(3);
    let mut tables: Vec<String> = Vec::new();
    for t in 0..table_n {
        let name = NAMES[rng.below(NAMES.len())].to_string();
        tables.push(name.clone());
        out.push_str(&format!("Table {name} {{\n"));
        let col_n = 1 + rng.below(4);
        let mut cols: Vec<String> = Vec::new();
        for c in 0..col_n {
            let col = COLS[rng.below(COLS.len())].to_string();
            cols.push(col.clone());
            out.push_str(&format!("  {col} {} ", TYPES[rng.below(TYPES.len())]));
            if rng.chance(70) {
                let n = 1 + rng.below(2);
                let sts: Vec<String> = (0..n)
                    .map(|_| setting(rng, &tables.clone(), &cols.clone()))
                    .collect();
                out.push_str(&format!("[{}] ", sts.join(", ")));
            } else if rng.chance(20) {
                // their inline-settings field: a bare word after the type
                out.push_str(&format!("{} ", rng.pick(WORDS)));
            }
            out.push('\n');
        }
        if rng.chance(40) {
            out.push_str("  indexes {\n");
            for _ in 0..1 + rng.below(2) {
                let cols = if rng.chance(50) {
                    format!("({})", cols[rng.below(cols.len())].trim())
                } else {
                    cols[rng.below(cols.len())].trim().to_string()
                };
                let mut st = Vec::new();
                if rng.chance(40) {
                    st.push("unique".into());
                }
                if rng.chance(30) {
                    st.push("pk".into());
                }
                if rng.chance(30) {
                    st.push(format!("name: 'ix{}'", rng.below(3)));
                }
                let st = if st.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", st.join(", "))
                };
                out.push_str(&format!("    {cols}{st}\n"));
            }
            out.push_str("  }\n");
        }
        if rng.chance(20) {
            out.push_str("  Note: 'table note'\n");
        }
        if rng.chance(15) {
            out.push_str(&format!(
                "  records ({}) {{\n    {}, {}\n  }}\n",
                cols[0].trim(),
                rng.pick(&["1", "'a'", "null", "2.5", "`x`"]),
                rng.pick(&["1", "'b'", "null", ""])
            ));
        }
        out.push_str("}\n");
    }
    for _ in 0..rng.below(3) {
        if tables.is_empty() {
            break;
        }
        let a = &tables[rng.below(tables.len())];
        let b = &tables[rng.below(tables.len())];
        if rng.chance(80) {
            out.push_str(&format!(
                "Ref: {}.{} {} {}.{}\n",
                a.trim_matches('"'),
                COLS[rng.below(COLS.len())].trim(),
                rng.pick(OPS),
                b.trim_matches('"'),
                COLS[rng.below(COLS.len())].trim()
            ));
        } else {
            out.push_str(&format!(
                "Ref r{}: {}.{} {} {}.{}\n",
                rng.below(3),
                a.trim_matches('"'),
                COLS[rng.below(COLS.len())].trim(),
                rng.pick(OPS),
                b.trim_matches('"'),
                COLS[rng.below(COLS.len())].trim()
            ));
        }
    }
    if rng.chance(40) {
        out.push_str("Enum e {\n  a\n  b [note: 'x']\n}\n");
    }
    if rng.chance(30) && !tables.is_empty() {
        out.push_str(&format!(
            "TableGroup g {{ {}\n}}\n",
            tables
                .iter()
                .map(|t| t.trim_matches('"').to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if rng.chance(20) {
        out.push_str("Note sticky {\n  '''\n  text\n  '''\n}\n");
    }
    if rng.chance(15) {
        // noise: a broken fragment to exercise error recovery
        out.push_str(&rng.pick(&[
            "Table {\n",
            "Ref: a\n",
            "Table t { id }\n",
            "Enum {\n",
            "}}}\n",
            "Table t { id integer indexes { (id) [pk] } }\n",
        ]));
    }
    out
}

/// Splice, delete, and flip random characters in real files.
fn mutate(rng: &mut Rng, src: &str) -> String {
    let mut bytes: Vec<char> = src.chars().collect();
    for _ in 0..1 + rng.below(4) {
        if bytes.is_empty() {
            break;
        }
        match rng.below(3) {
            0 => {
                let i = rng.below(bytes.len());
                bytes[i] = *rng.pick(&['{', '}', '[', ']', '(', ')', '\'', '"', '`', ',', ':', '?']);
            }
            1 => {
                let i = rng.below(bytes.len());
                bytes.remove(i);
            }
            _ => {
                let i = rng.below(bytes.len());
                let j = rng.below(bytes.len());
                let c = bytes[j];
                bytes.insert(i, c);
            }
        }
    }
    bytes.into_iter().collect()
}

fn check_invariants(src: &str, label: &str) {
    let out = parser::parse(src);
    for e in &out.errors {
        assert!(
            e.offset <= src.len() && src.is_char_boundary(e.offset),
            "{label}: parse error offset {} out of bounds for {src:?}",
            e.offset
        );
    }
    let diags = validate(&out.doc, src);
    let mut last = 0usize;
    for d in &diags {
        assert!(
            d.offset <= src.len() && src.is_char_boundary(d.offset),
            "{label}: diagnostic offset {} out of bounds for {src:?}",
            d.offset
        );
        assert!(
            d.offset >= last,
            "{label}: diagnostics not sorted by offset for {src:?}"
        );
        last = d.offset;
        for f in &d.quick_fixes {
            assert!(
                f.span.start <= f.span.end
                    && f.span.end <= src.len()
                    && src.is_char_boundary(f.span.start)
                    && src.is_char_boundary(f.span.end),
                "{label}: quick-fix span {:?} out of bounds for {src:?}",
                f.span
            );
        }
    }
    // formatting never panics, and is idempotent when it succeeds
    if let Some(f1) = dbml::fmt::format_source(src) {
        let f2 = dbml::fmt::format_source(&f1);
        assert_eq!(
            f2.as_deref(),
            Some(f1.as_str()),
            "{label}: format not idempotent\nfirst: {f1:?}"
        );
    }
}

#[test]
fn structured_inputs_hold_the_editor_invariants() {
    let corpus: Vec<String> = std::fs::read_dir("tests/fixtures")
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "dbml"))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .collect()
        })
        .unwrap_or_default();

    let base = std::env::var("FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0x5EED);
    for i in 0..ITERS {
        let mut rng = Rng::new(base + i as u64);
        let src = gen_dbml(&mut rng);
        check_invariants(&src, &format!("seed {}", base + i as u64));
    }
    // mutations of real files
    for i in 0..ITERS / 2 {
        let mut rng = Rng::new(base.wrapping_mul(31) + i as u64);
        let Some(real) = corpus.get(rng.below(corpus.len().max(1))) else {
            break;
        };
        let src = mutate(&mut rng, real);
        check_invariants(&src, &format!("mutated seed {}", base.wrapping_mul(31) + i as u64));
    }
}

#[test]
fn mutated_sql_never_panics_the_importer() {
    let mut sqls: Vec<String> = Vec::new();
    for entry in std::fs::read_dir("tests/dbml_fixtures/importer").unwrap().flatten() {
        let dialect_dir = entry.path().join("input");
        if let Ok(rd) = std::fs::read_dir(&dialect_dir) {
            for f in rd.filter_map(|f| f.ok()) {
                if f.path().extension().is_some_and(|x| x == "sql") {
                    if let Ok(s) = std::fs::read_to_string(f.path()) {
                        sqls.push(s);
                    }
                }
            }
        }
    }
    assert!(!sqls.is_empty(), "no sql fixtures found");
    let base = std::env::var("FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0x5EED);
    for i in 0..600 {
        let mut rng = Rng::new(base.wrapping_add(0x5A17) + i as u64);
        let pick = rng.below(sqls.len());
        let mutated = mutate(&mut rng, &sqls[pick]);
        // any dialect: the importer must not panic on broken input
        let _ = dbml_ui::dbml::sqldump::import(
            &mutated,
            dbml_ui::dbml::sql::Dialect::Postgres,
        );
    }
}
