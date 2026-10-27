//! Context-aware DBML completion items for the editor popup.
//!
//! The source of truth is the current layout (table/enum names and their
//! rows) plus the parser's keyword vocabulary — no second grammar to keep
//! in sync.

use dbml_ui::layout::Layout;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Keyword,
    Table,
    Enum,
    Column,
    EnumValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub kind: ItemKind,
}

const TOP_KWS: [&str; 5] = ["Project", "Table", "Ref", "Enum", "TableGroup"];
const BLOCK_KWS: [&str; 1] = ["indexes"];
const SETTING_KWS: [&str; 10] = [
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
];

/// Completions for the word being typed at the cursor.
///
/// `line_before` is the current line's text before the cursor column; it
/// decides the context: endswith `.` -> members of the named table/enum,
/// unbalanced `[` -> settings keywords, else top-level keywords + known
/// definition names.
pub fn complete(prefix: &str, line_before: &str, layout: &Layout) -> Vec<Item> {
    let mut items = if let Some(owner) = line_before
        .trim_end()
        .strip_suffix('.')
        .and_then(word_before)
    {
        member_items(layout, &owner)
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
    filter(&mut items, prefix);
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
        let items = complete("", "Ref: posts.user_id > users.", &l);
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
        let items = complete("d", "status post_status [default: post_status.", &l);
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
        assert!(complete("", "Ref: nope.", &l).is_empty());
    }

    #[test]
    fn settings_context_lists_attributes() {
        let l = sample_layout();
        let items = complete("", "  id integer [", &l);
        for want in ["pk", "unique", "not null", "default", "ref: >"] {
            assert!(items.iter().any(|i| i.label == want), "missing {want}");
        }
        assert!(
            !items.iter().any(|i| i.label == "Table"),
            "top keywords leak into settings"
        );
    }

    #[test]
    fn top_context_lists_keywords_and_names() {
        let l = sample_layout();
        let items = complete("", "", &l);
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
    fn filter_is_case_insensitive_and_prefix_first() {
        let l = sample_layout();
        let items = complete("US", "", &l);
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("users"));
        let items = complete("ta", "", &l);
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("Table"));
    }

    #[test]
    fn no_matches_is_empty() {
        let l = sample_layout();
        assert!(complete("zzz", "", &l).is_empty());
    }
}
