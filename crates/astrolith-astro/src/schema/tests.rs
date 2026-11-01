use serde_json::{Value, json};

use super::{
    CollectionSchema, Field, FieldConstraints, FieldKind, SchemaDialect, normalize_schema,
};

const ASTRO5_SCHEMA: &str = include_str!("fixtures/astro5-docs.schema.json");
const ASTRO6_SCHEMA: &str = include_str!("fixtures/astro6-docs.schema.json");
const ASTRO7_SCHEMA: &str = include_str!("fixtures/astro7-docs.schema.json");

/// The zod-4 email refinement Astro 6/7 emit beside `format: email`.
const ASTRO7_EMAIL_PATTERN: &str = "^(?:[A-Za-z0-9_'+\\-]+\\.)*[A-Za-z0-9_'+\\-]*[A-Za-z0-9_+-]@(?:[A-Za-z0-9][A-Za-z0-9\\-]*\\.)+[A-Za-z]{2,}$";

fn parse(source: &str) -> CollectionSchema {
    let value: Value = serde_json::from_str(source).expect("fixture is valid JSON");
    normalize_schema(&value).expect("schema normalizes")
}

fn field<'a>(schema: &'a CollectionSchema, name: &str) -> &'a Field {
    schema
        .fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("missing field `{name}`"))
}

fn field_names(schema: &CollectionSchema) -> Vec<&str> {
    schema
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect()
}

fn string_kind() -> FieldKind {
    FieldKind::String { format: None }
}

fn nested_field<'a>(schema: &'a CollectionSchema, object: &str, name: &str) -> &'a Field {
    let FieldKind::Object { fields } = &field(schema, object).kind else {
        panic!("`{object}` should be an object");
    };
    fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("missing nested field `{object}.{name}`"))
}

/// Clear constraint values so two dialects generated from the same Zod schema
/// can be compared structurally.
fn without_constraints(schema: CollectionSchema) -> CollectionSchema {
    fn clear(fields: &mut [Field]) {
        for field in fields {
            field.constraints = FieldConstraints::default();
            clear_kind(&mut field.kind);
        }
    }
    fn clear_kind(kind: &mut FieldKind) {
        match kind {
            FieldKind::Object { fields } => clear(fields),
            FieldKind::Array { items }
            | FieldKind::Nullable { inner: items }
            | FieldKind::Record { value: items } => clear_kind(items),
            _ => {}
        }
    }

    let mut schema = schema;
    clear(&mut schema.fields);
    schema
}

#[test]
fn astro5_and_astro7_normalize_to_the_same_dialect_independent_model() {
    let astro5 = parse(ASTRO5_SCHEMA);
    let astro7 = parse(ASTRO7_SCHEMA);

    assert_eq!(astro5.dialect, SchemaDialect::Draft7);
    assert_eq!(astro7.dialect, SchemaDialect::Draft202012);
    assert_eq!(
        without_constraints(astro5.clone()).fields,
        without_constraints(astro7.clone()).fields,
        "both real dialects must normalize to one identical model"
    );
    // Astro 7 emits a zod-4 email refinement pattern that the Astro 5 fixture
    // does not; the model must keep the divergence instead of dropping it.
    assert_eq!(
        nested_field(&astro7, "author", "email")
            .constraints
            .pattern
            .as_deref(),
        Some(ASTRO7_EMAIL_PATTERN)
    );
    assert!(
        nested_field(&astro5, "author", "email")
            .constraints
            .is_empty()
    );
    assert_eq!(astro5.diagnostics, astro7.diagnostics);
}

#[test]
fn astro6_oracle_matches_the_astro7_dialect_independent_model() {
    let astro6 = parse(ASTRO6_SCHEMA);
    let astro7 = parse(ASTRO7_SCHEMA);

    assert_eq!(astro6.dialect, SchemaDialect::Draft202012);
    assert_eq!(
        astro6.fields, astro7.fields,
        "Astro 6's generated draft 2020-12 schema must normalize like Astro 7"
    );
    assert_eq!(astro6.diagnostics, astro7.diagnostics);
}

#[test]
fn normalizes_scalar_object_and_container_fields() {
    let schema = parse(ASTRO5_SCHEMA);

    // Fields keep declaration order (see `collect_fields`); the synthetic Astro
    // `$schema` property is not a field.
    assert_eq!(
        field_names(&schema),
        [
            "title",
            "description",
            "date",
            "draft",
            "tags",
            "category",
            "author",
            "nested",
            "union",
            "literal",
            "record",
        ]
    );
    let orders: Vec<usize> = schema.fields.iter().map(|field| field.order).collect();
    assert_eq!(orders, (0..11).collect::<Vec<_>>());

    assert_eq!(field(&schema, "title").kind, string_kind());
    assert!(field(&schema, "title").required);
    assert_eq!(field(&schema, "description").kind, string_kind());
    assert!(!field(&schema, "description").required);

    assert_eq!(field(&schema, "date").kind, FieldKind::Date);
    assert_eq!(
        field(&schema, "date").default,
        Some(json!("2024-01-01T00:00:00.000Z"))
    );

    assert_eq!(field(&schema, "draft").kind, FieldKind::Boolean);
    assert_eq!(field(&schema, "draft").default, Some(json!(false)));

    assert_eq!(
        field(&schema, "tags").kind,
        FieldKind::Array {
            items: Box::new(string_kind())
        }
    );
    assert_eq!(field(&schema, "tags").default, Some(json!([])));

    assert_eq!(
        field(&schema, "category").kind,
        FieldKind::Enum {
            values: vec![json!("news"), json!("blog"), json!("docs")]
        }
    );

    assert_eq!(
        field(&schema, "literal").kind,
        FieldKind::Literal {
            value: json!("fixed")
        }
    );

    assert_eq!(
        field(&schema, "record").kind,
        FieldKind::Record {
            value: Box::new(FieldKind::Number)
        }
    );
}

#[test]
fn normalizes_nested_objects_and_nullable_scalars() {
    let schema = parse(ASTRO5_SCHEMA);

    let author = &field(&schema, "author").kind;
    let FieldKind::Object { fields } = author else {
        panic!("author should be an object, got {author:?}");
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "name");
    assert_eq!(fields[0].kind, string_kind());
    assert!(fields[0].required);
    assert_eq!(fields[1].name, "email");
    assert_eq!(
        fields[1].kind,
        FieldKind::String {
            format: Some("email".into())
        }
    );
    assert!(!fields[1].required);

    let nested = &field(&schema, "nested").kind;
    let FieldKind::Object { fields } = nested else {
        panic!("nested should be an object");
    };
    let deep = &fields[0];
    assert_eq!(deep.name, "deep");
    assert!(deep.required);
    let FieldKind::Object { fields } = &deep.kind else {
        panic!("deep should be an object");
    };
    let value = &fields[0];
    assert_eq!(value.name, "value");
    assert!(value.required);
    assert_eq!(
        value.kind,
        FieldKind::Nullable {
            inner: Box::new(FieldKind::Number)
        }
    );
}

#[test]
fn heterogeneous_unions_become_unsupported_with_a_diagnostic() {
    let schema = parse(ASTRO5_SCHEMA);

    let union = &field(&schema, "union").kind;
    assert!(
        matches!(union, FieldKind::Unsupported { .. }),
        "union must not be coerced to a string: {union:?}"
    );
    assert_eq!(schema.diagnostics.len(), 1);
    assert_eq!(schema.diagnostics[0].path, "union");
    assert!(schema.diagnostics[0].message.contains("union"));
}

#[test]
fn type_array_nullable_and_union_are_distinguished() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "maybe": { "type": ["string", "null"] },
            "mixed": { "type": ["string", "number"] }
        }
    }))
    .expect("schema normalizes");

    assert_eq!(
        field(&schema, "maybe").kind,
        FieldKind::Nullable {
            inner: Box::new(string_kind())
        }
    );
    assert!(matches!(
        field(&schema, "mixed").kind,
        FieldKind::Unsupported { .. }
    ));
    assert_eq!(schema.diagnostics.len(), 1);
    assert_eq!(schema.diagnostics[0].path, "mixed");
}

#[test]
fn any_of_date_union_is_recognized_and_nullable_date_is_wrapped() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "published": {
                "anyOf": [
                    { "type": "string", "format": "date-time" },
                    { "type": "string", "format": "date" },
                    { "type": "integer", "format": "unix-time" }
                ]
            },
            "maybeDate": {
                "anyOf": [
                    { "type": "string", "format": "date-time" },
                    { "type": "null" }
                ]
            }
        }
    }))
    .expect("schema normalizes");

    assert_eq!(field(&schema, "published").kind, FieldKind::Date);
    assert_eq!(
        field(&schema, "maybeDate").kind,
        FieldKind::Nullable {
            inner: Box::new(FieldKind::Date)
        }
    );
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn arrays_and_scalar_defaults_are_preserved() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "list": {
                "type": "array",
                "items": { "type": "integer" },
                "default": []
            },
            "count": { "type": "integer", "default": 0 }
        }
    }))
    .expect("schema normalizes");

    assert_eq!(
        field(&schema, "list").kind,
        FieldKind::Array {
            items: Box::new(FieldKind::Integer)
        }
    );
    assert_eq!(field(&schema, "list").default, Some(json!([])));
    assert_eq!(field(&schema, "count").kind, FieldKind::Integer);
    assert_eq!(field(&schema, "count").default, Some(json!(0)));
}

#[test]
fn descriptions_and_constraints_are_captured() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "title": {
                "type": "string",
                "description": "Shown in the browser tab",
                "minLength": 1,
                "maxLength": 80,
                "pattern": "^[A-Z]"
            },
            "rating": { "type": "number", "minimum": 0.5, "maximum": 5 },
            "rank": { "type": "integer", "minimum": 1 },
            "tags": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "maxItems": 4
            }
        }
    }))
    .expect("schema normalizes");

    let title = field(&schema, "title");
    assert_eq!(
        title.description.as_deref(),
        Some("Shown in the browser tab")
    );
    assert_eq!(
        title.constraints,
        FieldConstraints {
            minimum: None,
            maximum: None,
            min_length: Some(1),
            max_length: Some(80),
            pattern: Some("^[A-Z]".into()),
        }
    );
    assert_eq!(
        field(&schema, "rating").constraints,
        FieldConstraints {
            minimum: Some(0.5),
            maximum: Some(5.0),
            ..FieldConstraints::default()
        }
    );
    assert_eq!(field(&schema, "rank").constraints.minimum, Some(1.0));
    let tags = &field(&schema, "tags").constraints;
    assert_eq!(tags.min_length, Some(1));
    assert_eq!(tags.max_length, Some(4));
    assert!(tags.pattern.is_none());
    assert!(!tags.is_empty());
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn empty_object_schema_is_valid_and_has_no_fields() {
    let schema = normalize_schema(&json!({ "type": "object", "properties": {} }))
        .expect("empty object schema normalizes");
    assert!(schema.fields.is_empty());
    assert!(schema.diagnostics.is_empty());
    assert_eq!(schema.dialect, SchemaDialect::Unknown);
}

#[test]
fn structurally_invalid_documents_are_errors() {
    assert!(normalize_schema(&json!([])).is_err());
    assert!(normalize_schema(&json!("not a schema")).is_err());
    assert!(normalize_schema(&json!({ "type": "array" })).is_err());
    assert!(
        normalize_schema(&json!({
            "$ref": "#/definitions/missing",
            "definitions": {}
        }))
        .is_err(),
        "an unresolved root reference must be an error, not an empty schema"
    );
}

#[test]
fn cyclic_references_do_not_recurse_forever() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": { "loop": { "$ref": "#/definitions/node" } },
        "definitions": {
            "node": {
                "type": "object",
                "properties": { "self": { "$ref": "#/definitions/node" } }
            }
        }
    }))
    .expect("a cyclic schema must not crash normalization");

    let FieldKind::Object { fields } = &field(&schema, "loop").kind else {
        panic!("a resolved reference should be an object");
    };
    assert!(matches!(fields[0].kind, FieldKind::Unsupported { .. }));
    assert!(
        schema
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cyclic"))
    );
}

#[test]
fn multiple_diagnostics_are_sorted_deterministically() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "zeta": { "type": ["string", "number"] },
            "alpha": { "type": ["boolean", "object"] }
        }
    }))
    .expect("schema normalizes");

    let paths: Vec<&str> = schema
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.path.as_str())
        .collect();
    assert_eq!(paths, ["alpha", "zeta"]);
}

#[test]
fn defs_references_are_supported() {
    let schema = normalize_schema(&json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "properties": { "value": { "$ref": "#/$defs/name" } },
        "$defs": { "name": { "type": "string" } }
    }))
    .expect("$defs reference resolves");

    assert_eq!(schema.dialect, SchemaDialect::Draft202012);
    assert_eq!(field(&schema, "value").kind, string_kind());
}

#[test]
fn open_objects_tuples_and_null_only_fields_are_unsupported() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "open": { "type": "object" },
            "tuple": { "type": "array", "items": [{ "type": "string" }] },
            "nothing": { "type": "null" },
            "nothingUnion": { "type": ["null"] }
        }
    }))
    .expect("schema normalizes");

    for name in ["open", "tuple", "nothing", "nothingUnion"] {
        assert!(
            matches!(field(&schema, name).kind, FieldKind::Unsupported { .. }),
            "{name} should be unsupported"
        );
    }
    assert_eq!(schema.diagnostics.len(), 4);
}

#[test]
fn nested_unresolved_reference_is_unsupported_not_fatal() {
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": { "broken": { "$ref": "#/definitions/missing" } },
        "definitions": {}
    }))
    .expect("a nested unresolved reference must not abort the whole schema");

    assert!(matches!(
        field(&schema, "broken").kind,
        FieldKind::Unsupported { .. }
    ));
    assert_eq!(schema.diagnostics[0].path, "broken");
}

#[test]
fn nullable_date_union_is_wrapped_not_unsupported() {
    // Astro 5 flattens a nullable date to its three date branches plus a null branch.
    let schema = normalize_schema(&json!({
        "type": "object",
        "properties": {
            "when": {
                "anyOf": [
                    { "type": "string", "format": "date-time" },
                    { "type": "string", "format": "date" },
                    { "type": "integer", "format": "unix-time" },
                    { "type": "null" }
                ]
            }
        }
    }))
    .expect("schema normalizes");

    assert_eq!(
        field(&schema, "when").kind,
        FieldKind::Nullable {
            inner: Box::new(FieldKind::Date)
        }
    );
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn deeply_nested_schemas_are_bounded_not_crashing() {
    let mut value = json!({ "type": "string" });
    for _ in 0..100 {
        value = json!({ "type": "object", "properties": { "x": value } });
    }
    let schema = normalize_schema(&json!({ "type": "object", "properties": { "root": value } }))
        .expect("excessive depth must be bounded, not fatal");
    assert!(!schema.diagnostics.is_empty());
}
