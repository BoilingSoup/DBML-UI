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
    pub inline_ref: Option<InlineRef>,
}

#[derive(Debug, Clone)]
pub struct InlineRef {
    pub card: Cardinality,
    pub to: Endpoint,
}

#[derive(Debug, Clone)]
pub struct Index {
    pub columns: Vec<String>,
    pub unique: bool,
    pub pk: bool,
    pub name: Option<String>,
    pub index_type: Option<String>,
    pub span: Span,
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
    /// `[delete: cascade]` and friends (uppercased by the parser).
    pub on_delete: Option<String>,
    pub on_update: Option<String>,
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
    pub note: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TableGroup {
    pub name: String,
    pub span: Span,
    pub tables: Vec<String>,
}
