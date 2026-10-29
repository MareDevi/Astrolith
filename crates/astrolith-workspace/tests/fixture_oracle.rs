use astrolith_astro::{Cancellation, DiscoveryOutcome, DiscoveryRequest, inspect_project};
use astrolith_core::ProjectId;
use astrolith_workspace::scan_collections;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

fn copy_tree(source: &Path, destination: &Path) {
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "node_modules" || name == ".astro" {
            continue;
        }
        let target = destination.join(&name);
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fixture_entries(fixture: &str, version: &str) -> BTreeSet<(String, String, String)> {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(fixture);
    let temp = tempfile::tempdir().unwrap();
    copy_tree(&source, temp.path());
    fs::create_dir_all(temp.path().join("node_modules/astro")).unwrap();
    fs::write(
        temp.path().join("node_modules/astro/package.json"),
        format!(r#"{{"version":"{version}"}}"#),
    )
    .unwrap();
    fs::create_dir_all(temp.path().join("node_modules/@astrojs/mdx")).unwrap();
    let mdx_version = if version == "5.18.2" {
        "4.3.14"
    } else {
        "8.1.2"
    };
    fs::write(
        temp.path().join("node_modules/@astrojs/mdx/package.json"),
        format!(r#"{{"version":"{mdx_version}"}}"#),
    )
    .unwrap();
    let request = DiscoveryRequest::new(
        ProjectId::new(fixture).unwrap(),
        1,
        temp.path().to_path_buf(),
        Cancellation::default(),
    );
    let DiscoveryOutcome::Ready(manifest) = inspect_project(&request).unwrap() else {
        panic!("fixture discovery was not authoritative");
    };
    scan_collections(&manifest, &Cancellation::default())
        .unwrap()
        .into_iter()
        .map(|entry| {
            (
                entry.entry().id().collection().as_str().to_owned(),
                entry.entry().id().id().to_owned(),
                entry.entry().source().as_str().to_owned(),
            )
        })
        .collect()
}

fn assert_oracle_mappings(entries: &BTreeSet<(String, String, String)>) {
    let expected = [
        ("archive", "guides", "content/docs/guides/index.md"),
        ("archive", "guides/nested", "content/docs/guides/nested.md"),
        (
            "archive",
            "stable-source-slug",
            "content/docs/introduction.md",
        ),
        ("archive", "深い/日本語", "content/docs/深い/日本語.mdx"),
        ("docs", "guides", "content/docs/guides/index.md"),
        ("docs", "guides/nested", "content/docs/guides/nested.md"),
        ("docs", "stable-source-slug", "content/docs/introduction.md"),
        ("docs", "深い/日本語", "content/docs/深い/日本語.mdx"),
        ("markdownDocs", "guides", "content/docs/guides/index.md"),
        (
            "markdownDocs",
            "guides/nested",
            "content/docs/guides/nested.md",
        ),
        (
            "markdownDocs",
            "stable-source-slug",
            "content/docs/introduction.md",
        ),
    ];
    assert_eq!(
        entries,
        &expected
            .into_iter()
            .map(|(collection, id, file_path)| {
                (collection.to_owned(), id.to_owned(), file_path.to_owned())
            })
            .collect()
    );
}

#[test]
fn astro5_fixture_matches_the_real_loader_oracle() {
    assert_oracle_mappings(&fixture_entries("astro5", "5.18.2"));
}

#[test]
fn astro7_fixture_matches_the_real_loader_oracle() {
    assert_oracle_mappings(&fixture_entries("astro7", "7.4.4"));
}
