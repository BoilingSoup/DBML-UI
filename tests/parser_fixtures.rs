use dbml_ui::dbml::parser::{self};

macro_rules! parse_fixture {
    ($name:literal) => {
        parser::parse(include_str!(concat!("fixtures/", $name)))
    };
}

#[test]
fn basic_fixture_parses_cleanly() {
    let out = parse_fixture!("basic.dbml");
    assert!(out.errors.is_empty(), "{:?}", out.errors);

    let doc = &out.doc;
    assert_eq!(doc.tables.len(), 3);
    assert_eq!(doc.enums.len(), 1);
    assert_eq!(doc.table_groups.len(), 1);
    // 2 standalone + 2 inline refs
    assert_eq!(doc.refs.len(), 2);
    assert_eq!(
        doc.tables
            .iter()
            .filter(|t| t.columns.iter().any(|c| c.settings.inline_ref.is_some()))
            .count(),
        2
    );

    let users = doc.tables.iter().find(|t| t.name == "users").unwrap();
    assert_eq!(users.columns.len(), 5);
    assert_eq!(users.indexes.len(), 1);
    assert!(users.matches("U"));

    let posts = doc.tables.iter().find(|t| t.name == "posts").unwrap();
    assert_eq!(posts.columns.len(), 6);
    assert_eq!(posts.header_color.as_deref(), Some("#2ECC71"));
    let status = posts.columns.iter().find(|c| c.name == "status").unwrap();
    assert_eq!(status.ty, "post_status");

    let group = &doc.table_groups[0];
    assert_eq!(group.tables, ["posts", "comments"]);
}

#[test]
fn basic_fixture_spans_are_valid_slices() {
    let out = parse_fixture!("basic.dbml");
    let src = include_str!("fixtures/basic.dbml");
    for t in &out.doc.tables {
        let slice = &src[t.span.clone()];
        assert!(slice.starts_with("Table"), "{slice:?}");
        assert!(slice.contains(&t.name));
        assert_eq!(&src[t.name_span.clone()], t.name);
    }
}

#[test]
fn broken_fixture_recovers_and_reports() {
    let out = parse_fixture!("broken.dbml");
    assert!(!out.errors.is_empty());

    let names: Vec<&str> = out.doc.tables.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"good1"), "{names:?}");
    assert!(names.contains(&"good2"), "{names:?}");
    assert!(names.contains(&"quoted table"), "{names:?}");

    // good2 loses only the broken line
    let good2 = out.doc.tables.iter().find(|t| t.name == "good2").unwrap();
    assert_eq!(good2.columns.len(), 2);

    // ref after the broken construct still parsed
    assert_eq!(out.doc.refs.len(), 1);

    // broken enum keeps its good values
    let enm = &out.doc.enums[0];
    assert_eq!(enm.values.len(), 2);

    // every error offset points at a char boundary in range
    let src = include_str!("fixtures/broken.dbml");
    for e in &out.errors {
        assert!(e.offset <= src.len());
        assert!(src.is_char_boundary(e.offset));
        let (line, _) = parser::line_col_at(src, e.offset);
        assert!(line < src.lines().count() + 1);
    }
}
