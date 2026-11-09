//! Deterministic file-list filtering and ordering.
//!
//! The list never guesses: filters and sort keys read only the mapped
//! frontmatter fields and the bounded scalar metadata produced by the index.
//! Every ordering ends with the canonical source path, so equal keys always
//! keep one stable, reproducible order.

use std::cmp::Ordering;

use astrolith_core::preferences::FieldMappings;

use crate::{CollectionEntry, MetaValue};

/// Frontmatter field used for the title when no mapping overrides it.
pub const DEFAULT_TITLE_FIELD: &str = "title";
/// Frontmatter field used for the publication date when no mapping overrides it.
pub const DEFAULT_DATE_FIELD: &str = "pubDate";
/// Frontmatter field used for the draft flag when no mapping overrides it.
pub const DEFAULT_DRAFT_FIELD: &str = "draft";
/// Frontmatter field used by the numeric `order` sort mode.
pub const ORDER_FIELD: &str = "order";

/// Active file-list filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryFilter {
    /// Case-insensitive filename and mapped-title search text.
    pub query: String,
    /// Show only entries whose mapped draft field is the literal boolean `true`.
    pub drafts_only: bool,
}

/// One file-list ordering.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortMode {
    /// Publication date descending, undated entries first, mapped title tiebreak.
    Default,
    /// Mapped title (filename when unmapped).
    Title,
    /// File name.
    Filename,
    /// Source modification time.
    LastModified,
    /// One schema date field by exact frontmatter name.
    DateField(String),
    /// Numeric `order` frontmatter field.
    Order,
}

/// Sort direction for every mode except [`SortMode::Default`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortDirection {
    /// Smallest/oldest first.
    #[default]
    Ascending,
    /// Largest/newest first.
    Descending,
}

impl SortDirection {
    /// The opposite direction.
    pub fn flipped(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }
}

/// One mode/direction pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntrySort {
    /// Selected ordering.
    pub mode: SortMode,
    /// Direction; ignored by [`SortMode::Default`].
    pub direction: SortDirection,
}

impl Default for EntrySort {
    fn default() -> Self {
        Self {
            mode: SortMode::Default,
            direction: SortDirection::Ascending,
        }
    }
}

/// A calendar date with an optional wall-clock time, comparable lexicographically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DateKey {
    year: u16,
    month: u8,
    day: u8,
    time: (u8, u8, u8),
}

impl std::fmt::Display for DateKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

/// Parse a frontmatter date scalar.
///
/// Accepts `YYYY-MM-DD`, `YYYY-M-D`, an optional `T` or space separator, and an
/// optional `HH:MM[:SS]` time with any timezone suffix. Times are compared as
/// written, which is exact for entries that share one offset and never panics
/// for the rest.
pub fn parse_date(value: &str) -> Option<DateKey> {
    let bytes = value.as_bytes();
    let year = digits(bytes, 0, 4)? as u16;
    if bytes.get(4) != Some(&b'-') {
        return None;
    }
    let month = digits(bytes, 5, 2)? as u8;
    let separator = 5 + count_digits(bytes, 5);
    if bytes.get(separator) != Some(&b'-') {
        return None;
    }
    let day_start = separator + 1;
    let day = digits(bytes, day_start, 2)? as u8;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    let mut time = (0, 0, 0);
    let mut index = day_start + count_digits(bytes, day_start);
    if matches!(bytes.get(index), Some(b'T' | b't' | b' ')) {
        index += 1;
        let hour = digits(bytes, index, 2)? as u8;
        if bytes.get(index + 2) != Some(&b':') {
            return None;
        }
        let minute = digits(bytes, index + 3, 2)? as u8;
        index += 5;
        let mut second = 0;
        if bytes.get(index) == Some(&b':') {
            second = digits(bytes, index + 1, 2)? as u8;
        }
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        time = (hour, minute, second);
    }
    Some(DateKey {
        year,
        month,
        day,
        time,
    })
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

fn digits(bytes: &[u8], start: usize, max: usize) -> Option<u32> {
    let count = count_digits(bytes, start).min(max);
    if count == 0 {
        return None;
    }
    let mut value = 0_u32;
    for byte in &bytes[start..start + count] {
        value = value * 10 + u32::from(byte - b'0');
    }
    Some(value)
}

fn count_digits(bytes: &[u8], start: usize) -> usize {
    bytes[start..]
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
}

/// The mapped title field, falling back to [`DEFAULT_TITLE_FIELD`].
pub fn title_field(mappings: Option<&FieldMappings>) -> &str {
    mappings
        .and_then(|mappings| mappings.title.as_deref())
        .unwrap_or(DEFAULT_TITLE_FIELD)
}

/// The mapped date field, falling back to [`DEFAULT_DATE_FIELD`].
pub fn date_field(mappings: Option<&FieldMappings>) -> &str {
    mappings
        .and_then(|mappings| mappings.date.as_deref())
        .unwrap_or(DEFAULT_DATE_FIELD)
}

/// The mapped draft field, falling back to [`DEFAULT_DRAFT_FIELD`].
pub fn draft_field(mappings: Option<&FieldMappings>) -> &str {
    mappings
        .and_then(|mappings| mappings.draft.as_deref())
        .unwrap_or(DEFAULT_DRAFT_FIELD)
}

/// The mapped title, when the field holds a string.
pub fn mapped_title<'a>(
    entry: &'a CollectionEntry,
    mappings: Option<&FieldMappings>,
) -> Option<&'a str> {
    match entry.metadata().field(title_field(mappings)) {
        Some(MetaValue::String(text)) => Some(text),
        _ => None,
    }
}

/// The file name of one entry source.
pub fn file_name(entry: &CollectionEntry) -> &str {
    let source = entry.entry().source().as_str();
    source.rsplit('/').next().unwrap_or(source)
}

/// The title shown for one entry: the mapped title, else the file name.
pub fn display_title(entry: &CollectionEntry, mappings: Option<&FieldMappings>) -> String {
    mapped_title(entry, mappings)
        .map(str::to_owned)
        .unwrap_or_else(|| file_name(entry).to_owned())
}

/// True only when the mapped draft field holds the literal boolean `true`.
pub fn is_draft(entry: &CollectionEntry, mappings: Option<&FieldMappings>) -> bool {
    matches!(
        entry.metadata().field(draft_field(mappings)),
        Some(MetaValue::Boolean(true))
    )
}

/// The mapped publication date, when it parses as a date.
pub fn publication_date(
    entry: &CollectionEntry,
    mappings: Option<&FieldMappings>,
) -> Option<DateKey> {
    entry_date(entry, date_field(mappings))
}

/// One named frontmatter date field, when it parses as a date.
pub fn entry_date(entry: &CollectionEntry, field: &str) -> Option<DateKey> {
    match entry.metadata().field(field) {
        Some(MetaValue::String(text)) => parse_date(text),
        _ => None,
    }
}

/// The numeric `order` value, when the field holds a number.
pub fn numeric_order(entry: &CollectionEntry) -> Option<f64> {
    match entry.metadata().field(ORDER_FIELD) {
        Some(MetaValue::Integer(value)) => Some(*value as f64),
        Some(MetaValue::Real(value)) => Some(*value),
        Some(MetaValue::String(text)) => text.parse::<f64>().ok(),
        _ => None,
    }
}

/// Whether one entry survives the active filter.
pub fn matches_filter(
    entry: &CollectionEntry,
    mappings: Option<&FieldMappings>,
    filter: &EntryFilter,
) -> bool {
    if filter.drafts_only && !is_draft(entry, mappings) {
        return false;
    }
    let query = filter.query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    if file_name(entry).to_lowercase().contains(&query) {
        return true;
    }
    mapped_title(entry, mappings).is_some_and(|title| title.to_lowercase().contains(&query))
}

/// Compare two entries under one ordering.
///
/// The final tiebreak is always the canonical source path, so equal keys keep a
/// stable order that does not depend on scan order.
pub fn compare_entries(
    left: &CollectionEntry,
    right: &CollectionEntry,
    mappings: Option<&FieldMappings>,
    sort: &EntrySort,
) -> Ordering {
    let ordering = match &sort.mode {
        SortMode::Default => default_order(left, right, mappings),
        SortMode::Title => {
            let ordering = display_title(left, mappings)
                .to_lowercase()
                .cmp(&display_title(right, mappings).to_lowercase());
            directed(ordering, sort.direction)
        }
        SortMode::Filename => {
            let ordering = file_name(left)
                .to_lowercase()
                .cmp(&file_name(right).to_lowercase());
            directed(ordering, sort.direction)
        }
        SortMode::LastModified => {
            let ordering = left
                .metadata()
                .modified_unix_millis()
                .unwrap_or(0)
                .cmp(&right.metadata().modified_unix_millis().unwrap_or(0));
            directed(ordering, sort.direction)
        }
        SortMode::DateField(field) => optional_dates(
            entry_date(left, field),
            entry_date(right, field),
            sort.direction,
        )
        .then_with(|| title_order(left, right, mappings)),
        SortMode::Order => {
            let ordering = optional_numbers(numeric_order(left), numeric_order(right));
            directed(ordering, sort.direction)
        }
    };
    ordering.then_with(|| {
        left.entry()
            .source()
            .as_str()
            .cmp(right.entry().source().as_str())
    })
}

fn default_order(
    left: &CollectionEntry,
    right: &CollectionEntry,
    mappings: Option<&FieldMappings>,
) -> Ordering {
    optional_dates(
        publication_date(left, mappings),
        publication_date(right, mappings),
        SortDirection::Descending,
    )
    .then_with(|| title_order(left, right, mappings))
}

fn title_order(
    left: &CollectionEntry,
    right: &CollectionEntry,
    mappings: Option<&FieldMappings>,
) -> Ordering {
    display_title(left, mappings)
        .to_lowercase()
        .cmp(&display_title(right, mappings).to_lowercase())
}

fn directed(ordering: Ordering, direction: SortDirection) -> Ordering {
    match direction {
        SortDirection::Ascending => ordering,
        SortDirection::Descending => ordering.reverse(),
    }
}

/// Undated entries sort first in both directions; dated entries follow the direction.
fn optional_dates(
    left: Option<DateKey>,
    right: Option<DateKey>,
    direction: SortDirection,
) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left), Some(right)) => directed(left.cmp(&right), direction),
    }
}

/// Entries without a numeric order sort last when ascending.
fn optional_numbers(left: Option<f64>, right: Option<f64>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => left.total_cmp(&right),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use astrolith_astro::{
        Cancellation, DiscoveryOutcome, DiscoveryRequest, ProjectManifest, inspect_project,
    };
    use astrolith_core::{ProjectId, preferences::FieldMappings};

    use super::*;
    use crate::scan_collections;

    fn project(entries: &[(&str, &str)]) -> (tempfile::TempDir, ProjectManifest) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("node_modules/astro")).unwrap();
        fs::write(
            root.join("node_modules/astro/package.json"),
            r#"{"version":"5.18.2"}"#,
        )
        .unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"dependencies":{"astro":"5.18.2"}}"#,
        )
        .unwrap();
        fs::write(
            root.join("astro.config.mjs"),
            "import { defineConfig } from 'astro/config';\nexport default defineConfig({});\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("src/posts")).unwrap();
        fs::write(
            root.join("src/content.config.ts"),
            "import { defineCollection } from 'astro:content';\nimport { glob } from 'astro/loaders';\nconst blog = defineCollection({ loader: glob({ base: './src/posts', pattern: '**/*.md' }) });\nexport const collections = { blog };\n",
        )
        .unwrap();
        for (name, source) in entries {
            fs::write(root.join("src/posts").join(name), source).unwrap();
        }
        let request = DiscoveryRequest::new(
            ProjectId::new("list-fixture").unwrap(),
            1,
            root.to_path_buf(),
            Cancellation::default(),
        );
        let manifest = match inspect_project(&request).unwrap() {
            DiscoveryOutcome::Ready(manifest) => manifest,
            DiscoveryOutcome::Unsupported { diagnostics, .. } => {
                panic!("list fixture discovery failed: {diagnostics:?}");
            }
        };
        (directory, manifest)
    }

    fn entries(entries: &[(&str, &str)]) -> (tempfile::TempDir, Vec<CollectionEntry>) {
        let (directory, manifest) = project(entries);
        let scanned = scan_collections(&manifest, &Cancellation::default()).unwrap();
        (directory, scanned)
    }

    fn ordered(entries: &[CollectionEntry], sort: &EntrySort) -> Vec<String> {
        let mut rows: Vec<&CollectionEntry> = entries.iter().collect();
        rows.sort_by(|left, right| compare_entries(left, right, None, sort));
        rows.into_iter()
            .map(|entry| file_name(entry).to_owned())
            .collect()
    }

    #[test]
    fn metadata_keeps_top_level_scalars_and_modified_time() {
        let (_directory, entries) = entries(&[(
            "a.md",
            "---\ntitle: 'Title'\npubDate: 2024-02-03\ndraft: true\nnested:\n  title: hidden\n---\n",
        )]);
        let metadata = entries[0].metadata();
        assert_eq!(
            metadata.field("title"),
            Some(&MetaValue::String("Title".into()))
        );
        assert_eq!(
            metadata.field("pubDate"),
            Some(&MetaValue::String("2024-02-03".into()))
        );
        assert_eq!(metadata.field("draft"), Some(&MetaValue::Boolean(true)));
        assert_eq!(metadata.field("nested"), None);
        assert!(metadata.modified_unix_millis().is_some());
    }

    #[test]
    fn draft_filter_reads_only_the_mapped_field_and_literal_true() {
        let (_directory, entries) = entries(&[
            ("true.md", "---\nstate: true\n---\n"),
            ("quoted.md", "---\nstate: 'true'\n---\n"),
            ("false.md", "---\nstate: false\n---\n"),
            ("unmapped.md", "---\ndraft: true\n---\n"),
            ("plain.md", "---\nstate: true\ndraft: true\n---\n"),
        ]);
        let mappings = FieldMappings {
            draft: Some("state".into()),
            ..FieldMappings::default()
        };
        let filter = EntryFilter {
            query: String::new(),
            drafts_only: true,
        };
        let mut names: Vec<String> = entries
            .iter()
            .filter(|entry| matches_filter(entry, Some(&mappings), &filter))
            .map(|entry| file_name(entry).to_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["plain.md", "true.md"]);
    }

    #[test]
    fn search_matches_filename_and_mapped_title_case_insensitively() {
        let (_directory, entries) = entries(&[
            ("alpha.md", "---\nheadline: 'Quarterly Report'\n---\n"),
            ("beta.md", "---\nheadline: 'Notes'\n---\n"),
        ]);
        let mappings = FieldMappings {
            title: Some("headline".into()),
            ..FieldMappings::default()
        };
        let matching = |query: &str| {
            let filter = EntryFilter {
                query: query.into(),
                drafts_only: false,
            };
            let mut names: Vec<String> = entries
                .iter()
                .filter(|entry| matches_filter(entry, Some(&mappings), &filter))
                .map(|entry| file_name(entry).to_owned())
                .collect();
            names.sort();
            names
        };
        assert_eq!(matching("ALPHA"), ["alpha.md"]);
        assert_eq!(matching("quarterly"), ["alpha.md"]);
        assert_eq!(matching("note"), ["beta.md"]);
        assert!(matching("").len() == 2);
    }

    #[test]
    fn default_sort_places_undated_first_then_newest_and_breaks_ties_by_title() {
        let (_directory, entries) = entries(&[
            ("old.md", "---\ntitle: 'Zulu'\npubDate: 2020-01-01\n---\n"),
            ("new.md", "---\ntitle: 'Alpha'\npubDate: 2024-05-01\n---\n"),
            ("undated.md", "---\ntitle: 'Bravo'\n---\n"),
            (
                "same-b.md",
                "---\ntitle: 'Charlie'\npubDate: 2024-05-01\n---\n",
            ),
            (
                "same-a.md",
                "---\ntitle: 'Alpha'\npubDate: 2024-05-01\n---\n",
            ),
        ]);
        assert_eq!(
            ordered(&entries, &EntrySort::default()),
            ["undated.md", "new.md", "same-a.md", "same-b.md", "old.md"]
        );
    }

    #[test]
    fn stable_keys_use_the_source_path_and_direction_toggles_reverse() {
        let (_directory, entries) = entries(&[
            ("b.md", "---\ntitle: 'Same'\n---\n"),
            ("a.md", "---\ntitle: 'Same'\n---\n"),
            ("c.md", "---\ntitle: 'Other'\n---\n"),
        ]);
        let ascending = EntrySort {
            mode: SortMode::Title,
            direction: SortDirection::Ascending,
        };
        assert_eq!(ordered(&entries, &ascending), ["c.md", "a.md", "b.md"]);
        let descending = EntrySort {
            mode: SortMode::Title,
            direction: SortDirection::Descending,
        };
        assert_eq!(ordered(&entries, &descending), ["a.md", "b.md", "c.md"]);
    }

    #[test]
    fn schema_date_fields_and_numeric_order_sort_independently() {
        let (_directory, entries) = entries(&[
            (
                "one.md",
                "---\ntitle: 'One'\nupdated: 2024-01-02\norder: 2\n---\n",
            ),
            (
                "two.md",
                "---\ntitle: 'Two'\nupdated: 2023-12-31\norder: 1\n---\n",
            ),
            ("three.md", "---\ntitle: 'Three'\n---\n"),
        ]);
        let by_updated = EntrySort {
            mode: SortMode::DateField("updated".into()),
            direction: SortDirection::Descending,
        };
        assert_eq!(
            ordered(&entries, &by_updated),
            ["three.md", "one.md", "two.md"]
        );
        let by_order = EntrySort {
            mode: SortMode::Order,
            direction: SortDirection::Ascending,
        };
        assert_eq!(
            ordered(&entries, &by_order),
            ["two.md", "one.md", "three.md"]
        );
        let by_order_descending = EntrySort {
            mode: SortMode::Order,
            direction: SortDirection::Descending,
        };
        assert_eq!(
            ordered(&entries, &by_order_descending),
            ["three.md", "one.md", "two.md"]
        );
    }

    #[test]
    fn date_parser_accepts_dates_and_timestamps_and_rejects_junk() {
        assert_eq!(
            parse_date("2024-02-03").map(|key| (key.year, key.month, key.day)),
            Some((2024, 2, 3))
        );
        assert!(parse_date("2024-02-03T10:20:30Z").is_some());
        assert!(parse_date("2024-02-03 10:20").is_some());
        assert_eq!(parse_date("not a date"), None);
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date(""), None);
    }

    #[test]
    fn mapped_defaults_are_used_when_no_mapping_is_configured() {
        let (_directory, entries) = entries(&[(
            "default.md",
            "---\ntitle: 'Mapped'\npubDate: 2024-01-01\ndraft: true\n---\n",
        )]);
        let entry = &entries[0];
        assert_eq!(mapped_title(entry, None), Some("Mapped"));
        assert!(is_draft(entry, None));
        assert!(publication_date(entry, None).is_some());
    }
}
