//! Hand-written recursive-descent DBML parser.
//!
//! Design goals: byte-accurate spans for editor integration, and error
//! recovery at statement / block-line granularity so partially typed
//! documents still render while editing.

use super::ast::*;

#[derive(Debug, Clone)]
pub struct ParseError {
    pub offset: usize,
    /// The squiggle's end when the finding spans tokens (their endColumn)
    pub end: Option<usize>,
    pub message: String,
}

#[derive(Debug, Default, Clone)]
pub struct ParseOutput {
    pub doc: Document,
    pub errors: Vec<ParseError>,
    /// Their validateNode-class findings raised while parsing (broken-ref
    /// shape sets) — binder-class, gate the interpretation layer. Each
    /// carries its token end (their endColumn) when the node spans.
    pub binder_errors: Vec<(usize, usize, String)>,
    /// The ref-body binder requests (their bindFields scan) — resolved
    /// against the document in validate.rs.
    pub binder_chains: Vec<super::ast::BinderChain>,
    /// Records rows whose value count mismatches the column list
    /// (anchor, expected, got) — their extractDataFromRow rule; the
    /// error is interpret-layer and only exists while the file is
    /// otherwise clean (gated in validate.rs).
    pub record_count_errors: Vec<(usize, usize, usize, usize)>,
}

struct Fail {
    offset: usize,
    end: Option<usize>,
    message: String,
}

type R<T> = Result<T, Fail>;

fn offset(input: &str, base: usize) -> usize {
    // pointer differences against `base`: every input handed here must be
    // a slice of the parsed source (the skip helpers above keep their
    // exhausted-state slices inside it — a foreign "" would underflow)
    input.as_ptr() as usize - base
}

fn fail<T>(o: usize, m: impl Into<String>) -> R<T> {
    Err(Fail {
        offset: o,
        end: None,
        message: m.into(),
    })
}

fn err_at(o: usize, m: &str) -> ParseError {
    ParseError {
        offset: o,
        end: None,
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
        // their quotedVariable: consumeUntil("\"", raw: false) — escapes
        // decode inline and cannot end the token
        let mut name = String::new();
        let mut r = rest;
        loop {
            if r.is_empty() {
                return fail(offset(input, base), "unterminated quoted identifier");
            }
            if r.starts_with('\\') {
                r = &r[1..];
                let mut tmp = r;
                let dec = escaped_string(&mut tmp, base)?;
                name.push_str(&dec);
                r = tmp;
                continue;
            }
            if r.starts_with('"') {
                *input = &r[1..];
                return Ok(name);
            }
            let c = r.chars().next().unwrap();
            name.push(c);
            r = &r[c.len_utf8()..];
        }
    }
    if let Some(rest) = input.strip_prefix('`') {
        let end = rest.find('`').ok_or(Fail {
            offset: offset(input, base),
            end: None,
            message: "unterminated backticked identifier".to_string(),
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

// Their lexer's escapedString (core/lexer/lexer.ts): one backslash
// escape, decoded inline — the string token carries the DECODED value.
fn invalid_unicode_err(hex: &str) -> String {
    format!("Invalid unicode escape sequence '\\u{hex}', only unicode escape sequences of the form '\\uHHHH' where H is a hexadecimal number are allowed")
}

fn escaped_string(input: &mut &str, base: usize) -> R<String> {
    let mut chars = input.chars();
    let Some(c) = chars.next() else {
        return Ok("\\ ".to_string());
    };
    *input = &input[c.len_utf8()..];
    Ok(match c {
        '\r' => {
            if input.starts_with('\n') {
                *input = &input[1..];
            }
            String::new()
        }
        '\n' => String::new(),
        't' => "\t".to_string(),
        'n' => "\n".to_string(),
        '\\' => "\\".to_string(),
        'r' => "\r".to_string(),
        '\'' => "'".to_string(),
        '"' => "\"".to_string(),
        '`' => "`".to_string(),
        '0' => "\0".to_string(),
        'b' => "\u{8}".to_string(),
        'v' => "\u{b}".to_string(),
        'f' => "\u{c}".to_string(),
        ' ' => "\\ ".to_string(),
        'u' => {
            let mut hex = String::new();
            for _ in 0..4 {
                let Some(h) = input.chars().next() else {
                    return fail(offset(input, base), &invalid_unicode_err(&hex));
                };
                if !h.is_ascii_hexdigit() {
                    return fail(offset(input, base), &invalid_unicode_err(&hex));
                }
                hex.push(h);
                *input = &input[h.len_utf8()..];
            }
            let cp = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
            char::from_u32(cp).unwrap_or('\u{FFFD}').to_string()
        }
        other => other.to_string(),
    })
}

/// Single-quoted string — their consumeUntil("'", raw: false): backslash
/// escapes decode inline (so an escaped quote cannot end the string),
/// there is no `''` doubling, and a newline or EOF inside is their parse
/// error.
fn sq_string(input: &mut &str, base: usize) -> R<String> {
    if !eat(input, "'") {
        return fail(offset(input, base), "expected 'string'");
    }
    let mut out = String::new();
    loop {
        if input.is_empty() {
            return fail(offset(input, base), "EOF reached while parsing");
        }
        if input.starts_with('\n') {
            return fail(
                offset(input, base),
                "Invalid newline encountered while parsing",
            );
        }
        if input.starts_with('\\') {
            *input = &input[1..];
            out.push_str(&escaped_string(input, base)?);
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

/// Their normalizeNote (core/utils/interpret.ts): drop leading empty
/// lines, then strip the smallest common indent of the non-empty lines —
/// the trailing (empty-ish) line keeps its newline.
fn normalize_note(content: &str) -> String {
    let lines: Vec<&str> = content.split('\n').collect();
    let start = lines
        .iter()
        .position(|l| !l.trim_start().is_empty())
        .unwrap_or(lines.len());
    let trimmed = &lines[start..];
    let non_empty = trimmed.iter().filter(|l| !l.trim_start().is_empty());
    let Some(min_indent) = non_empty.map(|l| l.len() - l.trim_start().len()).min() else {
        return trimmed.join("\n");
    };
    trimmed
        .iter()
        .map(|l| l.get(min_indent..).unwrap_or(l.trim_start()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn note_value(input: &mut &str, base: usize) -> R<String> {
    if input.starts_with('"') {
        return ident(input, base);
    }
    if eat(input, "'''") {
        // their consumeUntil("'''", allowNewline, raw: false): escapes
        // decode inline and cannot end the string
        let mut raw = String::new();
        loop {
            if input.is_empty() {
                return fail(offset(input, base), "EOF reached while parsing");
            }
            if input.starts_with("'''") {
                *input = &input[3..];
                return Ok(normalize_note(&raw));
            }
            if input.starts_with('\\') {
                *input = &input[1..];
                raw.push_str(&escaped_string(input, base)?);
                continue;
            }
            let c = input.chars().next().unwrap();
            raw.push(c);
            *input = &input[c.len_utf8()..];
        }
    } else {
        let v = sq_string(input, base)?;
        Ok(normalize_note(&v))
    }
}

/// Column type, e.g. `varchar(255)`, `decimal(10,2)`, or a qualified /
/// quoted name (`"Var Char"`, `user.e`, `"user"."e"`).
/// Column type: one word — quoted, dotted, then optional parenthesized
/// args (`varchar(255)`, `decimal(10, 2)`, `"character varying"`). Their
/// grammar has no multi-word types: `timestamp with time zone` reads as
/// type `timestamp` plus the bare fields `with`, `time`, `zone`.
fn type_str(input: &mut &str, base: usize) -> R<String> {
    let mut out = String::new();
    loop {
        if let Some(rest) = input.strip_prefix('"') {
            let end = rest.find('"').ok_or(Fail {
                offset: offset(input, base),
                end: None,
                message: "unterminated quoted type".to_string(),
            })?;
            out.push('"');
            out.push_str(&rest[..end + 1]); // verbatim, both quotes
            *input = &rest[end + 1..];
        } else {
            let end = input
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(input.len());
            out.push_str(&input[..end]);
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
    // parenthesized args ride the word, spaces inside them intact —
    // `decimal (10, 2)` keeps the gap, `varchar(255)` does not grow one
    let save = *input;
    sp(input);
    if peek_is(input, "(") {
        let mut depth = 0i32;
        let mut end = input.len();
        for (k, &c) in input.as_bytes().iter().enumerate() {
            match c {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = k + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push_str(&input[..end]);
        *input = &input[end..];
    } else {
        *input = save;
    }
    // array suffixes bind to the type with NO space (`int[]`, `int[3]`,
    // multi-dimensional `int[2][3]`) — a spaced `int []` is a settings
    // list instead (probe-verified on their engine)
    while input.starts_with('[') {
        let after = &input[1..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let close = 1 + digits.len();
        if after[digits.len()..].starts_with(']') {
            out.push_str(&input[..close + 1]);
            *input = &input[close + 1..];
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
        // longest first, so `?<` cannot shadow `?<>` / `?<>?`
        ("?<>?", Cardinality::ManyToMany, true, true),
        ("?>?", Cardinality::ManyToOne, true, true),
        ("?<?", Cardinality::OneToMany, true, true),
        ("?-?", Cardinality::OneToOne, true, true),
        ("?<>", Cardinality::ManyToMany, true, false),
        ("<>?", Cardinality::ManyToMany, false, true),
        ("?>", Cardinality::ManyToOne, true, false),
        (">?", Cardinality::ManyToOne, false, true),
        ("?<", Cardinality::OneToMany, true, false),
        ("<?", Cardinality::OneToMany, false, true),
        ("?-", Cardinality::OneToOne, true, false),
        ("-?", Cardinality::OneToOne, false, true),
        ("<>", Cardinality::ManyToMany, false, false),
        (">", Cardinality::ManyToOne, false, false),
        ("<", Cardinality::OneToMany, false, false),
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
            if peek_is(input, ")") {
                break;
            }
            if !eat(input, ",") {
                return fail(offset(input, base), "Expect a comma ','");
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
    Ref(Cardinality, bool, bool, Endpoint, usize, usize, Option<String>),
}

/// `[key, key: value, ref: > t.c, ...]` — parsed generically; callers
/// interpret the pairs.
fn settings_list(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Vec<(String, Option<SettingVal>)>> {
    settings_list_occs(input, base, errs).map(|(kvs, _)| kvs)
}

/// The settings list plus every key occurrence (their attribute nodes —
/// the duplicate/conflict rules anchor each one).
fn settings_list_occs(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
) -> R<(Vec<(String, Option<SettingVal>)>, Vec<super::ast::SettingOcc>)> {
    if !eat(input, "[") {
        return fail(offset(input, base), "expected '['");
    }
    let mut occs: Vec<super::ast::SettingOcc> = Vec::new();
    let mut out = Vec::new();
    loop {
        // their lexer treats newlines/comments as trivia inside the
        // bracket list (probe: `[pk,\n not null, unique]` parses clean)
        ws(input);
        if eat(input, "]") {
            return Ok((out, occs));
        }
        if input.is_empty() {
            return fail(offset(input, base), "unterminated settings");
        }
        let key_start = offset(input, base);
        let end = input
            .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ' ' || c == '\t'))
            .unwrap_or(input.len());
        let key = input[..end].trim().to_ascii_lowercase();
        if key.is_empty() {
            return fail(offset(input, base), "expected setting key");
        }
        let occ_key = key.clone();
        *input = &input[end..];
        sp(input);
        let occ_before_value = input.len();
        let mut val_quoted = false;
        if eat(input, ":") {
            sp(input);
            // a newline between `:` and the value is trivia too
            ws(input);
            sp(input);
            // quotedness must be read BEFORE the value parses — the
            // parsed value arrives unquoted (their
            // isExpressionAQuotedString)
            val_quoted = input.starts_with('\'') || input.starts_with('"');
            if key == "ref" {
                let op_offset = offset(input, base);
                let (card, l_opt, r_opt) = cardinality_full(input, base)?;
                ws(input);
                // their endpoint grammar: a bare word is a column of the
                // HOST table, and words after a parsed endpoint are
                // application args — each arg errs once (their
                // INVALID_REF_FIELD, a binder-class error)
                let ep_start = *input;
                let ep_start_off = offset(input, base);
                let to = match endpoint(input, base) {
                    Ok(to) => to,
                    Err(_) => {
                        // a bare word = the host table's column — read from
                        // BEFORE the failed attempt (endpoint consumes its
                        // first ident before failing on the missing dot)
                        *input = ep_start;
                        let w = match ident(input, base) {
                            Ok(w) => w,
                            Err(_) => {
                                *input = ep_start;
                                return fail(ep_start_off, "expected a reference endpoint");
                            }
                        };
                        super::ast::Endpoint {
                            table: String::new(),
                            schema: None,
                            columns: vec![w],
                            span: ep_start_off..offset(input, base),
                        }
                    }
                };
                // junk words after the endpoint are the settings tail's
                // "Expect a comma ','" (their settings consume) — the
                // endpoint still binds; the single-binary error belongs to
                // STANDALONE ref lines only (probe mw1 vs mw2)
                ws(input);
                // endpoint settings bracket: `ref: > t.c [color: …]` —
                // the relationship's color lives there (only `color` is
                // interpreted; other keys ride along unused)
                let mut color = None;
                if peek_is(input, "[") {
                    if let Ok(kvs) = settings_list(input, base, errs) {
                        for (k, v) in kvs {
                            if let (Some(SettingVal::Str(sv)), "color") = (&v, k.as_str()) {
                                color = Some(sv.clone());
                            }
                        }
                    }
                }
                out.push((key, Some(SettingVal::Ref(card, l_opt, r_opt, to, op_offset, key_start, color))));
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
        // the key's occurrence: the value text when it parsed as a plain
        // string (verbatim, quotes kept), plus whether the value was a
        // quoted string literal
        let last_val = match out.last() {
            Some((_, Some(SettingVal::Str(v)))) => Some(v.clone()),
            _ => None,
        };
        occs.push(super::ast::SettingOcc {
            key: occ_key,
            offset: key_start,
            end: offset(input, base),
            has_value: occ_before_value != input.len(),
            quoted: val_quoted,
            value: last_val,
        });
        if eat(input, ",") {
            continue;
        }
        sp(input);
        if eat(input, "]") {
            return Ok((out, occs));
        }
        // their recovery: the parse error is recorded and the list still
        // yields what parsed (probe: `ref: -? orders.odd col` keeps the
        // endpoint, and the binder flags the missing column)
        errs.push(err_at(offset(input, base), "Expect a comma ','"));
        // sync to the bracket's close, else the line end — exhausting the
        // whole source here would swallow every later construct
        let nl = input.find('\n');
        match (input.find(']'), nl) {
            (Some(k), _) => *input = &input[k + 1..],
            (None, Some(nl)) => *input = &input[nl + 1..],
            (None, None) => *input = &input[input.len()..],
        }
        return Ok((out, occs));
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
            Some(SettingVal::Ref(card, l_opt, r_opt, to, op_offset, attr_start, color)) => {
                if s.inline_ref.is_none() {
                    s.inline_ref = Some(InlineRef { card, left_optional: l_opt, right_optional: r_opt, to, op_offset, attr_start, color });
                }
            }
        }
    }
    s
}

fn column(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Column> {
    let start = offset(input, base);
    let name_off = offset(input, base);
    let name = ident(input, base)?;
    let name_end = offset(input, base);
    sp(input);
    let mut binder_errors: Vec<(usize, String)> = Vec::new();
    // their validateFields (table/validate.ts:112): a column field with
    // args=[] errs 'A column must have a type' anchored at the callee
    // (the name). The type is missing when the line/scope ends — or when
    // the type token was an invalid number, which their lexer gathered
    // out (with its 'Invalid number' error) leaving only the name. An
    // attribute LIST at the type position is the field's arg —
    // 'Invalid column type' at that list (probe c5/c6).
    let ty: String;
    if input.is_empty() || input.starts_with(['\n', '\r', '}']) {
        binder_errors.push((name_off, "A column must have a type".to_string()));
        ty = String::new();
    } else if input.starts_with('[') {
        ty = String::new();
    } else if input.starts_with(|c: char| c.is_ascii_digit())
        && matches!(
            numeric_lit_or_ident(input.as_bytes(), 0),
            NumScan::Invalid(_)
        )
    {
        // the type token is an invalid-number run (their invalid token is
        // gathered out of the stream; args stay empty)
        let run_start = offset(input, base);
        let n = match numeric_lit_or_ident(input.as_bytes(), 0) {
            NumScan::Invalid(n) => n,
            _ => unreachable!(),
        };
        errs.push(err_at(run_start, "Invalid number"));
        *input = &input[n..];
        binder_errors.push((name_off, "A column must have a type".to_string()));
        ty = String::new();
    } else {
        ty = type_str(input, base)?;
    }
    sp(input);
    // their column grammar's inline-settings fields: bare `pk`/`unique`
    // words (case-insensitive) set the flag; any other bare word errs
    // ("Inline column settings can only be `pk` or `unique`") — unless it
    // drags a `{ … }` block behind it (`indexes {`, `notes {`), which
    // errs twice ("These fields must be some inline settings optionally
    // ended with a setting list", at the word and the brace) and swallows
    // the balanced block before the column ends. Words may follow a
    // setting list too (probe c6: `id [pk] integer` errs at `integer`).
    let mut inline_pk = false;
    let mut inline_unique = false;
    let mut settings: Option<super::ast::ColumnSettings> = None;
    // the FIRST list stands in as the (invalid) type when neither
    let mut ty_is_list = false;
    loop {
        sp(input);
        if input.is_empty() || input.starts_with(['\n', '\r', '}', '#']) {
            break;
        }
        if peek_is(input, "[") {
            let settings_start = offset(input, base);
            let (kvs, occs) = settings_list_occs(input, base, errs)?;
            if ty.is_empty() && !ty_is_list {
                // the list became the field's arg — no valid column type
                binder_errors.push((settings_start, "Invalid column type".to_string()));
                ty_is_list = true;
            }
            // a SECOND bracket list on the same line is their fields error,
            // anchored at both lists (probe: `int [] [pk]` errs at `[` and `[`)
            sp(input);
            if peek_is(input, "[") {
                let msg = "These fields must be some inline settings optionally ended with a setting list";
                binder_errors.push((settings_start, msg.to_string()));
                binder_errors.push((offset(input, base), msg.to_string()));
                swallow_balanced_braces(input);
            }
            let mut s = column_settings_from(kvs);
            s.occurrences = occs;
            settings = Some(s);
            continue;
        }
        let off = offset(input, base);
        let save = *input;
        let Ok(word) = ident(input, base) else {
            *input = save;
            break;
        };
        sp(input);
        if peek_is(input, "{") {
            // their validateNode class — these gate the interpretation
            // layer, so they ride the column for validate() to emit
            let msg = "These fields must be some inline settings optionally ended with a setting list";
            binder_errors.push((off, msg.to_string()));
            binder_errors.push((offset(input, base), msg.to_string()));
            swallow_balanced_braces(input);
            break;
        }
        match word.to_ascii_lowercase().as_str() {
            "pk" => inline_pk = true,
            "unique" => inline_unique = true,
            _ => binder_errors.push((
                off,
                "Inline column settings can only be `pk` or `unique`".to_string(),
            )),
        }
    }
    let mut out_settings = settings.unwrap_or_default();
    out_settings.pk |= inline_pk;
    out_settings.unique |= inline_unique;
    Ok(Column {
        name,
        name_span: name_off..name_end,
        ty,
        span: start..offset(input, base),
        binder_errors,
        settings: out_settings,
    })
}

/// Skip a balanced `{ … }` block (nested braces included) — the recovery
/// path after the misplaced-block field error.
fn swallow_balanced_braces(input: &mut &str) {
    let mut depth = 0i32;
    for (k, &c) in input.as_bytes().iter().enumerate() {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    *input = &input[k + 1..];
                    return;
                }
            }
            _ => {}
        }
    }
    // exhaust within the source: a foreign "" slice would break the
    // pointer-difference offsets
    *input = &input[input.len()..];
}


/// Their check-block grammar: each field is a backticked function
/// expression, optionally followed by a settings list carrying `name`.
/// A non-backticked field is their error.
fn parse_check_entries(
    raw: &str,
    raw_base: usize,
    errs: &mut Vec<ParseError>,
) -> Vec<super::ast::TableCheck> {
    let mut out = Vec::new();
    let mut rest = raw;
    loop {
        let skipped = rest.len() - rest.trim_start().len();
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let _ = skipped;
        let entry_off = raw_base + (raw.len() - rest.len());
        if let Some(exp) = rest.strip_prefix('`') {
            let Some(close) = exp.find('`') else {
                errs.push(err_at(entry_off, "unterminated check expression"));
                break;
            };
            let expression = exp[..close].to_string();
            rest = &exp[close + 1..];
            let mut name = None;
            let after = rest.trim_start();
            if after.starts_with('[') {
                let Some(close_b) = after.find(']') else {
                    errs.push(err_at(entry_off, "unterminated check settings"));
                    break;
                };
                let inner = &after[1..close_b];
                for kv in inner.split(',') {
                    let kv = kv.trim();
                    if let Some((k, v)) = kv.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("name") {
                            name = Some(v.trim().trim_matches('\'').to_string());
                        }
                    }
                }
                rest = &after[close_b + 1..];
            }
            out.push(super::ast::TableCheck { expression, name });
        } else {
            errs.push(err_at(entry_off, "A check field must be a function expression"));
            // skip to the next backtick entry or the end
            match rest.find('`') {
                Some(k) => rest = &rest[k..],
                None => break,
            }
        }
    }
    out
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
    // their grammar never joins bare words here: `odd col` reads as two
    // columns, each flagged separately (probe-verified)
    ident(input, base)
}

fn index(input: &mut &str, base: usize, errs: &mut Vec<ParseError>) -> R<Index> {
    let start = offset(input, base);
    let mut columns = Vec::new();
    let mut expressions = Vec::new();
    let mut column_spans = Vec::new();
    if eat(input, "(") {
        loop {
            ws(input);
            if peek_is(input, ")") {
                break;
            }
            let is_expr = input.starts_with('`');
            let col_start = offset(input, base);
            columns.push(index_ident(input, base)?);
            column_spans.push(col_start..offset(input, base));
            expressions.push(is_expr);
            ws(input);
            if peek_is(input, ")") {
                break;
            }
            // their tuple grammar requires the separator (parser.ts
            // "Expect a comma ','") — but the recovery keeps what parsed:
            // the error is recorded, the offending token skipped, and the
            // tuple continues (probe: `(odd col)` flags odd AND errs at
            // col, keeping the index)
            if !eat(input, ",") {
                errs.push(err_at(offset(input, base), "Expect a comma ','"));
                if ident(input, base).is_err() {
                    return fail(offset(input, base), "Expect a comma ','");
                }
                ws(input);
                if peek_is(input, ")") {
                    break;
                }
                if !eat(input, ",") {
                    return fail(offset(input, base), "Expect a comma ','");
                }
            }
        }
        if !eat(input, ")") {
            return fail(offset(input, base), "expected ')' after index columns");
        }
    } else {
        let is_expr = input.starts_with('`');
        let col_start = offset(input, base);
        columns.push(index_ident(input, base)?);
        column_spans.push(col_start..offset(input, base));
        expressions.push(is_expr);
    }
    sp(input);
    let mut comma_form = false;
    let mut idx = Index {
        columns,
        expressions,
        column_spans,
        unique: false,
        pk: false,
        name: None,
        index_type: None,
        span: start..0,
        comma_form: false,
    };
    if peek_is(input, "[") {
        let kvs = settings_list(input, base, errs)?;
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

/// Their numericLiteralOrIdentifier (lexer.ts:439) verbatim — reached at a
/// DIGIT (`i0` is its offset; their scanTokens has already advance()d that
/// first digit, so the walk starts one past it and the token always spans
/// from `i0`):
///
/// - the run continues into alphanumerics → ONE identifier token (`2z3`,
///   `1e+2x`, `1_000`, `2.e` — chars.ts isAlphaNumeric = \p{L}|_|\p{M}|digit);
/// - dots count: a lone trailing `2.` before whitespace/other is a VALID
///   literal whose text includes the dot; more than one dot, or an
///   alphanumeric after the lone dot (`12.3abc`, `2.5x`), is an INVALID
///   number: token kind NUMERIC_LITERAL isInvalid, gathered out of the
///   stream by their gatherInvalid with an 'Invalid number' error;
/// - one e-exponent: valid only with a digit after the optional +/- sign
///   (`1e+2`); if alphanumerics continue after the exponent (`1e2abc`)
///   the whole run is an identifier; an e without an exponent (`2e`, `2e+`)
///   breaks — never consumed by the numeric walk — and the identifier run
///   picks the alphanumerics up from there (`2e` → ident, `2e+` → ident `2e`
///   plus an OP token).
///
/// The gathered-invalid arm is the reason a junk run inside a records row
/// or a ref body vanishes instead of splitting into value/operand tokens.
enum NumScan {
    /// clean numeric literal (text length, includes any `2.` tail or `1e-2`)
    Numeric(usize),
    /// digit-led identifier (text length)
    IdentToken(usize),
    /// invalid number, token gathered out (text length)
    Invalid(usize),
}

/// as above, but `sentinel` is the byte that follows this slice in the
/// real source (`None` = the slice ends the file): their lexer scans the
/// WHOLE source, so a run sitting at a line boundary (`2.` before the
/// newline, an exponent at end-of-line) must evaluate against that next
/// byte, not an artificial end-of-text. The sentinel is never consumed.
fn numeric_lit_or_ident_sentinel(b: &[u8], i0: usize, sentinel: Option<u8>) -> NumScan {
    let ch = |i: usize, look: usize| -> Option<u8> {
        if i + look < b.len() {
            Some(b[i + look])
        } else {
            sentinel
        }
    };
    let is_alnum = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80;
    let mut n_dots = 0usize;
    let mut i = i0 + 1;
    if ch(i, 0).is_none() {
        return NumScan::Numeric(i - i0);
    }
    loop {
        let c0 = match ch(i, 0) {
            Some(c) => c,
            None => break,
        };
        let is_dot = c0 == b'.';
        if is_dot {
            n_dots += 1;
            if n_dots > 1 {
                break;
            }
        }
        // scientific notation: e/E then optional sign then digits — only
        // consume it when the exponent is actually valid
        if c0 == b'e' || c0 == b'E' {
            let digit_pos = if matches!(ch(i, 1), Some(b'+') | Some(b'-')) {
                ch(i, 2)
            } else {
                ch(i, 1)
            };
            if digit_pos.is_some_and(|c| c.is_ascii_digit()) {
                i += 1; // the e
                if matches!(ch(i, 0), Some(b'+') | Some(b'-')) {
                    i += 1; // the sign
                }
                while ch(i, 0).is_some_and(|c| c.is_ascii_digit()) {
                    i += 1;
                }
                // a clean exponent ending the run is the literal; trailing
                // alphanumerics make it an identifier instead
                if ch(i, 0).is_none() || !is_alnum(ch(i, 0).unwrap()) {
                    return NumScan::Numeric(i - i0);
                }
                break;
            }
            // 'e' without an exponent: treat as identifier (never consumed)
            break;
        }
        let is_last = ch(i + 1, 0).is_none();
        // a digit as the last character of the text: valid literal
        if !is_dot && is_last && c0.is_ascii_digit() {
            return NumScan::Numeric(i + 1 - i0);
        }
        // a non-alphanumeric, non-dot character ahead: the literal ends
        if !is_dot && !is_alnum(c0) {
            return NumScan::Numeric(i - i0);
        }
        // letters after digits: identifier
        if !is_dot && !c0.is_ascii_digit() {
            break;
        }
        i += 1;
    }
    if n_dots > 0 {
        // invalid number: gather all alphanumerics and dots of the run
        while ch(i, 0).is_some_and(|c| c == b'.' || is_alnum(c)) {
            i += 1;
        }
        NumScan::Invalid(i - i0)
    } else {
        // identifier
        while ch(i, 0).is_some_and(is_alnum) {
            i += 1;
        }
        NumScan::IdentToken(i - i0)
    }
}

/// Slice-scanning entry: no sentinel (the slice IS the whole text).
fn numeric_lit_or_ident(b: &[u8], i0: usize) -> NumScan {
    numeric_lit_or_ident_sentinel(b, i0, None)
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
                end: None,
                message: "unterminated record row".to_string(),
            })?
            + 1;
        let v = input[1..end].to_string();
        *input = &input[end + 1..];
        return Ok((RecordValue::Expr(v), start..offset(input, base)));
    }
    // their numericLiteralOrIdentifier: digit-led runs are ONE token —
    // clean numbers keep the full text (`1e+2`, `2.`), digit-led
    // identifiers are invalid record values (the row drops before the
    // model ever sees them), dot-junk runs (`2.5x`) are gathered invalid
    // numbers, surfacing here as an EmptyNode-sized null
    if input.starts_with(|c: char| c.is_ascii_digit()) {
        match numeric_lit_or_ident(input.as_bytes(), 0) {
            NumScan::Numeric(n) => {
                let v = input[..n].to_string();
                *input = &input[n..];
                return Ok((RecordValue::Number(v), start..offset(input, base)));
            }
            NumScan::IdentToken(n) => {
                let name = input[..n].to_string();
                *input = &input[n..];
                return Ok((RecordValue::Ident(name), start..offset(input, base)));
            }
            NumScan::Invalid(n) => {
                *input = &input[n..];
                return Ok((RecordValue::Null, start..offset(input, base)));
            }
        }
    }
    // a leading-dot numeric (`.5`): their lexer emits it as an OP token
    // (rows with one never validate — seen as "Unexpected '.'"), the
    // signed form is kept for the model
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
fn record_row(input: &mut &str, base: usize) -> R<(Vec<RecordValue>, Vec<Span>)> {
    let mut row = Vec::new();
    let mut spans = Vec::new();
    loop {
        sp_comment(input);
        let start = offset(input, base);
        // their commaExpression: empty fields (a leading, consecutive or
        // trailing comma) are EmptyNode values
        if input.is_empty()
            || input.starts_with(['\n', '\r'])
            || peek_is(input, ",")
            || peek_is(input, "}")
        {
            row.push(RecordValue::Null);
            spans.push(start..start);
        } else {
            let (v, span) = record_value(input, base)?;
            row.push(v);
            spans.push(span);
        }
        sp_comment(input);
        if !eat(input, ",") {
            break;
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

/// Consume one lexer-shaped token (their `synchronize*` advances one
/// token at a time): a quoted string/ident, a word/number, or one
/// punctuation/operator char. Returns the consumed length.
fn consume_one_token(input: &mut &str) -> Option<usize> {
    let b = input.as_bytes();
    if b.is_empty() {
        return None;
    }
    let n = match b[0] {
        b'"' | b'\'' | b'`' => {
            let q = b[0];
            let mut i = 1;
            while i < b.len() && b[i] != q {
                i += 1;
            }
            (i + 1).min(b.len())
        }
        c if c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80 => b
            .iter()
            .take_while(|&&x| x.is_ascii_alphanumeric() || x == b'_' || x >= 0x80)
            .count(),
        _ => 1,
    };
    *input = &input[n..];
    Some(n)
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
    rec_start: usize,
    count_errors: &mut Vec<(usize, usize, usize, usize)>,
    binder_out: &mut Vec<(usize, usize, String)>,
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
    let mut column_spans = Vec::new();
    if eat(input, "(") {
        // their tupleExpression: one element, then a comma-terminated
        // loop; a missing comma errs once at the offending token and
        // synchronizeTuple skips silently to the next `,`/`)` — the
        // skipped tokens never become elements (probe: `(odd col)`
        // flags odd, errs at col; `(a b c)` errs at b only, tuple [a])
        loop {
            sp(input);
            if input.is_empty() || eat(input, ")") {
                break;
            }
            let start = offset(input, base);
            let name = ident(input, base)?;
            column_spans.push(start..offset(input, base));
            columns.push(name);
            sp(input);
            if eat(input, ",") {
                continue;
            }
            if input.is_empty() || eat(input, ")") {
                break;
            }
            errs.push(err_at(offset(input, base), "Expect a comma ','"));
            // their canHandle (parser.ts): the tuple recovers locally only
            // when its own handler context is the current one — findHandler
            // Context scans forward for a `,`/`)` (group context) BEFORE
            // any `}` (a block close means the error belongs to the block
            // handler and PROPAGATES, leaving the rest for the caller's
            // recovery). Skip until the `,`/`)` WITHOUT consuming — the
            // next iteration's comma consume takes the `,`, the outer
            // loop's closing check takes the `)`.
            let to_comma = input.find(',');
            let to_rparen = input.find(')');
            let block_close = input.find('}');
            let sync_local = match (to_comma, to_rparen, block_close) {
                (Some(c), Some(p), Some(b)) => c.min(p) < b,
                (Some(c), None, Some(b)) => c < b,
                (None, Some(p), Some(b)) => p < b,
                (None, None, _) => false,
                // no block close ahead: the tuple's own handler recovers
                _ => true,
            };
            if sync_local {
                loop {
                    sp(input);
                    if input.is_empty() || input.starts_with(')') || input.starts_with(',') {
                        break;
                    }
                    if consume_one_token(input).is_none() {
                        break;
                    }
                }
            }
        }
        sp(input);
    }
    let mut example = false;
    if peek_is(input, "[") {
        for (key, val) in settings_list(input, base, errs)? {
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
        span: rec_start..0,
        columns,
        column_spans,
        rows: Vec::new(),
        value_spans: Vec::new(),
        example,
    };
    'rows: loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated records block"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        // their validateDataRow (local_modules/records/validate.ts): the
        // row line is an application — juxtaposed values (args) make the
        // structure invalid; non-simple values err per element. Both drop
        // the row.
        let row_start = offset(input, base);
        let row_input: &str = *input;
        // their blockExpression token stream has NO newlines (they are
        // trivia): a line ending in a dropped invalid token loses its
        // trailing newline WITH the removed token (gatherTrivia attached
        // it to that token), so hasTrailingNewLines(previous) is false and
        // the row's application CONTINUES onto the next line. Chain the
        // line slices into one token stream while the tail stays dropped.
        let mut toks: Vec<RTok> = Vec::new();
        let mut chained = 0usize;
        loop {
            let line_end = input.find('\n').unwrap_or(input.len());
            let line = &input[..line_end];
            let line_base = offset(input, base);
            let sentinel = if line_end < input.len() {
                Some(b'\n')
            } else {
                None
            };
            let (mut line_toks, mut dropped) = ref_tokens(line, line_base, sentinel, errs);
            // an invalid NUMBER gathered out at the tail takes the newline
            // with it the same way — their gatherInvalid removes every
            // invalid token, trivia attached
            if !dropped {
                dropped = line_toks
                    .iter()
                    .rev()
                    .find(|t| t.kind != RTKind::Eof)
                    .map(|t| t.invalid)
                    .unwrap_or(false);
            }
            let mut lex_errs: Vec<ParseError> = Vec::new();
            line_toks = gather_invalid_num(line_toks, &mut lex_errs);
            errs.append(&mut lex_errs);
            // their blockExpression: a line whose only tokens are trivia
            // and the closing brace (a gathered junk token leaves NOTHING)
            // never becomes a field — no empty application, no operand
            // errors. First line only: a chained brace line APPENDS, the
            // application's stop-kind sees the brace.
            if chained == 0
                && !line_toks
                    .iter()
                    .any(|t| t.kind != RTKind::Eof && t.kind != RTKind::RBrace)
            {
                match input.find(['\n', '}']) {
                    Some(k) if input.as_bytes()[k] == b'}' => {
                        *input = &input[k..];
                    }
                    Some(k) => *input = &input[k + 1..],
                    None => *input = &input[input.len()..],
                }
                continue 'rows;
            }
            line_toks.pop(); // this line's Eof — one stream, one Eof
            toks.extend(line_toks);
            chained += 1;
            // advance past the line — a same-line `}` stays for the block
            // loop and ends the chain (the application stops at RBrace)
            match input.find(['\n', '}']) {
                Some(k) if input.as_bytes()[k] == b'}' => {
                    *input = &input[k..];
                    break;
                }
                Some(k) => *input = &input[k + 1..],
                None => *input = &input[input.len()..],
            }
            if !dropped || input.is_empty() {
                break;
            }
        }
        toks.push(RTok {
            kind: RTKind::Eof,
            value: String::new(),
            start: offset(input, base),
            end: offset(input, base),
            space_after: false,
            invalid: false,
        });
        let chained_end: &str = *input;
        let mut rp = RParser {
            toks,
            pos: 0,
            errs,
            poisoned: None,
        };
        let tree = rp.application();
        // their leftExpression_bp THROWS on an invalid operand: the
        // PartialParsingError propagates to the records ELEMENT, which
        // drops entirely (rows after the damage included). Check BEFORE
        // any binder pushes; skip this block's remaining lines to its
        // closing brace — left for the block loop — and abort the block.
        if let Some((pos, end, msg)) = rp.poisoned.clone() {
            loop {
                let line_end = input.find('\n').unwrap_or(input.len());
                if input[..line_end].trim_start().starts_with('}') {
                    break;
                }
                if line_end >= input.len() {
                    *input = &input[input.len()..];
                    break;
                }
                *input = &input[line_end + 1..];
            }
            return Err(Fail { offset: pos, end: end, message: msg });
        }
        // their error node = the ROW: SyntaxNode.end = the last child's
        // endPos (types/nodes.ts) — the row's last token's end, no
        // trailing trivia (probe: "Expected 2 values but got 3" ends at
        // endColumn 21 on a 20-char row line)
        let row_end = rp.prev().end;
        let mut row_ok = true;
        if let RNode::App { callee, args, .. } = &tree {
            if !args.is_empty() {
                binder_out.push((
                    callee.start(),
                    row_end,
                    "Invalid record row structure".to_string(),
                ));
                row_ok = false;
            } else {
                let elems: Vec<&RNode> = match &**callee {
                    RNode::Comma { elems, .. } => elems.iter().collect(),
                    other => vec![other],
                };
                for e in elems {
                    if !is_valid_record_value(e) {
                        binder_out.push((
                            e.start(),
                            row_end,
                            "Records can only contain simple values (literals, null, true, \
                             false, or enum references). Complex expressions are not allowed."
                                .to_string(),
                        ));
                        row_ok = false;
                    }
                }
            }
        }
        if !row_ok {
            if chained >= 2 {
                // the chain already consumed the swallowed lines; resume
                // after them
                *input = chained_end;
            } else {
                // drop the damaged row: skip to the line end, leaving a
                // same-line `}` for the block loop
                match input.find(['\n', '}']) {
                    Some(k) if input.as_bytes()[k] == b'}' => {
                        *input = &input[k..];
                    }
                    Some(k) => *input = &input[k + 1..],
                    None => *input = &input[input.len()..],
                }
            }
            continue;
        }
        if chained >= 2 {
            // their extractDataFromRow reads the AST — no text re-scan;
            // the swallow always co-occurs with a lexer error, so their
            // interpretation is gated and the merged row's values never
            // surface. Drop it quietly: no re-scan errors, no count rule.
            *input = chained_end;
            continue;
        }
        // the chain consumed the row's own line; record_row re-scans the
        // values from the text
        *input = row_input;
        match record_row(input, base) {
            Ok((row, spans)) => {
                // their extractDataFromRow count rule is INTERPRET-layer:
                // the row is dropped from the model and the error only
                // exists while the file is otherwise clean (gated in
                // validate.rs)
                let expected = if rec.columns.is_empty() {
                    None
                } else {
                    Some(rec.columns.len())
                };
                if let Some(n) = expected {
                    if row.len() != n {
                        count_errors.push((row_start, row_end, n, row.len()));
                        continue;
                    }
                }
                rec.rows.push(row);
                rec.value_spans.push(spans);
            }
            Err(e) => {
                errs.push(err_at(e.offset, &e.message));
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
    rec.span = rec.span.start..offset(input, base);
    Ok((target, rec))
}

// --- top-level constructs ------------------------------------------------

enum Item {
    Prj(Project),
    Tbl(Table),
    Ref(Ref),
    Enm(Enum),
    Grp(TableGroup),
    Skip,
    Note(StickyNote),
}

fn top_item(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    doc_binder_errors: &mut Vec<(usize, usize, String)>,
    binder_chains: &mut Vec<super::ast::BinderChain>,
    record_count_errors: &mut Vec<(usize, usize, usize, usize)>,
) -> R<Item> {
    if peek_kw(input, "TableGroup") {
        table_group(input, base, errs, doc_binder_errors, binder_chains).map(Item::Grp)
    } else if peek_kw(input, "Table") {
        table(input, base, errs, record_count_errors, doc_binder_errors).map(Item::Tbl)
    } else if peek_kw(input, "Project") {
        project(input, base, errs).map(Item::Prj)
    } else if peek_kw(input, "Ref") {
        ref_stmt(input, base, errs, doc_binder_errors, binder_chains).map(|r| r.map_or(Item::Skip, Item::Ref))
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
            errs.push(err_at(e.offset, &e.message));
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

fn table(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    count_errors: &mut Vec<(usize, usize, usize, usize)>,
    binder_out: &mut Vec<(usize, usize, String)>,
) -> R<Table> {
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
        check_entries: Vec::new(),
        records: None,
        custom: Vec::new(),
        custom_spans: Vec::new(),
        note_span: None,
    };
    if peek_kw(input, "as") {
        let _ = eat(input, "as");
        ws(input);
        t.alias = Some(ident(input, base)?);
        ws(input);
    }
    if peek_is(input, "[") {
        let (kvs, occs) = settings_list_occs(input, base, errs)?;
        for (key, val) in kvs {
            if let Some(SettingVal::Str(v)) = val {
                match key.as_str() {
                    "note" => t.note = Some(v),
                    "headercolor" => t.header_color = Some(v),
                    // any other key:value is a dbdiagram custom property
                    _ => {
                        if let Some(o) = occs.iter().find(|o| o.key == key) {
                            t.custom_spans.push((key.clone(), o.offset..o.end));
                        }
                        t.custom.push((key, v));
                    }
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
            // their expect('}') at the EOF token
            errs.push(err_at(offset(input, base), "Expect a closing brace '}'"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        if peek_kw_then(input, "Note", ':') {
            let note_start = offset(input, base);
            let _ = eat(input, "Note");
            ws(input);
            let _ = eat(input, ":");
            ws(input);
            match note_value(input, base) {
                Ok(n) => {
                    t.note = Some(n);
                    t.note_span = Some(note_start..offset(input, base));
                }
                Err(e) => {
                    errs.push(err_at(e.offset, &e.message));
                    skip_line(input);
                }
            }
            continue;
        }
        if peek_kw(input, "Records") {
            // records [target][(cols)] [example] { rows } — sample rows
            // shown in the record view
            let rec_start = offset(input, base);
            let _ = eat(input, "Records");
            match records_block(input, base, errs, rec_start, count_errors, binder_out) {
                Ok((_, rec)) => t.records = Some(rec),
                Err(e) => {
                    // an empty message = the error already pushed (the
                    // poisoned abort's marker)
                    if !e.message.is_empty() {
                        errs.push(err_at(e.offset, &e.message));
                    }
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
            let raw_start = input[..end].len() - input[..end].trim_start().len();
            let entries = parse_check_entries(&raw, base + raw_start, errs);
            *input = &input[end..];
            let _ = eat(input, "}");
            t.checks = Some(raw);
            t.check_entries = entries;
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
                match index(input, base, errs) {
                    Ok(ix) => t.indexes.push(ix),
                    Err(e) => {
                        errs.push(err_at(e.offset, &e.message));
                        skip_line(input);
                    }
                }
            }
            continue;
        }
        match column(input, base, errs) {
            Ok(c) => t.columns.push(c),
            Err(e) => {
                errs.push(err_at(e.offset, &e.message));
                skip_line(input);
            }
        }
    }
    t.span = start..offset(input, base);
    Ok(t)
}

fn ref_stmt(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    doc_binder_errors: &mut Vec<(usize, usize, String)>,
    binder_chains: &mut Vec<super::ast::BinderChain>,
) -> R<Option<Ref>> {
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
        let (on_delete, on_update, color) =
            ref_settings(input, base, errs, doc_binder_errors);
        return Ok(Some(Ref {
            name,
            clean_binary: true,
            binder_errors: Vec::new(),
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
        }));
    }
    if !eat(input, ":") {
        return fail(offset(input, base), "expected ':' after Ref");
    }
    ws(input);
    let body_input = *input;
    let body_base = offset(input, base);
    // the strict attempt runs on a scratch copy so the fallback sees the
    // original body
    let mut scratch = body_input;
    // the scratch shares the source pointer, so sub-parsers keep taking
    // the source `base` — their offsets are pointer differences
    // their parse NEVER fails on these shapes — the body always parses
    // into an expression (their Pratt parser breaks at non-op
    // juxtapositions, wrapping the rest as application args) — so a
    // strict-parse failure here means the shape classifier runs below
    let strict: R<(super::ast::Endpoint, super::ast::Endpoint, Cardinality, bool, bool, usize)> = (|| {
        let from = endpoint(&mut scratch, base)?;
        ws(&mut scratch);
        // the op binds to the next expression immediately (their Pratt
        // grammar) — no args can sit between the op and the right endpoint
        let op_offset = offset(&mut scratch, base);
        let (card, l_opt, r_opt) = cardinality_full(&mut scratch, base)?;
        ws(&mut scratch);
        let to = endpoint(&mut scratch, base)?;
        Ok((from, to, card, l_opt, r_opt, op_offset))
    })();
    match strict {
        Ok((from, to, card, l_opt, r_opt, op_offset)) => {
            *input = scratch;
            let (on_delete, on_update, color) =
                ref_settings(input, base, errs, doc_binder_errors);
            // trailing args after the last endpoint: their application
            // parse rebinds the whole line — `junk` words become args
            // (single-binary each) and an op-leading tail REBINDS the
            // callee (`a.b > c.d - e.f` → infix(`-`) callee). The tree's
            // diagnostics join with the callee shape errors + ARG chains
            // only (the model ref checks the callee's endpoints).
            sp(input);
            let mut callee_clean = true;
            if !input.is_empty()
                && !input.starts_with('\n')
                && !input.starts_with('\r')
                && !input.starts_with('}')
            {
                let mut shape: Vec<(usize, usize, String)> = Vec::new();
                let mut arg_chains = Vec::new();
                let mut body_scratch = body_input;
                ref_body_pipeline_mode(
                    &mut body_scratch,
                    body_base,
                    errs,
                    &mut shape,
                    &mut arg_chains,
                    true,
                    &mut callee_clean,
                );
                doc_binder_errors.extend(shape);
                binder_chains.extend(arg_chains);
                skip_line(input);
            }
            Ok(Some(Ref {
                name,
                clean_binary: callee_clean,
                binder_errors: Vec::new(),
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
            }))
        }
        Err(_) => {
            // their ref-field shape rules (ref/validate.ts validateFields
            // + the field-args walk): classify the broken body, emit
            // their error set, recover at the line end — binder-class
            // errors, routed to the parse output's binder channel
            let mut shape: Vec<(usize, usize, String)> = Vec::new();
            let mut body_scratch = body_input;
            ref_body_pipeline(&mut body_scratch, body_base, errs, &mut shape, binder_chains);
            doc_binder_errors.extend(shape);
            skip_line(input);
            Ok(None)
        }
    }
}

// ==== their ref-body pipeline: for bodies the strict parse rejects,
// their parser NEVER fails — the body parses into ONE application tree
// (parser.ts expression()/expression_bp()), and the diagnostics come
// from local_modules/ref/validate.ts validateFields + global_modules/
// ref/bind.ts bindFields + nodeRefereeOfRefEndpoint. Ported node for
// node; every rule below cites its TS source. ====

#[derive(Debug, Clone, PartialEq)]
enum RNode {
    /// PrimaryExpression(VariableNode) — identifier or quoted variable
    Var { name: String, start: usize },
    /// PrimaryExpression(LiteralNode) — number/string literal (color
    /// literals flagged: their isValidRecordValue rejects them)
    Lit { start: usize, color: bool },
    Wildcard { start: usize },
    /// backtick function expression
    Func { start: usize },
    /// any infix op (`.` included — the member access IS an infix)
    Infix { op: String, left: Box<RNode>, right: Box<RNode>, start: usize },
    Prefix { op: String, expr: Box<RNode>, start: usize },
    /// (a, b, ...) — TupleExpressionNode (2+ elements)
    Tuple { elems: Vec<RNode>, start: usize },
    /// (a) — GroupExpressionNode (a TupleExpressionNode with 1 element)
    Group { inner: Box<RNode>, start: usize },
    /// a(x, y) — CallExpressionNode (postfix `(`)
    Call { callee: Box<RNode>, args: Vec<RNode>, start: usize },
    /// FunctionApplicationNode — the juxtaposition wrapper
    App { callee: Box<RNode>, args: Vec<RNode>, start: usize },
    /// `[...]` consumed as an expression — ListExpressionNode
    Array { start: usize, end: usize },
    Block { start: usize },
    /// CommaExpressionNode (CSV-like, empty fields = Empty)
    Comma { elems: Vec<RNode>, start: usize },
    /// EmptyNode — invalid operand / leading comma field
    Empty { start: usize },
}

impl RNode {
    fn start(&self) -> usize {
        match self {
            RNode::Var { start, .. }
            | RNode::Lit { start, .. }
            | RNode::Wildcard { start }
            | RNode::Func { start }
            | RNode::Infix { start, .. }
            | RNode::Prefix { start, .. }
            | RNode::Tuple { start, .. }
            | RNode::Group { start, .. }
            | RNode::Call { start, .. }
            | RNode::App { start, .. }
            | RNode::Array { start, .. }
            | RNode::Block { start }
            | RNode::Comma { start, .. }
            | RNode::Empty { start } => *start,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RTKind {
    Ident,
    Quoted,
    Num,
    Str,
    Color,
    Func,
    Op,
    Comma,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    Semicolon,
    Wildcard,
    Unknown,
    Eof,
}

#[derive(Debug, Clone)]
struct RTok {
    kind: RTKind,
    value: String,
    start: usize,
    end: usize,
    /// hasTrailingSpaces(previous) — a space/tab directly after the token
    space_after: bool,
    /// their isInvalid token (an invalid numeric run): never reaches the
    /// parser — gatherInvalid drops it, each one emitting 'Invalid number'
    invalid: bool,
}

/// Their lexer over a single ref-body line: punctuation singles, the
/// operator munching table (lexer.ts operator()), unicode identifiers
/// (chars.ts: \p{L}|_|\p{M}), quoted variables, function expressions,
/// numeric literals with e-notation. `sentinel` is the byte following the
/// slice in the real source (None at end-of-file): numeric runs at the
/// slice edge evaluate against it (`2.` at a line end is a valid literal),
/// but it is never tokenized.
/// Returns the tokens plus `tail_dropped`: the scan reached the slice
/// end and dropped an invalid partial there (their gatherInvalid) — the
/// line's trailing trivia (the newline) rode along with the removed
/// token, so in their parser the enclosing application continues onto
/// the next line (hasTrailingNewLines(previous) = false).
fn ref_tokens(
    body: &str,
    base: usize,
    sentinel: Option<u8>,
    errs: &mut Vec<ParseError>,
) -> (Vec<RTok>, bool) {
    let b = body.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0usize;
    let mut tail_dropped = false;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let kind;
        let mut value = String::new();
        let mut end = i + 1;
        let mut invalid = false;
        let mut dropped = false;
        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'\n' => break,
            b',' => kind = RTKind::Comma,
            b'(' => kind = RTKind::LParen,
            b')' => kind = RTKind::RParen,
            b'[' => kind = RTKind::LBracket,
            b']' => kind = RTKind::RBracket,
            b'{' => kind = RTKind::LBrace,
            b'}' => kind = RTKind::RBrace,
            b':' => kind = RTKind::Colon,
            b';' => {
                kind = RTKind::Semicolon;
                value = ";".to_string();
            },
            b'\'' => {
                // singleLineStringLiteral (consumeUntil, allowNewline=false,
                // allowEof=false): a newline or EOF before the closing quote
                // is their UNEXPECTED_NEWLINE / UNEXPECTED_EOF, anchored on
                // the PARTIAL TOKEN [start..scan-stop). The token itself is
                // invalid — their gatherInvalid removes it from the stream,
                // taking the line's trailing trivia (the newline) with it.
                // A brace sentinel means the scan would continue past our
                // slice — that cut is our artifact, not the truth: no
                // error, and the partial lands as before.
                let mut k = i + 1;
                while k < b.len() && b[k] != b'\'' && b[k] != b'\n' {
                    if b[k] == b'\\' {
                        k += 1; // the escaped char rides along
                    }
                    k += 1;
                }
                // the slice excludes the trailing newline; the sentinel
                // says what follows it (their consumeUntil scans the whole
                // file, so a scan reaching the slice end with a newline
                // sentinel = their UNEXPECTED_NEWLINE, at true EOF =
                // their EOF error)
                let at_end = k >= b.len();
                if at_end && sentinel == Some(b'\n') {
                    errs.push(ParseError {
                        offset: base + start,
                        end: Some(base + k),
                        message: "Invalid newline encountered while parsing".to_string(),
                    });
                    dropped = true;
                } else if at_end && sentinel.is_none() {
                    // allowEof=false: their consumeUntil errs at EOF too
                    errs.push(ParseError {
                        offset: base + start,
                        end: Some(base + k),
                        message: "EOF reached while parsing".to_string(),
                    });
                    dropped = true;
                }
                if dropped {
                    kind = RTKind::Str;
                    end = k;
                } else {
                    if k < b.len() && b[k] == b'\'' {
                        k += 1;
                    }
                    kind = RTKind::Str;
                    value = body[start..k.min(body.len())].to_string();
                    end = k;
                }
            }
            b'"' => {
                // quotedVariable: allowNewline=false — the same rule as the
                // single-quoted string above: the error anchors the partial
                // token and the token is dropped (their gatherInvalid)
                let mut k = i + 1;
                while k < b.len() && b[k] != b'"' && b[k] != b'\n' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
                let at_end = k >= b.len();
                if at_end && sentinel == Some(b'\n') {
                    errs.push(ParseError {
                        offset: base + start,
                        end: Some(base + k),
                        message: "Invalid newline encountered while parsing".to_string(),
                    });
                    dropped = true;
                } else if at_end && sentinel.is_none() {
                    errs.push(ParseError {
                        offset: base + start,
                        end: Some(base + k),
                        message: "EOF reached while parsing".to_string(),
                    });
                    dropped = true;
                }
                if dropped {
                    kind = RTKind::Quoted;
                    end = k;
                } else {
                    value = body[start + 1..k.min(body.len())].to_string();
                    if k < b.len() && b[k] == b'"' {
                        k += 1;
                    }
                    kind = RTKind::Quoted;
                    end = k;
                }
            }
            b'`' => {
                let mut k = i + 1;
                while k < b.len() && b[k] != b'`' && b[k] != b'\n' {
                    if b[k] == b'\\' {
                        k += 1;
                    }
                    k += 1;
                }
                if k < b.len() && b[k] == b'`' {
                    k += 1;
                }
                kind = RTKind::Func;
                value = body[start..k.min(body.len())].to_string();
                end = k;
            }
            b'#' => {
                let mut k = i + 1;
                while k < b.len() && b[k].is_ascii_alphanumeric() {
                    k += 1;
                }
                kind = RTKind::Color;
                value = body[start..k].to_string();
                end = k;
            }
            b'*' => kind = RTKind::Wildcard,
            b'/' if body[i..].starts_with("//") => {
                // a comment: trivia through the line end
                i = body.len();
                continue;
            }
            b'/' if body[i..].starts_with("/*") => {
                // a block comment: trivia through the closing */
                match body[i + 2..].find("*/") {
                    Some(k) => i = i + 2 + k + 2,
                    None => i = body.len(),
                }
                continue;
            }
            _ if is_op_char(c) => {
                // their operator() munching table
                let mut k = i + 1;
                match c {
                    b'<' => {
                        if b.get(k) == Some(&b'=') {
                            k += 1; // <=
                        } else if b.get(k) == Some(&b'>') {
                            k += 1; // <>
                            if b.get(k) == Some(&b'?') {
                                k += 1; // <>?
                            }
                        } else if b.get(k) == Some(&b'?') {
                            k += 1; // <?
                        } else if b.get(k) == Some(&b'-') {
                            k += 1; // <-
                        }
                    }
                    b'>' => {
                        if b.get(k) == Some(&b'=') {
                            k += 1; // >=
                        } else if b.get(k) == Some(&b'?') {
                            k += 1; // >?
                        }
                    }
                    b'-' => {
                        if b.get(k) == Some(&b'>') {
                            k += 1; // ->
                        } else if b.get(k) == Some(&b'?') {
                            k += 1; // -?
                        }
                    }
                    b'?' => {
                        if b.get(k) == Some(&b'-') {
                            k += 1; // ?-
                            if b.get(k) == Some(&b'?') {
                                k += 1; // ?-?
                            }
                        } else if b.get(k) == Some(&b'<') {
                            k += 1; // ?<
                            if b.get(k) == Some(&b'>') {
                                k += 1; // ?<>
                                if b.get(k) == Some(&b'?') {
                                    k += 1; // ?<>?
                                }
                            } else if b.get(k) == Some(&b'?') {
                                k += 1; // ?<?
                            }
                        } else if b.get(k) == Some(&b'>') {
                            k += 1; // ?>
                            if b.get(k) == Some(&b'?') {
                                k += 1; // ?>?
                            }
                        }
                    }
                    b'=' => {
                        if b.get(k) == Some(&b'=') {
                            k += 1; // ==
                        }
                    }
                    b'!' => {
                        if b.get(k) == Some(&b'=') {
                            k += 1; // !=
                        }
                    }
                    _ => {}
                }
                kind = RTKind::Op;
                value = body[start..k].to_string();
                end = k;
            }
            _ if c.is_ascii_digit() => {
                // their numericLiteralOrIdentifier (the helper above):
                // digit-led runs continue into alphanumerics as ONE
                // identifier token; a dot-run like `12.3abc` is an invalid
                // number token, gathered out with an 'Invalid number' error
                match numeric_lit_or_ident_sentinel(b, start, sentinel) {
                    NumScan::Numeric(n) => {
                        kind = RTKind::Num;
                        value = body[start..start + n].to_string();
                        end = start + n;
                    }
                    NumScan::IdentToken(n) => {
                        kind = RTKind::Ident;
                        value = body[start..start + n].to_string();
                        end = start + n;
                    }
                    NumScan::Invalid(n) => {
                        kind = RTKind::Num;
                        value = body[start..start + n].to_string();
                        end = start + n;
                        invalid = true;
                    }
                }
            }
            _ if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 => {
                // identifier(): \p{L}|_|\p{M} start, then alphanumeric run
                let mut k = i;
                while k < b.len()
                    && (b[k].is_ascii_alphanumeric() || b[k] == b'_' || b[k] >= 0x80)
                {
                    k += 1;
                }
                kind = RTKind::Ident;
                value = body[start..k].to_string();
                end = k;
            }
            _ => kind = RTKind::Unknown,
        }
        let kind = kind;
        if dropped {
            // their gatherInvalid: the invalid partial never reaches the
            // parser; the scan resumes after it. dropped only happens when
            // the scan ran to the slice end (the newline/EOF failure), so
            // the line's tail is what dropped.
            tail_dropped = true;
            i = end;
            continue;
        }
        let space_after = b.get(end) == Some(&b' ') || b.get(end) == Some(&b'\t');
        toks.push(RTok {
            kind,
            value,
            start: base + start,
            end: base + end,
            space_after,
            invalid,
        });
        i = end;
    }
    toks.push(RTok {
        kind: RTKind::Eof,
        value: String::new(),
        start: base + b.len(),
        end: base + b.len(),
        space_after: false,
        invalid: false,
    });
    (toks, tail_dropped)
}

fn is_op_char(c: u8) -> bool {
    matches!(
        c,
        b'+' | b'-' | b'/' | b'%' | b'<' | b'>' | b'=' | b'!' | b'?' | b'.' | b'&' | b'|' | b'~'
    )
}

/// infixBpMap (parser.ts): (left, right) binding powers; None = not an
/// infix op.
fn infix_bp(op: &str) -> Option<(u8, u8)> {
    let lb = |l: u8| Some((l, l + 1));
    match op {
        "." => lb(16),       // Prec.Member
        "/" | "%" => lb(11), // Prec.Multiplicity
        "+" => lb(9),        // Prec.Additive
        "=" => lb(2),        // Prec.Assign
        "==" | "!=" => lb(4), // Prec.Equality
        // the one-to-one `-` family binds like `-` (Additive)
        "-" | "-?" | "?-" | "?-?" => lb(9),
        // every other relationship op binds like a comparison
        "<" | "<=" | ">" | ">=" | "<>" | "?>" | ">?" | "?<" | "<?" | "?>?" | "?<?" | "?<>"
        | "<>?" | "?<>?" | "->" | "<-" => lb(7), // Prec.Comparison
        _ => None,
    }
}

/// prefixBpMap: the right binding power of prefix ops
fn prefix_bp(op: &str) -> Option<u8> {
    match op {
        "+" | "-" | "<" | ">" | "<>" | "-?" | "?-" | "?-?" | "?>" | ">?" | "?<" | "<?" | "?>?"
        | "?<?" | "?<>" | "<>?" | "?<>?" | "->" | "<-" | "!" | "~" => Some(15), // Prec.Prefix
        _ => None,
    }
}

struct RParser<'a> {
    toks: Vec<RTok>,
    pos: usize,
    errs: &'a mut Vec<ParseError>,
    /// An invalid operand (their INVALID_OPERAND): their parser THROWS
    /// here, aborting the enclosing element — the flag defers the abort
    /// to the caller (the whole records block drops).
    poisoned: Option<(usize, Option<usize>, String)>,
}

/// Their gatherInvalid (lexer.ts:247): invalid tokens never reach the
/// parser; each emits its span-anchored 'Invalid number' error on the way
/// out. The remaining stream is what their parser sees.
fn gather_invalid_num(toks: Vec<RTok>, errs: &mut Vec<ParseError>) -> Vec<RTok> {
    for t in toks.iter().filter(|t| t.invalid) {
        errs.push(err_at(t.start, "Invalid number"));
    }
    toks.into_iter().filter(|t| !t.invalid).collect()
}

impl RParser<'_> {
    fn peek(&self) -> &RTok {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }
    fn prev(&self) -> &RTok {
        &self.toks[self.pos.saturating_sub(1).min(self.toks.len() - 1)]
    }
    fn advance(&mut self) -> RTok {
        let t = self.peek().clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn check(&self, k: &RTKind) -> bool {
        self.peek().kind == *k
    }
    fn at_end(&self) -> bool {
        self.check(&RTKind::Eof)
    }
    fn consume(&mut self, msg: &str, k: RTKind) -> Result<RTok, ()> {
        if self.check(&k) {
            Ok(self.advance())
        } else {
            self.errs.push(err_at(self.peek().start, msg));
            Err(())
        }
    }

    /// shouldStopFunctionApplication (parser.ts)
    fn should_stop(&self) -> bool {
        self.at_end()
            || matches!(
                self.peek().kind,
                RTKind::RBrace | RTKind::RBracket | RTKind::RParen | RTKind::Comma | RTKind::Colon
            )
    }

    /// shouldStopCommaExpression (parser.ts)
    fn should_stop_comma(&self) -> bool {
        self.at_end()
            || matches!(
                self.peek().kind,
                RTKind::RBrace
                    | RTKind::LBrace
                    | RTKind::RBracket
                    | RTKind::LBracket
                    | RTKind::RParen
                    | RTKind::LParen
                    | RTKind::Colon
            )
    }

    /// expression() — the application loop: callee then juxtaposed args.
    /// Their parse ALWAYS returns a FunctionApplicationNode (even for a
    /// lone expression — callee only, empty args); an arg following
    /// without a space logs their MISSING_SPACES
    fn application(&mut self) -> RNode {
        let start = self.peek().start;
        let callee = self.comma_expression();
        let mut args = Vec::new();
        let mut prev_start = callee.start();
        let mut prev_end = self.prev().end;
        while !self.should_stop() {
            // their parser THREW on the invalid operand: the unwind
            // happens before any further diagnostics
            if self.poisoned.is_some() {
                break;
            }
            if !self.prev().space_after {
                self.errs.push(ParseError {
                    offset: prev_start,
                    end: Some(prev_end),
                    message: "Expect a following space".to_string(),
                });
            }
            let arg = self.comma_expression();
            prev_start = arg.start();
            prev_end = self.prev().end;
            args.push(arg);
        }
        RNode::App {
            callee: Box::new(callee),
            args,
            start,
        }
    }

    /// commaExpression (parser.ts)
    fn comma_expression(&mut self) -> RNode {
        let start = self.peek().start;
        let first = if self.check(&RTKind::Comma) {
            RNode::Empty { start }
        } else {
            self.normal_expression()
        };
        if !self.check(&RTKind::Comma) {
            return first;
        }
        let mut elems = vec![first];
        loop {
            if self.poisoned.is_some() {
                break;
            }
            self.advance(); // the comma
            if self.should_stop_comma() {
                elems.push(RNode::Empty {
                    start: self.prev().end,
                });
                break;
            }
            if self.check(&RTKind::Comma) {
                elems.push(RNode::Empty {
                    start: self.prev().end,
                });
                continue;
            }
            elems.push(self.normal_expression());
            if self.should_stop_comma() || !self.check(&RTKind::Comma) {
                break;
            }
        }
        RNode::Comma { elems, start }
    }

    fn normal_expression(&mut self) -> RNode {
        self.expression_bp(0)
    }

    /// expression_bp — Pratt (parser.ts)
    fn expression_bp(&mut self, mbp: u8) -> RNode {
        let mut left = self.left_expression_bp();
        loop {
            if self.at_end() {
                break;
            }
            let t = self.peek().clone();
            if t.kind == RTKind::LParen {
                // call expression; `(` at the start of a line breaks, but
                // the body slice is single-line so that cannot occur here
                let tuple = self.tuple_expression();
                let start = left.start();
                let args = match tuple {
                    RNode::Tuple { elems, .. } => elems,
                    RNode::Group { inner, .. } => vec![*inner],
                    other => vec![other],
                };
                left = RNode::Call {
                    callee: Box::new(left),
                    args,
                    start,
                };
            } else if t.kind == RTKind::LBracket {
                // a space before `[` starts a settings list — break out
                if t.space_after {
                    break;
                }
                let list = self.list_expression();
                let start = left.start();
                let end = match list {
                    RNode::Array { end, .. } => end,
                    _ => start,
                };
                left = RNode::Array { start, end };
            } else if !matches!(t.kind, RTKind::Op | RTKind::Unknown) {
                break;
            } else {
                let Some(p) = infix_bp(&t.value) else {
                    break;
                };
                if p.0 <= mbp {
                    break;
                }
                self.advance();
                let right = if t.value == "." {
                    self.extract_operand()
                } else {
                    self.expression_bp(p.1)
                };
                let start = left.start();
                left = RNode::Infix {
                    op: t.value.clone(),
                    left: Box::new(left),
                    right: Box::new(right),
                    start,
                };
            }
        }
        left
    }

    /// leftExpression_bp (parser.ts)
    fn left_expression_bp(&mut self) -> RNode {
        if matches!(self.peek().kind, RTKind::Op | RTKind::Unknown) {
            let op = self.peek().clone();
            let Some(right) = prefix_bp(&op.value) else {
                // UNKNOWN_PREFIX_OP: their logError + Empty partial; the
                // rethrow abandons the rest of the line
                self.errs.push(err_at(
                    op.start,
                    &format!("Unexpected '{}' in an expression", op.value),
                ));
                while !self.at_end() {
                    self.advance();
                }
                return RNode::Empty { start: op.start };
            };
            self.advance();
            let expr = self.expression_bp(right);
            let start = op.start;
            return RNode::Prefix {
                op: op.value.clone(),
                expr: Box::new(expr),
                start,
            };
        }
        self.extract_operand()
    }

    /// extractOperand (parser.ts)
    fn extract_operand(&mut self) -> RNode {
        let t = self.peek().clone();
        match t.kind {
            RTKind::Wildcard => {
                self.advance();
                RNode::Wildcard { start: t.start }
            }
            RTKind::Num | RTKind::Str | RTKind::Color => {
                self.advance();
                RNode::Lit {
                    start: t.start,
                    color: t.kind == RTKind::Color,
                }
            }
            RTKind::Quoted | RTKind::Ident => {
                self.advance();
                RNode::Var {
                    name: t.value.clone(),
                    start: t.start,
                }
            }
            RTKind::Func => {
                self.advance();
                RNode::Func { start: t.start }
            }
            RTKind::LBracket => {
                let list = self.list_expression();
                let end = match list {
                    RNode::Array { end, .. } => end,
                    _ => t.start,
                };
                RNode::Array { start: t.start, end }
            }
            RTKind::LBrace => {
                let _ = self.block_expression();
                RNode::Block { start: t.start }
            }
            RTKind::LParen => self.tuple_expression(),
            _ => {
                if self.at_end() {
                    self.errs.push(err_at(t.start, "Unexpected EOF"));
                } else {
                    // their INVALID_OPERAND: logError + EmptyNode, then
                    // leftExpression_bp THROWS — the error defers to the
                    // element-level abort via the poisoned flag
                    self.poisoned = Some((
                        t.start,
                        Some(t.end),
                        format!("Invalid start of operand \"{}\"", t.value),
                    ));
                    self.advance();
                }
                RNode::Empty { start: t.start }
            }
        }
    }

    /// tupleExpression (parser.ts) — with their synchronizeTuple recovery
    fn tuple_expression(&mut self) -> RNode {
        let start = self.peek().start;
        let _ = self.consume("Expect an opening parenthesis '('", RTKind::LParen);
        let mut elems: Vec<RNode> = Vec::new();
        if !self.at_end() && !self.check(&RTKind::RParen) {
            elems.push(self.normal_expression());
        }
        while !self.at_end() && !self.check(&RTKind::RParen) {
            if self.consume("Expect a comma ','", RTKind::Comma).is_ok() {
                // their loop pushes an element unconditionally — a `)`
                // right after the comma errors as an invalid operand
                elems.push(self.normal_expression());
            } else {
                // synchronizeTuple: skip to the next `,`/`)` unmarked
                while !self.at_end()
                    && !self.check(&RTKind::RParen)
                    && !self.check(&RTKind::Comma)
                {
                    self.advance();
                }
            }
        }
        let _ = self.consume("Expect a closing parenthesis ')'", RTKind::RParen);
        if elems.len() == 1 {
            let inner = elems.pop().unwrap();
            RNode::Group {
                inner: Box::new(inner),
                start,
            }
        } else {
            RNode::Tuple { elems, start }
        }
    }

    /// listExpression — attributes are not modeled; scan to the
    /// matching bracket
    fn list_expression(&mut self) -> RNode {
        let start = self.peek().start;
        let _ = self.consume("Expect an opening bracket '['", RTKind::LBracket);
        let mut depth = 1usize;
        while !self.at_end() && depth > 0 {
            match self.peek().kind {
                RTKind::LBracket => {
                    self.advance();
                    depth += 1;
                }
                RTKind::RBracket => {
                    self.advance();
                    depth -= 1;
                }
                _ => {
                    self.advance();
                }
            }
        }
        RNode::Array {
            start,
            end: self.prev().end,
        }
    }

    /// blockExpression — scan to the matching brace
    fn block_expression(&mut self) -> RNode {
        let start = self.peek().start;
        let _ = self.consume("Expect an opening brace '{'", RTKind::LBrace);
        let mut depth = 1usize;
        while !self.at_end() && depth > 0 {
            match self.peek().kind {
                RTKind::LBrace => {
                    self.advance();
                    depth += 1;
                }
                RTKind::RBrace => {
                    self.advance();
                    depth -= 1;
                }
                _ => {
                    self.advance();
                }
            }
        }
        RNode::Block { start }
    }
}

// ---- the binding scan (core/utils/expression.ts + global_modules/utils.ts)
// -----------------------------------------------------------------------

/// One bindee: the variables stack (schema..., table, column) plus the
/// tuple elements when the chain ends in `.(a, b)`.
#[derive(Default, Debug, Clone)]
struct Bindee {
    variables: Vec<RNode>,
    tuple: Vec<RNode>,
}

/// destructureMemberAccessExpression: flatten the `.` chain
fn destructure_member_access(n: &RNode, out: &mut Vec<RNode>) {
    if let RNode::Infix {
        op,
        left,
        right,
        ..
    } = n
    {
        if op == "." {
            destructure_member_access(left, out);
            out.push((**right).clone());
            return;
        }
    }
    out.push(n.clone());
}

fn is_var(n: &RNode) -> bool {
    matches!(n, RNode::Var { .. })
}

/// isTupleOfVariables: every element a variable
fn is_tuple_of_variables(elems: &[RNode]) -> bool {
    !elems.is_empty() && elems.iter().all(is_var)
}

/// destructureComplexVariableTuple (expression.ts)
fn destructure_cvt(n: &RNode) -> Option<Bindee> {
    let mut fragments = Vec::new();
    destructure_member_access(n, &mut fragments);
    if fragments.is_empty() {
        return None;
    }
    let mut tuple = Vec::new();
    if !fragments.last().map(is_var).unwrap_or(false) {
        let top = fragments.pop()?;
        match top {
            RNode::Tuple { elems, .. } if is_tuple_of_variables(&elems) => tuple = elems,
            RNode::Group { inner, .. } if is_var(&inner) => tuple = vec![*inner],
            _ => return None,
        }
    }
    if !fragments.iter().all(is_var) {
        return None;
    }
    Some(Bindee {
        variables: fragments,
        tuple,
    })
}

/// destructureComplexVariable (expression.ts): the `.` chain flattened,
/// every fragment a plain variable — else undefined
fn destructure_complex_variable(n: &RNode) -> Option<Vec<String>> {
    let mut fragments = Vec::new();
    destructure_member_access(n, &mut fragments);
    let mut names = Vec::new();
    for f in &fragments {
        match f {
            RNode::Var { name, .. } => names.push(name.clone()),
            _ => return None,
        }
    }
    Some(names)
}

/// scanNonListNodeForBinding (global_modules/utils.ts)
fn scan_binding(n: &RNode, out: &mut Vec<Bindee>) {
    match n {
        RNode::Var { .. } => out.push(Bindee {
            variables: vec![n.clone()],
            tuple: Vec::new(),
        }),
        RNode::Infix { left, right, .. } => {
            if let Some(b) = destructure_cvt(n) {
                out.push(b);
            } else {
                scan_binding(left, out);
                scan_binding(right, out);
            }
        }
        RNode::Prefix { expr, .. } => scan_binding(expr, out),
        RNode::Tuple { elems, .. } => {
            if let Some(b) = destructure_cvt(n) {
                out.push(b);
            } else {
                for e in elems {
                    scan_binding(e, out);
                }
            }
        }
        RNode::Group { inner, .. } => {
            if let Some(b) = destructure_cvt(n) {
                out.push(b);
            } else {
                scan_binding(inner, out);
            }
        }
        // CallExpression, ListExpression, CommaExpression, literals,
        // blocks, Empty: nothing binds
        _ => {}
    }
}

// ---- validateFields (local_modules/ref/validate.ts) -------------------

/// isRelationshipOp (types/relation.ts RELATIONSHIP_OPS)
fn is_relationship_op(op: &str) -> bool {
    matches!(
        op,
        ">" | "<" | "-" | "<>" | ">?" | "<?" | "?>" | "?<" | "-?" | "?-" | ">=" | "<="
            | "?>?" | "?<?" | "?<>" | "<>?" | "?<>?" | "?-?"
    )
}

/// isBinaryRelationship (utils/validate.ts)
fn is_binary_relationship(n: &RNode) -> bool {
    match n {
        RNode::Infix { op, left, right, .. } => {
            is_relationship_op(op)
                && destructure_cvt(left).is_some()
                && destructure_cvt(right).is_some()
        }
        _ => false,
    }
}

/// isEqualTupleOperands (utils/validate.ts) — a non-infix callee has no
/// operand sides (their value.leftExpression is undefined) → false
fn is_equal_tuple_operands(n: &RNode) -> bool {
    match n {
        RNode::Infix { left, right, .. } => {
            match (destructure_cvt(left), destructure_cvt(right)) {
                (Some(l), Some(r)) => l.tuple.len() == r.tuple.len(),
                _ => false,
            }
        }
        _ => false,
    }
}

/// validateFields (local_modules/ref/validate.ts) + nodeSettings' args
/// walk (local_modules/ref/index.ts): must-be-binary, per-side Invalid
/// column reference, Unequal, and one single-binary per non-settings arg
/// validateFields (local_modules/ref/validate.ts) split by source: the
/// callee checks (must-be-binary, per-side Invalid column reference,
/// Unequal) vs the nodeSettings args walk (one single-binary per
/// non-settings arg)
fn validate_fields_split(callee: &RNode, args: &[RNode]) -> (Vec<(usize, String)>, Vec<(usize, String)>) {
    let mut callee_errors = Vec::new();
    let mut arg_errors = Vec::new();
    if !is_binary_relationship(callee) {
        callee_errors.push((
            callee.start(),
            "A Ref field must be a binary relationship".to_string(),
        ));
    } else if let RNode::Infix { left, right, .. } = callee {
        // their per-side destructure counts
        for side in [left.as_ref(), right.as_ref()] {
            let frag = destructure_cvt(side).unwrap_or_default();
            if frag.variables.len() + frag.tuple.len().min(1) < 2 {
                callee_errors.push((side.start(), "Invalid column reference".to_string()));
            }
        }
    }
    if !is_equal_tuple_operands(callee) {
        callee_errors.push((
            callee.start(),
            "Unequal fields in ref endpoints".to_string(),
        ));
    }
    // nodeSettings: a trailing or leading `[...]` is the settings list;
    // every other arg errs once
    let mut rest: Vec<&RNode> = args.iter().collect();
    if matches!(rest.last(), Some(RNode::Array { .. })) {
        rest.pop();
    } else if matches!(rest.first(), Some(RNode::Array { .. })) {
        rest.remove(0);
    }
    for a in rest {
        arg_errors.push((
            a.start(),
            "A Ref field should only have a single binary relationship".to_string(),
        ));
    }
    (callee_errors, arg_errors)
}

// ---- bindFields (global_modules/ref/bind.ts): scan [callee, ...args],
// skip bindees whose table or column fragment is absent, and hand the
// surviving chains to validate.rs for their nodeReferee resolution (the
// document may declare the tables after the ref, so resolution happens
// at validate time)
fn var_pair(v: &RNode) -> (String, usize) {
    match v {
        RNode::Var { name, start } => (name.clone(), *start),
        other => (String::new(), other.start()),
    }
}

/// bindFields (global_modules/ref/bind.ts): scan [callee, ...args]; the
/// split keeps the callee's chains separable (the strict path's model
/// ref already checks those endpoints)
fn bind_fields_split(
    callee: &RNode,
    args: &[RNode],
) -> (Vec<super::ast::BinderChain>, Vec<super::ast::BinderChain>) {
    let scan_one = |n: &RNode, out: &mut Vec<super::ast::BinderChain>| {
        let mut bindees = Vec::new();
        scan_binding(n, &mut bindees);
        for b in bindees {
            if !b.tuple.is_empty() {
                // columnBindees = tupleElements; tableBindee = variables.pop()
                if b.variables.is_empty() {
                    continue;
                }
                out.push(super::ast::BinderChain {
                    vars: b.variables.iter().map(var_pair).collect(),
                    tuple: b.tuple.iter().map(var_pair).collect(),
                    group: false,
                });
            } else {
                // columnBindees = variables.pop(); tableBindee = variables.pop()
                if b.variables.len() < 2 {
                    continue;
                }
                out.push(super::ast::BinderChain {
                    vars: b.variables.iter().map(var_pair).collect(),
                    tuple: Vec::new(),
                    group: false,
                });
            }
        }
    };
    let mut callee_chains = Vec::new();
    let mut arg_chains = Vec::new();
    scan_one(callee, &mut callee_chains);
    for a in args {
        scan_one(a, &mut arg_chains);
    }
    (callee_chains, arg_chains)
}


// ---- validateFieldSettings (local_modules/ref/validate.ts): the rules
// over a ref's `[...]` settings; the occurrences carry each attribute's
// span (the error anchors) -------------------------------------------

/// isValidPolicy (ref/validate.ts): a bare word among the FK actions —
/// quoted strings are NOT policies
/// isValidRecordValue (local_modules/records/validate.ts): empty fields,
/// signed numbers, quoted strings, backtick expressions, the null/true/
/// false keywords, and dotted enum references — everything else errs
fn is_valid_record_value(v: &RNode) -> bool {
    match v {
        RNode::Empty { .. } => true,
        RNode::Lit { color, .. } => !*color,
        RNode::Func { .. } => true,
        RNode::Var { name, .. } => {
            matches!(name.to_ascii_lowercase().as_str(), "null" | "true" | "false")
        }
        // member access: enum field references (status.active) — any
        // dotted chain passes the validate layer
        RNode::Infix { op, .. } if op == "." => true,
        // signed numbers: a prefix +/- on a literal
        RNode::Prefix { op, expr, .. }
            if matches!(op.as_str(), "+" | "-") && matches!(**expr, RNode::Lit { color: false, .. }) =>
        {
            true
        }
        _ => false,
    }
}

fn is_valid_policy(occ: &super::ast::SettingOcc) -> bool {
    if occ.quoted || !occ.has_value {
        return false;
    }
    let Some(v) = &occ.value else { return false };
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "cascade" | "no action" | "set null" | "set default" | "restrict"
    )
}

fn validate_field_settings(occs: &[super::ast::SettingOcc], out: &mut Vec<(usize, usize, String)>) {
    // aggregateSettingList: group the attributes per lowercased name, in
    // first-appearance order
    let mut order: Vec<String> = Vec::new();
    let mut groups: Vec<Vec<&super::ast::SettingOcc>> = Vec::new();
    for o in occs {
        if let Some(i) = order.iter().position(|k| *k == o.key) {
            groups[i].push(o);
        } else {
            order.push(o.key.clone());
            groups.push(vec![o]);
        }
    }
    for (key, attrs) in order.into_iter().zip(groups.into_iter()) {
        match key.as_str() {
            "delete" | "update" => {
                if attrs.len() > 1 {
                    for a in &attrs {
                        out.push((a.offset, a.end, format!("'{}' can only appear once", key)));
                    }
                }
                for a in &attrs {
                    if !is_valid_policy(a) {
                        out.push((
                            a.offset,
                            a.end,
                            format!(
                                "'{}' can only have values \"cascade\", \"no action\", \
                                 \"set null\", \"set default\" or \"restrict\"",
                                key
                            ),
                        ));
                    }
                }
            }
            "color" => {
                if attrs.len() > 1 {
                    for a in &attrs {
                        out.push((a.offset, a.end, "'color' can only appear once".to_string()));
                    }
                }
                for a in &attrs {
                    // extractHexColor: a color literal
                    let ok = a
                        .value
                        .as_deref()
                        .map(|v| {
                            let v = v.trim();
                            v.starts_with('#')
                                && v.len() > 1
                                && v[1..]
                                    .chars()
                                    .all(|c| c.is_ascii_hexdigit())
                        })
                        .unwrap_or(false)
                        && !a.quoted;
                    if !ok {
                        out.push((
                            a.offset,
                            a.end,
                            "'color' must be a color literal".to_string(),
                        ));
                    }
                }
            }
            "note" => {
                if attrs.len() > 1 {
                    for a in &attrs {
                        out.push((a.offset, a.end, "'note' can only appear once".to_string()));
                    }
                }
                for a in &attrs {
                    // extractNote: a string literal
                    if !(a.quoted || a.value.as_deref().map(|v| v.starts_with('\'')).unwrap_or(false))
                    {
                        out.push((
                            a.offset,
                            a.end,
                            "'note' must be a string literal".to_string(),
                        ));
                    }
                }
            }
            "inactive" => {
                if attrs.len() > 1 {
                    for a in &attrs {
                        out.push((
                            a.offset,
                            a.end,
                            "'inactive' can only appear once".to_string(),
                        ));
                    }
                }
                for a in &attrs {
                    if a.has_value {
                        out.push((
                            a.offset,
                            a.end,
                            "'inactive' cannot have a value".to_string(),
                        ));
                    }
                }
            }
            _ => {
                // validateCustomInlineMetadata
                if attrs.len() > 1 {
                    for a in &attrs {
                        out.push((a.offset, a.end, format!("'{}' can only appear once", key)));
                    }
                }
                for a in &attrs {
                    // extractMetadataValue: bool / number / quoted string /
                    // color — a bare word resolves to none of those
                    let ok = a.quoted
                        || a.value.as_deref().map(|v| {
                            let v = v.trim();
                            v == "true"
                                || v == "false"
                                || v.parse::<f64>().is_ok()
                                || (v.starts_with('#')
                                    && v.len() > 1
                                    && v[1..].chars().all(|c| c.is_ascii_hexdigit()))
                        }).unwrap_or(false);
                    if !ok {
                        out.push((
                            a.offset,
                            a.end,
                            format!(
                                "Custom setting '{}' must be a string, a color literal, a number \
                                 literal, or a boolean literal",
                                key
                            ),
                        ));
                    }
                }
            }
        }
    }
}

/// The pipeline entry: parse the body line into their application tree,
/// run validateFields for the shape errors, bindFields for the binder
/// chains. Their parse never fails here, so this always yields their
/// exact diagnostic set.
fn ref_body_pipeline(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    out: &mut Vec<(usize, usize, String)>,
    chains: &mut Vec<super::ast::BinderChain>,
) {
    ref_body_pipeline_mode(input, base, errs, out, chains, false, &mut true);
}

/// `args_only` = the strict path already consumed a clean binary ref:
/// the model carries the callee's endpoint checks, so the diagnostics
/// join from the tree's shape errors (their validateFields — the callee
/// checks still fire: op-leading junk REBINDS the callee, e.g.
/// `a.b > c.d - e.f` makes infix(`-`) the callee) and the ARG chains
/// only. The settings rules ran in ref_settings already, so the Array
/// arg is not re-validated here.
fn ref_body_pipeline_mode(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    out: &mut Vec<(usize, usize, String)>,
    chains: &mut Vec<super::ast::BinderChain>,
    args_only: bool,
    callee_clean: &mut bool,
) {
    let line_end = input.find('\n').unwrap_or(input.len());
    let body = &input[..line_end];
    let sentinel = if line_end < input.len() {
        Some(b'\n')
    } else {
        None
    };
    let (mut toks, _tail_dropped) = ref_tokens(body, base, sentinel, errs);
    let mut lex_errs: Vec<ParseError> = Vec::new();
    toks = gather_invalid_num(toks, &mut lex_errs);
    errs.append(&mut lex_errs);
    // their blockExpression: a line with only trivia and the block's
    // closing brace is not a field — an empty application would produce
    // operand errors THEIR parser never reaches
    if !toks
        .iter()
        .any(|t| t.kind != RTKind::Eof && t.kind != RTKind::RBrace)
    {
        return;
    }
    let mut p = RParser {
        toks,
        pos: 0,
        errs,
        poisoned: None,
    };
    let tree = p.application();
    if let RNode::App { callee, args, .. } = &tree {
        let (callee_errors, arg_errors) = validate_fields_split(callee, args);
        let (callee_chains, arg_chains) = bind_fields_split(callee, args);
        *callee_clean = callee_errors.is_empty();
        let widen = |v: Vec<(usize, String)>| {
            v.into_iter()
                .map(|(o, m)| (o, o, m))
                .collect::<Vec<(usize, usize, String)>>()
        };
        if args_only {
            // the callee's SHAPE errors still fire (op-leading junk
            // rebinds the callee — `a.b > c.d - e.f` → infix(`-`) is not
            // a binary relationship). The CHAINS follow the clean split:
            // a clean callee's endpoint checks live in the model ref, an
            // unclean one's checks live ONLY in the chains (validate.rs
            // skips the non-clean ref's model checks — their interpret is
            // gated by the file's errors)
            out.extend(widen(callee_errors));
            out.extend(widen(arg_errors));
            if *callee_clean {
                chains.extend(arg_chains);
            } else {
                chains.extend(callee_chains);
                chains.extend(arg_chains);
            }
        } else {
            out.extend(widen(callee_errors));
            out.extend(widen(arg_errors));
            chains.extend(callee_chains);
            chains.extend(arg_chains);
            // their nodeSettings validates the settings list (the Array
            // arg) — validateFieldSettings
            for a in args {
                if let RNode::Array { start, end } = a {
                    let slice_start = *start - base;
                    let slice_end = (*end).min(base + body.len()) - base;
                    if slice_start < slice_end {
                        let sub = body[slice_start..slice_end].to_string();
                        let mut cursor = sub.as_str();
                        let mut sub_errs: Vec<ParseError> = Vec::new();
                        if let Ok((_, occs)) =
                            settings_list_occs(&mut cursor, *start, &mut sub_errs)
                        {
                            validate_field_settings(&occs, out);
                        }
                    }
                }
            }
        }
    }
}

fn ref_settings(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    binder_out: &mut Vec<(usize, usize, String)>,
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
    if let Ok((kvs, occs)) = settings_list_occs(input, base, errs) {
        // their validateFieldSettings runs on every ref's settings list
        validate_field_settings(&occs, binder_out);
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
                let mut note_span = None;
                let mut full_end = v_end;
                if peek_is(input, "[") {
                    match settings_list_occs(input, base, errs) {
                        Ok((kvs, occs)) => {
                            // their enum-field token spans the settings
                            // list; the note carries its entry's token
                            full_end = offset(input, base);
                            for occ in &occs {
                                if occ.key == "note" {
                                    if let Some(v) = &occ.value {
                                        note = Some(v.clone());
                                    }
                                    note_span = Some(occ.offset..occ.end);
                                }
                            }
                            let _ = kvs;
                        }
                        Err(err) => {
                            errs.push(err_at(err.offset, &err.message));
                            skip_line(input);
                            e.values.push(EnumValue {
                                name: vname,
                                name_span: v_off..v_end,
                                note: None,
                                note_span: None,
                                full_end: v_end,
                            });
                            continue;
                        }
                    }
                }
                e.values.push(EnumValue {
                    name: vname,
                    name_span: v_off..v_end,
                    note,
                    note_span,
                    full_end,
                });
            }
            Err(err) => {
                errs.push(err_at(err.offset, &err.message));
                skip_line(input);
            }
        }
    }
    e.span = start..offset(input, base);
    Ok(e)
}

fn table_group(
    input: &mut &str,
    base: usize,
    errs: &mut Vec<ParseError>,
    doc_binder_errors: &mut Vec<(usize, usize, String)>,
    binder_chains: &mut Vec<super::ast::BinderChain>,
) -> R<TableGroup> {
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
    let mut custom_spans: Vec<(String, Span)> = Vec::new();
    if peek_is(input, "[") {
        if let Ok((kvs, occs)) = settings_list_occs(input, base, errs) {
            for (k, v) in kvs {
                if let (Some(SettingVal::Str(sv)), "color") = (&v, k.as_str()) {
                    color = Some(sv.clone());
                } else if let Some(SettingVal::Str(sv)) = &v {
                    if let Some(o) = occs.iter().find(|o| o.key == k) {
                        custom_spans.push((k.clone(), o.offset..o.end));
                    }
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
        custom_spans: Vec::new(),
        note_span: None,
    };
    let mut out_group_errors: Vec<(usize, usize, String)> = Vec::new();
    loop {
        ws(input);
        if input.is_empty() {
            errs.push(err_at(offset(input, base), "unterminated table group"));
            break;
        }
        if eat(input, "}") {
            break;
        }
        // `Note: ...` — the group's note (label-hover tooltip)
        if peek_kw_then(input, "Note", ':') {
            let _ = eat(input, "Note");
            ws(input);
            let _ = eat(input, ":");
            ws(input);
            let note_start = offset(input, base);
            match note_value(input, base) {
                Ok(n) => {
                    g.note = Some(n);
                    g.note_span = Some(note_start..offset(input, base));
                }
                Err(e) => {
                    errs.push(err_at(e.offset, &e.message));
                    skip_line(input);
                }
            }
            continue;
        }
        // one member line = their application parse (the same lexer +
        // Pratt tree as ref bodies): callee = the comma expression of
        // comma-separated members, args = the juxtaposed words after the
        // last comma element. validateFields-group + the group binder
        // run on the line (probe4/p12 verified against their engine).
        let line_start = offset(input, base);
        let line_end = input.find('\n').unwrap_or(input.len());
        // a `}` on this line ends the block: the body slice stops there
        let brace = input[..line_end].find('}');
        let slice_end = brace.unwrap_or(line_end);
        let line = &input[..slice_end];
        let sentinel = match brace {
            Some(_) => Some(b'}'),
            None if slice_end < input.len() => Some(b'\n'),
            None => None,
        };
        let (mut line_toks, _tail_dropped) = ref_tokens(line, line_start, sentinel, errs);
        let mut lex_errs: Vec<ParseError> = Vec::new();
        line_toks = gather_invalid_num(line_toks, &mut lex_errs);
        errs.append(&mut lex_errs);
        // their blockExpression: only trivia + the closing brace → not a field
        if !line_toks
            .iter()
            .any(|t| t.kind != RTKind::Eof && t.kind != RTKind::RBrace)
        {
            // advance past the consumed slice (the parser worked on a copy)
            *input = &input[slice_end..];
            if brace.is_some() {
                let _ = eat(input, "}");
                break;
            }
            if input.is_empty() {
                break;
            }
            continue;
        }
        let mut lp = RParser {
            toks: line_toks,
            pos: 0,
            errs,
            poisoned: None,
        };
        let tree = lp.application();
        if let RNode::App { callee, args, .. } = &tree {
            // validateFields-group: the callee must destructure to plain
            // variables; every arg errs once
            if destructure_complex_variable(callee).is_none() {
                out_group_errors.push((
                    callee.start(),
                    callee.start(),
                    "A TableGroup field must be of the form <table> or <schema>.<table>"
                        .to_string(),
                ));
            }
            for a in args {
                out_group_errors.push((
                    a.start(),
                    a.start(),
                    "A TableGroup field should only have a single Table name".to_string(),
                ));
            }
            // the group binder: tableBindee = variables.pop() — ONE pop,
            // so lone words bind too; chains resolve in validate.rs
            let mut bindees = Vec::new();
            scan_binding(callee, &mut bindees);
            for a in args {
                scan_binding(a, &mut bindees);
            }
            for b in bindees {
                if b.variables.is_empty() {
                    continue;
                }
                binder_chains.push(super::ast::BinderChain {
                    vars: b.variables.iter().map(var_pair).collect(),
                    tuple: Vec::new(),
                    group: true,
                });
            }
            // the model keeps the plain variable members (the callee's
            // comma elements / lone callee)
            match &**callee {
                RNode::Comma { elems, .. } => {
                    for e in elems {
                        if let RNode::Var { name, .. } = e {
                            g.tables.push(name.clone());
                        }
                    }
                }
                RNode::Var { name, .. } => g.tables.push(name.clone()),
                _ => {}
            }
        }
        // advance past the consumed slice (the parser worked on a copy)
        *input = &input[slice_end..];
        if brace.is_some() {
            let _ = eat(input, "}");
            break;
        }
        if input.is_empty() {
            break;
        }
    }
    doc_binder_errors.extend(out_group_errors);
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
                errs.push(err_at(e.offset, &e.message));
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
                errs.push(err_at(e.offset, &e.message));
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
    let mut record_count_errors: Vec<(usize, usize, usize, usize)> = Vec::new();
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
            let rec_start = offset(&mut attempt, base);
            match records_block(&mut attempt, base, &mut out.errors, rec_start, &mut record_count_errors, &mut out.binder_errors) {
                Ok((target, rec)) => {
                    rest = attempt;
                    top_records.push((target, rec));
                }
                Err(e) => {
                    // an empty message = the error already pushed (the
                    // poisoned abort's marker)
                    if !e.message.is_empty() {
                        out.errors.push(ParseError {
                            offset: e.offset,
                            end: e.end,
                            message: e.message.to_string(),
                        });
                    }
                    rest = recover(attempt);
                }
            }
            continue;
        }
        let mut attempt = rest;
        match top_item(&mut attempt, base, &mut out.errors, &mut out.binder_errors, &mut out.binder_chains, &mut record_count_errors) {
            Ok(item) => {
                rest = attempt;
                match item {
                    Item::Prj(p) => out.doc.project = Some(p),
                    Item::Tbl(t) => out.doc.tables.push(t),
                    Item::Skip => {}
                    Item::Ref(r) => out.doc.refs.push(r),
                    Item::Enm(e) => out.doc.enums.push(e),
                    Item::Grp(g) => out.doc.table_groups.push(g),
                    Item::Note(n) => out.doc.notes.push(n),
                }
            }
            Err(e) => {
                out.errors.push(ParseError {
                    offset: e.offset,
                    end: None,
                    message: e.message.to_string(),
                });
                rest = recover(attempt);
            }
        }
    }
    // Top-level records blocks attach to their table, which may be defined
    // later in the file (the target matches by name or alias; unmatched
    // targets are dropped — the diagram simply has nothing to show).
    out.record_count_errors = record_count_errors;
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
Table posts {
  id integer [pk]
  user_id integer [ref: > users.id]
}
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
    1, 'First Post', true, 2.50, 1e3, 'draft', `now()`
    2, 'It\'s fine', false, -3, 0.5, NULL,
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
                Text("draft".into()),
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
                Number("0.5".into()),
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
        // their extractDataFromRow: the row is dropped and the count
        // error is interpret-layer ("Expected 2 values but got 3",
        // emitted only while the file is otherwise clean)
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        assert_eq!(
            out.record_count_errors,
            vec![(
                src.find("2, 'too'").unwrap(),
                // the row span ends at the row's last token (their endColumn)
                src.find("'many'").unwrap() + "'many'".len(),
                2,
                3,
            )]
        );
        let d = crate::dbml::validate::validate_with_parse(
            &out.doc,
            src,
            &out.binder_errors,
            &out.binder_chains,
            &out.record_count_errors,
            &out.errors,
        );
        assert!(d
            .iter()
            .any(|x| x.message == "Expected 2 values but got 3"), "{d:?}");
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


#[cfg(test)]
mod errs2_tests {
    use super::*;

    #[test]
    fn errs2_invalid_operand_drops_the_block() {
        let src = std::fs::read_to_string("/home/boilingsoup/Desktop/errs2.dbml")
            .expect("errs2 on the desktop");
        let out = parse(&src);
        for e in &out.errors {
            println!("ERR {}..{:?}: {}", e.offset, e.end, e.message);
        }
        let users = out.doc.tables.iter().find(|t| t.name == "users").unwrap();
        println!(
            "users records: {:?}",
            users.records.as_ref().map(|r| r.rows.len())
        );
        assert!(
            out.errors.iter().any(|e| e.message.contains("Invalid start of operand")),
            "the invalid operand error fires"
        );
        assert!(
            users.records.is_none(),
            "the damaged records block drops entirely"
        );
    }

    /// Their gatherInvalid removes the failed string partial, taking the
    /// line's trailing newline with it — the row's application continues
    /// onto the next line (errs.dbml line 33-34, span-verified against
    /// their engine: row 33 col 3 .. row 34 col 24, 1-based)
    #[test]
    fn dropped_partial_swallows_the_next_row() {
        let src = r#"
Table users {
  id integer
  username varchar
    Records users(id, username) {
    0, 'Alice, 'admin'
    1, 'Bob', 'moderator'
    2, 'Candice'
  }
}
"#;
        let out = parse(src);
        let errs: Vec<&ParseError> = out
            .errors
            .iter()
            .filter(|e| e.message != "Invalid record row structure")
            .collect();
        let row0 = src.find("0, 'Alice").unwrap();
        let admin_quote = src.find("'admin'").unwrap() + "'admin'".len() - 1;
        let moderator_end = src.find("'moderator'").unwrap() + "'moderator'".len();
        // the lexer error anchors the PARTIAL token [quote..scan-stop)
        assert!(
            out.errors.iter().any(|e| {
                e.message == "Invalid newline encountered while parsing"
                    && e.offset == admin_quote
                    && e.end == Some(admin_quote + 1)
            }),
            "the newline error anchors the partial token: {:?}",
            out.errors
        );
        // the two MISSING_SPACES anchor the previous expression
        assert_eq!(
            errs.iter().filter(|e| e.message == "Expect a following space").count(),
            2,
            "two missing-space errors: {:?}",
            errs
        );
        // the structure error (binder channel, surfaces through validate)
        // spans the swallowed application: [row start .. last swallowed
        // token end]
        let d = crate::dbml::validate::validate_with_parse(
            &out.doc,
            src,
            &out.binder_errors,
            &out.binder_chains,
            &out.record_count_errors,
            &out.errors,
        );
        assert!(
            d.iter().any(|x| {
                x.message == "Invalid record row structure"
                    && x.offset == row0
                    && x.end == Some(moderator_end)
            }),
            "the structure error spans the swallow: {d:?}"
        );
        // the swallowed next row does not parse again; the third row stands
        let users = out.doc.tables.iter().find(|t| t.name == "users").unwrap();
        let rec = users.records.as_ref().unwrap();
        assert_eq!(rec.rows.len(), 1, "only 'Candice' survives: {:?}", rec.rows);
    }

    /// c1 battery: a partial string in an otherwise-clean row merges the
    /// next row's values into the callee (no structure error — the values
    /// are comma elements) and the only diagnostic is the lexer error
    #[test]
    fn dropped_partial_merges_values_without_structure_error() {
        let src = r#"
Table t {
  a int
  b varchar
  records (a, b) {
    1, 'ok
    2, 'fine'
    3, 'x'
  }
}
"#;
        let out = parse(src);
        assert_eq!(out.errors.len(), 1, "{:?}", out.errors);
        let quote = src.find("'ok").unwrap();
        assert_eq!(
            (out.errors[0].offset, out.errors[0].end),
            (quote, Some(quote + 3)),
            "the newline error anchors the partial token 'ok"
        );
        assert_eq!(
            out.errors[0].message,
            "Invalid newline encountered while parsing"
        );
    }
}
