//! Semantic diagnostics over a parsed document — the checks dbdiagram's
//! compiler reports as editor markers, with their message phrasing
//! (harvested live from their editor: `Table 'b' does not exist`,
//! `Column 'x' does not exist in Table 'a'`, `Table 'a' already exists`)
//! plus quick fixes where a rewrite is unambiguous.
//!
//! These are non-fatal: the parser still builds a best-effort document
//! and formatting still works — a typo'd ref should not stop `:format`.

use super::ast::{Document, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Red squiggle — something is wrong (dbdiagram's parseError class).
    Error,
    /// Soft-blue squiggle — valid, but flagged (dbdiagram's warning/info
    /// class, their dark theme's `editorWarning.foreground` #64A0DC).
    Warning,
}

/// One quick fix: replace `span` in the source with `new_text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickFix {
    pub title: String,
    pub span: Span,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub offset: usize,
    pub message: String,
    pub severity: Severity,
    /// Group header in the diagnostics panel; None groups under "Other"
    /// (dbdiagram's uncategorized bucket).
    pub category: Option<String>,
    /// Accordion bullets shown when the row is expanded.
    pub explanation: Vec<String>,
    pub quick_fixes: Vec<QuickFix>,
}

impl Diagnostic {
    pub fn plain(offset: usize, message: String) -> Self {
        Self {
            offset,
            message,
            severity: Severity::Error,
            category: None,
            explanation: Vec::new(),
            quick_fixes: Vec::new(),
        }
    }
}

/// Validate a parsed document: duplicate definition names, refs and
/// TableGroup members pointing at tables or columns that do not exist,
/// index columns missing from their table, the nullable-FK constraint
/// mismatch, and bare-comma index settings. `src` feeds the explanation
/// bullets their raw source-line quotes (the ref statement and the column
/// definition, verbatim).
pub fn validate(doc: &Document, src: &str) -> Vec<Diagnostic> {
    // the trimmed source line containing `offset` — their explanation
    // quotes the ref statement and the column definition as written
    let line_text = |offset: usize| -> String {
        let (row, _) = super::parser::line_col_at(src, offset);
        src.split('\n')
            .nth(row)
            .map(|l| l.trim().to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();

    // duplicates (the later declaration is flagged, like dbdiagram)
    let mut tables: Vec<&str> = Vec::new();
    for t in &doc.tables {
        if tables.contains(&t.name.as_str()) {
            out.push(Diagnostic::plain(
                t.name_span.start,
                format!("Table '{}' already exists", t.name),
            ));
        }
        tables.push(t.name.as_str());
    }
    let mut enums: Vec<&str> = Vec::new();
    for e in &doc.enums {
        if enums.contains(&e.name.as_str()) {
            out.push(Diagnostic::plain(
                e.name_span.start,
                format!("Enum '{}' already exists", e.name),
            ));
        }
        enums.push(e.name.as_str());
    }

    // every ref endpoint must resolve to a real table and column (block
    // refs and inline `ref:` settings alike)
    fn check_endpoint(doc: &Document, ep: &super::ast::Endpoint, anchor: usize, out: &mut Vec<Diagnostic>) {
        // recovered endpoints can carry an empty span — anchor those on
        // the caller-provided position instead of offset 0
        let at = if ep.span.is_empty() { anchor } else { ep.span.start };
        match doc.tables.iter().find(|t| t.matches(&ep.table)) {
            None => out.push(Diagnostic::plain(at, format!("Table '{}' does not exist", ep.table))),
            Some(t) => {
                for col in &ep.columns {
                    if !t.columns.iter().any(|c| &c.name == col) {
                        out.push(Diagnostic::plain(
                            at,
                            format!("Column '{}' does not exist in Table '{}'", col, ep.table),
                        ));
                    }
                }
            }
        }
    }
    for r in &doc.refs {
        check_endpoint(doc, &r.from, r.span.start, &mut out);
        check_endpoint(doc, &r.to, r.span.start, &mut out);
    }
    for t in &doc.tables {
        for c in &t.columns {
            if let Some(ir) = &c.settings.inline_ref {
                check_endpoint(doc, &ir.to, c.name_span.start, &mut out);
            }
        }
    }

    // TableGroup members must name a table (or its alias)
    for g in &doc.table_groups {
        for m in &g.tables {
            if doc.tables.iter().any(|t| t.matches(m)) {
                continue;
            }
            out.push(Diagnostic::plain(
                g.span.start,
                format!("Table '{}' does not exist", m),
            ));
        }
    }

    // index columns must exist when they are plain identifiers
    // (functional expressions in backticks are SQL, not checkable);
    // bare-comma settings get dbdiagram's grammar warning + a rewrite fix
    for t in &doc.tables {
        for ix in &t.indexes {
            for col in &ix.columns {
                let plain = !col.contains(|c: char| !(c.is_alphanumeric() || c == '_'));
                if plain && !t.columns.iter().any(|c| &c.name == col) {
                    out.push(Diagnostic::plain(
                        ix.span.start,
                        format!("Column '{}' does not exist in Table '{}'", col, t.name),
                    ));
                }
            }
            if ix.comma_form {
                let mut st = Vec::new();
                if ix.unique {
                    st.push("unique".to_string());
                }
                if ix.pk {
                    st.push("pk".to_string());
                }
                if let Some(n) = &ix.name {
                    st.push(format!("name: '{n}'"));
                }
                if let Some(ty) = &ix.index_type {
                    st.push(format!("type: {ty}"));
                }
                let list = ix.columns.join(", ");
                let new_text = if st.is_empty() {
                    format!("({list})")
                } else {
                    format!("({list}) [{}]", st.join(", "))
                };
                out.push(Diagnostic {
                    offset: ix.span.start,
                    message: "An index field must be an identifier, a quoted identifier, a functional expression or a tuple of such".to_string(),
                    severity: Severity::Warning,
                    category: Some("Index settings".to_string()),
                    explanation: vec![
                        format!("Settings belong inside the brackets: `({list}) [unique]`, not after a comma."),
                        "The line still renders here; dbdiagram's grammar rejects it.".to_string(),
                    ],
                    quick_fixes: vec![QuickFix {
                        title: "Use bracketed settings".to_string(),
                        span: ix.span.clone(),
                        new_text,
                    }],
                });
            }
        }
    }

    // nullable-FK constraint mismatch (their category "Mismatched
    // optional ref constraints"): the `>` / `<` operator requires the FK
    // column to be NOT NULL — `-` and `<>` allow nulls. The FK side is
    // the LEFT operand of `>` and the RIGHT operand of `<`.
    // pk implies NOT NULL (SQL semantics — dbdiagram does not flag
    // `[pk]`-only columns)
    //
    // their panel emits TWO anchors per mismatch: the nullable column's
    // definition (once per column) and the FK endpoint of every ref
    // that trips it (live-verified 18:3 + 95:6 on the showcase)
    let column_of = |table: &str, col: &str| -> Option<&super::ast::Column> {
        doc.tables
            .iter()
            .find(|t| t.matches(table))
            .and_then(|t| t.columns.iter().find(|c| c.name == col))
    };
    let is_nullable = |table: &str, col: &str| -> bool {
        column_of(table, col).is_some_and(|c| {
            c.settings.not_null != Some(true) && !c.settings.pk
        })
    };
    let mut col_anchors: Vec<(String, String)> = Vec::new();
    let mut nullable_check = |is_nullable: bool,
                              tbl: &str,
                              col: &str,
                              op: char,
                              op_offset: usize,
                              ref_anchor: Option<usize>,
                              ref_line: &str,
                              out: &mut Vec<Diagnostic>| {
        if !is_nullable {
            return;
        }
        // their mismatched-constraints diagnostic shares ONE body across
        // every anchor it renders at (live-verified: the column-definition
        // entry carries the same explanation bullets and the same quick
        // fixes — whose diff previews quote the ref line) — so both
        // anchors here get the identical explanation and fix list
        let col_line = column_of(tbl, col)
            .map(|c| {
                // their column quote is the node's own text, not the line:
                // a single-line table would drag the whole `Table b { … }`
                // opener in
                src.get(c.span.start..c.span.end)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| line_text(c.name_span.start))
            })
            .unwrap_or_default();
        let explanation = vec![
            format!(
                "In ref definition `{}`, the operator `{op}` requires `{tbl}.{col}` to reference an existing row.",
                ref_line
            ),
            format!(
                "In column definition `{}`, the column is nullable, which allows rows with no parent.",
                col_line
            ),
        ];
        let quick_fixes = vec![
            QuickFix {
                title: "Change to -".to_string(),
                span: op_offset..op_offset + 1,
                new_text: "-".to_string(),
            },
            QuickFix {
                title: "Change to <>".to_string(),
                span: op_offset..op_offset + 1,
                new_text: "<>".to_string(),
            },
        ];
        // the column-definition anchor, once per column
        if !col_anchors.iter().any(|(t, c)| t == tbl && c == col) {
            if let Some(c) = column_of(tbl, col) {
                out.push(Diagnostic {
                    offset: c.name_span.start,
                    message: format!(
                        "Column `{tbl}.{col}` is nullable but operator `{op}` requires it to be NOT NULL"
                    ),
                    severity: Severity::Warning,
                    category: Some("Mismatched optional ref constraints".to_string()),
                    explanation: explanation.clone(),
                    quick_fixes: quick_fixes.clone(),
                });
                col_anchors.push((tbl.to_string(), col.to_string()));
            }
        }
        // the ref-side anchor (their [95:6]: the FK endpoint's own span)
        out.push(Diagnostic {
            offset: ref_anchor.unwrap_or(op_offset),
            message: format!(
                "Column `{tbl}.{col}` is nullable but operator `{op}` requires it to be NOT NULL"
            ),
            severity: Severity::Warning,
            category: Some("Mismatched optional ref constraints".to_string()),
            explanation,
            quick_fixes,
        });
    };
    for r in &doc.refs {
        // per column pair; single-column refs are the common case
        for (fc, tc) in r.from.columns.iter().zip(r.to.columns.iter()) {
            // the raw ref statement, quoted verbatim in the explanation
            let ref_line = line_text(r.span.start);
            match r.card {
                super::ast::Cardinality::ManyToOne => nullable_check(
                    is_nullable(&r.from.table, fc),
                    &r.from.table,
                    fc,
                    '>',
                    r.op_offset,
                    Some(r.from.span.start),
                    &ref_line,
                    &mut out,
                ),
                super::ast::Cardinality::OneToMany => nullable_check(
                    is_nullable(&r.to.table, tc),
                    &r.to.table,
                    tc,
                    '<',
                    r.op_offset,
                    Some(r.to.span.start),
                    &ref_line,
                    &mut out,
                ),
                _ => {}
            }
        }
    }
    for t in &doc.tables {
        for c in &t.columns {
            let Some(ir) = &c.settings.inline_ref else {
                continue;
            };
            // the inline ref lives on the column's own line
            let ref_line = line_text(c.name_span.start);
            match ir.card {
                // the host column is the FK side
                super::ast::Cardinality::ManyToOne => nullable_check(
                    c.settings.not_null != Some(true),
                    &t.name,
                    &c.name,
                    '>',
                    ir.op_offset,
                    Some(c.name_span.start),
                    &ref_line,
                    &mut out,
                ),
                // the host is the referenced side; the FK is the other
                // end's column
                super::ast::Cardinality::OneToMany => nullable_check(
                    is_nullable(&ir.to.table, ir.to.columns.first().map(|s| s.as_str()).unwrap_or("")),
                    &ir.to.table,
                    ir.to.columns.first().map(|s| s.as_str()).unwrap_or(""),
                    '<',
                    ir.op_offset,
                    Some(c.name_span.start),
                    &ref_line,
                    &mut out,
                ),
                _ => {}
            }
        }
    }

    // record constraint violations (their category "Record constraint
    // violations", live-verified 9 entries on the showcase): sample-rows
    // integrity across refs — each side's recorded values must exist on
    // the other side of the relationship when that side has records.
    // Their messages: the parent-side miss has no prefix, the FK-side
    // miss reads "FK violation: ...".
    let value_column = |rec: &super::ast::TableRecords,
                        table: &str,
                        col: &str,
                        doc: &Document|
     -> Option<usize> {
        if rec.columns.is_empty() {
            // implicit header: the table's columns in definition order
            doc.tables
                .iter()
                .find(|t| t.matches(table))
                .and_then(|t| t.columns.iter().position(|c| c.name == col))
        } else {
            rec.columns.iter().position(|c| c == col)
        }
    };
    let recorded_values = |rec: &super::ast::TableRecords,
                           idx: usize|
     -> Vec<(String, Span)> {
        rec.rows
            .iter()
            .zip(rec.value_spans.iter())
            .filter_map(|(row, spans)| {
                row.get(idx)
                    .zip(spans.get(idx))
                    .map(|(v, s)| (v.label(), s.clone()))
            })
            .collect()
    };
    let record_checks = |fk_tbl: &str,
                         fk_col: &str,
                         parent_tbl: &str,
                         parent_col: &str,
                         doc: &Document,
                         out: &mut Vec<Diagnostic>| {
        // each check runs on its own side's records: a side WITHOUT
        // records contributes an empty value set, so every value on the
        // other side misses (their 9 on the showcase fired against
        // tables with no records at all)
        let fk_rec = doc
            .tables
            .iter()
            .find(|t| t.matches(fk_tbl))
            .and_then(|t| t.records.as_ref());
        let parent_rec = doc
            .tables
            .iter()
            .find(|t| t.matches(parent_tbl))
            .and_then(|t| t.records.as_ref());
        let fk_vals = match fk_rec {
            Some(rec) => value_column(rec, fk_tbl, fk_col, doc)
                .map(|idx| recorded_values(rec, idx))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let parent_vals = match parent_rec {
            Some(rec) => value_column(rec, parent_tbl, parent_col, doc)
                .map(|idx| recorded_values(rec, idx))
                .unwrap_or_default(),
            None => Vec::new(),
        };
        // the parent's recorded values must appear among the child's
        if let Some(parent_rec) = parent_rec {
            if !parent_rec.rows.is_empty() {
                for (v, span) in &parent_vals {
                    if v == "null" {
                        continue;
                    }
                    if !fk_vals.iter().any(|(fv, _)| fv == v) {
                        out.push(Diagnostic {
                            offset: span.start,
                            message: format!(
                                "{parent_tbl}.{parent_col} = {v} does not exist in {fk_tbl}.{fk_col}"
                            ),
                            severity: Severity::Warning,
                            category: Some("Record constraint violations".to_string()),
                            explanation: vec![],
                            quick_fixes: vec![],
                        });
                    }
                }
            }
        }
        // the child's recorded FK values must exist on the parent
        if let Some(fk_rec) = fk_rec {
            if !fk_rec.rows.is_empty() {
                for (v, span) in &fk_vals {
                    if v == "null" {
                        continue;
                    }
                    if !parent_vals.iter().any(|(pv, _)| pv == v) {
                        out.push(Diagnostic {
                            offset: span.start,
                            message: format!(
                                "FK violation: {fk_tbl}.{fk_col} = {v} does not exist in {parent_tbl}.{parent_col}"
                            ),
                            severity: Severity::Warning,
                            category: Some("Record constraint violations".to_string()),
                            explanation: vec![],
                            quick_fixes: vec![],
                        });
                    }
                }
            }
        }
    };
    for r in &doc.refs {
        for (fc, tc) in r.from.columns.iter().zip(r.to.columns.iter()) {
            // ManyToOne: from is the FK (child) side, to the referenced
            // parent; OneToMany is mirrored
            let (fk_tbl, fk_col, parent_tbl, parent_col) = match r.card {
                super::ast::Cardinality::ManyToOne => (&r.from.table, fc, &r.to.table, tc),
                super::ast::Cardinality::OneToMany => (&r.to.table, tc, &r.from.table, fc),
                _ => continue,
            };
            record_checks(fk_tbl, fk_col, parent_tbl, parent_col, doc, &mut out);
        }
    }
    // inline refs take the same record checks (the host column is the FK
    // for `ref: >`, the target for `ref: <`)
    for t in &doc.tables {
        for c in &t.columns {
            let Some(ir) = &c.settings.inline_ref else {
                continue;
            };
            for hc in [&c.name] {
                for tc in &ir.to.columns {
                    let (fk_tbl, fk_col, parent_tbl, parent_col) = match ir.card {
                        super::ast::Cardinality::ManyToOne => {
                            (&t.name, hc, &ir.to.table, tc)
                        }
                        super::ast::Cardinality::OneToMany => {
                            (&ir.to.table, tc, &t.name, hc)
                        }
                        _ => continue,
                    };
                    record_checks(fk_tbl, fk_col, parent_tbl, parent_col, doc, &mut out);
                }
            }
        }
    }

    for t in &doc.tables {
        for c in &t.columns {
            let Some(ir) = &c.settings.inline_ref else {
                continue;
            };
            let definition = format!(
                "`ref: {} {}` on `{}.{}`",
                ir.card.as_str(),
                ir.to.table,
                t.name,
                c.name
            );
            match ir.card {
                // the host column is the FK side
                super::ast::Cardinality::ManyToOne => nullable_check(
                    c.settings.not_null != Some(true),
                    &t.name,
                    &c.name,
                    '>',
                    ir.op_offset,
                    Some(c.name_span.start),
                    &definition,
                    &mut out,
                ),
                // the host is the referenced side; the FK is the other
                // end's column
                super::ast::Cardinality::OneToMany => nullable_check(
                    is_nullable(&ir.to.table, ir.to.columns.first().map(|s| s.as_str()).unwrap_or("")),
                    &ir.to.table,
                    ir.to.columns.first().map(|s| s.as_str()).unwrap_or(""),
                    '<',
                    ir.op_offset,
                    Some(c.name_span.start),
                    &definition,
                    &mut out,
                ),
                _ => {}
            }
        }
    }

    out.sort_by_key(|d| d.offset);
    out
}

#[cfg(test)]
mod tests {
    use super::super::parser;
    use super::*;

    fn diags(src: &str) -> Vec<String> {
        validate(&parser::parse(src).doc, src)
            .into_iter()
            .map(|d| d.message)
            .collect()
    }

    #[test]
    fn clean_documents_validate_empty() {
        assert!(diags(include_str!("../../tests/fixtures/basic.dbml")).is_empty());
        assert!(diags(crate::SAMPLE_DBML).is_empty());
        // showcase.dbml's findings are the nullable self-ref FK (their
        // editor anchors it twice: the column definition + the ref's FK
        // endpoint — 2 entries) plus 9 record-constraint violations
        // (their panel, live-verified: 3 posts records x {author_id not
        // in users' (missing) records, id not in comments'/likes'}
        // (missing) records)
        let d = diags(include_str!("../../showcase.dbml"));
        let nullable = d
            .iter()
            .filter(|x| x.contains("users.referrer_id` is nullable but operator `>`"))
            .count();
        let record_violations = d
            .iter()
            .filter(|x| {
                x.contains("does not exist in")
                    || x.contains("FK violation:")
            })
            .count();
        assert!(
            nullable == 2 && record_violations == 9 && d.len() == 11,
            "{d:?}"
        );
    }

    #[test]
    fn duplicate_names_are_flagged_at_the_later_declaration() {
        let d = diags("Table a { id integer }\nTable a { x integer }\nEnum e { x }\nEnum e { y }\n");
        assert!(d.iter().any(|m| m == "Table 'a' already exists"), "{d:?}");
        assert!(d.iter().any(|m| m == "Enum 'e' already exists"), "{d:?}");
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn ref_endpoints_resolve_or_flag() {
        // dbdiagram's exact phrasings (harvested from their editor)
        let d = diags("Table a { id integer [pk] }\nRef: b.x > a.nope\n");
        assert!(d.contains(&"Table 'b' does not exist".to_string()), "{d:?}");
        assert!(
            d.contains(&"Column 'nope' does not exist in Table 'a'".to_string()),
            "{d:?}"
        );
    }

    #[test]
    fn inline_refs_validate_too() {
        // not null: the nullable-FK lint must not join in (it has its own
        // test below)
        let d = diags("Table a { id integer }\nTable b { a_id integer [not null, ref: > a.nope] }\n");
        assert_eq!(
            d,
            vec!["Column 'nope' does not exist in Table 'a'".to_string()]
        );
    }

    #[test]
    fn group_members_and_index_columns_resolve() {
        let d = diags(
            "Table a { id integer }\nTableGroup g {\n  nope\n}\nTable c {\n  x integer\n  indexes {\n    (zzz) [unique]\n  }\n}\n",
        );
        assert!(d.contains(&"Table 'nope' does not exist".to_string()), "{d:?}");
        assert!(
            d.contains(&"Column 'zzz' does not exist in Table 'c'".to_string()),
            "{d:?}"
        );
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn aliases_satisfy_refs() {
        let d = diags("Table users as U { id integer }\nRef: posts.uid > U.id\n");
        // 'posts' itself is the unknown one; U resolves via the alias
        assert_eq!(d, vec!["Table 'posts' does not exist".to_string()]);
    }

    #[test]
    fn functional_index_expressions_are_not_checked() {
        let d = diags(
            "Table a {\n  id integer\n  indexes {\n    (`LOWER(id)`) [name: 'lo']\n  }\n}\n",
        );
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn nullable_fk_gets_category_bullets_and_two_fixes() {
        // the FK is the LEFT operand of `>` and genuinely nullable
        // (`[pk]` alone implies NOT NULL, so `a.id` would not qualify).
        // Their panel anchors the mismatch TWICE: the column definition
        // and the ref's FK endpoint (live-verified on their editor)
        let src = "Table a { id integer [pk] }\nTable b { a_id integer [null] }\nRef: b.a_id > a.id\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        assert_eq!(d.len(), 2, "{d:?}");
        // BOTH anchors share one body: same message, the two explanation
        // bullets (their raw-line quotes), and both operator fixes (the
        // column-definition entry's fix diff still quotes the ref line —
        // live-verified on their editor)
        for (i, d) in d.iter().enumerate() {
            assert_eq!(d.severity, Severity::Warning);
            assert_eq!(
                d.message,
                "Column `b.a_id` is nullable but operator `>` requires it to be NOT NULL"
            );
            assert_eq!(
                d.category.as_deref(),
                Some("Mismatched optional ref constraints")
            );
            assert_eq!(
                d.explanation,
                vec![
                    "In ref definition `Ref: b.a_id > a.id`, the operator `>` requires `b.a_id` to reference an existing row.".to_string(),
                    "In column definition `a_id integer [null]`, the column is nullable, which allows rows with no parent.".to_string(),
                ],
                "anchor {i}"
            );
            assert_eq!(d.quick_fixes.len(), 2, "anchor {i}");
            assert_eq!(d.quick_fixes[0].title, "Change to -");
            assert_eq!(d.quick_fixes[0].new_text, "-");
            assert_eq!(d.quick_fixes[1].title, "Change to <>");
        }
        assert_eq!(d[0].offset, out.doc.tables[1].columns[0].name_span.start);
        assert_eq!(d[1].offset, out.doc.refs[0].from.span.start);
        // not-null FKs stay silent
        let src = "Table a { id integer [pk] }\nTable b { a_id integer [not null] }\nRef: a.id > b.a_id\n";
        let out = parser::parse(src);
        assert!(validate(&out.doc, src).is_empty());
    }

    #[test]
    fn record_values_are_checked_across_refs() {
        // their "Record constraint violations": posts' records reference
        // users.id (which has no records — every author_id misses) and
        // are referenced by comments/likes (whose records are empty —
        // every posts.id misses). 3 rows x 3 checks = 9, each anchored
        // at the violated value's own span
        let src = "Table users { id integer [pk] }\nTable posts { id integer [pk]\n  author_id integer\n  records (id, author_id) {\n    1, 1\n    2, 7\n  }\n}\nRef: posts.author_id > users.id\nRef: comments.post_id > posts.id\nTable comments { id integer [pk]\n  post_id integer\n  records (post_id) {\n    9\n  }\n}\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        let violations: Vec<_> = d
            .iter()
            .filter(|x| x.category.as_deref() == Some("Record constraint violations"))
            .collect();
        // 2 FK misses (author_id 1/7 against users' empty records) + 2
        // parent misses (posts.id 1/2 against comments' single 9) + 1 FK
        // miss the other way (comments.post_id 9 against posts' {1,2})
        assert_eq!(violations.len(), 5, "{d:?}");
        // the FK side: author_id 1 and 7 miss users.id's (empty) records
        assert!(
            violations
                .iter()
                .any(|x| x.message == "FK violation: posts.author_id = 1 does not exist in users.id"),
            "{d:?}"
        );
        assert!(
            violations
                .iter()
                .any(|x| x.message == "FK violation: posts.author_id = 7 does not exist in users.id"),
            "{d:?}"
        );
        // the parent side: posts.id 1 and 2 miss comments.post_id's
        // records (which hold only 9)
        assert!(
            violations
                .iter()
                .any(|x| x.message == "posts.id = 1 does not exist in comments.post_id"),
            "{d:?}"
        );
        assert!(
            violations
                .iter()
                .any(|x| x.message == "posts.id = 2 does not exist in comments.post_id"),
            "{d:?}"
        );
        // a matching value passes: comments.post_id = 9 exists in ...
        // (no ref points the other way here — covered by the empty case
        // above)
        // the anchors land on the record VALUES, not the row starts
        let anchored = violations.iter().all(|x| {
            out.doc.tables.iter().any(|t| {
                t.records
                    .as_ref()
                    .is_some_and(|r| {
                        r.value_spans
                            .iter()
                            .flatten()
                            .any(|s| s.start == x.offset)
                    })
            })
        });
        assert!(anchored, "every violation anchors on a record value");
    }

    #[test]
    fn comma_index_settings_get_a_bracketed_rewrite_fix() {
        let src =
            "Table t {\n  id integer\n  indexes {\n    (id), unique, name: 'x'\n  }\n}\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].category.as_deref(), Some("Index settings"));
        assert_eq!(d[0].quick_fixes.len(), 1);
        let fix = &d[0].quick_fixes[0];
        assert_eq!(fix.title, "Use bracketed settings");
        assert_eq!(fix.new_text, "(id) [unique, name: 'x']");
        assert_eq!(fix.span, out.doc.tables[0].indexes[0].span);
    }
}
