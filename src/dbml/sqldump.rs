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
    /// Identifier or keyword; the flag marks quoted idents ("x", `x`, [x])
    /// which must stay quoted when re-emitted inside expressions.
    W(String, bool),
    /// Single-quoted string literal (unescaped).
    S(String),
    /// Number literal.
    N(String),
    /// Punctuation.
    P(char),
}

impl Tok {
    fn is_w(&self, kw: &str) -> bool {
        matches!(self, Tok::W(w, _) if w.eq_ignore_ascii_case(kw))
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
                out.push(Tok::W(s, true));
            }
            '[' if i + 1 < b.len() && b[i + 1] == ']' => {
                // empty brackets: postgres array-type postfix (TEXT[])
                out.push(Tok::P('['));
                out.push(Tok::P(']'));
                i += 2;
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
                out.push(Tok::W(s, true));
            }
            '0'..='9' => {
                let start = i;
                if b[i] == '0'
                    && i + 1 < b.len()
                    && matches!(b[i + 1], 'b' | 'B' | 'o' | 'O' | 'x' | 'X')
                {
                    // radix literals: 0b1010, 0o52, 0x2A
                    i += 2;
                    while i < b.len() && b[i].is_ascii_alphanumeric() {
                        i += 1;
                    }
                } else {
                    while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.' || b[i] == '-') {
                        // stop '-' unless part of a number context (rare in DDL)
                        if b[i] == '-' && !(i > start && b[i - 1] == 'e') {
                            break;
                        }
                        i += 1;
                    }
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
                out.push(Tok::W(b[start..i].iter().collect(), false));
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
    /// the type was quoted in SQL (spaces/symbols) — re-quote in DBML
    ty_quoted: bool,
    pk: bool,
    increment: bool,
    unique: bool,
    not_null: bool,
    default: Option<String>,
    note: Option<String>,
    /// Inline `REFERENCES t (c)` — merged like standalone FKs.
    /// Inline REFERENCES: (schema, table, cols — cols may be empty until
    /// finish() resolves the target's pk).
    fk: Option<(Option<String>, String, Vec<String>)>,
    check: Option<String>,
}

/// One index entry: (columns, name, unique, index_type, pk).
type IndexDef = (Vec<String>, Option<String>, bool, Option<String>, bool);

/// One table-level foreign key: (name, fk_table, fk_cols, ref_schema,
/// ref_table, ref_cols).
type FkDef = (
    Option<String>,
    String,
    Vec<String>,
    Option<String>,
    String,
    Vec<String>,
);

/// A queued PK/UNIQUE constraint merge: (table, cols, is_pk, name, type,
/// declared in a CREATE TABLE body vs ALTER TABLE).
type PendingUnique = (
    String,
    Vec<String>,
    bool,
    Option<String>,
    Option<String>,
    bool,
);

#[derive(Debug, Default, Clone)]
struct ITable {
    schema: Option<String>,
    name: String,
    cols: Vec<ICol>,
    note: Option<String>,
    /// (columns, name, unique, index_type, pk)
    indexes: Vec<IndexDef>,
    /// (constraint name, expression)
    checks: Vec<(Option<String>, String)>,
}

#[derive(Debug)]
struct Importer {
    dialect: Dialect,
    tables: Vec<ITable>,
    enums: Vec<(Option<String>, String, Vec<String>)>,
    /// (name, fk_table, fk_cols, ref_table, ref_cols)
    fks: Vec<FkDef>,
    /// Uniques/PKs to merge (see PendingUnique)
    pending_uniques: Vec<PendingUnique>,
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
            // UNIQUE sits before INDEX: detect from the full statement
            let unique = s[..kind].iter().any(|t| t.is_w("UNIQUE"));
            self.create_index(rest, unique);
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
        let Some(Tok::W(first, _)) = s.get(i) else {
            return (None, String::new(), i);
        };
        let first = first.clone();
        i += 1;
        if i + 1 < s.len() && s[i] == Tok::P('.') {
            if let Tok::W(second, _) = &s[i + 1] {
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
                // bare CHECK constraints belong to the table being built
                // (not yet in self.tables) — route them locally
                // mysql inline KEY/UNIQUE KEY entries belong to the table
                // being built (not yet in self.tables)
                let fulltext = s[i].is_w("FULLTEXT") || s[i].is_w("SPATIAL");
                let is_index_kw = s[i].is_w("KEY")
                    || fulltext
                    || (s[i].is_w("INDEX")
                        && s.get(i + 2).is_some_and(|t| matches!(t, Tok::P('(')))
                        && s.get(i + 3).is_some_and(|t| !matches!(t, Tok::P(')'))));
                let mut key_is_column = false;
                if self.dialect == Dialect::MySQL && is_index_kw {
                    // [FULLTEXT|SPATIAL] KEY|INDEX name [USING t1] (cols)
                    // [USING t2] — the LAST USING wins (golden rule). A
                    // `key` followed by a TYPE is a column named key, not
                    // an index.
                    let mut j = if fulltext { i + 2 } else { i + 1 }; // past KEY
                    let mut iname = None;
                    if let Some(Tok::W(n, _)) = s.get(j) {
                        // only an index when '(' or USING follows the name
                        let nxt = s.get(j + 1);
                        if nxt.is_some_and(|t| matches!(t, Tok::P('(')) || t.is_w("USING")) {
                            iname = Some(n.clone());
                            j += 1;
                        }
                    }
                    if !fulltext
                        && iname.is_none()
                        && s.get(j)
                            .is_some_and(|t| matches!(t, Tok::W(_, _)) && !t.is_w("USING"))
                    {
                        // `key` followed by a type word (not '(' / USING):
                        // a column named `key`, not an index
                        key_is_column = true;
                    }
                    let mut ity = None;
                    let take_using = |j: &mut usize, ity: &mut Option<String>| {
                        if s.get(*j).is_some_and(|t| t.is_w("USING")) {
                            if let Some(Tok::W(t, _)) = s.get(*j + 1) {
                                *ity = Some(t.to_ascii_lowercase());
                            }
                            *j += 2;
                        }
                    };
                    take_using(&mut j, &mut ity);
                    if (iname.is_some() || fulltext) && matches!(s.get(j), Some(Tok::P('('))) {
                        let (icols, jj) = ident_list(s, j + 1);
                        j = jj;
                        take_using(&mut j, &mut ity);
                        while j < s.len() && !matches!(s[j], Tok::P(',')) {
                            j += 1;
                        }
                        if !icols.is_empty() {
                            table.indexes.push((icols, iname, false, ity, false));
                            i = j;
                            if i < s.len() && s[i] == Tok::P(',') {
                                i += 1;
                            }
                            continue;
                        }
                    }
                }
                if s[i].is_w("UNIQUE") && self.dialect == Dialect::MySQL {
                    // UNIQUE KEY name (cols) USING ...
                    let mut j = i + 2; // past UNIQUE KEY
                    let mut iname = None;
                    if let Some(Tok::W(n, _)) = s.get(j) {
                        iname = Some(n.clone());
                        j += 1;
                    }
                    if matches!(s.get(j), Some(Tok::P('('))) {
                        let (icols, jj) = ident_list(s, j + 1);
                        j = jj;
                        let mut ity = None;
                        while j < s.len() && !matches!(s[j], Tok::P(',')) {
                            if s[j].is_w("USING") {
                                if let Some(Tok::W(t, _)) = s.get(j + 1) {
                                    ity = Some(t.to_ascii_lowercase());
                                }
                            }
                            j += 1;
                        }
                        if !icols.is_empty() {
                            table.indexes.push((icols, iname, true, ity, false));
                            i = j;
                            if i < s.len() && s[i] == Tok::P(',') {
                                i += 1;
                            }
                            continue;
                        }
                    }
                }
                if s[i].is_w("CHECK") || s[i].is_w("CONSTRAINT") {
                    // [CONSTRAINT name] CHECK (expr) — belongs to the table
                    // being built (not yet in self.tables)
                    let mut j = i;
                    let mut cname = None;
                    if s[j].is_w("CONSTRAINT") {
                        j += 1;
                        if let Some(Tok::W(n, _)) = s.get(j) {
                            cname = Some(n.clone());
                            j += 1;
                        }
                    }
                    if s.get(j).is_some_and(|t| t.is_w("CHECK")) {
                        j += 1;
                        if matches!(s.get(j), Some(Tok::P('('))) {
                            let end = skip_group(s, j);
                            table.checks.push((cname, tokens_text(&s[j + 1..end - 1])));
                            i = end;
                            if i < s.len() && s[i] == Tok::P(',') {
                                i += 1;
                            }
                            continue;
                        }
                    }
                }
                let consumed = if key_is_column {
                    0 // a column named `key` — parse it as a column below
                } else {
                    self.table_constraint(&s[i..], &table.name, true)
                };
                if consumed > 0 {
                    i += consumed;
                    if i < s.len() && s[i] == Tok::P(',') {
                        i += 1;
                    }
                    continue;
                }
            }
            // LIKE "src" [INCLUDING/EXCLUDING ...] and EXCLUDE USING ...
            // table elements produce no columns — skip the whole element
            if matches!(&s[i], Tok::W(w, false) if w.eq_ignore_ascii_case("LIKE")
                || w.eq_ignore_ascii_case("EXCLUDE"))
            {
                i = item_end(s, i);
                if i < s.len() && s[i] == Tok::P(',') {
                    i += 1;
                }
                continue;
            }
            // column def: name type... until ',' at depth 1
            let start = i;
            let end = item_end(s, i);
            if end > start {
                self.column_def(&mut table, &s[start..end]);
                // inline `REFERENCES t (c)` renders as a top-level
                // unnamed Ref (golden rule), in statement order
                if let Some(col) = table.cols.last_mut() {
                    if col.fk.as_ref().is_some_and(|(_, _, rc)| !rc.is_empty()) {
                        if let Some((rs, rt, rc)) = col.fk.take() {
                            self.fks.push((
                                None,
                                table.name.clone(),
                                vec![col.name.clone()],
                                rs,
                                rt,
                                rc,
                            ));
                        }
                    }
                }
            }
            i = end;
            if i < s.len() && s[i] == Tok::P(',') {
                i += 1;
            }
        }
        // fold table_constraint-queued pendings for THIS table in now, so
        // index order matches the source item order (finish() handles
        // only ALTER-time pendings)
        let pend: Vec<_> = std::mem::take(&mut self.pending_uniques);
        for (tname, cols, is_pk, name, ity, _from_create) in pend {
            if tname != table.name {
                self.pending_uniques
                    .push((tname, cols, is_pk, name, ity, true));
                continue;
            }
            if is_pk && cols.len() == 1 {
                if let Some(col) = table.cols.iter_mut().find(|x| x.name == cols[0]) {
                    col.pk = true;
                }
            } else if is_pk {
                table.indexes.insert(0, (cols, name, false, ity, true));
            } else if cols.len() == 1 && name.is_none() {
                if let Some(col) = table.cols.iter_mut().find(|x| x.name == cols[0]) {
                    col.unique = true;
                }
            } else {
                table.indexes.push((cols, name, true, ity, false));
            }
        }
        // mysql table trailer: ) ... COMMENT[=]'...' or COMMENT "..." —
        // table options live after the column list's closing paren, so
        // find it with a fresh depth scan (item consumed-counts can
        // overshoot the paren)
        let mut depth = 0i32;
        let mut close = s.len();
        for (j, t) in s.iter().enumerate() {
            match t {
                Tok::P('(') => depth += 1,
                Tok::P(')') => {
                    depth -= 1;
                    if depth == 0 {
                        close = j;
                        break;
                    }
                }
                _ => {}
            }
        }
        for j in close..s.len() {
            if !s[j].is_w("COMMENT") {
                continue;
            }
            let mut k = j + 1;
            if s.get(k) == Some(&Tok::P('=')) {
                k += 1;
            }
            // COMMENT 'x' (single-quoted) or COMMENT "x" (double-quoted)
            match s.get(k) {
                Some(Tok::S(c)) | Some(Tok::W(c, true)) if !c.is_empty() => {
                    table.note = Some(c.clone());
                    break;
                }
                _ => {}
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
        for k in [
            "PRIMARY", "FOREIGN", "UNIQUE", "CHECK", "KEY", "INDEX", "FULLTEXT", "SPATIAL",
        ] {
            if kw(i, k) {
                return Some(1);
            }
        }
        None
    }

    /// Parse a table-level constraint; returns consumed token count.
    fn table_constraint(&mut self, s: &[Tok], table: &str, from_create: bool) -> usize {
        let mut i = 0;
        let mut name: Option<String> = None;
        if s.first().is_some_and(|t| t.is_w("CONSTRAINT")) {
            i = 1;
            // `CONSTRAINT PRIMARY KEY ...` (unnamed constraint): the next
            // word is the constraint KIND, not its name
            if let Some(Tok::W(n, _)) = s.get(i) {
                if !n.eq_ignore_ascii_case("PRIMARY")
                    && !n.eq_ignore_ascii_case("FOREIGN")
                    && !n.eq_ignore_ascii_case("UNIQUE")
                    && !n.eq_ignore_ascii_case("CHECK")
                    && !n.eq_ignore_ascii_case("KEY")
                {
                    name = Some(n.clone());
                    i += 1;
                }
            }
        }
        let Some(t) = s.get(i) else { return 0 };
        let mut cols = Vec::new();
        let mut ref_schema: Option<String> = None;
        let mut ref_table = String::new();
        let mut ref_cols = Vec::new();
        let mut consumed = 0;
        if t.is_w("PRIMARY") || t.is_w("KEY") || t.is_w("UNIQUE") {
            let is_pk = t.is_w("PRIMARY") || t.is_w("KEY");
            i += if t.is_w("PRIMARY") { 2 } else { 1 };
            // `UNIQUE KEY (...)`: KEY is a connector, not an index name
            if t.is_w("UNIQUE") && s.get(i).is_some_and(|t| t.is_w("KEY")) {
                i += 1;
            }
            // pg: UNIQUE NULLS [NOT] DISTINCT precedes the column list
            if s.get(i).is_some_and(|t| t.is_w("NULLS")) {
                i += 3; // NULLS [NOT] DISTINCT
            }
            let mut index_type = None;
            // mysql: [CONSTRAINT n] PRIMARY KEY <index_name> [USING t]
            // (cols) — tolerate a stray index name and/or USING type
            // before the column list
            while !matches!(s.get(i), Some(Tok::P('('))) {
                let Some(tok) = s.get(i) else { break };
                if tok.is_w("USING") {
                    if let Some(Tok::W(tyw, _)) = s.get(i + 1) {
                        index_type = Some(tyw.to_ascii_lowercase());
                    }
                    i += 2;
                } else if matches!(tok, Tok::W(_, _))
                    && !tok.is_w("CONSTRAINT")
                    && !tok.is_w("PRIMARY")
                    && !tok.is_w("FOREIGN")
                    && !tok.is_w("UNIQUE")
                    && !tok.is_w("CHECK")
                {
                    if name.is_none() {
                        // bare `CONSTRAINT PRIMARY KEY idx (cols)`: the
                        // stray word is the index name (golden rule)
                        if let Tok::W(w, _) = tok {
                            name = Some(w.clone());
                        }
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            if let Some(Tok::P('(')) = s.get(i) {
                i += 1;
                (cols, i) = ident_list(s, i);
            }
            // optional USING BTREE/HASH, INCLUDE (cols) and other tail
            // tokens — nested groups hold commas, so track depth and stop
            // only at a top-level ',' / ')' / next constraint keyword
            let mut depth = 0i32;
            while i < s.len() {
                match &s[i] {
                    Tok::P('(') => depth += 1,
                    Tok::P(')') => {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    }
                    Tok::P(',') if depth == 0 => break,
                    t if depth == 0
                        && (t.is_w("CONSTRAINT")
                            || t.is_w("PRIMARY")
                            || t.is_w("FOREIGN")
                            || t.is_w("UNIQUE")
                            || t.is_w("CHECK")) =>
                    {
                        break;
                    }
                    _ => {}
                }
                if s[i].is_w("USING") {
                    if let Some(Tok::W(tyw, _)) = s.get(i + 1) {
                        index_type = Some(tyw.to_ascii_lowercase());
                    }
                }
                i += 1;
            }
            if !cols.is_empty() {
                self.pending_uniques.push((
                    table.to_string(),
                    cols.clone(),
                    is_pk,
                    name.clone(),
                    index_type,
                    from_create,
                ));
            }
            consumed = i;
        } else if t.is_w("CHECK") {
            i += 1;
            if matches!(s.get(i), Some(Tok::P('('))) {
                let end = skip_group(s, i);
                if let Some(ti) = self.tables.iter().position(|t| t.name == table) {
                    self.tables[ti]
                        .checks
                        .push((name.clone(), tokens_text(&s[i + 1..end - 1])));
                }
            }
            consumed = i;
        } else if t.is_w("FOREIGN") {
            // FOREIGN KEY (cols) REFERENCES t (cols) [ON DELETE/UPDATE ...]
            i += 2;
            if let Some(Tok::P('(')) = s.get(i) {
                i += 1;
                (cols, i) = ident_list(s, i);
            }
            if s.get(i).is_some_and(|t| t.is_w("REFERENCES")) {
                i += 1;
                // [schema.]table — qualified references carry the schema
                let (rs, rt, used) = self.qual_name(&s[i..]);
                if !rt.is_empty() {
                    ref_schema = rs;
                    ref_table = rt;
                    i += used;
                }
                if let Some(Tok::P('(')) = s.get(i) {
                    i += 1;
                    (ref_cols, i) = ident_list(s, i);
                }
            }
            // skip ON DELETE/UPDATE <action> trailers
            while i < s.len() {
                if s[i].is_w("ON") {
                    i += 3; // ON <ACTION> <action-word>
                } else {
                    break;
                }
            }
            if !cols.is_empty() && !ref_table.is_empty() {
                // REFERENCES t without a column list: the target's pk
                let ref_cols = if ref_cols.is_empty() {
                    self.pk_of(&ref_schema, &ref_table)
                } else {
                    ref_cols
                };
                self.fks.push((
                    name.clone(),
                    table.to_string(),
                    cols,
                    ref_schema,
                    ref_table,
                    ref_cols,
                ));
            }
            consumed = i;
        }
        consumed
    }

    fn column_def(&mut self, table: &mut ITable, s: &[Tok]) {
        let Some(Tok::W(name, _)) = s.first() else {
            return;
        };
        let mut col = ICol {
            name: name.clone(),
            ..Default::default()
        };
        // type: tokens until a constraint keyword
        let mut ty: Vec<String> = Vec::new();
        let mut i = 1;
        let mut type_depth = 0i32;
        let mut array_pending = false;
        while i < s.len() {
            let t = &s[i];
            if *t == Tok::P('(') {
                type_depth += 1;
            }
            let mut close = false;
            if *t == Tok::P(')') {
                if type_depth == 0 {
                    break; // end of the column item
                }
                type_depth -= 1;
                close = true;
            }
            let is_array_kw = matches!(t, Tok::W(w, _) if w.eq_ignore_ascii_case("array"));
            let stop = type_depth == 0
                && !close
                && !is_array_kw
                && (matches!(t, Tok::W(w, _)
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
                    // bare BINARY attribute after a type (varchar(22)
                    // binary) — but `binary` and binary(16) ARE types
                    || (w.eq_ignore_ascii_case("BINARY")
                        && !ty.is_empty()
                        && !matches!(s.get(i + 1), Some(Tok::P('('))))
                    || w.eq_ignore_ascii_case("ON")
                    || w.eq_ignore_ascii_case("COLLATE")
                    || w.eq_ignore_ascii_case("ENCODED")
                    || w.eq_ignore_ascii_case("NULLS"))
                // mysql `... CHARACTER SET x` attribute stops the type —
                // `character varying` is a type and must be collected
                || (t.is_w("CHARACTER")
                    && s.get(i + 1).is_some_and(|x| x.is_w("SET"))));
            if stop || (*t == Tok::P(',') && type_depth == 0) {
                break;
            }
            match t {
                // ARRAY keyword renders as []; a numeric subscript right
                // after it replaces the bare [] with [N]
                Tok::W(w, _) if w.eq_ignore_ascii_case("array") => {
                    array_pending = true;
                    ty.push("[]".into());
                }
                // quoted [N] from the tokenizer (bracket-ident) after a
                // base type is an array subscript
                Tok::W(w, true)
                    if !ty.is_empty() && w.trim().chars().all(|c| c.is_ascii_digit()) =>
                {
                    if array_pending {
                        array_pending = false;
                        ty.pop(); // drop the bare []
                    }
                    ty.push(format!("[{}]", w.trim()));
                }
                Tok::W(w, _) => ty.push(w.clone()),
                Tok::N(n) => ty.push(n.clone()),
                Tok::P('(') if !close => ty.push("(".into()),
                Tok::P(')') => ty.push(")".into()),
                Tok::P(',') if type_depth > 0 => ty.push(",".into()),
                Tok::P('[') => ty.push("[".into()),
                Tok::P(']') => ty.push("]".into()),
                Tok::P('.') => ty.push(".".into()),
                _ => {}
            }
            i += 1;
        }
        let joined = join_type(&ty);
        // mysql inline ENUM('a','b') -> a named enum type
        if self.dialect == Dialect::MySQL
            && s.get(1).is_some_and(|t| t.is_w("ENUM"))
            && matches!(s.get(2), Some(Tok::P('(')))
        {
            let end = skip_group(s, 2);
            let mut values = Vec::new();
            for t in &s[3..end - 1] {
                if let Tok::S(v) = t {
                    values.push(v.clone());
                }
            }
            let enum_name = format!("{}_{}_enum", table.name, name);
            if !values.is_empty() && !self.enums.iter().any(|(_, n, _)| *n == enum_name) {
                self.enums
                    .push((table.schema.clone(), enum_name.clone(), values));
            }
            col.ty = match &table.schema {
                Some(sc) => format!("{sc}.{enum_name}"),
                None => enum_name,
            };
            col.ty_quoted = false;
            // skip the type tokens entirely; options start after the group
            let rest_opts = &s[end..];
            self.column_options(&mut col, rest_opts);
            table.cols.push(col);
            return;
        }
        let normalized_preview = normalize_type(&joined, self.dialect);
        let ty_quoted =
            matches!(s.get(1), Some(Tok::W(_, true))) || normalized_preview.contains(' ');
        col.ty_quoted = ty_quoted || joined.contains('[');
        if joined.contains('[') {
            // array types (TEXT[], timetz(2)[]) are quoted in DBML —
            // never double-quote an already-quoted type; normalize the
            // base type first (goldens drop with/without time zone)
            let inner = normalize_type(joined.trim_matches('"'), self.dialect);
            col.ty = format!("\"{inner}\"");
        } else {
            col.ty = normalize_type(&joined, self.dialect);
        }
        self.column_options(&mut col, &s[i..]);

        if col.pk && matches!(self.dialect, Dialect::Postgres) {
            // serial-family types imply generation
            let low = col.ty.to_ascii_lowercase();
            if low.contains("serial") {
                col.increment = true;
            }
        }
        table.cols.push(col);
    }

    /// Column constraint options after the type (NOT NULL, DEFAULT, ...).
    fn column_options(&mut self, col: &mut ICol, s: &[Tok]) {
        let mut i = 0;
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
                // GENERATED [BY DEFAULT|ALWAYS] AS IDENTITY -> increment;
                // GENERATED ... AS (expr) STORED/VIRTUAL -> computed, skip
                let mut j = i + 1;
                let mut identity = false;
                let mut stored = false;
                let constraint_kw = |t: &Tok| {
                    matches!(t, Tok::W(w, _)
                        if w.eq_ignore_ascii_case("PRIMARY")
                            || w.eq_ignore_ascii_case("NOT")
                            || w.eq_ignore_ascii_case("NULL")
                            || w.eq_ignore_ascii_case("UNIQUE")
                            || w.eq_ignore_ascii_case("DEFAULT")
                            || w.eq_ignore_ascii_case("REFERENCES")
                            || w.eq_ignore_ascii_case("CHECK")
                            || w.eq_ignore_ascii_case("COMMENT")
                            || w.eq_ignore_ascii_case("CONSTRAINT"))
                };
                // stop at real constraint keywords, but only once the
                // IDENTITY token has been seen ("BY DEFAULT AS" inside the
                // clause must not stop the scan)
                while j < s.len() && !matches!(s[j], Tok::P(',')) {
                    if s[j].is_w("IDENTITY") {
                        identity = true;
                    } else if identity && constraint_kw(&s[j]) {
                        break;
                    }
                    if s[j].is_w("STORED") || s[j].is_w("VIRTUAL") {
                        stored = true;
                    }
                    j += 1;
                }
                if identity && !stored {
                    col.increment = true;
                }
                i = j;
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
                let (rs, rt, used) = self.qual_name(&s[i..]);
                if !rt.is_empty() {
                    i += used;
                    let mut rc = Vec::new();
                    if matches!(s.get(i), Some(Tok::P('('))) {
                        i += 1;
                        (rc, i) = ident_list(s, i);
                    }
                    col.fk = Some((rs, rt, rc));
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
                    col.check = Some(format!("`{}`", tokens_text(&s[i + 1..end - 1])));
                    i = end;
                }
            } else if t.is_w("CONSTRAINT") {
                i += 2;
            } else if t.is_w("ON") {
                // ON UPDATE/CURRENT/DELETE actions after REFERENCES
                i += 2;
                if i < s.len() && s[i].is_w("CURRENT_TIMESTAMP") {
                    // with AUTO_INCREMENT the golden drops the default
                    if !col.increment {
                        col.default = Some("`CURRENT_TIMESTAMP`".into());
                    }
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
    }

    fn create_index(&mut self, s: &[Tok], unique: bool) {
        // CREATE [UNIQUE] INDEX [IF NOT EXISTS] name ON table (cols)
        // [USING btree|hash]; `unique` arrives separately (the UNIQUE
        // token precedes INDEX and is stripped from `s`)
        let mut i = 0;
        // optional IF NOT EXISTS, then the index name before ON
        let mut name = None;
        while i < s.len() && !s[i].is_w("ON") {
            if let Tok::W(w, _) = &s[i] {
                if !w.eq_ignore_ascii_case("not") && !w.eq_ignore_ascii_case("exists") {
                    name = Some(w.clone());
                }
            }
            i += 1;
        }
        i += 1;
        if i >= s.len() {
            return;
        }
        let (_schema, table, j) = self.qual_name(&s[i..]);
        i += j;
        let mut cols = Vec::new();
        if matches!(s.get(i), Some(Tok::P('('))) {
            (cols, _) = ident_list(s, i + 1);
        }
        // USING <type> — appears after cols (mysql) or between table and
        // cols (postgres: CREATE INDEX ... ON t USING HASH (cols))
        let mut index_type = None;
        let mut k = 0;
        while k < s.len() {
            if s[k].is_w("USING") {
                if let Some(Tok::W(t, _)) = s.get(k + 1) {
                    index_type = Some(t.to_ascii_lowercase());
                }
            }
            k += 1;
        }
        if cols.is_empty() {
            return;
        }
        let Some(ti) = self
            .tables
            .iter()
            .position(|t| t.name == table && (_schema.is_none() || t.schema == _schema))
        else {
            return;
        };
        // single-column unnamed unique folds onto the column
        if cols.len() == 1 && name.is_none() && unique {
            if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == cols[0]) {
                col.unique = true;
            }
            return;
        }
        self.tables[ti]
            .indexes
            .push((cols, name, unique, index_type, false));
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
        let mut off = 2;
        if s.get(2).is_some_and(|t| t.is_w("ONLY")) {
            off = 3; // pg_dump: ALTER TABLE ONLY schema.table
        }
        let (schema, name, mut i) = self.qual_name(&s[off..]);
        i += off;
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
                let consumed = self.table_constraint(&s[i..], &table_name, false);
                i += consumed.max(1);
            } else if s[i].is_w("ALTER") {
                // ALTER [COLUMN] x SET NOT NULL | SET DEFAULT v | DROP NOT NULL
                i += 2;
                let Some(Tok::W(col, _)) = s.get(i) else {
                    break;
                };
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
        // COLUMN path: [schema.]table.column (2 or 3 idents) — read them
        // explicitly; qual_name would eat table.column as schema.table
        let mut schema = None;
        let name;
        let mut col: Option<String> = None;
        let mut i = 3;
        if on_table {
            let (sc, nm, used) = self.qual_name(&s[3..]);
            schema = sc;
            name = nm;
            i += used;
        } else {
            let mut parts = Vec::new();
            while i < s.len() {
                if let Some(Tok::W(w, _)) = s.get(i) {
                    parts.push(w.clone());
                    i += 1;
                    if matches!(s.get(i), Some(Tok::P('.'))) {
                        i += 1;
                        continue;
                    }
                }
                break;
            }
            match parts.len() {
                2 => {
                    name = parts[0].clone();
                    col = Some(parts[1].clone());
                }
                3 => {
                    schema = Some(parts[0].clone());
                    name = parts[1].clone();
                    col = Some(parts[2].clone());
                }
                _ => return,
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

    /// The pk column(s) of a table (for REFERENCES t without a list).
    fn pk_of(&self, schema: &Option<String>, table: &str) -> Vec<String> {
        self.tables
            .iter()
            .find(|t| t.name == table && t.schema == *schema)
            .map(|t| {
                let pks: Vec<&String> = t
                    .indexes
                    .iter()
                    .filter(|(_, _, _, _, pk)| *pk)
                    .flat_map(|(c, _, _, _, _)| c.iter())
                    .collect();
                if !pks.is_empty() {
                    return pks.into_iter().cloned().collect();
                }
                t.cols
                    .iter()
                    .filter(|c| c.pk)
                    .map(|c| c.name.clone())
                    .collect()
            })
            .unwrap_or_default()
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
        // forward REFERENCES t (no column list): resolve the target's pk
        // now that all tables exist
        // collect resolutions first, apply after (pk_of borrows self)
        let mut fixes: Vec<(usize, usize, Option<Vec<String>>)> = Vec::new();
        for (ti, t) in self.tables.iter().enumerate() {
            for (ci, c) in t.cols.iter().enumerate() {
                if let Some((rs, rt, rc)) = &c.fk {
                    if rc.is_empty() {
                        let resolved = self.pk_of(rs, rt);
                        fixes.push((
                            ti,
                            ci,
                            if resolved.is_empty() {
                                None // dangling: drop like the golden
                            } else {
                                Some(resolved)
                            },
                        ));
                    }
                }
            }
        }
        for (ti, ci, resolved) in fixes {
            if let Some(c) = self.tables.get_mut(ti).and_then(|t| t.cols.get_mut(ci)) {
                match (resolved, &c.fk) {
                    (Some(cols), Some((rs, rt, _))) => {
                        c.fk = Some((rs.clone(), rt.clone(), cols));
                    }
                    (None, _) => c.fk = None,
                    _ => {}
                }
            }
        }
        // merge pending PK/UNIQUE constraints FIRST: bare REFERENCES t
        // resolves against the target's pk, and composite table-level
        // PKs only become visible to pk_of here
        for (table, cols, is_pk, name, ity, from_create) in
            std::mem::take(&mut self.pending_uniques)
        {
            let Some(ti) = self.tables.iter().position(|t| t.name == table) else {
                continue;
            };
            // single-column constraints fold onto the column — except in
            // Postgres CREATE TABLE bodies, where PK/UNIQUE constraints
            // always render as indexes-block entries (golden rule); pg
            // ALTER-added ones fold inline (name dropped)
            let fold = cols.len() == 1
                && match (self.dialect, from_create) {
                    (Dialect::Postgres, true) => false,
                    (Dialect::Postgres, false) => true,
                    _ => is_pk || name.is_none(),
                };
            if fold && is_pk && cols.len() == 1 {
                if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == cols[0]) {
                    col.pk = true;
                }
            } else if fold && !is_pk && cols.len() == 1 && name.is_none() {
                if let Some(col) = self.tables[ti].cols.iter_mut().find(|x| x.name == cols[0]) {
                    col.unique = true;
                }
            } else if is_pk {
                // composite PK: an indexes-block [pk] entry (golden rule)
                self.tables[ti]
                    .indexes
                    .insert(0, (cols, name, false, ity, true));
            } else {
                // composite/named constraints render as an indexes block
                self.tables[ti]
                    .indexes
                    .push((cols, name, !is_pk, ity, false));
            }
        }
        let mut fks = std::mem::take(&mut self.fks);
        for fk in &mut fks {
            if fk.5.is_empty() {
                fk.5 = self.pk_of(&fk.3, &fk.4);
            }
        }
        // drop unresolvable FKs (no target table / no pk) — emitting a
        // dangling reference would break the output
        fks.retain(|fk| !fk.5.is_empty());
        self.fks = fks;
    }

    /// Emit DBML (always quoted identifiers; semantic comparison makes the
    /// exact quoting style irrelevant).
    fn emit(&self) -> String {
        let mut out = String::new();
        let q = |n: &str| format!("\"{n}\"");
        // named FK constraints -> top-level Ref statements (golden style)
        for (name, fk_table, fk_cols, ref_schema, ref_table, ref_cols) in &self.fks {
            let fks = fk_cols.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
            let rcs = ref_cols.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
            let (f, r) = if fk_cols.len() == 1 {
                (fks.clone(), rcs.clone())
            } else {
                (format!("({fks})"), format!("({rcs})"))
            };
            let ref_head = match ref_schema {
                Some(sc) => format!("{}.{}", q(sc), q(ref_table)),
                None => q(ref_table),
            };
            match name {
                Some(n) => out.push_str(&format!(
                    "Ref {}: {}.{} < {}.{}\n\n",
                    q(n),
                    ref_head,
                    r,
                    q(fk_table),
                    f
                )),
                None => out.push_str(&format!(
                    "Ref: {}.{} < {}.{}\n\n",
                    ref_head,
                    r,
                    q(fk_table),
                    f
                )),
            }
        }
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
        // golden rule: default-schema tables first (creation order), then
        // other schemas' tables in creation order
        let mut ordered: Vec<&ITable> = self.tables.iter().collect();
        ordered.sort_by_key(|t| t.schema.is_some());
        for t in ordered {
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
                    if d.starts_with("`nextval(") {
                        // sequence-driven serials: the golden drops the
                        // default entirely
                    } else if d.eq_ignore_ascii_case("null") {
                        // DEFAULT NULL is the default: omitted
                    } else if d.eq_ignore_ascii_case("current_timestamp") {
                        settings.push("default: `CURRENT_TIMESTAMP`".into());
                    } else {
                        settings.push(format!("default: {d}"));
                    }
                }
                if let Some((rs, rt, rc)) = &c.fk {
                    let rcs = rc.iter().map(|x| q(x)).collect::<Vec<_>>().join(", ");
                    let head = match rs {
                        Some(sc) => format!("{}.{}", q(sc), q(rt)),
                        None => q(rt),
                    };
                    if rc.len() == 1 {
                        settings.push(format!("ref: > {}.{}", head, rcs));
                    } else {
                        settings.push(format!("ref: > {}.({})", head, rcs));
                    }
                }
                if let Some(n) = &c.note {
                    settings.push(format!("note: '{}'", n.replace('\'', "''")));
                }
                if let Some(ch) = &c.check {
                    settings.push(format!("check: {ch}"));
                }
                let ty = if c.ty.is_empty() {
                    "int".to_string()
                } else if c.ty.starts_with('"') && c.ty.ends_with('"') {
                    c.ty.clone() // already quoted (arrays/pre-quoted)
                } else if c.ty_quoted {
                    q(&c.ty)
                } else {
                    c.ty.clone()
                };
                out.push_str(&format!("  {} {}", q(&c.name), ty));
                if !settings.is_empty() {
                    out.push_str(&format!(" [{}]", settings.join(", ")));
                }
                out.push('\n');
            }
            if !t.indexes.is_empty() {
                out.push_str("\n  indexes {\n");
                for (cols, name, unique, ty, pk) in &t.indexes {
                    let col_list = cols.iter().map(|c| q(c)).collect::<Vec<_>>().join(", ");
                    let mut settings = Vec::new();
                    if *pk {
                        settings.push("pk".into());
                    }
                    if *unique {
                        settings.push("unique".into());
                    }
                    if let Some(n) = name {
                        settings.push(format!("name: '{}'", n.replace('\'', "''")));
                    }
                    if let Some(ty) = ty {
                        settings.push(format!("type: {ty}"));
                    }
                    let suffix = if settings.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", settings.join(", "))
                    };
                    out.push_str(&format!("    ({col_list}){suffix}\n"));
                }
                out.push_str("  }\n");
            }
            if !t.checks.is_empty() {
                out.push_str("\n  Checks {\n");
                for (name, expr) in &t.checks {
                    let suffix = name
                        .as_ref()
                        .map(|n| format!(" [name: '{}']", n.replace('\'', "''")))
                        .unwrap_or_default();
                    out.push_str(&format!("    `{expr}`{suffix}\n"));
                }
                out.push_str("  }\n");
            }
            if let Some(n) = &t.note {
                out.push_str(&format!("  note: '{}'\n", n.replace('\'', "''")));
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
            Tok::W(w, _) => out.push(w.clone()),
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
    if matches!(s.get(i), Some(Tok::P('('))) {
        // parenthesized expression: backtick the group's content
        let end = skip_group(s, i);
        let expr = tokens_text(&s[i + 1..end - 1]).replace(' ', "");
        return (format!("`{expr}`"), end);
    }
    match s.get(i) {
        Some(Tok::S(v)) => (format!("'{}'", v.replace('\'', "''")), i + 1),
        Some(Tok::N(n)) => (parse_int_literal(n), i + 1),
        Some(Tok::W(w, _)) => {
            let mut v = w.clone();
            i += 1;
            // dotted name: public.uuid_generate_v1()
            while matches!(s.get(i), Some(Tok::P('.'))) {
                if let Some(Tok::W(next, _)) = s.get(i + 1) {
                    v.push('.');
                    v.push_str(next);
                    i += 2;
                } else {
                    break;
                }
            }
            // function call: name(...)
            if matches!(s.get(i), Some(Tok::P('('))) {
                let end = skip_group(s, i);
                let args = tokens_text(&s[i + 1..end - 1]).replace(' ', "");
                v = format!("`{v}({args})`");
                i = end;
            }
            (v, i)
        }
        _ => (String::new(), i),
    }
}

/// Render a numeric literal: 0b/0o/0x and leading-zero forms convert to
/// their decimal value (golden rule from integer_literals).
fn parse_int_literal(n: &str) -> String {
    let lower = n.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("0b") {
        if let Ok(v) = i64::from_str_radix(rest, 2) {
            return v.to_string();
        }
    }
    if let Some(rest) = lower.strip_prefix("0x") {
        if let Ok(v) = i64::from_str_radix(rest, 16) {
            return v.to_string();
        }
    }
    if let Some(rest) = lower.strip_prefix("0o") {
        if let Ok(v) = i64::from_str_radix(rest, 8) {
            return v.to_string();
        }
    }
    if lower.len() > 1 && lower.starts_with('0') && lower.chars().all(|c| c.is_ascii_digit()) {
        // golden rule: leading zeros drop (decimal reading)
        return lower.trim_start_matches('0').to_string();
    }
    n.to_string()
}

fn tokens_text(s: &[Tok]) -> String {
    let mut out = String::new();
    for (i, t) in s.iter().enumerate() {
        let _ = i;
        if i > 0 {
            // space before word/number tokens, and before opening parens
            // when they follow a word (LOWER(x)); never inside call groups
            let needs_space = match t {
                Tok::W(_, _) | Tok::N(_) | Tok::S(_) => !out.ends_with(',') && !out.ends_with('('),
                Tok::P('(') => out.ends_with(|c: char| c.is_alphanumeric() || c == '_'),
                Tok::P('>' | '<' | '=' | '~') => {
                    !out.ends_with(' ') && !out.ends_with(['>', '<', '=', '~'])
                }
                Tok::P(',') => false,
                _ => false,
            };
            if needs_space && !out.ends_with(' ') {
                out.push(' ');
            }
        }
        match t {
            Tok::W(w, quoted) => {
                if *quoted {
                    out.push_str(&format!("\"{w}\""));
                } else {
                    out.push_str(w);
                }
            }
            Tok::N(n) => out.push_str(n),
            Tok::S(v) => {
                out.push('\'');
                out.push_str(&v.replace('\'', "''"));
                out.push('\'');
            }
            Tok::P(c) => {
                out.push(*c);
                if *c == ',' {
                    out.push(' '); // golden style: space after commas
                }
            }
        }
    }
    out
}

/// Join collected type tokens: no spaces inside parens, single spaces
/// between words (`int ( 11 )` -> `int(11)`).
fn join_type(toks: &[String]) -> String {
    let mut out = String::new();
    for t in toks {
        match t.as_str() {
            "(" | ")" | "," | "." | "[" | "]" => out.push_str(t),
            _ => {
                if !out.is_empty()
                    && !out.ends_with('(')
                    && !out.ends_with(',')
                    && !out.ends_with('.')
                    && !out.ends_with('[')
                    && !t.starts_with('[')
                {
                    out.push(' ');
                }
                out.push_str(t);
            }
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
    if matches!(d, Dialect::Postgres) {
        // phrase replacements that keep any (n,m) suffix
        for (from, to) in [
            ("int4", "int"),
            ("int8", "bigint"),
            ("int2", "smallint"),
            ("float4", "real"),
            ("float8", "double precision"),
            ("bool", "boolean"),
        ] {
            let lower = t.to_ascii_lowercase();
            if let Some(pos) = lower.find(from) {
                let after = pos + from.len();
                // word-boundary: 'bool' must not match inside 'boolean'
                let boundary = lower[..after]
                    .chars()
                    .next_back()
                    .map_or(true, |c| !c.is_alphanumeric())
                    || lower[after..]
                        .chars()
                        .next()
                        .map_or(true, |c| !c.is_alphanumeric());
                if boundary {
                    t = format!("{}{}{}", &t[..pos], to, &t[after..]);
                }
            }
        }
    }
    t.trim().to_string()
}
