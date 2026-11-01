use serde_json::{Value, json};

use super::{
    CollectionSchema, Field, FieldConstraints, FieldKind, SchemaDialect, ValidationDiagnostic,
    ValidationSeverity, ValidationSource, validate_frontmatter,
};

fn schema(fields: Vec<Field>) -> CollectionSchema {
    CollectionSchema {
        dialect: SchemaDialect::Draft202012,
        fields,
        diagnostics: Vec::new(),
    }
}

fn field(name: &str, required: bool, kind: FieldKind) -> Field {
    Field {
        name: name.to_owned(),
        required,
        default: None,
        kind,
        description: None,
        constraints: FieldConstraints::default(),
        order: 0,
    }
}

fn paths(diagnostics: &[ValidationDiagnostic]) -> Vec<&str> {
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic.path.as_str())
        .collect()
}

#[test]
fn missing_required_field_is_reported_and_optional_absence_is_allowed() {
    let schema = schema(vec![
        field("title", true, FieldKind::String { format: None }),
        field("subtitle", false, FieldKind::String { format: None }),
    ]);

    let diagnostics = validate_frontmatter(&schema, &json!({}));

    assert_eq!(paths(&diagnostics), ["title"]);
    assert_eq!(diagnostics[0].severity, ValidationSeverity::Error);
    assert_eq!(diagnostics[0].source, ValidationSource::Local);
}

#[test]
fn scalar_type_mismatches_are_reported_by_path() {
    let schema = schema(vec![
        field("name", true, FieldKind::String { format: None }),
        field("score", true, FieldKind::Number),
        field("count", true, FieldKind::Integer),
        field("draft", true, FieldKind::Boolean),
    ]);

    let diagnostics = validate_frontmatter(
        &schema,
        &json!({ "name": 1, "score": "high", "count": 1.5, "draft": "no" }),
    );

    assert_eq!(paths(&diagnostics), ["count", "draft", "name", "score"]);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity == ValidationSeverity::Error)
    );
}

#[test]
fn valid_scalars_pass() {
    let schema = schema(vec![
        field("name", true, FieldKind::String { format: None }),
        field("score", true, FieldKind::Number),
        field("count", true, FieldKind::Integer),
        field("draft", true, FieldKind::Boolean),
    ]);

    let diagnostics = validate_frontmatter(
        &schema,
        &json!({ "name": "ok", "score": 1.5, "count": 3, "draft": true }),
    );

    assert!(diagnostics.is_empty(), "unexpected: {diagnostics:?}");
}

#[test]
fn enum_and_literal_must_match() {
    let schema = schema(vec![
        field(
            "category",
            true,
            FieldKind::Enum {
                values: vec![json!("news"), json!("blog")],
            },
        ),
        field(
            "version",
            true,
            FieldKind::Literal {
                value: json!("fixed"),
            },
        ),
    ]);

    let diagnostics =
        validate_frontmatter(&schema, &json!({ "category": "docs", "version": "other" }));

    assert_eq!(paths(&diagnostics), ["category", "version"]);
    assert!(diagnostics[0].message.contains("expected one of"));
    assert!(diagnostics[1].message.contains("expected the literal"));
}

#[test]
fn nested_objects_are_validated_recursively() {
    let schema = schema(vec![field(
        "author",
        true,
        FieldKind::Object {
            fields: vec![
                field("name", true, FieldKind::String { format: None }),
                field(
                    "email",
                    false,
                    FieldKind::String {
                        format: Some("email".to_owned()),
                    },
                ),
            ],
        },
    )]);

    let missing = validate_frontmatter(&schema, &json!({ "author": {} }));
    assert_eq!(paths(&missing), ["author.name"]);

    let wrong = validate_frontmatter(
        &schema,
        &json!({ "author": { "name": "A", "email": "not-an-email" } }),
    );
    assert_eq!(paths(&wrong), ["author.email"]);
    assert!(wrong[0].message.contains("email"));

    let valid = validate_frontmatter(
        &schema,
        &json!({ "author": { "name": "A", "email": "a@example.com" } }),
    );
    assert!(valid.is_empty(), "unexpected: {valid:?}");
}

#[test]
fn arrays_report_indexed_item_paths() {
    let schema = schema(vec![field(
        "tags",
        true,
        FieldKind::Array {
            items: Box::new(FieldKind::String { format: None }),
        },
    )]);

    let diagnostics = validate_frontmatter(&schema, &json!({ "tags": ["ok", 2, false] }));

    assert_eq!(paths(&diagnostics), ["tags[1]", "tags[2]"]);
}

#[test]
fn records_validate_each_entry_value() {
    let schema = schema(vec![field(
        "weights",
        true,
        FieldKind::Record {
            value: Box::new(FieldKind::Number),
        },
    )]);

    let diagnostics =
        validate_frontmatter(&schema, &json!({ "weights": { "a": 1, "b": "heavy" } }));

    assert_eq!(paths(&diagnostics), ["weights.b"]);
}

#[test]
fn nullable_fields_accept_null_but_other_fields_reject_it() {
    let schema = schema(vec![
        field(
            "maybe",
            true,
            FieldKind::Nullable {
                inner: Box::new(FieldKind::Number),
            },
        ),
        field("required_text", true, FieldKind::String { format: None }),
    ]);

    let nullable_ok =
        validate_frontmatter(&schema, &json!({ "maybe": null, "required_text": "x" }));
    assert!(nullable_ok.is_empty(), "unexpected: {nullable_ok:?}");

    let null_rejected =
        validate_frontmatter(&schema, &json!({ "maybe": null, "required_text": null }));
    assert_eq!(paths(&null_rejected), ["required_text"]);
    assert!(null_rejected[0].message.contains("null is not allowed"));

    let wrong_inner =
        validate_frontmatter(&schema, &json!({ "maybe": "text", "required_text": "x" }));
    assert_eq!(paths(&wrong_inner), ["maybe"]);
}

#[test]
fn dates_accept_date_strings_and_unix_integers_only() {
    let schema = schema(vec![field("when", true, FieldKind::Date)]);

    assert!(validate_frontmatter(&schema, &json!({ "when": "2024-02-03" })).is_empty());
    assert!(
        validate_frontmatter(&schema, &json!({ "when": "2024-02-03T10:00:00.000Z" })).is_empty()
    );
    assert!(validate_frontmatter(&schema, &json!({ "when": 1_700_000_000 })).is_empty());
    let diagnostics = validate_frontmatter(&schema, &json!({ "when": "not-a-date" }));
    assert_eq!(paths(&diagnostics), ["when"]);
}

#[test]
fn image_and_reference_fields_require_entry_strings() {
    let schema = schema(vec![
        field("cover", true, FieldKind::Image),
        field(
            "author",
            true,
            FieldKind::Reference {
                collections: vec!["authors".to_owned()],
            },
        ),
    ]);

    let valid = validate_frontmatter(
        &schema,
        &json!({ "cover": "./images/a.png", "author": "jane" }),
    );
    assert!(valid.is_empty(), "unexpected: {valid:?}");

    let object_reference = validate_frontmatter(
        &schema,
        &json!({ "cover": "./images/a.png", "author": { "id": "jane" } }),
    );
    assert!(
        object_reference.is_empty(),
        "unexpected: {object_reference:?}"
    );

    let wrong = validate_frontmatter(&schema, &json!({ "cover": 3, "author": 7 }));
    assert_eq!(paths(&wrong), ["author", "cover"]);
    assert!(wrong[1].message.contains("image path"));
}

#[test]
fn unsupported_fields_are_warnings_not_silent_passes() {
    let schema = schema(vec![field(
        "anything",
        true,
        FieldKind::Unsupported {
            reason: "union of string | number is not supported for editing".to_owned(),
        },
    )]);

    let diagnostics = validate_frontmatter(&schema, &json!({ "anything": "x" }));

    assert_eq!(paths(&diagnostics), ["anything"]);
    assert_eq!(diagnostics[0].severity, ValidationSeverity::Warning);
    assert!(
        diagnostics[0]
            .message
            .contains("cannot validate unsupported field")
    );
}

#[test]
fn non_mapping_frontmatter_is_a_root_error_and_extra_keys_are_ignored() {
    let schema = schema(vec![field(
        "title",
        true,
        FieldKind::String { format: None },
    )]);

    let root = validate_frontmatter(&schema, &json!(["not", "a", "map"]));
    assert_eq!(paths(&root), ["<root>"]);

    let extra = validate_frontmatter(&schema, &json!({ "title": "ok", "unknown": 1 }));
    assert!(extra.is_empty(), "unexpected: {extra:?}");
}

#[test]
fn diagnostics_are_sorted_and_deterministic() {
    let schema = schema(vec![
        field("zeta", true, FieldKind::String { format: None }),
        field("alpha", true, FieldKind::String { format: None }),
    ]);

    let diagnostics = validate_frontmatter(&schema, &json!({ "zeta": 1, "alpha": 2 }));

    assert_eq!(paths(&diagnostics), ["alpha", "zeta"]);
}

#[test]
fn value_type_helper_round_trips_through_serde() {
    // Guard the validation entry point against non-object primitives.
    let schema = schema(vec![field(
        "title",
        true,
        FieldKind::String { format: None },
    )]);
    let value: Value = serde_json::from_str("null").unwrap();
    let diagnostics = validate_frontmatter(&schema, &value);
    assert_eq!(paths(&diagnostics), ["<root>"]);
}

#[test]
fn dates_reject_impossible_calendar_dates_but_accept_leap_days() {
    let schema = schema(vec![field("when", true, FieldKind::Date)]);

    for accepted in ["2024-02-29", "2024-01-31", "2023-02-28", "2024-12-31"] {
        let diagnostics = validate_frontmatter(&schema, &json!({ "when": accepted }));
        assert!(
            diagnostics.is_empty(),
            "valid date {accepted} was rejected: {diagnostics:?}"
        );
    }

    for rejected in ["2024-02-31", "2023-02-29", "2024-13-01", "2024-04-31"] {
        let diagnostics = validate_frontmatter(&schema, &json!({ "when": rejected }));
        assert_eq!(
            paths(&diagnostics),
            ["when"],
            "impossible date {rejected} was accepted"
        );
    }
}
