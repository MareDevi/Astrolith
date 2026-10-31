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
    let (mdx_version, mdx_peer) = match version {
        "5.18.2" => ("4.3.14", "^5.0.0"),
        "6.4.8" => ("6.0.3", "^6.4.0"),
        "7.4.4" => ("8.1.2", "^7.2.10"),
        other => panic!("unmapped fixture version {other}"),
    };
    fs::write(
        temp.path().join("node_modules/@astrojs/mdx/package.json"),
        format!(r#"{{"version":"{mdx_version}","peerDependencies":{{"astro":"{mdx_peer}"}}}}"#),
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

fn assert_oracle_mappings(
    entries: &BTreeSet<(String, String, String)>,
    expected: &[(&str, &str, &str)],
) {
    assert_eq!(
        entries,
        &expected
            .iter()
            .map(|(collection, id, file_path)| {
                (
                    (*collection).to_owned(),
                    (*id).to_owned(),
                    (*file_path).to_owned(),
                )
            })
            .collect()
    );
}

const SINGLE_PATTERN_ORACLE: &[(&str, &str, &str)] = &[
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

const ARRAY_PATTERN_ORACLE: &[(&str, &str, &str)] = &[
    ("archive", "drafts/wip", "content/docs/drafts/wip.md"),
    ("archive", "guides", "content/docs/guides/index.md"),
    ("archive", "guides/nested", "content/docs/guides/nested.md"),
    (
        "archive",
        "stable-source-slug",
        "content/docs/introduction.md",
    ),
    ("archive", "深い/日本語", "content/docs/深い/日本語.mdx"),
    ("docs", "drafts/wip", "content/docs/drafts/wip.md"),
    ("docs", "guides", "content/docs/guides/index.md"),
    ("docs", "guides/nested", "content/docs/guides/nested.md"),
    ("docs", "stable-source-slug", "content/docs/introduction.md"),
    ("docs", "深い/日本語", "content/docs/深い/日本語.mdx"),
    ("markdownDocs", "drafts/wip", "content/docs/drafts/wip.md"),
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
    (
        "negativeFirstDocs",
        "guides",
        "content/docs/guides/index.md",
    ),
    (
        "negativeFirstDocs",
        "guides/nested",
        "content/docs/guides/nested.md",
    ),
    (
        "negativeFirstDocs",
        "stable-source-slug",
        "content/docs/introduction.md",
    ),
    (
        "negativeFirstDocs",
        "深い/日本語",
        "content/docs/深い/日本語.mdx",
    ),
    ("publishedDocs", "guides", "content/docs/guides/index.md"),
    (
        "publishedDocs",
        "guides/nested",
        "content/docs/guides/nested.md",
    ),
    (
        "publishedDocs",
        "stable-source-slug",
        "content/docs/introduction.md",
    ),
    (
        "publishedDocs",
        "深い/日本語",
        "content/docs/深い/日本語.mdx",
    ),
    ("selectedDocs", "drafts/wip", "content/docs/drafts/wip.md"),
];

#[test]
fn astro5_fixture_matches_the_real_loader_oracle() {
    assert_oracle_mappings(&fixture_entries("astro5", "5.18.2"), SINGLE_PATTERN_ORACLE);
}

#[test]
fn astro6_fixture_matches_the_real_loader_oracle() {
    assert_oracle_mappings(&fixture_entries("astro6", "6.4.8"), ARRAY_PATTERN_ORACLE);
}

#[test]
fn astro7_fixture_matches_the_real_loader_oracle() {
    assert_oracle_mappings(&fixture_entries("astro7", "7.4.4"), SINGLE_PATTERN_ORACLE);
}
