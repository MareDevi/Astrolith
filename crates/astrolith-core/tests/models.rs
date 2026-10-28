use std::collections::HashSet;

use astrolith_core::{CollectionId, Entry, EntryId, ProjectId, SourcePath};

#[test]
fn entry_identity_includes_collection() {
    let ids = HashSet::from([
        EntryId::new("blog", "intro").unwrap(),
        EntryId::new("docs", "intro").unwrap(),
        EntryId::new("blog", "intro").unwrap(),
    ]);
    assert_eq!(ids.len(), 2);
}

#[test]
fn logical_ids_preserve_custom_values_without_path_normalization() {
    let entry = EntryId::new("Blog", "../自定义 Slug?draft=1").unwrap();
    assert_eq!(entry.id(), "../自定义 Slug?draft=1");
    assert_ne!(entry, EntryId::new("blog", entry.id()).unwrap());
}

#[test]
fn invalid_identities_are_rejected() {
    for invalid in ["", "   ", "a\0b", "a\nb", "a\u{7f}b"] {
        assert!(ProjectId::new(invalid).is_err(), "{invalid:?}");
        assert!(CollectionId::new(invalid).is_err(), "{invalid:?}");
        assert!(EntryId::new("blog", invalid).is_err(), "{invalid:?}");
        assert!(EntryId::new(invalid, "intro").is_err(), "{invalid:?}");
    }
}

#[test]
fn source_mapping_is_independent_of_custom_logical_id() {
    let entry = Entry::new(
        EntryId::new("blog", "published-slug").unwrap(),
        SourcePath::new("articles/2026/原稿.mdx").unwrap(),
    );
    assert_eq!(entry.source().as_str(), "articles/2026/原稿.mdx");
    assert_eq!(entry.id().id(), "published-slug");
}

#[test]
fn overlapping_collections_can_map_to_same_source() {
    let source = SourcePath::new("articles/shared.md").unwrap();
    let first = Entry::new(EntryId::new("blog", "shared").unwrap(), source.clone());
    let second = Entry::new(EntryId::new("featured", "shared").unwrap(), source);
    assert_eq!(first.source(), second.source());
    assert_ne!(first.id(), second.id());
}

#[test]
fn unsafe_or_ambiguous_source_paths_are_rejected_on_every_platform() {
    for invalid in [
        "",
        ".",
        "..",
        "../article.md",
        "content/../article.md",
        "./a.md",
        "content/./a.md",
        "/tmp/a.md",
        "C:/a.md",
        "C:a.md",
        "a\\b.md",
        "\\\\server\\share\\a.md",
        "a//b.md",
        "a/",
        "a\0.md",
        "a\n.md",
    ] {
        assert!(SourcePath::new(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn source_paths_preserve_unicode_case_and_spaces() {
    let source = SourcePath::new("文章/My Post.MD").unwrap();
    assert_eq!(source.as_str(), "文章/My Post.MD");
    assert_ne!(source, SourcePath::new("文章/my post.md").unwrap());
}

#[test]
fn windows_trailing_dot_and_space_aliases_are_rejected() {
    for invalid in [
        ".. /outside.md",
        "content./a.md",
        "content /a.md",
        "a.md.",
        "a.md ",
    ] {
        assert!(SourcePath::new(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn project_id_is_stable_and_distinguishes_projects() {
    let projects = HashSet::from([
        ProjectId::new("workspace-a").unwrap(),
        ProjectId::new("workspace-a").unwrap(),
        ProjectId::new("workspace-b").unwrap(),
    ]);
    assert_eq!(projects.len(), 2);
}
