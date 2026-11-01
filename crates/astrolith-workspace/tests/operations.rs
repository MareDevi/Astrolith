use astrolith_astro::schema::{CollectionSchema, Field, FieldKind, SchemaDialect};
use astrolith_core::{
    CollectionId, SourcePath,
    preferences::{FieldMappings, FileExtension},
};
use astrolith_workspace::operations::{
    self, CopyPathRequest, CreateRequest, CreationInputKind, DuplicateRequest, MoveRequest,
    NewEntryName, OperationError, PathForm, ProjectScope, RevealRequest, ScalarValue, TrashBackend,
    TrashRequest,
};
use std::{fs, path::Path, sync::Mutex};

fn project() -> (tempfile::TempDir, ProjectScope) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("src/posts/guides")).unwrap();
    fs::create_dir_all(root.join("src/other")).unwrap();
    let scope = ProjectScope::new(root, 7);
    (directory, scope)
}

fn create_request(directory: &str) -> CreateRequest {
    CreateRequest {
        generation: 7,
        collection: CollectionId::new("blog").unwrap(),
        directory: Some(SourcePath::new(directory).unwrap()),
        extension: FileExtension::Md,
        name: NewEntryName::Date("2026-11-01".into()),
        fields: vec![
            operations::CreationField {
                name: "title".into(),
                value: ScalarValue::String("New Post".into()),
            },
            operations::CreationField {
                name: "pubDate".into(),
                value: ScalarValue::String("2026-11-01".into()),
            },
        ],
    }
}

#[test]
fn create_writes_quoted_frontmatter_and_deduplicates_date_names() {
    let (directory, scope) = project();
    let root = directory.path();
    let first = operations::create(&scope, &create_request("src/posts")).unwrap();
    assert_eq!(first.source().as_str(), "src/posts/2026-11-01.md");
    assert_eq!(
        fs::read_to_string(first.absolute()).unwrap(),
        "---\ntitle: 'New Post'\npubDate: '2026-11-01'\n---\n\n"
    );
    let second = operations::create(&scope, &create_request("src/posts")).unwrap();
    assert_eq!(second.source().as_str(), "src/posts/2026-11-01-1.md");
    let third = operations::create(&scope, &create_request("src/posts")).unwrap();
    assert_eq!(third.source().as_str(), "src/posts/2026-11-01-2.md");

    // A nested subdirectory of the collection is honoured.
    let nested = operations::create(&scope, &create_request("src/posts/guides")).unwrap();
    assert_eq!(nested.source().as_str(), "src/posts/guides/2026-11-01.md");
    assert!(root.join(nested.source().as_str()).is_file());
}

#[test]
fn create_and_rename_at_the_project_root_use_no_directory() {
    let (directory, scope) = project();
    let root = directory.path();
    let mut request = create_request("src/posts");
    request.directory = None;
    request.name = NewEntryName::Explicit("root-entry".into());
    let created = operations::create(&scope, &request).unwrap();
    assert_eq!(created.source().as_str(), "root-entry.md");
    assert_eq!(
        fs::read_to_string(root.join("root-entry.md")).unwrap(),
        "---\ntitle: 'New Post'\npubDate: '2026-11-01'\n---\n\n"
    );
    let moved = operations::rename(
        &scope,
        &MoveRequest {
            generation: 7,
            source: created.source().clone(),
            destination: SourcePath::new("renamed-root.md").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(moved.to().as_str(), "renamed-root.md");
    assert!(root.join("renamed-root.md").is_file());
    assert!(!root.join("root-entry.md").exists());
}

#[test]
fn create_without_fields_writes_an_empty_entry() {
    let (_directory, scope) = project();
    let mut request = create_request("src/posts");
    request.fields.clear();
    let created = operations::create(&scope, &request).unwrap();
    assert_eq!(fs::read_to_string(created.absolute()).unwrap(), "");
}

#[test]
fn create_refuses_collisions_for_explicit_names_and_unsafe_names() {
    let (directory, scope) = project();
    let root = directory.path();
    let mut request = create_request("src/posts");
    request.name = NewEntryName::Explicit("hello".into());
    operations::create(&scope, &request).unwrap();
    assert_eq!(
        operations::create(&scope, &request).unwrap_err(),
        OperationError::Collision(root.join("src/posts/hello.md"))
    );
    for name in [
        "",
        "..",
        ".hidden",
        "a/b",
        "a\\b",
        "a:b",
        "trailing.",
        "trailing ",
    ] {
        let mut request = create_request("src/posts");
        request.name = NewEntryName::Explicit(name.into());
        assert!(
            matches!(
                operations::create(&scope, &request),
                Err(OperationError::InvalidName(_))
            ),
            "{name:?} must be refused"
        );
    }
    let mut request = create_request("src/posts");
    request.name = NewEntryName::Date("2026-02-30".into());
    assert!(matches!(
        operations::create(&scope, &request),
        Err(OperationError::InvalidName(_))
    ));
}

#[test]
fn create_and_rename_refuse_traversal_and_symlink_escapes() {
    let (directory, scope) = project();
    let root = directory.path();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.join("src/posts/2026-11-01.md"), "# Entry\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("src/escape")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("x.md"), root.join("src/posts/link.md"))
        .unwrap();

    let mut request = create_request("src/escape");
    request.directory = Some(SourcePath::new("src/escape").unwrap());
    assert!(matches!(
        operations::create(&scope, &request),
        Err(OperationError::Symlink(_))
    ));

    let traversal = SourcePath::new("src/../outside").unwrap_err();
    let _ = traversal;

    let mut rename = MoveRequest {
        generation: 7,
        source: SourcePath::new("src/posts/link.md").unwrap(),
        destination: SourcePath::new("src/other/link.md").unwrap(),
    };
    assert!(matches!(
        operations::rename(&scope, &rename),
        Err(OperationError::Symlink(_))
    ));
    rename.source = SourcePath::new("src/posts/2026-11-01.md").unwrap();
    rename.destination = SourcePath::new("src/escape/x.md").unwrap();
    assert!(matches!(
        operations::rename(&scope, &rename),
        Err(OperationError::Symlink(_))
    ));
}

#[test]
fn duplicate_preserves_bytes_and_derives_copy_names() {
    let (directory, scope) = project();
    let root = directory.path();
    let original = root.join("src/posts/entry.md");
    let bytes = "---\r\ntitle: '日本語'\r\n---\r\n\r\n# Body 🦀\r\n"
        .as_bytes()
        .to_vec();
    fs::write(&original, &bytes).unwrap();

    let first = operations::duplicate(
        &scope,
        &DuplicateRequest {
            generation: 7,
            source: SourcePath::new("src/posts/entry.md").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(first.source().as_str(), "src/posts/entry-copy.md");
    assert_eq!(fs::read(first.absolute()).unwrap(), bytes);
    assert_eq!(fs::read(&original).unwrap(), bytes);

    let second = operations::duplicate(
        &scope,
        &DuplicateRequest {
            generation: 7,
            source: SourcePath::new("src/posts/entry.md").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(second.source().as_str(), "src/posts/entry-copy-2.md");
    assert_eq!(fs::read(second.absolute()).unwrap(), bytes);
}

#[test]
fn rename_moves_without_rewriting_content_and_refuses_collisions() {
    let (directory, scope) = project();
    let root = directory.path();
    let source = "---\ntitle: 'Keep me'\n---\n\n# Unchanged 中文\n";
    fs::write(root.join("src/posts/entry.md"), source).unwrap();
    fs::write(root.join("src/other/occupied.md"), "other").unwrap();

    let moved = operations::rename(
        &scope,
        &MoveRequest {
            generation: 7,
            source: SourcePath::new("src/posts/entry.md").unwrap(),
            destination: SourcePath::new("src/posts/guides/renamed.mdx").unwrap(),
        },
    )
    .unwrap();
    assert_eq!(moved.from().as_str(), "src/posts/entry.md");
    assert_eq!(moved.to().as_str(), "src/posts/guides/renamed.mdx");
    assert!(!root.join("src/posts/entry.md").exists());
    assert_eq!(fs::read_to_string(moved.absolute()).unwrap(), source);

    let collision = operations::rename(
        &scope,
        &MoveRequest {
            generation: 7,
            source: SourcePath::new("src/posts/guides/renamed.mdx").unwrap(),
            destination: SourcePath::new("src/other/occupied.md").unwrap(),
        },
    );
    assert_eq!(
        collision.unwrap_err(),
        OperationError::Collision(root.join("src/other/occupied.md"))
    );
    assert_eq!(
        fs::read_to_string(root.join("src/other/occupied.md")).unwrap(),
        "other"
    );
}

#[test]
fn trash_uses_only_the_backend_and_reports_its_failure() {
    struct Recording {
        fail: bool,
        trashed: Mutex<Option<std::path::PathBuf>>,
    }
    impl TrashBackend for Recording {
        fn trash(&self, path: &Path) -> Result<(), String> {
            if self.fail {
                return Err("no trash directory".into());
            }
            *self.trashed.lock().unwrap() = Some(path.to_path_buf());
            Ok(())
        }
    }

    let (directory, scope) = project();
    let root = directory.path();
    fs::write(root.join("src/posts/entry.md"), "# Entry\n").unwrap();
    let request = TrashRequest {
        generation: 7,
        source: SourcePath::new("src/posts/entry.md").unwrap(),
    };

    let failing = Recording {
        fail: true,
        trashed: Mutex::new(None),
    };
    let error = operations::trash(&scope, &request, &failing).unwrap_err();
    assert!(matches!(error, OperationError::TrashFailed { .. }));
    assert!(
        root.join("src/posts/entry.md").is_file(),
        "a refused trash must never fall back to permanent deletion"
    );

    let succeeding = Recording {
        fail: false,
        trashed: Mutex::new(None),
    };
    let trashed = operations::trash(&scope, &request, &succeeding).unwrap();
    assert_eq!(trashed.source().as_str(), "src/posts/entry.md");
    assert_eq!(
        succeeding.trashed.lock().unwrap().take(),
        Some(fs::canonicalize(root.join("src/posts/entry.md")).unwrap())
    );
}

#[test]
fn reveal_and_copy_path_expose_the_containing_directory_and_both_forms() {
    let (directory, scope) = project();
    let root = fs::canonicalize(directory.path()).unwrap();
    fs::write(root.join("src/posts/guides/entry.md"), "# Entry\n").unwrap();
    let source = SourcePath::new("src/posts/guides/entry.md").unwrap();

    let reveal = operations::reveal_target(
        &scope,
        &RevealRequest {
            generation: 7,
            source: source.clone(),
        },
    )
    .unwrap();
    assert_eq!(reveal.directory(), root.join("src/posts/guides"));

    let relative = operations::copy_path(
        &scope,
        &CopyPathRequest {
            generation: 7,
            source: source.clone(),
            form: PathForm::Relative,
        },
    )
    .unwrap();
    assert_eq!(relative.text(), "src/posts/guides/entry.md");
    let absolute = operations::copy_path(
        &scope,
        &CopyPathRequest {
            generation: 7,
            source,
            form: PathForm::Absolute,
        },
    )
    .unwrap();
    assert_eq!(
        absolute.text(),
        root.join("src/posts/guides/entry.md").to_string_lossy()
    );
}

#[test]
fn stale_generations_are_refused_before_any_io() {
    let (directory, scope) = project();
    let root = directory.path();
    let mut request = create_request("src/posts");
    request.generation = 6;
    assert_eq!(
        operations::create(&scope, &request).unwrap_err(),
        OperationError::StaleGeneration {
            request: 6,
            current: 7
        }
    );
    assert!(!root.join("src/posts/2026-11-01.md").exists());
}

fn schema(fields: Vec<Field>) -> CollectionSchema {
    CollectionSchema {
        dialect: SchemaDialect::Draft202012,
        fields,
        diagnostics: Vec::new(),
    }
}

fn field(name: &str, required: bool, kind: FieldKind) -> Field {
    Field {
        name: name.into(),
        required,
        default: None,
        kind,
    }
}

#[test]
fn creation_plan_generates_required_defaults_and_reports_the_rest() {
    let schema = schema(vec![
        field("title", true, FieldKind::String { format: None }),
        field("pubDate", true, FieldKind::Date),
        Field {
            default: Some(serde_json::json!(3)),
            ..field("weight", true, FieldKind::Integer)
        },
        field("draft", false, FieldKind::Boolean),
        field("subtitle", true, FieldKind::String { format: None }),
        field(
            "cover",
            true,
            FieldKind::Reference {
                collections: vec!["assets".into()],
            },
        ),
        field(
            "tags",
            true,
            FieldKind::Array {
                items: Box::new(FieldKind::String { format: None }),
            },
        ),
    ]);
    let mappings = FieldMappings::default();
    let plan = operations::creation_plan(Some(&schema), "posts", &mappings, "2026-11-01");
    assert_eq!(
        plan.fields,
        vec![
            operations::CreationField {
                name: "title".into(),
                value: ScalarValue::String("New Post".into())
            },
            operations::CreationField {
                name: "pubDate".into(),
                value: ScalarValue::String("2026-11-01".into())
            },
            operations::CreationField {
                name: "weight".into(),
                value: ScalarValue::Integer(3)
            },
        ]
    );
    assert_eq!(plan.inputs.len(), 1);
    assert_eq!(plan.inputs[0].name, "subtitle");
    assert_eq!(plan.inputs[0].kind, CreationInputKind::Text);
    assert_eq!(
        plan.unsupported,
        vec!["cover".to_owned(), "tags".to_owned()]
    );
    assert!(!plan.writes_immediately());
}

#[test]
fn creation_plan_uses_mapped_fields_and_writes_immediately_when_complete() {
    let schema = schema(vec![
        field("headline", true, FieldKind::String { format: None }),
        field("published", true, FieldKind::Date),
    ]);
    let mappings = FieldMappings {
        title: Some("headline".into()),
        date: Some("published".into()),
        ..FieldMappings::default()
    };
    let plan = operations::creation_plan(Some(&schema), "guides", &mappings, "2026-11-01");
    assert_eq!(
        plan.fields,
        vec![
            operations::CreationField {
                name: "headline".into(),
                value: ScalarValue::String("New Guide".into())
            },
            operations::CreationField {
                name: "published".into(),
                value: ScalarValue::String("2026-11-01".into())
            },
        ]
    );
    assert!(plan.writes_immediately());

    let no_schema = operations::creation_plan(None, "guides", &mappings, "2026-11-01");
    assert!(no_schema.writes_immediately());
    assert!(no_schema.fields.is_empty());
}

#[test]
fn creation_inputs_parse_and_reject_invalid_values() {
    let text = operations::CreationInput {
        name: "subtitle".into(),
        kind: CreationInputKind::Text,
    };
    assert_eq!(
        operations::parse_input(&text, " Hello ").unwrap(),
        ScalarValue::String("Hello".into())
    );
    let number = operations::CreationInput {
        name: "weight".into(),
        kind: CreationInputKind::Number,
    };
    assert!(operations::parse_input(&number, "heavy").is_err());
    assert_eq!(
        operations::parse_input(&number, "2.5").unwrap(),
        ScalarValue::Real(2.5)
    );
    let date = operations::CreationInput {
        name: "published".into(),
        kind: CreationInputKind::Date,
    };
    assert_eq!(operations::input_text(&date, "2026-11-01"), "2026-11-01");
    assert!(operations::parse_input(&date, "11/01/2026").is_err());
    let choice = operations::CreationInput {
        name: "state".into(),
        kind: CreationInputKind::Enum(vec!["draft".into(), "live".into()]),
    };
    assert_eq!(
        operations::parse_input(&choice, "live").unwrap(),
        ScalarValue::String("live".into())
    );
    assert!(operations::parse_input(&choice, "other").is_err());
}
