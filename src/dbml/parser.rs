//! Hand-written recursive-descent DBML parser.
//!
//! Design goals: byte-accurate spans for editor integration, and error
//! recovery at statement / block-line granularity so partially typed
//! documents still render while editing.

use super::ast::*;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub offset: usize,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
pub struct ParseOutput {
    pub doc: Document,
    pub errors: Vec<ParseError>,
}

struct Fail {
    offset: usize,
    message: &'static str,
}

type R<T> = Result<T, Fail>;

fn offset(input: &str, base: usize) -> usize {
    input.as_ptr() as usize - base
}

fn fail<T>(o: usize, m: &'static str) -> R<T> {
    Err(Fail {
        offset: o,
        message: m,
    })
}

fn err_at(o: usize, m: &str) -> ParseError {
    ParseError {
        offset: o,
        message: m.to_string(),
    }
}

// --- tiny lexing helpers -------------------------------------------------

/// Spaces and tabs only; never crosses a line break (keeps "name type" pairs
/// on one physical line).
fn sp(input: &mut &str) {
    while let Some(rest) = input.strip_prefix([' ', '\t']) {
        *input = rest;
    }
}

/// Whitespace plus `//` line comments and `/* */` block comments.
fn ws(input: &mut &str) {
    loop {
        sp(input);
        if let Some(rest) = input.strip_prefix('\n') {
            *input = rest;
            continue;
        }
        if let Some(rest) = input.strip_prefix('\r') {
            *input = rest;
            continue;
        }
        if let Some(rest) = input.strip_prefix("//") {
            *input = match rest.find('\n') {
                Some(i) => &rest[i..],
                None => "",
            };
            continue;
        }
        if let Some(rest) = input.strip_prefix("/*") {
            *input = match rest.find("*/") {
                Some(i) => &rest[i + 2..],
                None => "",
            };
            continue;
        }
        break;
    }
}

fn peek_is(input: &str, lit: &str) -> bool {
    input.starts_with(lit)
}

/// Consume the literal `lit` at the start of `input` (ASCII
/// case-insensitive so keyword eats match `table`/`TABLE`/...; punctuation
/// literals have no case).
fn eat(input: &mut &str, lit: &str) -> bool {
    let b = input.as_bytes();
    let l = lit.as_bytes();
    if b.len() >= l.len()
        && input.is_char_boundary(l.len())
        && input[..l.len()].eq_ignore_ascii_case(lit)
    {
        *input = &input[l.len()..];
        true
    } else {
        false
    }
}

/// Keyword `kw` at the start of `input`, ASCII case-insensitive (dbdiagram
/// accepts `table`, `TABLE`, `TaBlE`, ...), and not followed by more
/// identifier characters (so "Table" does not match "TableGroup").
fn peek_kw(input: &str, kw: &str) -> bool {
    input.len() >= kw.len()
        && input.is_char_boundary(kw.len())
        && input[..kw.len()].eq_ignore_ascii_case(kw)
        && input[kw.len()..].starts_with(|c: char| !is_bare_ident(c))
}

/// Keyword followed (after same-line spaces) by `next` — distinguishes the
/// `indexes {` block from a column that happens to be named "indexes".
fn peek_kw_then(input: &str, kw: &str, next: char) -> bool {
    if !peek_kw(input, kw) {
        return false;
    }
    if !input.is_char_boundary(kw.len()) {
        return false;
    }
    input[kw.len()..]
        .trim_start_matches([' ', '\t'])
        .starts_with(next)
}

fn skip_line(input: &mut &str) {
    match input.find('\n') {
        Some(i) => *input = &input[i + 1..],
        None => *input = "",
    }
}

fn is_bare_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Identifier: bare word, `"double quoted"`, or `` `backticked` ``.
/// Quotes are stripped.
fn ident(input: &mut &str, base: usize) -> R<String> {
    if let Some(rest) = input.strip_prefix('"') {
        let end = rest.find('"').ok_or(Fail {
            offset: offset(input, base),
            message: "unterminated quoted identifier",
        })?;
        let name = rest[..end].to_string();
        *input = &rest[end + 1..];
        return Ok(name);
    }
    if let Some(rest) = input.strip_prefix('`') {
        let end = rest.find('`').ok_or(Fail {
            offset: offset(input, base),
            message: "unterminated backticked identifier",
        })?;
        let name = rest[..end].to_string();
        *input = &rest[end + 1..];
        return Ok(name);
    }
    let end = input
        .find(|c: char| !is_bare_ident(c))
        .unwrap_or(input.len());
    if end == 0 {
        return fail(offset(input, base), "expected identifier");
    }
    let name = input[..end].to_string();
    *input = &input[end..];
    Ok(name)
}

/// Single-quoted string with `''` escaping.
fn sq_string(input: &mut &str, base: usize) -> R<String> {
    if !eat(input, "'") {
        return fail(offset(input, base), "expected 'string'");
    }
    let mut out = String::new();
    loop {
        if input.is_empty() {
            return fail(offset(input, base), "unterminated string");
        }
        if input.starts_with("''") {
            out.push('\'');
            *input = &input[2..];
            continue;
        }
        if input.starts_with('\'') {
            *input = &input[1..];
            return Ok(out);
        }
        let c = input.chars().next().unwrap();
        out.push(c);
        *input = &input[c.len_utf8()..];
    }
}

/// Note value: `'''triple quoted'''` or a normal single-quoted string.
fn note_value(input: &mut &str, base: usize) -> R<String> {
    if input.starts_with('"') {
        return ident(input, base);
    }
    if eat(input, "'''") {
        let end = input.find("'''").ok_or(Fail {
            offset: offset(input, base),
            message: "unterminated triple-quoted note",
        })?;
        let v = input[..end].to_string();
        *input = &input[end + 3..];
        Ok(v)
    } else {
        sq_string(input, base)
    }
}

fn is_type_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '(' | ')' | ',' | '.' | ' ' | '\t')
}

/// Column type, e.g. `varchar(255)`, `decimal(10,2)`, or a qualified /
/// quoted name (`"Var Char"`, `user.e`, `"user"."e"`).
fn type_str(input: &mut &str, base: usize) -> R<String> {
    let mut out = String::new();
    loop {
        if let Some(rest) = input.strip_prefix('"') {
            let end = rest.find('"').ok_or(Fail {
                offset: offset(input, base),
                message: "unterminated quoted type",
            })?;
            out.push('"');
            out.push_str(&rest[..end + 1]); // verbatim, both quotes
            *input = &rest[end + 1..];
        } else {
            let end = input
                .find(|c: char| !(is_type_char(c) && c != '"' && c != '.'))
                .unwrap_or(input.len());
            out.push_str(input[..end].trim_end());
            *input = &input[end..];
        }
        if out.is_empty() {
            return fail(offset(input, base), "expected column type");
        }
        if eat(input, ".") {
            out.push('.');
        } else {
            break;
        }
    }
    Ok(out)
}

/// A setting value: string, backtick expression, or bare token (number,
/// color, ...).
fn value(input: &mut &str, base: usize) -> R<String> {
    if input.starts_with('\'') {
        return note_value(input, base);
    }
    if input.starts_with('"') {
        return ident(input, base);
    }
    if input.starts_with('`') {
        let rest = &input[1..];
        let end = rest.find('`').ok_or(Fail {
            offset: offset(input, base),
            message: "unterminated backtick expression",
        })?;
        let v = rest[..end].to_string();
        *input = &rest[end + 1..];
        return Ok(v);
    }
    let end = input.find([',', ']', '\n', '\r']).unwrap_or(input.len());
    let v = input[..end].trim();
    if v.is_empty() {
        return fail(offset(input, base), "expected value");
    }
    let v = v.to_string();
    *input = &input[end..];
    Ok(v)
}

fn cardinality(input: &mut &str, base: usize) -> R<Cardinality> {
    if eat(input, "<>") {
        Ok(Cardinality::ManyToMany)
    } else if eat(input, ">") {
        Ok(Cardinality::ManyToOne)
    } else if eat(input, "<") {
        Ok(Cardinality::OneToMany)
    } else if eat(input, "-") {
        Ok(Cardinality::OneToOne)
    } else {
        fail(offset(input, base), "expected relationship (<, >, -, <>)")
    }
}

/// `[schema.]name` — either part may be quoted (`"Schema Name".users`).
fn qualified_name(input: &mut &str, base: usize) -> R<(Option<String>, String)> {
    let first = ident(input, base)?;
    if eat(input, ".") {
        let second = ident(input, base)?;
        Ok((Some(first), second))
    } else {
        Ok((None, first))
    }
}

/// Reference path: `[schema.]table.column` (each part may be quoted).
fn path(input: &mut &str, base: usize) -> R<(Option<String>, String, String)> {
    let mut parts = vec![ident(input, base)?];
    while eat(input, ".") {
        parts.push(ident(input, base)?);
        if parts.len() == 3 {
            break;
        }
    }
    match parts.as_slice() {
        [t, c] => Ok((None, t.clone(), c.clone())),
        [sc, t, c] => Ok((Some(sc.clone()), t.clone(), c.clone())),
        _ => fail(offset(input, base), "expected [schema.]table.column"),
    }
}

/// Reference endpoint: `[schema.]table.(col, ...)` (new composite form),
/// `([schema.]table.col, ...)` (legacy form), or `[schema.]table.col`.
fn endpoint(input: &mut &str, base: usize) -> R<Endpoint> {
    let start = offset(input, base);
    let mut columns: Vec<String> = Vec::new();
    let (schema, table);
    if eat(input, "(") {
        // legacy composite: (table.a, table.b)
        let mut first: Option<(Option<String>, String)> = None;
        loop {
            ws(input);
            let (sc, t, c) = path(input, base)?;
            if let Some((s0, t0)) = &first {
                if (s0.clone(), t0.clone()) != (sc, t) {
                    return fail(offset(input, base), "composite endpoints share one table");
                }
            } else {
                first = Some((sc, t));
            }
            columns.push(c);
            ws(input);
            if !eat(input, ",") {
                break;
            }
        }
        if !eat(input, ")") {
            return fail(
                offset(input, base),
                "expected ')' after composite reference",
            );
        }
        let (s0, t0) = first.unwrap();
        schema = s0;
        table = t0;
    } else {
        let p1 = ident(input, base)?;
        if !eat(input, ".") {
            return fail(offset(input, base), "expected '.' in reference");
        }
        if peek_is(input, "(") {
            // table.(a, b)
            table = p1;
            schema = None;
        } else {
            let p2 = ident(input, base)?;
            if eat(input, ".") {
                // schema.table.(a, b) or schema.table.col
                schema = Some(p1);
                table = p2;
            } else {
                columns.push(p2);
                schema = None;
                table = p1;
            }
        }
        if columns.is_empty() {
            if peek_is(input, "(") {
                let _ = eat(input, "(");
                loop {
                    ws(input);
                    columns.push(ident(input, base)?);
                    ws(input);
                    if !eat(input, ",") {
                        break;
                    }
                }
                if !eat(input, ")") {
                    return fail(
                        offset(input, base),
                        "expected ')' after composite reference",
                    );
                }
            } else {
                columns.push(ident(input, base)?);
            }
        }
    }
    let end = offset(input, base);
    Ok(Endpoint {
        table,
        schema,
        columns,
        span: start..end,
    })
}

enum SettingVal {
    Str(String),
    Ref(Cardinality, Endpoint),
}

/// `[key, key: value, ref: > t.c, ...]` — parsed generically; callers
/// interpret the pairs.
fn settings_list(input: &mut &str, base: usize) -> R<Vec<(String, Option<SettingVal>)>> {
    if !eat(input, "[") {
        return fail(offset(input, base), "expected '['");
    }
    let mut out = Vec::new();
    loop {
        sp(input);
        if eat(input, "]") {
            return Ok(out);
        }
        if input.is_empty() {
            return fail(offset(input, base), "unterminated settings");
        }
        let end = input
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ' ' || c == '\t'))
            .unwrap_or(input.len());
        let key = input[..end].trim().to_ascii_lowercase();
        if key.is_empty() {
            return fail(offset(input, base), "expected setting key");
        }
        *input = &input[end..];
        sp(input);
        if eat(input, ":") {
            sp(input);
            if key == "ref" {
                let card = cardinality(input, base)?;
                ws(input);
                let to = endpoint(input, base)?;
                out.push((key, Some(SettingVal::Ref(card, to))));
            } else if key == "default" {
                // verbatim: quoted strings keep their quotes so exporters
                // can distinguish literals from bare tokens
                let raw = if input.starts_with('\'') {
                    let mut end = 1;
                    let b = input.as_bytes();
                    while end < b.len() {
                        if b[end] == b'\'' {
                            if end + 1 < b.len() && b[end + 1] == b'\'' {
                                end += 2;
                                continue;
                            }
                            end += 1;
                            break;
                        }
                        end += 1;
                    }
                    let v = input[..end.min(input.len())].to_string();
                    *input = &input[end.min(input.len())..];
                    v
                } else if input.starts_with('`') {
                    // keep the backticks: marks an expression for exporters
                    let end = input[1..].find('`').map(|i| i + 1).unwrap_or(input.len());
                    let v = input[..end + 1].to_string();
                    *input = &input[end + 1..];
                    v
                } else {
                    let end = input.find([',', ']', '\n', '\r']).unwrap_or(input.len());
                    let v = input[..end].trim().to_string();
                    *input = &input[end..];
                    v
                };
                out.push((key, Some(SettingVal::Str(raw))));
            } else {
                let v = value(input, base)?;
                out.push((key, Some(SettingVal::Str(v))));
            }
        } else {
            out.push((key, None));
        }
        if eat(input, ",") {
            continue;
        }
        sp(input);
        if eat(input, "]") {
            return Ok(out);
        }
        return fail(offset(input, base), "expected ',' or ']' in settings");
    }
}

fn column_settings_from(kvs: Vec<(String, Option<SettingVal>)>) -> ColumnSettings {
    let mut s = ColumnSettings::default();
    for (key, val) in kvs {
        match val {
            None => match key.as_str() {
                "pk" | "primary key" | "primary_key" => s.pk = true,
                "increment" | "autoincrement" | "auto_increment" => s.increment = true,
                "unique" => s.unique = true,
                "not null" => s.not_null = Some(true),
                "null" => s.not_null = Some(false),
                _ => {}
            },
            Some(SettingVal::Str(v)) => match key.as_str() {
                "note" => s.note = Some(v),
                "default" => s.default = Some(v),
                _ => {}
            },
            Some(SettingVal::Ref(card, to)) => {
                if s.inline_ref.is_none() {
                    s.inline_ref = Some(InlineRef { card, to });
                }
            }
        }
    }
    s
}

fn column(input: &mut &str, base: usize) -> R<Column> {
    let start = offset(input, base);
    let name_off = offset(input, base);
    let name = ident(input, base)?;
    let name_end = offset(input, base);
    sp(input);
    let ty = type_str(input, base)?;
    sp(input);
    let settings = if peek_is(input, "[") {
        column_settings_from(settings_list(input, base)?)
    } else {
        ColumnSettings::default()
    };
    Ok(Column {
        name,
        name_span: name_off..name_end,
        ty,
        span: start..offset(input, base),
        settings,
    })
}

fn index(input: &mut &str, base: usize) -> R<Index> {
    let start = offset(input, base);
    let mut columns = Vec::new();
    if eat(input, "(") {
        loop {
            ws(input);
            columns.push(ident(input, base)?);
            ws(input);
            if !eat(input, ",") {
                break;
            }
        }
        if !eat(input, ")") {
            return fail(offset(input, base), "expected ')' after index columns");
        }
    } else {
        columns.push(ident(input, base)?);
    }
    sp(input);
    let mut idx = Index {
        columns,
        unique: false,
        pk: false,
        name: None,
        index_type: None,
        span: start..0,
    };
    if peek_is(input, "[") {
        let kvs = settings_list(input, base)?;
        for (key, val) in kvs {
            match val {
                None => match key.as_str() {
                    "unique" => idx.unique = true,
                    "pk" | "primary key" => idx.pk = true,
                    _ => {}
                },
                Some(SettingVal::Str(v)) => match key.as_str() {
                    "name" => idx.name = Some(v),
                    "type" => idx.index_type = Some(v),
                    _ => {}
                },
                Some(SettingVal::Ref(..)) => {}
            }
        }
    }
    idx.span = start..offset(input, base);
    Ok(idx)
}

// --- top-level constructs ------------------------------------------------

enum Item {
    Prj(Project),
    Tbl(Table),
    Ref(Ref),
    Enm(Enum),
    Grp(TableGroup),
}

fn top_item(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Item> {
    if peek_kw(input, "TableGroup") {
        table_group(input, base, errs).map(Item::Grp)
    } else if peek_kw(input, "Table") {
        table(input, base, errs).map(Item::Tbl)
    } else if peek_kw(input, "Project") {
        project(input, base, errs).map(Item::Prj)
    } else if peek_kw(input, "Ref") {
        ref_stmt(input, base).map(Item::Ref)
    } else if peek_kw(input, "Enum") {
        enum_def(input, base, errs).map(Item::Enm)
    } else {
        fail(
            offset(input, base),
            "expected Project, Table, Ref, Enum, or TableGroup",
        )
    }
}

fn table(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Table> {
    let start = offset(input, base);
    if !eat(input, "Table") {
        return fail(offset(input, base), "expected 'Table'");
    }
    ws(input);
    let name_off = offset(input, base);
    let (schema, name) = qualified_name(input, base)?;
    let name_end = offset(input, base);
    ws(input);
    let mut t = Table {
        name,
        schema,
        alias: None,
        span: start..0,
        name_span: name_off..name_end,
        header_color: None,
        note: None,
        columns: Vec::new(),
        indexes: Vec::new(),
    };
    if peek_kw(input, "as") {
        let _ = eat(input, "as");
        ws(input);
        t.alias = Some(ident(input, base)?);
        ws(input);
    }
    if peek_is(input, "[") {
        let kvs = settings_list(input, base)?;
        for (key, val) in kvs {
            if let Some(SettingVal::Str(v)) = val {
                match key.as_str() {
                    "note" => t.note = Some(v),
                    "headercolor" => t.header_color = Some(v),
                    _ => {}
                }
            }
        }
        ws(input);
    }
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after table name");
    }
    loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated table block"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        if peek_kw_then(input, "Note", ':') {
            let _ = eat(input, "Note");
            ws(input);
            let _ = eat(input, ":");
            ws(input);
            match note_value(input, base) {
                Ok(n) => t.note = Some(n),
                Err(e) => {
                    errs.push(err_at(e.offset, e.message));
                    skip_line(input);
                }
            }
            continue;
        }
        if peek_kw_then(input, "indexes", '{') {
            let _ = eat(input, "indexes");
            ws(input);
            let _ = eat(input, "{");
            loop {
                ws(input);
                if input.is_empty() {
                    errs.push(err_at(offset(input, base), "unterminated indexes block"));
                    break;
                }
                if eat(input, "}") {
                    break;
                }
                match index(input, base) {
                    Ok(ix) => t.indexes.push(ix),
                    Err(e) => {
                        errs.push(err_at(e.offset, e.message));
                        skip_line(input);
                    }
                }
            }
            continue;
        }
        match column(input, base) {
            Ok(c) => t.columns.push(c),
            Err(e) => {
                errs.push(err_at(e.offset, e.message));
                skip_line(input);
            }
        }
    }
    t.span = start..offset(input, base);
    Ok(t)
}

fn ref_stmt(input: &mut &str, base: usize) -> R<Ref> {
    let start = offset(input, base);
    if !eat(input, "Ref") {
        return fail(offset(input, base), "expected 'Ref'");
    }
    ws(input);
    let mut name = None;
    if !peek_is(input, ":") {
        name = Some(ident(input, base)?);
        ws(input);
    }
    if peek_is(input, "{") {
        // `Ref name { a.b > c.d }` block form
        let _ = eat(input, "{");
        ws(input);
        let from = endpoint(input, base)?;
        ws(input);
        let card = cardinality(input, base)?;
        ws(input);
        let to = endpoint(input, base)?;
        ws(input);
        if !eat(input, "}") {
            return fail(offset(input, base), "expected '}' after ref body");
        }
        let (on_delete, on_update) = ref_settings(input, base);
        return Ok(Ref {
            name,
            span: start..offset(input, base),
            from,
            to,
            card,
            on_delete,
            on_update,
        });
    }
    if !eat(input, ":") {
        return fail(offset(input, base), "expected ':' after Ref");
    }
    ws(input);
    let from = endpoint(input, base)?;
    ws(input);
    let card = cardinality(input, base)?;
    ws(input);
    let to = endpoint(input, base)?;
    let (on_delete, on_update) = ref_settings(input, base);
    Ok(Ref {
        name,
        span: start..offset(input, base),
        from,
        to,
        card,
        on_delete,
        on_update,
    })
}

/// Optional `[delete: cascade, update: no action]` referential actions.
fn ref_settings(input: &mut &str, base: usize) -> (Option<String>, Option<String>) {
    let mut out = (None, None);
    ws(input);
    if !peek_is(input, "[") {
        return out;
    }
    if let Ok(kvs) = settings_list(input, base) {
        for (k, v) in kvs {
            if let Some(SettingVal::Str(v)) = v {
                match k.as_str() {
                    "delete" | "on_delete" => out.0 = Some(v.to_ascii_uppercase()),
                    "update" | "on_update" => out.1 = Some(v.to_ascii_uppercase()),
                    _ => {}
                }
            }
        }
    }
    out
}

fn enum_def(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Enum> {
    let start = offset(input, base);
    if !eat(input, "Enum") {
        return fail(offset(input, base), "expected 'Enum'");
    }
    ws(input);
    let name_off = offset(input, base);
    let (schema, name) = qualified_name(input, base)?;
    let name_end = offset(input, base);
    ws(input);
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after enum name");
    }
    let mut e = Enum {
        name,
        schema,
        span: start..0,
        name_span: name_off..name_end,
        values: Vec::new(),
    };
    loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated enum block"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        match ident(input, base) {
            Ok(vname) => {
                sp(input);
                let mut note = None;
                if peek_is(input, "[") {
                    let kvs = match settings_list(input, base) {
                        Ok(kvs) => kvs,
                        Err(err) => {
                            errs.push(err_at(err.offset, err.message));
                            skip_line(input);
                            e.values.push(EnumValue {
                                name: vname,
                                note: None,
                            });
                            continue;
                        }
                    };
                    for (key, val) in kvs {
                        if key == "note" {
                            if let Some(SettingVal::Str(v)) = val {
                                note = Some(v);
                            }
                        }
                    }
                }
                e.values.push(EnumValue { name: vname, note });
            }
            Err(err) => {
                errs.push(err_at(err.offset, err.message));
                skip_line(input);
            }
        }
    }
    e.span = start..offset(input, base);
    Ok(e)
}

fn table_group(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<TableGroup> {
    let start = offset(input, base);
    if !eat(input, "TableGroup") {
        return fail(offset(input, base), "expected 'TableGroup'");
    }
    ws(input);
    let name = ident(input, base)?;
    ws(input);
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after group name");
    }
    let mut g = TableGroup {
        name,
        span: start..0,
        tables: Vec::new(),
    };
    loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated table group"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        match qualified_name(input, base) {
            Ok((sc, n)) => g.tables.push(sc.map_or(n.clone(), |s| format!("{s}.{n}"))),
            Err(e) => {
                errs.push(err_at(e.offset, e.message));
                skip_line(input);
            }
        }
    }
    g.span = start..offset(input, base);
    Ok(g)
}

fn project(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Project> {
    let start = offset(input, base);
    if !eat(input, "Project") {
        return fail(offset(input, base), "expected 'Project'");
    }
    ws(input);
    let name = ident(input, base)?;
    ws(input);
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after project name");
    }
    let mut p = Project {
        name,
        span: start..0,
        database_type: None,
        note: None,
    };
    loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated project block"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        let key = match ident(input, base) {
            Ok(k) => k,
            Err(e) => {
                errs.push(err_at(e.offset, e.message));
                skip_line(input);
                continue;
            }
        };
        sp(input);
        if !eat(input, ":") {
            errs.push(err_at(offset(input, base), "expected ':' in project field"));
            skip_line(input);
            continue;
        }
        sp(input);
        match value(input, base) {
            Ok(v) => match key.to_ascii_lowercase().as_str() {
                "database_type" => p.database_type = Some(v),
                "note" => p.note = Some(v),
                _ => {}
            },
            Err(e) => {
                errs.push(err_at(e.offset, e.message));
                skip_line(input);
            }
        }
    }
    p.span = start..offset(input, base);
    Ok(p)
}

/// After a failed top-level item, skip broken constructs line-wise, stopping
/// at the next line that starts a top-level keyword. Braces are deliberately
/// ignored: by the time we get here the construct header itself failed, so
/// any `{` on these lines belongs to the broken header.
const TOP_KWS: [&str; 5] = ["Project", "Table", "Ref", "Enum", "TableGroup"];

fn recover(mut input: &str) -> &str {
    while !input.is_empty() {
        let line_end = input.find('\n').unwrap_or(input.len());
        let line = input[..line_end].trim_start();
        if TOP_KWS.iter().any(|kw| peek_kw(line, kw)) {
            return input;
        }
        let mut rest = input;
        skip_line(&mut rest);
        input = rest;
    }
    input
}

/// Parse a full DBML document, recovering from errors so partial documents
/// (mid-typing) still produce a usable diagram.
pub fn parse(src: &str) -> ParseOutput {
    let base = src.as_ptr() as usize;
    let mut rest = src;
    let mut out = ParseOutput::default();
    loop {
        ws(&mut rest);
        if rest.is_empty() {
            break;
        }
        let mut attempt = rest;
        match top_item(&mut attempt, base, &mut out.errors) {
            Ok(item) => {
                rest = attempt;
                match item {
                    Item::Prj(p) => out.doc.project = Some(p),
                    Item::Tbl(t) => out.doc.tables.push(t),
                    Item::Ref(r) => out.doc.refs.push(r),
                    Item::Enm(e) => out.doc.enums.push(e),
                    Item::Grp(g) => out.doc.table_groups.push(g),
                }
            }
            Err(e) => {
                out.errors.push(ParseError {
                    offset: e.offset,
                    message: e.message.to_string(),
                });
                rest = recover(attempt);
            }
        }
    }
    out
}

/// 0-based (line, byte-column) for a byte offset.
pub fn line_col_at(src: &str, offset: usize) -> (usize, usize) {
    let mut o = offset.min(src.len());
    while o > 0 && !src.is_char_boundary(o) {
        o -= 1;
    }
    let before = &src[..o];
    let line = before.matches('\n').count();
    let col = before.rfind('\n').map(|i| o - i - 1).unwrap_or(o);
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(out: &ParseOutput) -> &[Table] {
        &out.doc.tables
    }

    #[test]
    fn basic_table_with_settings() {
        let src = r#"
Table users as U [note: 'Accounts', headercolor: #3498DB] {
  id integer [pk, increment]
  username varchar(80) [not null, unique]
  email varchar(255) [not null]
  role varchar [default: 'member']
  created_at timestamp [default: `now()`]

  indexes {
    (username, email) [unique, name: 'idx_contact', type: hash]
  }

  Note: 'Registered accounts'
}
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(tables(&out).len(), 1);
        let t = &tables(&out)[0];
        assert_eq!(t.name, "users");
        assert_eq!(t.alias.as_deref(), Some("U"));
        assert_eq!(t.note.as_deref(), Some("Registered accounts"));
        assert_eq!(t.header_color.as_deref(), Some("#3498DB"));
        assert_eq!(t.columns.len(), 5);

        let id = &t.columns[0];
        assert_eq!((id.name.as_str(), id.ty.as_str()), ("id", "integer"));
        assert!(id.settings.pk && id.settings.increment);

        let username = &t.columns[1];
        assert_eq!(username.ty, "varchar(80)");
        assert_eq!(username.settings.not_null, Some(true));
        assert!(username.settings.unique);

        assert_eq!(t.columns[3].settings.default.as_deref(), Some("'member'"));
        // backtick expressions keep their markers
        assert_eq!(t.columns[4].settings.default.as_deref(), Some("`now()`"));

        assert_eq!(t.indexes.len(), 1);
        let ix = &t.indexes[0];
        assert_eq!(ix.columns, ["username", "email"]);
        assert!(ix.unique);
        assert_eq!(ix.name.as_deref(), Some("idx_contact"));
        assert_eq!(ix.index_type.as_deref(), Some("hash"));
    }

    #[test]
    fn refs_standalone_inline_composite() {
        let src = r#"
Table posts { id integer [pk] user_id integer [ref: > users.id] }
Table users { id integer [pk] }
Table tenants { id integer [pk] org text }
Ref: posts.user_id > users.id
Ref named: (tenants.id, tenants.org) < (users.id, users.org)
Ref: a.b - c.d
Ref: a.b <> c.d
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(tables(&out).len(), 3);
        assert_eq!(out.doc.refs.len(), 4);

        let r0 = &out.doc.refs[0];
        assert_eq!(r0.name, None);
        assert_eq!(
            (r0.from.table.as_str(), r0.from.columns[0].as_str()),
            ("posts", "user_id")
        );
        assert_eq!(r0.card, Cardinality::ManyToOne);

        let r1 = &out.doc.refs[1];
        assert_eq!(r1.name.as_deref(), Some("named"));
        assert_eq!(r1.from.table, "tenants");
        assert_eq!(r1.from.columns, ["id", "org"]);
        assert_eq!(r1.card, Cardinality::OneToMany);

        assert_eq!(out.doc.refs[2].card, Cardinality::OneToOne);
        assert_eq!(out.doc.refs[3].card, Cardinality::ManyToMany);

        let inline = &tables(&out)[0].columns[1].settings.inline_ref;
        let inline = inline.as_ref().unwrap();
        assert_eq!(inline.card, Cardinality::ManyToOne);
        assert_eq!(inline.to.table, "users");
        assert_eq!(inline.to.columns, ["id"]);
    }

    #[test]
    fn enum_group_project() {
        let src = r#"
Project blog {
  database_type: 'PostgreSQL'
  Note: 'A blog'
}
Enum post_status {
  draft
  published [note: 'live']
  archived
}
TableGroup content {
  posts
  comments
}
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let p = out.doc.project.as_ref().unwrap();
        assert_eq!(p.name, "blog");
        assert_eq!(p.database_type.as_deref(), Some("PostgreSQL"));
        assert_eq!(p.note.as_deref(), Some("A blog"));
        let e = &out.doc.enums[0];
        assert_eq!(e.name, "post_status");
        assert_eq!(e.values.len(), 3);
        assert_eq!(e.values[1].note.as_deref(), Some("live"));
        let g = &out.doc.table_groups[0];
        assert_eq!(g.tables, ["posts", "comments"]);
    }

    #[test]
    fn quoted_identifiers_and_strings() {
        let src = r#"
Table "user accounts" {
  "full name" varchar(100) [not null]
  quote varchar [default: 'it''s fine']
}
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let t = &tables(&out)[0];
        assert_eq!(t.name, "user accounts");
        assert_eq!(t.columns[0].name, "full name");
        // defaults are stored verbatim (quotes + '' escapes intact) so the
        // SQL exporter can distinguish literals from bare tokens
        assert_eq!(
            t.columns[1].settings.default.as_deref(),
            Some("'it''s fine'")
        );
    }

    #[test]
    fn comments_everywhere() {
        let src = r#"
// leading comment
/* block
   comment */
Table a { // trailing
  id integer [pk] /* inline */
}
Ref: a.id > a.id // trailing ref comment
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(tables(&out).len(), 1);
        assert_eq!(out.doc.refs.len(), 1);
    }

    #[test]
    fn spans_point_at_names() {
        let src = "Table users {\n  id integer\n}\nEnum st { a }\n";
        let out = parse(src);
        let t = &tables(&out)[0];
        assert_eq!(&src[t.name_span.clone()], "users");
        assert_eq!(&src[t.columns[0].name_span.clone()], "id");
        let e = &out.doc.enums[0];
        assert_eq!(&src[e.name_span.clone()], "st");
        assert_eq!(&src[t.span.clone()], "Table users {\n  id integer\n}");
    }

    #[test]
    fn error_recovery_keeps_good_tables() {
        let src = r#"
Table good1 {
  id integer [pk]
}

Tabl oops {

Table good2 {
  id integer [pk]
  !!!broken
  name varchar
}
"#;
        let out = parse(src);
        assert!(!out.errors.is_empty());
        let names: Vec<&str> = tables(&out).iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"good1"), "{names:?}");
        assert!(names.contains(&"good2"), "{names:?}");
        let good2 = tables(&out).iter().find(|t| t.name == "good2").unwrap();
        assert_eq!(good2.columns.len(), 2);
        assert_eq!(good2.columns[1].name, "name");
    }

    #[test]
    fn unterminated_blocks_still_parse_contents() {
        let src = "Table halfopen {\n  id integer [pk]\n";
        let out = parse(src);
        assert!(!out.errors.is_empty());
        assert_eq!(tables(&out).len(), 1);
        assert_eq!(tables(&out)[0].columns.len(), 1);
    }

    #[test]
    fn line_col_basics() {
        let src = "abc\ndef\ngh";
        assert_eq!(line_col_at(src, 0), (0, 0));
        assert_eq!(line_col_at(src, 4), (1, 0));
        assert_eq!(line_col_at(src, 6), (1, 2));
        assert_eq!(line_col_at(src, 8), (2, 0));
        assert_eq!(line_col_at(src, 999), (2, 2));
    }
}

#[cfg(test)]
mod case_tests {
    use super::*;

    #[test]
    fn top_level_keywords_accept_any_case() {
        for kw in ["table", "TABLE", "TaBlE"] {
            let out = parse(&format!("{kw} users {{ id integer [pk] }}"));
            assert!(out.errors.is_empty(), "{kw}: {:?}", out.errors);
            assert_eq!(out.doc.tables.len(), 1);
            assert_eq!(out.doc.tables[0].name, "users");
        }
        for kw in ["ref", "REF", "Ref"] {
            let src = format!("Table a {{ id integer }}\n{kw}: a.id > a.id");
            let out = parse(&src);
            assert!(out.errors.is_empty(), "{kw}: {:?}", out.errors);
            assert_eq!(out.doc.refs.len(), 1);
        }
    }

    #[test]
    fn settings_accept_any_case() {
        let src = "TABLE users {\n  id INTEGER [PK, NOT NULL, UNIQUE, DEFAULT: 'x']\n}";
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let col = &out.doc.tables[0].columns[0];
        assert!(col.settings.pk);
        assert_eq!(col.settings.not_null, Some(true));
        assert!(col.settings.unique);
    }

    #[test]
    fn mixed_case_fixture_matches_canonical_layout() {
        let mixed = "project blog {
  database_type: 'PostgreSQL'
  note: 'A small blog schema'
}

TABLE users AS U [NOTE: 'Registered accounts'] {
  ID integer [PK, increment]
  username varchar(80) [Not Null, Unique]
  email varchar(255) [not null]
}

Table posts {
  id integer [pk]
  user_id integer [not null, REF: > U.id]
}

Ref: posts.user_id > users.id

ENUM post_status {
  draft
  published
}
";
        let canonical = crate::layout::layout(&crate::model::build(&parse(mixed).doc));
        assert!(parse(mixed).errors.is_empty(), "{:?}", parse(mixed).errors);
        assert_eq!(canonical.boxes.len(), 3);
        assert_eq!(canonical.edges.len(), 2, "inline + top-level Ref");
        let users = canonical.boxes.iter().find(|b| b.title == "users").unwrap();
        assert_eq!((users.x, users.y), (0, 0));
    }
}
