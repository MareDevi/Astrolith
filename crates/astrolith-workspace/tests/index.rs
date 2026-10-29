use astrolith_astro::{Cancellation, DiscoveryOutcome, DiscoveryRequest, inspect_project};
use astrolith_core::ProjectId;
use astrolith_workspace::scan_collections;
use std::{fs, path::Path};
use tempfile::TempDir;

fn discover(root: &Path) -> astrolith_astro::ProjectManifest {
    let request = DiscoveryRequest::new(
        ProjectId::new("fixture").unwrap(),
        7,
        root.to_path_buf(),
        Cancellation::default(),
    );
    match inspect_project(&request).unwrap() {
        DiscoveryOutcome::Ready(manifest) => manifest,
        DiscoveryOutcome::Unsupported { diagnostics, .. } => {
            panic!("discovery unexpectedly rejected fixture: {diagnostics:?}")
        }
    }
}

fn manifest(root: &Path) -> astrolith_astro::ProjectManifest {
    fs::create_dir_all(root.join("node_modules/astro")).unwrap();
    fs::write(
        root.join("package.json"),
        r#"{"dependencies":{"astro":"5.18.2"}}"#,
    )
    .unwrap();
    fs::write(
        root.join("node_modules/astro/package.json"),
        r#"{"version":"5.18.2"}"#,
    )
    .unwrap();
    fs::create_dir_all(root.join("node_modules/@astrojs/mdx")).unwrap();
    fs::write(
        root.join("node_modules/@astrojs/mdx/package.json"),
        r#"{"version":"4.3.14"}"#,
    )
    .unwrap();
    fs::write(root.join("astro.config.mjs"),"import {defineConfig} from 'astro/config'; import mdx from '@astrojs/mdx'; export default defineConfig({integrations:[mdx()]});").unwrap();
    fs::create_dir_all(root.join("src/posts/guides")).unwrap();
    fs::write(
        root.join("src/content.config.ts"),
        r#"import {defineCollection} from 'astro:content';
import {glob} from 'astro/loaders';
const docs = defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx}'})});
export const collections = {docs};
"#,
    )
    .unwrap();

    discover(root)
}

fn project() -> (TempDir, astrolith_astro::ProjectManifest) {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src/posts/guides")).unwrap();
    fs::write(
        temp.path().join("src/posts/guides/nested.md"),
        "---\ntitle: Nested\n---\n# Nested\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/guides/index.md"),
        "---\ntitle: Index\n---\n# Index\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/日本語.mdx"),
        "---\ntitle: MDX\n---\n# 日本語\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/slugged.md"),
        "---\nslug: stable-source-slug\n---\n# Slugged\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/Café + Notes.md"),
        "# Exact slugger punctuation\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/posts/ignored.txt"), "# ignored\n").unwrap();
    fs::write(temp.path().join("src/posts/.hidden.md"), "# hidden\n").unwrap();
    for ignored in [
        "src/posts/.hidden",
        "src/posts/node_modules",
        "src/posts/.git",
    ] {
        fs::create_dir_all(temp.path().join(ignored)).unwrap();
        fs::write(temp.path().join(ignored).join("hidden.md"), "# hidden\n").unwrap();
    }
    let manifest = manifest(temp.path());
    (temp, manifest)
}

#[test]
fn scans_nested_supported_sources_and_uses_astro_ids() {
    let (_temp, manifest) = project();
    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();

    let actual = entries
        .iter()
        .map(|entry| {
            (
                entry.entry().id().id().to_owned(),
                entry.entry().source().as_str().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        vec![
            ("café--notes".into(), "src/posts/Café + Notes.md".into()),
            ("guides".into(), "src/posts/guides/index.md".into()),
            ("guides/nested".into(), "src/posts/guides/nested.md".into()),
            ("stable-source-slug".into(), "src/posts/slugged.md".into()),
            ("日本語".into(), "src/posts/日本語.mdx".into()),
        ]
    );
    for entry in &entries {
        assert!(entry.absolute_source().is_absolute());
        assert_eq!(
            entry.absolute_source(),
            fs::canonicalize(manifest.root().join(entry.entry().source().as_str())).unwrap()
        );
    }
}

#[test]
fn cancelled_scan_stops_before_filesystem_work() {
    let (_temp, manifest) = project();
    let cancellation = Cancellation::default();
    cancellation.cancel();
    let error = scan_collections(&manifest, &cancellation).unwrap_err();
    assert!(matches!(
        error,
        astrolith_workspace::WorkspaceError::Cancelled
    ));
}

#[test]
fn yaml_quoted_slug_is_used_and_falsey_slug_falls_back_to_the_path() {
    let (temp, manifest) = project();
    fs::write(
        temp.path().join("src/posts/quoted.md"),
        "---\nslug: \"quoted slug\"\n---\n# Quoted\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/fallback.md"),
        "---\nslug: false\n---\n# Fallback\n",
    )
    .unwrap();
    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.entry().id().id() == "quoted slug")
    );
    assert!(
        entries
            .iter()
            .any(|entry| entry.entry().id().id() == "fallback")
    );
}

#[test]
fn bom_and_leading_blank_before_yaml_delimiters_are_parsed() {
    let (temp, manifest) = project();
    fs::write(
        temp.path().join("src/posts/bom.md"),
        "\u{feff}---\nslug: bom-slug\n---\n# BOM\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/leading-blank.md"),
        "\n \n---\nslug: leading-blank-slug\n---\n# Leading blank\n",
    )
    .unwrap();
    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
    let ids = entries
        .iter()
        .map(|entry| entry.entry().id().id())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"bom-slug"));
    assert!(ids.contains(&"leading-blank-slug"));
}

#[test]
fn js_falsey_numeric_frontmatter_slugs_fall_back_to_the_path() {
    let (temp, manifest) = project();
    for (name, slug) in [
        ("integer-zero", "0"),
        ("float-zero", "0.0"),
        ("nan", ".nan"),
    ] {
        fs::write(
            temp.path().join(format!("src/posts/{name}.md")),
            format!("---\nslug: {slug}\n---\n# Falsey\n"),
        )
        .unwrap();
    }

    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
    let ids = entries
        .iter()
        .map(|entry| entry.entry().id().id())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"integer-zero"));
    assert!(ids.contains(&"float-zero"));
    assert!(ids.contains(&"nan"));
}

#[test]
fn astro7_truthy_integer_slugs_remain_explicitly_unsupported() {
    let (temp, _) = project();
    fs::write(
        temp.path().join("package.json"),
        r#"{"dependencies":{"astro":"7.4.4"}}"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("node_modules/astro/package.json"),
        r#"{"version":"7.4.4"}"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("node_modules/@astrojs/mdx/package.json"),
        r#"{"version":"8.1.2"}"#,
    )
    .unwrap();

    fs::write(
        temp.path().join("src/posts/truthy-integer.md"),
        "---\nslug: 42\n---\n# Truthy integer\n",
    )
    .unwrap();
    let manifest = discover(temp.path());
    let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
    assert!(matches!(
        error,
        astrolith_workspace::WorkspaceError::UnsupportedSlug(_)
    ));
}

#[test]
fn delimiter_like_first_lines_must_be_exact_or_explicitly_unsupported() {
    let (temp, manifest) = project();
    let path = temp.path().join("src/posts/delimiter-prefix.md");
    for source in [
        "---  \nslug: custom\n---\n# YAML delimiter prefix\n",
        "\n+++\t\nslug = 'custom'\n+++\n# TOML delimiter prefix\n",
        "----\nslug: custom\n---\n# Long delimiter\n",
    ] {
        fs::write(&path, source).unwrap();
        let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
        assert!(
            matches!(
                error,
                astrolith_workspace::WorkspaceError::UnsupportedFrontmatter { .. }
            ),
            "delimiter-looking line silently fell back to the filename: {error}"
        );
    }
}

#[test]
fn js_yaml_plain_timestamp_slug_is_explicitly_unsupported() {
    let (temp, manifest) = project();
    let path = temp.path().join("src/posts/timestamp.md");
    for slug in ["2026-01-01", "2026-1-1T12:34:56Z", "2026-01-01T12:34:56Z"] {
        fs::write(&path, format!("---\nslug: {slug}\n---\n# Timestamp\n")).unwrap();
        let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
        assert!(
            matches!(
                error,
                astrolith_workspace::WorkspaceError::UnsupportedSlug(_)
            ),
            "implicit timestamp was interpreted as a Rust string: {error}"
        );
    }
}

#[test]
fn ambiguous_plain_yaml_slug_strings_are_explicitly_unsupported() {
    let (temp, manifest) = project();
    let path = temp.path().join("src/posts/ambiguous-slug.md");
    for slug in ["NULL", "Null", "0b10", "0xFF_00", "1_000"] {
        fs::write(&path, format!("---\nslug: {slug}\n---\n# Ambiguous slug\n")).unwrap();
        let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
        assert!(
            matches!(
                error,
                astrolith_workspace::WorkspaceError::UnsupportedSlug(_)
            ),
            "ambiguous plain slug was treated as a Rust string: {error}"
        );
    }
}

#[test]
fn quoted_ambiguous_yaml_slug_strings_remain_supported() {
    let (temp, manifest) = project();
    let path = temp.path().join("src/posts/quoted-ambiguous.md");
    for (slug, expected) in [("NULL", "NULL"), ("0b10", "0b10"), ("1_000", "1_000")] {
        fs::write(
            &path,
            format!("---\nslug: \"{slug}\"\n---\n# Quoted ambiguous slug\n"),
        )
        .unwrap();
        let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| entry.entry().id().id() == expected),
            "quoted slug {slug:?} did not retain string semantics"
        );
    }
}

#[test]
fn toml_frontmatter_is_explicitly_unsupported() {
    let (temp, manifest) = project();
    let path = temp.path().join("src/posts/toml.md");
    for source in [
        "+++\nslug = 'toml-slug'\n+++\n# TOML\n",
        "\u{feff}+++\nslug = 'toml-slug'\n+++\n# TOML with BOM\n",
        "\n\n+++\nslug = 'toml-slug'\n+++\n# TOML after blank lines\n",
    ] {
        fs::write(&path, source).unwrap();
        let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
        assert!(
            error.to_string().contains("TOML"),
            "unexpected error: {error}"
        );
    }
}

#[test]
fn unsafe_yaml_events_are_rejected_before_yaml_value_loading() {
    let (temp, manifest) = project();
    let path = temp.path().join("src/posts/unsafe-yaml.md");
    for (source, expected) in [
        (
            "---\n<<: {slug: merged-slug}\n---\n# Merge key\n",
            "merge key",
        ),
        (
            "---\nmetadata: &metadata {title: Shared}\nslug: *metadata\n---\n# Alias\n",
            "anchor/alias",
        ),
        ("---\nslug: !!str 42\n---\n# Tag\n", "tag"),
    ] {
        fs::write(&path, source).unwrap();
        let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "expected {expected:?} in error: {error}"
        );
    }
}

#[test]
fn merge_marker_scalars_outside_mapping_keys_remain_allowed() {
    let (temp, manifest) = project();
    fs::write(
        temp.path().join("src/posts/literal-merge.md"),
        "---\nslug: literal-merge\nlabel: '<<'\nitems:\n  - <<\n---\n# Literal merge markers\n",
    )
    .unwrap();

    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.entry().id().id() == "literal-merge")
    );
}

#[test]
fn unportable_yaml_slug_refuses_to_guess() {
    let (temp, manifest) = project();
    fs::write(
        temp.path().join("src/posts/numeric.md"),
        "---\nslug: 42\n---\n# Numeric\n",
    )
    .unwrap();
    let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
    assert!(matches!(
        error,
        astrolith_workspace::WorkspaceError::UnsupportedSlug(_)
    ));
}

#[test]
fn duplicate_logical_ids_within_one_collection_are_rejected() {
    let (temp, manifest) = project();
    fs::write(
        temp.path().join("src/posts/first.md"),
        "---\nslug: duplicate\n---\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("src/posts/second.md"),
        "---\nslug: duplicate\n---\n",
    )
    .unwrap();
    let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
    assert!(
        matches!(error, astrolith_workspace::WorkspaceError::DuplicateEntryId { collection, ref id } if collection == "docs" && id == "duplicate")
    );
}

#[test]
fn overlapping_collections_keep_identities_and_coalesce_shared_sources() {
    let (temp, _initial_manifest) = project();
    let config = r#"import {defineCollection} from 'astro:content';
import {glob} from 'astro/loaders';
const docs = defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx}'})});
const archive = defineCollection({loader:glob({base:'src/posts',pattern:'**/*.{md,mdx}'})});
export const collections = {docs, archive};
"#;
    fs::write(temp.path().join("src/content.config.ts"), config).unwrap();
    let manifest = discover(temp.path());
    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
    assert_eq!(entries.len(), 10);
    let groups = astrolith_workspace::coalesce_by_source(entries);
    assert_eq!(groups.len(), 5);
    assert!(groups.iter().all(|group| group.entries().len() == 2));
}

#[cfg(unix)]
#[test]
fn directory_and_file_symlinks_are_refused() {
    use std::os::unix::fs::symlink;

    let (_temp, manifest) = project();
    let root = manifest.root();
    let outside = root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.md"), "# outside\n").unwrap();
    symlink(&outside, root.join("src/posts/linked-directory")).unwrap();
    let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
    assert!(matches!(
        error,
        astrolith_workspace::WorkspaceError::Symlink(_)
    ));

    fs::remove_file(root.join("src/posts/linked-directory")).unwrap();
    fs::write(root.join("src/posts/target.md"), "# target\n").unwrap();
    symlink(
        root.join("src/posts/target.md"),
        root.join("src/posts/linked-file.md"),
    )
    .unwrap();
    let error = scan_collections(&manifest, &Cancellation::default()).unwrap_err();
    assert!(matches!(
        error,
        astrolith_workspace::WorkspaceError::Symlink(_)
    ));
}

#[cfg(unix)]
#[test]
fn ignored_directory_symlinks_do_not_abort_the_scan() {
    use std::os::unix::fs::symlink;

    let (temp, manifest) = project();
    let outside = temp.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::remove_dir_all(temp.path().join("src/posts/node_modules")).unwrap();
    for name in [".cache", "node_modules"] {
        symlink(&outside, temp.path().join("src/posts").join(name)).unwrap();
    }

    let entries = scan_collections(&manifest, &Cancellation::default()).unwrap();
    assert!(entries.iter().all(|entry| {
        let source = entry.entry().source().as_str();
        !source.contains("/.cache/") && !source.contains("/node_modules/")
    }));
}
