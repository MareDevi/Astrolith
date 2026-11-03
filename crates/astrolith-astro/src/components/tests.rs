use std::path::{Path, PathBuf};

use crate::discovery::Cancellation;

use super::*;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/components/project")
}

fn fixture_file(relative: &str) -> String {
    std::fs::read_to_string(fixture_root().join(relative))
        .unwrap_or_else(|error| panic!("read {relative}: {error}"))
}

fn expected_file(kind: &str, name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/components/expected")
            .join(kind)
            .join(name),
    )
    .unwrap_or_else(|error| panic!("read expected {kind}/{name}: {error}"))
}

fn index() -> ComponentIndex {
    inspect_components(&ComponentIndexRequest::new(
        fixture_root(),
        7,
        Cancellation::default(),
    ))
    .expect("fixture index")
}

fn project<'a>(index: &'a ComponentIndex, relative: &str) -> &'a Component {
    index
        .components()
        .iter()
        .find(|component| {
            component.project_source().is_some_and(|source| {
                source
                    .strip_prefix(fixture_root().canonicalize().unwrap())
                    .is_ok_and(|path| path == Path::new(relative))
            })
        })
        .unwrap_or_else(|| panic!("project component {relative}"))
}

fn package<'a>(index: &'a ComponentIndex, name: &str) -> &'a Component {
    index
        .components()
        .iter()
        .find(|component| component.name == name && component.package_specifier().is_some())
        .unwrap_or_else(|| panic!("package component {name}"))
}

fn prop<'a>(component: &'a Component, name: &str) -> &'a Prop {
    component
        .prop(name)
        .unwrap_or_else(|| panic!("prop {name} on {}", component.name))
}

#[test]
fn discovery_lists_frameworks_skips_index_and_keeps_parse_failures() {
    let index = index();
    assert_eq!(index.generation(), 7);
    assert!(index.starlight());
    assert!(index.auto_import());
    assert!(
        index
            .diagnostics()
            .iter()
            .any(|message| message.contains("astro-auto-import"))
    );
    assert!(
        index
            .components()
            .iter()
            .all(|component| !component.id().contains("index.astro"))
    );
    for (relative, framework) in [
        ("src/components/Callout.astro", Framework::Astro),
        ("src/components/Alert.tsx", Framework::React),
        ("src/components/Toggle.jsx", Framework::React),
        ("src/components/Badge.vue", Framework::Vue),
        ("src/components/Tag.svelte", Framework::Svelte),
    ] {
        assert_eq!(project(&index, relative).framework, framework, "{relative}");
    }
    let broken = project(&index, "src/components/Broken.astro");
    assert!(broken.props.is_empty());
    assert!(
        index
            .diagnostics()
            .iter()
            .any(|message| message.contains("Could not parse"))
    );
    let duplicates = index
        .components()
        .iter()
        .filter(|component| component.name == "Duplicate")
        .count();
    assert_eq!(duplicates, 2);
    let cards = index
        .components()
        .iter()
        .filter(|component| component.name == "Card")
        .count();
    assert_eq!(cards, 2);
    assert!(index.clone().accept_for_generation(7).is_ok());
    assert!(matches!(
        index.accept_for_generation(8),
        Err(ComponentIndexError::StaleGeneration {
            result: 7,
            current: 8
        })
    ));

    let cancelled = Cancellation::default();
    cancelled.cancel();
    assert!(matches!(
        inspect_components(&ComponentIndexRequest::new(fixture_root(), 1, cancelled,)),
        Err(ComponentIndexError::Cancelled)
    ));
}

#[test]
fn astro_props_cover_docs_defaults_unions_arrays_images_entries_dates_and_slots() {
    let index = index();
    let callout = project(&index, "src/components/Callout.astro");
    assert_eq!(
        callout.description.as_deref(),
        Some("A framed callout with an optional named header. It never runs project code.")
    );
    assert_eq!(
        callout
            .props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        [
            "title", "tone", "level", "count", "open", "updated", "tags", "counts", "cover",
            "related"
        ]
    );
    assert_eq!(prop(callout, "title").kind, PropKind::String);
    assert!(prop(callout, "title").required);
    assert_eq!(
        prop(callout, "title").description.as_deref(),
        Some("Callout heading.")
    );
    assert_eq!(
        prop(callout, "tone").kind,
        PropKind::Enum {
            values: vec!["note".into(), "tip".into(), "danger".into()]
        }
    );
    assert_eq!(
        prop(callout, "tone").default,
        Some(PropDefault::String("note".into()))
    );
    assert_eq!(
        prop(callout, "level").kind,
        PropKind::Enum {
            values: vec!["low".into(), "high".into()]
        }
    );
    assert_eq!(prop(callout, "count").kind, PropKind::Number);
    assert_eq!(
        prop(callout, "count").default,
        Some(PropDefault::Number("0".into()))
    );
    assert_eq!(prop(callout, "open").kind, PropKind::Boolean);
    assert_eq!(
        prop(callout, "open").default,
        Some(PropDefault::Boolean(true))
    );
    assert_eq!(prop(callout, "updated").kind, PropKind::Date);
    assert_eq!(prop(callout, "tags").kind, PropKind::StringArray);
    assert_eq!(
        prop(callout, "tags").default,
        Some(PropDefault::List(Vec::new()))
    );
    assert_eq!(prop(callout, "counts").kind, PropKind::NumberArray);
    assert_eq!(prop(callout, "cover").kind, PropKind::Image);
    assert_eq!(
        prop(callout, "related").kind,
        PropKind::CollectionEntry {
            collection: "blog".into()
        }
    );
    assert!(callout.has_default_slot());
    assert!(
        callout
            .slots
            .iter()
            .any(|slot| slot.name.as_deref() == Some("header"))
    );
    assert!(callout.client_directives.is_empty());
}

#[test]
fn react_props_cover_parameter_fc_and_inferred_jsx_types() {
    let index = index();
    let alert = project(&index, "src/components/Alert.tsx");
    assert_eq!(
        alert
            .props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["level", "count", "dismissible"]
    );
    assert_eq!(
        prop(alert, "level").kind,
        PropKind::Enum {
            values: vec!["info".into(), "warn".into(), "error".into()]
        }
    );
    assert_eq!(
        prop(alert, "level").default,
        Some(PropDefault::String("info".into()))
    );
    assert_eq!(
        prop(alert, "level").description.as_deref(),
        Some("Visual severity of the alert.")
    );
    assert_eq!(prop(alert, "count").kind, PropKind::Number);
    assert_eq!(prop(alert, "dismissible").kind, PropKind::Boolean);
    assert_eq!(
        prop(alert, "dismissible").default,
        Some(PropDefault::Boolean(false))
    );
    assert!(alert.has_default_slot());
    assert_eq!(alert.client_directives.len(), 5);

    let button = project(&index, "src/components/Button.tsx");
    assert_eq!(
        button
            .props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["label", "disabled"]
    );
    assert!(prop(button, "label").required);
    assert_eq!(
        prop(button, "disabled").default,
        Some(PropDefault::Boolean(false))
    );

    let toggle = project(&index, "src/components/Toggle.jsx");
    assert_eq!(
        toggle
            .props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["checked", "label", "onChange"]
    );
    assert!(prop(toggle, "checked").optional());
    assert!(prop(toggle, "label").required);
    assert_eq!(prop(toggle, "label").kind, PropKind::String);
    assert_eq!(prop(toggle, "onChange").kind, PropKind::String);
}

#[test]
fn vue_and_svelte_props_cover_define_props_defaults_and_props_call() {
    let index = index();
    let badge = project(&index, "src/components/Badge.vue");
    assert_eq!(
        badge
            .props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["label", "tone", "count", "tags"]
    );
    assert!(prop(badge, "label").required);
    assert_eq!(
        prop(badge, "tone").kind,
        PropKind::Enum {
            values: vec!["neutral".into(), "accent".into()]
        }
    );
    assert_eq!(
        prop(badge, "tone").default,
        Some(PropDefault::String("neutral".into()))
    );
    assert_eq!(
        prop(badge, "count").default,
        Some(PropDefault::Number("0".into()))
    );
    assert_eq!(prop(badge, "tags").kind, PropKind::StringArray);
    assert_eq!(
        prop(badge, "tags").default,
        Some(PropDefault::List(Vec::new()))
    );
    assert!(badge.has_default_slot());

    let tag = project(&index, "src/components/Tag.svelte");
    assert_eq!(
        tag.props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["label", "tone", "count"]
    );
    assert!(prop(tag, "label").required);
    assert_eq!(
        prop(tag, "label").description.as_deref(),
        Some("Badge label.")
    );
    assert_eq!(
        prop(tag, "tone").default,
        Some(PropDefault::String("neutral".into()))
    );
    assert_eq!(prop(tag, "count").kind, PropKind::Number);
    assert!(tag.has_default_slot());

    let pill = project(&index, "src/components/Pill.vue");
    assert_eq!(
        pill.props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["text", "tone", "count"]
    );
    assert!(prop(pill, "text").required);
    assert_eq!(prop(pill, "text").kind, PropKind::String);
    assert_eq!(
        prop(pill, "text").description.as_deref(),
        Some("Pill text.")
    );
    assert_eq!(
        prop(pill, "tone").default,
        Some(PropDefault::String("neutral".into()))
    );
    assert_eq!(prop(pill, "count").kind, PropKind::Number);
    assert!(prop(pill, "count").optional());
    assert!(pill.has_default_slot());

    let widget = project(&index, "src/components/Widget.svelte");
    assert_eq!(
        widget
            .props
            .iter()
            .map(|prop| prop.name.as_str())
            .collect::<Vec<_>>(),
        ["title", "count", "hint"]
    );
    assert!(prop(widget, "title").required);
    assert_eq!(
        prop(widget, "title").description.as_deref(),
        Some("Widget title.")
    );
    assert_eq!(
        prop(widget, "count").default,
        Some(PropDefault::Number("1".into()))
    );
    assert!(prop(widget, "hint").optional());
    assert_eq!(prop(widget, "hint").kind, PropKind::String);
    assert!(widget.has_default_slot());
}

#[test]
fn unknown_types_become_strings_and_relative_types_are_followed() {
    let index = index();
    let callout = project(&index, "src/components/Callout.astro");
    // `Tone` and `Level` come from ./types; an unresolved ReactNode has no control.
    assert!(matches!(prop(callout, "tone").kind, PropKind::Enum { .. }));
    assert!(matches!(prop(callout, "level").kind, PropKind::Enum { .. }));
    let alert = project(&index, "src/components/Alert.tsx");
    assert!(alert.prop("children").is_none());
    let tag = project(&index, "src/components/Tag.svelte");
    assert_eq!(prop(tag, "label").kind, PropKind::String);
}

#[test]
fn starlight_catalog_loads_only_when_declared() {
    let index = index();
    let aside = package(&index, "Aside");
    assert_eq!(aside.framework, Framework::Astro);
    assert_eq!(
        aside.package_specifier(),
        Some("@astrojs/starlight/components")
    );
    assert_eq!(
        prop(aside, "type").kind,
        PropKind::Enum {
            values: vec![
                "note".into(),
                "tip".into(),
                "caution".into(),
                "danger".into()
            ]
        }
    );
    assert_eq!(
        prop(aside, "type").default,
        Some(PropDefault::String("note".into()))
    );
    assert!(aside.has_default_slot());
    let catalog = starlight_catalog().expect("embedded catalog");
    assert_eq!(catalog.len(), 12);
    assert_eq!(
        catalog[0].id(),
        "package:@astrojs/starlight/components#Aside"
    );

    // Expressive Code rejects child content and requires `code`.
    let code = package(&index, "Code");
    assert!(!code.has_default_slot());
    assert!(prop(code, "code").required);
    assert_eq!(prop(code, "code").kind, PropKind::String);
    assert!(prop(code, "lang").optional());
    assert_eq!(prop(code, "lang").kind, PropKind::String);

    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"dependencies":{"astro":"7.4.4"}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("src/components/Only.astro"),
        "---\ninterface Props { title: string; }\n---\n<div>{title}</div>",
    )
    .unwrap();
    let without = inspect_components(&ComponentIndexRequest::new(
        dir.path(),
        1,
        Cancellation::default(),
    ))
    .unwrap();
    assert!(!without.starlight());
    assert_eq!(without.components().len(), 1);
    assert!(without.components()[0].package_specifier().is_none());

    std::fs::write(
        dir.path().join("package.json"),
        r#"{"devDependencies":{"astro":"7.4.4","@astrojs/starlight":"0.42.8"}}"#,
    )
    .unwrap();
    let with_dev_dependency = inspect_components(&ComponentIndexRequest::new(
        dir.path(),
        2,
        Cancellation::default(),
    ))
    .unwrap();
    assert!(with_dev_dependency.starlight());
    assert_eq!(with_dev_dependency.components().len(), 13);
}

#[cfg(unix)]
#[test]
fn discovery_follows_contained_symlinks_and_skips_escapes() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src/components/nested")).unwrap();
    std::fs::create_dir_all(root.join("src/shared")).unwrap();
    std::fs::write(
        root.join("src/shared/Linked.astro"),
        "---\ninterface Props { title: string; }\n---\n<div>{title}</div>",
    )
    .unwrap();
    std::fs::write(
        outside.path().join("Escaped.astro"),
        "---\ninterface Props { title: string; }\n---\n<div>{title}</div>",
    )
    .unwrap();
    std::os::unix::fs::symlink(root.join("src/shared"), root.join("src/components/linked"))
        .unwrap();
    std::os::unix::fs::symlink(
        root.join("src/shared/Linked.astro"),
        root.join("src/components/Alias.astro"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("Escaped.astro"),
        root.join("src/components/Escaped.astro"),
    )
    .unwrap();
    let index = inspect_components(&ComponentIndexRequest::new(
        root,
        3,
        Cancellation::default(),
    ))
    .unwrap();
    let names = index
        .components()
        .iter()
        .map(|component| component.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, ["Linked"]);
    assert!(
        index
            .diagnostics()
            .iter()
            .any(|message| message.contains("escaping the project root"))
    );
}

fn callout(index: &ComponentIndex) -> &Component {
    project(index, "src/components/Callout.astro")
}

/// A required-prop placeholder must sit inside the empty quoted value.
fn assert_required_placeholder_between_quotes(text: &str, placeholder: &SnippetPlaceholder) {
    if !matches!(placeholder.kind, PlaceholderKind::RequiredProp(_)) {
        return;
    }
    let start = placeholder.range.start;
    assert_eq!(text.as_bytes()[start - 1], b'"', "{text}");
    assert_eq!(text.as_bytes()[start], b'"', "{text}");
}

#[test]
fn tag_generation_uses_required_placeholders_and_wrapping_scaffold() {
    let index = index();
    let callout_component = callout(&index);
    let snippet = generate_tag(&TagRequest {
        component: callout_component,
        values: vec![
            ("title".into(), SnippetValue::String("Hello".into())),
            ("tone".into(), SnippetValue::String("tip".into())),
            ("count".into(), SnippetValue::Number("3".into())),
            ("open".into(), SnippetValue::Boolean(true)),
            (
                "tags".into(),
                SnippetValue::List(vec!["a".into(), "b".into()]),
            ),
        ],
        content: Some("Body".into()),
        client_directive: None,
    })
    .unwrap();
    assert_eq!(
        snippet.text,
        expected_file("snippets", "callout-values.txt")
    );
    assert!(snippet.placeholders.is_empty());

    let empty = generate_tag(&TagRequest {
        component: callout_component,
        values: Vec::new(),
        content: None,
        client_directive: None,
    })
    .unwrap();
    assert_eq!(empty.text, expected_file("snippets", "callout-empty.txt"));
    assert_eq!(
        empty.placeholders,
        vec![
            SnippetPlaceholder {
                range: 16..16,
                kind: PlaceholderKind::RequiredProp("title".into())
            },
            SnippetPlaceholder {
                range: 21..21,
                kind: PlaceholderKind::Content
            }
        ]
    );
    for placeholder in &empty.placeholders {
        assert_required_placeholder_between_quotes(&empty.text, placeholder);
    }

    let card = project(&index, "src/components/Nested/Card.astro");
    let card_snippet = generate_tag(&TagRequest {
        component: card,
        values: Vec::new(),
        content: Some("# Body".into()),
        client_directive: None,
    })
    .unwrap();
    assert_eq!(
        card_snippet.text,
        expected_file("snippets", "card-empty-required.txt")
    );
    assert_eq!(
        card_snippet.placeholders,
        vec![SnippetPlaceholder {
            range: 13..13,
            kind: PlaceholderKind::RequiredProp("title".into())
        }]
    );
    for placeholder in &card_snippet.placeholders {
        assert_required_placeholder_between_quotes(&card_snippet.text, placeholder);
    }
}

#[test]
fn tag_generation_handles_directives_lists_and_content_errors() {
    let index = index();
    let alert = project(&index, "src/components/Alert.tsx");
    let snippet = generate_tag(&TagRequest {
        component: alert,
        values: vec![
            ("level".into(), SnippetValue::String("warn".into())),
            ("dismissible".into(), SnippetValue::Boolean(false)),
        ],
        content: Some("Something happened".into()),
        client_directive: Some(ClientDirectiveValue {
            name: "client:load".into(),
            value: None,
        }),
    })
    .unwrap();
    assert_eq!(
        snippet.text,
        expected_file("snippets", "alert-directive.txt")
    );
    assert!(snippet.placeholders.is_empty());

    let icon = package(&index, "Icon");
    assert_eq!(
        generate_tag(&TagRequest {
            component: icon,
            values: Vec::new(),
            content: Some("nope".into()),
            client_directive: None,
        }),
        Err(SnippetError::ContentUnsupported)
    );
    assert_eq!(
        generate_tag(&TagRequest {
            component: icon,
            values: Vec::new(),
            content: None,
            client_directive: Some(ClientDirectiveValue {
                name: "client:load".into(),
                value: None,
            }),
        }),
        Err(SnippetError::DirectiveUnsupported {
            name: "client:load".into()
        })
    );
    assert_eq!(
        generate_tag(&TagRequest {
            component: alert,
            values: Vec::new(),
            content: None,
            client_directive: Some(ClientDirectiveValue {
                name: "client:media".into(),
                value: None,
            }),
        }),
        Err(SnippetError::DirectiveValueMissing {
            name: "client:media".into()
        })
    );
}

#[test]
fn tag_generation_self_closes_controls_and_array_values() {
    let index = index();
    let icon = package(&index, "Icon");
    let self_closing = generate_tag(&TagRequest {
        component: icon,
        values: vec![("name".into(), SnippetValue::String("star".into()))],
        content: None,
        client_directive: None,
    })
    .unwrap();
    assert_eq!(self_closing.text, "<Icon name=\"star\" />");
    assert!(self_closing.placeholders.is_empty());

    let callout_component = callout(&index);
    let numbers = generate_tag(&TagRequest {
        component: callout_component,
        values: vec![
            ("title".into(), SnippetValue::String("Numbers".into())),
            (
                "counts".into(),
                SnippetValue::List(vec!["1".into(), "2".into()]),
            ),
        ],
        content: None,
        client_directive: None,
    })
    .unwrap();
    assert_eq!(
        numbers.text,
        "<Callout title=\"Numbers\" counts={[1, 2]}>\n  \n</Callout>"
    );
    assert_eq!(numbers.placeholders.len(), 1);
    assert_eq!(numbers.placeholders[0].kind, PlaceholderKind::Content);

    let alert = project(&index, "src/components/Alert.tsx");
    let media = generate_tag(&TagRequest {
        component: alert,
        values: Vec::new(),
        content: None,
        client_directive: Some(ClientDirectiveValue {
            name: "client:media".into(),
            value: Some("(max-width: 600px)".into()),
        }),
    })
    .unwrap();
    assert!(media.text.contains("client:media=\"(max-width: 600px)\""));
}

#[test]
fn tag_generation_escapes_line_breaks_and_preserves_crlf_content() {
    let index = index();
    let snippet = generate_tag(&TagRequest {
        component: callout(&index),
        values: vec![
            ("title".into(), SnippetValue::String("Two\nLines".into())),
            (
                "tags".into(),
                SnippetValue::List(vec!["a\nb".into(), "c\r\nd".into()]),
            ),
        ],
        content: Some("first\r\nsecond".into()),
        client_directive: None,
    })
    .unwrap();
    assert_eq!(
        snippet.text,
        "<Callout title=\"Two&#10;Lines\" tags={[\"a\\nb\", \"c\\r\\nd\"]}>\n  first\r\n  second\n</Callout>"
    );
}

fn plan(
    document: &str,
    component: &Component,
    values: &[PlannedValue],
    enabled: bool,
    current: &[CorroboratedImport],
    other: &[CorroboratedImport],
) -> Result<ImportPlan, ImportError> {
    let root = fixture_root();
    let document = root.join(document);
    let source = std::fs::read_to_string(&document).unwrap();
    plan_imports(&ImportPlanRequest {
        source: &source,
        root: &root,
        document: &document,
        component,
        values,
        enabled,
        current_collection_imports: current,
        other_collection_imports: other,
    })
}

#[test]
fn import_plan_preserves_quote_semicolon_and_multiline_style() {
    let index = index();
    let card = project(&index, "src/components/Nested/Card.astro");
    let plain = plan("src/content/blog/plain.mdx", card, &[], true, &[], &[]).unwrap();
    assert_eq!(plain.source(), expected_file("imports", "plain.mdx"));
    assert!(plain.warnings().is_empty());

    let single = plan("src/content/blog/single.mdx", card, &[], true, &[], &[]).unwrap();
    assert_eq!(single.source(), expected_file("imports", "single.mdx"));

    let code = package(&index, "Code");
    let multiline = plan("src/content/blog/multiline.mdx", code, &[], true, &[], &[]).unwrap();
    assert_eq!(
        multiline.source(),
        expected_file("imports", "multiline.mdx")
    );

    let new_multiline = plan(
        "src/content/blog/multiline-new.mdx",
        code,
        &[],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        new_multiline.source(),
        expected_file("imports", "multiline-new.mdx")
    );

    let trailing = plan(
        "src/content/blog/trailing-comment.mdx",
        card,
        &[],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        trailing.source(),
        expected_file("imports", "trailing-comment.mdx")
    );
}

#[test]
fn import_plan_reuses_corroborated_imports_and_skips_bound_names() {
    let index = index();
    let card = project(&index, "src/components/Nested/Card.astro");
    let document = fixture_root()
        .join("src/content/blog/entry.mdx")
        .canonicalize()
        .unwrap();
    let corroborated = [CorroboratedImport {
        name: "Card".into(),
        specifier: "@components/Nested/Card.astro".into(),
        document,
    }];
    let reused = plan(
        "src/content/blog/values.mdx",
        card,
        &[],
        true,
        &corroborated,
        &[],
    )
    .unwrap();
    assert_eq!(reused.source(), expected_file("imports", "reused.mdx"));

    let other_reused = plan(
        "src/content/blog/values.mdx",
        card,
        &[],
        true,
        &[],
        &corroborated,
    )
    .unwrap();
    assert_eq!(
        other_reused.source(),
        expected_file("imports", "reused.mdx")
    );

    let bound = plan("src/content/blog/bound.mdx", card, &[], true, &[], &[]).unwrap();
    assert_eq!(bound.source(), fixture_file("src/content/blog/bound.mdx"));

    let code = package(&index, "Code");
    let package_bound = plan(
        "src/content/blog/package-bound.mdx",
        code,
        &[],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        package_bound.source(),
        fixture_file("src/content/blog/package-bound.mdx")
    );
}

#[test]
fn import_plan_refuses_collisions_and_lowercase_components() {
    let index = index();
    let card = project(&index, "src/components/Nested/Card.astro");
    assert!(matches!(
        plan("src/content/blog/collision.mdx", card, &[], true, &[], &[]),
        Err(ImportError::NameOccupied(name)) if name == "Card"
    ));
    // A same-name default import from a package is not this component.
    assert!(matches!(
        plan(
            "src/content/blog/package-name.mdx",
            card,
            &[],
            true,
            &[],
            &[]
        ),
        Err(ImportError::NameOccupied(name)) if name == "Card"
    ));
    // A package export imported under another local name leaves the component
    // name unbound.
    let code = package(&index, "Code");
    assert!(matches!(
        plan(
            "src/content/blog/package-alias.mdx",
            code,
            &[],
            true,
            &[],
            &[]
        ),
        Err(ImportError::NameOccupied(name)) if name == "Code"
    ));
    // A path alias that clearly names the component file is accepted as-is.
    let alias = plan(
        "src/content/blog/alias-bound.mdx",
        card,
        &[],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        alias.source(),
        fixture_file("src/content/blog/alias-bound.mdx")
    );
    let lower = project(&index, "src/components/lower.astro");
    let lowercase = plan("src/content/blog/values.mdx", lower, &[], true, &[], &[]).unwrap();
    assert_eq!(
        lowercase.source(),
        fixture_file("src/content/blog/values.mdx")
    );
    assert_eq!(lowercase.warnings().len(), 1);
    assert!(lowercase.warnings()[0].contains("uppercase"));
}

#[test]
fn import_plan_skips_type_only_imports_and_reuses_loader_aliases() {
    let index = index();
    let code = package(&index, "Code");
    let typed = plan(
        "src/content/blog/type-import.mdx",
        code,
        &[],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(typed.source(), expected_file("imports", "type-import.mdx"));

    let aliased = plan(
        "src/content/blog/entry-alias.mdx",
        callout(&index),
        &[PlannedValue::Entry {
            prop: "related".into(),
            collection: "blog".into(),
            id: "ada".into(),
        }],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        aliased.source(),
        expected_file("imports", "entry-alias.mdx")
    );
    assert_eq!(
        aliased.identifiers().get("related").map(String::as_str),
        Some("relatedEntry")
    );
}

#[test]
fn import_plan_resolves_images_and_entries_with_collision_safe_identifiers() {
    let index = index();
    let callout_component = callout(&index);
    let values = [
        PlannedValue::Image {
            prop: "cover".into(),
            path: "../../assets/cover.png".into(),
        },
        PlannedValue::Entry {
            prop: "related".into(),
            collection: "blog".into(),
            id: "ada".into(),
        },
    ];
    let planned = plan(
        "src/content/blog/values.mdx",
        callout_component,
        &values,
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(planned.source(), expected_file("imports", "values.mdx"));
    assert_eq!(
        planned.identifiers().get("cover").map(String::as_str),
        Some("cover")
    );
    assert_eq!(
        planned.identifiers().get("related").map(String::as_str),
        Some("relatedEntry")
    );

    let public = plan(
        "src/content/blog/public.mdx",
        callout_component,
        &[PlannedValue::Image {
            prop: "cover".into(),
            path: "public/banner.png".into(),
        }],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(public.source(), expected_file("imports", "public.mdx"));
    assert!(public.identifiers().is_empty());

    let root_public = plan(
        "src/content/blog/public.mdx",
        callout_component,
        &[PlannedValue::Image {
            prop: "cover".into(),
            path: "/banner.png".into(),
        }],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(root_public.source(), expected_file("imports", "public.mdx"));

    // `/src/...` is absolute-mode project storage; two spellings of the same
    // file must share one identifier and one import.
    let absolute = plan(
        "src/content/blog/absolute.mdx",
        callout_component,
        &[
            PlannedValue::Image {
                prop: "cover".into(),
                path: "../../assets/cover.png".into(),
            },
            PlannedValue::Image {
                prop: "cover".into(),
                path: "/src/assets/cover.png".into(),
            },
        ],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(absolute.source(), expected_file("imports", "absolute.mdx"));
    assert_eq!(
        absolute.identifiers().get("cover").map(String::as_str),
        Some("cover")
    );

    let reserved = plan(
        "src/content/blog/absolute.mdx",
        callout_component,
        &[PlannedValue::Image {
            prop: "cover".into(),
            path: "../../assets/let.png".into(),
        }],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        reserved.identifiers().get("cover").map(String::as_str),
        Some("letValue")
    );

    let collision = plan(
        "src/content/blog/ident-collision.mdx",
        callout_component,
        &[PlannedValue::Image {
            prop: "cover".into(),
            path: "../../assets/cover.png".into(),
        }],
        true,
        &[],
        &[],
    )
    .unwrap();
    assert_eq!(
        collision.source(),
        expected_file("imports", "ident-collision.mdx")
    );
    assert_eq!(
        collision.identifiers().get("cover").map(String::as_str),
        Some("cover2")
    );
}

#[test]
fn import_plan_is_opt_in_and_rejects_stale_plans() {
    let index = index();
    let card = project(&index, "src/components/Nested/Card.astro");
    let source = fixture_file("src/content/blog/plain.mdx");
    let root = fixture_root();
    let document = root.join("src/content/blog/plain.mdx");
    let disabled = plan_imports(&ImportPlanRequest {
        source: &source,
        root: &root,
        document: &document,
        component: card,
        values: &[],
        enabled: false,
        current_collection_imports: &[],
        other_collection_imports: &[],
    })
    .unwrap();
    assert_eq!(disabled.source(), source);
    assert!(!disabled.is_stale(&source));
    assert!(disabled.is_stale(&format!("{source} ")));
    assert_eq!(
        disabled.revalidate(&format!("{source} ")),
        Err(ImportError::StaleSource)
    );
    assert!(disabled.revalidate(&source).is_ok());
}
