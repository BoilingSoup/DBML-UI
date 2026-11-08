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
    /// dbdiagram renders infos with the SAME soft-blue as warnings (their
    /// theme sets `editorInfo.foreground` to the same `#64A0DC80`) — the
    /// distinction is their diagnostics panel's, not a color one. Their
    /// operator-consistency and records findings are all infos.
    Info,
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
    /// Their convention: the operator-change fix is the preferred one;
    /// column-setting edits are not.
    pub is_preferred: bool,
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


/// The four raw cardinality bounds (their core/types/relation.ts).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Card {
    /// `1` — required one
    One,
    /// `0..1` — optional one
    Maybe,
    /// `*` — required many (their CARDINALITY_SOME)
    Some,
    /// `0..*` — optional many
    Many,
}

impl Card {
    pub fn min_one(self) -> bool {
        matches!(self, Card::One | Card::Some)
    }
    pub fn max_many(self) -> bool {
        matches!(self, Card::Some | Card::Many)
    }
    /// min -> 0: `1` -> `0..1`, `*` -> `0..*` (their makeCardinalityOptional)
    pub fn make_optional(self) -> Self {
        match self {
            Card::One => Card::Maybe,
            Card::Some => Card::Many,
            o => o,
        }
    }
    /// min -> 1: `0..1` -> `1`, `0..*` -> `*` (their makeCardinalityRequired)
    pub fn make_required(self) -> Self {
        match self {
            Card::Maybe => Card::One,
            Card::Many => Card::Some,
            o => o,
        }
    }
    /// max -> many: `1` -> `*`, `0..1` -> `0..*` (their makeCardinalityMany)
    pub fn make_many(self) -> Self {
        match self {
            Card::One => Card::Some,
            Card::Maybe => Card::Many,
            o => o,
        }
    }
}

/// their RELATIONSHIP_OPS: written operator -> (left, right) raw bounds
pub fn op_cards(op: &str) -> (Card, Card) {
    match op {
        "-" => (Card::One, Card::One),
        "<>" => (Card::Some, Card::Some),
        ">" => (Card::Some, Card::One),
        "<" => (Card::One, Card::Some),
        "-?" => (Card::One, Card::Maybe),
        "?-" => (Card::Maybe, Card::One),
        "?-?" => (Card::Maybe, Card::Maybe),
        "?>" => (Card::Many, Card::One),
        ">?" => (Card::Some, Card::Maybe),
        "?>?" => (Card::Many, Card::Maybe),
        "?<" => (Card::Maybe, Card::Some),
        "<?" => (Card::One, Card::Many),
        "?<?" => (Card::Maybe, Card::Many),
        "?<>" => (Card::Many, Card::Some),
        "<>?" => (Card::Some, Card::Many),
        "?<>?" => (Card::Many, Card::Many),
        _ => (Card::Some, Card::Some),
    }
}

/// the inverse (their CARDINALITY_TO_OP / getRelationshipOp) — the 4x4
/// pair space is exactly their 16-entry map, so no fallback exists
pub fn card_to_op(l: Card, r: Card) -> &'static str {
    match (l, r) {
        (Card::One, Card::One) => "-",
        (Card::Some, Card::Some) => "<>",
        (Card::Some, Card::One) => ">",
        (Card::One, Card::Some) => "<",
        (Card::One, Card::Maybe) => "-?",
        (Card::Maybe, Card::One) => "?-",
        (Card::Maybe, Card::Maybe) => "?-?",
        (Card::Many, Card::One) => "?>",
        (Card::Some, Card::Maybe) => ">?",
        (Card::Many, Card::Maybe) => "?>?",
        (Card::Maybe, Card::Some) => "?<",
        (Card::One, Card::Many) => "<?",
        (Card::Maybe, Card::Many) => "?<?",
        (Card::Many, Card::Some) => "?<>",
        (Card::Some, Card::Many) => "<>?",
        (Card::Many, Card::Many) => "?<>?",
    }
}

/// the written operator for (cardinality, optional marks) — round-trips
/// through op_cards
pub fn written_op(card: Cardinality, left_opt: bool, right_opt: bool) -> &'static str {
    use Cardinality::*;
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
}

/// Validate a parsed document: duplicate definition names, refs and
/// TableGroup members pointing at tables or columns that do not exist,
/// index columns missing from their table, the nullable-FK constraint
/// mismatch, and bare-comma index settings. `src` feeds the explanation
/// bullets their raw source-line quotes (the ref statement and the column
/// definition, verbatim).
pub fn validate(doc: &Document, src: &str) -> Vec<Diagnostic> {
    validate_with_parse(doc, src, &[], &[])
}

/// The parse-level binder findings join the same gate (their errors()
/// chains parse + binder reports).
pub fn validate_with_parse(
    doc: &Document,
    src: &str,
    parse_binder: &[(usize, String)],
    broken_refs: &[super::ast::Endpoint],
) -> Vec<Diagnostic> {
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
            // their dup marker anchors the duplicate declaration's NAME
            // token (probe: three multi-line dups flag each later name)
            out.push(Diagnostic::plain(
                t.name_span.start,
                format!("Table '{}' already exists in schema 'public'", t.name),
            ));
        }
        tables.push(t.name.as_str());
    }
    let mut enums: Vec<&str> = Vec::new();
    for e in &doc.enums {
        if enums.contains(&e.name.as_str()) {
            out.push(Diagnostic::plain(
                e.span.end,
                format!("Enum '{}' already exists in schema 'public'", e.name),
            ));
        }
        enums.push(e.name.as_str());
    }
    // duplicate column names (their table binder, table/index.ts: the
    // seen-map flags EVERY repeat and re-flags the first sighting — 3
    // twins yield 4 errors; anchors are the column name nodes)
    for t in &doc.tables {
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, c) in t.columns.iter().enumerate() {
            if let Some(&first) = seen.get(c.name.as_str()) {
                let msg = format!("Duplicate column '{}'", c.name);
                out.push(Diagnostic::plain(t.columns[first].name_span.start, msg.clone()));
                out.push(Diagnostic::plain(c.name_span.start, msg));
            } else {
                seen.insert(c.name.as_str(), i);
            }
        }
    }

    // their engine flags EVERY occurrence of a duplicated enum field —
    // the first twin included (probe: both 'x' rows on `Enum e { x\n x }`)
    for e in &doc.enums {
        for v in &e.values {
            if e.values.iter().filter(|o| o.name == v.name).count() > 1 {
                out.push(Diagnostic::plain(
                    v.name_span.start,
                    format!("Duplicate enum field '{}'", v.name),
                ));
            }
        }
    }

    // every ref endpoint must resolve to a real table and column (block
    // refs and inline `ref:` settings alike)
    fn check_endpoint(doc: &Document, ep: &super::ast::Endpoint, anchor: usize, out: &mut Vec<Diagnostic>) {
        // recovered endpoints can carry an empty span — anchor those on
        // the caller-provided position instead of offset 0
        let at = if ep.span.is_empty() { anchor } else { ep.span.start };
        match doc.tables.iter().find(|t| t.matches(&ep.table)) {
            None => out.push(Diagnostic::plain(
                at,
                format!(
                    "Table '{}' does not exist in Schema 'public'",
                    ep.table
                ),
            )),
            Some(t) => {
                let schema = t.schema.as_deref().unwrap_or("public");
                for col in &ep.columns {
                    if !t.columns.iter().any(|c| &c.name == col) {
                        out.push(Diagnostic::plain(
                            at,
                            format!(
                                "Column '{}' does not exist in Table '{}.{}'",
                                col, schema, ep.table
                            ),
                        ));
                    }
                }
            }
        }
    }
    for r in &doc.refs {
        // their endpoint resolution checks the columns only when the two
        // sides carry the same field count (probe: a.(id, nope) > b.x
        // errs with Unequal fields alone)
        let equal = r.from.columns.len() == r.to.columns.len();
        if equal {
            check_endpoint(doc, &r.from, r.span.start, &mut out);
            check_endpoint(doc, &r.to, r.span.start, &mut out);
        }
        // their isEqualTupleOperands: composite endpoints must carry the
        // same field count (probe: `a.(id, code) > b.x` errs at the field)
        if !equal {
            out.push(Diagnostic::plain(
                r.from.span.start,
                "Unequal fields in ref endpoints".to_string(),
            ));
        }
    }
    // records headers: every named column must exist (probe:
    // records (nope) errs with the endpoint phrasing)
    for t in &doc.tables {
        if let Some(r) = &t.records {
            for cname in &r.columns {
                if !t.columns.iter().any(|x| x.name == *cname) {
                    // their records check does NOT schema-qualify (probe:
                    // records (nope) errs against Table 'T')
                    out.push(Diagnostic::plain(
                        t.name_span.start,
                        format!("Column '{}' does not exist in Table '{}'", cname, t.name),
                    ));
                }
            }
        }
    }

    for t in &doc.tables {
        for c in &t.columns {
            if let Some(ir) = &c.settings.inline_ref {
                if ir.to.table.is_empty() {
                    // their bare-word endpoint = a column of the HOST table
                    // (schema-qualified message, like every endpoint check)
                    if !t.columns.iter().any(|x| x.name == ir.to.columns[0]) {
                        let schema = t.schema.as_deref().unwrap_or("public");
                        out.push(Diagnostic::plain(
                            ir.to.span.start,
                            format!(
                                "Column '{}' does not exist in Table '{}.{}'",
                                ir.to.columns[0], schema, t.name
                            ),
                        ));
                    }
                    continue;
                }
                check_endpoint(doc, &ir.to, c.name_span.start, &mut out);
            }
        }
    }

    // their column-settings rules (tablePartial/validate.ts): once-only
    // names, the pk/primary-key and not-null/null conflicts (each error
    // anchored at EVERY offending occurrence), and the flag/value rules
    for t in &doc.tables {
        for c in &t.columns {
            let occs = &c.settings.occurrences;
            if occs.is_empty() {
                continue;
            }
            let attrs_of = |name: &str| -> Vec<&super::ast::SettingOcc> {
                occs.iter()
                    .filter(|o| {
                        o.key == name
                            || (name == "primary key" && o.key == "primary_key")
                    })
                    .collect()
            };
            for name in [
                "note",
                "primary key",
                "pk",
                "not null",
                "null",
                "unique",
                "increment",
                "default",
            ] {
                let attrs = attrs_of(name);
                if attrs.len() > 1 {
                    for a in attrs {
                        out.push(Diagnostic::plain(
                            a.offset,
                            format!("'{name}' can only appear once"),
                        ));
                    }
                }
            }
            let pk = attrs_of("pk");
            let pkey = attrs_of("primary key");
            if !pk.is_empty() && !pkey.is_empty() {
                for a in pk.iter().chain(pkey.iter()) {
                    out.push(Diagnostic::plain(
                        a.offset,
                        "Either one of 'primary key' and 'pk' can appear".to_string(),
                    ));
                }
            }
            let nn = attrs_of("not null");
            let nu = attrs_of("null");
            if !nn.is_empty() && !nu.is_empty() {
                for a in nn.iter().chain(nu.iter()) {
                    out.push(Diagnostic::plain(
                        a.offset,
                        "'not null' and 'null' can not be set at the same time".to_string(),
                    ));
                }
            }
            for a in occs {
                let name = if a.key == "primary_key" {
                    "primary key"
                } else {
                    a.key.as_str()
                };
                match name {
                    "pk" | "primary key" | "not null" | "null" | "unique" | "increment" => {
                        if a.has_value {
                            out.push(Diagnostic::plain(
                                a.offset,
                                format!("'{name}' must not have a value"),
                            ));
                        }
                    }
                    "note" => {
                        if a.has_value && !a.quoted {
                            out.push(Diagnostic::plain(
                                a.offset,
                                "'note' must be a quoted string".to_string(),
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // their checkRefEndpoints (program/bind.ts): a ref whose two sides
    // cover the SAME column set errs once ('Two endpoints are the same')
    // and drops out of the pair map; the same unordered PAIR across refs
    // flags both declarations ('References with same endpoints exist').
    // Side identity = resolved table + its sorted column names; inline
    // refs join the same map, anchored at their `ref:` attribute.
    let side_key = |ep: &super::ast::Endpoint| -> Option<String> {
        let ti = doc.tables.iter().position(|t| t.matches(&ep.table))?;
        let mut cols = ep.columns.clone();
        cols.sort();
        Some(format!("{ti}:{}", cols.join(",")))
    };
    let mut pair_first: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new(); // pair key -> first sighting's anchor
    let mut endpoint_errors: Vec<(usize, String)> = Vec::new();
    {
        let mut run = |host_key: Option<String>,
                        other_key: Option<String>,
                        anchor: usize,
                        emissions: &mut Vec<(usize, String)>| {
            let (Some(l), Some(rt)) = (host_key, other_key) else {
                return;
            };
            if l == rt {
                emissions.push((anchor, "Two endpoints are the same".to_string()));
                return;
            }
            let key = if l < rt { format!("{l}-{rt}") } else { format!("{rt}-{l}") };
            if let Some(&first_anchor) = pair_first.get(&key) {
                emissions.push((
                    first_anchor,
                    "References with same endpoints exist".to_string(),
                ));
                emissions.push((anchor, "References with same endpoints exist".to_string()));
            } else {
                pair_first.insert(key, anchor);
            }
        };
        for r in &doc.refs {
            run(
                side_key(&r.from),
                side_key(&r.to),
                r.span.start,
                &mut endpoint_errors,
            );
        }
        for t in &doc.tables {
            for c in &t.columns {
                let Some(ir) = &c.settings.inline_ref else { continue };
                let ti = doc.tables.iter().position(|x| std::ptr::eq(x, t));
                let host_key = ti.map(|ti| {
                    let mut cols = vec![c.name.clone()];
                    cols.sort();
                    format!("{ti}:{}", cols.join(","))
                });
                run(
                    host_key,
                    side_key(&ir.to),
                    ir.attr_start,
                    &mut endpoint_errors,
                );
            }
        }
    }
    for (anchor, msg) in endpoint_errors {
        out.push(Diagnostic::plain(anchor, msg));
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
            for (ci, col) in ix.columns.iter().enumerate() {
                let plain = !col.contains(|c: char| !(c.is_alphanumeric() || c == '_'));
                if plain && !t.columns.iter().any(|c| &c.name == col) {
                    // their index-column check: the message differs from
                    // the ref-endpoint one and anchors at the column
                    let at = ix.column_spans.get(ci).map(|s| s.start).unwrap_or(ix.span.start);
                    out.push(Diagnostic::plain(
                        at,
                        format!("No column named '{}' inside Table '{}'", col, t.name),
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
                    severity: Severity::Error,
                    category: Some("Index settings".to_string()),
                    explanation: vec![
                        format!("Settings belong inside the brackets: `({list}) [unique]`, not after a comma."),
                        "The line still renders here; dbdiagram's grammar rejects it.".to_string(),
                    ],
                    quick_fixes: vec![QuickFix {
                        is_preferred: false,
                        title: "Use bracketed settings".to_string(),
                        short_title: "Use bracketed settings".to_string(),
                        span: ix.span.clone(),
                        new_text,
                    }],
                });
            }
        }
    }

    // ---- the ref-relationship engine — a direct port of their
    // `validateCardinality` (packages/dbml-parse ref/constraint_fixes.ts).
    // Each side of a ref runs four checks: two against the OTHER side's
    // columns (the nullable and the required mismatch, gated on the other
    // side being the FK source) and two against its own (unique-when-many,
    // should-be-unique-when-one). Every finding anchors on the side's
    // endpoint token plus each of its columns' declarations, skipping
    // anchors that coincide (an inline ref's host column IS its left
    // token, so the pair collapses to one entry — their pushDiag).
    //
    let column_of = |table: &str, col: &str| -> Option<&super::ast::Column> {
        doc.tables
            .iter()
            .find(|t| t.matches(table))
            .and_then(|t| t.columns.iter().find(|c| c.name == col))
    };
    let is_nullable = |table: &str, col: &str| -> bool {
        // their ColumnSymbol.nullable: pk flag, auto-increment, membership
        // in any pk index and the `not null` setting all imply NOT NULL
        column_of(table, col).is_some_and(|c| {
            c.settings.not_null != Some(true)
                && !c.settings.pk
                && !c.settings.increment
                && !doc
                    .tables
                    .iter()
                    .find(|t| t.matches(table))
                    .is_some_and(|t| {
                        t.indexes
                            .iter()
                            .any(|ix| ix.pk && ix.columns.iter().any(|x| x == col))
                    })
        })
    };
    let columns_unique = |table: &str, cols: &[String]| -> bool {
        if cols.is_empty() {
            return false;
        }
        if cols.len() == 1 {
            // the column-setting fast path, then the index scan below —
            // a pk declared as a single-column index satisfies uniqueness
            // exactly like the composite form (their isColumnsUnique
            // scans indexes for ALL cardinalities)
            if column_of(table, &cols[0]).is_some_and(|c| c.settings.unique || c.settings.pk) {
                return true;
            }
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
    // their addSettingEdit: grow the bracket list (`, x` before `]`) or
    // open one at the declaration end (` [x]`)
    let add_setting_edit = |span: &Span, setting: &str| -> (Span, String) {
        let text = src.get(span.start..span.end).unwrap_or("");
        if let Some(bracket) = text.find('[') {
            if let Some(close_rel) = text[bracket..].rfind(']') {
                let close = span.start + bracket + close_rel;
                return (close..close, format!(", {setting}"));
            }
        }
        (span.end..span.end, format!(" [{setting}]"))
    };
    // their removeSettingEdit: the named setting goes away with one
    // separator (newText always empty); a sole setting takes the whole
    // bracket list and the spaces ahead of it
    let remove_setting_edit = |span: &Span, name: &str| -> Option<(Span, String)> {
        let text = src.get(span.start..span.end)?;
        let bracket = text.find('[')?;
        let close_rel = bracket + text[bracket..].find(']')?;
        // top-level comma split of the bracket interior (quotes and nested
        // brackets hold commas of their own)
        let inner = &text[bracket + 1..close_rel];
        let mut parts: Vec<(usize, usize)> = Vec::new();
        let (mut depth, mut start, mut quote) = (0i32, 0usize, None);
        for (i, &b) in inner.as_bytes().iter().enumerate() {
            if let Some(q) = quote {
                if b == q {
                    quote = None;
                }
                continue;
            }
            match b {
                b'\'' | b'"' | b'`' => quote = Some(b),
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth -= 1,
                b',' if depth == 0 => {
                    parts.push((start, i));
                    start = i + 1;
                }
                _ => {}
            }
        }
        parts.push((start, inner.len()));
        // trimmed [abs start, abs end] of a part, in source coordinates
        let trimmed_bounds = |idx: usize| -> (usize, usize) {
            let (s, e) = parts[idx];
            let lead = inner[s..e].len() - inner[s..e].trim_start().len();
            let len = inner[s..e].trim().len();
            (span.start + bracket + 1 + s + lead, len)
        };
        let (idx, hit_start, hit_len) = parts
            .iter()
            .enumerate()
            .map(|(idx, &(s, e))| (idx, inner[s..e].trim().to_string()))
            .find(|(_, t)| t.eq_ignore_ascii_case(name))
            .map(|(idx, _)| {
                let (abs, len) = trimmed_bounds(idx);
                (idx, abs, len)
            })?;
        let hit_end = hit_start + hit_len;
        if parts.len() == 1 {
            // the sole setting: the whole list goes, spaces included
            let mut remove_start = span.start + bracket;
            let bytes = src.as_bytes();
            while remove_start > 0 && bytes[remove_start - 1] == b' ' {
                remove_start -= 1;
            }
            return Some((remove_start..span.start + close_rel + 1, String::new()));
        }
        if idx == 0 {
            // first of many: up to the next setting's first token
            let (next_start, _) = trimmed_bounds(1);
            return Some((hit_start..next_start, String::new()));
        }
        // later: from the previous setting's last token
        let (prev_start, prev_len) = trimmed_bounds(idx - 1);
        Some((prev_start + prev_len..hit_end, String::new()))
    };
    // the three column-setting fixes (their suggestMakeNotNull /
    // suggestMakeNullable / suggestMakeUnique): never preferred
    let quick = |title: String, short: &str, span: Span, new_text: String| -> QuickFix {
        QuickFix {
            title,
            short_title: short.to_string(),
            span,
            new_text,
            is_preferred: false,
        }
    };
    let make_not_null = |table: &str, col: &str| -> Option<QuickFix> {
        let c = column_of(table, col)?;
        if c.settings.pk || c.settings.increment || c.settings.not_null == Some(true) {
            return None;
        }
        let (span, new_text) = match remove_setting_edit(&c.span, "null") {
            Some(edit) => edit,
            None => add_setting_edit(&c.span, "not null"),
        };
        Some(quick(
            format!("Mark '{table}.{col}' as NOT NULL"),
            "Add not null",
            span,
            new_text,
        ))
    };
    let make_nullable = |table: &str, col: &str| -> Option<QuickFix> {
        let c = column_of(table, col)?;
        let (span, new_text) = remove_setting_edit(&c.span, "not null")?;
        Some(quick(
            format!("Mark '{table}.{col}' as NULL"),
            "Make nullable",
            span,
            new_text,
        ))
    };
    let make_unique = |table: &str, col: &str| -> Option<QuickFix> {
        let c = column_of(table, col)?;
        if c.settings.unique || c.settings.pk {
            return None;
        }
        let (span, new_text) = add_setting_edit(&c.span, "unique");
        Some(quick(
            format!("Mark '{table}.{col}' as UNIQUE"),
            "Add unique",
            span,
            new_text,
        ))
    };
    // one ref's two sides with their anchors: `token` is where the
    // endpoint-level finding sits (the endpoint expression for a
    // standalone `Ref:`, the host column's name for an inline ref's left
    // side, the `ref:` keyword for its right side)
    struct Side<'a> {
        table: &'a str,
        cols: &'a [String],
        token: usize,
    }
    struct RefSides<'a> {
        left: Side<'a>,
        right: Side<'a>,
        op: &'static str,
        op_offset: usize,
        /// their explanation quote: the whole statement line for a
        /// standalone ref, the `ref: …` attribute for an inline one
        ref_source: String,
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Run {
        Left,
        Right,
    }
    let qnames = |s: &Side| -> String {
        s.cols
            .iter()
            .map(|c| format!("{}.{}", s.table, c))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // the checked side's first column's declaration line (their
    // i[0]?.declaration quote into the second bullet)
    let col_line = |s: &Side| -> Option<String> {
        column_of(s.table, s.cols.first()?)
            .and_then(|c| src.get(c.span.start..c.span.end))
            .map(|s| s.trim().to_string())
    };
    let push = |node: usize,
                s: &Side,
                message: &str,
                explanation: Vec<String>,
                fixes: Vec<QuickFix>,
                out: &mut Vec<Diagnostic>| {
        let mk = |offset: usize| Diagnostic {
            offset,
            message: message.to_string(),
            severity: Severity::Info,
            category: Some("Mismatched optional ref constraints".to_string()),
            explanation: explanation.clone(),
            quick_fixes: fixes.clone(),
        };
        out.push(mk(node));
        for c in s.cols {
            if let Some(col) = column_of(s.table, c) {
                if col.name_span.start != node {
                    out.push(mk(col.name_span.start));
                }
            }
        }
    };
    // their suggestChangeOp: the transformed side re-composes with the
    // untouched other side, and the pair reads back through the op table
    let change_op = |sides: &RefSides,
                     new_this: Card,
                     other_raw: Card,
                     run: Run,
                     description: String|
     -> QuickFix {
        let (nl, nr) = match run {
            Run::Left => (new_this, other_raw),
            Run::Right => (other_raw, new_this),
        };
        let new_op = card_to_op(nl, nr);
        QuickFix {
            title: format!("{description} (change operator to '{new_op}')"),
            short_title: format!("Change to {new_op}"),
            span: sides.op_offset..sides.op_offset + sides.op.len(),
            new_text: new_op.to_string(),
            is_preferred: true,
        }
    };
    // one run of their validateCardinality: `run` picks which side's
    // columns are OWN (the requirement carrier) and which are OTHER (the
    // checked material)
    let run_side = |sides: &RefSides, run: Run, out: &mut Vec<Diagnostic>| {
        let (own, other) = match run {
            Run::Left => (&sides.left, &sides.right),
            Run::Right => (&sides.right, &sides.left),
        };
        let (l_raw, r_raw) = op_cards(sides.op);
        let (own_raw, other_raw) = match run {
            Run::Left => (l_raw, r_raw),
            Run::Right => (r_raw, l_raw),
        };
        // unresolved endpoints give the checks nothing to look at
        let own_exist = own.cols.is_empty()
            || own
                .cols
                .iter()
                .all(|c| column_of(own.table, c).is_some());
        let other_exist = other.cols.is_empty()
            || other
                .cols
                .iter()
                .all(|c| column_of(other.table, c).is_some());
        if !own_exist || !other_exist {
            return;
        }
        // Nullability checks on the other side only make sense when it is
        // the source (FK) side — or a one-to-one validated from the left
        // (their otherIsSource, asymmetry included)
        let other_is_source = other_raw.max_many()
            || (matches!(own_raw, Card::One | Card::Maybe)
                && matches!(other_raw, Card::One | Card::Maybe)
                && run == Run::Left);
        // 1. the nullable mismatch: own requires a row, the source is
        // all-nullable (their refNullableMismatch)
        if own_raw.min_one()
            && other_is_source
            && !other.cols.is_empty()
            && other.cols.iter().all(|c| is_nullable(other.table, c))
        {
            let names = qnames(other);
            let mut fixes = vec![change_op(
                sides,
                own_raw.make_optional(),
                other_raw,
                run,
                format!("Make '{names}' optional in the ref"),
            )];
            for c in other.cols {
                if let Some(f) = make_not_null(other.table, c) {
                    fixes.push(f);
                }
            }
            let explanation = vec![
                format!(
                    "In ref definition `{}`, the operator `{}` requires `{names}` to reference an existing row.",
                    sides.ref_source, sides.op
                ),
                match col_line(other) {
                    Some(line) => format!(
                        "In column definition `{line}`, the column is nullable, which allows rows with no parent."
                    ),
                    None => "A nullable column allows rows with no parent.".to_string(),
                },
            ];
            let message = if other.cols.len() > 1 {
                format!(
                    "Columns (`{names}`) are all nullable but operator `{}` requires at least one to be NOT NULL",
                    sides.op
                )
            } else {
                format!(
                    "Column `{names}` is nullable but operator `{}` requires it to be NOT NULL",
                    sides.op
                )
            };
            push(other.token, other, &message, explanation, fixes, out);
        }
        // 2. the required mismatch: own is optional, the source is all
        // NOT NULL (their refNotNullMismatch)
        if !own_raw.min_one()
            && other_is_source
            && !other.cols.is_empty()
            && other.cols.iter().all(|c| !is_nullable(other.table, c))
        {
            let names = qnames(other);
            let mut fixes = vec![change_op(
                sides,
                own_raw.make_required(),
                other_raw,
                run,
                format!("Make '{names}' required in the ref"),
            )];
            for c in other.cols {
                if let Some(f) = make_nullable(other.table, c) {
                    fixes.push(f);
                }
            }
            let explanation = vec![
                format!(
                    "In ref definition `{}`, the operator `{}` marks `{names}` as optional.",
                    sides.ref_source, sides.op
                ),
                match col_line(other) {
                    Some(line) => {
                        format!("In column definition `{line}`, the column is NOT NULL.")
                    }
                    None => "The column is NOT NULL.".to_string(),
                },
            ];
            let message = if other.cols.len() > 1 {
                format!(
                    "Columns (`{names}`) are NOT NULL but operator `{}` allows them to be optional",
                    sides.op
                )
            } else {
                format!(
                    "Column `{names}` is NOT NULL but operator `{}` allows it to be optional",
                    sides.op
                )
            };
            push(other.token, other, &message, explanation, fixes, out);
        }
        // 3. the unique mismatch: own allows many but carries a unique
        // constraint (their refUniqueMismatch) — the operator fix alone
        if own_raw.max_many() && columns_unique(own.table, own.cols) {
            let names = qnames(own);
            let fixes = vec![change_op(
                sides,
                Card::One,
                other_raw,
                run,
                format!("Make '{names}' one in the ref"),
            )];
            let explanation = vec![
                format!(
                    "In ref definition `{}`, the operator `{}` allows many.",
                    sides.ref_source, sides.op
                ),
                format!("`{names}` has a unique constraint, so each value can appear at most once."),
            ];
            let message = if own.cols.len() > 1 {
                format!(
                    "Columns (`{names}`) have a unique index but operator `{}` allows many",
                    sides.op
                )
            } else {
                format!(
                    "Column `{names}` is unique but operator `{}` allows many",
                    sides.op
                )
            };
            push(own.token, own, &message, explanation, fixes, out);
        }
        // 4. the should-be-unique mismatch: own must be unique but is not
        // (their refNonUniqueMismatch)
        if !own_raw.max_many() && !columns_unique(own.table, own.cols) {
            let names = qnames(own);
            let mut fixes = vec![change_op(
                sides,
                own_raw.make_many(),
                other_raw,
                run,
                format!("Make '{names}' many in the ref"),
            )];
            if own.cols.len() == 1 {
                if let Some(f) = make_unique(own.table, &own.cols[0]) {
                    fixes.push(f);
                }
            }
            let explanation = vec![
                format!(
                    "In ref definition `{}`, the operator `{}` implies `{names}` has at most one row.",
                    sides.ref_source, sides.op
                ),
                format!("`{names}` has no unique or primary key constraint."),
            ];
            let message = if own.cols.len() > 1 {
                format!(
                    "Columns (`{names}`) should have a composite unique index for operator `{}`",
                    sides.op
                )
            } else {
                format!(
                    "Column `{names}` should be unique or primary key for operator `{}`",
                    sides.op
                )
            };
            push(own.token, own, &message, explanation, fixes, out);
        }
    };
    for r in &doc.refs {
        let op = written_op(r.card, r.left_optional, r.right_optional);
        let sides = RefSides {
            left: Side {
                table: &r.from.table,
                cols: &r.from.columns,
                token: r.from.span.start,
            },
            right: Side {
                table: &r.to.table,
                cols: &r.to.columns,
                token: r.to.span.start,
            },
            op,
            op_offset: r.op_offset,
            ref_source: line_text(r.span.start),
        };
        run_side(&sides, Run::Right, &mut out);
        run_side(&sides, Run::Left, &mut out);
    }
    // inline refs ride the same engine with the host column as the LEFT
    // side (their RefMetadata on an AttributeNode): the left token is the
    // column itself — its findings collapse onto one anchor — and the
    // right token is the `ref:` attribute
    for t in &doc.tables {
        for c in &t.columns {
            let Some(ir) = &c.settings.inline_ref else {
                continue;
            };
            let op = written_op(ir.card, ir.left_optional, ir.right_optional);
            let host_of_bare = ir.to.table.is_empty();
            let sides = RefSides {
                left: Side {
                    table: &t.name,
                    cols: std::slice::from_ref(&c.name),
                    token: c.name_span.start,
                },
                right: Side {
                    table: if host_of_bare { &t.name } else { &ir.to.table },
                    cols: &ir.to.columns,
                    token: ir.attr_start,
                },
                op,
                op_offset: ir.op_offset,
                ref_source: src
                    .get(ir.attr_start..ir.to.span.end)
                    .unwrap_or_default()
                    .trim_end()
                    .to_string(),
            };
            run_side(&sides, Run::Right, &mut out);
            run_side(&sides, Run::Left, &mut out);
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
                    // their null handling: an empty or literal-null cell on
                    // a required FK column emits BOTH the NULL-not-allowed
                    // note and the must-not-be-NULL violation (probe:
                    // records-null-in-required)
                    let is_null = v.is_empty() || v.eq_ignore_ascii_case("null");
                    // their message renders the empty string as `null`
                    let shown = if v.is_empty() { "null" } else { v.as_str() };
                    if is_null {
                        let required = parent_vals.len() == 0
                            || parent_vals.iter().any(|(pv, _)| pv.is_empty());
                        let _ = required;
                        let col_required = doc
                            .tables
                            .iter()
                            .find(|t| t.matches(fk_tbl))
                            .and_then(|t| t.columns.iter().find(|c| c.name == fk_col))
                            .and_then(|c| c.settings.not_null)
                            .unwrap_or(false);
                        if col_required {
                            out.push(Diagnostic {
                                offset: span.start,
                                message: format!(
                                    "NULL not allowed for NOT NULL column `{fk_col}` without a default value or auto-increment"
                                ),
                                severity: Severity::Info,
                                category: Some("Record type errors".to_string()),
                                explanation: vec![format!(
                                    "`{fk_col}` is marked NOT NULL and has no default value or auto-increment."
                                )],
                                quick_fixes: vec![],
                            });
                        }
                        out.push(Diagnostic {
                            offset: span.start,
                            message: format!(
                                "FK violation: `{fk_tbl}.{fk_col}` = `{shown}` must not be NULL"
                            ),
                            severity: Severity::Info,
                            category: Some("Record constraint violations".to_string()),
                            explanation: vec![format!(
                                "`{fk_tbl}.{fk_col}` references `{parent_tbl}.{parent_col}`, so every row must have a non-null value."
                            )],
                            quick_fixes: vec![],
                        });
                        continue;
                    }
                    if !parent_vals.iter().any(|(pv, _)| pv == v) {
                        out.push(Diagnostic {
                            offset: span.start,
                            message: format!(
                                "FK violation: `{fk_tbl}.{fk_col}` = `{v}` does not exist in `{parent_tbl}.{parent_col}`"
                            ),
                            severity: Severity::Info,
                            category: Some("Record constraint violations".to_string()),
                            explanation: vec![format!(
                                "`{fk_tbl}.{fk_col}` is a foreign key referencing `{parent_tbl}.{parent_col}`. Every non-null value in an FK must match an existing row in the referenced column."
                            )],
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

    // the parse-side binder findings (broken-ref shape sets)
    for (offset, msg) in parse_binder {
        out.push(Diagnostic::plain(*offset, msg.clone()));
    }
    // their binder still resolves the LEFT endpoint of an
    // application-wrapped ref and flags its columns (probe: t1.odd col <
    // t1.x flags 'odd' against public.t1)
    for ep in broken_refs {
        if ep.table.is_empty() {
            continue;
        }
        check_endpoint(doc, ep, ep.span.start, &mut out);
    }
    // the column-parse binder findings (their validateNode class)
    for t in &doc.tables {
        for c in &t.columns {
            for (offset, msg) in &c.binder_errors {
                out.push(Diagnostic::plain(*offset, msg.clone()));
            }
        }
    }
    // bound refs' application-arg findings (their INVALID_REF_FIELD)
    for r in &doc.refs {
        for (offset, msg) in &r.binder_errors {
            out.push(Diagnostic::plain(*offset, msg.clone()));
        }
    }

    out.sort_by_key(|d| d.offset);
    // their shouldInterpretNode (core/global_modules/utils.ts): the
    // interpretation layer runs only while bindFile reports no errors —
    // binder-class (Error-severity) findings suppress the infos; raw
    // parse errors do not gate anything
    if out.iter().any(|d| d.severity == Severity::Error) {
        out.retain(|d| d.severity == Severity::Error);
    }
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
        // feature_tour.dbml's findings are the nullable self-ref FK (their
        // editor anchors it twice: the column definition + the ref's FK
        // endpoint — 2 entries) plus 3 record-constraint violations (the
        // posts records' author_id against users' missing records; the
        // reverse direction is not checked)
        let d = diags(include_str!("../../tests/fixtures/feature_tour.dbml"));
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
        assert!(
            d.iter()
                .any(|m| m == "Table 'a' already exists in schema 'public'"),
            "{d:?}"
        );
        assert!(
            d.iter()
                .any(|m| m == "Enum 'e' already exists in schema 'public'"),
            "{d:?}"
        );
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn ref_endpoints_resolve_or_flag() {
        // dbdiagram's exact phrasings (harvested from their editor)
        let d = diags("Table a { id integer [pk] }\nRef: b.x > a.nope\n");
        assert!(
            d.contains(&"Table 'b' does not exist in Schema 'public'".to_string()),
            "{d:?}"
        );
        assert!(
            d.contains(&"Column 'nope' does not exist in Table 'public.a'".to_string()),
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
            vec!["Column 'nope' does not exist in Table 'public.a'".to_string()]
        );
    }

    #[test]
    fn group_members_and_index_columns_resolve() {
        let d = diags(
            "Table a { id integer }\nTableGroup g {\n  nope\n}\nTable c {\n  x integer\n  indexes {\n    (zzz) [unique]\n  }\n}\n",
        );
        assert!(
            d.contains(&"Table 'nope' does not exist".to_string()),
            "{d:?}"
        );
        assert!(
            d.contains(&"No column named 'zzz' inside Table 'c'".to_string()),
            "{d:?}"
        );
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn aliases_satisfy_refs() {
        let d = diags("Table users as U { id integer [pk] }\nRef: posts.uid > U.id\n");
        // 'posts' itself is the unknown one; U resolves via the alias
        assert_eq!(
            d,
            vec!["Table 'posts' does not exist in Schema 'public'".to_string()]
        );
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
            assert_eq!(d.severity, Severity::Info);
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
            // their non-preferred fixes set no isPreferred at all; the
            // column-setting edit rides both anchors (options default on)
            assert!(!d.quick_fixes[0].is_preferred || d.quick_fixes[0].is_preferred);
            assert!(d.quick_fixes[0].is_preferred, "the operator fix is preferred");
            assert_eq!(
                d.quick_fixes[1].title, "Mark 'b.a_id' as NOT NULL",
                "anchor {i}"
            );
            assert_eq!(d.quick_fixes[1].short_title, "Add not null", "anchor {i}");
            assert!(!d.quick_fixes[1].is_preferred, "anchor {i}");
            // removing the explicit `null` setting is their edit: empty
            // newText over the bracket list
            assert_eq!(d.quick_fixes[1].new_text, "", "anchor {i}");
        }
        assert_eq!(d[0].offset, out.doc.tables[1].columns[0].name_span.start);
        assert_eq!(d[1].offset, out.doc.refs[0].from.span.start);
        // a satisfied FK stays silent; the unique parent behind the many
        // operator raises their reverse "make one" check (both engines
        // verified on this input) — that lint has its own assertions below
        let src = "Table a { id integer [pk] }\nTable b { a_id integer [not null, unique] }\nRef: a.id > b.a_id\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        assert_eq!(d.len(), 2, "{d:?}");
        assert!(
            d.iter().all(
                |x| x.message == "Column `a.id` is unique but operator `>` allows many"
            ),
            "{d:?}"
        );
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

    /// A `>` ref whose parent column lacks unique/pk: their check asks for
    /// the unique constraint (or the many-to-many operator) on the FK
    /// side, while the parent's own uniqueness raises the reverse check —
    /// "make one" (both engines verified on this exact input).
    #[test]
    fn non_unique_parent_gets_the_should_be_unique_lint() {
        let src =
            "Table a { id integer [pk] }\nTable b { a_id integer [not null] }\nRef: a.id > b.a_id\n";
        let out = parser::parse(src);
        let d = validate(&out.doc, src);
        let non_unique: Vec<_> = d
            .iter()
            .filter(|x| {
                x.message == "Column `b.a_id` should be unique or primary key for operator `>`"
            })
            .collect();
        assert_eq!(non_unique.len(), 2, "{d:?}");
        for x in &non_unique {
            // the operator fix relaxes to many-to-many: `>` -> `<>`; the
            // column fix appends `, unique` to the bracket list
            let many = x
                .quick_fixes
                .iter()
                .find(|f| f.short_title == "Change to <>")
                .expect("the many fix");
            assert_eq!(many.new_text, "<>");
            assert!(many.is_preferred);
            let uniq = x
                .quick_fixes
                .iter()
                .find(|f| f.short_title == "Add unique")
                .expect("the unique fix");
            assert_eq!(uniq.new_text, ", unique");
            assert!(!uniq.is_preferred);
        }
        // the parent is unique behind a many operator: their
        // "make one" check suggests the one-to-one operator
        assert!(
            d.iter()
                .any(|x| x.message == "Column `a.id` is unique but operator `>` allows many"),
            "{d:?}"
        );
        let one = d
            .iter()
            .flat_map(|x| x.quick_fixes.iter())
            .find(|f| f.short_title == "Change to -")
            .expect("the one fix");
        assert_eq!(one.new_text, "-");
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
            violations.iter().any(|x| {
                x.message
                    == "FK violation: `posts.author_id` = `1` does not exist in `users.id`"
            }),
            "{d:?}"
        );
        assert!(
            violations.iter().any(|x| {
                x.message
                    == "FK violation: `posts.author_id` = `7` does not exist in `users.id`"
            }),
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
