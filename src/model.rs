//! Semantic model: the AST resolved into nodes and relationship edges.

use crate::dbml::ast::{Cardinality, Document, RecordValue, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Table,
    Enum,
}

#[derive(Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    /// Span of the name token in the source (the jump target when a
    /// diagram column row is clicked).
    pub name_span: Span,
    pub ty: String,
    pub pk: bool,
    /// Participates in a relationship (foreign key side or referenced side).
    pub fk: bool,
    /// Some(true) = not null, Some(false) = explicit null.
    pub not_null: Option<bool>,
    pub unique: bool,
    pub note: Option<String>,
}

/// A table's `records` block, resolved against the node: `columns` are
/// indices into `Node::columns` (the implicit all-columns form expands to
/// every column in definition order), each row is parallel to it, and
/// `targets` marks which value columns navigate to a referenced table
/// that itself has records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRecords {
    pub columns: Vec<usize>,
    pub rows: Vec<Vec<RecordValue>>,
    pub example: bool,
    /// Per value column: `(node, column)` of a ref edge leading to a
    /// table with records (a navigable cell in the record view).
    pub targets: Vec<Option<(usize, usize)>>,
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
    /// `records` sample rows (tables only).
    pub records: Option<NodeRecords>,
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
                    name_span: c.name_span.clone(),
                    // display form: unquoted (the AST keeps verbatim quotes)
                    ty: c.ty.replace('"', ""),
                    pk: c.settings.pk,
                    fk: false,
                    not_null: c.settings.not_null,
                    unique: c.settings.unique,
                    note: c.settings.note.clone(),
                })
                .collect(),
            header_color: t.header_color.clone(),
            name_span: Some(t.name_span.clone()),
            records: None,
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
                    name_span: v.name_span.clone(),
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
            records: None,
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
        // the referenced side keeps its pk/unique identity: dbdiagram never
        // draws the link icon on the column being pointed at
        schema.edges.push(Edge {
            from,
            from_col,
            to,
            to_col,
            card: s.card,
        });
    }

    resolve_records(doc, &mut schema);

    schema
}

/// Attach each table's `records` to its node: resolve the value columns
/// against the node's columns (implicit header = every column in
/// definition order), align the rows, and mark the value columns whose
/// ref edge leads to a table that itself has records. Tables are the
/// first `doc.tables.len()` nodes, in document order.
fn resolve_records(doc: &Document, schema: &mut Schema) {
    let has_records = |n: usize| doc.tables.get(n).is_some_and(|t| t.records.is_some());
    for (ni, t) in doc.tables.iter().enumerate() {
        let Some(rec) = &t.records else {
            continue;
        };
        let node = &schema.nodes[ni];
        // value position p -> node column index (None = header column
        // that does not exist; its values are dropped, alignment kept)
        let resolved: Vec<Option<usize>> = if rec.columns.is_empty() {
            (0..node.columns.len()).map(Some).collect()
        } else {
            rec.columns
                .iter()
                .map(|name| node.columns.iter().position(|c| &c.name == name))
                .collect()
        };
        let columns: Vec<usize> = resolved.iter().filter_map(|c| *c).collect();
        let rows: Vec<Vec<RecordValue>> = rec
            .rows
            .iter()
            .map(|row| {
                resolved
                    .iter()
                    .enumerate()
                    .filter_map(|(p, r)| {
                        r.map(|_| {
                            match row.get(p) {
                                // an empty quoted field is a null
                                Some(RecordValue::Text(s)) if s.is_empty() => RecordValue::Null,
                                Some(v) => v.clone(),
                                None => RecordValue::Null,
                            }
                        })
                    })
                    .collect()
            })
            .collect();
        let targets: Vec<Option<(usize, usize)>> = columns
            .iter()
            .map(|&ci| {
                schema.edges.iter().find_map(|e| {
                    if e.from == ni && e.from_col == ci && has_records(e.to) {
                        Some((e.to, e.to_col))
                    } else if e.to == ni && e.to_col == ci && has_records(e.from) {
                        Some((e.from, e.from_col))
                    } else {
                        None
                    }
                })
            })
            .collect();
        schema.nodes[ni].records = Some(NodeRecords {
            columns,
            rows,
            example: rec.example,
            targets,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbml::parser;

    fn schema(src: &str) -> Schema {
        build(&parser::parse(src).doc)
    }

    #[test]
    fn records_resolve_explicit_and_implicit_columns() {
        // explicit header: value columns map onto the named columns
        let s = schema(
            "Table t {\n  a integer\n  b integer [pk]\n  c integer\n  records (c, a) {\n    1, 2\n  }\n}\n",
        );
        let rec = s.nodes[0].records.as_ref().unwrap();
        assert_eq!(rec.columns, [2, 0], "c then a by node-column index");
        assert_eq!(
            rec.rows,
            vec![vec![
                crate::dbml::ast::RecordValue::Number("1".into()),
                crate::dbml::ast::RecordValue::Number("2".into()),
            ]]
        );
        assert_eq!(rec.targets, [None, None]);

        // implicit header: every column in definition order; a trailing
        // comma's empty field and '' both become nulls
        let s = schema(
            "Table u {\n  id integer [pk]\n  name varchar\n  records {\n    1, 'Ann'\n    2,\n  }\n}\n",
        );
        let rec = s.nodes[0].records.as_ref().unwrap();
        assert_eq!(rec.columns, [0, 1]);
        assert_eq!(
            rec.rows,
            vec![
                vec![
                    crate::dbml::ast::RecordValue::Number("1".into()),
                    crate::dbml::ast::RecordValue::Text("Ann".into()),
                ],
                vec![
                    crate::dbml::ast::RecordValue::Number("2".into()),
                    crate::dbml::ast::RecordValue::Null,
                ],
            ]
        );
        let s = schema("Table u {\n  id integer [pk]\n  name varchar\n  records (name) {\n    ''\n  }\n}\n");
        let rec = s.nodes[0].records.as_ref().unwrap();
        assert_eq!(rec.columns, [1], "name only");
        assert_eq!(rec.rows, vec![vec![crate::dbml::ast::RecordValue::Null]]);

        // a header column that does not exist is dropped, alignment kept
        // (the row count matches the header, per the parser's check)
        let s = schema(
            "Table t {\n  a integer\n  records (a, missing) {\n    1, 2\n  }\n}\n",
        );
        let rec = s.nodes[0].records.as_ref().unwrap();
        assert_eq!(rec.columns, [0]);
        assert_eq!(
            rec.rows,
            vec![vec![crate::dbml::ast::RecordValue::Number("1".into())]]
        );
    }

    #[test]
    fn records_mark_fk_cells_that_lead_to_records() {
        let src = r#"
Table users {
  id integer [pk]
  records {
    1
    2
  }
}

Table posts {
  id integer [pk]
  user_id integer
  records (id, user_id) {
    10, 1
    11, 9
  }
}

Ref: posts.user_id > users.id
"#;
        let s = schema(src);
        let users = s.nodes.iter().find(|n| n.name == "users").unwrap();
        let posts = s.nodes.iter().find(|n| n.name == "posts").unwrap();
        let users_idx = s.nodes.iter().position(|n| n.name == "users").unwrap();

        // posts.user_id refs users.id, which has records -> navigable
        let rec = posts.records.as_ref().unwrap();
        let uid_pos = rec.columns.iter().position(|&c| c == 1).unwrap();
        assert_eq!(rec.targets[uid_pos], Some((users_idx, 0)));
        // posts.id has no edge -> not navigable
        let id_pos = rec.columns.iter().position(|&c| c == 0).unwrap();
        assert_eq!(rec.targets[id_pos], None);

        // the referenced side navigates back too (users.id -> posts)
        let rec = users.records.as_ref().unwrap();
        assert_eq!(rec.targets[0].map(|(n, _)| n), Some(s.nodes.iter().position(|n| n.name == "posts").unwrap()));

        // enums never carry records
        let s = schema("Enum e { a }\nTable t { x e }\n");
        assert!(s.nodes[0].records.is_none());
    }
}
