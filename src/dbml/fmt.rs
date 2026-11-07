//! Canonical DBML source formatting: 2-space indents, single spaces, one
//! blank line between top-level blocks, settings in a fixed order.
//!
//! Comments are preserved: a pre-scan anchors every comment to the
//! construct or body item it precedes (or trails, when it shares the
//! item's line), and the renderer re-emits them at those anchored spots.
//! Returns None when the document has parse errors — reformatting a
//! broken doc would destroy information.

use super::ast::{Cardinality, Document};
use super::parser;
use std::collections::HashMap;

/// Format a full DBML document; None when it does not parse cleanly.
pub fn format_source(src: &str) -> Option<String> {
    let out = parser::parse(src);
    if !out.errors.is_empty() {
        return None;
    }
    let cm = scan_comments(src);
    Some(render(&out.doc, &cm))
}

/// DBML note literal: plain single-quoted, or triple-quoted with
/// backslash-escaped quotes when the note contains quotes (upstream
/// exporter convention — db_dump golden). An already-escaped `\'` in the
/// content stays as-is so formatting is idempotent.
pub(crate) fn quote_note(n: &str) -> String {
    if n.contains('\'') {
        let mut esc = String::with_capacity(n.len() + 1);
        let mut prev: Option<char> = None;
        for c in n.chars() {
            if c == '\'' && prev != Some('\\') {
                esc.push('\\');
            }
            esc.push(c);
            prev = Some(c);
        }
        format!("'''{esc}'''")
    } else {
        format!("'{n}'")
    }
}

// --- comment scanning ------------------------------------------------------

/// Attachments for one construct (project / table / enum / group / ref),
/// keyed by a stable identity string ("T:users#0", "P:#0", "R:#3", "EOF").
#[derive(Debug, Default, Clone)]
struct CAtta {
    /// Standalone comment lines before the construct header.
    leading: Vec<String>,
    /// Trailing comment on the header line.
    header_trailing: Option<String>,
    /// Trailing comment on the closing brace.
    trailing_close: Option<String>,
    /// Standalone comment lines before a body item.
    item_leading: HashMap<String, Vec<String>>,
    /// Trailing comment on a body item line.
    item_trailing: HashMap<String, String>,
    /// Loose comments inside a body with no per-item identity (group names).
    loose: Vec<String>,
}

/// All comment attachments for a document, keyed by construct identity.
#[derive(Debug, Default)]
struct CommentMap {
    constructs: HashMap<String, CAtta>,
}

impl CommentMap {
    fn atta(&mut self, key: &str) -> &mut CAtta {
        self.constructs.entry(key.to_string()).or_default()
    }
}

/// Split a line into (code, comment) at the first comment opener outside
/// string literals. `None` comment = no comment on the line.
fn split_comment(line: &str) -> (&str, Option<String>) {
    let b = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'\'' | b'"' | b'`' => quote = Some(c),
                b'/' if i + 1 < b.len() && b[i + 1] == b'/' => {
                    return (&line[..i], Some(line[i..].to_string()));
                }
                b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                    let end = line[i + 2..].find("*/").map_or(line.len(), |k| i + 2 + k + 2);
                    return (&line[..i], Some(line[i..end].to_string()));
                }
                _ => {}
            },
        }
        i += 1;
    }
    (line, None)
}

fn first_token(code: &str) -> String {
    code.trim()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Name token of a construct header: the word after the keyword,
/// quote-stripped. A quoted name may contain spaces ("Content System"):
/// the quoted span is taken verbatim — the scan keys must match the
/// parser's (and renderer's) full name or the construct's comments
/// silently drop.
fn header_name(code: &str, keyword: &str) -> String {
    let t = code.trim();
    let rest = t[keyword.len()..].trim_start();
    match rest.chars().next() {
        Some(q @ ('"' | '`' | '\'')) => rest[1..].chars().take_while(|c| *c != q).collect(),
        _ => rest
            .trim_start_matches(['"', '`'])
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .collect(),
    }
}

#[derive(Default)]
struct ScanState {
    cm: CommentMap,
    pending: Vec<String>,
    occ: HashMap<String, usize>,
    // context stack: (construct key, kind char, next body-item ordinal)
    stack: Vec<(String, char, usize)>,
    in_block: bool,
    bracket_open: bool,
    /// Inside a table's `records { ... }` block: its rows and closing
    /// brace must not count as columns or close the table's stack frame.
    in_records: bool,
}

impl ScanState {
    fn line(&mut self, line: &str) {
        // a multiline /* */ block: skip lines until it closes; if code
        // resumes after the closer, keep processing that remainder
        if self.in_block {
            match line.find("*/") {
                None => {
                    // continuation line of the block: keep it verbatim
                    // (the closing marker on the last line must survive)
                    let t = line.trim();
                    if !t.is_empty() {
                        self.pending.push(t.to_string());
                    }
                }
                Some(k) => {
                    let head = line[..k + 2].trim();
                    if !head.is_empty() {
                        self.pending.push(head.to_string());
                    }
                    self.in_block = false;
                    let rest = &line[k + 2..];
                    if !rest.trim().is_empty() {
                        self.line(rest);
                    }
                }
            }
            return;
        }
        let (code, comment) = split_comment(line);
        if let Some(c) = &comment {
            if c.starts_with("/*") && !c.contains("*/") && code.trim().is_empty() {
                self.pending.push(c.clone());
                self.in_block = true;
                return;
            }
        }
        let code_trim = code.trim();
        if code_trim.is_empty() {
            if let Some(c) = comment {
                self.pending.push(c);
            }
            return;
        }
        // records rows and their closing brace: comments ride the records
        // item; the brace closes the records block, not the table
        if self.in_records {
            if code_trim.starts_with('}') {
                self.in_records = false;
                if let Some(c) = &comment {
                    if let Some((key, ..)) = self.stack.last() {
                        self.cm.atta(key).item_trailing.insert("r0".into(), c.clone());
                    }
                }
                return;
            }
            if let Some(c) = &comment {
                if let Some((key, _, _)) = self.stack.last() {
                    let at = self.cm.atta(key);
                    let entry = at.item_trailing.entry("r0".into()).or_default();
                    if !entry.is_empty() {
                        entry.push(' ');
                    }
                    entry.push_str(c);
                }
            }
            return;
        }
        // bracket continuation belongs to the previous body item
        if self.bracket_open {
            if let Some(c) = &comment {
                if let Some((key, _, ord)) = self.stack.last() {
                    let k = format!("c{ord}");
                    if let Some(at) = self.cm.atta(key).item_trailing.get_mut(&k) {
                        *at = format!("{at} {c}");
                    }
                }
            }
            if code_trim.contains(']') {
                self.bracket_open = false;
            }
            return;
        }
        let first = first_token(code_trim);
        if first == "}" || code_trim.starts_with('}') {
            if let Some(c) = &comment {
                if let Some((key, ..)) = self.stack.last() {
                    self.cm.atta(key).trailing_close = Some(c.clone());
                }
            }
            self.stack.pop();
            return;
        }
        let Some((ckey, kind, ord)) = self.stack.last().map(|c| (c.0.clone(), c.1, c.2)) else {
            // top level: identify the construct, anchor pending as leading
            let key = match first.as_str() {
                "project" => Some("P:#0".to_string()),
                "table" => {
                    let name = header_name(code_trim, "table");
                    let e = self.occ.entry(format!("T:{name}")).or_insert(0);
                    let key = format!("T:{name}#{}", *e);
                    *e += 1;
                    Some(key)
                }
                "enum" | "enumeration" => {
                    let name = header_name(code_trim, &first);
                    let e = self.occ.entry(format!("E:{name}")).or_insert(0);
                    let key = format!("E:{name}#{}", *e);
                    *e += 1;
                    Some(key)
                }
                "tablegroup" => {
                    let name = header_name(code_trim, "tablegroup");
                    let e = self.occ.entry(format!("G:{name}")).or_insert(0);
                    let key = format!("G:{name}#{}", *e);
                    *e += 1;
                    Some(key)
                }
                "ref" => {
                    let e = self.occ.entry("R".into()).or_insert(0);
                    let key = format!("R:#{}", *e);
                    *e += 1;
                    Some(key)
                }
                // top-level sticky notes: their leading comments anchor to
                // the N:#i key the renderer emits (without this, a comment
                // above a Note stayed pending and landed AFTER the block —
                // at EOF for a trailing note)
                "note" => {
                    let e = self.occ.entry("N".into()).or_insert(0);
                    let key = format!("N:#{}", *e);
                    *e += 1;
                    Some(key)
                }
                _ => None,
            };
            if let Some(key) = key {
                let a = self.cm.atta(&key);
                a.leading = std::mem::take(&mut self.pending);
                a.header_trailing = comment.clone();
                let kind = match first.as_str() {
                    "project" => 'P',
                    "table" => 'T',
                    "enum" | "enumeration" => 'E',
                    "tablegroup" => 'G',
                    "note" => 'N',
                    _ => 'R',
                };
                // refs and notes are header-only constructs: no body frame
                // (the note's ''' text lines must not scan as body items)
                if kind != 'R' && kind != 'N' {
                    self.stack.push((key, kind, 0));
                }
            } else if let Some(c) = comment {
                self.pending.push(c);
            }
            return;
        };
        let mut ord = ord;
        let mut handled = true;
        match (kind, first.as_str()) {
            ('T', "indexes") => {
                let a = self.cm.atta(&ckey);
                a.item_leading.entry("i0".into()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert("i0".into(), c.clone());
                }
            }
            ('T', "checks") => {
                let a = self.cm.atta(&ckey);
                a.item_leading.entry("k0".into()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert("k0".into(), c.clone());
                }
            }
            ('T', "note") => {
                let a = self.cm.atta(&ckey);
                a.item_leading.entry("n0".into()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert("n0".into(), c.clone());
                }
            }
            ('T', "records") => {
                let a = self.cm.atta(&ckey);
                a.item_leading.entry("r0".into()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert("r0".into(), c.clone());
                }
                // a multi-line block: its rows and closer are handled by
                // the in_records fast path (a one-line `records { ... }`
                // needs no special treatment)
                if code_trim.ends_with('{') {
                    self.in_records = true;
                }
            }
            ('T', _) => {
                let key = format!("c{ord}");
                let a = self.cm.atta(&ckey);
                a.item_leading.entry(key.clone()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert(key, c.clone());
                }
                ord += 1;
                if code_trim.contains('[') && !code_trim.contains(']') {
                    self.bracket_open = true;
                }
            }
            ('E', _) => {
                let key = format!("v{ord}");
                let a = self.cm.atta(&ckey);
                a.item_leading.entry(key.clone()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert(key, c.clone());
                }
                ord += 1;
            }
            ('G', _) => {
                self.cm.atta(&ckey).loose.extend(std::mem::take(&mut self.pending));
                if let Some(c) = &comment {
                    self.cm.atta(&ckey).loose.push(c.clone());
                }
            }
            ('P', "database_type") => {
                let a = self.cm.atta(&ckey);
                a.item_leading.entry("db".into()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert("db".into(), c.clone());
                }
            }
            ('P', "note") => {
                let a = self.cm.atta(&ckey);
                a.item_leading.entry("pn".into()).or_default().append(&mut self.pending);
                if let Some(c) = &comment {
                    a.item_trailing.insert("pn".into(), c.clone());
                }
            }
            ('P', _) => {
                self.cm.atta(&ckey).loose.extend(std::mem::take(&mut self.pending));
                if let Some(c) = &comment {
                    self.cm.atta(&ckey).loose.push(c.clone());
                }
            }
            _ => handled = false,
        }
        if !handled {
            if let Some(c) = &comment {
                self.pending.push(c.clone());
            }
        }
        if let Some(last) = self.stack.last_mut() {
            last.2 = ord;
        }
    }
}

/// Scan the source and build the comment attachment map. Standalone comment
/// lines stack until the next item line and anchor to it; a comment sharing
/// a line with an item anchors as that item's trailing comment.
fn scan_comments(src: &str) -> CommentMap {
    let mut st = ScanState::default();
    for line in src.lines() {
        st.line(line);
    }
    st.cm.atta("EOF").leading = st.pending;
    st.cm
}

// --- rendering -------------------------------------------------------------

/// Threaded comment emitter: pulls anchored comments around each line.
struct Emit<'a> {
    cm: &'a CommentMap,
    out: String,
}

impl Emit<'_> {
    fn leading(&mut self, key: &str, item: Option<&str>, indent: &str) {
        let lines: &[String] = match (self.cm.constructs.get(key), item) {
            (Some(a), Some(it)) => a.item_leading.get(it).map(|v| v.as_slice()).unwrap_or(&[]),
            (Some(a), None) => a.leading.as_slice(),
            _ => &[],
        };
        for c in lines {
            self.out.push_str(indent);
            self.out.push_str(c);
            self.out.push('\n');
        }
    }

    fn item_trailing(&self, key: &str, item: &str) -> Option<&String> {
        self.cm.constructs.get(key).and_then(|a| a.item_trailing.get(item))
    }

    fn line(&mut self, indent: &str, key: &str, item: &str, text: &str) {
        self.leading(key, Some(item), indent);
        let tr = self.item_trailing(key, item).cloned();
        self.out.push_str(indent);
        self.out.push_str(text);
        if let Some(tr) = tr {
            self.out.push_str("  ");
            self.out.push_str(&tr);
        }
        self.out.push('\n');
    }

    /// A construct header line: leading comments, then the line with its
    /// header trailing comment.
    fn header(&mut self, indent: &str, key: &str, text: &str) {
        self.leading(key, None, indent);
        self.out.push_str(indent);
        self.out.push_str(text);
        if let Some(a) = self.cm.constructs.get(key) {
            if let Some(tr) = &a.header_trailing {
                self.out.push_str("  ");
                self.out.push_str(tr);
            }
        }
        self.out.push('\n');
    }

    /// Closing brace with its trailing comment.
    fn close(&mut self, indent: usize, key: &str) {
        self.out.push_str(&" ".repeat(indent));
        self.out.push('}');
        if let Some(a) = self.cm.constructs.get(key) {
            if let Some(tr) = &a.trailing_close {
                self.out.push_str("  ");
                self.out.push_str(tr);
            }
        }
        self.out.push('\n');
    }

    fn blank(&mut self) {
        if !self.out.ends_with("\n\n") {
            self.out.push('\n');
        }
    }

    fn loose(&mut self, key: &str, indent: &str) {
        if let Some(a) = self.cm.constructs.get(key) {
            for c in &a.loose {
                self.out.push_str(indent);
                self.out.push_str(c);
                self.out.push('\n');
            }
        }
    }
}

fn render(doc: &Document, cm: &CommentMap) -> String {
    let mut e = Emit { cm, out: String::new() };
    let mut t_occ: HashMap<&str, usize> = HashMap::new();
    let mut e_occ: HashMap<&str, usize> = HashMap::new();

    // Top-level items in SOURCE order. The AST groups constructs by kind
    // (doc.tables, doc.refs, ...), so walking those vectors one kind after
    // another moved every Ref to the end of the file — formatting
    // restructured the code. Each construct carries its header span: merge
    // them, sort by start offset, and emit in the order the user wrote
    // them (the per-kind occurrence counters still see each kind in its
    // own source order, so the comment-map keys are unchanged).
    enum Top<'a> {
        Project(&'a super::ast::Project),
        Table(&'a super::ast::Table),
        Enum(&'a super::ast::Enum),
        Group(&'a super::ast::TableGroup),
        Note(usize, &'a super::ast::StickyNote),
        Ref(usize, &'a super::ast::Ref),
    }
    let mut tops: Vec<(usize, Top)> = Vec::new();
    if let Some(p) = &doc.project {
        tops.push((p.span.start, Top::Project(p)));
    }
    for t in &doc.tables {
        tops.push((t.span.start, Top::Table(t)));
    }
    for en in &doc.enums {
        tops.push((en.span.start, Top::Enum(en)));
    }
    for g in &doc.table_groups {
        tops.push((g.span.start, Top::Group(g)));
    }
    for (i, n) in doc.notes.iter().enumerate() {
        tops.push((n.span.start, Top::Note(i, n)));
    }
    for (i, r) in doc.refs.iter().enumerate() {
        tops.push((r.span.start, Top::Ref(i, r)));
    }
    tops.sort_by_key(|(start, _)| *start);

    for (_start, top) in &tops {
        match top {
            Top::Project(p) => {
                let key = "P:#0";
                e.header("", key, &format!("Project {} {{", p.name));
                if let Some(t) = &p.database_type {
                    e.line("  ", key, "db", &format!("database_type: '{t}'"));
                }
                if let Some(n) = &p.note {
                    e.line("  ", key, "pn", &format!("note: {}", quote_note(n)));
                }
                e.close(0, key);
                e.blank();
            }
            Top::Table(t) => {
                let occ = { let v = t_occ.entry(t.name.as_str()).or_insert(0); let o = *v; *v += 1; o };
                let key = format!("T:{}#{occ}", t.name);
                let mut header = format!("Table {}", ident(&t.name));
                if let Some(a) = &t.alias {
                    header.push_str(&format!(" as {}", ident(a)));
                }
                if t.header_color.is_some() || !t.custom.is_empty() {
                    let mut st = Vec::new();
                    if let Some(c) = &t.header_color {
                        st.push(format!("headercolor: {c}"));
                    }
                    for (k, v) in &t.custom {
                        st.push(format!("{k}: \"{v}\""));
                    }
                    header.push_str(&format!(" [{}]", st.join(", ")));
                }
                header.push_str(" {");
                e.header("", &key, &header);
                for (i, c) in t.columns.iter().enumerate() {
                    let item = &format!("c{i}");
                    let mut text = ident(&c.name);
                    if !c.ty.is_empty() {
                        // a type with whitespace ("character varying(15)")
                        // needs quotes to re-parse as one type — but the
                        // parser keeps the source quotes in `ty`, so an
                        // already-quoted type goes back verbatim
                        let quoted =
                            c.ty.starts_with('"') && c.ty.ends_with('"') && c.ty.len() >= 2;
                        let ty = if quoted || !c.ty.chars().any(|ch| ch.is_whitespace()) {
                            c.ty.clone()
                        } else {
                            format!("\"{}\"", c.ty)
                        };
                        text.push_str(&format!(" {}", ty));
                    }
                    let st = column_settings(&c.settings);
                    if !st.is_empty() {
                        text.push_str(&format!(" [{}]", st.join(", ")));
                    }
                    e.line("  ", &key, item, &text);
                }
                if !t.indexes.is_empty() {
                    e.leading(&key, Some("i0"), "  ");
                    e.out.push_str("  indexes {\n");
                    for ix in &t.indexes {
                        // dbdiagram's grammar: a bracketed column list, then any
                        // settings INSIDE one [...] — `(a, b) [unique, name: 'x']`.
                        // Settings after a bare comma do not parse there.
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
                        if let Some(t) = &ix.index_type {
                            st.push(format!("type: {t}"));
                        }
                        let line = if st.is_empty() {
                            format!("({})", ix.columns.join(", "))
                        } else {
                            format!("({}) [{}]", ix.columns.join(", "), st.join(", "))
                        };
                        e.out.push_str(&format!("    {line}\n"));
                    }
                    e.out.push_str("  }\n");
                }
                if let Some(rec) = &t.records {
                    let mut blob = String::from("records");
                    if !rec.columns.is_empty() {
                        blob.push_str(&format!(" ({})", rec.columns.join(", ")));
                    }
                    if rec.example {
                        blob.push_str(" [example]");
                    }
                    blob.push_str(" {");
                    for row in &rec.rows {
                        blob.push_str("\n    ");
                        blob.push_str(
                            &row.iter().map(record_value_src).collect::<Vec<_>>().join(", "),
                        );
                    }
                    blob.push_str("\n  }");
                    e.line("  ", &key, "r0", &blob);
                }
                if let Some(c) = &t.checks {
                    e.leading(&key, Some("k0"), "  ");
                    e.line("  ", &key, "k0", &format!("Checks {{\n{c}\n  }}"));
                }
                if let Some(n) = &t.note {
                    e.line("  ", &key, "n0", &format!("note: {}", quote_note(n)));
                }
                e.close(0, &key);
                e.blank();
            }
            Top::Enum(en) => {
                let occ = { let v = e_occ.entry(en.name.as_str()).or_insert(0); let o = *v; *v += 1; o };
                let key = format!("E:{}#{occ}", en.name);
                e.header("", &key, &format!("Enum {} {{", ident(&en.name)));
                for (i, v) in en.values.iter().enumerate() {
                    let item = &format!("v{i}");
                    let text = if let Some(n) = &v.note {
                        format!("{} [note: {}]", ident(&v.name), quote_note(n))
                    } else {
                        ident(&v.name)
                    };
                    e.line("  ", &key, item, &text);
                }
                e.close(0, &key);
                e.blank();
            }
            Top::Group(g) => {
                let key = format!("G:{}#0", g.name);
                // names with spaces need quoting (dbdiagram's doc example)
                let name = if g.name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    g.name.clone()
                } else {
                    format!("\"{}\"", g.name)
                };
                let settings = if g.color.is_some() {
                    format!(" [color: {}]", g.color.clone().unwrap_or_default())
                } else {
                    String::new()
                };
                e.header("", &key, &format!("TableGroup {name}{settings} {{"));
                e.loose(&key, "  ");
                // one member per line, no commas — dbdiagram's grammar separates
                // group members with whitespace only
                let body = g.tables.iter().map(|t| ident(t)).collect::<Vec<_>>().join("\n  ");
                e.line("  ", &key, "g0", &body);
                if let Some(n) = &g.note {
                    // multi-line notes emit the uniform expanded block
                    // (quotes on their own lines, lines indented) — a raw
                    // newline inside quote_note's single quotes would
                    // produce output that no longer parses
                    if n.contains('\n') {
                        e.line("  ", &key, "gn", "Note: '''");
                        for l in n.lines() {
                            e.line("  ", &key, "gn", l);
                        }
                        e.line("  ", &key, "gn", "'''");
                    } else {
                        e.line("  ", &key, "gn", &format!("Note: {}", quote_note(n)));
                    }
                }
                e.close(0, &key);
                e.blank();
            }
            Top::Note(i, n) => {
                // top-level sticky notes: single-line text stays inline; multi-line
                // text emits the uniform expanded block (each line indented, the
                // quotes on their own lines) — an embedded raw newline inside one
                // e.line() mangled the block's line structure
                let key = format!("N:#{i}");
                e.header("", &key, &format!("Note {} {{", ident(&n.name)));
                if n.text.contains('\n') {
                    e.line("  ", &key, "n0", "'''");
                    for l in n.text.lines() {
                        e.line("  ", &key, "n0", l);
                    }
                    e.line("  ", &key, "n0", "'''");
                } else {
                    e.line("  ", &key, "n0", &format!("'''{}'''", n.text));
                }
                e.close(0, &key);
                e.blank();
            }
            Top::Ref(i, r) => {
                let key = format!("R:#{i}");
                // named grammar is `Ref name: from > to` — `Ref: name ...`
                // parses the name as the FROM table and breaks on re-parse
                let name = match r.name.as_deref() {
                    Some(n) => format!(" {}", ident(n)),
                    None => String::new(),
                };
                let mut settings = Vec::new();
                if let Some(d) = &r.on_delete {
                    settings.push(format!("delete: {d}"));
                }
                if let Some(u) = &r.on_update {
                    settings.push(format!("update: {u}"));
                }
                if let Some(c) = &r.color {
                    settings.push(format!("color: {c}"));
                }
                let st = if settings.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", settings.join(", "))
                };
                e.header(
                    "",
                    &key,
                    &format!(
                        "Ref{name}: {} {} {}{}",
                        endpoint(&r.from),
                        card_str_full(r.card, r.left_optional, r.right_optional),
                        endpoint(&r.to),
                        st
                    ),
                );
                // single-line construct, but the one-blank-line-between-
                // blocks rule applies here too now that a Ref can sit
                // between two blocks instead of ending the file
                e.blank();
            }
        }
    }
    // comments after everything
    e.leading("EOF", None, "");
    let out = std::mem::take(&mut e.out);
    let mut out = out.trim_end().to_string();
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// The operator with its optional marks round-tripped (`?>`, `>-?`, …).
fn card_str_full(c: Cardinality, left_opt: bool, right_opt: bool) -> String {
    let base = match c {
        Cardinality::OneToMany => "<",
        Cardinality::ManyToOne => ">",
        Cardinality::OneToOne => "-",
        Cardinality::ManyToMany => "<>",
    };
    format!(
        "{}{}{}",
        if left_opt { "?" } else { "" },
        base,
        if right_opt { "?" } else { "" }
    )
}

/// Records value as source text. Strings re-quote with the record
/// string's own `''` convention (the note style's triple quotes would not
/// re-parse through the records lexer); backtick expressions go back
/// verbatim.
fn record_value_src(v: &super::ast::RecordValue) -> String {
    match v {
        super::ast::RecordValue::Null => "null".to_string(),
        super::ast::RecordValue::Text(s) => format!("'{}'", s.replace('\'', "''")),
        super::ast::RecordValue::Number(n) => n.clone(),
        super::ast::RecordValue::Bool(b) => {
            if *b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        super::ast::RecordValue::Ident(s) => s.clone(),
        super::ast::RecordValue::Expr(s) => format!("`{s}`"),
    }
}

/// An identifier rendered so it re-parses to the same name: bare when
/// every char is a bare-ident char, double-quoted otherwise (a name with
/// a space or `#` emitted bare would parse as junk — `Table Customer
/// Data {` — and corrupt the file on the next format).
fn ident(n: &str) -> String {
    if !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
        n.to_string()
    } else {
        format!("\"{n}\"")
    }
}

fn endpoint(e: &super::ast::Endpoint) -> String {
    match e.columns.as_slice() {
        [] | [_] if e.columns.first().map_or(true, |c| c == "*") => ident(&e.table),
        [_] => format!("{}.{}", ident(&e.table), ident(&e.columns[0])),
        cols => format!(
            "{}.({})",
            ident(&e.table),
            cols.iter().map(|c| ident(c)).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn column_settings(st: &super::ast::ColumnSettings) -> Vec<String> {
    let mut out = Vec::new();
    if st.pk {
        out.push("pk".to_string());
    }
    if st.increment {
        out.push("increment".to_string());
    }
    match st.not_null {
        Some(true) => out.push("not null".to_string()),
        Some(false) => out.push("null".to_string()),
        None => {}
    }
    if st.unique {
        out.push("unique".to_string());
    }
    if let Some(d) = &st.default {
        out.push(format!("default: {d}"));
    }
    if let Some(r) = &st.inline_ref {
        // the endpoint color bracket round-trips (the picker writes it)
        let color = r
            .color
            .as_deref()
            .map(|c| format!(" [color: {c}]"))
            .unwrap_or_default();
        out.push(format!(
            "ref: {} {}{}",
            card_str_full(r.card, r.left_optional, r.right_optional),
            endpoint(&r.to),
            color
        ));
    }
    if let Some(c) = &st.check {
        out.push(format!("check: {c}"));
    }
    if let Some(n) = &st.note {
        out.push(format!("note: {}", quote_note(n)));
    }
    // custom properties re-emit double-quoted (dbdiagram's own style), so
    // values with spaces or empty strings round-trip
    for (k, v) in &st.custom {
        out.push(format!("{k}: \"{v}\""));
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn comments_above_a_sticky_note_stay_above_it() {
        // the scanner's top-level recognition had no `note` arm: a comment
        // above a Note stayed pending and landed AFTER the block (at EOF
        // for a trailing note — the user's feature_tour.dbml report)
        let src = "Table t {\n  id int\n}\n\n// sticky notes howto\nNote todo {\n  '''one'''\n}\n";
        let out = format_source(src).unwrap();
        let c = out.find("// sticky notes howto").unwrap();
        let n = out.find("Note todo {").unwrap();
        assert!(c < n, "the comment stays above the note: {out}");
        assert!(!out[c..n].contains("Note todo {"));
        // and the comment is not duplicated after the block
        assert_eq!(out.matches("// sticky notes howto").count(), 1);
    }

    #[test]
    fn sticky_note_blocks_round_trip() {
        // the Note header lost its braces once (an ident() refactor
        // dropped the '{{') and the file stopped parsing — pin the block
        let src = "Table t { id int }\nNote todo {\n  '''one\ntwo'''\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("Note todo {"), "{out}");
        assert!(out.contains("'''"), "{out}");
        assert!(out.trim_end().ends_with('}'), "{out}");
        // re-parses to the same note
        let reparsed = crate::dbml::parser::parse(&out);
        assert!(reparsed.errors.is_empty(), "{:?}", reparsed.errors);
        assert_eq!(reparsed.doc.notes.len(), 1);
        assert_eq!(reparsed.doc.notes[0].name, "todo");
        assert_eq!(reparsed.doc.notes[0].text, "one\ntwo");
    }

    #[test]
    fn every_construct_kind_formats_idempotently() {
        // format(format(x)) == format(x) for a document containing every
        // top-level kind: the renderer's output must always re-parse to
        // the same model (this catches dropped braces, broken quoting,
        // and type round-trips in one net)
        let src = "Project demo {\n  database_type: 'PostgreSQL'\n}\n\nTable \"user data\" [headercolor: #123456, owner: \"team\"] {\n  \"id\" integer [pk, increment]\n  \"name\" \"character varying(80)\" [not null, ref: > \"order rows\".\"order id\"]\n  indexes { (\"name\") [unique] }\n  note: 'the users'\n}\n\nEnum \"order state\" {\n  open\n  closed [note: 'done']\n}\n\nTableGroup \"Content System\" [color: #1E69FD] {\n  \"user data\"\n}\n\nRef \"fk_1\": \"order rows\".\"order id\" > \"user data\".\"id\" [delete: cascade]\n\nNote todo {\n  '''\n  multi\n  line\n  '''\n}\n";
        let once = format_source(src).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice, "format must be idempotent:\n{once}\n----\n{twice}");
        let reparsed = crate::dbml::parser::parse(&once);
        assert!(reparsed.errors.is_empty(), "{:?}", reparsed.errors);
        assert_eq!(reparsed.doc.tables.len(), 1);
        assert_eq!(reparsed.doc.enums.len(), 1);
        assert_eq!(reparsed.doc.table_groups.len(), 1);
        assert_eq!(reparsed.doc.notes.len(), 1);
        assert_eq!(reparsed.doc.refs.len(), 1);
    }

    #[test]
    fn inline_ref_colors_round_trip() {
        // the picker writes the endpoint color bracket on inline refs;
        // the formatter must keep it (it re-renders the line's color)
        let src = "Table a { id integer [pk] }\nTable b { user_id integer [ref: > a.id [color: #ff0000]] }\n";
        let once = format_source(src).unwrap();
        assert!(once.contains("[color: #ff0000]"), "{once}");
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice, "idempotent: {once}");
    }

    #[test]
    fn top_level_items_keep_their_source_order() {
        // the AST groups by kind; the renderer must NOT — a Ref written
        // between two tables used to move to the end of the file
        let src = "Table a {\n  x int\n}\n\nRef: b.y > a.x\n\nTable b {\n  y int\n}\n";
        let out = format_source(src).unwrap();
        let a = out.find("Table a").unwrap();
        let r = out.find("Ref:").unwrap();
        let b = out.find("Table b").unwrap();
        assert!(a < r && r < b, "Ref stays where it was written: {out}");
        // a blank line separates the interleaved Ref from its neighbors
        assert!(out.contains("}\n\nRef: b.y > a.x\n\nTable b {"), "{out}");
    }

    #[test]
    fn quoted_group_names_keep_their_leading_comments() {
        // the scan's header name must be the quoted span (spaces inside):
        // keying by the first word made the scan's key differ from the
        // renderer's, silently dropping the group's comments
        let src = "Table users {\n  id int\n}\n\n// about the group\nTableGroup \"Content System\" {\n  users\n}\n";
        let out = format_source(src).unwrap();
        assert!(
            out.find("// about the group").unwrap() < out.find("TableGroup").unwrap(),
            "the group's comment survives: {out}"
        );
    }

    #[test]
    fn group_multiline_notes_emit_the_expanded_block() {
        // a raw newline inside quote_note's single quotes produced output
        // that no longer parsed
        let src = "TableGroup g {\n  a\n  Note: '''\n  one\n  two\n  '''\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("Note: '''\n  one\n  two\n  '''"), "{out}");
        // and it round-trips: the re-parse sees the same note
        let reparsed = crate::dbml::parser::parse(&out);
        assert_eq!(reparsed.doc.table_groups[0].note.as_deref(), Some("one\ntwo"));
    }

    #[test]
    fn custom_properties_round_trip() {
        // double-quoted re-emission keeps spaces round-tripping
        let src = "Table t [owner: \"data team\"] {\n  a int [zone: \"a 1\"]\n}\n";
        let once = format_source(src).unwrap();
        let out = crate::dbml::parser::parse(&once);
        assert_eq!(
            out.doc.tables[0].custom,
            vec![("owner".to_string(), "data team".to_string())]
        );
        assert_eq!(
            out.doc.tables[0].columns[0].settings.custom,
            vec![("zone".to_string(), "a 1".to_string())]
        );
        // and the second format is stable
        assert_eq!(format_source(&once).unwrap(), once);
    }


    use super::*;

    #[test]
    fn formats_a_messy_document() {
        let src = "Project   blog { database_type:'MySQL' }\nTable   users  as U  [headercolor:#123456] {\nid integer[pk,increment]\nemail varchar(80) [not null,unique]\nb_ref integer [ref:> users.id]\n}\nEnum  st { a b [note:'x'] }\nRef: users.b_ref  <  st.a\n";
        let f = format_source(src).expect("parses");
        assert_eq!(
            f,
            "Project blog {\n  database_type: 'MySQL'\n}\n\n\
             Table users as U [headercolor: #123456] {\n  \
             id integer [pk, increment]\n  \
             email varchar(80) [not null, unique]\n  \
             b_ref integer [ref: > users.id]\n}\n\n\
             Enum st {\n  a\n  b [note: 'x']\n}\n\n\
             Ref: users.b_ref < st.a\n"
        );
        // the formatted output re-parses cleanly and is a fixed point
        let again = format_source(&f).expect("idempotent parse");
        assert_eq!(again, f, "formatting is idempotent");
    }

    #[test]
    fn broken_source_is_rejected() {
        assert!(format_source("Table x { id integer").is_none());
        assert!(format_source("").is_some());
    }

    #[test]
    fn debug_comment_map() {
        let src = "Table users {\n  // the identifier\n  id integer [pk] // inline\n  email varchar\n}\n";
        let cm = scan_comments(src);
        for (k, v) in &cm.constructs {
            println!("KEY {k}\n  leading={:?}\n  items={:?}\n  item_tr={:?}\n  hdr={:?}\n  close={:?}",
                v.leading, v.item_leading, v.item_trailing, v.header_trailing, v.trailing_close);
        }
    }

    #[test]
    fn comments_survive_formatting() {
        let src = "// top note\nTable users {\n  // the identifier\n  id integer [pk] // inline\n  // about email\n  email varchar(80)\n  /* block\n     comment */\n  name varchar\n}\n\n// users posts\nRef: posts.user_id > users.id\n";
        let f = format_source(src).expect("parses with comments");
        for want in [
            "// top note",
            "// the identifier",
            "// inline",
            "// about email",
            "/* block",
            "comment */",
            "// users posts",
        ] {
            assert!(f.contains(want), "lost {want:?} in:\n{f}");
        }
        // anchored positions: top note first, trailing comment on id line
        assert!(f.starts_with("// top note\nTable users {"));
        assert!(f.contains("id integer [pk]  // inline"));
        assert!(f.contains("// the identifier\n  id"), "anchored to its column");
        // idempotent: formatting again keeps the comments in place
        let again = format_source(&f).expect("re-parse");
        assert_eq!(again, f, "idempotent with comments");
    }

    #[test]
    fn enum_and_project_comments_survive() {
        let src = "Project p {\n  // db kind\n  database_type: 'MySQL'\n}\nEnum st {\n  // first\n  a\n  b\n}\n";
        let f = format_source(src).expect("parses");
        assert!(f.contains("// db kind\n  database_type"), "{}", f);
        assert!(f.contains("// first\n  a"), "{}", f);
    }

    #[test]
    fn records_blocks_render_and_round_trip() {
        let src = "Table t {\n  id integer [pk]\n  name varchar\n  records (id, name) {\n    1, 'Alice'\n    2, 'Bob''s'\n    3, null\n    4, `now()`\n  }\n}\n";
        let f = format_source(src).expect("parses");
        assert!(f.contains("  records (id, name) {"), "{f}");
        assert!(f.contains("    1, 'Alice'\n"), "{f}");
        assert!(f.contains("'Bob''s'"), "{f}");
        assert!(f.contains("3, null"), "{f}");
        assert!(f.contains("4, `now()`"), "{f}");
        let again = format_source(&f).expect("re-parse");
        assert_eq!(again, f, "records formatting is idempotent");
    }

    #[test]
    fn records_implicit_and_example_render() {
        let src = "records u(id, name) [example] {\n  1, 'A'\n}\nTable u {\n  id integer [pk]\n  name varchar\n  indexes {\n    (name) [unique]\n  }\n}\nTable bare {\n  x integer\n  records {\n    9\n  }\n}\n";
        let f = format_source(src).expect("parses");
        // top-level records attach to their table and render after the
        // indexes block, with the header list and [example] kept
        let ipos = f.find("indexes {").expect("indexes");
        let rpos = f.find("records (id, name) [example] {").expect("records rendered");
        assert!(ipos < rpos);
        assert!(f.contains("    1, 'A'"), "{f}");
        // the implicit form stays implicit (the model resolves the order)
        assert!(f.contains("  records {\n    9\n  }"), "{f}");
        let again = format_source(&f).expect("re-parse");
        assert_eq!(again, f);
    }

    #[test]
    fn records_render_with_comments_present() {
        let src = "// header\nTable t {\n  // a column\n  id integer [pk]\n  // about records\n  records (id) {\n    1\n  }\n  name varchar // trailing\n}\n";
        let f = format_source(src).expect("parses");
        assert!(f.contains("// header"), "{f}");
        assert!(f.contains("// a column\n  id"), "{f}");
        assert!(f.contains("// about records\n  records (id) {"), "{f}");
        assert!(f.contains("1\n  }\n"), "{f}");
        assert!(f.contains("name varchar  // trailing"), "{f}");
        let again = format_source(&f).expect("re-parse");
        assert_eq!(again, f, "idempotent with comments around records");
    }
}
