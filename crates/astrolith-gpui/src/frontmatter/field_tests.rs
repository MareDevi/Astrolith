use astrolith_astro::schema::{
    CollectionSchema, Field, FieldConstraints, FieldKind, SchemaDialect,
};
use astrolith_core::preferences::FieldMappings;
use astrolith_document::{FieldShape, FrontmatterValue, parse_document};
use serde_json::Value;

use super::*;

fn field(order: usize, name: &str, required: bool, kind: FieldKind) -> Field {
    Field {
        name: name.to_owned(),
        required,
        default: None,
        kind,
        description: None,
        constraints: FieldConstraints::default(),
        order,
    }
}

fn schema(fields: Vec<Field>) -> CollectionSchema {
    CollectionSchema {
        dialect: SchemaDialect::Draft202012,
        fields,
        diagnostics: Vec::new(),
    }
}

fn text() -> FieldKind {
    FieldKind::String { format: None }
}

fn controls(
    schema: Option<&CollectionSchema>,
    mappings: &FieldMappings,
    document: &str,
) -> Vec<FieldControl> {
    let parsed = parse_document(document).expect("valid document");
    build_controls(schema, mappings, parsed.fields())
}

#[test]
fn title_first_then_declaration_order_then_unknown_alphabetically() {
    let schema = schema(vec![
        field(0, "date", true, FieldKind::Date),
        field(1, "zeta", false, text()),
        field(2, "title", true, text()),
        field(3, "alpha", false, FieldKind::Boolean),
    ]);
    let controls = controls(
        Some(&schema),
        &FieldMappings::default(),
        "---\ntitle: T\ndate: 2024-01-01\nzeta: z\nalpha: true\nzzz: unknown\nbbb: other\n---\n",
    );
    let paths: Vec<&str> = controls
        .iter()
        .map(|control| control.path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["title", "date", "zeta", "alpha", "bbb", "zzz"],
        "title first, schema declaration order, unknown fields alphabetical"
    );
    assert_eq!(controls[0].role, Some(FieldRole::Title));
}

#[test]
fn absent_schema_fields_are_listed_with_defaults_and_constraints_as_hints() {
    let mut title = field(0, "title", true, text());
    title.description = Some("Display title".to_owned());
    title.constraints = FieldConstraints {
        min_length: Some(1),
        max_length: Some(120),
        ..FieldConstraints::default()
    };
    let mut order = field(1, "order", false, FieldKind::Number);
    order.default = Some(Value::from(0));
    order.constraints = FieldConstraints {
        minimum: Some(0.0),
        maximum: Some(999.0),
        ..FieldConstraints::default()
    };
    let controls = controls(
        Some(&schema(vec![title, order])),
        &FieldMappings::default(),
        "---\ntitle: Existing\n---\n",
    );
    let title = controls.iter().find(|c| c.path == "title").unwrap();
    assert_eq!(title.description.as_deref(), Some("Display title"));
    assert_eq!(
        title.hint().as_deref(),
        Some("Length: 1–120"),
        "a present field shows its own constraints without a default"
    );
    let order = controls.iter().find(|c| c.path == "order").unwrap();
    assert!(!order.present, "optional absent fields are listed");
    assert_eq!(
        order.hint().as_deref(),
        Some("Default: 0 · Min: 0 · Max: 999")
    );
    assert!(order.editable, "an absent optional field can be inserted");
    assert!(!order.removable);
}

#[test]
fn every_schema_control_family_maps_to_its_native_control() {
    use ControlKind::*;
    let schema = schema(vec![
        field(0, "title", true, text()),
        field(
            1,
            "email",
            false,
            FieldKind::String {
                format: Some("email".into()),
            },
        ),
        field(2, "score", false, FieldKind::Number),
        field(3, "count", false, FieldKind::Integer),
        field(4, "draft", false, FieldKind::Boolean),
        field(5, "published", false, FieldKind::Date),
        field(6, "cover", false, FieldKind::Image),
        field(
            7,
            "status",
            false,
            FieldKind::Enum {
                values: vec![Value::String("draft".into()), Value::String("live".into())],
            },
        ),
        field(
            8,
            "author",
            false,
            FieldKind::Reference {
                collections: vec!["authors".into()],
            },
        ),
        field(
            9,
            "related",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::Reference {
                    collections: vec!["blog".into()],
                }),
            },
        ),
        field(
            10,
            "tags",
            false,
            FieldKind::Array {
                items: Box::new(text()),
            },
        ),
        field(
            11,
            "author_group",
            false,
            FieldKind::Object {
                fields: vec![
                    field(0, "name", false, text()),
                    field(1, "avatar", false, FieldKind::Image),
                ],
            },
        ),
    ]);
    let controls = controls(
        Some(&schema),
        &FieldMappings::default(),
        "---\ntitle: T\nemail: a@b.co\nscore: 1.5\ncount: 2\ndraft: false\npublished: 2024-01-01\ncover: ./c.png\nauthor: ada\nrelated: [a]\ntags: [x, y]\nauthor_group:\n  name: Ada\n  avatar: ./a.png\n---\n",
    );
    let kind = |path: &str| {
        controls
            .iter()
            .find(|c| c.path == path)
            .unwrap()
            .kind
            .clone()
    };
    assert!(matches!(kind("title"), Text { format: None }));
    assert!(matches!(kind("email"), Text { format: Some(f) } if f == "email"));
    assert_eq!(kind("score"), Number);
    assert_eq!(kind("count"), Integer);
    assert_eq!(kind("draft"), Boolean);
    assert_eq!(kind("published"), Date);
    assert_eq!(kind("cover"), Image);
    assert!(matches!(kind("status"), Enum { .. }));
    assert!(matches!(
        kind("author"),
        Reference { collections, multiple: false } if collections == ["authors".to_owned()]
    ));
    assert!(matches!(
        kind("related"),
        Reference { collections, multiple: true } if collections == ["blog".to_owned()]
    ));
    assert_eq!(kind("tags"), Tags);
    assert_eq!(kind("author_group"), Object);
    assert_eq!(
        kind("author_group.avatar"),
        Image,
        "a nested image keeps the real image control"
    );
    assert_eq!(controls[0].path, "title");
}

#[test]
fn unsupported_shapes_fall_back_to_raw_yaml_or_an_explicit_refusal() {
    let schema = schema(vec![
        field(
            0,
            "tags",
            false,
            FieldKind::Array {
                items: Box::new(text()),
            },
        ),
        field(
            1,
            "meta",
            false,
            FieldKind::Record {
                value: Box::new(text()),
            },
        ),
        field(2, "note", false, text()),
        field(
            3,
            "legacy",
            false,
            FieldKind::Unsupported {
                reason: "union".into(),
            },
        ),
        field(
            4,
            "fixed",
            false,
            FieldKind::Literal {
                value: Value::String("x".into()),
            },
        ),
    ]);
    let controls = controls(
        Some(&schema),
        &FieldMappings::default(),
        "---\ntags: [a, 2]\nmeta: {a: b}\nnote: |\n  text\nlegacy: 3\nfixed: x\n---\n",
    );
    let by = |path: &str| controls.iter().find(|c| c.path == path).unwrap();
    assert!(
        matches!(by("tags").kind, ControlKind::RawYaml { .. }) && by("tags").editable,
        "a heterogeneous array is a validated raw-YAML control, never coerced"
    );
    assert!(matches!(by("meta").kind, ControlKind::RawYaml { .. }));
    assert!(matches!(by("note").kind, ControlKind::ReadOnly));
    assert!(matches!(by("legacy").kind, ControlKind::ReadOnly));
    assert!(matches!(by("fixed").kind, ControlKind::Literal { .. }));
    assert!(!by("fixed").editable);
}

#[test]
fn non_scalar_sequences_fall_back_to_raw_yaml() {
    let schema = schema(vec![field(
        0,
        "rows",
        false,
        FieldKind::Array {
            items: Box::new(text()),
        },
    )]);
    let controls = controls(
        Some(&schema),
        &FieldMappings::default(),
        "---\nrows:\n  - name: a\n  - name: b\n---\n",
    );
    let rows = &controls[0];
    assert!(matches!(rows.kind, ControlKind::RawYaml { .. }));
    assert!(rows.editable);
    assert_eq!(rows.refusal, None);
}

#[test]
fn no_schema_shows_only_existing_fields_alphabetically() {
    let controls = controls(
        None,
        &FieldMappings::default(),
        "---\nzeta: 1\nalpha: x\ndraft: true\n---\n",
    );
    let paths: Vec<&str> = controls.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["alpha", "draft", "zeta"]);
    assert!(matches!(controls[0].kind, ControlKind::Text { .. }));
    assert_eq!(controls[1].kind, ControlKind::Boolean);
    assert_eq!(controls[2].kind, ControlKind::Integer);
    assert_eq!(controls[0].role, None);
}

#[test]
fn mapped_fields_get_roles_and_mappings_change_the_panel() {
    // The mapped title is declared last so the ordering assertion is meaningful.
    let schema = schema(vec![
        field(0, "summary", false, text()),
        field(1, "status", false, FieldKind::Boolean),
        field(2, "released", false, FieldKind::Date),
        field(3, "headline", true, text()),
    ]);
    let mappings = FieldMappings {
        title: Some("headline".into()),
        description: Some("summary".into()),
        draft: Some("status".into()),
        date: Some("released".into()),
    };
    let controls = controls(
        Some(&schema),
        &mappings,
        "---\nheadline: Hi\nsummary: Long\nstatus: false\nreleased: 2024-01-01\n---\n",
    );
    let role = |path: &str| controls.iter().find(|c| c.path == path).unwrap().role;
    assert_eq!(role("headline"), Some(FieldRole::Title));
    assert_eq!(role("summary"), Some(FieldRole::Description));
    assert_eq!(role("status"), Some(FieldRole::Draft));
    assert_eq!(role("released"), Some(FieldRole::Date));
    assert_eq!(
        controls
            .iter()
            .map(|control| control.path.as_str())
            .collect::<Vec<_>>(),
        ["headline", "summary", "status", "released"],
        "the mapped title renders first, then schema declaration order"
    );
}

#[test]
fn mapped_description_defaults_are_marked_without_a_schema() {
    let controls = controls(
        None,
        &FieldMappings::default(),
        "---\ntitle: T\ndescription: D\n---\n",
    );
    let role = |path: &str| controls.iter().find(|c| c.path == path).unwrap().role;
    assert_eq!(role("title"), Some(FieldRole::Title));
    assert_eq!(role("description"), Some(FieldRole::Description));
}

#[test]
fn unknown_nested_fields_are_listed_after_schema_children() {
    let schema = schema(vec![field(
        0,
        "seo",
        false,
        FieldKind::Object {
            fields: vec![field(0, "title", false, text())],
        },
    )]);
    let controls = controls(
        Some(&schema),
        &FieldMappings::default(),
        "---\nseo:\n  title: T\n  extra: x\n---\n",
    );
    let paths: Vec<&str> = controls.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["seo", "seo.title", "seo.extra"]);
}

#[test]
fn tags_parse_and_scalar_edits_are_typed() {
    let tags = parse_value(&ControlKind::Tags, "hello world").unwrap();
    assert_eq!(tags, FrontmatterValue::String("hello world".to_owned()));
    assert!(matches!(
        parse_value(&ControlKind::Integer, "3"),
        Ok(FrontmatterValue::Integer(3))
    ));
    assert!(parse_value(&ControlKind::Integer, "x").is_err());
    assert_eq!(
        parse_value(&ControlKind::Boolean, "false"),
        Ok(FrontmatterValue::Boolean(false))
    );
    assert!(matches!(
        parse_value(
            &ControlKind::Enum {
                values: vec![Value::String("a".into())]
            },
            "a"
        ),
        Ok(FrontmatterValue::String(_))
    ));
    assert!(parse_value(&ControlKind::Object, "x").is_err());
    assert!(parse_value(&ControlKind::ReadOnly, "x").is_err());
}

#[test]
fn slugify_is_stable_and_unicode_aware() {
    assert_eq!(slugify("Hello, World!"), "hello-world");
    assert_eq!(slugify("  Multiple   spaces  "), "multiple-spaces");
    assert_eq!(slugify("Ünïcode Straße"), "ünïcode-straße");
    assert_eq!(slugify("C++ & Rust"), "c-rust");
    assert_eq!(slugify(""), "");
}

#[test]
fn block_sequence_shapes_are_detected_for_the_form() {
    let parsed = parse_document("---\ntags:\n  - one\n  - two\n---\n").unwrap();
    let info = parsed.field_info("tags").unwrap();
    assert_eq!(info.shape, FieldShape::Sequence);
    assert_eq!(
        info.items,
        Some(vec![
            FrontmatterValue::String("one".into()),
            FrontmatterValue::String("two".into())
        ])
    );
}

#[test]
fn block_containers_map_to_raw_yaml_controls() {
    let schema = schema(vec![
        field(
            0,
            "scores",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::Integer),
            },
        ),
        field(
            1,
            "people",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::Object {
                    fields: vec![field(0, "name", true, text())],
                }),
            },
        ),
        field(2, "note", false, text()),
    ]);
    let document =
        "---\nscores:\n  - 1\n  - 2\npeople:\n  - name: a\n  - name: b\nnote:\n  a: 1\n---\n";
    let controls = controls(Some(&schema), &FieldMappings::default(), document);
    let by = |path: &str| {
        controls
            .iter()
            .find(|control| control.path == path)
            .unwrap()
    };
    for path in ["scores", "people", "note"] {
        assert!(
            matches!(by(path).kind, ControlKind::RawYaml { .. }) && by(path).editable,
            "{path} must fall back to a validated raw-YAML control"
        );
    }

    let mut parsed = parse_document(document).unwrap();
    parsed.replace_raw("scores", "- 3").unwrap();
    parsed.replace_raw("people", "- name: c").unwrap();
    parsed.replace_raw("note", "b: 2").unwrap();
    let output = parsed.serialize().unwrap();
    assert_eq!(
        output,
        "---\nscores:\n  - 3\npeople:\n  - name: c\nnote:\n  b: 2\n---\n"
    );
}

#[test]
fn every_editable_control_can_apply_at_least_one_edit() {
    let schema = schema(vec![
        field(0, "title", true, text()),
        field(1, "score", false, FieldKind::Number),
        field(2, "count", false, FieldKind::Integer),
        field(3, "draft", false, FieldKind::Boolean),
        field(4, "published", false, FieldKind::Date),
        field(5, "cover", false, FieldKind::Image),
        field(
            6,
            "author",
            false,
            FieldKind::Reference {
                collections: vec!["authors".into()],
            },
        ),
        field(
            7,
            "related",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::Reference {
                    collections: vec!["authors".into()],
                }),
            },
        ),
        field(
            8,
            "tags",
            false,
            FieldKind::Array {
                items: Box::new(text()),
            },
        ),
        field(
            9,
            "status",
            false,
            FieldKind::Enum {
                values: vec![Value::String("draft".into())],
            },
        ),
        field(
            10,
            "seo",
            false,
            FieldKind::Object {
                fields: vec![field(0, "description", false, text())],
            },
        ),
        field(
            11,
            "meta",
            false,
            FieldKind::Record {
                value: Box::new(text()),
            },
        ),
        field(
            12,
            "scores",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::Integer),
            },
        ),
    ]);
    let document = "---\ntitle: T\nscore: 1.5\ncount: 2\ndraft: false\npublished: 2024-01-01\ncover: cover.png\nauthor: a\nrelated:\n  - a\ntags: [x]\nstatus: draft\nseo:\n  description: D\nmeta: {a: b}\nscores:\n  - 1\n---\n";
    let parsed = parse_document(document).unwrap();
    let controls = build_controls(Some(&schema), &FieldMappings::default(), parsed.fields());
    let editable: Vec<&FieldControl> = controls.iter().filter(|control| control.editable).collect();
    assert!(!editable.is_empty());
    for control in editable {
        let mut document = parsed.clone();
        let applied = match &control.kind {
            ControlKind::Tags | ControlKind::Reference { multiple: true, .. } => document
                .insert_sequence_item(&control.path, 0, FrontmatterValue::String("x".into()))
                .is_ok(),
            ControlKind::RawYaml { .. } => {
                let raw = document
                    .field_info(&control.path)
                    .and_then(|info| info.raw.clone())
                    .expect("raw controls expose their current text");
                document.replace_raw(&control.path, &raw).is_ok()
            }
            kind => {
                let value = match kind {
                    ControlKind::Number => FrontmatterValue::Float(1.5),
                    ControlKind::Integer => FrontmatterValue::Integer(1),
                    ControlKind::Boolean => FrontmatterValue::Boolean(true),
                    _ => FrontmatterValue::String("value".into()),
                };
                document.set_field(&control.path, value).is_ok()
            }
        };
        assert!(
            applied,
            "editable control {} ({:?}) must accept an edit",
            control.path, control.kind
        );
    }
}
