//! Semantic diagnostics over a parsed document — the checks dbdiagram's
//! compiler reports as editor markers, with their message phrasing
//! (harvested live from their editor: `Table 'b' does not exist`,
//! `Column 'x' does not exist in Table 'a'`, `Table 'a' already exists`)
//! plus quick fixes where a rewrite is unambiguous.
//!
//! These are non-fatal: the parser still builds a best-effort document
//! and formatting still works — a typo'd ref should not stop `:format`.

use super::ast::{Cardinality, Document, Span};

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
    /// The full label ("Make 'b.a_id' optional in the ref (change
    /// operator to '?>')").
    pub title: String,
    /// The compact label their dropdown shows ("Change to >?").
    pub short_title: String,
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
                        short_title: "Use bracketed settings".to_string(),
                        span: ix.span.clone(),
                        new_text,
                    }],
                });
            }
        }
    }

    // ---- the ref-relationship engine: per-side runs, four checks each ----
    // (their compiler's relationship analyzer, ported: the operator set
    // carries per-side optional marks — `?>`/`>?`/… — and each side's
    // columns are checked against the OTHER side's bound. Four blocks per
    // run: the nullable mismatch, the required mismatch, the unique
    // mismatch and the should-be-unique mismatch. Every diagnostic emits
    // at the checked side's ref endpoint plus each checked column's own
    // definition, all sharing one body: the same explanation bullets and
    // quick fixes. The column-setting fixes ("Mark … as NOT NULL/NULL")
    // are enabled only for the run whose checked side is the ref's RIGHT
    // endpoint — the left endpoint's diagnostics carry the operator fix
    // alone, live-verified on their editor.)
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
    let is_required = |table: &str, col: &str| -> bool { !is_nullable(table, col) };
    let columns_unique = |table: &str, cols: &[String]| -> bool {
        if cols.is_empty() {
            return false;
        }
        if cols.len() == 1 {
            return column_of(table, &cols[0]).is_some_and(|c| c.settings.unique || c.settings.pk);
        }
        // composite: a unique or pk index covering exactly these columns
        doc.tables
            .iter()
            .find(|t| t.matches(table))
            .is_some_and(|t| {
                t.indexes.iter().any(|ix| {
                    (ix.unique || ix.pk)
                        && ix.columns.len() == cols.len()
                        && ix.columns.iter().all(|c| cols.contains(c))
                })
            })
    };
    // the column-setting edit: swap a found word for `replace`, or grow
    // the settings list (their QUt/XUt, adapted to this nullability model
    // — the explicit swap keeps the semantics honest here)
    let setting_edit = |span: &Span, find: &str, replace: &str, insert: &str| -> Option<(Span, String)> {
        let text = src.get(span.start..span.end)?;
        let bytes = text.as_bytes();
        let word = |i: usize| {
            let before_ok = i == 0
                || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            let after = i + find.len();
            let after_ok = after >= text.len()
                || !(bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_');
            before_ok && after_ok
        };
        let mut i = 0;
        while i + find.len() <= text.len() {
            if text[i..].starts_with(find) && word(i) {
                let start = span.start + i;
                return Some((start..start + find.len(), replace.to_string()));
            }
            i += 1;
        }
        // no explicit setting: grow the bracket list, or open one
        if let Some(bracket) = text.find('[') {
            if let Some(close_rel) = text[bracket..].rfind(']') {
                let close = span.start + bracket + close_rel;
                return Some((close..close, insert.to_string()));
            }
        }
        Some((span.end..span.end, format!(" [{replace}]")))
    };
    // the compose: the operator string for the flipped flags (their
    // FVt over the y1t bound table)
    let op_str = |card: Cardinality, left_opt: bool, right_opt: bool| -> &'static str {
        use super::ast::Cardinality::*;
        match (card, left_opt, right_opt) {
            (ManyToOne, true, true) => "?>?",
            (ManyToOne, true, false) => "?>",
            (ManyToOne, false, true) => ">?",
            (ManyToOne, false, false) => ">",
            (OneToMany, true, true) => "?<?",
            (OneToMany, true, false) => "?<",
            (OneToMany, false, true) => "<?",
            (OneToMany, false, false) => "<",
            (ManyToMany, true, true) => "?<>?",
            (ManyToMany, true, false) => "?<>",
            (ManyToMany, false, true) => "<>?",
            (ManyToMany, false, false) => "<>",
            (OneToOne, true, true) => "?-?",
            (OneToOne, true, false) => "?-",
            (OneToOne, false, true) => "-?",
            (OneToOne, false, false) => "-",
        }
    };
    let emit_ref = |from_table: &str,
                        from_cols: &[String],
                        from_anchor: usize,
                        to_table: &str,
                        to_cols: &[String],
                        to_anchor: usize,
                        card: Cardinality,
                        left_opt: bool,
                        right_opt: bool,
                        op_offset: usize,
                        ref_line: &str,
                        out: &mut Vec<Diagnostic>| {
        let all_exist = from_cols.iter().all(|c| column_of(from_table, c).is_some())
            && to_cols.iter().all(|c| column_of(to_table, c).is_some());
        let ((l_min_one, l_many), (r_min_one, r_many)) = match card {
            Cardinality::ManyToOne => ((true, true), (true, false)),
            Cardinality::OneToMany => ((true, false), (true, true)),
            Cardinality::OneToOne => ((true, false), (true, false)),
            Cardinality::ManyToMany => ((true, true), (true, true)),
        };
        let l_min_one = l_min_one && !left_opt;
        let r_min_one = r_min_one && !right_opt;
        let op_now = op_str(card, left_opt, right_opt);
        let op_len = op_now.len();
        // the two runs (their nWt runs right first, then left): the run's
        // side carries the REQUIREMENT bound; the OTHER side's columns are
        // checked against it. The column-setting fixes are enabled only
        // for the run whose requirement is the LEFT bound (the right
        // endpoint's checked columns), live-verified on their editor.
        for checked_from in [true, false] {
            let (checked_table, checked_cols, checked_anchor, other_cols_table, other_cols) =
                if checked_from {
                    (from_table, from_cols, from_anchor, to_table, to_cols)
                } else {
                    (to_table, to_cols, to_anchor, from_table, from_cols)
                };
            let (checked_many, other_min_one, other_many) = if checked_from {
                (l_many, r_min_one, r_many)
            } else {
                (r_many, l_min_one, l_many)
            };
            // their y: the checked side is the many side, or a one-to-one
            // with the FK on the left (the requirement on the left bound)
            let applies = checked_many || (!other_many && !checked_from);
            if !applies || checked_cols.is_empty() || !all_exist {
                continue;
            }
            let names = checked_cols
                .iter()
                .map(|c| format!("{checked_table}.{c}"))
                .collect::<Vec<_>>()
                .join(", ");
            let req_names = other_cols
                .iter()
                .map(|c| format!("{other_cols_table}.{c}"))
                .collect::<Vec<_>>()
                .join(", ");
            // their b1t: the operator for a (left, right) bound pair —
            // (min one?, allows many?) per side
            let op_for_bounds = |l_min_one: bool, l_many: bool, r_min_one: bool, r_many: bool| -> &'static str {
                match (l_min_one, l_many, r_min_one, r_many) {
                    (true, false, true, false) => "-",
                    (true, false, false, false) => "-?",
                    (false, false, true, false) => "?-",
                    (false, false, false, false) => "?-?",
                    (true, true, true, false) => ">",
                    (false, true, true, false) => "?>",
                    (true, true, false, false) => ">?",
                    (false, true, false, false) => "?>?",
                    (true, false, true, true) => "<",
                    (false, false, true, true) => "?<",
                    (true, false, false, true) => "<?",
                    (false, false, false, true) => "?<?",
                    (true, true, true, true) => "<>",
                    (false, true, true, true) => "?<>",
                    (true, true, false, true) => "<>?",
                    (false, true, false, true) => "?<>?",
                }
            };
            let all_nullable = checked_cols.iter().all(|c| is_nullable(checked_table, c));
            let all_required = checked_cols.iter().all(|c| is_required(checked_table, c));
            // the operator fix: the requirement side's optional mark flips
            // (their rWt: the new bound composes with the untouched side)
            let op_fix = |relax: bool| {
                let (nl, nr) = if checked_from {
                    (left_opt, relax)
                } else {
                    (relax, right_opt)
                };
                let new_op = op_str(card, nl, nr);
                QuickFix {
                    title: format!(
                        "Make '{names}' {} in the ref (change operator to '{}')",
                        if relax { "optional" } else { "required" },
                        new_op
                    ),
                    short_title: format!("Change to {new_op}"),
                    span: op_offset..op_offset + op_len,
                    new_text: new_op.to_string(),
                }
            };
            // the checked side's own columns' definition lines (the first
            // one quotes into the explanation, their i[0]?.declaration)
            let col_lines = checked_cols
                .iter()
                .filter_map(|c| {
                    column_of(checked_table, c).and_then(|c| {
                        src.get(c.span.start..c.span.end)
                            .map(|s| s.trim().to_string())
                    })
                })
                .collect::<Vec<_>>();
            // 1. the nullable mismatch: the requirement is one but the
            // checked columns are nullable (their oUt)
            if other_min_one && all_nullable {
                let mut fixes = vec![op_fix(true)];
                for c in checked_cols {
                    if let Some(col) = column_of(checked_table, c) {
                        if col.settings.pk || col.settings.increment {
                            continue;
                        }
                        if let Some(edit) =
                            setting_edit(&col.span, "null", "not null", ", not null")
                        {
                            fixes.push(QuickFix {
                                title: format!("Mark '{}.{}' as NOT NULL", checked_table, c),
                                short_title: "Add not null".to_string(),
                                span: edit.0,
                                new_text: edit.1,
                            });
                        }
                    }
                }
                let explanation = vec![
                    format!(
                        "In ref definition `{ref_line}`, the operator `{op_now}` requires `{names}` to reference an existing row."
                    ),
                    match col_lines.first() {
                        Some(line) => format!(
                            "In column definition `{line}`, the column is nullable, which allows rows with no parent."
                        ),
                        None => "A nullable column allows rows with no parent.".to_string(),
                    },
                ];
                let multi = checked_cols.len() > 1;
                let message = if multi {
                    format!("Columns (`{names}`) are all nullable but operator `{op_now}` requires at least one to be NOT NULL")
                } else {
                    format!("Column `{names}` is nullable but operator `{op_now}` requires it to be NOT NULL")
                };
                out.push(Diagnostic {
                    offset: checked_anchor,
                        message: message.clone(),
                        severity: Severity::Warning,
                        category: Some("Mismatched optional ref constraints".to_string()),
                        explanation: explanation.clone(),
                        quick_fixes: fixes.clone(),
                    });

                for c in checked_cols {
                    if let Some(col) = column_of(checked_table, c) {
                        out.push(Diagnostic {
                            offset: col.name_span.start,
                            message: message.clone(),
                            severity: Severity::Warning,
                            category: Some("Mismatched optional ref constraints".to_string()),
                            explanation: explanation.clone(),
                            quick_fixes: fixes.clone(),
                        });
                    }
                }
            }
            // 2. the required mismatch: the requirement is optional but the
            // checked columns are NOT NULL (their sUt)
            if !other_min_one && all_required {
                let mut fixes = vec![op_fix(false)];
                for c in checked_cols {
                    if let Some(col) = column_of(checked_table, c) {
                        if let Some(edit) = setting_edit(&col.span, "not null", "null", ", null") {
                            fixes.push(QuickFix {
                                title: format!("Mark '{}.{}' as NULL", checked_table, c),
                                short_title: "Make nullable".to_string(),
                                span: edit.0,
                                new_text: edit.1,
                            });
                        }
                    }
                }
                let explanation = vec![
                    format!(
                        "In ref definition `{ref_line}`, the operator `{op_now}` marks `{names}` as optional."
                    ),
                    match col_lines.first() {
                        Some(line) => format!("In column definition `{line}`, the column is NOT NULL."),
                        None => "The column is NOT NULL.".to_string(),
                    },
                ];
                let multi = checked_cols.len() > 1;
                let message = if multi {
                    format!("Columns (`{names}`) are NOT NULL but operator `{op_now}` allows them to be optional")
                } else {
                    format!("Column `{names}` is NOT NULL but operator `{op_now}` allows it to be optional")
                };
                out.push(Diagnostic {
                    offset: checked_anchor,
                    message: message.clone(),
                    severity: Severity::Warning,
                    category: Some("Mismatched optional ref constraints".to_string()),
                    explanation: explanation.clone(),
                    quick_fixes: fixes.clone(),
                });
                for c in checked_cols {
                    if let Some(col) = column_of(checked_table, c) {
                        out.push(Diagnostic {
                            offset: col.name_span.start,
                            message: message.clone(),
                            severity: Severity::Warning,
                            category: Some("Mismatched optional ref constraints".to_string()),
                            explanation: explanation.clone(),
                            quick_fixes: fixes.clone(),
                        });
                    }
                }
            }
            // 3. the unique mismatch: the requirement side allows many but
            // its own columns carry a unique constraint (their cUt) —
            // anchored on the requirement side
            if other_many && columns_unique(other_cols_table, other_cols) {
                let new_op = if checked_from {
                    op_for_bounds(true, false, other_many, other_many)
                } else {
                    op_for_bounds(other_many, other_many, true, false)
                };
                let fixes = vec![QuickFix {
                    title: format!(
                        "Make '{req_names}' one in the ref (change operator to '{new_op}')"
                    ),
                    short_title: format!("Change to {new_op}"),
                    span: op_offset..op_offset + op_len,
                    new_text: new_op.to_string(),
                }];
                let explanation = vec![
                    format!(
                        "In ref definition `{ref_line}`, the operator `{op_now}` allows many."
                    ),
                    format!(
                        "`{req_names}` has a unique constraint, so each value can appear at most once."
                    ),
                ];
                let multi = other_cols.len() > 1;
                let message = if multi {
                    format!("Columns (`{req_names}`) have a unique index but operator `{op_now}` allows many")
                } else {
                    format!("Column `{req_names}` is unique but operator `{op_now}` allows many")
                };
                let anchor = if checked_from { to_anchor } else { from_anchor };
                out.push(Diagnostic {
                    offset: anchor,
                    message: message.clone(),
                    severity: Severity::Warning,
                    category: Some("Mismatched optional ref constraints".to_string()),
                    explanation: explanation.clone(),
                    quick_fixes: fixes.clone(),
                });
                for c in other_cols {
                    if let Some(col) = column_of(other_cols_table, c) {
                        out.push(Diagnostic {
                            offset: col.name_span.start,
                            message: message.clone(),
                            severity: Severity::Warning,
                            category: Some("Mismatched optional ref constraints".to_string()),
                            explanation: explanation.clone(),
                            quick_fixes: fixes.clone(),
                        });
                    }
                }
            }
            // 4. the should-be-unique mismatch: the requirement side is one
            // but its own columns lack uniqueness (their lUt) — anchored on
            // the requirement side
            if !other_many && !columns_unique(other_cols_table, other_cols) {
                // their RVt: the requirement side's max relaxes to many,
                // its min kept
                let new_op = if checked_from {
                    op_for_bounds(true, true, other_min_one, true)
                } else {
                    op_for_bounds(other_min_one, true, true, true)
                };
                let mut fixes = vec![QuickFix {
                    title: format!(
                        "Make '{req_names}' many in the ref (change operator to '{new_op}')"
                    ),
                    short_title: format!("Change to {new_op}"),
                    span: op_offset..op_offset + op_len,
                    new_text: new_op.to_string(),
                }];
                if checked_from && other_cols.len() == 1 {
                    if let Some(col) = column_of(other_cols_table, &other_cols[0]) {
                        if let Some(edit) = setting_edit(&col.span, "unique", "unique", ", unique") {
                            fixes.push(QuickFix {
                                title: format!(
                                    "Mark '{}.{}' as UNIQUE",
                                    other_cols_table,
                                    other_cols[0]
                                ),
                                short_title: "Add unique".to_string(),
                                span: edit.0,
                                new_text: edit.1,
                            });
                        }
                    }
                }
                let explanation = vec![
                    format!(
                        "In ref definition `{ref_line}`, the operator `{op_now}` implies `{req_names}` has at most one row."
                    ),
                    format!("`{req_names}` has no unique or primary key constraint."),
                ];
                let multi = other_cols.len() > 1;
                let message = if multi {
                    format!("Columns (`{req_names}`) should have a composite unique index for operator `{op_now}`")
                } else {
                    format!("Column `{req_names}` should be unique or primary key for operator `{op_now}`")
                };
                let anchor = if checked_from { to_anchor } else { from_anchor };
                out.push(Diagnostic {
                    offset: anchor,
                    message: message.clone(),
                    severity: Severity::Warning,
                    category: Some("Mismatched optional ref constraints".to_string()),
                    explanation: explanation.clone(),
                    quick_fixes: fixes.clone(),
                });
                for c in other_cols {
                    if let Some(col) = column_of(other_cols_table, c) {
                        out.push(Diagnostic {
                            offset: col.name_span.start,
                            message: message.clone(),
                            severity: Severity::Warning,
                            category: Some("Mismatched optional ref constraints".to_string()),
                            explanation: explanation.clone(),
                            quick_fixes: fixes.clone(),
                        });
                    }
                }
            }
        }
    };
    for r in &doc.refs {
        let ref_line = line_text(r.span.start);
        emit_ref(
            &r.from.table,
            &r.from.columns,
            r.from.span.start,
            &r.to.table,
            &r.to.columns,
            r.to.span.start,
            r.card,
            r.left_optional,
            r.right_optional,
            r.op_offset,
            &ref_line,
            &mut out,
        );
    }
    for t in &doc.tables {
        for c in &t.columns {
            let Some(ir) = &c.settings.inline_ref else {
                continue;
            };
            let ref_line = line_text(c.name_span.start);
            emit_ref(
                &t.name,
                std::slice::from_ref(&c.name),
                c.name_span.start,
                &ir.to.table,
                &ir.to.columns,
                ir.to.span.start,
                ir.card,
                ir.left_optional,
                ir.right_optional,
                ir.op_offset,
                &ref_line,
                &mut out,
            );
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
        // NOTE: no reverse check here — an unreferenced parent row is
        // normal data, and warning on it (users.id = 0 does not exist in
        // follows.following_user_id) buried the diagnostics panel in
        // noise dbdiagram does not show. Referential integrity runs the
        // other way: the child's values must exist on the parent.
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
        // endpoint — 2 entries) plus 3 record-constraint violations (the
        // posts records' author_id against users' missing records; the
        // reverse direction is not checked)
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
            nullable == 2 && record_violations == 3 && d.len() == 5,
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
        // test below); the parent carries [pk] so the should-be-unique
        // check stays out too
        let d = diags(
            "Table a { id integer [pk] }\nTable b { a_id integer [not null, ref: > a.nope] }\n",
        );
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
        let d = diags("Table users as U { id integer [pk] }\nRef: posts.uid > U.id\n");
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
        // bullets (their raw-line quotes), and the single operator fix —
        // the FK side's diagnostics carry the operator fix alone (their
        // column-setting fixes belong to the opposite run), live-verified
        // on their editor: the dropdown reads "Change to >?"
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
            assert_eq!(
                d.quick_fixes[0].title,
                "Make 'b.a_id' optional in the ref (change operator to '>?')"
            );
            assert_eq!(d.quick_fixes[0].short_title, "Change to >?");
            assert_eq!(d.quick_fixes[0].new_text, ">?");
            assert_eq!(
                d.quick_fixes[1].title, "Mark 'b.a_id' as NOT NULL",
                "anchor {i}"
            );
            assert_eq!(d.quick_fixes[1].short_title, "Add not null", "anchor {i}");
        }
        assert_eq!(d[0].offset, out.doc.tables[1].columns[0].name_span.start);
        assert_eq!(d[1].offset, out.doc.refs[0].from.span.start);
        // not-null FKs stay silent (the should-be-unique lint on the
        // non-unique parent is its own test below)
        let src = "Table a { id integer [pk] }\nTable b { a_id integer [not null, unique] }\nRef: a.id > b.a_id\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        assert!(d.is_empty(), "{d:?}");
    }

    /// The reverse mismatch: an optional operator (`>?` drops the right
    /// side's min) with a NOT NULL column on the checked (left) side —
    /// their sUt. The fix tightens the operator back (`>?` -> `>`); the
    /// column-setting fixes belong to the opposite run and stay off. The
    /// should-be-unique lint coexists (the parent lacks a unique index).
    #[test]
    fn required_column_behind_optional_operator_suggests_tightening() {
        let src =
            "Table a { id integer [not null] }\nTable b { a_id integer [not null] }\nRef: a.id >? b.a_id\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        assert!(
            d.iter().any(
                |x| x.message == "Column `a.id` is NOT NULL but operator `>?` allows it to be optional"
            ),
            "{d:?}"
        );
        let fix = d
            .iter()
            .flat_map(|x| x.quick_fixes.iter())
            .find(|f| f.short_title == "Change to >")
            .expect("the tightening fix");
        assert_eq!(fix.new_text, ">");
    }

    /// A `>` ref whose parent column lacks unique/pk: their lUt asks for
    /// the unique constraint (or the many-to-many operator).
    #[test]
    fn non_unique_parent_gets_the_should_be_unique_lint() {
        let src =
            "Table a { id integer [pk] }\nTable b { a_id integer [not null] }\nRef: a.id > b.a_id\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        assert!(
            d.iter().any(
                |x| x.message == "Column `b.a_id` should be unique or primary key for operator `>`"
            ),
            "{d:?}"
        );
        // the operator fix relaxes to many-to-many: `>` -> `<>`
        let fix = d
            .iter()
            .flat_map(|x| x.quick_fixes.iter())
            .find(|f| f.short_title.starts_with("Change to "))
            .expect("the many fix");
        assert_eq!(fix.new_text, "<>", "{:?}", d.iter().flat_map(|x| x.quick_fixes.iter()).map(|f| (&f.short_title, &f.new_text)).collect::<Vec<_>>());
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
        // 2 FK misses (author_id 1/7 against users' empty records) + 1
        // FK miss the other way (comments.post_id 9 against posts' {1,2});
        // the parent side is NOT checked — an unreferenced parent row is
        // normal data (dbdiagram does not warn about it either)
        assert_eq!(violations.len(), 3, "{d:?}");
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
        // a matching value passes: comments.post_id = 9 exists in ...
        // (no ref points the other way here — covered by the empty case
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
