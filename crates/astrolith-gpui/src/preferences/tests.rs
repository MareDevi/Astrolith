//! Focused tests for the preferences draft rules and Reset All scope.

use astrolith_astro::schema::{
    CollectionSchema, Field, FieldConstraints, FieldKind, SchemaDialect,
};
use astrolith_core::preferences::{
    AutosaveDelay, CollectionPreferencesV1, FieldMappings, FileExtension, GlobalPreferences,
    ImagePathMode, ProjectPreferences, resolve,
};

use super::*;

fn schema(fields: &[(&str, FieldKind)]) -> CollectionSchema {
    CollectionSchema {
        dialect: SchemaDialect::Draft7,
        fields: fields
            .iter()
            .enumerate()
            .map(|(order, (name, kind))| Field {
                name: (*name).to_owned(),
                required: false,
                default: None,
                kind: kind.clone(),
                description: None,
                constraints: FieldConstraints::default(),
                order,
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

#[test]
fn project_and_collection_panes_follow_the_open_project() {
    let panes = |has_project| {
        Pane::ALL
            .into_iter()
            .filter(|pane| pane.available(has_project))
            .map(Pane::label)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        panes(false),
        vec![
            tr!("preferences.pane.appearance.label"),
            tr!("preferences.pane.workspace.label"),
            tr!("preferences.pane.preview.label"),
            tr!("preferences.pane.advanced.label"),
        ]
    );
    assert_eq!(
        panes(true),
        vec![
            tr!("preferences.pane.appearance.label"),
            tr!("preferences.pane.workspace.label"),
            tr!("preferences.pane.preview.label"),
            tr!("preferences.pane.project.label"),
            tr!("preferences.pane.collections.label"),
            tr!("preferences.pane.advanced.label"),
        ]
    );
}

#[test]
fn directories_are_project_relative_and_normalized() {
    assert_eq!(directory_value("").unwrap(), None);
    assert_eq!(
        directory_value("  src/content  ").unwrap(),
        Some("src/content".into())
    );
    assert_eq!(
        directory_value("./src//content/").unwrap(),
        Some("src/content".into())
    );
    for invalid in ["/src/content", "../escape", "src/../escape", "~", "."] {
        assert!(
            directory_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn font_size_and_autosave_reject_unsupported_values() {
    assert_eq!(font_size_value("18").unwrap(), 18.);
    assert_eq!(font_size_value("1").unwrap(), 1.);
    assert_eq!(font_size_value("30").unwrap(), 30.);
    for invalid in ["0", "31", "-4", "large", ""] {
        assert!(
            font_size_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
    for seconds in [1, 2, 5, 10] {
        assert!(AutosaveDelay::from_seconds(seconds).is_some());
    }
    assert_eq!(AutosaveDelay::from_seconds(3), None);
    assert_eq!(AutosaveDelay::default().seconds(), 2);
}

#[test]
fn line_height_stays_inside_the_readable_band() {
    assert_eq!(line_height_value("2").unwrap(), 2.);
    assert_eq!(line_height_value("1.2").unwrap(), 1.2);
    assert_eq!(line_height_value("3.0").unwrap(), 3.);
    for invalid in ["1.1", "3.1", "0", "-2", "loose", ""] {
        assert!(
            line_height_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn page_width_percent_stays_inside_the_readable_band() {
    assert_eq!(page_width_value("64").unwrap(), 64.);
    assert_eq!(page_width_value("30").unwrap(), 30.);
    assert_eq!(page_width_value("100").unwrap(), 100.);
    for invalid in ["29", "101", "-64", "wide", ""] {
        assert!(
            page_width_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn interface_font_size_stays_inside_the_readable_band() {
    assert_eq!(ui_font_size_value("14").unwrap(), 14.);
    assert_eq!(ui_font_size_value("8").unwrap(), 8.);
    assert_eq!(ui_font_size_value("28").unwrap(), 28.);
    for invalid in ["7", "29", "-2", "large", ""] {
        assert!(
            ui_font_size_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn panel_radius_accepts_zero_through_twenty_four_logical_pixels() {
    assert_eq!(panel_radius_value("0").unwrap(), 0.);
    assert_eq!(panel_radius_value("10").unwrap(), 10.);
    assert_eq!(panel_radius_value("24").unwrap(), 24.);
    for invalid in ["-1", "25", "NaN", "wide", ""] {
        assert!(
            panel_radius_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn ide_argv_validates_the_executable_and_known_placeholders() {
    let command = ide_command_value("hx", "{project}\n{file}\n--line\n{line}:{column}").unwrap();
    assert_eq!(command.executable, "hx");
    assert_eq!(
        command.arguments,
        ["{project}", "{file}", "--line", "{line}:{column}"]
    );
    assert_eq!(ide_command_value("", "  ").unwrap(), IdeCommand::default());
    assert!(ide_command_value("", "{file}").is_err());
    assert!(ide_command_value("hx", "{workspace}").is_err());
    assert!(ide_command_value("hx", "unclosed {file").is_err());
    assert!(ide_command_value("hx", "stray }").is_err());
}

#[test]
fn url_pattern_requires_a_slug_and_an_absolute_shape() {
    assert_eq!(url_pattern_value("").unwrap(), None);
    assert_eq!(
        url_pattern_value(" /blog/{slug}/ ").unwrap(),
        Some("/blog/{slug}/".into())
    );
    assert_eq!(
        url_pattern_value("https://example.com/{slug}").unwrap(),
        Some("https://example.com/{slug}".into())
    );
    for invalid in [
        "/blog/",
        "blog/{slug}",
        "/blog/{slug}/{other}",
        "/blog/{slug} x",
    ] {
        assert!(
            url_pattern_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn mappings_are_checked_against_the_collection_schema_kinds() {
    let published = schema(&[
        ("title", FieldKind::String { format: None }),
        ("published", FieldKind::Date),
        ("summary", FieldKind::String { format: None }),
        ("draft", FieldKind::Boolean),
        ("count", FieldKind::Integer),
    ]);
    let schemas = [&published];
    assert_eq!(
        mapping_value(MappingRole::Title, "title", &schemas).unwrap(),
        Some("title".into())
    );
    assert_eq!(
        mapping_value(MappingRole::Date, "published", &schemas).unwrap(),
        Some("published".into())
    );
    assert_eq!(
        mapping_value(MappingRole::Draft, "draft", &schemas).unwrap(),
        Some("draft".into())
    );
    assert_eq!(
        mapping_value(MappingRole::Title, "", &schemas).unwrap(),
        None
    );
    assert!(mapping_value(MappingRole::Draft, "title", &schemas).is_err());
    assert!(mapping_value(MappingRole::Date, "count", &schemas).is_err());
    assert!(mapping_value(MappingRole::Description, "missing", &schemas).is_err());
    assert!(mapping_value(MappingRole::Title, "two words", &schemas).is_err());
    // Without a schema the name is carried through unchanged.
    assert_eq!(
        mapping_value(MappingRole::Description, "summary", &[]).unwrap(),
        Some("summary".into())
    );
}

#[test]
fn heading_colours_accept_hex_and_nothing_else() {
    assert_eq!(heading_color_value("").unwrap(), None);
    assert_eq!(
        heading_color_value(" #AABBCC ").unwrap(),
        Some("#aabbcc".into())
    );
    for invalid in ["aabbcc", "#abc", "#gggggg"] {
        assert!(
            heading_color_value(invalid).is_err(),
            "{invalid} must be refused"
        );
    }
}

#[test]
fn edited_scopes_resolve_collection_over_project_over_global() {
    let global = GlobalPreferences {
        default_extension: FileExtension::Mdx,
        ..GlobalPreferences::default()
    };
    let project = ProjectPreferences {
        content_directory: directory("src/posts"),
        default_extension: Some(FileExtension::Md),
        image_path_mode: Some(ImagePathMode::Absolute),
        insert_mdx_imports: Some(false),
        field_mappings: FieldMappings {
            title: mapping("headline"),
            ..FieldMappings::default()
        },
        ..ProjectPreferences::default()
    };
    let collection = CollectionPreferencesV1 {
        content_directory: directory("src/blog"),
        image_path_mode: Some(ImagePathMode::Relative),
        field_mappings: FieldMappings {
            date: mapping("published"),
            ..FieldMappings::default()
        },
        ..CollectionPreferencesV1::default()
    };

    let resolved = resolve(&global, Some(&project), Some(&collection));
    assert_eq!(resolved.content_directory.as_deref(), Some("src/blog"));
    // Collection absent values fall back to the project, not the global default.
    assert_eq!(resolved.default_extension, FileExtension::Md);
    assert_eq!(resolved.image_path_mode, ImagePathMode::Relative);
    assert_eq!(resolved.field_mappings.title.as_deref(), Some("headline"));
    assert_eq!(resolved.field_mappings.date.as_deref(), Some("published"));
    // An explicit false is a set value, never inheritance.
    assert_eq!(resolved.insert_mdx_imports, Some(false));

    let resolved = resolve(&global, Some(&project), None);
    assert_eq!(resolved.default_extension, FileExtension::Md);
    assert_eq!(resolved.image_path_mode, ImagePathMode::Absolute);
    assert_eq!(resolved.field_mappings.date, None);

    let resolved = resolve(&global, None, None);
    assert_eq!(resolved.default_extension, FileExtension::Mdx);
    assert_eq!(resolved.image_path_mode, ImagePathMode::Relative);
}

#[test]
fn versions_report_the_application_and_every_envelope() {
    let versions = advanced::versions();
    let lines = versions.lines();
    assert_eq!(
        lines[0],
        ("Astrolith".into(), env!("CARGO_PKG_VERSION").into())
    );
    assert_eq!(
        lines[1],
        (
            tr!("preferences.advanced.global_preferences"),
            tr!("preferences.advanced.version", version = versions.global)
        )
    );
    assert_eq!(
        lines[2],
        (
            tr!("preferences.advanced.project_preferences"),
            tr!("preferences.advanced.version", version = versions.project)
        )
    );
    assert_eq!(
        lines[3],
        (
            tr!("preferences.advanced.project_registry"),
            tr!("preferences.advanced.version", version = versions.registry)
        )
    );
}

#[test]
fn reset_all_removes_only_preferences_and_the_registry() {
    let directory = tempfile::tempdir().unwrap();
    let preferences = directory.path().join("preferences");
    std::fs::create_dir_all(preferences.join("projects")).unwrap();
    std::fs::create_dir_all(directory.path().join("recovery")).unwrap();
    std::fs::create_dir_all(directory.path().join("logs")).unwrap();
    std::fs::create_dir_all(directory.path().join("save-failures")).unwrap();
    for path in [
        preferences.join("global.json"),
        preferences.join("projects.json"),
        preferences.join("projects/site.json"),
        directory.path().join("recovery/pending.json"),
        directory.path().join("logs/astrolith.log"),
        directory.path().join("save-failures/failed.json"),
    ] {
        std::fs::write(path, b"{}").unwrap();
    }

    advanced::reset_preferences(&preferences).unwrap();

    assert!(!preferences.join("global.json").exists());
    assert!(!preferences.join("projects.json").exists());
    assert!(!preferences.join("projects").exists());
    assert!(directory.path().join("recovery/pending.json").exists());
    assert!(directory.path().join("logs/astrolith.log").exists());
    assert!(directory.path().join("save-failures/failed.json").exists());
    // A second run is a no-op rather than an error.
    advanced::reset_preferences(&preferences).unwrap();
}

#[test]
fn preferred_browser_argv_rejects_shell_ambiguous_placeholders() {
    let empty = super::browser_command("  ", "\n").expect("empty inherits the system opener");
    assert!(!empty.is_configured());

    let browser =
        super::browser_command("firefox", "--new-window\n{url}/docs\n").expect("a valid argv");
    assert_eq!(browser.executable, "firefox");
    assert_eq!(
        browser.arguments,
        vec!["--new-window".to_owned(), "{url}/docs".to_owned()]
    );

    assert!(
        super::browser_command("", "{url}")
            .unwrap_err()
            .contains("before its arguments")
    );
    assert!(
        super::browser_command("firefox", "{page}")
            .unwrap_err()
            .contains("Unknown browser placeholder")
    );
    assert!(
        super::browser_command("fire\nfox", "{url}")
            .unwrap_err()
            .contains("control characters")
    );
}
