use crate::buffer::DocumentError;
use crate::{FrontmatterValue, LineEnding, parse_document, serialize_document};

const RICH: &str = concat!(
    "---\r\n",
    "title: \"Hello\"\r\n",
    "slug: stable-source-slug\r\n",
    "# a real comment\r\n",
    "draft: false\r\n",
    "tags: [rust, astro]\r\n",
    "description: |\r\n",
    "  A multiline\r\n",
    "  block scalar.\r\n",
    "author:\r\n",
    "  name: \"Ada Lovelace\"\r\n",
    "  email: ada@example.com\r\n",
    "unknown_field: 你好\r\n",
    "date: 2024-01-01\r\n",
    "---\r\n",
    "import Card from \"./Card.astro\";\r\n",
    "\r\n",
    "# 見出し\r\n",
    "<Card>本文</Card>\r\n",
);

#[test]
fn empty_file_round_trips() {
    let document = parse_document("").expect("empty file parses");
    assert!(!document.has_frontmatter());
    assert_eq!(serialize_document(&document).unwrap(), "");
}

#[test]
fn file_without_frontmatter_preserves_body() {
    let source = "# Title\n\nBody without frontmatter\n";
    let document = parse_document(source).unwrap();
    assert!(!document.has_frontmatter());
    assert_eq!(document.body(), source);
    assert_eq!(serialize_document(&document).unwrap(), source);
}

#[test]
fn unchanged_rich_frontmatter_round_trips_exactly() {
    let document = parse_document(RICH).expect("rich document parses");
    assert!(document.has_frontmatter());
    assert_eq!(document.line_ending(), LineEnding::Crlf);
    assert_eq!(serialize_document(&document).unwrap(), RICH);
}

#[test]
fn reads_typed_and_nested_scalars() {
    let document = parse_document(RICH).unwrap();
    assert_eq!(
        document.field("title"),
        Some(&FrontmatterValue::String("Hello".to_owned()))
    );
    assert_eq!(
        document.field("draft"),
        Some(&FrontmatterValue::Boolean(false))
    );
    assert_eq!(
        document.field("author.name"),
        Some(&FrontmatterValue::String("Ada Lovelace".to_owned()))
    );
    assert_eq!(
        document.field("author.email"),
        Some(&FrontmatterValue::String("ada@example.com".to_owned()))
    );
    assert_eq!(
        document.field("date"),
        Some(&FrontmatterValue::String("2024-01-01".to_owned()))
    );
    assert!(
        document.field("tags").is_none(),
        "opaque flow field is not exposed as a typed value"
    );
    assert!(document.field("missing").is_none());
}

#[test]
fn patch_nested_scalar_preserves_all_other_bytes() {
    let mut document = parse_document(RICH).unwrap();
    document
        .set_field(
            "author.name",
            FrontmatterValue::String("Grace Hopper".to_owned()),
        )
        .expect("nested scalar is patchable");

    let output = serialize_document(&document).unwrap();
    let expected = RICH.replace("\"Ada Lovelace\"", "Grace Hopper");

    assert_eq!(output, expected, "only the patched scalar may change");
    assert!(output.contains("# a real comment"), "comment preserved");
    assert!(
        output.contains("  A multiline\r\n  block scalar."),
        "block scalar preserved"
    );
    assert!(
        output.contains("unknown_field: 你好"),
        "unicode key preserved"
    );
    assert!(
        output.contains("import Card from \"./Card.astro\";\r\n"),
        "body bytes preserved"
    );

    let reparsed = parse_document(&output).unwrap();
    assert_eq!(
        reparsed.field("author.name"),
        Some(&FrontmatterValue::String("Grace Hopper".to_owned()))
    );
}

#[test]
fn patched_string_that_needs_quoting_round_trips() {
    let mut document = parse_document("---\ntitle: hello\n---\nBody\n").unwrap();
    document
        .set_field("title", FrontmatterValue::String("a: b".to_owned()))
        .unwrap();
    let output = serialize_document(&document).unwrap();
    assert_eq!(output, "---\ntitle: 'a: b'\n---\nBody\n");
    assert_eq!(
        parse_document(&output).unwrap().field("title"),
        Some(&FrontmatterValue::String("a: b".to_owned()))
    );
}

#[test]
fn patched_boolean_and_integer_are_plain() {
    let mut document = parse_document("---\ndraft: false\ncount: 1\n---\n").unwrap();
    document
        .set_field("draft", FrontmatterValue::Boolean(true))
        .unwrap();
    document
        .set_field("count", FrontmatterValue::Integer(5))
        .unwrap();
    let output = serialize_document(&document).unwrap();
    assert_eq!(output, "---\ndraft: true\ncount: 5\n---\n");
}

#[test]
fn malformed_yaml_is_invalid() {
    assert!(matches!(
        parse_document("---\ntitle: [unclosed\n---\nbody\n"),
        Err(DocumentError::InvalidFrontmatter)
    ));
    assert!(matches!(
        parse_document("---\ntitle: x\n"),
        Err(DocumentError::InvalidFrontmatter)
    ));
}

#[test]
fn anchors_aliases_and_tags_are_unsupported() {
    assert!(matches!(
        parse_document("---\ntitle: &a value\n---\n"),
        Err(DocumentError::UnsupportedFrontmatter { .. })
    ));
    // A bare alias is invalid YAML on its own; either refusal is acceptable.
    assert!(parse_document("---\ntitle: *alias\n---\n").is_err());
}

#[test]
fn duplicate_keys_are_rejected() {
    assert!(matches!(
        parse_document("---\ntitle: a\ntitle: b\n---\n"),
        Err(DocumentError::InvalidFrontmatter)
    ));
}

#[test]
fn patching_unknown_field_is_an_error() {
    let mut document = parse_document("---\ntitle: x\n---\n").unwrap();
    assert_eq!(
        document.set_field("missing", FrontmatterValue::String("y".to_owned())),
        Err(DocumentError::UnknownField {
            path: "missing".to_owned()
        })
    );
}

#[test]
fn block_scalar_field_is_readable_but_not_patchable() {
    let mut document = parse_document("---\nbody: |\n  line one\n  line two\n---\n").unwrap();
    assert!(document.field("body").is_none());
    assert!(matches!(
        document.set_field("body", FrontmatterValue::String("x".to_owned())),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn trailing_newline_presence_is_preserved() {
    for source in ["plain text", "plain text\n", "plain text\r\n"] {
        assert_eq!(
            serialize_document(&parse_document(source).unwrap()).unwrap(),
            source
        );
    }
}

#[test]
fn patched_strings_that_look_like_other_types_are_quoted() {
    let mut document = parse_document("---\na: x\nb: y\n---\n").unwrap();
    document
        .set_field("a", FrontmatterValue::String("true".to_owned()))
        .unwrap();
    document
        .set_field("b", FrontmatterValue::String("123".to_owned()))
        .unwrap();
    let output = serialize_document(&document).unwrap();
    assert_eq!(output, "---\na: 'true'\nb: '123'\n---\n");
    let reparsed = parse_document(&output).unwrap();
    assert_eq!(
        reparsed.field("a"),
        Some(&FrontmatterValue::String("true".to_owned()))
    );
    assert_eq!(
        reparsed.field("b"),
        Some(&FrontmatterValue::String("123".to_owned()))
    );
}

#[test]
fn patched_string_with_line_break_is_refused() {
    let mut document = parse_document("---\na: x\n---\n").unwrap();
    assert!(matches!(
        document.set_field("a", FrontmatterValue::String("line1\nline2".to_owned())),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn bom_and_escaped_quotes_round_trip_exactly() {
    let source = "\u{feff}---\ntitle: \"a \\\"b\\\" c\"\n---\nbody";
    let document = parse_document(source).unwrap();
    assert_eq!(serialize_document(&document).unwrap(), source);
    assert_eq!(
        document.field("title"),
        Some(&FrontmatterValue::String("a \"b\" c".to_owned()))
    );
}

#[test]
fn two_level_nested_scalar_patch() {
    let mut document = parse_document("---\na:\n  b:\n    c: 1\n---\n").unwrap();
    document
        .set_field("a.b.c", FrontmatterValue::Integer(2))
        .unwrap();
    assert_eq!(
        serialize_document(&document).unwrap(),
        "---\na:\n  b:\n    c: 2\n---\n"
    );
}

#[test]
fn patched_string_ending_in_colon_is_quoted() {
    let mut document = parse_document("---\ntitle: x\n---\n").unwrap();
    document
        .set_field("title", FrontmatterValue::String("Chapter 1:".to_owned()))
        .unwrap();
    let output = serialize_document(&document).unwrap();
    assert_eq!(output, "---\ntitle: 'Chapter 1:'\n---\n");
    assert_eq!(
        parse_document(&output).unwrap().field("title"),
        Some(&FrontmatterValue::String("Chapter 1:".to_owned()))
    );
}

#[test]
fn opaque_flow_field_patch_is_refused() {
    let mut document = parse_document("---\ntags: [a, b]\n---\n").unwrap();
    assert!(matches!(
        document.set_field("tags", FrontmatterValue::String("x".to_owned())),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn null_leaf_is_readable_but_not_editable() {
    let document = parse_document("---\ntitle:\n---\n").unwrap();
    assert_eq!(document.field("title"), Some(&FrontmatterValue::Null));

    let mut editable = parse_document("---\ntitle:\n---\n").unwrap();
    assert!(matches!(
        editable.set_field("title", FrontmatterValue::String("x".to_owned())),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn closing_delimiter_trailing_spaces_recognized() {
    let source = "---\ntitle: x\n---   \nbody";
    let document = parse_document(source).unwrap();
    assert!(document.has_frontmatter());
    assert_eq!(serialize_document(&document).unwrap(), source);
}

#[test]
fn control_character_patch_is_refused() {
    let mut document = parse_document("---\ntitle: x\n---\n").unwrap();
    assert!(matches!(
        document.set_field("title", FrontmatterValue::String("a\u{1}b".to_owned())),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}
