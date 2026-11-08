use std::sync::{Arc, Mutex};

use astrolith_astro::schema::{
    CollectionSchema, Field, FieldConstraints, FieldKind, SchemaDialect,
};

use super::*;

fn schema(fields: Vec<Field>) -> Arc<CollectionSchema> {
    Arc::new(CollectionSchema {
        dialect: SchemaDialect::Draft202012,
        fields,
        diagnostics: Vec::new(),
    })
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

fn editor(fields: Vec<Field>, text: &str) -> FrontmatterEditor {
    FrontmatterEditor::new(
        Some(schema(fields)),
        FieldMappings::default(),
        text.to_owned(),
    )
    .expect("valid document")
}

/// Build controls for a document whose frontmatter is absent, so every schema
/// field is listed as a not-yet-present field.
fn control_list(fields: Vec<Field>) -> Vec<FieldControl> {
    let schema = schema(fields);
    fields::build_controls(Some(&schema), &FieldMappings::default(), &[])
}

/// Build controls for `document` against the given schema fields.
fn control_list_for(fields: Vec<Field>, document: &str) -> Vec<FieldControl> {
    let schema = schema(fields);
    let parsed = parse_document(document).expect("valid document");
    fields::build_controls(Some(&schema), &FieldMappings::default(), parsed.fields())
}

#[test]
fn maps_every_field_kind_to_its_native_control() {
    use ControlKind::*;
    let controls = control_list(vec![
        field("title", true, FieldKind::String { format: None }),
        field(
            "email",
            false,
            FieldKind::String {
                format: Some("email".to_owned()),
            },
        ),
        field("score", false, FieldKind::Number),
        field("count", false, FieldKind::Integer),
        field("draft", false, FieldKind::Boolean),
        field("published", false, FieldKind::Date),
        field("cover", false, FieldKind::Image),
        field(
            "author",
            false,
            FieldKind::Reference {
                collections: vec!["authors".to_owned()],
            },
        ),
        field(
            "status",
            false,
            FieldKind::Enum {
                values: vec![Value::String("draft".into()), Value::String("live".into())],
            },
        ),
        field(
            "version",
            false,
            FieldKind::Literal {
                value: Value::String("1".into()),
            },
        ),
        field(
            "tags",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::String { format: None }),
            },
        ),
        field(
            "meta",
            false,
            FieldKind::Record {
                value: Box::new(FieldKind::String { format: None }),
            },
        ),
        field(
            "union",
            false,
            FieldKind::Unsupported {
                reason: "heterogeneous union".to_owned(),
            },
        ),
    ]);

    let by_path = |path: &str| controls.iter().find(|c| c.path == path).unwrap().clone();
    assert!(matches!(by_path("title").kind, Text { format: None }));
    assert!(matches!(
        by_path("email").kind,
        Text { format: Some(ref f) } if f == "email"
    ));
    assert_eq!(by_path("score").kind, Number);
    assert_eq!(by_path("score").control_label(), "number");
    assert_eq!(by_path("count").kind, Integer);
    assert_eq!(by_path("draft").kind, Boolean);
    assert_eq!(by_path("published").kind, Date);
    assert_eq!(by_path("cover").kind, Image);
    assert!(
        matches!(by_path("author").kind, Reference { ref collections, multiple: false } if collections == &["authors".to_owned()])
    );
    assert!(matches!(
        by_path("tags").kind,
        Tags if by_path("tags").editable
    ));
    assert!(matches!(by_path("meta").kind, ReadOnly));

    // Scalars and tags are editable; literals, containers and unsupported
    // constructs are read-only with an explicit reason.
    for path in [
        "title",
        "email",
        "score",
        "count",
        "draft",
        "published",
        "cover",
        "author",
        "tags",
    ] {
        assert!(by_path(path).editable, "{path} must be editable");
    }
    assert!(!by_path("version").editable);
    assert!(by_path("version").refusal.is_some());
    assert!(!by_path("meta").editable);
    assert_eq!(
        by_path("meta").refusal.as_ref().map(RefusalReason::text),
        Some("record values are preserved verbatim".to_owned())
    );
    assert!(!by_path("union").editable);
    assert_eq!(
        by_path("union").refusal.as_ref().map(RefusalReason::text),
        Some("heterogeneous union".to_owned()),
        "unsupported unions surface their explicit refusal reason"
    );
}

#[test]
fn object_fields_emit_a_container_and_editable_children_only_when_present() {
    let fields = vec![field(
        "author",
        true,
        FieldKind::Object {
            fields: vec![
                field("name", true, FieldKind::String { format: None }),
                field("age", false, FieldKind::Integer),
            ],
        },
    )];

    // An absent mapping has no editable children: inserting `author.name`
    // without the parent mapping would not be lossless.
    let absent = control_list(fields.clone());
    let paths: Vec<&str> = absent.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["author"]);
    assert_eq!(absent[0].kind, ControlKind::Object);
    assert!(!absent[0].editable);
    assert!(!absent[0].present);

    let controls = control_list_for(fields, "---\nauthor:\n  name: Ada\n  age: 36\n---\n");
    let paths: Vec<&str> = controls.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["author", "author.name", "author.age"]);
    assert_eq!(controls[0].kind, ControlKind::Object);
    assert!(!controls[0].editable);
    assert!(controls[1].editable && controls[2].editable);
    assert_eq!(controls[1].control_label(), "text");
    assert_eq!(controls[2].control_label(), "integer");
    assert!(controls[1].present && controls[2].present);
}

#[test]
fn nullable_scalars_stay_editable_and_flag_nullable() {
    let controls = control_list(vec![field(
        "rating",
        false,
        FieldKind::Nullable {
            inner: Box::new(FieldKind::Number),
        },
    )]);
    assert_eq!(controls[0].kind, ControlKind::Number);
    assert!(controls[0].editable);
    assert!(controls[0].nullable);
}

#[test]
fn editing_a_string_updates_the_value_and_preserves_every_other_byte() {
    let text = "---\ntitle: Old\nslug: keep-me\n---\n\n# Body 中文\n<X>{42}</X>\n";
    let mut editor = editor(
        vec![field("title", true, FieldKind::String { format: None })],
        text,
    );
    let updated = editor.set_value("title", "New title").unwrap();
    assert_eq!(
        updated,
        "---\ntitle: New title\nslug: keep-me\n---\n\n# Body 中文\n<X>{42}</X>\n"
    );
    assert_eq!(
        editor.value("title"),
        Some(FrontmatterValue::String("New title".to_owned()))
    );
    assert_eq!(editor.text(), updated);
}

#[test]
fn editing_parses_typed_scalars() {
    let text = "---\nscore: 1\ncount: 2\ndraft: false\npublished: 2020-01-01\n---\n";
    let mut editor = editor(
        vec![
            field("score", false, FieldKind::Number),
            field("count", false, FieldKind::Integer),
            field("draft", false, FieldKind::Boolean),
            field("published", false, FieldKind::Date),
        ],
        text,
    );
    editor.set_value("score", "2.5").unwrap();
    editor.set_value("count", "7").unwrap();
    editor.set_value("draft", "true").unwrap();
    editor.set_value("published", "2024-12-31").unwrap();
    assert_eq!(editor.value("score"), Some(FrontmatterValue::Float(2.5)));
    assert_eq!(editor.value("count"), Some(FrontmatterValue::Integer(7)));
    assert_eq!(editor.value("draft"), Some(FrontmatterValue::Boolean(true)));
    assert_eq!(
        editor.value("published"),
        Some(FrontmatterValue::String("2024-12-31".to_owned()))
    );
    assert!(editor.diagnostics_for("score").next().is_none());
}

#[test]
fn invalid_scalar_input_is_reported_and_leaves_the_document_unchanged() {
    let text = "---\ncount: 2\n---\n";
    let mut editor = editor(vec![field("count", false, FieldKind::Integer)], text);
    let error = editor.set_value("count", "not-an-int").unwrap_err();
    assert!(error.contains("not a valid integer"), "got {error}");
    assert_eq!(editor.text(), text, "failed edits must not change the text");
    assert_eq!(editor.value("count"), Some(FrontmatterValue::Integer(2)));
}

#[test]
fn enum_edits_accept_allowed_values_and_reject_others() {
    let text = "---\nstatus: draft\n---\n";
    let mut editor = editor(
        vec![field(
            "status",
            false,
            FieldKind::Enum {
                values: vec![
                    Value::String("draft".into()),
                    Value::String("published".into()),
                ],
            },
        )],
        text,
    );
    editor.set_value("status", "published").unwrap();
    assert_eq!(
        editor.value("status"),
        Some(FrontmatterValue::String("published".to_owned()))
    );
    let error = editor.set_value("status", "archived").unwrap_err();
    assert!(
        error.contains("not one of the allowed values"),
        "got {error}"
    );
}

#[test]
fn nullable_scalar_can_be_edited() {
    let text = "---\nrating: 3\n---\n";
    let mut editor = editor(
        vec![field(
            "rating",
            false,
            FieldKind::Nullable {
                inner: Box::new(FieldKind::Integer),
            },
        )],
        text,
    );
    editor.set_value("rating", "5").unwrap();
    assert_eq!(editor.value("rating"), Some(FrontmatterValue::Integer(5)));
}

#[test]
fn read_only_fields_refuse_edits_with_an_explicit_reason() {
    let text = "---\ntags: [a, b]\nversion: \"1\"\n---\n";
    let mut editor = editor(
        vec![
            field(
                "tags",
                false,
                FieldKind::Array {
                    items: Box::new(FieldKind::String { format: None }),
                },
            ),
            field(
                "version",
                false,
                FieldKind::Literal {
                    value: Value::String("1".into()),
                },
            ),
            field(
                "union",
                false,
                FieldKind::Unsupported {
                    reason: "heterogeneous union".to_owned(),
                },
            ),
        ],
        text,
    );
    for (path, expected) in [
        ("tags", "item by item"),
        ("version", "literal"),
        ("union", "heterogeneous union"),
    ] {
        let error = editor.set_value(path, "attempt").unwrap_err();
        assert!(
            error.contains(expected),
            "path {path} must refuse with {expected:?}, got {error:?}"
        );
    }
    assert_eq!(editor.text(), text);
}

#[test]
fn unsupported_fields_are_never_coerced_to_string() {
    let controls = control_list(vec![field(
        "value",
        false,
        FieldKind::Unsupported {
            reason: "heterogeneous union [string, number]".to_owned(),
        },
    )]);
    assert_eq!(controls[0].kind, ControlKind::ReadOnly);
    assert!(!controls[0].editable);
    assert_eq!(
        controls[0].refusal.as_ref().map(RefusalReason::text),
        Some("heterogeneous union [string, number]".to_owned()),
        "the schema's refusal reason is surfaced verbatim"
    );
}

#[test]
fn validation_feedback_reports_missing_required_and_wrong_types() {
    let text = "---\ncount: oops\n---\n";
    let editor = editor(
        vec![
            field("title", true, FieldKind::String { format: None }),
            field("count", false, FieldKind::Integer),
        ],
        text,
    );
    let missing: Vec<_> = editor
        .diagnostics_for("title")
        .map(|d| d.message.clone())
        .collect();
    assert!(
        missing.iter().any(|m| m.contains("required")),
        "expected a required diagnostic, got {missing:?}"
    );
    let wrong: Vec<_> = editor
        .diagnostics_for("count")
        .map(|d| d.message.clone())
        .collect();
    assert!(
        wrong.iter().any(|m| m.contains("integer")),
        "expected an integer diagnostic, got {wrong:?}"
    );
}

#[test]
fn validation_feedback_reports_email_format() {
    let editor = editor(
        vec![field(
            "email",
            false,
            FieldKind::String {
                format: Some("email".to_owned()),
            },
        )],
        "---\nemail: not-an-email\n---\n",
    );
    let messages: Vec<_> = editor
        .diagnostics_for("email")
        .map(|d| d.message.clone())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("email")),
        "expected an email diagnostic, got {messages:?}"
    );
}

#[test]
fn editing_clears_a_previously_reported_diagnostic() {
    let text = "---\ncount: oops\n---\n";
    let mut editor = editor(vec![field("count", false, FieldKind::Integer)], text);
    assert!(editor.diagnostics_for("count").next().is_some());
    editor.set_value("count", "3").unwrap();
    assert!(editor.diagnostics_for("count").next().is_none());
}

#[test]
fn absent_optional_fields_and_sequence_items_edit_losslessly() {
    let text = "---\r\ntitle: Old  # keep\r\ntags:\r\n  - a  # first\r\n  - b\r\n---\r\nBody\r\n";
    let mut editor = editor(
        vec![
            field("title", true, FieldKind::String { format: None }),
            field(
                "tags",
                false,
                FieldKind::Array {
                    items: Box::new(FieldKind::String { format: None }),
                },
            ),
            field("draft", false, FieldKind::Boolean),
        ],
        text,
    );

    // Inserting an absent optional field appends one line and preserves every
    // other byte, including comments and CRLF line endings.
    let inserted = editor.set_value("draft", "true").unwrap();
    assert_eq!(
        inserted,
        "---\r\ntitle: Old  # keep\r\ntags:\r\n  - a  # first\r\n  - b\r\ndraft: true\r\n---\r\nBody\r\n"
    );

    // Removing it restores the original document byte for byte.
    let removed = editor.remove_field("draft").unwrap();
    assert_eq!(removed, text);
    let error = editor.remove_field("title").unwrap_err();
    assert!(error.contains("required"), "{error}");

    editor.add_item("tags", "c").unwrap();
    assert_eq!(
        editor.text(),
        "---\r\ntitle: Old  # keep\r\ntags:\r\n  - a  # first\r\n  - b\r\n  - c\r\n---\r\nBody\r\n"
    );
    editor.remove_item("tags", 1).unwrap();
    assert_eq!(
        editor.text(),
        "---\r\ntitle: Old  # keep\r\ntags:\r\n  - a  # first\r\n  - c\r\n---\r\nBody\r\n"
    );
    assert_eq!(
        editor.items("tags"),
        vec![
            FrontmatterValue::String("a".to_owned()),
            FrontmatterValue::String("c".to_owned()),
        ]
    );
}

#[test]
fn raw_yaml_containers_are_validated_and_never_coerced() {
    let text = "---\nmeta: {x: 1}\nmatrix: [[1, 2]]\n---\n";
    let mut editor = editor(
        vec![
            field(
                "meta",
                false,
                FieldKind::Record {
                    value: Box::new(FieldKind::Number),
                },
            ),
            field(
                "matrix",
                false,
                FieldKind::Array {
                    items: Box::new(FieldKind::Array {
                        items: Box::new(FieldKind::Integer),
                    }),
                },
            ),
        ],
        text,
    );

    // Flow containers the typed controls cannot express are edited as validated
    // raw YAML; they are never coerced into a typed value.
    assert!(matches!(
        editor.control("meta").unwrap().kind,
        ControlKind::RawYaml { .. }
    ));
    assert!(editor.control("meta").unwrap().editable);
    assert_eq!(editor.raw("meta"), Some("{x: 1}"));
    assert!(matches!(
        editor.control("matrix").unwrap().kind,
        ControlKind::RawYaml { .. }
    ));
    assert!(editor.control("matrix").unwrap().editable);
    assert_eq!(editor.raw("matrix"), Some("[[1, 2]]"));

    editor.set_value("meta", "{y: 2}").unwrap();
    assert_eq!(editor.raw("meta"), Some("{y: 2}"));
    assert_eq!(
        editor.value("meta"),
        None,
        "a replaced container is never coerced into a scalar"
    );

    let before = editor.text().to_owned();
    for invalid in [
        "{y: 1}\nz: 2",
        "&anchor {y: 1}",
        "*alias",
        "!tag {y: 1}",
        "|",
        "[y",
    ] {
        let error = editor.set_value("meta", invalid).unwrap_err();
        assert!(!error.is_empty(), "{invalid:?} must be refused");
        assert_eq!(
            editor.text(),
            before,
            "{invalid:?} must not change the document"
        );
    }
    editor.set_value("matrix", "[[3]]").unwrap();
    assert_eq!(editor.raw("matrix"), Some("[[3]]"));
    assert_eq!(editor.value("matrix"), None);
}

#[test]
fn empty_frontmatter_values_are_editable_even_when_required() {
    let text = "---\ntitle:  # needs a value\nslug:\n---\n";
    let mut editor = editor(
        vec![
            field("title", true, FieldKind::String { format: None }),
            field("slug", false, FieldKind::String { format: None }),
        ],
        text,
    );
    assert!(editor.control("title").unwrap().editable);
    assert!(editor.control("title").unwrap().present);

    let updated = editor.set_value("title", "A value").unwrap();
    assert_eq!(
        updated,
        "---\ntitle: A value  # needs a value\nslug:\n---\n"
    );
    assert_eq!(
        editor.value("title"),
        Some(FrontmatterValue::String("A value".to_owned()))
    );

    // A required field with an empty value can be fixed even though it cannot be
    // removed.
    assert!(editor.remove_field("title").is_err());
    editor.set_value("slug", "a-slug").unwrap();
    assert_eq!(
        editor.value("slug"),
        Some(FrontmatterValue::String("a-slug".to_owned()))
    );
}

#[test]
fn absent_fields_insert_a_frontmatter_block_when_none_exists() {
    let mut editor = editor(
        vec![
            field("title", true, FieldKind::String { format: None }),
            field(
                "tags",
                false,
                FieldKind::Array {
                    items: Box::new(FieldKind::String { format: None }),
                },
            ),
        ],
        "# Heading\n\nBody\n",
    );
    editor.set_value("title", "Hello").unwrap();
    let text = editor.add_item("tags", "a,b").unwrap();
    assert_eq!(
        text,
        "---\ntitle: Hello\ntags: ['a,b']\n---\n\n# Heading\n\nBody\n"
    );
    assert_eq!(
        editor.items("tags"),
        vec![FrontmatterValue::String("a,b".to_owned())],
        "the inserted flow item must not split at the comma"
    );
}

#[test]
fn null_list_field_accepts_its_first_item() {
    for source in ["---\ntags:\n---\nBody\n", "---\ntags: null\n---\nBody\n"] {
        let mut editor = editor(
            vec![field(
                "tags",
                false,
                FieldKind::Array {
                    items: Box::new(FieldKind::String { format: None }),
                },
            )],
            source,
        );
        let text = editor.add_item("tags", "first").unwrap();
        assert_eq!(text, "---\ntags: [first]\n---\nBody\n", "{source:?}");
        assert_eq!(
            editor.items("tags"),
            vec![FrontmatterValue::String("first".to_owned())],
            "{source:?}"
        );
    }
}

#[test]
fn sequence_display_keeps_true_indices_for_null_items() {
    let editor = editor(
        vec![field(
            "tags",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::String { format: None }),
            },
        )],
        "---\ntags: [a, null, b]\n---\n",
    );
    let display: Vec<(usize, String)> = sequence_display(&editor, "tags")
        .into_iter()
        .map(|(index, item)| (index, item.to_string()))
        .collect();
    assert_eq!(
        display,
        vec![(0, "a".to_owned()), (2, "b".to_owned())],
        "a hidden null item must not shift the indices of later chips"
    );
    let mut editor = editor;
    editor.remove_item("tags", 2).unwrap();
    assert_eq!(
        editor.text(),
        "---\ntags: [a, null]\n---\n",
        "removing the chip for `b` must remove `b`, not the null item"
    );
}

#[test]
fn dropdown_labels_show_values_outside_the_schema() {
    let options = vec![(SharedString::from("Draft"), SharedString::from("draft"))];
    assert_eq!(selected_option_label(&options, "draft"), "Draft");
    assert_eq!(
        selected_option_label(&options, "archived"),
        "archived",
        "an out-of-schema value must stay visible"
    );
    assert_eq!(selected_option_label(&options, ""), "Select…");
}

#[test]
fn malformed_frontmatter_is_rejected_at_construction() {
    let result = FrontmatterEditor::new(
        Some(schema(vec![field(
            "title",
            false,
            FieldKind::String { format: None },
        )])),
        FieldMappings::default(),
        "---\ntitle: [unterminated\n---\n".to_owned(),
    );
    assert!(result.is_err());
}

#[gpui_kit::test]
async fn form_entity_renders_controls_and_applies_edits(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{WindowOptions, base::Root, test::TestWindowExt};

    struct Host {
        form: Entity<FrontmatterForm>,
        _sub: Subscription,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.form.clone()
        }
    }

    let events: Arc<Mutex<Vec<FrontmatterEdit>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = events.clone();

    let (handle, form) = cx.update(|cx| {
        gpui_kit::init(cx);
        let mut created = None;
        let (handle, _host) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let form = cx.new(FrontmatterForm::new);
            let schema = schema(vec![
                field("title", true, FieldKind::String { format: None }),
                field("draft", false, FieldKind::Boolean),
                field(
                    "tags",
                    false,
                    FieldKind::Array {
                        items: Box::new(FieldKind::String { format: None }),
                    },
                ),
            ]);
            form.update(cx, |form, cx| {
                form.set_document(
                    FrontmatterDocument {
                        schema: Some(schema),
                        mappings: FieldMappings::default(),
                        references: Arc::new(ReferenceCatalog::new()),
                        root: None,
                        source: None,
                        assets: None,
                        path_mode: astrolith_core::preferences::ImagePathMode::Relative,
                        text: "---\ntitle: Hi\ndraft: false\n---\n".to_owned(),
                    },
                    window,
                    cx,
                );
            });
            created = Some(form.clone());
            let subscriber = form.clone();
            let recorder = recorder.clone();
            cx.new(|cx| {
                let sub = cx.subscribe(
                    &subscriber,
                    move |_host, _form, event: &FrontmatterEdit, _cx| {
                        recorder.lock().unwrap().push(event.clone());
                    },
                );
                Host {
                    form: subscriber.clone(),
                    _sub: sub,
                }
            })
        })
        .unwrap();
        (handle.downcast::<Root>().unwrap(), created.unwrap())
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();

    let paths = form.read_with(cx, |form, _| form.control_paths());
    assert_eq!(paths, vec!["title", "draft", "tags"]);
    assert!(form.read_with(cx, |form, _| form.has_document()));

    form.update(cx, |form, cx| form.apply_value("title", "New", cx));
    assert!(
        form.read_with(cx, |form, _| form.editor().unwrap().text().to_owned())
            .contains("title: New")
    );
    // The form emits a targeted path edit, never a whole-document text.
    assert_eq!(
        events.lock().unwrap().as_slice(),
        &[FrontmatterEdit::Scalar {
            path: "title".to_owned(),
            value: FrontmatterValue::String("New".to_owned()),
            present: true,
        }]
    );

    // A list control refuses a whole-value replacement and emits nothing.
    form.update(cx, |form, cx| form.apply_value("tags", "[a]", cx));
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[gpui_kit::test]
async fn copy_as_slug_copies_without_touching_the_document(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{WindowOptions, base::Root, test::TestWindowExt};

    struct Host {
        form: Entity<FrontmatterForm>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.form.clone()
        }
    }

    let text = "---\ntitle: Hello, World!\ndescription: Keep me\n---\n\n# Body\n";
    let (handle, form) = cx.update(|cx| {
        gpui_kit::init(cx);
        let mut created = None;
        let (handle, _host) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let form = cx.new(FrontmatterForm::new);
            form.update(cx, |form, cx| {
                form.set_document(
                    FrontmatterDocument {
                        schema: Some(schema(vec![
                            field("title", true, FieldKind::String { format: None }),
                            field("description", false, FieldKind::String { format: None }),
                            field("slug", false, FieldKind::String { format: None }),
                        ])),
                        mappings: FieldMappings::default(),
                        references: Arc::new(ReferenceCatalog::new()),
                        root: None,
                        source: None,
                        assets: None,
                        path_mode: astrolith_core::preferences::ImagePathMode::Relative,
                        text: text.to_owned(),
                    },
                    window,
                    cx,
                );
            });
            created = Some(form.clone());
            cx.new(|_| Host { form: form.clone() })
        })
        .unwrap();
        (handle.downcast::<Root>().unwrap(), created.unwrap())
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();

    form.update(cx, |form, cx| form.copy_slug(cx));
    let copied = cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(copied.as_deref(), Some("hello-world"));
    assert_eq!(
        form.read_with(cx, |form, _| form.editor().unwrap().text().to_owned()),
        text,
        "copy as slug must not edit the document"
    );
}

#[gpui_kit::test]
async fn form_renders_the_typed_control_for_every_family(cx: &mut gpui_kit::TestAppContext) {
    use astrolith_workspace::ReferenceOption;
    use gpui_kit::{WindowOptions, base::Root, test::TestWindowExt};

    struct Host {
        form: Entity<FrontmatterForm>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.form.clone()
        }
    }

    let mut catalog = ReferenceCatalog::new();
    catalog.insert(
        "authors".to_owned(),
        vec![ReferenceOption {
            value: "ada".to_owned(),
            label: "Ada Lovelace".to_owned(),
        }],
    );
    let text = concat!(
        "---\n",
        "title: T\n",
        "score: 1.5\n",
        "count: 2\n",
        "draft: false\n",
        "published: 2024-01-01\n",
        "cover: images/cover.png\n",
        "author: ada\n",
        "related:\n  - ada\n",
        "tags: [a, b]\n",
        "status: archived\n",
        "seo:\n  socialImage: images/social.png\n",
        "meta: {a: b}\n",
        "fixed: x\n",
        "---\n",
    );
    let schema = schema(vec![
        field("title", true, FieldKind::String { format: None }),
        field("score", false, FieldKind::Number),
        field("count", false, FieldKind::Integer),
        field("draft", false, FieldKind::Boolean),
        field("published", false, FieldKind::Date),
        field("cover", false, FieldKind::Image),
        field(
            "author",
            true,
            FieldKind::Reference {
                collections: vec!["authors".to_owned()],
            },
        ),
        field(
            "related",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::Reference {
                    collections: vec!["authors".to_owned()],
                }),
            },
        ),
        field(
            "tags",
            false,
            FieldKind::Array {
                items: Box::new(FieldKind::String { format: None }),
            },
        ),
        field(
            "status",
            false,
            FieldKind::Enum {
                values: vec![Value::String("draft".into()), Value::String("live".into())],
            },
        ),
        field(
            "seo",
            false,
            FieldKind::Object {
                fields: vec![field("socialImage", false, FieldKind::Image)],
            },
        ),
        field(
            "meta",
            false,
            FieldKind::Record {
                value: Box::new(FieldKind::String { format: None }),
            },
        ),
        field(
            "fixed",
            false,
            FieldKind::Literal {
                value: Value::String("x".into()),
            },
        ),
    ]);

    let (handle, form) = cx.update(|cx| {
        gpui_kit::init(cx);
        let mut created = None;
        let (handle, _host) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let form = cx.new(FrontmatterForm::new);
            form.update(cx, |form, cx| {
                form.set_document(
                    FrontmatterDocument {
                        schema: Some(schema),
                        mappings: FieldMappings::default(),
                        references: Arc::new(catalog),
                        root: None,
                        source: None,
                        assets: None,
                        path_mode: astrolith_core::preferences::ImagePathMode::Relative,
                        text: text.to_owned(),
                    },
                    window,
                    cx,
                );
            });
            created = Some(form.clone());
            cx.new(|_| Host { form: form.clone() })
        })
        .unwrap();
        (handle.downcast::<Root>().unwrap(), created.unwrap())
    });

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let paths = form.read_with(cx, |form, _| form.control_paths());
        assert_eq!(
            paths,
            vec![
                "title",
                "score",
                "count",
                "draft",
                "published",
                "cover",
                "author",
                "related",
                "tags",
                "status",
                "seo",
                "seo.socialImage",
                "meta",
                "fixed",
            ]
        );
        let kinds = form.read_with(cx, |form, _| form.control_kinds());
        assert_eq!(
            kinds,
            vec![
                "text",
                "number",
                "integer",
                "boolean",
                "date",
                "image",
                "reference",
                "reference list",
                "tags",
                "enum",
                "object",
                "image",
                "raw yaml",
                "literal",
            ],
            "every schema family must keep its own control"
        );
        let index = |path: &str| {
            paths
                .iter()
                .position(|candidate| candidate == path)
                .unwrap()
        };

        for image in ["cover", "seo.socialImage"] {
            assert!(
                window
                    .try_find(("frontmatter-image-pick", index(image)))
                    .is_some(),
                "the image control for {image} must render its picker"
            );
        }
        assert!(
            window
                .try_find(("frontmatter-reference", index("author")))
                .is_some(),
            "a single reference must render its labelled dropdown"
        );
        assert!(
            window
                .try_find(("frontmatter-reference-add", index("related")))
                .is_some(),
            "an array reference must render its add menu"
        );
        assert!(
            window
                .try_find(("frontmatter-bool", index("draft")))
                .is_some(),
            "a boolean must render its toggle"
        );
        assert!(
            window
                .try_find(("frontmatter-enum", index("status")))
                .is_some(),
            "an enum must render its dropdown"
        );
        assert!(
            window
                .try_find(("frontmatter-chip-remove", index("tags") * 64))
                .is_some(),
            "a tags control must render one removable chip per item"
        );
    })
    .unwrap();
}

#[test]
fn image_picks_reference_in_project_files_and_copy_external_ones() {
    let project = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(project.path().join("src/assets")).unwrap();
    std::fs::write(project.path().join("src/assets/local.png"), b"png").unwrap();
    std::fs::write(external.path().join("Outside Photo.PNG"), b"png").unwrap();
    let assets = AssetLocation::resolve(project.path(), None).unwrap();

    let local = resolve_image_pick(
        Some(project.path().to_path_buf()),
        Some("src/posts/a.md".into()),
        Some(assets.clone()),
        ImagePathMode::Relative,
        project.path().join("src/assets/local.png"),
    )
    .unwrap();
    assert_eq!(local, "../assets/local.png");

    let absolute = resolve_image_pick(
        Some(project.path().to_path_buf()),
        Some("src/posts/a.md".into()),
        Some(assets.clone()),
        ImagePathMode::Absolute,
        project.path().join("src/assets/local.png"),
    )
    .unwrap();
    assert_eq!(absolute, "/src/assets/local.png");

    let copied = resolve_image_pick(
        Some(project.path().to_path_buf()),
        Some("src/posts/a.md".into()),
        Some(assets),
        ImagePathMode::Relative,
        external.path().join("Outside Photo.PNG"),
    )
    .unwrap();
    assert!(copied.starts_with("../assets/") && copied.ends_with("-outside-photo.png"));
    assert!(
        copied.rsplit('/').next().is_some_and(|name| project
            .path()
            .join("src/assets")
            .join(name)
            .is_file())
    );

    assert!(
        resolve_image_pick(
            Some(project.path().to_path_buf()),
            Some("src/posts/a.md".into()),
            None,
            ImagePathMode::Relative,
            external.path().join("Outside Photo.PNG"),
        )
        .unwrap_err()
        .contains("assets directory")
    );
}

#[gpui_kit::test]
fn image_value_uses_the_manual_path_verbatim_and_clears(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{WindowOptions, base::Root, test::TestWindowExt};

    struct Host {
        form: Entity<FrontmatterForm>,
    }
    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.form.clone()
        }
    }

    let (handle, form) = cx.update(|cx| {
        gpui_kit::init(cx);
        let mut created = None;
        let (handle, _host) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            let form = cx.new(FrontmatterForm::new);
            form.update(cx, |form, cx| {
                form.set_document(
                    FrontmatterDocument {
                        schema: Some(schema(vec![field("cover", false, FieldKind::Image)])),
                        mappings: FieldMappings::default(),
                        references: Arc::new(ReferenceCatalog::new()),
                        root: None,
                        source: None,
                        assets: None,
                        path_mode: ImagePathMode::Relative,
                        text: "---\ncover: old.png\n---\n".to_owned(),
                    },
                    window,
                    cx,
                );
            });
            created = Some(form.clone());
            cx.new(|_| Host { form })
        })
        .unwrap();
        (handle.downcast::<Root>().unwrap(), created.unwrap())
    });
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
    })
    .unwrap();

    // A manually typed path is stored exactly as written, failures included.
    form.update(cx, |form, cx| {
        form.set_image_path("cover", "  ../weird path?.png  ".into(), cx)
    });
    assert!(
        form.read_with(cx, |form, _| form.editor().unwrap().text().to_owned())
            .contains("cover: '  ../weird path?.png  '")
    );

    form.update(cx, |form, cx| form.apply_remove_field("cover", cx));
    assert!(!form.read_with(cx, |form, _| {
        form.editor().unwrap().document.has_field("cover")
    }));
}
