//! DBML syntax highlighting.
//!
//! A standalone scanner, not AST-span-based: it must produce sensible
//! colors on partial or broken input (the user is always mid-edit), which
//! a lexer does by construction — it classifies, it never fails. The
//! keyword vocabulary mirrors `parser.rs` exactly (same case-sensitive
//! strings) so highlighting and parsing agree.
//!
//! Output is a complete partition of the source: sorted, non-overlapping
//! tokens covering every byte.

/// Token classes the editor colors. Byte ranges index the scanned source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `Table Ref Enum ...`, settings words (`pk`, `unique`, ...), and
    /// `key:` positions (`Note:`, `database_type:`).
    Keyword,
    /// The name after `Table`/`Enum`/... and relation targets (`a.b`).
    Name,
    /// Column type position: `varchar(255)`, `integer`.
    Type,
    /// Column names, enum members, stray identifiers.
    Ident,
    /// `'...'`, `"..."`, backtick `` `...` ``, triple-quoted strings.
    String,
    /// `//`, `///`, `/* */`.
    Comment,
    Number,
    /// Brackets, commas, operators, dots.
    Punct,
    /// Whitespace and anything unrecognized.
    Plain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Keyword vocabulary — keep in sync with `parser.rs`.
const TOP_KWS: [&str; 5] = ["Project", "Table", "Ref", "Enum", "TableGroup"];
const BLOCK_KWS: [&str; 1] = ["indexes"];
const SETTING_KWS: [&str; 13] = [
    "pk",
    "primary",
    "key",
    "not",
    "null",
    "unique",
    "increment",
    "autoincrement",
    "auto_increment",
    "default",
    "note",
    "ref",
    "headercolor",
];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Block {
    Table,
    Enum,
    Other,
}

pub fn highlight(src: &str) -> Vec<Token> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0usize;

    let mut blocks: Vec<Block> = Vec::new();
    // set while a `Table <name>` / `Enum <name>` name has been seen, so the
    // following `{` opens a block of that kind
    let mut opened: Option<Block> = None;
    // the next bare/quoted word is a definition name (after Table/Enum/...)
    let mut expect_name = false;
    // words after `Ref:` / `[ref:` are relation targets
    let mut name_mode = false;
    // inside [...] settings
    let mut settings: usize = 0;
    // in a table body: subsequent words on the line are the column's type
    let mut type_mode = false;
    // a word has been scanned since the last newline (first-word detection)
    let mut line_has_word = false;

    while i < bytes.len() {
        let rest = &src[i..];
        if rest.starts_with("//") {
            let end = line_end(src, i);
            push(&mut out, i, end, Kind::Comment);
            i = end;
        } else if rest.starts_with("/*") {
            let end = src[i + 2..].find("*/").map_or(src.len(), |k| i + 2 + k + 2);
            push(&mut out, i, end, Kind::Comment);
            i = end;
        } else if rest.starts_with("'''") {
            let end = src[i + 3..]
                .find("'''")
                .map_or(src.len(), |k| i + 3 + k + 3);
            push(&mut out, i, end, Kind::String);
            i = end;
        } else if bytes[i] == b'\'' || bytes[i] == b'"' || bytes[i] == b'`' {
            let quote = bytes[i];
            let end = quoted_end(src, i, quote);
            // quoted identifiers take Name color in name/definition position
            let kind = if bytes[i] == b'"' && (expect_name || (name_mode && settings == 0)) {
                Kind::Name
            } else {
                Kind::String
            };
            push(&mut out, i, end, kind);
            if bytes[i] == b'"' && kind == Kind::Name {
                consume_name(&mut expect_name);
                mark_opened(src, i, &mut opened);
            }
            line_has_word = true;
            i = end;
        } else if bytes[i] == b'[' {
            push(&mut out, i, i + 1, Kind::Punct);
            settings += 1;
            i += 1;
        } else if bytes[i] == b']' {
            push(&mut out, i, i + 1, Kind::Punct);
            settings = settings.saturating_sub(1);
            i += 1;
        } else if bytes[i] == b'{' {
            push(&mut out, i, i + 1, Kind::Punct);
            blocks.push(opened.take().unwrap_or(Block::Other));
            type_mode = false;
            i += 1;
        } else if bytes[i] == b'}' {
            push(&mut out, i, i + 1, Kind::Punct);
            blocks.pop();
            type_mode = false;
            i += 1;
        } else if bytes[i] == b'\n' {
            let end = i + 1;
            push(&mut out, i, end, Kind::Plain);
            line_has_word = false;
            type_mode = false;
            name_mode = false;
            i = end;
        } else if (bytes[i] as char).is_whitespace() {
            let start = i;
            while i < bytes.len() && (bytes[i] as char).is_whitespace() && bytes[i] != b'\n' {
                i += 1;
            }
            push(&mut out, start, i, Kind::Plain);
        } else if (bytes[i] as char).is_ascii_digit() {
            let start = i;
            while i < bytes.len() && ((bytes[i] as char).is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let kind = if type_mode { Kind::Type } else { Kind::Number };
            push(&mut out, start, i, kind);
        } else if (bytes[i] as char).is_alphanumeric() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() {
                let ch = bytes[i] as char;
                if ch.is_alphanumeric() || ch == '_' {
                    i += 1;
                } else {
                    break;
                }
            }
            let word = &src[start..i];
            let kind = classify(
                word,
                &blocks,
                settings > 0,
                expect_name,
                name_mode,
                type_mode,
                line_has_word,
                rest,
            );
            // state updates
            if TOP_KWS.iter().any(|kw| word.eq_ignore_ascii_case(kw)) {
                if word.eq_ignore_ascii_case("Ref") {
                    name_mode = true;
                } else {
                    expect_name = true;
                    opened = None;
                }
            } else if word.eq_ignore_ascii_case("as") {
                // `Table users as U` — the alias is another name
                expect_name = true;
            } else if word.eq_ignore_ascii_case("ref") {
                // inline `[ref: > a.b]` — targets color as names
                name_mode = true;
            } else if kind == Kind::Name {
                consume_name(&mut expect_name);
                mark_opened(src, start, &mut opened);
            } else if kind == Kind::Ident && in_table_body(&blocks) && settings == 0 {
                // first word of the line is the column name; the rest of the
                // line (through the settings brackets) is its type
                if !line_has_word {
                    type_mode = true;
                }
            }
            line_has_word = true;
            push(&mut out, start, i, kind);
        } else {
            let len = src[i..].chars().next().map_or(1, char::len_utf8);
            push(&mut out, i, i + len, Kind::Punct);
            if bytes[i] == b',' || bytes[i] == b']' {
                name_mode = false;
            }
            i += len;
        }
    }
    out
}

fn consume_name(expect_name: &mut bool) {
    *expect_name = false;
}

fn in_table_body(blocks: &[Block]) -> bool {
    matches!(blocks.last(), Some(Block::Table))
}

/// Whether the text just before a definition name ends with one of `kws`
/// (ASCII case-insensitive — `table users` must register a table body the
/// same way `Table users` does).
fn prev_was(src: &str, name_start: usize, kws: &[&str]) -> bool {
    let before = &src[..name_start];
    let trimmed = before.trim_end_matches([' ', '\t']);
    kws.iter().any(|kw| {
        trimmed.len() >= kw.len() && trimmed[trimmed.len() - kw.len()..].eq_ignore_ascii_case(kw)
    })
}

/// If the name that just ended at `name_end` follows a `Table`/`Enum`
/// keyword, remember the block kind for the following `{`.
fn mark_opened(src: &str, name_end: usize, opened: &mut Option<Block>) {
    if opened.is_some() {
        return;
    }
    if prev_was(src, name_end, &["Table"]) {
        *opened = Some(Block::Table);
    } else if prev_was(src, name_end, &["Enum"]) {
        *opened = Some(Block::Enum);
    }
}

#[allow(clippy::too_many_arguments)] // scanner flags; a struct would obscure the port
fn classify(
    word: &str,
    blocks: &[Block],
    in_settings: bool,
    expect_name: bool,
    name_mode: bool,
    type_mode: bool,
    line_has_word: bool,
    rest: &str,
) -> Kind {
    if expect_name || name_mode {
        return Kind::Name;
    }
    if word.eq_ignore_ascii_case("as") {
        return Kind::Keyword;
    }
    if TOP_KWS.iter().any(|kw| word.eq_ignore_ascii_case(kw))
        || BLOCK_KWS.iter().any(|kw| word.eq_ignore_ascii_case(kw))
    {
        return Kind::Keyword;
    }
    if in_settings {
        if SETTING_KWS.iter().any(|kw| word.eq_ignore_ascii_case(kw)) {
            return Kind::Keyword;
        }
        return Kind::Plain;
    }
    // `key:` positions (Project settings, Note:, database_type:)
    if rest[word.len()..]
        .trim_start_matches([' ', '\t'])
        .starts_with(':')
    {
        return Kind::Keyword;
    }
    if type_mode && line_has_word && in_table_body(blocks) {
        return Kind::Type;
    }
    if type_mode {
        return Kind::Type;
    }
    match blocks.last() {
        Some(Block::Enum) | Some(Block::Table) => Kind::Ident,
        _ => Kind::Ident,
    }
}

fn push(out: &mut Vec<Token>, start: usize, end: usize, kind: Kind) {
    if end > start {
        out.push(Token { start, end, kind });
    }
}

fn line_end(src: &str, from: usize) -> usize {
    src[from..].find('\n').map_or(src.len(), |i| from + i)
}

/// End of a quoted run opening at `open` (the quote byte index). `''`
/// doubles as an escaped quote inside `'...'`. A newline always terminates
/// so one bad quote can't swallow the document. EOF if unterminated.
fn quoted_end(src: &str, open: usize, quote: u8) -> usize {
    let b = src.as_bytes();
    let mut i = open + 1;
    while i < b.len() {
        if b[i] == quote {
            if quote == b'\'' && i + 1 < b.len() && b[i + 1] == b'\'' {
                i += 2;
                continue;
            }
            return i + 1;
        }
        if b[i] == b'\n' {
            return i; // unterminated: stop at end of line
        }
        i += 1;
    }
    src.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(String, Kind)> {
        highlight(src)
            .into_iter()
            .filter(|t| {
                !matches!(t.kind, Kind::Plain | Kind::Punct) && src[t.start..t.end].trim() != ""
            })
            .map(|t| (src[t.start..t.end].to_string(), t.kind))
            .collect()
    }

    /// The output must be a sorted, non-overlapping cover of the source.
    fn assert_cover(src: &str) {
        let toks = highlight(src);
        let mut at = 0usize;
        for t in &toks {
            assert_eq!(t.start, at, "gap/overlap at {at}: {toks:?}");
            assert!(t.end <= src.len());
            at = t.end;
        }
        assert_eq!(at, src.len(), "not covering: {toks:?}");
    }

    #[test]
    fn covers_wellformed_and_broken_input() {
        for src in [
            "",
            "Table users { id integer [pk] }",
            "Table users { id integer [pk",
            "Table \"broken { id",
            "'''unterminated",
            "// comment\n/* unterminated",
            "'unclosed string\nTable x {",
            dbml_ui_sample(),
        ] {
            assert_cover(src);
        }
    }

    fn dbml_ui_sample() -> &'static str {
        crate::SAMPLE_DBML
    }

    #[test]
    fn colors_the_basic_document() {
        let ks = kinds("Table users as U {\n  id integer [pk, increment]\n  username varchar(80)\n}\nRef: posts.user_id > users.id");
        assert!(ks.contains(&("Table".into(), Kind::Keyword)));
        assert!(ks.contains(&("users".into(), Kind::Name)));
        assert!(ks.contains(&("as".into(), Kind::Keyword)));
        assert!(ks.contains(&("U".into(), Kind::Name)));
        assert!(ks.contains(&("id".into(), Kind::Ident)));
        assert!(ks.contains(&("integer".into(), Kind::Type)));
        assert!(ks.contains(&("varchar".into(), Kind::Type)));
        assert!(ks.contains(&("pk".into(), Kind::Keyword)));
        assert!(ks.contains(&("increment".into(), Kind::Keyword)));
        assert!(ks.contains(&("Ref".into(), Kind::Keyword)));
        // relation targets are names
        assert!(ks.contains(&("posts".into(), Kind::Name)));
        assert!(ks.contains(&("user_id".into(), Kind::Name)));
    }

    #[test]
    fn comments_and_strings_win_over_keywords() {
        let ks = kinds("// Table pk\n/* Ref */\nnote: 'it''s ok'\nNote: 'a' }");
        assert!(ks.contains(&("// Table pk".into(), Kind::Comment)));
        assert!(ks.contains(&("/* Ref */".into(), Kind::Comment)));
        // '' is an escaped quote and stays inside one string token
        assert!(ks.contains(&("'it''s ok'".into(), Kind::String)), "{ks:?}");
        assert!(ks.contains(&("'a'".into(), Kind::String)));
        assert!(ks.contains(&("Note".into(), Kind::Keyword)), "{ks:?}");
    }

    #[test]
    fn types_highlight_in_any_case_block() {
        // regression: prev_was was case-sensitive, so after renaming
        // `Table` to `table` the block stopped registering as a table body
        // and its types lost their color
        let src = "table users {\n  id integer [pk]\n}\nTaBlE posts {\n  title varchar(200)\n}\nTABLE comments {\n  body text\n}\n";
        let ks = kinds(src);
        for (word, _) in [
            ("integer", "integer"),
            ("varchar", "varchar"),
            ("text", "text"),
        ] {
            assert!(
                ks.iter().any(|(s, k)| s == word && *k == Kind::Type),
                "{word} not Type in {ks:?}"
            );
        }
        // names/keywords still fine in the mixed-case blocks
        for w in ["users", "posts", "comments"] {
            assert!(ks.iter().any(|(s, k)| s == w && *k == Kind::Name), "{ks:?}");
        }
        for w in ["table", "TaBlE", "TABLE"] {
            assert!(
                ks.iter().any(|(s, k)| s == w && *k == Kind::Keyword),
                "{ks:?}"
            );
        }
    }

    #[test]
    fn triple_quoted_notes_span_lines() {
        let src = "Table t {\n  a varchar [note: '''\n  multi\n  line\n  ''']\n}";
        let ks = kinds(src);
        assert!(
            ks.iter()
                .any(|(s, k)| *k == Kind::String && s.contains("multi")),
            "{ks:?}"
        );
        assert_cover(src);
    }

    #[test]
    fn unterminated_constructs_degrade_to_eof() {
        let ks = kinds("Table x { a varchar [note: 'oops");
        assert!(
            ks.iter()
                .any(|(s, k)| *k == Kind::String && s.starts_with("'oops")),
            "{ks:?}"
        );
        assert_cover("note: 'oops");
        assert_cover("/* never closed");
    }

    #[test]
    fn settings_and_project_keys() {
        let ks = kinds(
            "Project blog {\n  database_type: 'PostgreSQL'\n  Note: 'x'\n}\nTable t {\n  a integer [not null, unique, default: `now()`]\n}",
        );
        assert!(ks.contains(&("database_type".into(), Kind::Keyword)));
        assert!(ks.contains(&("Note".into(), Kind::Keyword)));
        assert!(ks.contains(&("not".into(), Kind::Keyword)));
        assert!(ks.contains(&("null".into(), Kind::Keyword)));
        assert!(ks.contains(&("unique".into(), Kind::Keyword)));
        assert!(ks.contains(&("default".into(), Kind::Keyword)));
        assert!(ks.contains(&("`now()`".into(), Kind::String)));
    }

    #[test]
    fn enum_members_and_inline_ref() {
        let ks = kinds(
            "Enum post_status {\n  draft\n  published\n}\nTable t { status post_status [ref: > e.v] }",
        );
        assert!(ks.contains(&("draft".into(), Kind::Ident)));
        assert!(ks.contains(&("published".into(), Kind::Ident)));
        assert!(ks.contains(&("ref".into(), Kind::Keyword)));
        assert!(ks.contains(&("e".into(), Kind::Name)));
    }

    #[test]
    fn indexes_block_and_tablegroup() {
        let ks = kinds("Table t {\n  indexes {\n    (a, b) [unique]\n  }\n}\nTableGroup g { t }");
        assert!(ks.contains(&("indexes".into(), Kind::Keyword)));
        assert!(ks.contains(&("TableGroup".into(), Kind::Keyword)));
        assert!(ks.contains(&("unique".into(), Kind::Keyword)));
        // `a` inside indexes is not a column name starting a type run
        assert!(!ks.contains(&("a".into(), Kind::Type)), "{ks:?}");
    }

    #[test]
    fn multiword_type_and_numbers() {
        let ks = kinds("Table t {\n  price decimal(10, 2)\n}");
        assert!(ks.contains(&("decimal".into(), Kind::Type)));
        assert!(ks.contains(&("10".into(), Kind::Type)));
        assert!(ks.contains(&("2".into(), Kind::Type)));
    }
}
