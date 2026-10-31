//! Entry route resolution: from a logical entry to a preview URL.
//!
//! [`resolve_entry_url`] joins a [`RouteMapping`] rule onto the dev server's base
//! URL. Every route path segment is percent-encoded with the URL standard
//! path-segment rules, so non-ASCII entry ids (the fixtures include CJK paths)
//! and other reserved characters become portable, valid URL paths. Resolution is
//! pure: no filesystem access, no process execution, no GUI types.

use astrolith_core::EntryId;
use url::Url;

use crate::config::RouteMapping;

/// Failure to resolve an entry to a preview URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteError {
    /// The entry's collection (or its explicit entry) is not mapped.
    UnmappedEntry {
        /// Collection name.
        collection: String,
        /// Entry id within the collection.
        id: String,
    },
    /// The base URL cannot host path routes (not an `http`/`https` base).
    InvalidBaseUrl {
        /// The offending base URL.
        url: String,
    },
    /// A mapped route is not a valid relative route path.
    InvalidRoute {
        /// The raw route that failed validation.
        route: String,
        /// Why it was rejected.
        reason: String,
    },
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnmappedEntry { collection, id } => {
                write!(
                    formatter,
                    "no route mapping for entry `{id}` in collection `{collection}`"
                )
            }
            Self::InvalidBaseUrl { url } => {
                write!(formatter, "`{url}` is not a valid http(s) base URL")
            }
            Self::InvalidRoute { route, reason } => {
                write!(formatter, "invalid route `{route}`: {reason}")
            }
        }
    }
}

impl std::error::Error for RouteError {}

/// Resolve `entry` to a preview URL under `base_url` using `mapping`.
///
/// The route is `/<collection>/<entry id>` by default, or an explicit override.
/// Each path segment is percent-encoded. An `index` final segment is folded into
/// its parent (`guides/index` → `guides`), matching Astro's content routing.
pub fn resolve_entry_url(
    entry: &EntryId,
    mapping: &RouteMapping,
    base_url: &Url,
) -> Result<Url, RouteError> {
    let collection = entry.collection().as_str();
    let id = entry.id();

    let raw = mapping
        .route_for(collection, id)
        .ok_or_else(|| RouteError::UnmappedEntry {
            collection: collection.to_owned(),
            id: id.to_owned(),
        })?;

    let segments = split_route(&raw)?;
    build_url(base_url, &segments)
}

/// Split a raw route into validated, unescaped path segments.
fn split_route(raw: &str) -> Result<Vec<String>, RouteError> {
    let trimmed = raw.trim_matches('/');
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        if segment.is_empty() {
            return Err(invalid_route(raw, "empty path segment"));
        }
        if segment == "." || segment == ".." {
            return Err(invalid_route(raw, "parent/dot path segment"));
        }
        if segment.contains('\\') || segment.chars().any(char::is_control) {
            return Err(invalid_route(raw, "forbidden character in path segment"));
        }
        segments.push(segment.to_owned());
    }
    // Fold Astro's `index` entry into its parent route.
    if segments.last().is_some_and(|segment| segment == "index") {
        segments.pop();
    }
    Ok(segments)
}

fn invalid_route(raw: &str, reason: &str) -> RouteError {
    RouteError::InvalidRoute {
        route: raw.to_owned(),
        reason: reason.to_owned(),
    }
}

/// Join validated segments onto an `http(s)` base URL, percent-encoding each.
fn build_url(base_url: &Url, segments: &[String]) -> Result<Url, RouteError> {
    if !matches!(base_url.scheme(), "http" | "https") {
        return Err(RouteError::InvalidBaseUrl {
            url: base_url.to_string(),
        });
    }
    let mut url = base_url.clone();
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| RouteError::InvalidBaseUrl {
                url: base_url.to_string(),
            })?;
        // Drop a trailing empty segment so pushes append instead of doubling '/'.
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_CONFIG: &str =
        include_str!("../../../fixtures/astro5/workspace/content.config.mjs");

    fn entry(collection: &str, id: &str) -> EntryId {
        EntryId::new(collection, id).expect("valid entry id")
    }

    fn base() -> Url {
        Url::parse("http://127.0.0.1:4321/").unwrap()
    }

    fn docs_mapping() -> RouteMapping {
        RouteMapping::new().with_collection("docs", "docs")
    }

    #[test]
    fn resolves_a_plain_slug() {
        let url =
            resolve_entry_url(&entry("docs", "introduction"), &docs_mapping(), &base()).unwrap();
        assert_eq!(url.as_str(), "http://127.0.0.1:4321/docs/introduction");
        assert_eq!(url.path(), "/docs/introduction");
    }

    #[test]
    fn resolves_nested_and_deep_paths() {
        let url =
            resolve_entry_url(&entry("docs", "guides/nested"), &docs_mapping(), &base()).unwrap();
        assert_eq!(url.path(), "/docs/guides/nested");
    }

    #[test]
    fn folds_index_entry_into_its_parent() {
        let mapping = docs_mapping();
        let nested = resolve_entry_url(&entry("docs", "guides/index"), &mapping, &base()).unwrap();
        assert_eq!(nested.path(), "/docs/guides");

        let root = resolve_entry_url(&entry("docs", "index"), &mapping, &base()).unwrap();
        assert_eq!(root.path(), "/docs");
    }

    #[test]
    fn percent_escapes_cjk_fixture_ids() {
        let url =
            resolve_entry_url(&entry("docs", "深い/日本語"), &docs_mapping(), &base()).unwrap();
        assert_eq!(
            url.path(),
            "/docs/%E6%B7%B1%E3%81%84/%E6%97%A5%E6%9C%AC%E8%AA%9E"
        );
        assert!(url.as_str().contains("%E6%97%A5%E6%9C%AC%E8%AA%9E"));
        // The URL parses back to the original Unicode path segments.
        assert_eq!(
            url.path_segments().unwrap().next_back().unwrap(),
            "%E6%97%A5%E6%9C%AC%E8%AA%9E"
        );
    }

    #[test]
    fn percent_escapes_spaces_and_reserved_characters() {
        let url =
            resolve_entry_url(&entry("docs", "hello world"), &docs_mapping(), &base()).unwrap();
        assert_eq!(url.path(), "/docs/hello%20world");

        let url = resolve_entry_url(&entry("docs", "a?b#c"), &docs_mapping(), &base()).unwrap();
        assert_eq!(url.path(), "/docs/a%3Fb%23c");
        assert!(url.query().is_none());
        assert!(url.fragment().is_none());
    }

    #[test]
    fn uses_explicit_entry_route_over_default() {
        let mapping = docs_mapping().with_entry("docs", "news", "blog/latest");
        let url = resolve_entry_url(&entry("docs", "news"), &mapping, &base()).unwrap();
        assert_eq!(url.path(), "/blog/latest");
    }

    #[test]
    fn resolves_against_a_subpath_base() {
        let base = Url::parse("https://example.test/preview/").unwrap();
        let url = resolve_entry_url(&entry("docs", "intro"), &docs_mapping(), &base).unwrap();
        assert_eq!(url.path(), "/preview/docs/intro");
    }

    #[test]
    fn clears_base_query_and_fragment() {
        let base = Url::parse("http://127.0.0.1:4321/?token=abc#section").unwrap();
        let url = resolve_entry_url(&entry("docs", "intro"), &docs_mapping(), &base).unwrap();
        assert!(url.query().is_none());
        assert!(url.fragment().is_none());
        assert_eq!(url.path(), "/docs/intro");
    }

    #[test]
    fn unknown_collection_is_unmapped() {
        let error =
            resolve_entry_url(&entry("notes", "intro"), &docs_mapping(), &base()).unwrap_err();
        assert_eq!(
            error,
            RouteError::UnmappedEntry {
                collection: "notes".into(),
                id: "intro".into()
            }
        );
    }

    #[test]
    fn explicit_only_collection_reports_unmapped_entry() {
        let mapping = RouteMapping::new().with_explicit_collection("notes");
        let error = resolve_entry_url(&entry("notes", "intro"), &mapping, &base()).unwrap_err();
        assert!(matches!(error, RouteError::UnmappedEntry { .. }));
    }

    #[test]
    fn dot_segment_is_an_invalid_route() {
        let error =
            resolve_entry_url(&entry("docs", "../escape"), &docs_mapping(), &base()).unwrap_err();
        match error {
            RouteError::InvalidRoute { route, .. } => assert_eq!(route, "docs/../escape"),
            other => panic!("expected InvalidRoute, got {other:?}"),
        }
    }

    #[test]
    fn empty_segment_is_an_invalid_route() {
        let mapping = docs_mapping().with_entry("docs", "bad", "a//b");
        let error = resolve_entry_url(&entry("docs", "bad"), &mapping, &base()).unwrap_err();
        assert!(matches!(error, RouteError::InvalidRoute { .. }));
    }

    #[test]
    fn non_http_base_is_invalid() {
        for raw in ["mailto:dev@example.com", "file:///tmp/preview/"] {
            let base = Url::parse(raw).unwrap();
            let error =
                resolve_entry_url(&entry("docs", "intro"), &docs_mapping(), &base).unwrap_err();
            assert!(
                matches!(error, RouteError::InvalidBaseUrl { .. }),
                "base {raw} should be rejected, got {error:?}"
            );
        }
    }

    #[test]
    fn resolves_fixture_cjk_id_through_parsed_content_config() {
        let mapping = RouteMapping::from_content_config(FIXTURE_CONFIG).unwrap();
        let url = resolve_entry_url(&entry("docs", "深い/日本語"), &mapping, &base()).unwrap();
        assert_eq!(
            url.path(),
            "/docs/%E6%B7%B1%E3%81%84/%E6%97%A5%E6%9C%AC%E8%AA%9E"
        );
    }

    #[test]
    fn empty_route_is_root_of_base() {
        let mapping = docs_mapping().with_entry("docs", "home", "/");
        let url = resolve_entry_url(&entry("docs", "home"), &mapping, &base()).unwrap();
        assert_eq!(url.path(), "/");
        assert_eq!(url.as_str(), "http://127.0.0.1:4321/");
    }
}
