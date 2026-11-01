use crate::buffer::DocumentError;
use crate::{
    FieldShape, FrontmatterValue, LineEnding, SequenceStyle, parse_document, serialize_document,
};

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
fn null_leaf_patch_inserts_the_value_before_a_trailing_comment() {
    let document = parse_document("---\ntitle:\n---\n").unwrap();
    assert_eq!(document.field("title"), Some(&FrontmatterValue::Null));

    let mut plain = parse_document("---\ntitle:\n---\n").unwrap();
    plain
        .set_field("title", FrontmatterValue::String("Chapter 1".to_owned()))
        .unwrap();
    let output = serialize_document(&plain).unwrap();
    assert_eq!(output, "---\ntitle: Chapter 1\n---\n");
    assert_eq!(
        parse_document(&output).unwrap().field("title"),
        Some(&FrontmatterValue::String("Chapter 1".to_owned()))
    );

    let mut commented = parse_document("---\ntitle:  # keep me\n---\n").unwrap();
    commented
        .set_field("title", FrontmatterValue::Integer(3))
        .unwrap();
    assert_eq!(
        serialize_document(&commented).unwrap(),
        "---\ntitle: 3  # keep me\n---\n"
    );
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

fn string(text: &str) -> FrontmatterValue {
    FrontmatterValue::String(text.to_owned())
}

#[test]
fn fields_expose_shapes_and_flow_raw_in_source_order() {
    let source = "---\ntitle: x\ntags: [a, b]\nauthor:\n  name: Ada\nbody: |\n  raw\n---\n";
    let document = parse_document(source).unwrap();
    let shapes: Vec<(&str, FieldShape)> = document
        .fields()
        .iter()
        .map(|field| (field.path.as_str(), field.shape))
        .collect();
    assert_eq!(
        shapes,
        vec![
            ("title", FieldShape::Scalar),
            ("tags", FieldShape::Sequence),
            ("author", FieldShape::Mapping),
            ("author.name", FieldShape::Scalar),
            ("body", FieldShape::BlockScalar),
        ]
    );
    let tags = document.field_info("tags").unwrap();
    assert_eq!(tags.raw.as_deref(), Some("[a, b]"));
    assert_eq!(tags.items, Some(vec![string("a"), string("b")]));
    assert_eq!(tags.sequence_style, Some(SequenceStyle::Flow));
    assert!(document.field("tags").is_none());
    assert!(document.field("author").is_none());
    assert!(document.has_field("author.name"));
    assert!(!document.has_field("missing"));
}

#[test]
fn block_sequence_items_are_readable() {
    let source = "---\ntags:\n  - one\n  - \"two\"  # note\nnested:\n  - key: 1\n---\n";
    let document = parse_document(source).unwrap();
    let tags = document.field_info("tags").unwrap();
    assert_eq!(tags.shape, FieldShape::Sequence);
    assert_eq!(tags.sequence_style, Some(SequenceStyle::Block));
    assert_eq!(tags.items, Some(vec![string("one"), string("two")]));
    assert_eq!(tags.raw.as_deref(), Some("- one\n- \"two\"  # note"));
    assert!(document.field("tags").is_none());
    let nested = document.field_info("nested").unwrap();
    assert_eq!(nested.shape, FieldShape::Sequence);
    assert_eq!(nested.items, None);
    assert_eq!(
        nested.raw.as_deref(),
        Some("- key: 1"),
        "a block sequence of mappings keeps its raw text for a raw editor"
    );
}

#[test]
fn flow_sequence_with_nested_items_is_not_item_editable() {
    let document = parse_document("---\na: [x, \"y\", 3]\nb: [{k: 1}]\n---\n").unwrap();
    let a = document.field_info("a").unwrap();
    assert_eq!(
        a.items,
        Some(vec![string("x"), string("y"), FrontmatterValue::Integer(3)])
    );
    assert_eq!(a.raw.as_deref(), Some("[x, \"y\", 3]"));
    let b = document.field_info("b").unwrap();
    assert_eq!(b.shape, FieldShape::Sequence);
    assert_eq!(b.items, None);
    assert_eq!(
        b.raw.as_deref(),
        Some("[{k: 1}]"),
        "an opaque flow sequence keeps its single-line raw text"
    );
}

#[test]
fn sequence_documents_round_trip_exactly() {
    let source = "---\r\ntags:\r\n  - one  # c\r\n  - \"two\"\r\nflow: [a, b]\r\n---\r\nBody\r\n";
    let document = parse_document(source).unwrap();
    assert_eq!(serialize_document(&document).unwrap(), source);
}

#[test]
fn insert_absent_top_level_field_preserves_bytes() {
    let source = "---\ntitle: x\ntags:\n  - a\n---\nbody\n";
    let mut document = parse_document(source).unwrap();
    document
        .insert_field("draft", FrontmatterValue::Boolean(false))
        .unwrap();
    assert_eq!(
        serialize_document(&document).unwrap(),
        "---\ntitle: x\ntags:\n  - a\ndraft: false\n---\nbody\n"
    );

    let mut crlf = parse_document("---\r\ntitle: x\r\n---\r\n").unwrap();
    crlf.insert_field("slug", string("s")).unwrap();
    assert_eq!(
        serialize_document(&crlf).unwrap(),
        "---\r\ntitle: x\r\nslug: s\r\n---\r\n"
    );
}

#[test]
fn insert_absent_sequence_field_is_lossless() {
    let mut document = parse_document("---\ntitle: x\n---\n").unwrap();
    document
        .insert_sequence("tags", &[string("a"), string("b c")])
        .unwrap();
    let output = serialize_document(&document).unwrap();
    assert_eq!(output, "---\ntitle: x\ntags: [a, b c]\n---\n");
    assert_eq!(
        parse_document(&output)
            .unwrap()
            .field_info("tags")
            .unwrap()
            .items,
        Some(vec![string("a"), string("b c")])
    );

    let mut empty = parse_document("---\ntitle: x\n---\n").unwrap();
    empty.insert_sequence("tags", &[]).unwrap();
    assert_eq!(
        serialize_document(&empty).unwrap(),
        "---\ntitle: x\ntags: []\n---\n"
    );

    let mut missing_parent = parse_document("---\ntitle: x\n---\n").unwrap();
    assert!(matches!(
        missing_parent.insert_sequence("author.tags", &[string("a")]),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn insert_absent_nested_field_uses_parent_indent() {
    let mut document = parse_document("---\nauthor:\n  name: Ada\n---\n").unwrap();
    document
        .insert_field("author.email", string("a@b"))
        .unwrap();
    assert_eq!(
        serialize_document(&document).unwrap(),
        "---\nauthor:\n  name: Ada\n  email: a@b\n---\n"
    );

    let mut duplicate = parse_document("---\nauthor:\n  name: Ada\n---\n").unwrap();
    assert!(matches!(
        duplicate.insert_field("author.name", string("x")),
        Err(DocumentError::UnknownField { .. })
    ));
}

#[test]
fn remove_scalar_nested_block_and_empty_parent() {
    let mut top = parse_document("---\ntitle: x\nauthor:\n  name: Ada\n---\n").unwrap();
    top.remove_field("title").unwrap();
    assert_eq!(
        serialize_document(&top).unwrap(),
        "---\nauthor:\n  name: Ada\n---\n"
    );

    let mut nested = parse_document("---\nauthor:\n  name: Ada\n  email: a@b\n---\n").unwrap();
    nested.remove_field("author.name").unwrap();
    assert_eq!(
        serialize_document(&nested).unwrap(),
        "---\nauthor:\n  email: a@b\n---\n"
    );

    let mut only = parse_document("---\nauthor:\n  name: Ada\n---\n").unwrap();
    only.remove_field("author.name").unwrap();
    assert_eq!(serialize_document(&only).unwrap(), "---\nauthor:\n---\n");

    let mut whole = parse_document("---\nauthor:\n  name: Ada\n---\n").unwrap();
    whole.remove_field("author").unwrap();
    assert_eq!(serialize_document(&whole).unwrap(), "---\n---\n");
}

const SEQ: &str = "---\ntags:\n  - one\n  - two  # keep\n  - 'three'\n---\n";

#[test]
fn block_sequence_insert_start_middle_and_end() {
    let mut start = parse_document(SEQ).unwrap();
    start
        .insert_sequence_item("tags", 0, string("zero"))
        .unwrap();
    assert_eq!(
        serialize_document(&start).unwrap(),
        "---\ntags:\n  - zero\n  - one\n  - two  # keep\n  - 'three'\n---\n"
    );

    let mut middle = parse_document(SEQ).unwrap();
    middle
        .insert_sequence_item("tags", 2, string("zero"))
        .unwrap();
    assert_eq!(
        serialize_document(&middle).unwrap(),
        "---\ntags:\n  - one\n  - two  # keep\n  - zero\n  - 'three'\n---\n"
    );

    let mut end = parse_document(SEQ).unwrap();
    end.insert_sequence_item("tags", 3, FrontmatterValue::Integer(4))
        .unwrap();
    assert_eq!(
        serialize_document(&end).unwrap(),
        "---\ntags:\n  - one\n  - two  # keep\n  - 'three'\n  - 4\n---\n"
    );

    let mut crlf = parse_document("---\r\ntags:\r\n  - a\r\n  - b\r\n---\r\n").unwrap();
    crlf.insert_sequence_item("tags", 2, string("c")).unwrap();
    assert_eq!(
        serialize_document(&crlf).unwrap(),
        "---\r\ntags:\r\n  - a\r\n  - b\r\n  - c\r\n---\r\n"
    );

    let mut out_of_range = parse_document(SEQ).unwrap();
    assert!(matches!(
        out_of_range.insert_sequence_item("tags", 4, string("x")),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn block_sequence_remove_and_set_preserve_other_items() {
    let mut middle = parse_document(SEQ).unwrap();
    middle.remove_sequence_item("tags", 1).unwrap();
    assert_eq!(
        serialize_document(&middle).unwrap(),
        "---\ntags:\n  - one\n  - 'three'\n---\n"
    );

    let mut last = parse_document(SEQ).unwrap();
    last.remove_sequence_item("tags", 2).unwrap();
    assert_eq!(
        serialize_document(&last).unwrap(),
        "---\ntags:\n  - one\n  - two  # keep\n---\n"
    );

    let mut set = parse_document(SEQ).unwrap();
    set.set_sequence_item("tags", 2, FrontmatterValue::Integer(4))
        .unwrap();
    assert_eq!(
        serialize_document(&set).unwrap(),
        "---\ntags:\n  - one\n  - two  # keep\n  - 4\n---\n"
    );

    let mut only = parse_document("---\ntags:\n  - one\nafter: x\n---\n").unwrap();
    only.remove_sequence_item("tags", 0).unwrap();
    assert_eq!(
        serialize_document(&only).unwrap(),
        "---\ntags: []\nafter: x\n---\n"
    );

    let mut following = parse_document("---\ntags:\n  - one\n  - two\nafter: x\n---\n").unwrap();
    following.remove_sequence_item("tags", 1).unwrap();
    assert_eq!(
        serialize_document(&following).unwrap(),
        "---\ntags:\n  - one\nafter: x\n---\n"
    );

    let mut out_of_range = parse_document(SEQ).unwrap();
    assert!(matches!(
        out_of_range.remove_sequence_item("tags", 3),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn validate_value_accepts_and_refuses_inline_yaml() {
    use crate::yaml::validate_value;
    for good in ["[a, b]", "{x: 1}", "\"quoted\"", "plain", "3"] {
        assert!(validate_value(good).is_ok(), "{good:?} must be valid");
    }
    for bad in ["", "[a", "a: [1]\nb: 2", "&a x", "*a", "!tag x", "|"] {
        assert!(validate_value(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn flow_sequence_edits_preserve_spacing() {
    let mut insert = parse_document("---\ntags: [a, \"b\", 3]\n---\n").unwrap();
    insert.insert_sequence_item("tags", 1, string("x")).unwrap();
    assert_eq!(
        serialize_document(&insert).unwrap(),
        "---\ntags: [a, x, \"b\", 3]\n---\n"
    );

    let mut append = parse_document("---\ntags: [a, \"b\"]\n---\n").unwrap();
    append.insert_sequence_item("tags", 2, string("c")).unwrap();
    assert_eq!(
        serialize_document(&append).unwrap(),
        "---\ntags: [a, \"b\", c]\n---\n"
    );

    let mut compact = parse_document("---\ntags: [a,b]\n---\n").unwrap();
    compact
        .insert_sequence_item("tags", 2, string("c"))
        .unwrap();
    assert_eq!(
        serialize_document(&compact).unwrap(),
        "---\ntags: [a,b,c]\n---\n"
    );

    let mut remove = parse_document("---\ntags: [a, \"b\", 3]\n---\n").unwrap();
    remove.remove_sequence_item("tags", 1).unwrap();
    assert_eq!(
        serialize_document(&remove).unwrap(),
        "---\ntags: [a, 3]\n---\n"
    );

    let mut remove_last = parse_document("---\ntags: [a, \"b\"]\n---\n").unwrap();
    remove_last.remove_sequence_item("tags", 1).unwrap();
    assert_eq!(
        serialize_document(&remove_last).unwrap(),
        "---\ntags: [a]\n---\n"
    );

    let mut remove_only = parse_document("---\ntags: [a]\n---\n").unwrap();
    remove_only.remove_sequence_item("tags", 0).unwrap();
    assert_eq!(
        serialize_document(&remove_only).unwrap(),
        "---\ntags: []\n---\n"
    );

    let mut empty = parse_document("---\ntags: []\n---\n").unwrap();
    empty.insert_sequence_item("tags", 0, string("x")).unwrap();
    assert_eq!(serialize_document(&empty).unwrap(), "---\ntags: [x]\n---\n");

    let mut set = parse_document("---\ntags: [a, \"b\"]\n---\n").unwrap();
    set.set_sequence_item("tags", 1, FrontmatterValue::Integer(7))
        .unwrap();
    assert_eq!(
        serialize_document(&set).unwrap(),
        "---\ntags: [a, 7]\n---\n"
    );
}

#[test]
fn replace_raw_validates_yaml_and_limits_shapes() {
    let mut document = parse_document("---\ntags: [a, b]\nmeta: {x: 1}\n---\n").unwrap();
    document.replace_raw("tags", "[x, \"y\"]").unwrap();
    document.replace_raw("meta", "{x: 2}").unwrap();
    let output = serialize_document(&document).unwrap();
    assert_eq!(output, "---\ntags: [x, \"y\"]\nmeta: {x: 2}\n---\n");
    assert_eq!(
        parse_document(&output)
            .unwrap()
            .field_info("tags")
            .unwrap()
            .items,
        Some(vec![string("x"), string("y")])
    );

    for bad in ["[a", "a: [1]\nb: 2", "&a x", "*a", "!tag x"] {
        let mut refused = parse_document("---\ntags: [a]\n---\n").unwrap();
        assert!(
            matches!(
                refused.replace_raw("tags", bad),
                Err(DocumentError::UnsupportedPatch { .. })
            ),
            "{bad:?} must be refused"
        );
    }

    let mut scalar = parse_document("---\ntitle: x\n---\n").unwrap();
    assert!(matches!(
        scalar.replace_raw("title", "y"),
        Err(DocumentError::UnsupportedPatch { .. })
    ));

    let mut block = parse_document("---\ntags:\n  - a\n---\n").unwrap();
    block.replace_raw("tags", "- b").unwrap();
    assert_eq!(
        serialize_document(&block).unwrap(),
        "---\ntags:\n  - b\n---\n"
    );

    let mut unknown = parse_document("---\ntags: [a]\n---\n").unwrap();
    assert!(matches!(
        unknown.replace_raw("missing", "[b]"),
        Err(DocumentError::UnknownField { .. })
    ));
}

#[test]
fn sequence_edits_nested_in_a_simple_mapping_preserve_comments_and_crlf() {
    let source =
        "---\r\nauthor:\r\n  name: Ada  # who\r\n  links:  # keep\r\n    - a\r\n    - b\r\n---\r\n";
    let mut insert = parse_document(source).unwrap();
    insert
        .insert_sequence_item("author.links", 2, string("c"))
        .unwrap();
    assert_eq!(
        serialize_document(&insert).unwrap(),
        "---\r\nauthor:\r\n  name: Ada  # who\r\n  links:  # keep\r\n    - a\r\n    - b\r\n    - c\r\n---\r\n"
    );

    let mut remove = parse_document(source).unwrap();
    remove.remove_sequence_item("author.links", 0).unwrap();
    assert_eq!(
        serialize_document(&remove).unwrap(),
        "---\r\nauthor:\r\n  name: Ada  # who\r\n  links:  # keep\r\n    - b\r\n---\r\n"
    );

    let mut set = parse_document(source).unwrap();
    set.set_sequence_item("author.links", 1, string("B"))
        .unwrap();
    assert_eq!(
        serialize_document(&set).unwrap(),
        "---\r\nauthor:\r\n  name: Ada  # who\r\n  links:  # keep\r\n    - a\r\n    - B\r\n---\r\n"
    );
}

#[test]
fn replace_raw_preserves_a_trailing_comment() {
    let mut flow = parse_document("---\nmeta: {x: 1}  # keep\n---\n").unwrap();
    flow.replace_raw("meta", "{x: 2}").unwrap();
    assert_eq!(
        serialize_document(&flow).unwrap(),
        "---\nmeta: {x: 2}  # keep\n---\n"
    );
}

#[test]
fn sequence_item_edits_refuse_non_scalar_items() {
    let mut block = parse_document("---\nnested:\n  - key: 1\n---\n").unwrap();
    assert_eq!(block.field_info("nested").unwrap().items, None);
    assert!(matches!(
        block.set_sequence_item("nested", 0, string("x")),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
    assert!(matches!(
        block.insert_sequence_item("nested", 0, string("x")),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
    assert!(matches!(
        block.remove_sequence_item("nested", 0),
        Err(DocumentError::UnsupportedPatch { .. })
    ));

    let mut flow = parse_document("---\nb: [{k: 1}]\n---\n").unwrap();
    assert!(matches!(
        flow.insert_sequence_item("b", 0, string("x")),
        Err(DocumentError::UnsupportedPatch { .. })
    ));

    let mut scalar = parse_document("---\ntitle: x\n---\n").unwrap();
    assert!(matches!(
        scalar.set_sequence_item("title", 0, string("x")),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn null_list_fields_can_gain_their_first_item() {
    for source in ["---\ntags:\n---\n", "---\ntags: null\n---\n"] {
        let mut document = parse_document(source).unwrap();
        document
            .insert_sequence_item("tags", 0, string("a,b"))
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        let output = serialize_document(&document).unwrap();
        assert_eq!(output, "---\ntags: ['a,b']\n---\n", "{source:?}");

        // A whole-sequence insert on the same null value is equally valid.
        let mut whole = parse_document(source).unwrap();
        whole
            .insert_sequence("tags", &[string("x"), string("y")])
            .unwrap_or_else(|error| panic!("{source:?}: {error}"));
        assert_eq!(
            serialize_document(&whole).unwrap(),
            "---\ntags: [x, y]\n---\n",
            "{source:?}"
        );
    }
}

#[test]
fn flow_items_with_structure_characters_round_trip_as_one_item() {
    // Absent field: every value must survive as exactly one item.
    let mut absent = parse_document("---\ntitle: x\n---\n").unwrap();
    absent
        .insert_sequence(
            "tags",
            &[
                string("a,b"),
                string(" leading"),
                string("[x]"),
                string("#y"),
            ],
        )
        .unwrap();
    let output = serialize_document(&absent).unwrap();
    assert_eq!(
        parse_document(&output)
            .unwrap()
            .field_info("tags")
            .unwrap()
            .items,
        Some(vec![
            string("a,b"),
            string(" leading"),
            string("[x]"),
            string("#y")
        ]),
        "a flow item carrying structure characters must not split or vanish"
    );

    // Empty flow sequence.
    let mut empty = parse_document("---\ntags: []\n---\n").unwrap();
    empty
        .insert_sequence_item("tags", 0, string("a,b"))
        .unwrap();
    let output = serialize_document(&empty).unwrap();
    assert_eq!(output, "---\ntags: ['a,b']\n---\n");

    // Middle and end of an existing flow sequence.
    let mut middle = parse_document("---\ntags: [x, y]\n---\n").unwrap();
    middle
        .insert_sequence_item("tags", 1, string("a,b"))
        .unwrap();
    let output = serialize_document(&middle).unwrap();
    assert_eq!(
        parse_document(&output)
            .unwrap()
            .field_info("tags")
            .unwrap()
            .items,
        Some(vec![string("x"), string("a,b"), string("y")])
    );

    let mut end = parse_document("---\ntags: [x]\n---\n").unwrap();
    end.insert_sequence_item("tags", 1, string("a,b")).unwrap();
    assert_eq!(
        serialize_document(&end).unwrap(),
        "---\ntags: [x, 'a,b']\n---\n"
    );

    // Replacing one flow item keeps it a single item.
    let mut set = parse_document("---\ntags: [x, y]\n---\n").unwrap();
    set.set_sequence_item("tags", 1, string("a,b")).unwrap();
    assert_eq!(
        serialize_document(&set).unwrap(),
        "---\ntags: [x, 'a,b']\n---\n"
    );
}

#[test]
fn insert_creates_a_frontmatter_block_when_the_document_has_none() {
    let mut empty = parse_document("").unwrap();
    empty
        .insert_field("draft", FrontmatterValue::Boolean(true))
        .unwrap();
    assert_eq!(
        serialize_document(&empty).unwrap(),
        "---\ndraft: true\n---\n\n"
    );

    let source = "# Title\n\nBody without frontmatter\n";
    let mut body = parse_document(source).unwrap();
    body.insert_sequence("tags", &[string("a")]).unwrap();
    body.insert_field("draft", FrontmatterValue::Boolean(false))
        .unwrap();
    let output = serialize_document(&body).unwrap();
    assert_eq!(
        output,
        format!("---\ntags: [a]\ndraft: false\n---\n\n{source}")
    );
    assert_eq!(
        parse_document(&output).unwrap().body(),
        format!("\n{source}"),
        "the body bytes after the inserted block are untouched"
    );

    let mut bom = parse_document("\u{feff}Body\n").unwrap();
    bom.insert_field("draft", FrontmatterValue::Boolean(true))
        .unwrap();
    assert_eq!(
        serialize_document(&bom).unwrap(),
        "\u{feff}---\ndraft: true\n---\n\nBody\n",
        "the byte-order mark stays first"
    );

    let mut crlf = parse_document("Body\r\n").unwrap();
    crlf.insert_field("draft", FrontmatterValue::Boolean(true))
        .unwrap();
    assert_eq!(
        serialize_document(&crlf).unwrap(),
        "---\r\ndraft: true\r\n---\r\n\r\nBody\r\n"
    );

    let mut duplicate = parse_document("Body\n").unwrap();
    duplicate
        .insert_field("draft", FrontmatterValue::Boolean(true))
        .unwrap();
    assert!(matches!(
        duplicate.insert_field("draft", FrontmatterValue::Boolean(false)),
        Err(DocumentError::UnknownField { .. })
    ));
    assert!(matches!(
        duplicate.insert_field("author.name", string("Ada")),
        Err(DocumentError::UnsupportedPatch { .. })
    ));
}

#[test]
fn replace_raw_handles_block_containers() {
    let source = "---\nrows:\n  - 1\n  - 2\nmeta:\n  a: 1\n  b: 2\nnote: x\n---\n";
    let mut document = parse_document(source).unwrap();
    document.replace_raw("rows", "- 3\n- 4\n- 5").unwrap();
    document.replace_raw("meta", "c: 3").unwrap();
    assert_eq!(
        serialize_document(&document).unwrap(),
        "---\nrows:\n  - 3\n  - 4\n  - 5\nmeta:\n  c: 3\nnote: x\n---\n"
    );

    // Invalid block raw is refused and never changes the bytes.
    let mut refused = parse_document(source).unwrap();
    for bad in ["", "  ", "&anchor x", "|", "- 1\nkey: 2"] {
        assert!(
            matches!(
                refused.replace_raw("rows", bad),
                Err(DocumentError::UnsupportedPatch { .. })
            ),
            "{bad:?} must be refused"
        );
    }
    assert_eq!(serialize_document(&refused).unwrap(), source);

    // A block mapping in a scalar-typed field is raw-editable too.
    let mut mapping = parse_document("---\nnote:\n  a: 1\n---\n").unwrap();
    mapping.replace_raw("note", "a: 2\nb: 3").unwrap();
    assert_eq!(
        serialize_document(&mapping).unwrap(),
        "---\nnote:\n  a: 2\n  b: 3\n---\n"
    );

    // Reindented input and nested structures keep their relative shape.
    let mut nested = parse_document("---\nrows:\n  - name: a\n  - name: b\n---\n").unwrap();
    nested
        .replace_raw("rows", "  - name: c\n    extra: 1\n")
        .unwrap();
    assert_eq!(
        serialize_document(&nested).unwrap(),
        "---\nrows:\n  - name: c\n    extra: 1\n---\n"
    );
}

#[test]
fn sequence_refusals_are_preserved() {
    assert!(matches!(
        parse_document("---\n\"title\": x\n---\n"),
        Err(DocumentError::UnsupportedFrontmatter { .. })
    ));
    for source in [
        "---\ntags: [a,\n  b]\n---\n",
        "---\nauthor:\n\tname: x\n---\n",
        "---\n- a\n---\n",
        "---\na: 1\n  - b\n---\n",
    ] {
        assert!(
            parse_document(source).is_err(),
            "{source:?} must be refused"
        );
    }
}

// ---------------------------------------------------------------------------
// Writer projection
// ---------------------------------------------------------------------------

use crate::projection::{
    DocumentKind, ProjectionError, ProjectionSegment, TextEdit, TextReplacement, WriterProjection,
    apply_visible_edit, diff_visible, map_visible_offset, project_writer,
};

const FRONTMATTER: &str = "---\ntitle: \"Exact\" # keep\n---\n";

fn hidden_bytes(source: &str, projection: &WriterProjection) -> Vec<String> {
    projection
        .segments
        .iter()
        .filter_map(|segment| match segment {
            ProjectionSegment::Hidden { source: range } => Some(source[range.clone()].to_owned()),
            ProjectionSegment::Visible { .. } => None,
        })
        .collect()
}

#[test]
fn markdown_hides_only_the_exact_leading_frontmatter() {
    let source = format!("{FRONTMATTER}\n# Title\n\nBody\n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    assert_eq!(projection.visible, "# Title\n\nBody\n");
    assert_eq!(projection.segments.len(), 2);
    assert_eq!(
        projection.segments[0],
        ProjectionSegment::Hidden {
            source: 0..FRONTMATTER.len() + 1
        }
    );
    assert_eq!(
        projection.segments[1],
        ProjectionSegment::Visible {
            source: FRONTMATTER.len() + 1..source.len(),
            visible: 0..projection.visible.len(),
        }
    );
    // An unchanged projection round-trips the whole source exactly.
    let rebuilt = apply_visible_edit(&source, &projection, TextEdit::default()).unwrap();
    assert_eq!(rebuilt, source);
    assert_eq!(
        hidden_bytes(&source, &projection),
        [format!("{FRONTMATTER}\n")]
    );
}

#[test]
fn projection_edges_stay_visible_or_append_after_the_block() {
    let plain = "# Body\n";
    let projection = project_writer(plain, DocumentKind::Markdown).unwrap();
    assert_eq!(projection.visible, plain);
    assert_eq!(
        projection.segments,
        [ProjectionSegment::Visible {
            source: 0..plain.len(),
            visible: 0..plain.len(),
        }]
    );

    let empty = project_writer("", DocumentKind::Markdown).unwrap();
    assert_eq!(empty.visible, "");
    assert!(empty.segments.is_empty());
    assert_eq!(
        apply_visible_edit("", &empty, TextEdit::replace(0..0, "# New\n")).unwrap(),
        "# New\n"
    );

    // A frontmatter-only file shows nothing; typing appends after the block.
    let only = project_writer(FRONTMATTER, DocumentKind::Markdown).unwrap();
    assert_eq!(only.visible, "");
    assert_eq!(only.source_len(), FRONTMATTER.len());
    assert_eq!(
        apply_visible_edit(FRONTMATTER, &only, TextEdit::replace(0..0, "\n# Body\n")).unwrap(),
        format!("{FRONTMATTER}\n# Body\n")
    );
    // A leading delimiter line is not frontmatter.
    let dashed = project_writer("---x\nbody\n", DocumentKind::Markdown).unwrap();
    assert_eq!(dashed.visible, "---x\nbody\n");
    // Non-Markdown sources never hide anything.
    let plain_text = project_writer(FRONTMATTER, DocumentKind::Plain).unwrap();
    assert_eq!(plain_text.visible, FRONTMATTER);
}

#[test]
fn malformed_frontmatter_is_reported_and_never_hidden() {
    for source in ["---\ntitle: x\n", "---", "---\r\n"] {
        assert_eq!(
            project_writer(source, DocumentKind::Markdown),
            Err(ProjectionError::MalformedFrontmatter),
            "{source:?}"
        );
    }
}

#[test]
fn blank_separators_after_metadata_are_hidden_and_only_ascii_blanks_count() {
    let frontmatter = "---\ntitle: x\n---\n";

    // The separator run is hidden even when it ends the file, so the writer never
    // shows a trailing empty line that belongs to the metadata lead.
    let source = format!("{frontmatter}\n \n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    assert_eq!(projection.visible, "");
    assert_eq!(hidden_bytes(&source, &projection), [source]);

    // Only ASCII space, tab and CR are blank: whitespace a user authored stays.
    for authored in ["\u{a0}", "\u{3000}"] {
        let source = format!("{frontmatter}\n{authored}\n# Body\n");
        let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
        assert_eq!(projection.visible, format!("{authored}\n# Body\n"));
    }

    // Indentation of the first content line is preserved.
    let source = format!("{frontmatter}\n\n  # Body\n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    assert_eq!(projection.visible, "  # Body\n");
}

#[test]
fn unicode_and_crlf_map_byte_exactly() {
    let frontmatter = "---\r\ntitle: \"Exact\" # 日本語\r\n---\r\n";
    let source = format!("{frontmatter}\r\n# 見出し 🦀\r\n\r\n本文 e\u{301}\r\n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    assert_eq!(projection.visible, "# 見出し 🦀\r\n\r\n本文 e\u{301}\r\n");
    assert_eq!(
        hidden_bytes(&source, &projection),
        [format!("{frontmatter}\r\n")]
    );

    let body = projection.visible.find("本文").unwrap();
    let edit = TextEdit::replace(body..body + "本文".len(), "変更 🦀");
    let updated = apply_visible_edit(&source, &projection, edit).unwrap();
    assert!(updated.starts_with(frontmatter));
    assert_eq!(
        updated,
        format!("{frontmatter}\r\n# 見出し 🦀\r\n\r\n変更 🦀 e\u{301}\r\n")
    );

    // Multi-byte characters are never split: the same edit at a char interior fails.
    let mid = body + 1;
    assert_eq!(
        apply_visible_edit(&source, &projection, TextEdit::replace(mid..mid + 1, "x")),
        Err(ProjectionError::InvalidRange {
            start: mid,
            end: mid + 1
        })
    );
}

#[test]
fn visible_offsets_map_monotonically_into_the_source() {
    let source = format!("{FRONTMATTER}\n# Title 🦀\n\nbody\n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    let marker = "\u{00a7}";
    let mut previous = 0;
    for offset in 0..=projection.visible.len() {
        if !projection.visible.is_char_boundary(offset) {
            continue;
        }
        let marked = apply_visible_edit(
            &source,
            &projection,
            TextEdit::replace(offset..offset, marker),
        )
        .unwrap();
        let mapped = marked.find(marker).unwrap();
        assert_eq!(mapped, FRONTMATTER.len() + 1 + offset, "{offset}");
        assert!(mapped >= previous, "{offset} went backwards");
        assert!(marked.is_char_boundary(mapped + marker.len()));
        previous = mapped;
    }
}

#[test]
fn caret_offsets_map_through_a_projection_rebuild() {
    // A frontmatter block created at the top is hidden together with the blank
    // line that separates it from the body, so the caret keeps its text.
    let before = "\n# Title\n\nBody text\n";
    let after = "# Title\n\nBody text\n";
    let caret = before.find("Body").unwrap();
    assert_eq!(map_visible_offset(before, after, caret), caret - 1);
    assert_eq!(map_visible_offset(after, after, caret - 1), caret - 1);
    // A caret inside replaced text collapses to the end of the replacement.
    assert_eq!(
        map_visible_offset(after, &after.replace("Body", ""), caret),
        caret - 1
    );
    assert_eq!(map_visible_offset(after, "", caret - 1), 0);
}

#[test]
fn mdx_hides_the_tree_sitter_proven_esm_prelude() {
    let prelude = "import Callout from \"@components/Callout.astro\";\n\
                   import { formatLabel as label } from \"@lib/format.ts\";\n\
                   \n\
                   export const importedLabel = label(\"Top-level   export\");\n";
    let source = format!(
        "{FRONTMATTER}\n{prelude}\n# Imported components\n\n<Callout>{{importedLabel}}</Callout>\n"
    );
    let projection = project_writer(&source, DocumentKind::Mdx).unwrap();
    assert_eq!(
        projection.visible,
        "# Imported components\n\n<Callout>{importedLabel}</Callout>\n"
    );
    let hidden = projection.segments[0].clone();
    let ProjectionSegment::Hidden { source: range } = hidden else {
        panic!("the MDX prelude must be hidden");
    };
    assert_eq!(range.start, 0);
    assert!(range.end > FRONTMATTER.len());
    let hidden_text = &source[range];
    assert!(hidden_text.contains("import Callout"));
    assert!(hidden_text.contains("export const importedLabel"));
    assert_eq!(hidden_bytes(&source, &projection).len(), 1);

    // Markdown hides only the frontmatter, so the same imports stay visible.
    let markdown = project_writer(&source, DocumentKind::Markdown).unwrap();
    assert!(markdown.visible.contains("import Callout"));
}

#[test]
fn ambiguous_mdx_prelude_syntax_stays_visible() {
    for body in [
        "import {\n  Callout,\n\n# Body\n",
        "imported prose that only looks like an import\n\n# Body\n",
        "  import Callout from \"@components/Callout.astro\";\n\n# Body\n",
        "// a comment\nimport Callout from \"@components/Callout.astro\";\n\n# Body\n",
        "```mdx\nimport Callout from \"./Callout.astro\";\n```\n",
        "# Body\n\nimport Callout from \"./Callout.astro\";\n",
        "export\n\n# Body\n",
    ] {
        let source = format!("{FRONTMATTER}\n{body}");
        let projection = project_writer(&source, DocumentKind::Mdx).unwrap();
        assert_eq!(projection.visible, body, "{body:?} must stay visible");
    }

    // A statement that becomes provably complete later does not retroactively hide
    // the ambiguous prefix: only the leading proven statements are hidden.
    let source = format!("{FRONTMATTER}\nimport A from \"./A.astro\";\n\nexport\n\n# Body\n");
    let projection = project_writer(&source, DocumentKind::Mdx).unwrap();
    assert_eq!(projection.visible, "export\n\n# Body\n");
    assert!(!projection.visible.contains("import A"));
}

#[test]
fn replace_all_batches_apply_right_to_left_against_original_offsets() {
    let source = format!("{FRONTMATTER}alpha alpha alpha\n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    let matches = projection
        .visible
        .match_indices("alpha")
        .map(|(start, _)| TextReplacement::new(start..start + 5, "beta"))
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 3);
    let updated = apply_visible_edit(
        &source,
        &projection,
        TextEdit {
            replacements: matches,
        },
    )
    .unwrap();
    assert_eq!(updated, format!("{FRONTMATTER}beta beta beta\n"));

    // Offsets are read against the pre-edit text: a stale second pass is refused.
    assert_eq!(
        apply_visible_edit(
            &source,
            &projection,
            TextEdit {
                replacements: vec![
                    TextReplacement::new(5..10, "x"),
                    TextReplacement::new(0..9, "y"),
                ],
            }
        ),
        Err(ProjectionError::OverlappingEdits)
    );

    // Right-to-left order is produced even when the caller supplies the reverse.
    let mut replacements = projection
        .visible
        .match_indices("alpha")
        .map(|(start, _)| TextReplacement::new(start..start + 5, "Z"))
        .collect::<Vec<_>>();
    replacements.reverse();
    assert_eq!(
        apply_visible_edit(&source, &projection, TextEdit { replacements }).unwrap(),
        format!("{FRONTMATTER}Z Z Z\n")
    );
}

#[test]
fn edits_touching_hidden_bytes_are_refused() {
    // A synthetic interleaved layout cannot come from `project_writer`, but the
    // mapping contract must hold for any projection it is handed.
    let source = "HHabcHHdef";
    let projection = WriterProjection {
        visible: "abcdef".to_owned(),
        segments: vec![
            ProjectionSegment::Hidden { source: 0..2 },
            ProjectionSegment::Visible {
                source: 2..5,
                visible: 0..3,
            },
            ProjectionSegment::Hidden { source: 5..7 },
            ProjectionSegment::Visible {
                source: 7..10,
                visible: 3..6,
            },
        ],
    };
    // Straddling a hidden run, and the ambiguous insertion on the shared edge.
    for range in [2..4, 3..3] {
        assert_eq!(
            apply_visible_edit(source, &projection, TextEdit::replace(range.clone(), "x")),
            Err(ProjectionError::HiddenBoundary {
                start: range.start,
                end: range.end
            }),
            "{range:?}"
        );
    }
    // Out-of-range and inverted requests are refused before any mapping.
    let (inverted_start, inverted_end) = (6, 5);
    for range in [0..7, inverted_start..inverted_end] {
        assert!(matches!(
            apply_visible_edit(source, &projection, TextEdit::replace(range, "x")),
            Err(ProjectionError::InvalidRange { .. })
        ));
    }
    // Edits that stay inside one visible run map through the hidden bytes.
    assert_eq!(
        apply_visible_edit(source, &projection, TextEdit::replace(0..1, "<")).unwrap(),
        "HH<bcHHdef"
    );
    assert_eq!(
        apply_visible_edit(source, &projection, TextEdit::replace(3..4, "D")).unwrap(),
        "HHabcHHDef"
    );
    assert_eq!(
        apply_visible_edit(source, &projection, TextEdit::replace(6..6, ">")).unwrap(),
        "HHabcHHdef>"
    );
}

#[test]
fn renew_visible_keeps_hidden_bytes_fixed() {
    let source = format!("{FRONTMATTER}\n# Title\n");
    let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
    let renewed = projection
        .renew_visible("# Changed 🦀\n\nBody\n".to_owned())
        .unwrap();
    assert_eq!(
        renewed.segments[0],
        ProjectionSegment::Hidden {
            source: 0..FRONTMATTER.len() + 1
        }
    );
    assert_eq!(renewed.segments.len(), 2);
    assert_eq!(
        renewed.source_len(),
        FRONTMATTER.len() + 1 + renewed.visible.len()
    );
    assert_eq!(
        apply_visible_edit(
            &format!("{FRONTMATTER}\n{}", renewed.visible),
            &renewed,
            TextEdit::replace(0..0, ">")
        )
        .unwrap(),
        format!("{FRONTMATTER}\n># Changed 🦀\n\nBody\n")
    );

    let interleaved = WriterProjection {
        visible: "v".to_owned(),
        segments: vec![
            ProjectionSegment::Hidden { source: 0..1 },
            ProjectionSegment::Visible {
                source: 1..2,
                visible: 0..1,
            },
            ProjectionSegment::Hidden { source: 2..3 },
        ],
    };
    assert_eq!(
        interleaved.renew_visible("w".to_owned()),
        Err(ProjectionError::UnrenewableLayout)
    );
}

#[test]
fn visible_diff_trims_to_one_replacement() {
    assert!(diff_visible("", "").is_empty());
    assert!(diff_visible("# Body\n", "# Body\n").is_empty());

    for (previous, next) in [
        ("# Body\n", "# BodX\n"),
        ("# Body\n", "🦀# Body\n"),
        ("# 見出し\n", "# 見出X\n"),
        ("abc", "ab"),
        ("ab", "abc"),
        ("", "中"),
        ("中", ""),
    ] {
        let edit = diff_visible(previous, next);
        assert_eq!(edit.replacements.len(), 1, "{previous:?} -> {next:?}");
        let TextReplacement { range, replacement } = &edit.replacements[0];
        assert!(previous.is_char_boundary(range.start));
        assert!(previous.is_char_boundary(range.end));
        assert_eq!(
            &next[range.start..range.start + replacement.len()],
            replacement
        );
        let mut rebuilt = previous.to_owned();
        rebuilt.replace_range(range.clone(), replacement);
        assert_eq!(rebuilt, next);
    }
}

// Diagnostic only: bounded CPU measurements of the projection diff, not native
// input-to-present latency. Run with:
//   cargo test -p astrolith-document --locked projection_diff_workload -- --ignored --nocapture
//   cargo test -p astrolith-document --release --locked projection_diff_workload -- --ignored --nocapture
#[test]
#[ignore = "bounded workload measurements"]
fn projection_diff_workload() {
    use std::time::Instant;

    let frontmatter = "---\ntitle: Workload\ndraft: false\n---\n";
    for (bytes, long_line) in [
        (1024usize, false),
        (100 * 1024, false),
        (1024 * 1024, false),
        (100 * 1024, true),
    ] {
        let unit = if long_line {
            "中文日本語🦀 **bold** `code` "
        } else {
            "# 中文 🦀\n\nA multilingual paragraph with **emphasis**, `code`, 日本語 and 中文.\n"
        };
        let body = unit.repeat(bytes.div_ceil(unit.len()));
        let source = format!("{frontmatter}{body}");
        let projection = project_writer(&source, DocumentKind::Markdown).unwrap();
        let mut samples: Vec<f64> = Vec::with_capacity(60);
        for sample in 0..60 {
            // Ordinary typing: append one character, then insert one at the start.
            let offset = if sample % 2 == 0 {
                projection.visible.len()
            } else {
                0
            };
            let mut next = projection.visible.clone();
            next.insert(offset, '中');
            let start = Instant::now();
            let edit = diff_visible(&projection.visible, &next);
            let updated = apply_visible_edit(&source, &projection, edit).unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1000.);
            assert!(updated.starts_with(frontmatter));
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "PROJECTION_DIFF bytes={} body_bytes={} lines={} long_line={long_line} samples=60 p50_ms={:.4} p95_ms={:.4} max_ms={:.4}",
            source.len(),
            projection.visible.len(),
            projection.visible.lines().count(),
            samples[29],
            samples[56],
            samples[59],
        );
    }
}
