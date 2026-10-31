use std::sync::{Arc, Mutex};

use astrolith_astro::schema::{CollectionSchema, Field, FieldKind, SchemaDialect};

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
    }
}

fn editor(fields: Vec<Field>, text: &str) -> FrontmatterEditor {
    FrontmatterEditor::new(schema(fields), text.to_owned()).expect("valid document")
}

#[test]
fn maps_every_field_kind_to_its_native_control() {
    use ControlKind::*;
    let controls = fields::build_controls(&[
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
        matches!(by_path("author").kind, Reference { ref collections } if collections == &["authors".to_owned()])
    );
    assert_eq!(by_path("tags").kind, Array);
    assert_eq!(by_path("meta").kind, Record);

    // Scalars are editable; containers/unsupported/literal are read-only with a reason.
    for path in [
        "title",
        "email",
        "score",
        "count",
        "draft",
        "published",
        "cover",
        "author",
    ] {
        assert!(by_path(path).editable, "{path} must be editable");
    }
    assert!(!by_path("version").editable);
    assert!(by_path("version").refusal.is_some());
    assert!(!by_path("tags").editable);
    assert!(
        by_path("tags")
            .refusal
            .as_deref()
            .unwrap()
            .contains("array")
    );
    assert!(!by_path("meta").editable);
    assert!(!by_path("union").editable);
    assert_eq!(
        by_path("union").refusal.as_deref(),
        Some("heterogeneous union"),
        "unsupported unions surface their explicit refusal reason"
    );
}

#[test]
fn object_fields_emit_a_container_and_editable_children() {
    let controls = fields::build_controls(&[field(
        "author",
        true,
        FieldKind::Object {
            fields: vec![
                field("name", true, FieldKind::String { format: None }),
                field("age", false, FieldKind::Integer),
            ],
        },
    )]);
    let paths: Vec<&str> = controls.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["author", "author.name", "author.age"]);
    assert_eq!(controls[0].kind, ControlKind::Object);
    assert!(!controls[0].editable);
    assert!(controls[1].editable && controls[2].editable);
    assert_eq!(controls[1].control_label(), "text");
}

#[test]
fn nullable_scalars_stay_editable_and_flag_nullable() {
    let controls = fields::build_controls(&[field(
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
        ("tags", "array"),
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
    let controls = fields::build_controls(&[field(
        "value",
        false,
        FieldKind::Unsupported {
            reason: "heterogeneous union [string, number]".to_owned(),
        },
    )]);
    assert_eq!(
        controls[0].kind,
        ControlKind::Unsupported {
            reason: "heterogeneous union [string, number]".to_owned()
        }
    );
    assert!(!controls[0].editable);
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
fn malformed_frontmatter_is_rejected_at_construction() {
    let result = FrontmatterEditor::new(
        schema(vec![field(
            "title",
            false,
            FieldKind::String { format: None },
        )]),
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

    let events: Arc<Mutex<Vec<(String, FrontmatterValue)>>> = Arc::new(Mutex::new(Vec::new()));
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
                    schema,
                    "---\ntitle: Hi\ndraft: false\n---\n".to_owned(),
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
                    move |_host, _form, event: &FrontmatterChanged, _cx| {
                        recorder
                            .lock()
                            .unwrap()
                            .push((event.path.clone(), event.value.clone()));
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
    // The form emits a targeted (path, value) change, never a whole-document text.
    assert_eq!(
        events.lock().unwrap().as_slice(),
        &[(
            "title".to_owned(),
            FrontmatterValue::String("New".to_owned())
        )]
    );

    // A read-only field emits nothing.
    form.update(cx, |form, cx| form.apply_value("tags", "[a]", cx));
    assert_eq!(events.lock().unwrap().len(), 1);
}
