//! Route configuration for preview URLs.
//!
//! [`RouteMapping`] describes how a content collection maps onto route paths for
//! the dev server. It is a pure value type: it never reads the filesystem or
//! executes project code. [`RouteMapping::from_content_config`] builds the
//! default mapping by parsing an Astro `content.config` *source string* with the
//! existing static parser in `astrolith-astro`; reading the file is the caller's
//! job.
//!
//! The default convention is `/<collection>/<entry id>`. A collection may also
//! be declared explicit-only (no default) and given per-entry routes, and an
//! individual entry id or slug can override its route. Resolution itself lives
//! in [`crate::routes::resolve_entry_url`].

use std::collections::BTreeMap;

use astrolith_astro::{ProjectManifest, config::parse_content_config};

/// Failure while building a [`RouteMapping`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// The content config could not be parsed statically.
    Parse(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(message) => write!(formatter, "invalid content config: {message}"),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Routing rule for one content collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteRule {
    /// Default route prefix (e.g. `docs`), or `None` when only explicit entries
    /// resolve.
    default_prefix: Option<String>,
    /// Explicit route overrides keyed by entry id or final id/slug segment.
    overrides: BTreeMap<String, String>,
}

impl RouteRule {
    /// A rule that maps every entry to `prefix/<id>`.
    pub fn with_prefix(prefix: impl AsRef<str>) -> Self {
        Self {
            default_prefix: Some(normalize_prefix(prefix.as_ref())),
            overrides: BTreeMap::new(),
        }
    }

    /// A rule where only explicitly mapped entries resolve.
    pub fn explicit_only() -> Self {
        Self {
            default_prefix: None,
            overrides: BTreeMap::new(),
        }
    }

    /// Add an override keyed by a full entry id or a final slug segment.
    pub fn with_entry(mut self, key: impl Into<String>, route: impl Into<String>) -> Self {
        self.overrides.insert(key.into(), route.into());
        self
    }

    /// The default route prefix, if this rule maps entries by convention.
    pub fn default_prefix(&self) -> Option<&str> {
        self.default_prefix.as_deref()
    }

    /// Explicit route overrides.
    pub fn overrides(&self) -> &BTreeMap<String, String> {
        &self.overrides
    }

    fn override_for(&self, id: &str) -> Option<&str> {
        if let Some(route) = self.overrides.get(id) {
            return Some(route);
        }
        // A "slug" override matches the final path segment of a nested id.
        if let Some((_, slug)) = id.rsplit_once('/')
            && let Some(route) = self.overrides.get(slug)
        {
            return Some(route);
        }
        None
    }

    /// Resolve the raw (unescaped) route for `id`, if this rule maps it.
    pub fn route_for(&self, id: &str) -> Option<String> {
        if let Some(route) = self.override_for(id) {
            return Some(route.to_owned());
        }
        let prefix = self.default_prefix.as_deref()?;
        if prefix.is_empty() {
            Some(id.to_owned())
        } else {
            Some(format!("{prefix}/{id}"))
        }
    }
}

fn normalize_prefix(prefix: &str) -> String {
    prefix.trim_matches('/').to_owned()
}

/// Collection-aware route mapping used by the entry route resolver.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RouteMapping {
    rules: BTreeMap<String, RouteRule>,
    default_locale: Option<String>,
    prefix_default_locale: bool,
}

impl RouteMapping {
    /// An empty mapping (every lookup is unmapped).
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the mapping has no collection rules.
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Build from `(collection, default prefix)` pairs.
    pub fn from_collections<I, C, P>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (C, P)>,
        C: Into<String>,
        P: AsRef<str>,
    {
        let mut mapping = Self::new();
        for (collection, prefix) in pairs {
            mapping = mapping.with_collection(collection, prefix);
        }
        mapping
    }

    /// Add or replace a collection's default route prefix, keeping overrides.
    pub fn with_collection(
        mut self,
        collection: impl Into<String>,
        prefix: impl AsRef<str>,
    ) -> Self {
        let rule = self
            .rules
            .entry(collection.into())
            .or_insert_with(RouteRule::explicit_only);
        rule.default_prefix = Some(normalize_prefix(prefix.as_ref()));
        self
    }

    /// Declare a collection that only resolves explicitly mapped entries.
    pub fn with_explicit_collection(mut self, collection: impl Into<String>) -> Self {
        self.rules
            .entry(collection.into())
            .or_insert_with(RouteRule::explicit_only);
        self
    }

    /// Add an entry (by full id or slug) route override for a collection.
    pub fn with_entry(
        mut self,
        collection: impl Into<String>,
        key: impl Into<String>,
        route: impl Into<String>,
    ) -> Self {
        let rule = self
            .rules
            .entry(collection.into())
            .or_insert_with(RouteRule::explicit_only);
        rule.overrides.insert(key.into(), route.into());
        self
    }

    /// The rule for `collection`, if present.
    pub fn rule(&self, collection: &str) -> Option<&RouteRule> {
        self.rules.get(collection)
    }

    /// Collection names that have a rule.
    pub fn collections(&self) -> impl Iterator<Item = &str> {
        self.rules.keys().map(String::as_str)
    }

    /// Resolve the raw route for `(collection, id)`, if mapped.
    pub fn route_for(&self, collection: &str, id: &str) -> Option<String> {
        let mut route = self.rules.get(collection)?.route_for(id)?;
        if let Some(locale) = self.default_locale.as_deref() {
            let collection_prefix = self
                .rules
                .get(collection)?
                .default_prefix
                .as_deref()
                .unwrap_or("");
            let locale_prefix = if collection_prefix.is_empty() {
                format!("{locale}/")
            } else {
                format!("{collection_prefix}/{locale}/")
            };
            if self.prefix_default_locale {
                let existing = if collection_prefix.is_empty() {
                    format!("{locale}/")
                } else {
                    format!("{collection_prefix}/{locale}/")
                };
                if !route.starts_with(&existing) {
                    route = if collection_prefix.is_empty() {
                        format!("{locale}/{route}")
                    } else {
                        format!(
                            "{collection_prefix}/{locale}/{}",
                            route
                                .strip_prefix(&format!("{collection_prefix}/"))
                                .unwrap_or(&route)
                        )
                    };
                }
            } else if route == collection_prefix && collection_prefix == locale {
                route.clear();
            } else if let Some(stripped) = route.strip_prefix(&locale_prefix) {
                if collection_prefix.is_empty() {
                    route = stripped.to_owned();
                } else {
                    route = format!("{collection_prefix}/{stripped}");
                }
            }
        }
        Some(route)
    }

    /// Apply Astro's default-locale URL prefix policy.
    pub fn with_i18n(
        mut self,
        default_locale: Option<String>,
        prefix_default_locale: bool,
    ) -> Self {
        self.default_locale = default_locale;
        self.prefix_default_locale = prefix_default_locale;
        self
    }

    /// Build the default mapping from an Astro content-config **source string**.
    ///
    /// Each collection maps by convention to `/<collection>/<entry id>`. The
    /// parser is static (tree-sitter); project code is never executed.
    pub fn from_content_config(source: &str) -> Result<Self, ConfigError> {
        let collections = parse_content_config(source).map_err(ConfigError::Parse)?;
        Ok(Self::from_collections(collections.iter().map(
            |collection| (collection.collection().to_owned(), collection.collection()),
        )))
    }

    /// Build the default mapping from an already-inspected project manifest.
    pub fn from_manifest(manifest: &ProjectManifest) -> Self {
        Self::from_collections(manifest.collections().iter().map(|collection| {
            (
                collection.id().as_str().to_owned(),
                collection.id().as_str(),
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTENT_CONFIG: &str = r#"
        import { defineCollection } from "astro:content";
        import { glob } from "astro/loaders";

        const docs = defineCollection({
            loader: glob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
        });
        const markdownDocs = defineCollection({
            loader: glob({ base: "./content/docs", pattern: "**/*.md" }),
        });
        const archivedDocs = defineCollection({
            loader: glob({ base: "./content/docs", pattern: "**/*.{md,mdx}" }),
        });

        export const collections = { docs, markdownDocs, archive: archivedDocs };
    "#;

    #[test]
    fn parses_content_config_into_collection_routes() {
        let mapping = RouteMapping::from_content_config(CONTENT_CONFIG).expect("config parses");
        let mut collections: Vec<&str> = mapping.collections().collect();
        collections.sort_unstable();
        assert_eq!(collections, vec!["archive", "docs", "markdownDocs"]);
        assert_eq!(
            mapping.route_for("docs", "introduction").as_deref(),
            Some("docs/introduction")
        );
        assert_eq!(
            mapping.route_for("archive", "guides/nested").as_deref(),
            Some("archive/guides/nested")
        );
    }

    #[test]
    fn malformed_config_is_a_typed_error() {
        let error = RouteMapping::from_content_config("export const collections = {")
            .expect_err("malformed config must fail");
        assert!(matches!(error, ConfigError::Parse(_)));
    }

    #[test]
    fn applies_astro_default_locale_prefix_policy() {
        let mapping = RouteMapping::from_collections([("note", "note")])
            .with_i18n(Some("zh-cn".into()), false);
        assert_eq!(
            mapping.route_for("note", "zh-cn/rust").as_deref(),
            Some("note/rust")
        );
        assert_eq!(
            mapping.route_for("note", "en/rust").as_deref(),
            Some("note/en/rust")
        );

        let prefixed = RouteMapping::from_collections([("note", "note")])
            .with_i18n(Some("zh-cn".into()), true);
        assert_eq!(
            prefixed.route_for("note", "zh-cn/rust").as_deref(),
            Some("note/zh-cn/rust")
        );
        assert_eq!(
            prefixed.route_for("note", "rust").as_deref(),
            Some("note/zh-cn/rust")
        );
    }

    #[test]
    fn explicit_only_collection_skips_unmapped_entries() {
        let mapping = RouteMapping::new()
            .with_explicit_collection("notes")
            .with_entry("notes", "pinned", "special/pinned");
        assert_eq!(
            mapping.route_for("notes", "pinned").as_deref(),
            Some("special/pinned")
        );
        assert!(mapping.route_for("notes", "other").is_none());
    }

    #[test]
    fn entry_override_accepts_full_id_and_final_slug() {
        let mapping = RouteMapping::new()
            .with_collection("docs", "docs")
            .with_entry("docs", "guides/nested", "deep/guide")
            .with_entry("docs", "about", "company/about");
        assert_eq!(
            mapping.route_for("docs", "guides/nested").as_deref(),
            Some("deep/guide")
        );
        // A slug override matches the final segment of a nested id.
        let mapping = mapping.with_entry("docs", "nested", "slug/nested");
        assert_eq!(
            mapping.route_for("docs", "guides/nested").as_deref(),
            Some("deep/guide"),
            "full-id override wins over slug override"
        );
        assert_eq!(
            mapping.route_for("docs", "about").as_deref(),
            Some("company/about")
        );
    }

    #[test]
    fn prefix_slashes_are_normalized() {
        let mapping = RouteMapping::new().with_collection("docs", "/docs/");
        assert_eq!(
            mapping.route_for("docs", "intro").as_deref(),
            Some("docs/intro")
        );
        let root = RouteMapping::new().with_collection("pages", "");
        assert_eq!(root.route_for("pages", "about").as_deref(), Some("about"));
    }
}
