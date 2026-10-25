//! SQL → DBML import (a Rust port of `@dbml/core`'s importers).
//!
//! A tokenizer plus DDL statement parsers (CREATE TABLE/INDEX/TYPE,
//! ALTER TABLE, COMMENT ON) rebuild a schema model; constraints scattered
//! across statements are merged into their tables before emitting DBML.
//! Correctness is pinned by the golden fixtures in `tests/dbml_fixtures`
//! (vendored from holistics/dbml, Apache-2.0), compared semantically:
//! both our output and the golden are parsed and canonicalized by our own
//! DBML formatter.

use super::sql::Dialect;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// Identifier or keyword (bare or quoted — quoting is dropped).
    W(String),
    /// Single-quoted string literal (unescaped).
    S(String),
    /// Number literal.
    N(String),
    /// Punctuation.
    P(char),
}

impl Tok {
    fn is_w(&self, kw: &str) -> bool {
        matches!(self, Tok::W(w) if w.eq_ignore_ascii_case(kw))
    }

    fn text(&self) -> String {
        match self {
            Tok::W(w) => w.clone(),
            Tok::N(n) => n.clone(),
            Tok::S(v) => v.clone(),
            Tok::P(c) => c.to_string(),
        }
    }
}

fn tokenize(src: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '-' if i + 1 < b.len() && b[i + 1] == '-' => {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            }
            '/' if i + 1 < b.len() && b[i + 1] == '*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            '\'' => {
                let (s, next) = read_string(&b, i)?;
                out.push(Tok::S(s));
                i = next;
            }
            '"' | '`' => {
                let close = c;
                let mut s = String::new();
                i += 1;
                while i < b.len() {
                    if b[i] == close {
                        // "" doubling escapes inside double quotes
                        if close == '"' && i + 1 < b.len() && b[i + 1] == '"' {
                            s.push('"');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    s.push(b[i]);
                    i += 1;
                }
                out.push(Tok::W(s));
            }
            '[' => {
                // mssql bracketed identifier
                let mut s = String::new();
                i += 1;
                while i < b.len() && b[i] != ']' {
                    s.push(b[i]);
                    i += 1;
                }
                i += 1;
                out.push(Tok::W(s));
            }
            '0'..='9' => {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.' || b[i] == '-') {
                    // stop '-' unless part of a number context (rare in DDL)
                    if b[i] == '-' && !(i > start && b[i - 1] == 'e') {
                        break;
                    }
                    i += 1;
                }
                out.push(Tok::N(b[start..i].iter().collect()));
            }
            c if c.is_alphanumeric() || c == '_' || c == '#' || c == '$' => {
                let start = i;
                while i < b.len()
                    && (b[i].is_alphanumeric() || b[i] == '_' || b[i] == '#' || b[i] == '$')
                {
                    i += 1;
                }
                out.push(Tok::W(b[start..i].iter().collect()));
            }
            other => {
                out.push(Tok::P(other));
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Read a `'...'` string starting at `i`; '' doubles as an escaped quote.
fn read_string(b: &[char], mut i: usize) -> Result<(String, usize), String> {
    i += 1; // opening quote
    let mut s = String::new();
    while i < b.len() {
        if b[i] == '\'' {
            if i + 1 < b.len() && b[i + 1] == '\'' {
                s.push('\'');
                i += 2;
                continue;
            }
            return Ok((s, i + 1));
        }
        s.push(b[i]);
        i += 1;
    }
    Err("unterminated string literal".into())
}

/// Split the token stream into `;`-terminated statements.
fn statements(toks: &[Tok]) -> Vec<&[Tok]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        if *t == Tok::P(';') {
            if i > start {
                out.push(&toks[start..i]);
            }
            start = i + 1;
        }
    }
    if start < toks.len() {
        out.push(&toks[start..]);
    }
    out
}

// ---- importer model ----

#[derive(Debug, Default, Clone)]
struct ICol {
    name: String,
    ty: String,
    pk: bool,
    increment: bool,
    unique: bool,
    not_null: bool,
    default: Option<String>,
    note: Option<String>,
    /// Inline `REFERENCES t (c)` — merged like standalone FKs.
    fk: Option<(String, Vec<String>)>,
    check: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct ITable {
    schema: Option<String>,
    name: String,
    cols: Vec<ICol>,
    note: Option<String>,
}

#[derive(Debug)]
struct Importer {
    dialect: Dialect,
    tables: Vec<ITable>,
    enums: Vec<(Option<String>, String, Vec<String>)>,
    /// (fk_table, fk_cols, ref_table, ref_cols)
    fks: Vec<(String, Vec<String>, String, Vec<String>)>,
    /// Uniques/PKs to merge: (table, cols, is_pk)
    pending_uniques: Vec<(String, Vec<String>, bool)>,
}

pub fn import(source: &str, dialect: Dialect) -> Result<String, String> {
    let toks = tokenize(source)?;
    let mut imp = Importer {
        dialect,
        tables: Vec::new(),
        enums: Vec::new(),
        fks: Vec::new(),
        pending_uniques: Vec::new(),
    };
    for stmt in statements(&toks) {
        imp.statement(stmt);
    }
    imp.finish();
    Ok(imp.emit())
}

impl Importer {
    fn statement(&mut self, s: &[Tok]) {
        if s.is_empty() {
            return;
        }
        if s[0].is_w("CREATE") {
            self.create(s);
        } else if s[0].is_w("ALTER") {
            self.alter(s);
        } else if s[0].is_w("COMMENT") {
            self.comment(s);
        }
        // INSERT / SET / BEGIN / COMMIT / DROP / ANALYZE …: ignored
    }

    fn create(&mut self, s: &[Tok]) {
        // CREATE [OR REPLACE] [TEMP|TEMPORARY|UNLOGGED|GLOBAL TEMPORARY|…]
        // find the object keyword (CREATE [OR REPLACE] [TEMP …] TABLE …)
        let kind = s
            .iter()
            .position(|t| {
                t.is_w("TABLE")
                    || t.is_w("INDEX")
                    || t.is_w("TYPE")
                    || t.is_w("VIEW")
                    || t.is_w("PROCEDURE")
                    || t.is_w("FUNCTION")
                    || t.is_w("TRIGGER")
                    || t.is_w("SEQUENCE")
                    || t.is_w("SCHEMA")
                    || t.is_w("EXTENSION")
                    || t.is_w("DATABASE")
            })
            .unwrap_or(s.len());
        let Some(kw) = s.get(kind) else { return };
        let rest = &s[kind + 1..];
        if kw.is_w("TABLE") {
            self.create_table(rest);
        } else if kw.is_w("INDEX") {
            self.create_index(rest);
        } else if kw.is_w("TYPE") {
            self.create_type(rest);
        }
        // views/procs/triggers/sequences/schemas: ignored
    }

    /// `[IF NOT EXISTS] [schema.]name`
    fn qual_name(&self, s: &[Tok]) -> (Option<String>, String, usize) {
        let mut i = 0;
        if i < s.len() && s[i].is_w("IF") {
            i += 3; // IF NOT EXISTS
        }
        let Some(Tok::W(first)) = s.get(i) else {
            return (None, String::new(), i);
        };
        let first = first.clone();
        i += 1;
        if i + 1 < s.len() && s[i] == Tok::P('.') {
            if let Tok::W(second) = &s[i + 1] {
                return (Some(first), second.clone(), i + 2);
            }
        }
        (None, first, i)
    }

    fn create_table(&mut self, s: &[Tok]) {
        let (schema, name, mut i) = self.qual_name(s);
        let Some(Tok::P('(')) = s.get(i) else { return };
        i += 1;
        let mut table = ITable {
            schema,
            name,
            ..Default::default()
        };
        // parse items until the matching ')'
        let mut depth = 1;
        while i < s.len() {
            if s[i] == Tok::P(')') && depth == 0 {
                break;
            }
            if s[i] == Tok::P('(') {
                depth += 1;
            }
            if s[i] == Tok::P(')') {
                depth -= 1;
            }
            // table-level constraint?
            if self.constraint_kw(s, i).is_some() {
                let consumed = self.table_constraint(&s[i..], &table.name);
                if consumed > 0 {
                    i += consumed;
                    if i < s.len() && s[i] == Tok::P(',') {
                        i += 1;
                    }
                    continue;
                }
            }
            // column def: name type... until ',' at depth 1
            let start = i;
            let end = item_end(s, i);
            if end > start {
                self.column_def(&mut table, &s[start..end]);
            }
            i = end;
            if i < s.len() && s[i] == Tok::P(',') {
                i += 1;
            }
        }
        self.tables.push(table);
    }

    /// Is a table-level CONSTRAINT keyword at `i`? Returns its length hint.
    fn constraint_kw(&self, s: &[Tok], i: usize) -> Option<usize> {
        let kw = |j: usize, w: &str| s.get(j).is_some_and(|t| t.is_w(w));
        if kw(i, "CONSTRAINT") {
            return Some(2); // CONSTRAINT name <kind>
        }
        for k in ["PRIMARY", "FOREIGN", "UNIQUE", "CHECK", "KEY"] {
            if kw(i, k) {
                return Some(1);
            }
        }
        None
    }

    /// Parse a table-level constraint; returns consumed token count.
    fn table_constraint(&mut self, s: &[Tok], table: &str) -> usize {
        let mut i = 0;
        let mut name: Option<String> = None;
        if s.first().is_some_and(|t| t.is_w("CONSTRAINT")) {
            i = 1;
            if let Some(Tok::W(n)) = s.get(i) {
                name = Some(n.clone());
                i += 1;
            }
        }
        let Some(t) = s.get(i) else { return 0 };
        let mut cols = Vec::new();
        let mut ref_table = String::new();
        let mut ref_cols = Vec::new();
        let mut consumed = 0;
        if t.is_w("PRIMARY") || t.is_w("KEY") || t.is_w("UNIQUE") {
            let is_pk = t.is_w("PRIMARY") || t.is_w("KEY");
            i += if t.is_w("PRIMARY") { 2 } else { 1 };
            if let Some(Tok::P('(')) = s.get(i) {
                i += 1;
                (cols, i) = ident_list(s, i);
            }
            if !cols.is_empty() {
                self.pending_uniques.push((table.to_string(), cols, is_pk));
            }
            consumed = i;
        } else if t.is_w("FOREIGN") {
            // FOREIGN KEY (cols) REFERENCES t (cols)
            i += 2;
            if let Some(Tok::P('(')) = s.get(i) {
                i += 1;
                (cols, i) = ident_list(s, i);
            }
            if let Some(Tok::P('(')) = s.get(i) {
                i += 1;
                (ref_cols, i) = ident_list(s, i);
            }
            if let Some(Tok::W(r)) = s.get(i) {
                ref_table = r.clone();
            }
            if !cols.is_empty() && !ref_table.is_empty() {
                self.fks
                    .push((table.to_string(), cols, ref_table, ref_cols));
            }
            consumed = i + 1; // past the table name token
            let _ = name;
        }
        consumed
    }

    fn column_def(&mut self, table: &mut ITable, s: &[Tok]) {
        let Some(Tok::W(name)) = s.first() else {
            return;
        };
        let mut col = ICol {
            name: name.clone(),
            ..Default::default()
        };
        // type: tokens until a constraint keyword
        let mut ty: Vec<String> = Vec::new();
        let mut i = 1;
        while i < s.len() {
            let t = &s[i];
            let stop = matches!(t, Tok::W(w)
                if w.eq_ignore_ascii_case("NOT")
                    || w.eq_ignore_ascii_case("NULL")
                    || w.eq_ignore_ascii_case("DEFAULT")
                    || w.eq_ignore_ascii_case("PRIMARY")
                    || w.eq_ignore_ascii_case("UNIQUE")
                    || w.eq_ignore_ascii_case("REFERENCES")
                    || w.eq_ignore_ascii_case("CHECK")
                    || w.eq_ignore_ascii_case("CONSTRAINT")
                    || w.eq_ignore_ascii_case("COMMENT")
                    || w.eq_ignore_ascii_case("AUTO_INCREMENT")
                    || w.eq_ignore_ascii_case("IDENTITY")
                    || w.eq_ignore_ascii_case("GENERATED")
                    || w.eq_ignore_ascii_case("UNSIGNED")
                    || w.eq_ignore_ascii_case("SIGNED")
                    || w.eq_ignore_ascii_case("ZEROFILL")
                    || w.eq_ignore_ascii_case("ON")
                    || w.eq_ignore_ascii_case("COLLATE")
                    || w.eq_ignore_ascii_case("CHARACTER")
                    || w.eq_ignore_ascii_case("ENCODED")
                    || w.eq_ignore_ascii_case("NULLS"));
            if stop || *t == Tok::P(',') {
                break;
            }
            if let Tok::W(w) = t {
                ty.push(w.clone());
            } else if let Tok::N(n) = t {
                ty.push(n.clone());
            }
            i += 1;
        }
        col.ty = normalize_type(&ty.join(" "), self.dialect);
        // options
        while i < s.len() {
            let t = &s[i];
            if *t == Tok::P(',') {
                break;
            }
            if t.is_w("NOT") && s.get(i + 1).is_some_and(|x| x.is_w("NULL")) {
                col.not_null = true;
                i += 2;
            } else if t.is_w("NULL") {
                i += 1;
            } else if t.is_w("PRIMARY") || t.is_w("SERIAL") {
                col.pk = true;
                i += if t.is_w("PRIMARY") { 2 } else { 1 };
            } else if t.is_w("AUTO_INCREMENT") {
                col.increment = true;
                i += 1;
            } else if t.is_w("IDENTITY") {
                col.increment = true;
                i += 1;
                if matches!(s.get(i), Some(Tok::P('('))) {
                    i = skip_group(s, i);
                }
            } else if t.is_w("GENERATED") {
                // GENERATED [BY DEFAULT | ALWAYS] AS IDENTITY / serial
                col.increment = true;
                i += 1;
                while i < s.len() && !s[i].is_w("IDENTITY") && !matches!(s[i], Tok::P(',')) {
                    if s[i].is_w("serial")
                        || s[i].is_w("SERIAL")
                        || s[i].is_w("bigserial")
                        || s[i].is_w("smallserial")
                    {
                        col.ty = s[i].text().to_uppercase();
                    }
                    i += 1;
                }
                i += 1;
                if matches!(s.get(i), Some(Tok::P('('))) {
                    i = skip_group(s, i);
                }
            } else if t.is_w("UNIQUE") {
                col.unique = true;
                i += 1;
            } else if t.is_w("DEFAULT") {
                i += 1;
                let (v, next) = default_value(s, i);
                col.default = Some(v);
                i = next;
            } else if t.is_w("REFERENCES") {
                i += 1;
                if let Some(Tok::W(rt)) = s.get(i) {
                    let rt = rt.clone();
                    i += 1;
                    let mut rc = Vec::new();
                    if matches!(s.get(i), Some(Tok::P('('))) {
                        i += 1;
                        (rc, i) = ident_list(s, i);
                    }
                    col.fk = Some((rt, rc));
                }
            } else if t.is_w("COMMENT") {
                i += 1;
                if let Some(Tok::S(c)) = s.get(i) {
                    col.note = Some(c.clone());
                    i += 1;
                }
            } else if t.is_w("CHECK") {
                i += 1;
                if matches!(s.get(i), Some(Tok::P('('))) {
                    let end = skip_group(s, i);
                    col.check = Some(tokens_text(&s[i + 1..end - 1]));
                    i = end;
                }
            } else if t.is_w("CONSTRAINT") {
                i += 2;
            } else if t.is_w("ON") {
                // ON UPDATE/CURRENT/DELETE actions after REFERENCES
                i += 2;
                if i < s.len() && s[i].is_w("CURRENT_TIMESTAMP") {
                    col.default = Some("current_timestamp".into());
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        if col.pk && matches!(self.dialect, Dialect::Postgres) {
            // serial-family types imply generation
            let low = col.ty.to_ascii_lowercase();
            if low.contains("serial") {
                col.increment = true;
            }
        }
        table.cols.push(col);
    }

    fn create_index(&mut self, s: &[Tok]) {
        // CREATE [UNIQUE] INDEX [IF NOT EXISTS] name ON table (cols) ...
        let mut i = 0;
        while i < s.len() && !s[i].is_w("ON") {
            i += 1;
        }
        i += 1;
        let (_schema, table, j) = self.qual_name(&s[i..]);
        i += j;
        let mut cols = Vec::new();
        if matches!(s.get(i), Some(Tok::P('('))) {
            (cols, _) = ident_list(s, i + 1);
        }
        if !cols.is_empty() {
            self.pending_uniques.push((table, cols, false));
        }
        let _ = i;
    }

    fn create_type(&mut self, s: &[Tok]) {
        // CREATE TYPE [schema.]name AS ENUM ( 'a', 'b', ... )
        let (schema, name, mut i) = self.qual_name(s);
        while i < s.len() && !s[i].is_w("ENUM") {
            i += 1;
        }
        i += 1;
        let mut values = Vec::new();
        if matches!(s.get(i), Some(Tok::P('('))) {
            i += 1;
            while i < s.len() && !matches!(s[i], Tok::P(')')) {
                if let Tok::S(v) = &s[i] {
                    values.push(v.clone());
                }
                i += 1;
            }
        }
        if !name.is_empty() {
            self.enums.push((schema, name, values));
        }
    }

    fn alter(&mut self, s: &[Tok]) {
        // ALTER TABLE [schema.]name <action>
        if !s.get(1).is_some_and(|t| t.is_w("TABLE")) {
            return;
        }
        let (schema, name, mut i) = self.qual_name(&s[2..]);
        i += 2;
        let Some(ti) = self.table_index(&schema, &name) else {
            return;
        };
        // ADD [COLUMN] name type... | ADD [CONSTRAINT n] PK/UNIQUE/FK | ALTER COLUMN …
        while i < s.len() {
            if s[i].is_w("ADD") {
                i += 1;
                if s.get(i).is_some_and(|t| t.is_w("COLUMN")) {
                    i += 1;
                    let end = item_end(s, i);
                    if end > i {
                        let mut t = std::mem::take(&mut self.tables[ti]);
                        self.column_def(&mut t, &s[i..end]);
                        self.tables[ti] = t;
                    }
                    i = end;
                    continue;
                }
                let table_name = self.tables[ti].name.clone();
                let consumed = self.table_constraint(&s[i..], &table_name);
                i += consumed.max(1);
            } else if s[i].is_w("ALTER") {
                // ALTER [COLUMN] x SET NOT NULL | SET DEFAULT v | DROP NOT NULL
                i += 2;
                let Some(Tok::W(col)) = s.get(i) else { break };
                let col = col.clone();
                i += 1;
                while i < s.len() {
                    if s[i].is_w("SET") {
                        i += 1;
                        if s.get(i).is_some_and(|t| t.is_w("NOT")) {
                            self.set_not_null(ti, &col, true);
                            i += 2;
                        } else if s.get(i).is_some_and(|t| t.is_w("DEFAULT")) {
                            i += 1;
                            let (v, next) = default_value(s, i);
                            self.set_default(ti, &col, Some(v));
                            i = next;
                        } else {
                            i += 1;
                        }
                    } else if s[i].is_w("DROP") {
                        i += 1;
                        if s.get(i).is_some_and(|t| t.is_w("NOT")) {
                            self.set_not_null(ti, &col, false);
                            i += 2;
                        } else {
                            i += 1;
                        }
                    } else {
                        break;
                    }
                }
            } else {
                i += 1;
            }
        }
        let _ = schema;
    }

    fn comment(&mut self, s: &[Tok]) {
        // COMMENT ON TABLE t IS '...' | COMMENT ON COLUMN t.c IS '...'
        let on_table = s.get(2).is_some_and(|t| t.is_w("TABLE"));
        let (schema, name, mut i) = self.qual_name(&s[3..]);
        i += 3;
        let mut col: Option<String> = None;
        if !on_table {
            if let Some(Tok::W(c)) = s.get(i) {
                col = Some(c.clone());
                i += 1;
            }
        }
        // IS '...'
        while i < s.len() && !s[i].is_w("IS") {
            i += 1;
        }
        i += 1;
        let Some(Tok::S(note)) = s.get(i) else { return };
        let note = note.clone();
        let Some(ti) = self.table_index(&schema, &name) else {
            return;
        };
        match col {
            Some(c) => {
                if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == c) {
                    col.note = Some(note);
                }
            }
            None => self.tables[ti].note = Some(note),
        }
    }

    fn table_index(&mut self, schema: &Option<String>, name: &str) -> Option<usize> {
        self.tables
            .iter()
            .position(|t| t.name == name && (t.schema == *schema || name.is_empty()))
    }

    fn set_not_null(&mut self, ti: usize, col: &str, v: bool) {
        if let Some(c) = self.tables[ti].cols.iter_mut().find(|x| x.name == col) {
            c.not_null = v;
        }
    }

    fn set_default(&mut self, ti: usize, col: &str, v: Option<String>) {
        if let Some(c) = self.tables[ti].cols.iter_mut().find(|x| x.name == col) {
            c.default = v;
        }
    }

    /// Merge pending uniques/PKs and inline FKs into the tables.
    fn finish(&mut self) {
        for (table, cols, is_pk) in std::mem::take(&mut self.pending_uniques) {
            let Some(ti) = self.tables.iter().position(|t| t.name == table) else {
                continue;
            };
            if is_pk {
                for c in &cols {
                    if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == *c) {
                        col.pk = true;
                    }
                }
            } else if cols.len() == 1 {
                if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == cols[0]) {
                    col.unique = true;
                }
            }
            // composite uniques have no single-column DBML equivalent; the
            // golden importer drops them too
        }
        for (fk_table, fk_cols, ref_table, ref_cols) in std::mem::take(&mut self.fks) {
            let Some(ti) = self.tables.iter().position(|t| t.name == fk_table) else {
                continue;
            };
            for (fc, rc) in fk_cols.iter().zip(ref_cols.iter()) {
                if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == *fc) {
                    col.fk = Some((ref_table.clone(), vec![rc.clone()]));
                }
            }
        }
    }

    /// Emit DBML (always quoted identifiers; semantic comparison makes the
    /// exact quoting style irrelevant).
    fn emit(&self) -> String {
        let mut out = String::new();
        let q = |n: &str| format!("\"{n}\"");
        for (schema, name, values) in &self.enums {
            if let Some(s) = schema {
                out.push_str(&format!("Enum {}.{} {{\n", q(s), q(name)));
            } else {
                out.push_str(&format!("Enum {} {{\n", q(name)));
            }
            for v in values {
                out.push_str(&format!("  {}\n", q(v)));
            }
            out.push_str("}\n\n");
        }
        for t in &self.tables {
            let head = match &t.schema {
                Some(s) => format!("Table {}.{} {{\n", q(s), q(&t.name)),
                None => format!("Table {} {{\n", q(&t.name)),
            };
            out.push_str(&head);
            for c in &t.cols {
                let mut settings = Vec::new();
                if c.pk {
                    settings.push("pk".into());
                }
                if c.increment {
                    settings.push("increment".into());
                }
                if c.unique {
                    settings.push("unique".into());
                }
                if c.not_null {
                    settings.push("not null".into());
                }
                if let Some(d) = &c.default {
                    settings.push(format!("default: {d}"));
                }
                if let Some((rt, rc)) = &c.fk {
                    let rcs = rc.iter().map(|x| q(x)).collect::<Vec<_>>().join(", ");
                    if rc.len() == 1 {
                        settings.push(format!("ref: > {}.{}", q(rt), rcs));
                    } else {
                        settings.push(format!("ref: > {}.({})", q(rt), rcs));
                    }
                }
                if let Some(n) = &c.note {
                    settings.push(format!("note: '{}'", n.replace('\'', "''")));
                }
                let ty = if c.ty.is_empty() {
                    "int".to_string()
                } else {
                    c.ty.clone()
                };
                out.push_str(&format!("  {} {}", q(&c.name), ty));
                if !settings.is_empty() {
                    out.push_str(&format!(" [{}]", settings.join(", ")));
                }
                out.push('\n');
            }
            if let Some(n) = &t.note {
                out.push_str(&format!("\n  Note: '{}'\n", n.replace('\'', "''")));
            }
            out.push_str("}\n\n");
        }
        out
    }
}

/// Tokens until the item end: ',' at depth 0 or the closing ')'.
fn item_end(s: &[Tok], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0i32;
    while i < s.len() {
        match s[i] {
            Tok::P('(') => depth += 1,
            Tok::P(')') => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            Tok::P(',') if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    i
}

/// Skip a balanced (…) group starting at `i`; returns the index AFTER ')'.
fn skip_group(s: &[Tok], i: usize) -> usize {
    let mut depth = 0i32;
    let mut j = i;
    while j < s.len() {
        match s[j] {
            Tok::P('(') => depth += 1,
            Tok::P(')') => {
                depth -= 1;
                if depth == 0 {
                    return j + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    j
}

/// `(a, b, c)` identifier list starting after the '('.
fn ident_list(s: &[Tok], mut i: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    while i < s.len() {
        match &s[i] {
            Tok::W(w) => out.push(w.clone()),
            Tok::P(')') => {
                i += 1;
                break;
            }
            _ => {}
        }
        i += 1;
    }
    (out, i)
}

/// DEFAULT value: string literal verbatim, backtick expr verbatim, bare
/// token (possibly signed number or function call tokens joined).
fn default_value(s: &[Tok], mut i: usize) -> (String, usize) {
    match s.get(i) {
        Some(Tok::S(v)) => (format!("'{}'", v.replace('\'', "''")), i + 1),
        Some(Tok::N(n)) => (n.clone(), i + 1),
        Some(Tok::W(w)) => {
            let mut v = w.clone();
            i += 1;
            // function call: name ( ... )
            if matches!(s.get(i), Some(Tok::P('('))) {
                let end = skip_group(s, i);
                v = format!("`{}`", tokens_text(&s[i..end]).replace(' ', ""));
                i = end;
            }
            (v, i)
        }
        _ => (String::new(), i),
    }
}

fn tokens_text(s: &[Tok]) -> String {
    let mut out = String::new();
    for (i, t) in s.iter().enumerate() {
        if i > 0 {
            if let Tok::W(_) | Tok::N(_) = t {
                out.push(' ');
            }
        }
        match t {
            Tok::W(w) => out.push_str(w),
            Tok::N(n) => out.push_str(n),
            Tok::S(v) => {
                out.push('\'');
                out.push_str(&v.replace('\'', "''"));
                out.push('\'');
            }
            Tok::P(c) => out.push(*c),
        }
    }
    out
}

/// Dialect type normalization (pg/mysql goldens drop time-zone modifiers
/// and map vendor spellings to canonical DBML types).
fn normalize_type(ty: &str, d: Dialect) -> String {
    let mut t = ty.trim().to_string();
    for drop in [
        "WITH TIME ZONE",
        "WITHOUT TIME ZONE",
        "with time zone",
        "without time zone",
    ] {
        t = t.replace(drop, "");
    }
    let low = t.to_ascii_lowercase();
    if matches!(d, Dialect::Postgres) {
        let mapped = match low.as_str() {
            "int4" => "int",
            "int8" => "bigint",
            "int2" => "smallint",
            "float4" => "real",
            "float8" => "double precision",
            "bool" => "boolean",
            "character varying" => "varchar",
            "time without time zone" => "time",
            _ => "",
        };
        if !mapped.is_empty() {
            return mapped.to_string();
        }
    }
    t
}
