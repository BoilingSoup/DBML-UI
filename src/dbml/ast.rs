//! DBML AST. Spans are byte offsets into the source document, used by the
//! editor gutter (errors), click-to-jump (table/enum name spans), and
//! highlight-style rendering.

use std::ops::Range;

pub type Span = Range<usize>;

#[derive(Debug, Default, Clone)]
pub struct Document {
    pub project: Option<Project>,
    pub tables: Vec<Table>,
    pub refs: Vec<Ref>,
    pub enums: Vec<Enum>,
    pub table_groups: Vec<TableGroup>,
    pub notes: Vec<StickyNote>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub span: Span,
    pub database_type: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    /// Optional `schema.name` qualifier (`Table public.users`).
    pub schema: Option<String>,
    pub alias: Option<String>,
    /// Whole `Table ... { ... }` block.
    pub span: Span,
    /// Just the name token; the jump target when clicking a table box.
    pub name_span: Span,
    pub header_color: Option<String>,
    pub note: Option<String>,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    /// `Checks { ... }` block content, verbatim (their importer emits it).
    pub checks: Option<String>,
    /// `records { ... }` sample rows (in-table or top-level, attached to
    /// the named table).
    pub records: Option<TableRecords>,
    /// `Table t [owner: "data-team", ...]` — dbdiagram custom properties,
    /// shown on the header hover card.
    pub custom: Vec<(String, String)>,
}

/// One `records` block: sample rows laid out against `columns` (a table's
/// columns, in definition order when the header list is omitted).
#[derive(Debug, Clone)]
pub struct TableRecords {
    /// Header column list; empty for the implicit all-columns form (the
    /// model resolves it against the table).
    pub columns: Vec<String>,
    /// One entry per row, each parallel to `columns` as written.
    pub rows: Vec<Vec<RecordValue>>,
    /// Each value's source span, parallel to `rows` (the diagnostics'
    /// record-violation anchors).
    pub value_spans: Vec<Vec<Span>>,
    /// `[example]` marker: dbdiagram keeps examples in DBML but excludes
    /// them from SQL dumps.
    pub example: bool,
}

/// A value in a records row. `Number` keeps its source spelling
/// (`2.50`, `1e3`); `Ident` is a bare word (enum member); `Expr` is a
/// backticked SQL expression stored without the backticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordValue {
    Null,
    Text(String),
    Number(String),
    Bool(bool),
    Ident(String),
    Expr(String),
}

impl RecordValue {
    /// The value as written (for the record-constraint messages and the
    /// equality check across both sides of a ref).
    pub fn label(&self) -> String {
        match self {
            RecordValue::Null => "null".to_string(),
            RecordValue::Text(t) => t.clone(),
            RecordValue::Number(n) => n.clone(),
            RecordValue::Bool(b) => b.to_string(),
            RecordValue::Ident(i) => i.clone(),
            RecordValue::Expr(e) => format!("`{e}`"),
        }
    }
}

impl Table {
    /// True if `reference` names this table by name or alias.
    pub fn matches(&self, reference: &str) -> bool {
        self.name == reference || self.alias.as_deref() == Some(reference)
    }
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub name_span: Span,
    pub ty: String,
    pub span: Span,
    pub settings: ColumnSettings,
}

#[derive(Debug, Default, Clone)]
pub struct ColumnSettings {
    pub pk: bool,
    pub increment: bool,
    pub unique: bool,
    /// Some(true) = "not null", Some(false) = explicit "null", None = unspecified.
    pub not_null: Option<bool>,
    pub default: Option<String>,
    pub note: Option<String>,
    /// `[check: \`expr\`]` — kept verbatim (backticks included).
    pub check: Option<String>,
    pub inline_ref: Option<InlineRef>,
    /// Arbitrary `key: "value"` pairs dbdiagram calls custom properties
    /// (`[masking: "partial"]`): any unrecognized key with a value, shown
    /// on the diagram hover cards.
    pub custom: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct InlineRef {
    pub card: Cardinality,
    /// `?` before/after the operator (see `Ref::left_optional`).
    pub left_optional: bool,
    pub right_optional: bool,
    pub to: Endpoint,
    /// Byte offset of the operator (see `Ref::op_offset`).
    pub op_offset: usize,
    /// Byte offset of the `ref` keyword itself — the attribute's start
    /// (their inline-ref right token anchors there).
    pub attr_start: usize,
    /// Endpoint settings bracket `[color: …]` — the relationship's color.
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Index {
    pub columns: Vec<String>,
    pub unique: bool,
    pub pk: bool,
    pub name: Option<String>,
    pub index_type: Option<String>,
    pub span: Span,
    /// Settings were written after a bare comma (`(a), unique`) instead of
    /// inside brackets — valid to our recovery, flagged by dbdiagram's
    /// grammar, fixed by one quick fix.
    pub comma_form: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    OneToMany,  // <
    ManyToOne,  // >
    OneToOne,   // -
    ManyToMany, // <>
}

impl Cardinality {
    pub fn as_str(self) -> &'static str {
        match self {
            Cardinality::OneToMany => "<",
            Cardinality::ManyToOne => ">",
            Cardinality::OneToOne => "-",
            Cardinality::ManyToMany => "<>",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Ref {
    pub name: Option<String>,
    pub span: Span,
    pub from: Endpoint,
    pub to: Endpoint,
    pub card: Cardinality,
    /// Byte offset of the cardinality operator itself (`>`, `<`, `-`,
    /// `<>`) — the target of the nullable-ref quick fixes.
    pub op_offset: usize,
    /// `?` before the operator (`?>`, `?-`, `?<>`): the left side's
    /// reference is optional (its min cardinality drops to zero).
    pub left_optional: bool,
    /// `?` after the operator (`>?`, `-?`, `<>?`): the right side's
    /// reference is optional.
    pub right_optional: bool,
    /// `[delete: cascade]` and friends (uppercased by the parser).
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
    /// `[color: #aabbcc]` — the whole relationship (line + end markers)
    /// renders in this color.
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub table: String,
    /// Optional `schema.table` qualifier in the reference path.
    pub schema: Option<String>,
    pub columns: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Enum {
    pub name: String,
    /// Optional `schema.name` qualifier (`Enum schemaB.gender`).
    pub schema: Option<String>,
    pub span: Span,
    pub name_span: Span,
    pub values: Vec<EnumValue>,
}

#[derive(Debug, Clone)]
pub struct EnumValue {
    pub name: String,
    /// Just the value token; the jump target when clicking an enum row.
    pub name_span: Span,
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TableGroup {
    pub name: String,
    pub span: Span,
    pub tables: Vec<String>,
    /// `[color: #1E69FD]` — the region's stroke + label color.
    pub color: Option<String>,
    /// `Note: '''...'''` inside the block, shown on label hover.
    pub note: Option<String>,
    /// Any other `key: "value"` header settings — dbdiagram custom
    /// properties, in order.
    pub custom: Vec<(String, String)>,
}

/// A top-level sticky note: `Note name { '''text''' }` — renders as a
/// yellow sticky on the diagram canvas.
#[derive(Debug, Clone)]
pub struct StickyNote {
    pub name: String,
    pub span: Span,
    pub text: String,
}
