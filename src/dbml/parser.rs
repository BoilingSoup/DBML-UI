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
    // pointer differences against `base`: every input handed here must be
    // a slice of the parsed source (the skip helpers above keep their
    // exhausted-state slices inside it — a foreign "" would underflow)
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
                // an empty slice OF the source: the offset math derives
                // spans from pointer differences, so a foreign "" would
                // underflow (and did, on web)
                None => &rest[rest.len()..],
            };
            continue;
        }
        if let Some(rest) = input.strip_prefix("/*") {
            *input = match rest.find("*/") {
                Some(i) => &rest[i + 2..],
                None => &rest[rest.len()..],
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
        None => *input = &input[input.len()..],
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

/// Dedent a triple-quoted note body: strip one leading newline, a
/// whitespace-only tail after the last line, and the common leading
/// indent of the non-empty lines (dbdiagram's indented `'''` blocks read
/// back as clean lines — the diagram and the GUI editor see the same
/// text the author meant).
fn dedent_note_body(raw: &str) -> String {
    let t = raw.strip_prefix('\n').unwrap_or(raw);
    // trim_end: the closing ''' line's indent + any empty trailing lines
    // are structure, not content (a single strip_suffix left one behind)
    let t = t.trim_end();
    let min_indent = t
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for (i, l) in t.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(l.get(min_indent..).unwrap_or(l.trim_start()));
    }
    out
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
        let v = dedent_note_body(&input[..end]);
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
        // a backtick closes the expression only when `,`/`]`/newline
        // follows — nested `ident` groups inside (upstream emits
        // `json_valid(`Privilege`)`) don't terminate it
        let b = input.as_bytes();
        let mut end = 1;
        let mut closed = false;
        while end < b.len() {
            if b[end] == b'`' {
                let mut k = end + 1;
                while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                    k += 1;
                }
                if k >= b.len() || matches!(b[k], b',' | b']' | b'\n' | b'\r') {
                    closed = true;
                    break;
                }
            }
            end += 1;
        }
        if !closed {
            return fail(offset(input, base), "unterminated backtick expression");
        }
        let v = input[1..end].to_string();
        *input = &input[end + 1..];
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

/// `(kind, left_optional, right_optional)` — the full 16-form operator
/// set (`?>`, `>?`, `?<>`, `-?`, …): the `?` on a side drops that side's
/// min cardinality to zero (the reference is optional).
fn cardinality_full(input: &mut &str, base: usize) -> R<(Cardinality, bool, bool)> {
    let table: &[(&str, Cardinality, bool, bool)] = &[
        // the longest forms first so a trailing `?` wins over a bare match
        ("?>?", Cardinality::ManyToOne, true, true),
        ("?>", Cardinality::ManyToOne, true, false),
        (">?", Cardinality::ManyToOne, false, true),
        (">", Cardinality::ManyToOne, false, false),
        ("?<?", Cardinality::OneToMany, true, true),
        ("?<", Cardinality::OneToMany, true, false),
        ("<?", Cardinality::OneToMany, false, true),
        ("?<>?", Cardinality::ManyToMany, true, true),
        ("?<>", Cardinality::ManyToMany, true, false),
        ("<>?", Cardinality::ManyToMany, false, true),
        ("<>", Cardinality::ManyToMany, false, false),
        ("<", Cardinality::OneToMany, false, false),
        ("?-?", Cardinality::OneToOne, true, true),
        ("?-", Cardinality::OneToOne, true, false),
        ("-?", Cardinality::OneToOne, false, true),
        ("-", Cardinality::OneToOne, false, false),
    ];
    for (tok, kind, l, r) in table {
        if eat(input, tok) {
            return Ok((*kind, *l, *r));
        }
    }
    fail(offset(input, base), "expected relationship (<, >, -, <>, with optional ? marks)")
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
    Ref(Cardinality, bool, bool, Endpoint, usize, Option<String>),
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
                let op_offset = offset(input, base);
                let (card, l_opt, r_opt) = cardinality_full(input, base)?;
                ws(input);
                let to = endpoint(input, base)?;
                // endpoint settings bracket: `ref: > t.c [color: …]` —
                // the relationship's color lives there (only `color` is
                // interpreted; other keys ride along unused)
                ws(input);
                let mut color = None;
                if peek_is(input, "[") {
                    if let Ok(kvs) = settings_list(input, base) {
                        for (k, v) in kvs {
                            if let (Some(SettingVal::Str(sv)), "color") = (&v, k.as_str()) {
                                color = Some(sv.clone());
                            }
                        }
                    }
                }
                out.push((key, Some(SettingVal::Ref(card, l_opt, r_opt, to, op_offset, color))));
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
                    // upstream importers emit dump text verbatim: a quoted
                    // default can carry trailing junk (`'utf8mb3'xxx'`) —
                    // extend to the terminator so it round-trips verbatim
                    if input.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '\'') {
                        let end2 = input.find([',', ']', '\n', '\r']).unwrap_or(input.len());
                        let whole = format!("{v}{}", &input[..end2]);
                        *input = &input[end2..];
                        whole
                    } else {
                        v
                    }
                } else if input.starts_with('`') {
                    // keep the backticks: marks an expression for exporters.
                    // A backtick closes the value only when a `,`/`]`/
                    // newline follows — nested `ident` groups inside the
                    // expression (upstream emits `json_valid(`Privilege`)`)
                    // don't terminate it
                    let b = input.as_bytes();
                    let mut end = 1;
                    while end < b.len() {
                        if b[end] == b'`' {
                            let mut k = end + 1;
                            while k < b.len() && (b[k] == b' ' || b[k] == b'\t') {
                                k += 1;
                            }
                            if k >= b.len() || matches!(b[k], b',' | b']' | b'\n' | b'\r') {
                                end += 1;
                                break;
                            }
                        }
                        end += 1;
                    }
                    let v = input[..end.min(input.len())].to_string();
                    *input = &input[end.min(input.len())..];
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
                // `check: \`expr\`` — value() strips the backticks; keep
                // them so the formatter can re-emit verbatim
                "check" => s.check = Some(format!("`{v}`")),
                // any other key:value is a dbdiagram custom property
                _ => s.custom.push((key, v)),
            },
            Some(SettingVal::Ref(card, l_opt, r_opt, to, op_offset, color)) => {
                if s.inline_ref.is_none() {
                    s.inline_ref = Some(InlineRef { card, left_optional: l_opt, right_optional: r_opt, to, op_offset, color });
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

/// Index column: an identifier. Upstream importers emit unquoted
/// multi-word names (`date of birth`), so consecutive bare words on the
/// same line join into one name.
fn index_ident(input: &mut &str, base: usize) -> R<String> {
    // functional-index expressions are backtick-quoted and can hold
    // nested `ident` groups (`(cast(json_extract(`col`,…)))`) — a
    // backtick inside the expression's parens is nested; one at paren
    // depth 0 closes it
    if input.starts_with('`') {
        let b = input.as_bytes();
        let mut depth = 0i32;
        let mut end = None;
        for (k, &c) in b.iter().enumerate().skip(1) {
            match c {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'`' if depth <= 0 => {
                    end = Some(k);
                    break;
                }
                b'\n' => break,
                _ => {}
            }
        }
        let Some(end) = end else {
            return fail(offset(input, base), "unterminated index expression");
        };
        let name = input[1..end].to_string();
        *input = &input[end + 1..];
        return Ok(name);
    }
    let mut name = ident(input, base)?;
    loop {
        let save = *input;
        let b = input.as_bytes();
        let mut k = 0;
        while k < b.len() && b[k].is_ascii_whitespace() && b[k] != b'\n' && b[k] != b'\r' {
            k += 1;
        }
        if k < b.len() && is_bare_ident(b[k] as char) {
            *input = &input[k..];
            name.push(' ');
            name.push_str(&ident(input, base)?);
        } else {
            *input = save;
            break;
        }
    }
    Ok(name)
}

fn index(input: &mut &str, base: usize) -> R<Index> {
    let start = offset(input, base);
    let mut columns = Vec::new();
    if eat(input, "(") {
        loop {
            ws(input);
            columns.push(index_ident(input, base)?);
            ws(input);
            if !eat(input, ",") {
                break;
            }
        }
        if !eat(input, ")") {
            return fail(offset(input, base), "expected ')' after index columns");
        }
    } else {
        columns.push(index_ident(input, base)?);
    }
    sp(input);
    let mut comma_form = false;
    let mut idx = Index {
        columns,
        unique: false,
        pk: false,
        name: None,
        index_type: None,
        span: start..0,
        comma_form: false,
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
    } else if eat(input, ",") {
        // bare comma form used by some importers: `(cols), pk, name: 'x'`.
        // dbdiagram's grammar rejects it (settings belong in [..]) — keep
        // the lenient parse (the diagram renders, the formatter
        // canonicalizes it away) and flag it for the validator, whose
        // quick fix rewrites the line into brackets.
        comma_form = true;
        let end = input.find(['\n', '}']).unwrap_or(input.len());
        let line = input[..end].trim();
        for kv in line.split(',') {
            let kv = kv.trim();
            if kv.eq_ignore_ascii_case("pk") || kv.eq_ignore_ascii_case("primary key") {
                idx.pk = true;
            } else if kv.eq_ignore_ascii_case("unique") {
                idx.unique = true;
            } else if let Some((k, v)) = kv.split_once(':') {
                let v = v.trim().trim_matches('\'').trim_matches('"');
                match k.trim().to_ascii_lowercase().as_str() {
                    "name" => idx.name = Some(v.to_string()),
                    "type" => idx.index_type = Some(v.to_string()),
                    _ => {}
                }
            }
        }
        *input = &input[end..];
    }
    idx.comma_form = comma_form;
    idx.span = start..offset(input, base);
    Ok(idx)
}

// --- records ---------------------------------------------------------------

/// Single-quoted record string: `''` and `\'` both escape a quote.
fn record_string(input: &mut &str, base: usize) -> R<String> {
    if !eat(input, "'") {
        return fail(offset(input, base), "expected 'string'");
    }
    let mut out = String::new();
    loop {
        if input.is_empty() || input.starts_with('\n') || input.starts_with('\r') {
            return fail(offset(input, base), "unterminated string");
        }
        if input.starts_with("''") {
            out.push('\'');
            *input = &input[2..];
            continue;
        }
        if input.starts_with("\\'") {
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

/// Length of a numeric literal at the start of `input` (0 when there is
/// none): optional sign, digits with one optional dot, optional exponent.
fn number_len(input: &str) -> usize {
    let b = input.as_bytes();
    let mut i = 0usize;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i > start
    };
    if !digits(&mut i) {
        // a leading-dot form (.5)
        if b.get(i) == Some(&b'.') {
            i += 1;
            if !digits(&mut i) {
                return 0;
            }
        } else {
            return 0;
        }
    } else if b.get(i) == Some(&b'.') {
        i += 1;
        digits(&mut i);
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > start {
            i = j;
        }
    }
    i
}

/// One value in a records row. An empty field (nothing before the next
/// comma / end of line) is a literal null.
fn record_value(input: &mut &str, base: usize) -> R<(RecordValue, Span)> {
    sp(input);
    let start = offset(input, base);
    if input.is_empty() || input.starts_with(['\n', '\r']) || peek_is(input, "//") {
        return Ok((RecordValue::Null, start..start));
    }
    if input.starts_with('\'') {
        return record_string(input, base)
            .map(|v| (RecordValue::Text(v), start..offset(input, base)));
    }
    if input.starts_with('`') {
        let end = input[1..]
            .find('`')
            .ok_or(Fail {
                offset: offset(input, base),
                message: "unterminated backtick expression",
            })?
            + 1;
        let v = input[1..end].to_string();
        *input = &input[end + 1..];
        return Ok((RecordValue::Expr(v), start..offset(input, base)));
    }
    let n = number_len(input);
    if n > 0 {
        let v = input[..n].to_string();
        *input = &input[n..];
        return Ok((RecordValue::Number(v), start..offset(input, base)));
    }
    for (kw, v) in [
        ("null", RecordValue::Null),
        ("true", RecordValue::Bool(true)),
        ("false", RecordValue::Bool(false)),
    ] {
        if peek_kw(input, kw) {
            let _ = eat(input, kw);
            return Ok((v, start..offset(input, base)));
        }
    }
    ident(input, base).map(|i| (RecordValue::Ident(i), start..offset(input, base)))
}

/// Same-line whitespace plus a trailing `//` comment (a comment runs to
/// the end of the line, so what follows is the next line).
fn sp_comment(input: &mut &str) {
    sp(input);
    if input.starts_with("//") {
        *input = match input.find('\n') {
            Some(i) => &input[i..],
            None => &input[input.len()..],
        };
    }
}

/// One comma-separated records row (the caller has positioned us at its
/// first value). Rows end at a newline, `}`, or EOF; when `ncols` is
/// known the count must match — a mismatch fails the row, not the block.
fn record_row(
    input: &mut &str,
    base: usize,
    ncols: Option<usize>,
) -> R<(Vec<RecordValue>, Vec<Span>)> {
    let mut row = Vec::new();
    let mut spans = Vec::new();
    loop {
        let (v, span) = record_value(input, base)?;
        row.push(v);
        spans.push(span);
        sp_comment(input);
        if !eat(input, ",") {
            break;
        }
        sp_comment(input);
        if input.is_empty() || input.starts_with(['\n', '\r']) {
            // trailing comma: an empty final field
            let end = offset(input, base);
            row.push(RecordValue::Null);
            spans.push(end..end);
            break;
        }
    }
    if let Some(n) = ncols {
        if row.len() != n {
            return fail(
                offset(input, base),
                "record row does not match the column list",
            );
        }
    }
    Ok((row, spans))
}

/// Skip the rest of the damaged line — unless the block's closing brace
/// starts the NEXT line, which must be left for the block loop (eating it
/// would swallow the rest of the enclosing table).
fn skip_damaged_line(input: &mut &str) {
    let rest = match input.find('\n') {
        Some(i) => &input[i + 1..],
        None => &input[input.len()..],
    };
    if rest.trim_start().starts_with('}') {
        return;
    }
    skip_line(input);
}

/// `records [target][.schema][(col, ...)] [example] { rows }` — sample
/// rows for a table. The caller has consumed the `records` keyword.
/// Returns the optional target table name (top-level form; the last path
/// segment) plus the block. The header (target, columns, `[example]`)
/// binds on one line; row errors recover per line and only structural
/// damage (header, missing `{`) fails the whole statement.
fn records_block(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
) -> R<(Option<String>, TableRecords)> {
    sp(input);
    // The target binds on the SAME line only: a bare `records` mid-typing
    // at end of line must fail here rather than swallow the next
    // statement as its target.
    let mut target = None;
    if !input.is_empty() && !input.starts_with(['(', '[', '{', '\n', '\r']) {
        let mut parts = vec![ident(input, base)?];
        while eat(input, ".") {
            parts.push(ident(input, base)?);
        }
        target = parts.pop();
    }
    sp(input);
    let mut columns = Vec::new();
    if eat(input, "(") {
        loop {
            sp(input);
            if eat(input, ")") {
                break;
            }
            columns.push(ident(input, base)?);
            sp(input);
            if !eat(input, ",") {
                sp(input);
                if !eat(input, ")") {
                    return fail(
                        offset(input, base),
                        "expected ')' after records columns",
                    );
                }
                break;
            }
        }
        sp(input);
    }
    let mut example = false;
    if peek_is(input, "[") {
        for (key, val) in settings_list(input, base)? {
            if key == "example" && val.is_none() {
                example = true;
            }
        }
        sp(input);
    }
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after records header");
    }
    let mut rec = TableRecords {
        columns,
        rows: Vec::new(),
        value_spans: Vec::new(),
        example,
    };
    loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated records block"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        let ncols = if rec.columns.is_empty() {
            None
        } else {
            Some(rec.columns.len())
        };
        match record_row(input, base, ncols) {
            Ok((row, spans)) => {
                rec.rows.push(row);
                rec.value_spans.push(spans);
            }
            Err(e) => {
                errs.push(err_at(e.offset, e.message));
                let before = input.len();
                skip_damaged_line(input);
                if input.len() == before {
                    // no progress: record_row consumed nothing and the
                    // brace-guard declined to eat the damaged line (the
                    // closing brace starts the NEXT one) — skip the line
                    // here so the loop always advances; the next line's
                    // `}` still lands on the block loop's own eat
                    skip_line(input);
                }
            }
        }
    }
    Ok((target, rec))
}

// --- top-level constructs ------------------------------------------------

enum Item {
    Prj(Project),
    Tbl(Table),
    Ref(Ref),
    Enm(Enum),
    Grp(TableGroup),
    Note(StickyNote),
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
    } else if peek_kw(input, "Note") {
        sticky_note(input, base, errs).map(Item::Note)
    } else {
        fail(
            offset(input, base),
            "expected Project, Table, Ref, Enum, Note, or TableGroup",
        )
    }
}

/// A top-level sticky note: `Note name { '''text''' }` — dbdiagram's
/// canvas sticky. The body is one note value (triple- or single-quoted).
fn sticky_note(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<StickyNote> {
    let start = offset(input, base);
    let _ = eat(input, "Note");
    ws(input);
    let name = ident(input, base)?;
    ws(input);
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after note name");
    }
    ws(input);
    let text = match note_value(input, base) {
        Ok(t) => t,
        Err(e) => {
            errs.push(err_at(e.offset, e.message));
            skip_line(input);
            String::new()
        }
    };
    ws(input);
    if !eat(input, "}") {
        // recover to the line end so the top loop can resync
        errs.push(err_at(offset(input, base), "expected '}' after note body"));
        skip_line(input);
    }
    Ok(StickyNote {
        name,
        span: start..offset(input, base),
        text,
    })
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
        checks: None,
        records: None,
        custom: Vec::new(),
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
                    // any other key:value is a dbdiagram custom property
                    _ => t.custom.push((key, v)),
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
        if peek_kw(input, "Records") {
            // records [target][(cols)] [example] { rows } — sample rows
            // shown in the record view
            let _ = eat(input, "Records");
            match records_block(input, base, errs) {
                Ok((_, rec)) => t.records = Some(rec),
                Err(e) => {
                    errs.push(err_at(e.offset, e.message));
                    // a bare `records` fails right at the table's closing
                    // brace — leave it for the block loop instead of
                    // swallowing the rest of the table
                    skip_damaged_line(input);
                }
            }
            continue;
        }
        if peek_kw_then(input, "Checks", '{') {
            let _ = eat(input, "Checks");
            ws(input);
            let _ = eat(input, "{");
            let mut depth = 1i32;
            let bytes = input.as_bytes();
            let mut end = bytes.len();
            for (k, &bch) in bytes.iter().enumerate() {
                if bch == b'{' {
                    depth += 1;
                } else if bch == b'}' {
                    depth -= 1;
                    if depth == 0 {
                        end = k;
                        break;
                    }
                }
            }
            let raw = input[..end].trim().to_string();
            *input = &input[end..];
            let _ = eat(input, "}");
            t.checks = Some(raw);
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
        let op_offset = offset(input, base);
        let (card, l_opt, r_opt) = cardinality_full(input, base)?;
        ws(input);
        let to = endpoint(input, base)?;
        ws(input);
        if !eat(input, "}") {
            return fail(offset(input, base), "expected '}' after ref body");
        }
        let (on_delete, on_update, color) = ref_settings(input, base);
        return Ok(Ref {
            name,
            span: start..offset(input, base),
            from,
            to,
            card,
            op_offset,
            left_optional: l_opt,
            right_optional: r_opt,
            on_delete,
            on_update,
            color,
        });
    }
    if !eat(input, ":") {
        return fail(offset(input, base), "expected ':' after Ref");
    }
    ws(input);
    let from = endpoint(input, base)?;
    ws(input);
    let op_offset = offset(input, base);
    let (card, l_opt, r_opt) = cardinality_full(input, base)?;
    ws(input);
    let to = endpoint(input, base)?;
    let (on_delete, on_update, color) = ref_settings(input, base);
    Ok(Ref {
        name,
        span: start..offset(input, base),
        from,
        to,
        card,
        op_offset,
        left_optional: l_opt,
        right_optional: r_opt,
        on_delete,
        on_update,
        color,
    })
}

/// Optional `[delete: cascade, update: no action, color: #aabbcc]`
/// referential actions + the relationship color.
fn ref_settings(
    input: &mut &str,
    base: usize,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut out = (None, None, None);
    // `ws` before the peek eats blank lines and comments too; when no
    // bracket follows, rewind — ref_stmt takes the Ref's span right
    // after this call, and a span that overshoots to the next item made
    // app.rs's line-color splice rewrite bystander lines (the
    // settings-less `Ref: …` on feature_tour.dbml appended the color to the
    // FOLLOWING comment and glued `Table likes` onto it)
    let before_ws = *input;
    ws(input);
    if !peek_is(input, "[") {
        *input = before_ws;
        return out;
    }
    if let Ok(kvs) = settings_list(input, base) {
        for (k, v) in kvs {
            if let Some(SettingVal::Str(v)) = v {
                match k.as_str() {
                    "delete" | "on_delete" => out.0 = Some(v.to_ascii_uppercase()),
                    "update" | "on_update" => out.1 = Some(v.to_ascii_uppercase()),
                    // `[color: #aabbcc]` — dbdiagram's relationship color
                    "color" => out.2 = Some(v),
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
        let v_off = offset(input, base);
        match ident(input, base) {
            Ok(vname) => {
                let v_end = offset(input, base);
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
                                name_span: v_off..v_end,
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
                e.values.push(EnumValue {
                    name: vname,
                    name_span: v_off..v_end,
                    note,
                });
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
    // optional settings before the body: `[color: #1E69FD]` plus any
    // other `key: "value"` pairs — dbdiagram custom properties, in order
    let mut color = None;
    let mut custom: Vec<(String, String)> = Vec::new();
    if peek_is(input, "[") {
        if let Ok(kvs) = settings_list(input, base) {
            for (k, v) in kvs {
                if let (Some(SettingVal::Str(sv)), "color") = (&v, k.as_str()) {
                    color = Some(sv.clone());
                } else if let Some(SettingVal::Str(sv)) = &v {
                    custom.push((k, sv.clone()));
                }
            }
        }
        ws(input);
    }
    if !eat(input, "{") {
        return fail(offset(input, base), "expected '{' after group name");
    }
    let mut g = TableGroup {
        name,
        span: start..0,
        tables: Vec::new(),
        color,
        note: None,
        custom,
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
        // `Note: '''...'''` — the group's note (label-hover tooltip)
        if peek_kw_then(input, "Note", ':') {
            let _ = eat(input, "Note");
            ws(input);
            let _ = eat(input, ":");
            ws(input);
            match note_value(input, base) {
                Ok(n) => g.note = Some(n),
                Err(e) => {
                    errs.push(err_at(e.offset, e.message));
                    skip_line(input);
                }
            }
            continue;
        }
        match qualified_name(input, base) {
            Ok((sc, n)) => g.tables.push(sc.map_or(n.clone(), |s| format!("{s}.{n}"))),
            Err(e) => {
                // dbdiagram's phrasing for a member that is not a table
                // reference (a comma, a setting, stray punctuation...)
                let msg = if e.message.contains("identifier") {
                    "A TableGroup field must be of the form <table> or <schema>.<table>"
                } else {
                    e.message
                };
                errs.push(err_at(e.offset, msg));
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
const TOP_KWS: [&str; 6] = ["Project", "Table", "Ref", "Enum", "TableGroup", "Records"];

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
    let mut top_records: Vec<(Option<String>, TableRecords)> = Vec::new();
    loop {
        ws(&mut rest);
        if rest.is_empty() {
            break;
        }
        // records [target][(cols)] [example] { rows } — sample rows for a
        // table, possibly defined later in the file
        if peek_kw(rest, "Records") {
            let mut attempt = rest;
            let _ = eat(&mut attempt, "Records");
            match records_block(&mut attempt, base, &mut out.errors) {
                Ok((target, rec)) => {
                    rest = attempt;
                    top_records.push((target, rec));
                }
                Err(e) => {
                    out.errors.push(ParseError {
                        offset: e.offset,
                        message: e.message.to_string(),
                    });
                    rest = recover(attempt);
                }
            }
            continue;
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
                    Item::Note(n) => out.doc.notes.push(n),
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
    // Top-level records blocks attach to their table, which may be defined
    // later in the file (the target matches by name or alias; unmatched
    // targets are dropped — the diagram simply has nothing to show).
    for (target, rec) in top_records {
        let Some(name) = target else {
            continue;
        };
        if let Some(t) = out.doc.tables.iter_mut().find(|t| {
            t.name == name || t.alias.as_deref() == Some(name.as_str())
        }) {
            t.records = Some(rec);
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
    #[test]
    fn inline_ref_endpoint_color_parses() {
        // `ref: > t.c [color: …]` — the endpoint settings bracket carries
        // the relationship's color (the picker writes it)
        let out = parse("Table a { id integer [pk] }\nTable b { user_id integer [ref: > a.id [color: #ff0000]] }\n");
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let col = &out.doc.tables[1].columns[0];
        let ir = col.settings.inline_ref.as_ref().expect("inline ref");
        assert_eq!(ir.color.as_deref(), Some("#ff0000"));
        assert_eq!(ir.to.table, "a");
        // and a bracket-less inline ref keeps color None
        let out = parse("Table a { id integer [pk] }\nTable b { user_id integer [ref: > a.id] }\n");
        let ir = &out.doc.tables[1].columns[0].settings.inline_ref;
        assert!(ir.as_ref().unwrap().color.is_none());
    }

    #[test]
    fn custom_properties_parse_on_tables_and_columns() {
        // the user's live-verified dbdiagram block: any `key: "value"`
        // pair inside the settings brackets are custom properties
        let src = "Table users [owner: \"data-team\", sla_hours: \"24\", pii: \"true\"] {\n  id int [pk, masking: \"partial\"]\n  email varchar [classification: \"confidential\"]\n}\n";
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let t = &out.doc.tables[0];
        assert_eq!(
            t.custom,
            vec![
                ("owner".to_string(), "data-team".to_string()),
                ("sla_hours".to_string(), "24".to_string()),
                ("pii".to_string(), "true".to_string()),
            ]
        );
        assert_eq!(
            t.columns[0].settings.custom,
            vec![("masking".to_string(), "partial".to_string())]
        );
        assert_eq!(
            t.columns[1].settings.custom,
            vec![("classification".to_string(), "confidential".to_string())]
        );
        // known settings still take their own fields, not custom
        assert!(t.columns[0].settings.pk);
        // single-quoted custom values work too
        let out = parse("Table t [tier: 'gold'] { a int [zone: 'a1'] }\n");
        assert_eq!(
            out.doc.tables[0].custom,
            vec![("tier".to_string(), "gold".to_string())]
        );
        assert_eq!(
            out.doc.tables[0].columns[0].settings.custom,
            vec![("zone".to_string(), "a1".to_string())]
        );
    }


    use super::*;

    fn tables(out: &ParseOutput) -> &[Table] {
        &out.doc.tables
    }

    #[test]
    fn bare_comma_index_settings_parse_but_flag_comma_form() {
        // the form some importers emit; dbdiagram's grammar rejects it.
        // The parse recovers and marks the index, the validator turns the
        // flag into a quick-fixable diagnostic (non-blocking: still formats)
        let src = "Table t {\n  id integer\n  indexes {\n    (id), unique, name: 'x'\n  }\n}\n";
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let ix = &out.doc.tables[0].indexes[0];
        assert!(ix.comma_form);
        assert!(ix.unique);
        assert_eq!(ix.name.as_deref(), Some("x"));
        // non-blocking: the doc still formats (the fixture runner and
        // :format depend on that)
        assert!(super::super::fmt::format_source(src).is_some());
    }

    #[test]
    fn bracketed_index_settings_stay_silent() {
        let src = "Table t {\n  id integer\n  indexes {\n    (id) [unique, name: 'x']\n  }\n}\n";
        let out = parse(src);
        assert!(out.errors.is_empty());
        assert!(!out.doc.tables[0].indexes[0].comma_form);
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
        assert_eq!(&src[e.values[0].name_span.clone()], "a");
        assert_eq!(&src[t.span.clone()], "Table users {\n  id integer\n}");
    }

    #[test]
    fn column_name_spans_cover_the_name_token() {
        // plain, quoted (the quotes stay in the span, like table name
        // spans), backticked, and an inline-ref column (the ref settings
        // must not shift the name span)
        let src = concat!(
            "Table t {\n",
            "  id integer [pk]\n",
            "  \"full name\" varchar [ref: > u.id]\n",
            "  `weird` int\n",
            "}\n",
            "Table u { id int [pk] }\n"
        );
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let cols = &tables(&out)[0].columns;
        assert_eq!(&src[cols[0].name_span.clone()], "id");
        assert_eq!(&src[cols[1].name_span.clone()], "\"full name\"");
        assert!(
            cols[1].settings.inline_ref.is_some(),
            "inline ref parsed alongside the span"
        );
        assert_eq!(&src[cols[2].name_span.clone()], "`weird`");
        // every column's span starts on its own definition line (the
        // click-to-jump target)
        for (i, c) in cols.iter().enumerate() {
            let (line, _) = line_col_at(src, c.name_span.start);
            assert_eq!(line, i + 1, "column {}: {:?}", i, c.name);
        }
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

    #[test]
    fn records_explicit_columns_and_values() {
        let src = r#"
Table posts {
  id int [pk]
  title varchar
  published boolean
  views decimal
  score real
  status post_status
  created_at timestamp
  records (id, title, published, views, score, status, created_at) [example] {
    1, 'First Post', true, 2.50, 1e3, draft, `now()`
    2, 'It''s fine', false, -3, .5, NULL,
  }
}
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let rec = tables(&out)[0].records.as_ref().expect("records");
        assert!(rec.example);
        assert_eq!(
            rec.columns,
            [
                "id", "title", "published", "views", "score", "status", "created_at"
            ]
        );
        assert_eq!(rec.rows.len(), 2);
        use RecordValue::*;
        assert_eq!(
            rec.rows[0],
            vec![
                Number("1".into()),
                Text("First Post".into()),
                Bool(true),
                Number("2.50".into()),
                Number("1e3".into()),
                Ident("draft".into()),
                Expr("now()".into()),
            ]
        );
        // '' escape, negative and leading-dot numbers, case-insensitive
        // null, and a trailing comma (an empty final field is a null)
        assert_eq!(
            rec.rows[1],
            vec![
                Number("2".into()),
                Text("It's fine".into()),
                Bool(false),
                Number("-3".into()),
                Number(".5".into()),
                Null,
                Null,
            ]
        );
    }

    #[test]
    fn records_implicit_columns_and_top_level_form() {
        let src = r#"
records users(id, name) [example] {
  1, 'Alice'
  2, null
}

Table users {
  id int [pk]
  name varchar
}

Table posts {
  id int [pk]
  records {
    7, 'x'
  }
}
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let users = tables(&out).iter().find(|t| t.name == "users").unwrap();
        let rec = users.records.as_ref().expect("attached");
        assert!(rec.example);
        assert_eq!(rec.columns, ["id", "name"]);
        assert_eq!(
            rec.rows,
            vec![
                vec![RecordValue::Number("1".into()), RecordValue::Text("Alice".into())],
                vec![RecordValue::Number("2".into()), RecordValue::Null],
            ]
        );
        // bare records { } inside a table: implicit all-columns order
        let posts = tables(&out).iter().find(|t| t.name == "posts").unwrap();
        let prec = posts.records.as_ref().expect("implicit records");
        assert!(prec.columns.is_empty(), "implicit: resolved by the model");
        assert_eq!(prec.rows.len(), 1);
        assert_eq!(prec.rows[0].len(), 2);
        assert!(!prec.example);
    }

    #[test]
    fn records_target_may_be_schema_qualified_or_an_alias() {
        let src = r#"
records public.users(U) {
  1
}
Table users as U {
  id int [pk]
}
"#;
        let out = parse(src);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(tables(&out)[0].records.is_some(), "attached to users");
    }

    #[test]
    fn records_row_count_mismatch_recovered_per_line() {
        let src = r#"
Table t {
  id int [pk]
  name varchar
  records (id, name) {
    1, 'ok'
    2, 'too', 'many'
    3, 'fine'
  }
}
"#;
        let out = parse(src);
        assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
        let rec = tables(&out)[0].records.as_ref().unwrap();
        assert_eq!(
            rec.rows,
            vec![
                vec![RecordValue::Number("1".into()), RecordValue::Text("ok".into())],
                vec![RecordValue::Number("3".into()), RecordValue::Text("fine".into())],
            ]
        );
    }

    #[test]
    fn records_partial_input_is_tolerated() {
        // mid-typing: bare keyword, unterminated header, no braces yet —
        // the document must keep parsing (the diagram re-parses on every
        // keystroke) and later tables survive
        for src in [
            "Table t {\n  id int [pk]\n  records\n}\nTable u { x int }",
            "Table t {\n  id int [pk]\n  records (a, b\n}\nTable u { x int }",
            "records\nTable u { x int }",
            "records users(\nTable u { x int }",
        ] {
            let out = parse(src);
            assert!(!out.errors.is_empty(), "reports the damage: {src:?}");
            let names: Vec<&str> = tables(&out).iter().map(|t| t.name.as_str()).collect();
            assert!(names.contains(&"u"), "later table survives {src:?}");
        }
    }

    #[test]
    fn records_keyword_is_case_insensitive_and_strings_escape() {
        let out = parse("TABLE t { id int [pk]\n  RECORDS (id) { 1 }\n}");
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert!(tables(&out)[0].records.is_some());
        // \' escapes like '' inside record strings
        let out = parse("Table t { id int [pk]\n  records (a) { 'it\\'s' }\n}");
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            tables(&out)[0].records.as_ref().unwrap().rows[0],
            vec![RecordValue::Text("it's".into())]
        );
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
        // enums are not boxes (an "Enum" pill rides their columns instead)
        assert_eq!(canonical.boxes.len(), 2);
        assert_eq!(canonical.enums.len(), 1);
        assert_eq!(canonical.enums[0].name, "post_status");
        assert_eq!(canonical.edges.len(), 2, "inline + top-level Ref");
        let users = canonical.boxes.iter().find(|b| b.title == "users").unwrap();
        assert_eq!((users.x, users.y), (0, 0));
    }
}
