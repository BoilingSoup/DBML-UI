//! Canonical DBML source formatting: 2-space indents, single spaces, one
//! blank line between top-level blocks, settings in a fixed order.
//!
//! Returns None when the document has parse errors — reformatting a broken
//! doc would destroy information. (Sources with comments are gated at the
//! caller: the AST does not carry them, so formatting would drop them.)

use super::ast::{Cardinality, Document};
use super::parser;

/// Format a full DBML document; None when it does not parse cleanly.
pub fn format_source(src: &str) -> Option<String> {
    let out = parser::parse(src);
    if !out.errors.is_empty() {
        return None;
    }
    Some(render(&out.doc))
}

fn render(doc: &Document) -> String {
    let mut s = String::new();
    if let Some(p) = &doc.project {
        s.push_str(&format!("Project {} {{\n", p.name));
        if let Some(t) = &p.database_type {
            s.push_str(&format!("  database_type: '{t}'\n"));
        }
        if let Some(n) = &p.note {
            s.push_str(&format!("  note: '{n}'\n"));
        }
        s.push_str("}\n\n");
    }
    for t in &doc.tables {
        s.push_str(&format!("Table {}", t.name));
        if let Some(a) = &t.alias {
            s.push_str(&format!(" as {a}"));
        }
        if let Some(c) = &t.header_color {
            s.push_str(&format!(" [headercolor: {c}]"));
        }
        s.push_str(" {\n");
        for c in &t.columns {
            s.push_str(&format!("  {}", c.name));
            if !c.ty.is_empty() {
                s.push_str(&format!(" {}", c.ty));
            }
            let st = column_settings(&c.settings);
            if !st.is_empty() {
                s.push_str(&format!(" [{}]", st.join(", ")));
            }
            s.push('\n');
        }
        if !t.indexes.is_empty() {
            s.push_str("  indexes {\n");
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
                s.push_str(&format!("    {}\n", parts.join(", ")));
            }
            s.push_str("  }\n");
        }
        if let Some(n) = &t.note {
            s.push_str(&format!("  note: '{n}'\n"));
        }
        s.push_str("}\n\n");
    }
    for e in &doc.enums {
        s.push_str(&format!("Enum {} {{\n", e.name));
        for v in &e.values {
            if let Some(n) = &v.note {
                s.push_str(&format!("  {} [note: '{n}']\n", v.name));
            } else {
                s.push_str(&format!("  {}\n", v.name));
            }
        }
        s.push_str("}\n\n");
    }
    for g in &doc.table_groups {
        s.push_str(&format!(
            "TableGroup {} {{\n  {}\n}}\n\n",
            g.name,
            g.tables.join(", ")
        ));
    }
    for r in &doc.refs {
        let name = r
            .name
            .as_deref()
            .map(|n| format!("{n} "))
            .unwrap_or_default();
        s.push_str(&format!(
            "Ref: {}{} {} {}\n",
            name,
            endpoint(&r.from),
            card_str(r.card),
            endpoint(&r.to)
        ));
    }
    let mut s = s.trim_end().to_string();
    if !s.is_empty() {
        s.push('\n');
    }
    s
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
    if let Some(n) = &st.note {
        out.push(format!("note: '{n}'"));
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
}
