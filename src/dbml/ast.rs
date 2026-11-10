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
    /// Their ProjectValidator findings raised while parsing the block
    /// (the inline-field/element-context errors) — emitted by validate().
    pub binder_errors: Vec<(usize, usize, String)>,
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
    /// The `Note:` block-entry token (their attribute node).
    pub note_span: Option<Span>,
    pub columns: Vec<Column>,
    pub indexes: Vec<Index>,
    /// `Checks { ... }` block content, verbatim (their importer emits it).
    pub checks: Option<String>,
    /// The block parsed into entries (their check interpretation: each
    /// field is a backticked function expression with optional settings).
    pub check_entries: Vec<TableCheck>,
    /// `records { ... }` sample rows (in-table or top-level, attached to
    /// the named table).
    pub records: Option<TableRecords>,
    /// `Table t [owner: "data-team", ...]` — dbdiagram custom properties,
    /// shown on the header hover card.
    pub custom: Vec<(String, String)>,
    /// Each custom entry's attribute span (key..value end).
    pub custom_spans: Vec<(String, Span)>,
}

/// One `records` block: sample rows laid out against `columns` (a table's
/// columns, in definition order when the header list is omitted).
#[derive(Debug, Clone)]
pub struct TableRecords {
    /// Whole `records … { … }` block.
    pub span: Span,
    /// Header column list; empty for the implicit all-columns form (the
    /// model resolves it against the table).
    pub columns: Vec<String>,
    /// Each header column's source span, parallel to `columns` (the
    /// missing-column anchors — their binder errs at the `columnBindee`,
    /// the primary expression in the records header).
    pub column_spans: Vec<Span>,
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
    /// The type's end offset (before any settings) — their column-node
    /// lint anchors read [name start .. type end].
    pub ty_end: usize,
    /// The column NODE's end (their union of the arg spans): the last real
    /// arg's end — the settings `]` when a list binds, else the type end.
    /// Trailing junk (a following `}`) never extends a node span.
    pub full_end: usize,
    pub span: Span,
    pub settings: ColumnSettings,
    /// Their validateNode-class findings raised while parsing this
    /// column (the inline-settings field errors) — they gate the
    /// interpretation layer like every binder error. The anchors carry
    /// the offending TOKEN spans.
    pub binder_errors: Vec<(usize, usize, String)>,
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
    /// Their validateNode-class findings raised while parsing this
    /// column (the inline-settings field errors) — they gate the
    /// interpretation layer like every binder error.
    pub binder_errors: Vec<(usize, String)>,
    pub inline_ref: Option<InlineRef>,
    /// Arbitrary `key: "value"` pairs dbdiagram calls custom properties
    /// (`[masking: "partial"]`): any unrecognized key with a value, shown
    /// on the diagram hover cards.
    pub custom: Vec<(String, String)>,
    /// Every key occurrence — feeds their duplicate/conflict settings
    /// rules.
    pub occurrences: Vec<SettingOcc>,
}

/// One settings-list key occurrence with its token — the duplicate /
/// conflict rules flag each occurrence individually (their attribute
/// nodes).
#[derive(Debug, Clone)]
pub struct SettingOcc {
    /// Lowercased key as written (`pk`, `not null`, …).
    pub key: String,
    /// Byte offset of the key token.
    pub offset: usize,
    /// Offset just past the attribute (their getTokenPosition end —
    /// the settings-entry tokens).
    pub end: usize,
    pub has_value: bool,
    /// The verbatim value text (quotes kept) when the value parsed as a
    /// plain string.
    pub value: Option<String>,
    /// The VALUE node's span — their settings-note checks anchor the
    /// value expression (the merged application after a swallow).
    pub value_span: Span,
    /// The value was a quoted string (their isExpressionAQuotedString).
    pub quoted: bool,
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

/// One `Checks { }` entry: the backticked expression plus an optional
/// `[name: '…']`.
#[derive(Debug, Clone, PartialEq)]
pub struct TableCheck {
    pub expression: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Index {
    pub columns: Vec<String>,
    /// Parallel to `columns`: true when the entry was a backticked
    /// functional expression (their index-column `type: "expression"`).
    pub expressions: Vec<bool>,
    /// Parallel to `columns`: each entry's source span (their index-column
    /// tokens — quotes included).
    pub column_spans: Vec<Span>,
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
    /// Their INVALID_REF_FIELD findings on a bound ref (application args
    /// between the endpoints) — binder-class, gate the infos.
    pub binder_errors: Vec<(usize, String)>,
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
    /// The strict parse found a clean binary relationship (`a.b > c.d`
    /// with no op-leading junk). Their checkStandaloneRefs skips refs
    /// whose tree callee is not `isBinaryRelationship` — they leave the
    /// same-endpoint registry AND their interpret-layer checks alone
    /// (the file errors gate the interpretation).
    pub clean_binary: bool,
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
    /// Their anchors carry the FRAGMENT nodes: the missing-table
    /// diagnostic anchors the table token, the missing-column one each
    /// column token (fuzz-span parity).
    pub table_span: Span,
    pub column_spans: Vec<Span>,
}

/// A ref-body binder request: a dotted chain (schema..., table, column)
/// with each fragment's source anchor, plus optional tuple columns at
/// the chain end (`t.(a, b)`). Emitted by the ref-body pipeline's
/// bindFields scan; validate.rs resolves the chain against the document
/// per their nodeRefereeOfRefEndpoint.
#[derive(Debug, Clone)]
pub struct BinderChain {
    /// (name, start, end) — their anchors carry the fragment TOKEN spans.
    pub vars: Vec<(String, usize, usize)>,
    pub tuple: Vec<(String, usize, usize)>,
    /// TableGroup members resolve with their own nodeReferee
    /// (nodeRefereeOfTableGroupField): one fragment per bindee, the
    /// schema context from depth 2, and the unresolved-left fall-through
    /// to a schema lookup
    pub group: bool,
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
    /// The note's settings-entry token (their attribute node).
    pub note_span: Option<Span>,
    /// The value's full extent: through the settings list when present
    /// (their enum-field token spans it).
    pub full_end: usize,
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
    /// The `Note:` block-entry token.
    pub note_span: Option<Span>,
    /// Any other `key: "value"` header settings — dbdiagram custom
    /// properties, in order.
    pub custom: Vec<(String, String)>,
    /// Each custom entry's attribute span (key..value end), keyed — the
    /// settings-entry tokens the schema contract carries.
    pub custom_spans: Vec<(String, Span)>,
}

/// A top-level sticky note: `Note name { '''text''' }` — renders as a
/// yellow sticky on the diagram canvas.
#[derive(Debug, Clone)]
pub struct StickyNote {
    pub name: String,
    pub span: Span,
    pub text: String,
}
