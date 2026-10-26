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
/// quote-stripped.
fn header_name(code: &str, keyword: &str) -> String {
    let t = code.trim();
    let rest = t[keyword.len()..].trim_start();
    rest.trim_start_matches(['"', '`'])
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect()
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
                    _ => 'R',
                };
                if kind != 'R' {
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

    if let Some(p) = &doc.project {
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
    for t in &doc.tables {
        let occ = { let v = t_occ.entry(t.name.as_str()).or_insert(0); let o = *v; *v += 1; o };
        let key = format!("T:{}#{occ}", t.name);
        let mut header = format!("Table {}", t.name);
        if let Some(a) = &t.alias {
            header.push_str(&format!(" as {a}"));
        }
        if let Some(c) = &t.header_color {
            header.push_str(&format!(" [headercolor: {c}]"));
        }
        header.push_str(" {");
        e.header("", &key, &header);
        for (i, c) in t.columns.iter().enumerate() {
            let item = &format!("c{i}");
            let mut text = c.name.to_string();
            if !c.ty.is_empty() {
                text.push_str(&format!(" {}", c.ty));
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
                let mut parts = vec![format!("({})", ix.columns.join(", "))];
                if ix.unique {
                    parts.push("unique".to_string());
                }
                if ix.pk {
                    parts.push("pk".to_string());
                }
                if let Some(n) = &ix.name {
                    parts.push(format!("name: '{n}'"));
                }
                if let Some(t) = &ix.index_type {
                    parts.push(format!("type: {t}"));
                }
                e.out.push_str(&format!("    {}\n", parts.join(", ")));
            }
            e.out.push_str("  }\n");
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
    for en in &doc.enums {
        let occ = { let v = e_occ.entry(en.name.as_str()).or_insert(0); let o = *v; *v += 1; o };
        let key = format!("E:{}#{occ}", en.name);
        e.header("", &key, &format!("Enum {} {{", en.name));
        for (i, v) in en.values.iter().enumerate() {
            let item = &format!("v{i}");
            let text = if let Some(n) = &v.note {
                format!("{} [note: {}]", v.name, quote_note(n))
            } else {
                v.name.clone()
            };
            e.line("  ", &key, item, &text);
        }
        e.close(0, &key);
        e.blank();
    }
    for g in &doc.table_groups {
        let key = format!("G:{}#0", g.name);
        e.header("", &key, &format!("TableGroup {} {{", g.name));
        e.loose(&key, "  ");
        e.line("  ", &key, "g0", &g.tables.join(", "));
        e.close(0, &key);
        e.blank();
    }
    for (i, r) in doc.refs.iter().enumerate() {
        let key = format!("R:#{i}");
        let name = r
            .name
            .as_deref()
            .map(|n| format!("{n} "))
            .unwrap_or_default();
        e.header(
            "",
            &key,
            &format!("Ref: {}{} {} {}", name, endpoint(&r.from), card_str(r.card), endpoint(&r.to)),
        );
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

fn card_str(c: Cardinality) -> &'static str {
    match c {
        Cardinality::OneToMany => "<",
        Cardinality::ManyToOne => ">",
        Cardinality::OneToOne => "-",
        Cardinality::ManyToMany => "<>",
    }
}

fn endpoint(e: &super::ast::Endpoint) -> String {
    match e.columns.as_slice() {
        [] | [_] if e.columns.first().map_or(true, |c| c == "*") => e.table.clone(),
        [_] => format!("{}.{}", e.table, e.columns[0]),
        cols => format!("{}.({})", e.table, cols.join(", ")),
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
        out.push(format!("ref: {} {}", card_str(r.card), endpoint(&r.to)));
    }
    if let Some(c) = &st.check {
        out.push(format!("check: {c}"));
    }
    if let Some(n) = &st.note {
        out.push(format!("note: {}", quote_note(n)));
    }
    out
}

#[cfg(test)]
mod tests {
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
}
