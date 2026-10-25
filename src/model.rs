//! Semantic model: the AST resolved into nodes and relationship edges.

use crate::dbml::ast::{Cardinality, Document, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Table,
    Enum,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub ty: String,
    pub pk: bool,
    /// Participates in a relationship (foreign key side or referenced side).
    pub fk: bool,
    /// Some(true) = not null, Some(false) = explicit null.
    pub not_null: Option<bool>,
    pub unique: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub name: String,
    pub alias: Option<String>,
    pub columns: Vec<ColumnInfo>,
    pub header_color: Option<String>,
    /// Span of the name token in the source (click-to-jump target).
    pub name_span: Option<Span>,
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: usize,
    pub from_col: usize,
    pub to: usize,
    pub to_col: usize,
    pub card: Cardinality,
}

#[derive(Debug, Default, Clone)]
pub struct Schema {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Refs that point at tables that don't exist (yet).
    pub dangling: usize,
}

struct EdgeSpec {
    table: String,
    column: String,
    card: Cardinality,
    to_table: String,
    to_column: String,
}

/// Build the semantic model from a parsed document. Tables first, then enums,
/// preserving document order; refs resolve against names or aliases.
pub fn build(doc: &Document) -> Schema {
    let mut schema = Schema::default();

    for t in &doc.tables {
        schema.nodes.push(Node {
            kind: NodeKind::Table,
            name: t.name.clone(),
            alias: t.alias.clone(),
            columns: t
                .columns
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name.clone(),
                    ty: c.ty.clone(),
                    pk: c.settings.pk,
                    fk: false,
                    not_null: c.settings.not_null,
                    unique: c.settings.unique,
                    note: c.settings.note.clone(),
                })
                .collect(),
            header_color: t.header_color.clone(),
            name_span: Some(t.name_span.clone()),
        });
    }
    for e in &doc.enums {
        schema.nodes.push(Node {
            kind: NodeKind::Enum,
            name: e.name.clone(),
            alias: None,
            columns: e
                .values
                .iter()
                .map(|v| ColumnInfo {
                    name: v.name.clone(),
                    ty: String::new(),
                    pk: false,
                    fk: false,
                    not_null: None,
                    unique: false,
                    note: v.note.clone(),
                })
                .collect(),
            header_color: None,
            name_span: Some(e.name_span.clone()),
        });
    }

    let find = |name: &str, nodes: &[Node]| -> Option<usize> {
        nodes
            .iter()
            .position(|n| n.kind == NodeKind::Table && n.name == name)
            .or_else(|| {
                nodes
                    .iter()
                    .position(|n| n.kind == NodeKind::Table && n.alias.as_deref() == Some(name))
            })
    };

    let mut specs: Vec<EdgeSpec> = Vec::new();
    for t in &doc.tables {
        let host = t.name.clone();
        for c in &t.columns {
            if let Some(r) = &c.settings.inline_ref {
                for col in &r.to.columns {
                    specs.push(EdgeSpec {
                        table: host.clone(),
                        column: c.name.clone(),
                        card: r.card,
                        to_table: r.to.table.clone(),
                        to_column: col.clone(),
                    });
                }
            }
        }
    }
    for r in &doc.refs {
        for (fc, tc) in r.from.columns.iter().zip(r.to.columns.iter()) {
            specs.push(EdgeSpec {
                table: r.from.table.clone(),
                column: fc.clone(),
                card: r.card,
                to_table: r.to.table.clone(),
                to_column: tc.clone(),
            });
        }
    }

    for s in specs {
        let (Some(from), Some(to)) = (
            find(&s.table, &schema.nodes),
            find(&s.to_table, &schema.nodes),
        ) else {
            schema.dangling += 1;
            continue;
        };
        let col_of = |idx: usize, name: &str| -> usize {
            schema.nodes[idx]
                .columns
                .iter()
                .position(|c| c.name == name)
                .unwrap_or(0)
        };
        let from_col = col_of(from, &s.column);
        let to_col = col_of(to, &s.to_column);
        if let Some(c) = schema.nodes[from].columns.get_mut(from_col) {
            c.fk = true;
        }
        if let Some(c) = schema.nodes[to].columns.get_mut(to_col) {
            c.fk = true;
        }
        schema.edges.push(Edge {
            from,
            from_col,
            to,
            to_col,
            card: s.card,
        });
    }

    schema
}
