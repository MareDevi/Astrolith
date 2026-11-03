//! Content Finder entry projection, search and link targets.
//!
//! The GPUI dialog renders one command row per indexed entry and filters with
//! the pure [`search`] function, so searching title and id never depends on
//! widget fuzzy matching. Link targets follow the collection URL pattern rules
//! from the workspace asset service.

use astrolith_workspace::assets::{document_relative_path, finder_link_target};

/// One indexed entry the Content Finder can open or link to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FinderEntry {
    /// Position in the workspace's published entry index.
    pub index: usize,
    /// Configured title, falling back to the entry id.
    pub title: String,
    /// Astro entry id.
    pub id: String,
    /// Collection identity.
    pub collection: String,
    /// Project-relative source.
    pub source: String,
    /// Frontmatter `slug`, when the entry has one.
    pub slug: Option<String>,
}

/// Case-insensitive title then id substring search.
///
/// An empty query returns every entry in index order. Results keep index order
/// so a command row maps back to the same entry after every query.
pub(crate) fn search(entries: &[FinderEntry], query: &str) -> Vec<FinderEntry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return entries.to_vec();
    }
    entries
        .iter()
        .filter(|entry| {
            entry.title.to_lowercase().contains(&query) || entry.id.to_lowercase().contains(&query)
        })
        .cloned()
        .collect()
}

/// The destination a Ctrl+Enter link uses for one entry.
///
/// With a collection URL pattern the encoded frontmatter slug (otherwise the
/// entry id) replaces `{slug}`; without one the entry's path relative to the
/// active document is escaped and used directly.
pub(crate) fn link_target(entry: &FinderEntry, document: &str, pattern: Option<&str>) -> String {
    let relative = document_relative_path(document, &entry.source);
    finder_link_target(pattern, entry.slug.as_deref(), &entry.id, &relative)
}

/// Complete Markdown link for one Content Finder insertion.
pub(crate) fn markdown_link(entry: &FinderEntry, document: &str, pattern: Option<&str>) -> String {
    format!(
        "[{}]({})",
        astrolith_workspace::assets::escape_link_text(&entry.title),
        link_target(entry, document, pattern)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: usize, title: &str, id: &str, source: &str, slug: Option<&str>) -> FinderEntry {
        FinderEntry {
            index,
            title: title.into(),
            id: id.into(),
            collection: "blog".into(),
            source: source.into(),
            slug: slug.map(str::to_owned),
        }
    }

    #[test]
    fn search_matches_titles_and_ids_ignoring_case() {
        let entries = vec![
            entry(0, "Hello World", "hello-world", "src/posts/a.md", None),
            entry(1, "Other", "ABC-123", "src/posts/b.md", None),
        ];
        assert_eq!(search(&entries, "").len(), 2);
        assert_eq!(search(&entries, "  hello ")[0].index, 0);
        assert_eq!(search(&entries, "abc")[0].index, 1);
        assert!(search(&entries, "missing").is_empty());
    }

    #[test]
    fn links_use_the_relative_path_or_a_pattern_with_slug_fallback() {
        let with_slug = entry(
            0,
            "Hello [World]",
            "hello",
            "src/posts/a(b).md",
            Some("Hello World"),
        );
        assert_eq!(
            link_target(&with_slug, "notes/deep/current.md", None),
            "../../src/posts/a\\(b\\).md"
        );
        assert_eq!(
            link_target(&with_slug, "notes/deep/current.md", Some("/blog/{slug}/")),
            "/blog/Hello%20World/"
        );
        let mut without_slug = with_slug.clone();
        without_slug.slug = None;
        assert_eq!(
            link_target(&without_slug, "notes/a.md", Some("/blog/{slug}/")),
            "/blog/hello/"
        );
        assert_eq!(
            markdown_link(&with_slug, "notes/current.md", Some("/blog/{slug}/")),
            "[Hello \\[World\\]](/blog/Hello%20World/)"
        );
    }
}
