//! Context-aware DBML completion items for the editor popup.
//!
//! The source of truth is the current layout (table/enum names and their
//! rows) plus the parser's keyword vocabulary — no second grammar to keep
//! in sync. A second source rides along like dbdiagram's does: Monaco's
//! word-based suggestions (every word already in the document), ranked
//! after the DBML-native items.

use dbml_ui::layout::Layout;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Keyword,
    Table,
    Enum,
    Column,
    EnumValue,
    /// A word already present in the document (dbdiagram's second source,
    /// Monaco's word-based suggestions), offered after the DBML items.
    Word,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub kind: ItemKind,
}

const TOP_KWS: [&str; 6] = ["Project", "Table", "Ref", "Enum", "TableGroup", "Note"];
const BLOCK_KWS: [&str; 1] = ["indexes"];
const SETTING_KWS: [&str; 11] = [
    "pk",
    "primary key",
    "unique",
    "increment",
    "not null",
    "null",
    "default",
    "note",
    "ref: >",
    "headercolor",
    "color",
];

/// Completions for the word being typed at the cursor.
///
/// `line_before` is the current line's text before the cursor column; it
/// decides the context: endswith `.` -> members of the named table/enum,
/// unbalanced `[` -> settings keywords, else top-level keywords + known
/// definition names. `source` is the whole document, feeding the
/// word-based suggestions (empty string disables them; they are skipped
/// in the member context, where the owner's columns/values are the point).
pub fn complete(prefix: &str, line_before: &str, layout: &Layout, source: &str) -> Vec<Item> {
    let member = line_before
        .trim_end()
        .strip_suffix('.')
        .and_then(word_before);
    let mut items = if let Some(owner) = &member {
        member_items(layout, owner)
    } else if unbalanced_bracket(line_before) {
        SETTING_KWS
            .iter()
            .map(|k| Item {
                label: k.to_string(),
                kind: ItemKind::Keyword,
            })
            .collect()
    } else {
        definition_items(layout)
    };
    if member.is_none() && !source.is_empty() {
        // words already in the document never duplicate a DBML item (a
        // table named `users` hides the word). dbdiagram merges both
        // sources into one score-ranked menu; here the DBML-native items
        // keep strict priority — they are ranked first, and document
        // words fill the remaining menu slots.
        let taken: HashSet<String> = items
            .iter()
            .map(|i| i.label.to_lowercase())
            .collect();
        let mut words: Vec<Item> = word_items(source, prefix)
            .into_iter()
            .filter(|w| !taken.contains(&w.label.to_lowercase()))
            .collect();
        filter(&mut items, prefix);
        filter(&mut words, prefix);
        items.extend(words);
    } else {
        filter(&mut items, prefix);
    }
    items.truncate(50);
    items
}

fn word_before(s: &str) -> Option<String> {
    let word: String = s
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let word = word.chars().rev().collect::<String>();
    (!word.is_empty()).then_some(word)
}

fn unbalanced_bracket(line: &str) -> bool {
    line.matches('[').count() > line.matches(']').count()
}

/// Columns of a table (or values of an enum) after `<name>.`. Enums are
/// not diagram boxes, so their values come from the layout's enum list.
fn member_items(layout: &Layout, owner: &str) -> Vec<Item> {
    if let Some(b) = layout.boxes.iter().find(|b| b.title == owner) {
        return b
            .rows
            .iter()
            .map(|r| Item {
                label: r.name.clone(),
                kind: ItemKind::Column,
            })
            .collect();
    }
    match layout.enums.iter().find(|e| e.name == owner) {
        Some(e) => e
            .values
            .iter()
            .map(|v| Item {
                label: v.clone(),
                kind: ItemKind::EnumValue,
            })
            .collect(),
        None => Vec::new(),
    }
}

/// Top-level keywords plus every table/enum name.
fn definition_items(layout: &Layout) -> Vec<Item> {
    let mut items: Vec<Item> = TOP_KWS
        .iter()
        .chain(BLOCK_KWS.iter())
        .map(|k| Item {
            label: k.to_string(),
            kind: ItemKind::Keyword,
        })
        .collect();
    for b in &layout.boxes {
        items.push(Item {
            label: b.title.clone(),
            kind: ItemKind::Table,
        });
    }
    for e in &layout.enums {
        items.push(Item {
            label: e.name.clone(),
            kind: ItemKind::Enum,
        });
    }
    items
}

/// Case-insensitive prefix matches first, then substring matches; capped.
fn filter(items: &mut Vec<Item>, prefix: &str) {
    if prefix.is_empty() {
        items.truncate(50);
        return;
    }
    let lower = prefix.to_lowercase();
    items.retain(|i| i.label.to_lowercase().contains(&lower));
    items.sort_by_key(|i| !i.label.to_lowercase().starts_with(&lower));
    items.truncate(50);
}

/// Words already in the document, in first-occurrence order — dbdiagram's
/// second completion source (Monaco's `textualSuggest` worker): every
/// identifier-like run, deduped, with the word being typed and pure
/// numbers excluded (their `!isNaN(Number(word))` skip). The completion
/// model does the actual matching; this only enumerates candidates.
pub fn word_items(source: &str, exclude: &str) -> Vec<Item> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for line in source.lines() {
        for word in line.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
            if word.is_empty() || word == exclude || word.parse::<f64>().is_ok() {
                continue;
            }
            if seen.insert(word) {
                out.push(Item {
                    label: word.to_string(),
                    kind: ItemKind::Word,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbml_ui::{dbml::parser, layout, model};

    fn sample_layout() -> Layout {
        layout::layout(&model::build(&parser::parse(dbml_ui::SAMPLE_DBML).doc))
    }

    #[test]
    fn dot_completes_table_columns() {
        let l = sample_layout();
        let items = complete("", "Ref: posts.user_id > users.", &l, "");
        for want in ["id", "username", "email", "role", "created_at"] {
            assert!(
                items
                    .iter()
                    .any(|i| i.label == want && i.kind == ItemKind::Column),
                "missing {want}: {items:?}"
            );
        }
    }

    #[test]
    fn dot_completes_enum_values() {
        let l = sample_layout();
        let items = complete("d", "status post_status [default: post_status.", &l, "");
        assert!(
            items
                .iter()
                .any(|i| i.label == "draft" && i.kind == ItemKind::EnumValue),
            "{items:?}"
        );
    }

    #[test]
    fn dot_with_unknown_table_is_empty() {
        let l = sample_layout();
        assert!(complete("", "Ref: nope.", &l, "").is_empty());
    }

    #[test]
    fn settings_context_lists_attributes() {
        let l = sample_layout();
        let items = complete("", "  id integer [", &l, "");
        for want in ["pk", "unique", "not null", "default", "ref: >"] {
            assert!(items.iter().any(|i| i.label == want), "missing {want}");
        }
        assert!(
            !items.iter().any(|i| i.label == "Table"),
            "top keywords leak into settings"
        );
    }

    #[test]
    fn color_keyword_is_offered_in_settings_context() {
        let l = sample_layout();
        // the TableGroup setting: `TableGroup g [color: #1e69fd] { ... }`
        let items = complete("", "TableGroup g [", &l, "");
        assert!(
            items
                .iter()
                .any(|i| i.label == "color" && i.kind == ItemKind::Keyword),
            "{items:?}"
        );
        // and it filters like every other keyword
        let items = complete("col", "TableGroup g [", &l, "");
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("color"));
    }

    #[test]
    fn top_context_lists_keywords_and_names() {
        let l = sample_layout();
        let items = complete("", "", &l, "");
        for want in [
            "Table",
            "Ref",
            "Enum",
            "Project",
            "TableGroup",
            "users",
            "post_status",
        ] {
            assert!(items.iter().any(|i| i.label == want), "missing {want}");
        }
    }

    #[test]
    fn sticky_note_is_offered_at_top_level() {
        let l = sample_layout();
        let items = complete("", "", &l, "");
        assert!(
            items
                .iter()
                .any(|i| i.label == "Note" && i.kind == ItemKind::Keyword),
            "{items:?}"
        );
        // it filters like every other keyword (same Item shape: label + kind)
        let items = complete("No", "", &l, "");
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("Note"));
        // and stays out of the settings menu, where the `note` attribute
        // already lives
        let items = complete("", "  id integer [", &l, "");
        assert!(!items.iter().any(|i| i.label == "Note"), "{items:?}");
    }

    #[test]
    fn filter_is_case_insensitive_and_prefix_first() {
        let l = sample_layout();
        let items = complete("US", "", &l, "");
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("users"));
        let items = complete("ta", "", &l, "");
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("Table"));
    }

    #[test]
    fn no_matches_is_empty() {
        let l = sample_layout();
        assert!(complete("zzz", "", &l, "").is_empty());
    }

    // ---- word-based suggestions (dbdiagram's second source) ----

    #[test]
    fn document_words_rank_after_the_dbml_items() {
        let l = sample_layout();
        let src = "Table posts as p {\n  author_id integer\n}\nRef: posts.author_id > users.id";
        let items = complete("a", "", &l, src);
        // the word is offered, as a Word item
        assert!(
            items
                .iter()
                .any(|i| i.label == "author_id" && i.kind == ItemKind::Word),
            "{items:?}"
        );
        // and no DBML item ever trails a Word one
        let first_word = items.iter().position(|i| i.kind == ItemKind::Word);
        let last_dbml = items.iter().rposition(|i| i.kind != ItemKind::Word);
        if let (Some(w), Some(d)) = (first_word, last_dbml) {
            assert!(d < w, "dbml items must precede words: {items:?}");
        }
    }

    #[test]
    fn words_skip_numbers_the_typed_word_and_duplicates() {
        // their worker: `word !== currentWord && isNaN(Number(word))`,
        // deduped with a case-SENSITIVE set (abc and ABC both survive)
        let items = word_items("abc 123 2.50 1e3 abc ABC\ntyped typed_x", "typed");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["abc", "ABC", "typed_x"]);
    }

    #[test]
    fn words_do_not_leak_into_the_member_context() {
        let l = sample_layout();
        let items = complete("u", "Ref: posts.user_id > users.", &l, "users posts qux");
        assert!(
            items.iter().all(|i| i.kind != ItemKind::Word),
            "member menus stay pure: {items:?}"
        );
    }

    #[test]
    fn words_never_duplicate_a_dbml_item() {
        let l = sample_layout();
        // `users` is a Table item; the same word in the doc must not repeat
        let src = "Ref: posts.user_id > users.id";
        let items = complete("users", "", &l, src);
        assert_eq!(
            items.iter().filter(|i| i.label == "users").count(),
            1,
            "{items:?}"
        );
    }
}
