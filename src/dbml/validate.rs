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
    /// The finding's token end (their endColumn) when the offending node
    /// spans past the anchor — the squiggle covers the node, not just
    /// its first char.
    pub end: Option<usize>,
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
    pub fn spanned(offset: usize, end: usize, message: String) -> Self {
        let mut d = Self::plain(offset, message);
        if end > offset {
            d.end = Some(end);
        }
        d
    }

    pub fn plain(offset: usize, message: String) -> Self {
        Self {
            offset,
            end: None,
            message,
            severity: Severity::Error,
            category: None,
            explanation: Vec::new(),
            quick_fixes: Vec::new(),
        }
    }

    /// The full-shape constructor for the info-class findings (the
    /// record-constraint infos carry a category, an explanation and the
    /// VALUE token's own span).
    pub fn detail(
        offset: usize,
        end: usize,
        message: String,
        severity: Severity,
        category: Option<String>,
        explanation: Vec<String>,
    ) -> Self {
        let mut d = Self {
            offset,
            end: None,
            message,
            severity,
            category,
            explanation,
            quick_fixes: Vec::new(),
        };
        if end > offset {
            d.end = Some(end);
        }
        d
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

/// Their column-settings rules (tablePartial/validate.ts), operating on a
/// single column's setting occurrences: once-only names, the pk/primary-key
/// and not-null/null conflicts (each error anchored at EVERY offending
/// occurrence), and the flag/value rules. Returns `(start, end, message)`
/// triples — `end: None` for the token-anchored (plain) findings, `Some`
/// for the node-spanned ones. Shared by the model validate path and the
/// parser's sentinel path, where a `}` aborts the list and the partial
/// attributes still validate (their validator walks the partial nodes —
/// probes c25h/c25j).
pub(crate) fn column_setting_errors(
    occs: &[super::ast::SettingOcc],
) -> Vec<(usize, Option<usize>, String)> {
    let mut out = Vec::new();
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
                out.push((
                    a.offset,
                    Some(a.end),
                    format!("'{name}' can only appear once"),
                ));
            }
        }
    }
    let pk = attrs_of("pk");
    let pkey = attrs_of("primary key");
    if !pk.is_empty() && !pkey.is_empty() {
        for a in pk.iter().chain(pkey.iter()) {
            out.push((
                a.offset,
                None,
                "Either one of 'primary key' and 'pk' can appear".to_string(),
            ));
        }
    }
    let nn = attrs_of("not null");
    let nu = attrs_of("null");
    if !nn.is_empty() && !nu.is_empty() {
        for a in nn.iter().chain(nu.iter()) {
            // their anchor = the setting entry's token span
            out.push((
                a.offset,
                Some(a.end),
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
            "default" => {
                // their checkDefault (isValidDefaultValue): a quoted
                // string, a signed number, true/false/null, a backtick
                // function expression, or a 2-3 fragment enum reference;
                // the anchor = the VALUE node ?? the NAME (a1: the
                // value-less default errs at the name; a3/a4: the parsed
                // invalid value errs at the value node). The
                // partial-attribute semantics: a dispatched-broken
                // default (probe e21: `[default: 'x\n}`) never reaches
                // their check — the broken flag skips our swallow-route
                // occurrences
                if a.broken {
                    continue;
                }
                let (vs, ve) = if a.has_value && a.value_span.start != a.value_span.end {
                    (a.value_span.start, Some(a.value_span.end))
                } else {
                    (a.offset, Some(a.end))
                };
                let v = a.value.as_deref().unwrap_or("").trim();
                let frags: Vec<&str> = v.split('.').collect();
                let valid = a.quoted
                    || v.starts_with('`')
                    || v == "true"
                    || v == "false"
                    || v == "null"
                    || v.parse::<f64>().is_ok()
                    || ((frags.len() == 2 || frags.len() == 3)
                        && frags.iter().all(|f| !f.is_empty()));
                if !valid {
                    out.push((vs, ve, "'default' must be an enum value, a string literal, number literal, function expression, true, false or null".to_string()));
                }
            }
            "pk" | "primary key" | "not null" | "null" | "unique" | "increment" => {
                if a.has_value {
                    // their anchor = the VALUE expression node (probe
                    // m43_0155: `not null: parent.id` anchors parent.id);
                    // a value node that never parsed still rides the key
                    let (vs, ve) = if a.value_span.start != a.value_span.end {
                        (a.value_span.start, Some(a.value_span.end))
                    } else {
                        (a.offset, Some(a.end))
                    };
                    out.push((vs, ve, format!("'{name}' must not have a value")));
                }
            }
            "note" => {
                if a.has_value && !a.quoted {
                    // their anchor = the VALUE expression node
                    out.push((
                        a.value_span.start,
                        Some(a.value_span.end),
                        "'note' must be a quoted string".to_string(),
                    ));
                }
            }
            _ => {
                // their custom-setting check (c21/c23/c17): a key
                // outside the known set must carry a string, color,
                // number, or boolean value — the anchor is the key
                // node when valueless, the value node when valued
                const KNOWN: [&str; 14] = [
                    "pk",
                    "primary key",
                    "primary_key",
                    "increment",
                    "autoincrement",
                    "auto_increment",
                    "unique",
                    "not null",
                    "null",
                    "note",
                    "default",
                    "check",
                    "ref",
                    "color",
                ];
                if KNOWN.contains(&name) {
                    // handled above or a known key: nothing
                } else {
                    let value_ok = a.has_value && {
                        let v = a.value.as_deref().unwrap_or("");
                        a.quoted
                            || v.starts_with('#')
                            || v.parse::<f64>().is_ok()
                            || matches!(v.to_ascii_lowercase().as_str(), "true" | "false")
                    };
                    if !value_ok {
                        let (s, e) = if a.has_value {
                            (a.value_span.start, a.value_span.end)
                        } else {
                            (a.offset, a.end)
                        };
                        out.push((
                            s,
                            Some(e),
                            format!(
                                "Custom setting '{name}' must be a string, a color \
                                 literal, a number literal, or a boolean literal"
                            ),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// Validate a parsed document: duplicate definition names, refs and
/// TableGroup members pointing at tables or columns that do not exist,
/// index columns missing from their table, the nullable-FK constraint
/// mismatch, and bare-comma index settings. `src` feeds the explanation
/// bullets their raw source-line quotes (the ref statement and the column
/// definition, verbatim).
pub fn validate(doc: &Document, src: &str) -> Vec<Diagnostic> {
    validate_with_parse(doc, src, &[], &[], &[], &[])
}

/// The parse-level binder findings join the same gate (their errors()
/// chains parse + binder reports).
pub fn validate_with_parse(
    doc: &Document,
    src: &str,
    parse_binder: &[(usize, usize, String)],
    binder_chains: &[super::ast::BinderChain],
    record_count_errors: &[(usize, usize, usize, usize)],
    parse_errors: &[super::parser::ParseError],
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
            // token span (probe: three multi-line dups flag each later name)
            out.push(Diagnostic::spanned(
                t.name_span.start,
                t.name_span.end,
                format!("Table '{}' already exists in schema 'public'", t.name),
            ));
        }
        tables.push(t.name.as_str());
    }
    let mut enums: Vec<&str> = Vec::new();
    for e in &doc.enums {
        if enums.contains(&e.name.as_str()) {
            // their dup marker anchors the duplicate declaration's NAME
            // token (their schema dup map: errorNode = declaration.name —
            // the same rule as the table dup above; corpus dup-enum
            // 2,6-2,7)
            out.push(Diagnostic::spanned(
                e.name_span.start,
                e.name_span.end,
                format!("Enum '{}' already exists in schema 'public'", e.name),
            ));
        }
        enums.push(e.name.as_str());
    }
    // duplicate column names (their table binder, table/index.ts: the
    // seen-map flags EVERY repeat and re-flags the first sighting — 3
    // twins yield 4 errors; anchors are the column name nodes). Their
    // guard: a column whose NAME symbol is undefined (our parser's
    // empty-name recovery for junk fields) skips the machinery entirely
    for t in &doc.tables {
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, c) in t.columns.iter().enumerate() {
            if c.name.is_empty() {
                continue;
            }
            if let Some(&first) = seen.get(c.name.as_str()) {
                let msg = format!("Duplicate column '{}'", c.name);
                // their anchor = the column's full node (name through the
                // settings list — fuzz-span parity)
                // their anchor = the column NODE (the callee .. the last
                // arg) — a type-less column's node is just the name token
                out.push(Diagnostic::spanned(
                    t.columns[first].name_span.start,
                    t.columns[first].full_end,
                    msg.clone(),
                ));
                out.push(Diagnostic::spanned(c.name_span.start, c.full_end, msg));
            } else {
                seen.insert(c.name.as_str(), i);
            }
        }
    }

    // their enum binder's seen-map flags EVERY repeat and re-flags the
    // first sighting — 3 twins yield 4 errors (the same rule as the
    // duplicate columns above; probe p3). The anchor = the field NODE:
    // name .. the settings end (probe p1: `x [note: 'n']` spans the
    // whole field), never a point
    for e in &doc.enums {
        let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for (i, v) in e.values.iter().enumerate() {
            if v.name.is_empty() {
                continue;
            }
            if let Some(&first) = seen.get(v.name.as_str()) {
                let msg = format!("Duplicate enum field '{}'", v.name);
                out.push(Diagnostic::spanned(
                    e.values[first].name_span.start,
                    e.values[first].full_end,
                    msg.clone(),
                ));
                out.push(Diagnostic::spanned(v.name_span.start, v.full_end, msg));
            } else {
                seen.insert(v.name.as_str(), i);
            }
        }
    }

    // their table/tableGroup validateSubElements (table/validate.ts:167
    // + tableGroup/validate.ts:131, NOTE_REDEFINED 3046): more than one
    // Note-kind sub-element in the body -> EVERY one errs, anchored at
    // its element span. The colon entries and the block declarations
    // BOTH count (probes zc4/zc5 — the handoff's block-only note was
    // wrong); the TableGroup collection covers the colon form (n4; the
    // group's block-note arm = a parse-side gap, recorded).
    for t in &doc.tables {
        if t.note_element_spans.len() > 1 {
            for sp in &t.note_element_spans {
                out.push(Diagnostic::spanned(
                    sp.start,
                    sp.end,
                    "Duplicate notes are defined".to_string(),
                ));
            }
        }
    }
    for g in &doc.table_groups {
        if g.note_element_spans.len() > 1 {
            for sp in &g.note_element_spans {
                out.push(Diagnostic::spanned(
                    sp.start,
                    sp.end,
                    "Duplicate notes are defined".to_string(),
                ));
            }
        }
    }

    // their schema-member dup machinery covers TableGroups and top-level
    // sticky Notes too (the anchor = the declaration's NAME token — the
    // schema dup map's errorNode = declaration.name; each LATER
    // declaration flags, the tables/enums rule), plus the group-field
    // checks: the within-group seen-map (first re-flagged per repeat —
    // the enum-field rule; the fullname = the WRITTEN member under the
    // default schema — probes g15/g16) and the membership map
    // (program/bind.ts: the RESOLVED table claimed by its first group;
    // every later claimant errs, the same group's repeats included —
    // g13; unresolved members skip the machinery — g16). The group/note
    // ASTs carry no name/member spans, so the tokens are re-scanned from
    // the source: their name-form = one identifier or quoted identifier,
    // and a bound member = a pure identifier line (comma-runs and
    // continuations bind nothing — probes g5/g9/g10).
    fn scan_name_token(src: &str, elem_start: usize) -> Option<Span> {
        let text = src.get(elem_start..)?;
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        if i >= bytes.len() {
            return None;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let q = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != q {
                i += 1;
            }
            if i < bytes.len() {
                i += 1;
            }
            return Some(elem_start + start..elem_start + i);
        }
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] >= 0x80)
        {
            i += 1;
        }
        if i == start {
            return None;
        }
        Some(elem_start + start..elem_start + i)
    }

    fn scan_group_members(src: &str, gspan: &Span) -> Vec<(String, Span)> {
        let mut out = Vec::new();
        let Some(text) = src.get(gspan.start..gspan.end) else {
            return out;
        };
        let Some(open_rel) = text.find('{') else {
            return out;
        };
        let mut depth = 0i32;
        let mut end_rel = text.len();
        for (i, &b) in text.as_bytes().iter().enumerate().skip(open_rel) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end_rel = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        let ident = |s: &str| {
            s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
                && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        };
        let mut off = open_rel + 1;
        for line in text[open_rel + 1..end_rel].split('\n') {
            let line_start = off;
            off += line.len() + 1;
            let content = match line.find("//") {
                Some(k) => &line[..k],
                None => line,
            };
            // the block's own close may share the member's line (the
            // inline form) — it rides outside the member
            let content = match content.find('}') {
                Some(k) => &content[..k],
                None => content,
            };
            let t = content.trim();
            if ident(t) {
                let lead = content.len() - content.trim_start().len();
                let s = gspan.start + line_start + lead;
                out.push((t.to_string(), s..s + t.len()));
            }
        }
        out
    }

    let mut group_names: Vec<&str> = Vec::new();
    for g in &doc.table_groups {
        if group_names.contains(&g.name.as_str()) {
            if let Some(sp) = scan_name_token(src, g.span.start) {
                out.push(Diagnostic::spanned(
                    sp.start,
                    sp.end,
                    format!(
                        "TableGroup '{}' already exists in schema 'public'",
                        g.name
                    ),
                ));
            }
        }
        group_names.push(&g.name);
    }
    let mut note_names: Vec<&str> = Vec::new();
    for n in &doc.notes {
        if note_names.contains(&n.name.as_str()) {
            if let Some(sp) = scan_name_token(src, n.span.start) {
                out.push(Diagnostic::spanned(
                    sp.start,
                    sp.end,
                    format!("Duplicate Note '{}' in schema 'public'", n.name),
                ));
            }
        }
        note_names.push(&n.name);
    }
    let mut field_dups: Vec<(usize, usize, String)> = Vec::new();
    let mut reappears: Vec<(usize, usize, String)> = Vec::new();
    let mut member_owner: std::collections::HashMap<usize, &str> =
        std::collections::HashMap::new();
    for g in &doc.table_groups {
        let members = scan_group_members(src, &g.span);
        let mut seen: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, (name, sp)) in members.iter().enumerate() {
            if let Some(&first) = seen.get(name.as_str()) {
                let msg = format!("Duplicate TableGroupField 'public.{name}'");
                field_dups.push((members[first].1.start, members[first].1.end, msg.clone()));
                field_dups.push((sp.start, sp.end, msg));
            } else {
                seen.insert(name.as_str(), i);
            }
            if let Some(t) = doc.tables.iter().find(|t| t.matches(name)) {
                let ti = doc.tables.iter().position(|x| std::ptr::eq(x, t)).unwrap_or(0);
                if let Some(owner) = member_owner.get(&ti) {
                    reappears.push((
                        sp.start,
                        sp.end,
                        format!("Table \"{name}\" already appears in group \"{owner}\""),
                    ));
                } else {
                    member_owner.insert(ti, g.name.as_str());
                }
            }
        }
    }
    for (s, e, msg) in field_dups {
        out.push(Diagnostic::spanned(s, e, msg));
    }
    for (s, e, msg) in reappears {
        out.push(Diagnostic::spanned(s, e, msg));
    }

    // every ref endpoint must resolve to a real table and column (block
    // refs and inline `ref:` settings alike)
    fn check_endpoint(doc: &Document, ep: &super::ast::Endpoint, anchor: usize, out: &mut Vec<Diagnostic>) {
        // recovered endpoints can carry an empty span — anchor those on
        // the caller-provided position instead of offset 0. Their anchors
        // carry the FRAGMENT tokens: the missing table anchors the table
        // token, the missing column the column token.
        let frag = |sp: &Span, fallback: usize| -> Span {
            if sp.is_empty() {
                fallback..fallback
            } else {
                sp.clone()
            }
        };
        let table_frag = frag(&ep.table_span, anchor);
        match doc.tables.iter().find(|t| t.matches(&ep.table)) {
            None => out.push(Diagnostic::spanned(
                table_frag.start,
                table_frag.end,
                format!(
                    "Table '{}' does not exist in Schema 'public'",
                    ep.table
                ),
            )),
            Some(t) => {
                let schema = t.schema.as_deref().unwrap_or("public");
                for (i, col) in ep.columns.iter().enumerate() {
                    if !t.columns.iter().any(|c| &c.name == col) {
                        let csp = ep
                            .column_spans
                            .get(i)
                            .cloned()
                            .unwrap_or_else(|| ep.span.clone());
                        let cfrag = frag(&csp, anchor);
                        out.push(Diagnostic::spanned(
                            cfrag.start,
                            cfrag.end,
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
        if !r.clean_binary {
            // their checkStandaloneRefs skips refs whose tree callee is
            // not a clean binary relationship; the binder chains carry
            // the endpoint checks instead, and the interpret-layer
            // checks are gated off by the file's errors
            continue;
        }
        // the endpoint column checks run on BOTH sides regardless of the
        // tuple lengths (probe g5: a.(id, code) > b.x errs with Unequal
        // AND the missing 'code'; the earlier mc3 probe stayed silent
        // only because its 'nope' existed)
        check_endpoint(doc, &r.from, r.span.start, &mut out);
        check_endpoint(doc, &r.to, r.span.start, &mut out);
        // their isEqualTupleOperands: composite endpoints must carry the
        // same field count (probe: `a.(id, code) > b.x` errs at the field)
        if r.from.columns.len() != r.to.columns.len() {
            out.push(Diagnostic::plain(
                r.from.span.start,
                "Unequal fields in ref endpoints".to_string(),
            ));
        }
    }
    // records headers: every named column must exist (probe:
    // records (nope) errs with the endpoint phrasing); the anchor is
    // the header column itself — their binder errs at the
    // `columnBindee` (records/bind.ts), not the table declaration
    for t in &doc.tables {
        if let Some(r) = &t.records {
            for (i, cname) in r.columns.iter().enumerate() {
                if !t.columns.iter().any(|x| x.name == *cname) {
                    // their records check does NOT schema-qualify (probe:
                    // records (nope) errs against Table 'T')
                    let at = r
                        .column_spans
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| t.name_span.start..t.name_span.end);
                    out.push(Diagnostic::spanned(
                        at.start,
                        at.end,
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
                    // (schema-qualified message, like every endpoint check);
                    // the anchor = the COLUMN TOKEN (the hard-won rule —
                    // probe m43_0193: `poref` spans 44-49, not a point)
                    if !t.columns.iter().any(|x| x.name == ir.to.columns[0]) {
                        let schema = t.schema.as_deref().unwrap_or("public");
                        let frag = ir
                            .to
                            .column_spans
                            .first()
                            .cloned()
                            .unwrap_or(ir.to.span.clone());
                        out.push(Diagnostic::spanned(
                            frag.start,
                            frag.end,
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
            for (offset, end, message) in column_setting_errors(occs) {
                match end {
                    Some(end) => out.push(Diagnostic::spanned(offset, end, message)),
                    None => out.push(Diagnostic::plain(offset, message)),
                }
            }
            // their enum-reference default BINDS: the fragments resolve
            // schema → enum → field (probes d1-d5: the missing schema
            // errs at the FIRST fragment, the missing enum at the first,
            // the missing field at the second; the valid reference is
            // silent)
            for a in occs {
                if a.key != "default" || a.broken {
                    continue;
                }
                let v = a.value.as_deref().unwrap_or("").trim();
                let frags: Vec<&str> = v.split('.').collect();
                if a.quoted
                    || v.starts_with('`')
                    || !(frags.len() == 2 || frags.len() == 3)
                    || frags.iter().any(|f| f.is_empty())
                {
                    continue;
                }
                if v.parse::<f64>().is_ok()
                    || v == "true"
                    || v == "false"
                    || v == "null"
                {
                    continue;
                }
                let fspans: Vec<(usize, usize)> = {
                    let mut acc = a.value_span.start;
                    let mut v2 = vec![];
                    for f in &frags {
                        v2.push((acc, acc + f.len()));
                        acc += f.len() + 1;
                    }
                    v2
                };
                let default_schema = "public";
                if frags.len() == 3 {
                    let schema_exists = frags[0] == default_schema
                        || doc.enums.iter().any(|e| e.schema.as_deref() == Some(frags[0]))
                        || doc.tables.iter().any(|t| t.schema.as_deref() == Some(frags[0]));
                    if !schema_exists {
                        let (fs, fe) = fspans[0];
                        out.push(Diagnostic::spanned(fs, fe, format!("Schema '{}' does not exist in Schema '{}'", frags[0], default_schema)));
                    }
                } else {
                    let enm = doc.enums.iter().find(|e| {
                        e.name == frags[0]
                            && (e.schema.is_none() || e.schema.as_deref() == Some("public"))
                    });
                    match enm {
                        None => {
                            let (fs, fe) = fspans[0];
                            out.push(Diagnostic::spanned(fs, fe, format!("Enum '{}' does not exist in Schema '{}'", frags[0], default_schema)));
                        }
                        Some(e) => {
                            if !e.values.iter().any(|v| v.name == frags[1]) {
                                let (fs, fe) = fspans[1];
                                let schema = e.schema.as_deref().unwrap_or("public");
                                out.push(Diagnostic::spanned(fs, fe, format!("Enum field '{}' does not exist in Enum '{}.{}'", frags[1], schema, e.name)));
                            }
                        }
                    }
                }
            }
        }
    }

    // their checkRefEndpoints (program/bind.ts): a ref whose two sides
    // resolve to the SAME SYMBOL SET errs once ('Two endpoints are the
    // same') and drops out of the pair map; the same unordered PAIR across
    // refs flags both declarations ('References with same endpoints
    // exist'). Side identity = the RESOLVED column symbols (their interned
    // symbols; duplicates resolve to the last declaration — probes:
    // the self-ref on the first of three duplicate x columns binds two
    // different symbols and stays silent, while the self-ref on the last
    // duplicate fires); inline refs join the same map.
    let side_key = |ep: &super::ast::Endpoint| -> Option<String> {
        let t = doc.tables.iter().find(|t| t.matches(&ep.table))?;
        let mut ids: Vec<usize> = Vec::new();
        for col in &ep.columns {
            // an unresolved column keeps the ref out of the pair machinery
            // entirely (probe: two refs sharing only missing endpoints
            // never pair)
            match t.columns.iter().rev().find(|c| c.name == *col) {
                Some(c) => ids.push(c.name_span.start),
                None => return None,
            }
        }
        ids.sort_unstable();
        Some(format!(
            "{}:{}",
            doc.tables.iter().position(|t2| std::ptr::eq(t2, t))?,
            ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        ))
    };
    let mut pair_first: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new(); // pair key -> first sighting's anchor
    let mut endpoint_errors: Vec<(usize, Option<usize>, String)> = Vec::new();
    {
        let mut run = |host_key: Option<String>,
                        other_key: Option<String>,
                        anchor: usize,
                        end: Option<usize>,
                        emissions: &mut Vec<(usize, Option<usize>, String)>| {
            let (Some(l), Some(rt)) = (host_key, other_key) else {
                return;
            };
            if l == rt {
                emissions.push((anchor, end, "Two endpoints are the same".to_string()));
                return;
            }
            let key = if l < rt { format!("{l}-{rt}") } else { format!("{rt}-{l}") };
            if let Some(&first_anchor) = pair_first.get(&key) {
                emissions.push((
                    first_anchor,
                    None,
                    "References with same endpoints exist".to_string(),
                ));
                emissions.push((anchor, None, "References with same endpoints exist".to_string()));
            } else {
                pair_first.insert(key, anchor);
            }
        };
        for r in &doc.refs {
            if !r.clean_binary {
                continue;
            }
            // their anchor = the whole ref NODE span
            run(
                side_key(&r.from),
                side_key(&r.to),
                r.span.start,
                Some(r.span.end),
                &mut endpoint_errors,
            );
        }
        for t in &doc.tables {
            for c in &t.columns {
                let Some(ir) = &c.settings.inline_ref else { continue };
                let ti = doc.tables.iter().position(|x| std::ptr::eq(x, t));
                // the host side's symbol = the declaring column itself;
                // the target side resolves through the symbol table
                // (last declaration wins)
                let host_key = ti.map(|tidx| format!("{tidx}:{}", c.name_span.start));
                let other_key = if ir.to.table.is_empty() {
                    // bare-word endpoint: the host table's column
                    t.columns
                        .iter()
                        .rev()
                        .find(|x| x.name == ir.to.columns[0])
                        .map(|resolved| format!("{}:{}", ti.unwrap_or(0), resolved.name_span.start))
                } else {
                    side_key(&ir.to)
                };
                run(
                    host_key,
                    other_key,
                    ir.attr_start,
                    Some(ir.to.span.end),
                    &mut endpoint_errors,
                );
            }
        }
    }
    for (anchor, end, msg) in endpoint_errors {
        out.push(Diagnostic::spanned(anchor, end.unwrap_or(anchor), msg));
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
                    let at = ix
                        .column_spans
                        .get(ci)
                        .cloned()
                        .unwrap_or_else(|| ix.span.start..ix.span.start);
                    out.push(Diagnostic::spanned(
                        at.start,
                        at.end,
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
            end: None,
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
        // their symbol table: later declarations overwrite earlier ones —
        // a duplicate name resolves to the LAST declaration (probe: the
        // self-inline ref on the first of three duplicate x columns binds
        // to two different symbols, so no same-endpoint finding)
        doc.tables
            .iter()
            .find(|t| t.matches(table))
            .and_then(|t| t.columns.iter().rev().find(|c| c.name == col))
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
        // their addSettingEdit's fallback anchors declaration.end — the
        // node's union end (full_end), not the consumption end
        let node_span = c.span.start..c.full_end;
        let (span, new_text) = match remove_setting_edit(&c.span, "null") {
            Some(edit) => edit,
            None => add_setting_edit(&node_span, "not null"),
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
        let node_span = c.span.start..c.full_end;
        let (span, new_text) = add_setting_edit(&node_span, "unique");
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
        /// their lint anchors carry the endpoint/column NODE span
        token_end: usize,
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
        let mk = |offset: usize, end: usize| Diagnostic {
            // exact: their node spans are non-degenerate on these paths
            end: Some(end),
            offset,
            message: message.to_string(),
            severity: Severity::Info,
            category: Some("Mismatched optional ref constraints".to_string()),
            explanation: explanation.clone(),
            quick_fixes: fixes.clone(),
        };
        out.push(mk(node, s.token_end));
        for c in s.cols {
            if let Some(col) = column_of(s.table, c) {
                if col.name_span.start != node {
                    // their anchor = the resolved COLUMN node — the union of
                    // its args (name .. settings end), not name..type-end
                    out.push(mk(col.name_span.start, col.full_end));
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
        if !r.clean_binary {
            continue;
        }
        let op = written_op(r.card, r.left_optional, r.right_optional);
        let sides = RefSides {
            left: Side {
                table: &r.from.table,
                cols: &r.from.columns,
                token: r.from.span.start,
                token_end: r.from.span.end,
            },
            right: Side {
                table: &r.to.table,
                cols: &r.to.columns,
                token: r.to.span.start,
                token_end: r.to.span.end,
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
                    // their lint anchors the resolved COLUMN NODE (name ..
                    // the settings end, full_end) — the hard-won rule;
                    // probe p2: `a_id integer [ref: > a.id]` anchors
                    // 3,3-3,29 (through the `]`), not the type end
                    token_end: c.full_end,
                },
                right: Side {
                    table: if host_of_bare { &t.name } else { &ir.to.table },
                    cols: &ir.to.columns,
                    token: ir.attr_start,
                    token_end: ir.to.span.end,
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
    // their formatValues (records/utils/constraints/helper.ts) renders
    // the row value with JSON.stringify — strings carry double quotes,
    // numbers normalize through the parsed form (2.50 -> 2.5)
    let json_quote = |t: &str| -> String {
        format!("\"{}\"", t.replace('\\', "\\\\").replace('"', "\\\""))
    };
    let json_label = |v: &super::ast::RecordValue| -> String {
        match v {
            super::ast::RecordValue::Null => "null".to_string(),
            super::ast::RecordValue::Bool(b) => b.to_string(),
            super::ast::RecordValue::Number(n) => n
                .parse::<f64>()
                .map(|f| format!("{f}"))
                .unwrap_or_else(|_| n.clone()),
            super::ast::RecordValue::Text(t) => json_quote(t),
            super::ast::RecordValue::Ident(i) => json_quote(i),
            super::ast::RecordValue::Expr(e) => json_quote(e),
        }
    };
    let recorded_values = |rec: &super::ast::TableRecords,
                           idx: usize|
     -> Vec<(String, Span, String)> {
        rec.rows
            .iter()
            .zip(rec.value_spans.iter())
            .filter_map(|(row, spans)| {
                row.get(idx).zip(spans.get(idx)).map(|(v, s)| {
                    (v.label(), s.clone(), json_label(v))
                })
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
                for (v, span, json) in &fk_vals {
                    // their null handling: an empty or literal-null cell on
                    // a required FK column emits BOTH the NULL-not-allowed
                    // note and the must-not-be-NULL violation (probe:
                    // records-null-in-required)
                    let is_null = v.is_empty() || v.eq_ignore_ascii_case("null");
                    // their message renders the empty string as `null`
                    let shown = if v.is_empty() { "null" } else { v.as_str() };
                    let _ = shown;
                    if is_null {
                        let required = parent_vals.len() == 0
                            || parent_vals.iter().any(|(pv, _, _)| pv.is_empty());
                        let _ = required;
                        let col_required = doc
                            .tables
                            .iter()
                            .find(|t| t.matches(fk_tbl))
                            .and_then(|t| t.columns.iter().find(|c| c.name == fk_col))
                            .and_then(|c| c.settings.not_null)
                            .unwrap_or(false);
                        if col_required {
                            // their anchor = the record VALUE's own token
                            // span (corpus records-null-in-required:
                            // `''` = 6,8-6,10, the quotes included)
                            out.push(Diagnostic::detail(
                                span.start,
                                span.end,
                                format!(
                                    "NULL not allowed for NOT NULL column `{fk_col}` without a default value or auto-increment"
                                ),
                                Severity::Info,
                                Some("Record type errors".to_string()),
                                vec![format!(
                                    "`{fk_col}` is marked NOT NULL and has no default value or auto-increment."
                                )],
                            ));
                        }
                        out.push(Diagnostic::detail(
                            span.start,
                            span.end,
                            format!(
                                "FK violation: `{fk_tbl}.{fk_col}` = `{shown}` must not be NULL"
                            ),
                            Severity::Info,
                            Some("Record constraint violations".to_string()),
                            vec![format!(
                                "`{fk_tbl}.{fk_col}` references `{parent_tbl}.{parent_col}`, so every row must have a non-null value."
                            )],
                        ));
                        continue;
                    }
                    if !parent_vals.iter().any(|(pv, _, _)| pv == v) {
                        out.push(Diagnostic::detail(
                            span.start,
                            span.end,
                            format!(
                                "FK violation: `{fk_tbl}.{fk_col}` = `{json}` does not exist in `{parent_tbl}.{parent_col}`"
                            ),
                            Severity::Info,
                            Some("Record constraint violations".to_string()),
                            vec![format!(
                                "`{fk_tbl}.{fk_col}` is a foreign key referencing `{parent_tbl}.{parent_col}`. Every non-null value in an FK must match an existing row in the referenced column."
                            )],
                        ));
                    }
                }
            }
        }
    };
    for r in &doc.refs {
        if !r.clean_binary {
            continue;
        }
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

    // ---- the records value-type checks (their records/interpret.ts
    // extractValue — the RecordValueTypeMismatch family). Their type
    // classifiers = the UNION over the five dialect sets
    // (records/utils/data/sqlTypes.ts); the dispatch order = the null-ish
    // short-circuit, enum, numeric, boolean, datetime, string, and
    // unknown types stay unchecked.
    fn norm_ty(ty: &str) -> String {
        ty.split('(')
            .next()
            .unwrap_or("")
            .to_lowercase()
            .trim()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
    fn type_args(ty: &str) -> Vec<String> {
        let inner = match ty.find('(') {
            Some(k) => match ty[k + 1..].find(')') {
                Some(e) => &ty[k + 1..k + 1 + e],
                None => return Vec::new(),
            },
            None => return Vec::new(),
        };
        inner
            .split(',')
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty())
            .collect()
    }
    const INTEGER_TYPES: &[&str] = &[
        "int",
        "integer",
        "smallint",
        "bigint",
        "tinyint",
        "mediumint",
        "serial",
        "bigserial",
        "smallserial",
    ];
    const FLOAT_TYPES: &[&str] = &[
        "decimal",
        "numeric",
        "float",
        "double",
        "real",
        "double precision",
        "number",
    ];
    const BOOL_TYPES: &[&str] = &["bool", "boolean", "bit"];
    const STRING_TYPES: &[&str] = &[
        "varchar",
        "char",
        "text",
        "tinytext",
        "mediumtext",
        "longtext",
        "string",
        "character",
        "character varying",
        "nvarchar",
        "nchar",
        "ntext",
        "varchar2",
        "nvarchar2",
    ];
    const DATETIME_TYPES: &[&str] = &[
        "date",
        "datetime",
        "timestamp",
        "time",
        "timestamptz",
        "timestamp with time zone",
        "timestamp without time zone",
        "timetz",
        "time with time zone",
        "time without time zone",
        "datetime2",
        "smalldatetime",
        "timestamp with local time zone",
    ];
    const SERIAL_TYPES: &[&str] = &["serial", "smallserial", "bigserial"];
    // their tryExtractDateTime (the luxon format list, pragmatic Rust):
    // the time-only forms, the date-only forms, the space/T datetime
    // forms, then the lax ISO fallback
    fn valid_zone(z: &str) -> bool {
        let b = z.as_bytes();
        b.len() == 5
            && b[2] == b':'
            && b[..2].iter().all(|c| c.is_ascii_digit())
            && b[3..].iter().all(|c| c.is_ascii_digit())
    }
    fn valid_time(s: &str) -> bool {
        let s = s.strip_suffix('Z').unwrap_or(s);
        let s = match s.char_indices().find(|(i, c)| *i >= 2 && (*c == '+' || *c == '-')) {
            Some((k, _)) if valid_zone(&s[k + 1..]) => &s[..k],
            _ => s,
        };
        let (hms, frac) = match s.split_once('.') {
            Some((a, b)) => (a, Some(b)),
            None => (s, None),
        };
        let parts: Vec<&str> = hms.split(':').collect();
        if parts.len() != 3 {
            return false;
        }
        let p2 = |p: &str| -> Option<u32> {
            let b = p.as_bytes();
            if b.len() == 2 && b.iter().all(|c| c.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        };
        let (Some(h), Some(m), Some(sec)) = (p2(parts[0]), p2(parts[1]), p2(parts[2])) else {
            return false;
        };
        if h > 23 || m > 59 || sec > 59 {
            return false;
        }
        frac.is_none_or(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_digit()))
    }
    fn days_in(month: usize, year: i64) -> usize {
        match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }
    fn valid_iso_date(s: &str) -> bool {
        let b = s.as_bytes();
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return false;
        }
        let (Ok(y), Ok(m), Ok(d)) = (s[0..4].parse::<i64>(), s[5..7].parse::<usize>(), s[8..10].parse::<usize>()) else {
            return false;
        };
        m >= 1 && m <= 12 && d >= 1 && d <= days_in(m, y)
    }
    fn valid_mdy(s: &str) -> bool {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 3 {
            return false;
        }
        let (Ok(m), Ok(d), Ok(y)) = (
            parts[0].parse::<usize>(),
            parts[1].parse::<usize>(),
            parts[2].parse::<i64>(),
        ) else {
            return false;
        };
        parts[0].len() <= 2
            && parts[1].len() <= 2
            && parts[2].len() == 4
            && m >= 1
            && m <= 12
            && d >= 1
            && d <= days_in(m, y)
    }
    const MONTHS: &[&str] = &[
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    fn valid_d_mmm_yyyy(s: &str) -> bool {
        let parts: Vec<&str> = s.split(' ').collect();
        if parts.len() != 3 {
            return false;
        }
        let (Ok(d), Ok(y)) = (parts[0].parse::<usize>(), parts[2].parse::<i64>()) else {
            return false;
        };
        let Some(m) = MONTHS.iter().position(|mm| mm.eq_ignore_ascii_case(parts[1])) else {
            return false;
        };
        parts[0].len() <= 2
            && parts[2].len() == 4
            && d >= 1
            && d <= days_in(m + 1, y)
    }
    fn valid_mmm_d_yyyy(s: &str) -> bool {
        let parts: Vec<&str> = s.split(' ').collect();
        if parts.len() != 3 {
            return false;
        }
        let Some(m) = MONTHS.iter().position(|mm| mm.eq_ignore_ascii_case(parts[0])) else {
            return false;
        };
        let day = parts[1].strip_suffix(',');
        let Some(Ok(d)) = day.map(|s| s.parse::<usize>()) else {
            return false;
        };
        let (Ok(y), true) = (parts[2].parse::<i64>(), parts[1].len() <= 3 && parts[2].len() == 4)
        else {
            return false;
        };
        d >= 1 && d <= days_in(m + 1, y)
    }
    fn valid_datetime(s: &str) -> bool {
        if valid_time(s) {
            return true;
        }
        if valid_iso_date(s) || valid_mdy(s) || valid_d_mmm_yyyy(s) || valid_mmm_d_yyyy(s) {
            return true;
        }
        // <date>(T| )<time> — the ISO-T form also accepts HH:mm (fromISO)
        for (di, dc) in s.char_indices() {
            if dc == 'T' || dc == ' ' {
                let (d, t) = (&s[..di], &s[di + 1..]);
                let date_ok =
                    valid_iso_date(d) || valid_mdy(d) || valid_d_mmm_yyyy(d) || valid_mmm_d_yyyy(d);
                if date_ok && (valid_time(t) || valid_iso_time_short(t)) {
                    return true;
                }
            }
        }
        false
    }
    fn valid_iso_time_short(s: &str) -> bool {
        // HH:mm (the fromISO lax time) — the zone rules as above
        let s = s.strip_suffix('Z').unwrap_or(s);
        let s = match s.char_indices().find(|(i, c)| *i >= 2 && (*c == '+' || *c == '-')) {
            Some((k, _)) if valid_zone(&s[k + 1..]) => &s[..k],
            _ => s,
        };
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return false;
        }
        let p2 = |p: &str| -> Option<u32> {
            let b = p.as_bytes();
            if b.len() == 2 && b.iter().all(|c| c.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        };
        match (p2(parts[0]), p2(parts[1])) {
            (Some(h), Some(m)) => h <= 23 && m <= 59,
            _ => false,
        }
    }
    // their extractValue — the (message, explanation) for a value that
    // fails its column's type, None when the value passes (or the type
    // is not one of the checked families)
    #[allow(clippy::too_many_arguments)]
    fn value_type_check(
        v: &super::ast::RecordValue,
        col_name: &str,
        tn: &str,
        targs: &[String],
        not_null: bool,
        has_default: bool,
        increment: bool,
        enum_members: Option<&[String]>,
    ) -> Option<(String, String)> {
        let is_string = STRING_TYPES.contains(&tn);
        let is_serial = SERIAL_TYPES.contains(&tn);
        let null_branch = || {
            (not_null && !has_default && !increment && !is_serial).then(|| (
                format!("NULL not allowed for NOT NULL column `{col_name}` without a default value or auto-increment"),
                format!("`{col_name}` is marked NOT NULL and has no default value or auto-increment."),
            ))
        };
        match v {
            // their FunctionExpressionNode: the backtick expression rides
            // unchecked
            super::ast::RecordValue::Expr(_) => None,
            super::ast::RecordValue::Null => null_branch(),
            super::ast::RecordValue::Text(t) if t.is_empty() && !is_string => null_branch(),
            _ if enum_members.is_some() => {
                // their enum branch: the quoted string or the bare
                // variable; anything else (a number, a bool) misses
                let val = match v {
                    super::ast::RecordValue::Text(t) => Some(t.clone()),
                    super::ast::RecordValue::Ident(i) => Some(i.clone()),
                    _ => None,
                };
                let ok = val.is_some_and(|val| {
                    enum_members.unwrap().iter().any(|m| *m == val)
                });
                (!ok).then(|| (
                    format!("Invalid enum value for column `{col_name}`"),
                    format!("`{col_name}` uses an enum type. The value must match one of the defined enum members."),
                ))
            }
            _ if INTEGER_TYPES.contains(&tn) || FLOAT_TYPES.contains(&tn) => {
                // their extractSignedNumber: the numeric literal (or a
                // signed prefix) — quoted strings and bare words fail
                let num = match v {
                    super::ast::RecordValue::Number(n) => n.parse::<f64>().ok(),
                    _ => None,
                };
                let Some(num) = num else {
                    return Some((
                        format!("Invalid numeric value for column `{col_name}`"),
                        format!("`{col_name}` has type `{tn}` which expects a numeric value. Provide a valid integer or decimal."),
                    ));
                };
                if INTEGER_TYPES.contains(&tn) && num.fract() != 0.0 {
                    // their message formats the parsed JS number (2.50 ->
                    // 2.5) — Rust's Display agrees for the common forms
                    return Some((
                        format!("Invalid integer value `{num}` for column `{col_name}`: expected integer, got decimal"),
                        format!("`{col_name}` is an integer column and cannot store decimal values."),
                    ));
                }
                if FLOAT_TYPES.contains(&tn) && !targs.is_empty() {
                    let precision: u64 = targs[0].parse().unwrap_or(u64::MAX);
                    let scale: u64 = targs.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    // their digit count runs on the number's toString
                    let num_str = format!("{num}");
                    let (ip, dp) = match num_str.split_once('.') {
                        Some((a, b)) => (a, b),
                        None => (num_str.as_str(), ""),
                    };
                    let ip = ip.trim_start_matches('-');
                    let total = ip.len() + dp.len();
                    if total as u64 > precision {
                        return Some((
                            format!("Numeric value `{num}` for column `{col_name}` exceeds precision: expected at most {precision} total digits, got {total}"),
                            format!("`{col_name}` is defined as `{tn}({},{})`, allowing at most {precision} total digits. The value `{num}` has {total}.", targs[0], targs.get(1).map(|s| s.as_str()).unwrap_or("0")),
                        ));
                    }
                    if dp.len() as u64 > scale {
                        return Some((
                            format!("Numeric value `{num}` for column `{col_name}` exceeds scale: expected at most {scale} decimal digits, got {}" , dp.len()),
                            format!("`{col_name}` is defined as `{tn}({},{})`, allowing at most {scale} decimal places. The value `{num}` has {}.", targs[0], targs.get(1).map(|s| s.as_str()).unwrap_or("0"), dp.len()),
                        ));
                    }
                }
                None
            }
            _ if BOOL_TYPES.contains(&tn) => {
                let ok = match v {
                    super::ast::RecordValue::Bool(b) => Some(*b),
                    super::ast::RecordValue::Number(n) => n.parse::<f64>().ok().and_then(|f| {
                        if f == 0.0 {
                            Some(false)
                        } else if f == 1.0 {
                            Some(true)
                        } else {
                            None
                        }
                    }),
                    super::ast::RecordValue::Ident(i) => match i.to_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    },
                    super::ast::RecordValue::Text(t) => match t.to_lowercase().as_str() {
                        "true" | "yes" | "y" | "t" | "1" => Some(true),
                        "false" | "no" | "n" | "f" | "0" => Some(false),
                        _ => None,
                    },
                    _ => None,
                };
                (!ok.is_some()).then(|| (
                    format!("Invalid boolean value for column `{col_name}`"),
                    format!("`{col_name}` is a boolean column. Use `true`/`false`, `1`/`0`, or `'yes'`/`'no'`."),
                ))
            }
            _ if DATETIME_TYPES.contains(&tn) => {
                // their tryExtractDateTime reads the QUOTED string only
                let ok = match v {
                    super::ast::RecordValue::Text(t) => valid_datetime(t),
                    _ => false,
                };
                (!ok).then(|| (
                    format!("Invalid datetime value for column `{col_name}`"),
                    format!("`{col_name}` expects a date or time value. Supported formats include `'YYYY-MM-DD'`, `'HH:MM:SS'`, `'YYYY-MM-DD HH:MM:SS'`, `'MM/DD/YYYY'`, `'D MMM YYYY'`, or `'MMM D, YYYY'`."),
                ))
            }
            _ if is_string => {
                // their tryExtractString: the quoted text, else the
                // number, else the boolean (the bare true/false pass)
                let sval = match v {
                    super::ast::RecordValue::Text(t) => Some(t.clone()),
                    super::ast::RecordValue::Number(n) => Some(n.clone()),
                    super::ast::RecordValue::Bool(b) => Some(b.to_string()),
                    super::ast::RecordValue::Ident(i)
                        if i.eq_ignore_ascii_case("true") || i.eq_ignore_ascii_case("false") =>
                    {
                        Some(i.to_lowercase())
                    }
                    _ => None,
                };
                let Some(sv) = sval else {
                    return Some((
                        format!("Invalid string value for column `{col_name}`"),
                        format!("`{col_name}` is a string column. Values must be wrapped in quotes."),
                    ));
                };
                if let Some(ml) = targs.first().and_then(|a| a.parse::<usize>().ok()) {
                    if ml > 0 {
                        let actual = sv.len(); // their UTF-8 byte length
                        if actual > ml {
                            return Some((
                                format!("String value for column `{col_name}` exceeds maximum length: expected at most {ml} bytes (UTF-8), got {actual} bytes"),
                                format!("`{col_name}` is defined as `{tn}({})`, limiting values to {ml} bytes in UTF-8 encoding. The provided value uses {actual} bytes.", targs[0]),
                            ));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }
    let mut value_type_errors: Vec<Diagnostic> = Vec::new();
    for t in &doc.tables {
        let Some(rec) = &t.records else {
            continue;
        };
        if rec.rows.is_empty() {
            continue;
        }
        for (ri, row) in rec.rows.iter().enumerate() {
            for (vi, val) in row.iter().enumerate() {
                let cname = if rec.columns.is_empty() {
                    t.columns.get(vi).map(|c| c.name.clone())
                } else {
                    rec.columns.get(vi).cloned()
                };
                let Some(cname) = cname else {
                    continue;
                };
                let Some(col) = t.columns.iter().find(|c| c.name == cname) else {
                    continue;
                };
                let Some(span) = rec.value_spans.get(ri).and_then(|sp| sp.get(vi)) else {
                    continue;
                };
                let tn = norm_ty(&col.ty);
                let enum_members = doc
                    .enums
                    .iter()
                    .find(|e| {
                        e.name == tn && (e.schema.is_none() || e.schema.as_deref() == Some("public"))
                    })
                    .map(|e| e.values.iter().map(|v| v.name.clone()).collect::<Vec<_>>());
                let targs = type_args(&col.ty);
                let has_default = col
                    .settings
                    .default
                    .as_deref()
                    .map(|d| !d.eq_ignore_ascii_case("null"))
                    .unwrap_or(false);
                if let Some((msg, expl)) = value_type_check(
                    val,
                    &cname,
                    &tn,
                    &targs,
                    col.settings.not_null == Some(true),
                    has_default,
                    col.settings.increment,
                    enum_members.as_deref(),
                ) {
                    value_type_errors.push(Diagnostic::detail(
                        span.start,
                        span.end,
                        msg,
                        Severity::Info,
                        Some("Record type errors".to_string()),
                        vec![expl],
                    ));
                }
            }
        }
    }

    /// their nodeRefereeOfRefEndpoint / nodeRefereeOfTableGroupField over
    /// a dotted chain: fragment 1 resolves as a schema when the chain is
    /// 3+ deep (refs) or 2+ deep (groups), else as a table (by name or
    /// alias); a fragment right of a schema resolves as a table or
    /// schema; a fragment right of a table resolves as a column with the
    /// schema-qualified fullname; an unresolved left link stays silent
    /// (refs) or falls through to a schema lookup (groups). Each
    /// fragment resolves as its own nodeReferee call with QUIET left
    /// contexts — the lefts' errors surface only through the fragments
    /// bindFields lists explicitly.
    fn schema_exists(doc: &Document, name: &str) -> bool {
        name == "public"
            || doc.tables.iter().any(|t| t.schema.as_deref() == Some(name))
            || doc.enums.iter().any(|e| e.schema.as_deref() == Some(name))
    }

    enum Sym<'a> {
        Schema(&'a str),
        Table(&'a super::ast::Table),
        Column,
    }

    fn resolve_prefix<'a>(
        doc: &'a Document,
        vars: &'a [(String, usize, usize)],
        k: usize,
        full_len: usize,
        group: bool,
        emit: bool,
        out: &mut Vec<Diagnostic>,
    ) -> Option<Sym<'a>> {
        if k == 0 {
            return Some(Sym::Schema("public"));
        }
        let (name, off, vend) = &vars[k - 1];
        // their anchors carry the fragment TOKEN span
        let frag = *off..*vend;
        if k == 1 {
            if full_len >= 3 || (group && full_len >= 2) {
                // the left-of-access context: a schema lookup
                if schema_exists(doc, name) {
                    return Some(Sym::Schema(name));
                }
                if emit {
                    out.push(Diagnostic::spanned(
                        frag.start,
                        frag.end,
                        format!("Schema '{}' does not exist", name),
                    ));
                }
                return None;
            }
            // the standalone context: a table lookup (name or alias)
            return match doc.tables.iter().find(|t| t.matches(name)) {
                Some(t) => Some(Sym::Table(t)),
                None => {
                    if emit {
                        out.push(Diagnostic::spanned(
                            frag.start,
                            frag.end,
                            format!("Table '{}' does not exist in Schema 'public'", name),
                        ));
                    }
                    None
                }
            };
        }
        let left = resolve_prefix(doc, vars, k - 1, full_len, group, false, &mut Vec::new());
        match left {
            Some(Sym::Schema(s)) => {
                if let Some(t) = doc
                    .tables
                    .iter()
                    .find(|t| t.schema.as_deref() == Some(s) && t.matches(name))
                {
                    Some(Sym::Table(t))
                } else if schema_exists(doc, name) {
                    Some(Sym::Schema(name))
                } else {
                    if emit {
                        // their anchors carry the FRAGMENT token (probe
                        // g11: the member `b.a` errs 4:18-4:19)
                        out.push(Diagnostic::spanned(
                            frag.start,
                            frag.end,
                            format!("Table or schema '{}' does not exist", name),
                        ));
                    }
                    None
                }
            }
            Some(Sym::Table(t)) => {
                if t.columns.iter().any(|c| c.name == *name) {
                    Some(Sym::Column)
                } else {
                    let schema = t.schema.as_deref().unwrap_or("public");
                    if emit {
                        out.push(Diagnostic::spanned(
                            frag.start,
                            frag.end,
                            format!(
                                "Column '{}' does not exist in Table '{}.{}'",
                                name, schema, t.name
                            ),
                        ));
                    }
                    None
                }
            }
            // right of a column (or an unresolved left in a REF chain):
            // silent — except the group fall-through, which retries the
            // fragment as a schema
            _ if group && left.is_none() => {
                if schema_exists(doc, name) {
                    Some(Sym::Schema(name))
                } else {
                    if emit {
                        // the fragment token, not a point (probe g11)
                        out.push(Diagnostic::spanned(
                            frag.start,
                            frag.end,
                            format!("Schema '{}' does not exist", name),
                        ));
                    }
                    None
                }
            }
            _ => None,
        }
    }

    fn bind_chain(doc: &Document, chain: &super::ast::BinderChain, out: &mut Vec<Diagnostic>) {
        let n = chain.vars.len();
        if n == 0 {
            return;
        }
        if chain.group {
            // their group binder resolves ONLY the popped tableBindee —
            // the chain's last fragment
            resolve_prefix(doc, &chain.vars, n, n, true, true, out);
            return;
        }
        if chain.tuple.is_empty() {
            // the plain chain: every fragment is its own nodeReferee call
            // in bindFields' [schemas..., table, column] walk
            for idx in 1..=n {
                resolve_prefix(doc, &chain.vars, idx, n, false, true, out);
            }
        } else {
            // the tuple form t.(a, b): the table fragment resolves with
            // the chain, each tuple column resolves against it quietly
            // (their standalone-column branch reads the symbol via
            // getFiltered — no error re-emission)
            resolve_prefix(doc, &chain.vars, n, n + 1, false, true, out);
            if let Some(Sym::Table(t)) =
                resolve_prefix(doc, &chain.vars, n, n + 1, false, false, &mut Vec::new())
            {
                let schema = t.schema.as_deref().unwrap_or("public");
                for (cname, coff, cend) in &chain.tuple {
                    if !t.columns.iter().any(|c| c.name == *cname) {
                        out.push(Diagnostic::spanned(
                            *coff,
                            *cend,
                            format!(
                                "Column '{}' does not exist in Table '{}.{}'",
                                cname, schema, t.name
                            ),
                        ));
                    }
                }
            }
        }
    }

    // the parse-side binder findings (ref-body shape sets)
    for (offset, end, msg) in parse_binder {
        out.push(Diagnostic::spanned(*offset, *end, msg.clone()));
    }
    // the ref-body binder chains (their bindFields scan →
    // nodeRefereeOfRefEndpoint): resolve each dotted chain against the
    // document with their context rules
    for chain in binder_chains {
        bind_chain(doc, chain, &mut out);
    }
    // the column-parse binder findings (their validateNode class)
    for t in &doc.tables {
        for c in &t.columns {
            for (offset, end, msg) in &c.binder_errors {
                out.push(Diagnostic::spanned(*offset, *end, msg.clone()));
            }
        }
    }
    // the project-parse validator findings (their ProjectValidator: the
    // inline-field/element-context errors)
    if let Some(p) = &doc.project {
        for (offset, end, msg) in &p.binder_errors {
            out.push(Diagnostic::spanned(*offset, *end, msg.clone()));
        }
    }
    // bound refs' application-arg findings (their INVALID_REF_FIELD)
    for r in &doc.refs {
        for (offset, msg) in &r.binder_errors {
            out.push(Diagnostic::plain(*offset, msg.clone()));
        }
    }

    out.sort_by_key(|d| d.offset);
    // their extractDataFromRow count rule (records/interpret.ts) is
    // INTERPRET-layer: the error exists only while shouldInterpretNode
    // holds — the file must be otherwise clean
    let has_blocking_error = out.iter().any(|d| d.severity == Severity::Error)
        || !parse_errors.is_empty();
    if record_count_errors.len() > 0 && !has_blocking_error {
        for (anchor, end, expected, got) in record_count_errors {
            out.push(Diagnostic::spanned(
                *anchor,
                *end,
                format!("Expected {} values but got {}", expected, got),
            ));
        }
        out.sort_by_key(|d| d.offset);
    }
    // the records value-type findings ride the same interpret gate; a
    // finding the constraint path already anchored (the same message at
    // the same value span — their engine's dedupe) stays dropped
    if !has_blocking_error {
        for v in value_type_errors {
            if !out.iter().any(|d| {
                d.severity == v.severity
                    && d.message == v.message
                    && d.offset == v.offset
                    && d.end == v.end
            }) {
                out.push(v);
            }
        }
        out.sort_by_key(|d| d.offset);
    }
    // their interpret-layer EMPTY_TABLE (table/interpret.ts, code 3018):
    // a zero-column table errs 'A Table must have at least one column'
    // anchored at the ELEMENT span — only while the file's binder/parse
    // error set is empty (the file gate; probes zc1/zc2 — a note-only
    // body still has zero columns; zc3/zc6/k4 — the dup-notes, the
    // missing-column and the nameless-name binder errors gate it off).
    // The gate is evaluated ABOVE, so the dup-notes errors pushed
    // earlier close it exactly like theirs.
    if !has_blocking_error {
        for t in &doc.tables {
            if t.columns.is_empty() {
                out.push(Diagnostic::spanned(
                    t.span.start,
                    t.span.end,
                    "A Table must have at least one column".to_string(),
                ));
            }
        }
        out.sort_by_key(|d| d.offset);
    }
    // their shouldInterpretNode (core/global_modules/utils.ts): the
    // interpretation layer runs only while bindFile reports no errors —
    // binder-class (Error-severity) findings suppress the infos; raw
    // parse errors do not gate anything
    // the parse errors gate too (probe: errs2's invalid operand gates the
    // FK infos — their shouldInterpretNode surfaces parse errors through
    // the bind chain); the parse errors themselves render from
    // out.errors, so retaining only Error diagnostics is exact
    if has_blocking_error {
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
        // the group member check flows through the binder-chain channel
        // (their tableGroup binder), so this test runs the full pipeline
        let src = "Table a { id integer }\nTableGroup g {\n  nope\n}\nTable c {\n  x integer\n  indexes {\n    (zzz) [unique]\n  }\n}\n";
        let out = super::super::parser::parse(src);
        let d: Vec<String> = validate_with_parse(&out.doc, src, &out.binder_errors, &out.binder_chains, &out.record_count_errors, &out.errors)
            .into_iter()
            .map(|x| x.message)
            .collect();
        assert!(
            d.contains(&"Table 'nope' does not exist in Schema 'public'".to_string()),
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

    /// The interpret-layer EMPTY_TABLE (their table/interpret.ts, 3018)
    /// + the duplicate-notes check (the validateSubElements 3046). The
    /// colon entries and the block declarations BOTH count (probes
    /// zc4/zc5); the duplicate-notes = a binder-class error that gates
    /// the EMPTY_TABLE off (probe zc3); a note-only body still has zero
    /// columns (probe zc2); the file's other binder errors gate the
    /// EMPTY_TABLE off (probe zc6).
    #[test]
    fn empty_table_and_duplicate_notes_follow_the_file_gate() {
        // zc1: the empty table errs over the ELEMENT span
        let out = parser::parse("Table t {\n}\n");
        let d = validate(&out.doc, "Table t {\n}\n");
        assert_eq!(
            d,
            vec![Diagnostic {
                end: Some(11),
                offset: 0,
                message: "A Table must have at least one column".to_string(),
                severity: Severity::Error,
                category: None,
                explanation: vec![],
                quick_fixes: vec![],
            }],
            "{d:?}"
        );
        // zc2: a note-only body still has zero columns
        let src2 = "Table t {\n  Note { 'x' }\n}\n";
        let out2 = parser::parse(src2);
        let d2 = validate(&out2.doc, src2);
        assert_eq!(d2.len(), 1, "{d:?}");
        assert_eq!(d2[0].message, "A Table must have at least one column");
        assert_eq!(d2[0].offset, 0);
        assert_eq!(d2[0].end, Some(26));
        // zc3: the duplicate-notes (both block forms) gate the
        // EMPTY_TABLE off; the anchors = the note-element spans
        let src3 = "Table t {\n  Note { 'x' }\n  Note { 'y' }\n}\n";
        let out3 = parser::parse(src3);
        let d3 = validate(&out3.doc, src3);
        assert_eq!(d3.len(), 2, "{d:?}");
        assert!(d3.iter().all(
            |x| x.message == "Duplicate notes are defined"
        ), "{d:?}");
        assert_eq!(d3[0].offset, 12);
        assert_eq!(d3[0].end, Some(24));
        assert_eq!(d3[1].offset, 27);
        // zc4: the colon form counts too (the handoff's block-only note
        // was wrong — their engine errs the colon-note spans)
        let src4 = "Table a {\n  id int\n  Note: 'x'\n  Note: 'y'\n}\n";
        let out4 = parser::parse(src4);
        let d4 = validate(&out4.doc, src4);
        assert_eq!(d4.len(), 2, "{d:?}");
        assert_eq!(d4[0].offset, 21);
        assert_eq!(d4[0].end, Some(30));
        // zc6: the file's binder errors gate the EMPTY_TABLE off (the
        // missing column on the empty table)
        let src5 = "Table t {\n}\nTable u { id int }\nRef: u.id > t.id\n";
        let out5 = parser::parse(src5);
        let d5 = validate_with_parse(
            &out5.doc,
            src5,
            &out5.binder_errors,
            &out5.binder_chains,
            &out5.record_count_errors,
            &out5.errors,
        );
        assert_eq!(d5.len(), 1, "{d:?}");
        assert!(
            d5[0].message.contains("does not exist"),
            "{d:?}"
        );
    }

    #[test]
    fn group_and_note_dups_flag_the_later_declarations() {
        // their schema dup map covers TableGroups and sticky Notes; the
        // anchor = the NAME token (g1/g3/g8), and the group fields carry
        // the within-group seen-map (g13) + the membership map (g4/g9)
        let out = super::super::parser::parse(
            "Table a { id int }\nTableGroup g { a }\nTableGroup g { a }\nTableGroup g { a }\n",
        );
        let d = validate(&out.doc, "Table a { id int }\nTableGroup g { a }\nTableGroup g { a }\nTableGroup g { a }\n");
        let group_dups: Vec<_> = d
            .iter()
            .filter(|x| x.message == "TableGroup 'g' already exists in schema 'public'")
            .collect();
        assert_eq!(group_dups.len(), 2, "{d:?}");
        // each later group's name token (g 3:12-3:13, 4:12-4:13)
        assert_eq!(group_dups[0].offset, 49);
        assert_eq!(group_dups[0].end, Some(50));
        // the later groups' member `a` reappears: Table "a" already
        // appears in group "g" (the first claimer named), anchored at
        // each member token
        let reappears: Vec<_> = d
            .iter()
            .filter(|x| x.message == "Table \"a\" already appears in group \"g\"")
            .collect();
        assert_eq!(reappears.len(), 2, "{d:?}");
        assert_eq!(reappears[0].offset, 53);

        let src2 = "Table a { id int }\nTableGroup g {\n  a\n  a\n}\n";
        let out2 = super::super::parser::parse(src2);
        let d2 = validate(&out2.doc, src2);
        // the within-group seen-map re-flags the first sighting (both
        // members err, the enum-field rule) + the second member's
        // membership error
        let field_dups: Vec<_> = d2
            .iter()
            .filter(|x| x.message == "Duplicate TableGroupField 'public.a'")
            .collect();
        assert_eq!(field_dups.len(), 2, "{d:?}");
        assert!(d2.iter().any(
            |x| x.message == "Table \"a\" already appears in group \"g\""
        ), "{d:?}");

        let src3 = "Note n { 'x' }\nNote m { 'y' }\nNote n { 'z' }\n";
        let out3 = super::super::parser::parse(src3);
        let d3 = validate(&out3.doc, src3);
        let note_dups: Vec<_> = d3
            .iter()
            .filter(|x| x.message == "Duplicate Note 'n' in schema 'public'")
            .collect();
        assert_eq!(note_dups.len(), 1, "{d:?}");
        assert_eq!(note_dups[0].offset, 35);

        // a comma-run body binds nothing (their field-form error rides
        // the parse side) — no member machinery fires (g5)
        let src4 = "Table a { id int }\nTableGroup g { a, a }\n";
        let out4 = super::super::parser::parse(src4);
        let d4 = validate(&out4.doc, src4);
        assert!(
            !d4.iter().any(|x| x.message.contains("already appears")),
            "{d:?}"
        );
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
