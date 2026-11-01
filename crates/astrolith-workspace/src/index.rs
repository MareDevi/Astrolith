//! Bounded collection indexing implementation.

use astrolith_astro::{Cancellation, EntryKind, ProjectManifest};
use astrolith_core::{Entry, EntryId, SourcePath};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsStr,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
    time::Instant,
};
use yaml_rust2::{
    Yaml, YamlLoader,
    parser::{Event, Parser},
    scanner::TScalarStyle,
};

use crate::slug_ranges::GITHUB_SLUGGER_ALLOWED;

/// Maximum directory depth below a collection base.
const MAX_SCAN_DEPTH: usize = 64;
/// Maximum number of directories visited by one project scan.
const MAX_SCAN_DIRECTORIES: usize = 100_000;
/// Maximum number of directory entries inspected by one project scan.
const MAX_SCAN_NODES: usize = 100_000;
/// Maximum number of selected entries returned by one project scan.
const MAX_SCAN_ENTRIES: usize = 100_000;
/// Maximum frontmatter prefix read while deriving an entry ID.
const MAX_FRONTMATTER_BYTES: usize = 1024 * 1024;
/// Maximum whole YAML data entry read while deriving its ID.
const MAX_DATA_BYTES: usize = 1024 * 1024;
/// Maximum parser events accepted from one frontmatter document.
const MAX_YAML_EVENTS: usize = 256 * 1024;
/// Maximum top-level scalar fields retained from one entry's YAML.
const MAX_METADATA_FIELDS: usize = 256;

/// One top-level YAML scalar from an entry's frontmatter or data document.
///
/// Containers are deliberately absent: the file list only sorts and filters on
/// the scalar values that the mapped title, date and draft fields can hold.
#[derive(Clone, Debug, PartialEq)]
pub enum MetaValue {
    /// A YAML string, including quoted dates and timestamps.
    String(String),
    /// A YAML boolean.
    Boolean(bool),
    /// A YAML integer.
    Integer(i64),
    /// A YAML floating-point number.
    Real(f64),
}

/// Bounded presentation metadata derived from one entry source.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EntryMetadata {
    fields: BTreeMap<String, MetaValue>,
    modified_unix_millis: Option<u64>,
}

impl EntryMetadata {
    /// Return one top-level scalar field by its exact YAML key.
    pub fn field(&self, name: &str) -> Option<&MetaValue> {
        self.fields.get(name)
    }

    /// Return the source's last-modified time in Unix milliseconds.
    pub fn modified_unix_millis(&self) -> Option<u64> {
        self.modified_unix_millis
    }

    fn set_modified(&mut self, modified: Option<std::time::SystemTime>) {
        self.modified_unix_millis = modified
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64);
    }
}

/// One logical entry and its authoritative canonical source file.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionEntry {
    entry: Entry,
    absolute_source: PathBuf,
    metadata: EntryMetadata,
}

impl CollectionEntry {
    /// Return the collection-scoped logical identity and explicit source mapping.
    pub fn entry(&self) -> &Entry {
        &self.entry
    }

    /// Return the canonical source path used to open the document.
    pub fn absolute_source(&self) -> &Path {
        &self.absolute_source
    }

    /// Return the bounded scalar metadata used by the file list.
    pub fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }
}

/// Entries from overlapping collections that share one canonical source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceGroup {
    absolute_source: PathBuf,
    entries: Vec<Entry>,
}

impl SourceGroup {
    /// Return the canonical source path used for the shared document load.
    pub fn absolute_source(&self) -> &Path {
        &self.absolute_source
    }

    /// Return every collection identity mapped to this canonical source.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

/// A filesystem operation failed without an authoritative index result.
#[derive(Debug)]
pub enum WorkspaceError {
    /// Cooperative cancellation was requested.
    Cancelled,
    /// A collection base no longer resolves inside the project root.
    BaseOutsideProject(PathBuf),
    /// A filesystem operation failed.
    Io {
        /// The affected path.
        path: PathBuf,
        /// The underlying operating-system error.
        source: io::Error,
    },
    /// A symlink appeared in the scanned source tree.
    Symlink(PathBuf),
    /// The scanner reached its fixed directory-depth ceiling.
    DepthLimit {
        /// The directory at the ceiling.
        path: PathBuf,
        /// The supported maximum depth.
        max: usize,
    },
    /// The traversal exceeded its fixed directory-entry ceiling.
    TraversalLimit {
        /// The directory whose entries exceeded the ceiling.
        path: PathBuf,
        /// The supported maximum count.
        max: usize,
    },
    /// One project scan exceeded the fixed inspected-entry ceiling.
    EntryLimit {
        /// The collection identity.
        collection: String,
        /// The supported maximum count.
        max: usize,
    },
    /// A path could not be represented by the project-relative source model.
    InvalidSourcePath {
        /// The rejected path.
        path: PathBuf,
    },
    /// Frontmatter was malformed or exceeded the bounded prefix.
    InvalidFrontmatter {
        /// The source document.
        path: PathBuf,
        /// A concise parser or size diagnostic.
        message: String,
    },
    /// Frontmatter uses a format Astrolith cannot index without guessing.
    UnsupportedFrontmatter {
        /// The source document.
        path: PathBuf,
        /// Why the frontmatter cannot be interpreted safely.
        message: String,
    },
    /// A truthy frontmatter slug has a type that cannot be mapped safely to an ID.
    UnsupportedSlug(String),
    /// Two sources in one collection produced the same logical ID.
    DuplicateEntryId {
        /// The collection identity.
        collection: String,
        /// The duplicated logical ID.
        id: String,
    },
    /// A valid ID or source path could not be constructed.
    Model(String),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("workspace scan cancelled"),
            Self::BaseOutsideProject(path) => {
                write!(
                    formatter,
                    "collection base escapes project root: {}",
                    path.display()
                )
            }
            Self::Io { path, source } => {
                write!(formatter, "{}: {source}", path.display())
            }
            Self::Symlink(path) => {
                write!(formatter, "symlinks are not indexed: {}", path.display())
            }
            Self::DepthLimit { path, max } => {
                write!(
                    formatter,
                    "directory depth exceeds {max}: {}",
                    path.display()
                )
            }
            Self::TraversalLimit { path, max } => {
                write!(
                    formatter,
                    "directory traversal exceeds {max} entries: {}",
                    path.display()
                )
            }
            Self::EntryLimit { collection, max } => {
                write!(
                    formatter,
                    "scan for collection {collection:?} exceeds {max} directory entries"
                )
            }
            Self::InvalidSourcePath { path } => {
                write!(
                    formatter,
                    "source path cannot be represented safely: {}",
                    path.display()
                )
            }
            Self::InvalidFrontmatter { path, message } => {
                write!(
                    formatter,
                    "invalid frontmatter in {}: {message}",
                    path.display()
                )
            }
            Self::UnsupportedFrontmatter { path, message } => {
                write!(
                    formatter,
                    "unsupported frontmatter in {}: {message}",
                    path.display()
                )
            }
            Self::UnsupportedSlug(message) => formatter.write_str(message),
            Self::DuplicateEntryId { collection, id } => {
                write!(
                    formatter,
                    "duplicate logical ID {id:?} in collection {collection:?}"
                )
            }
            Self::Model(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Scan every verified collection in a project and return explicit entry/source mappings.
///
/// This function performs synchronous filesystem work and must run on the application's
/// bounded blocking lane. Directory symlinks and file symlinks are refused rather than followed.
pub fn scan_collections(
    project: &ProjectManifest,
    cancellation: &Cancellation,
) -> Result<Vec<CollectionEntry>, WorkspaceError> {
    check_cancelled(cancellation)?;
    let started = Instant::now();
    let root = canonical_dir(project.root())?;
    let mut entries = Vec::new();
    let mut logical_ids = HashSet::new();
    let mut directories_visited = 0_usize;
    let mut nodes_visited = 0_usize;

    for descriptor in project.collections() {
        check_cancelled(cancellation)?;
        let base = canonical_dir(descriptor.base())?;
        if !base.starts_with(&root) {
            return Err(WorkspaceError::BaseOutsideProject(base));
        }
        let collection = descriptor.id().as_str().to_owned();
        let mut stack = vec![(base.clone(), 0_usize)];

        while let Some((directory, depth)) = stack.pop() {
            check_cancelled(cancellation)?;
            directories_visited += 1;
            if directories_visited > MAX_SCAN_DIRECTORIES {
                return Err(WorkspaceError::TraversalLimit {
                    path: directory,
                    max: MAX_SCAN_DIRECTORIES,
                });
            }
            let mut children = read_directory(&directory, cancellation)?;
            children.sort();
            for path in children.into_iter().rev() {
                check_cancelled(cancellation)?;
                nodes_visited += 1;
                if nodes_visited > MAX_SCAN_NODES {
                    return Err(WorkspaceError::EntryLimit {
                        collection: collection.clone(),
                        max: MAX_SCAN_NODES,
                    });
                }
                if is_ignored_name(&path) {
                    continue;
                }
                let metadata =
                    fs::symlink_metadata(&path).map_err(|source| WorkspaceError::Io {
                        path: path.clone(),
                        source,
                    })?;
                let file_type = metadata.file_type();
                if file_type.is_symlink() {
                    return Err(WorkspaceError::Symlink(path));
                }
                if file_type.is_dir() {
                    if depth >= MAX_SCAN_DEPTH {
                        return Err(WorkspaceError::DepthLimit {
                            path,
                            max: MAX_SCAN_DEPTH,
                        });
                    }
                    stack.push((path, depth + 1));
                    continue;
                }
                let relative_to_base = path
                    .strip_prefix(&base)
                    .map_err(|_| WorkspaceError::InvalidSourcePath { path: path.clone() })?;
                if !file_type.is_file() {
                    continue;
                }
                let Some(kind) = descriptor.pattern().entry_kind(relative_to_base) else {
                    continue;
                };
                if entries.len() >= MAX_SCAN_ENTRIES {
                    return Err(WorkspaceError::EntryLimit {
                        collection: collection.clone(),
                        max: MAX_SCAN_ENTRIES,
                    });
                }

                let (source, absolute_source) = source_path(&root, &path)?;
                let (declared_slug, mut metadata) = match kind {
                    EntryKind::Yaml => data_metadata(&absolute_source, cancellation)?,
                    EntryKind::Markdown | EntryKind::Mdx => {
                        frontmatter_metadata(&absolute_source, cancellation)?
                    }
                };
                metadata.set_modified(metadata_modified(&path));
                let slug = declared_slug.unwrap_or_else(|| default_logical_id(relative_to_base));
                let id = EntryId::new(collection.clone(), slug)
                    .map_err(|error| WorkspaceError::Model(error.to_string()))?;
                if !logical_ids.insert(id.clone()) {
                    return Err(WorkspaceError::DuplicateEntryId {
                        collection,
                        id: id.id().to_owned(),
                    });
                }
                entries.push(CollectionEntry {
                    entry: Entry::new(id, source),
                    absolute_source,
                    metadata,
                });
            }
        }
    }

    entries.sort_by(|left, right| {
        left.entry
            .id()
            .cmp(right.entry.id())
            .then_with(|| left.absolute_source.cmp(&right.absolute_source))
    });
    check_cancelled(cancellation)?;
    tracing::debug!(
        root = %root.display(),
        collections = project.collections().len(),
        entries = entries.len(),
        directories = directories_visited,
        nodes = nodes_visited,
        elapsed_ms = started.elapsed().as_millis(),
        "collection scan finished"
    );
    Ok(entries)
}

/// Coalesce duplicate source loads while preserving every logical collection identity.
pub fn coalesce_by_source(entries: Vec<CollectionEntry>) -> Vec<SourceGroup> {
    let mut grouped: BTreeMap<PathBuf, Vec<Entry>> = BTreeMap::new();
    for collection_entry in entries {
        grouped
            .entry(collection_entry.absolute_source)
            .or_default()
            .push(collection_entry.entry);
    }
    grouped
        .into_iter()
        .map(|(absolute_source, mut entries)| {
            entries.sort_by(|left, right| left.id().cmp(right.id()));
            SourceGroup {
                absolute_source,
                entries,
            }
        })
        .collect()
}

fn check_cancelled(cancellation: &Cancellation) -> Result<(), WorkspaceError> {
    cancellation.check().map_err(|_| WorkspaceError::Cancelled)
}

fn canonical_dir(path: &Path) -> Result<PathBuf, WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.is_dir() {
        return Err(WorkspaceError::Io {
            path: canonical,
            source: io::Error::new(io::ErrorKind::InvalidInput, "expected a directory"),
        });
    }
    Ok(canonical)
}

fn read_directory(
    path: &Path,
    cancellation: &Cancellation,
) -> Result<Vec<PathBuf>, WorkspaceError> {
    let mut children = Vec::new();
    for entry in fs::read_dir(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })? {
        check_cancelled(cancellation)?;
        if children.len() >= MAX_SCAN_NODES {
            return Err(WorkspaceError::TraversalLimit {
                path: path.to_path_buf(),
                max: MAX_SCAN_NODES,
            });
        }
        children.push(
            entry
                .map_err(|source| WorkspaceError::Io {
                    path: path.to_path_buf(),
                    source,
                })?
                .path(),
        );
    }
    Ok(children)
}

fn is_ignored_name(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    name.starts_with('.') || name == "node_modules" || name == ".git"
}

fn source_path(root: &Path, path: &Path) -> Result<(SourcePath, PathBuf), WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !canonical.starts_with(root) {
        return Err(WorkspaceError::BaseOutsideProject(canonical));
    }
    let relative = canonical
        .strip_prefix(root)
        .map_err(|_| WorkspaceError::InvalidSourcePath {
            path: canonical.clone(),
        })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(WorkspaceError::InvalidSourcePath {
                path: canonical.clone(),
            });
        };
        let segment = segment
            .to_str()
            .ok_or_else(|| WorkspaceError::InvalidSourcePath {
                path: canonical.clone(),
            })?;
        segments.push(segment);
    }
    let source = segments.join("/");
    let source =
        SourcePath::new(source).map_err(|error| WorkspaceError::Model(error.to_string()))?;
    Ok((source, canonical))
}

fn frontmatter_metadata(
    path: &Path,
    cancellation: &Cancellation,
) -> Result<(Option<String>, EntryMetadata), WorkspaceError> {
    check_cancelled(cancellation)?;
    let Some(frontmatter) = read_frontmatter(path)? else {
        return Ok((None, EntryMetadata::default()));
    };
    check_cancelled(cancellation)?;
    validate_yaml_events(path, &frontmatter, cancellation)?;
    check_cancelled(cancellation)?;
    let documents = YamlLoader::load_from_str(&frontmatter).map_err(|error| {
        WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    slug_and_metadata(path, &documents, "frontmatter")
}

/// Derive the slug override and scalar metadata from a whole YAML data entry.
///
/// Astro's data entry types parse the complete file and pass the resulting
/// object to the same `generateId` contract as Markdown frontmatter, so a
/// top-level string `slug` replaces the path-derived ID.
fn data_metadata(
    path: &Path,
    cancellation: &Cancellation,
) -> Result<(Option<String>, EntryMetadata), WorkspaceError> {
    check_cancelled(cancellation)?;
    let source = read_data_file(path)?;
    validate_yaml_events(path, &source, cancellation)?;
    check_cancelled(cancellation)?;
    let documents =
        YamlLoader::load_from_str(&source).map_err(|error| WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    slug_and_metadata(path, &documents, "YAML data")
}

fn slug_and_metadata(
    path: &Path,
    documents: &[Yaml],
    context: &str,
) -> Result<(Option<String>, EntryMetadata), WorkspaceError> {
    let Some(document) = documents.first() else {
        return Ok((None, EntryMetadata::default()));
    };
    let Yaml::Hash(data) = document else {
        if matches!(document, Yaml::BadValue | Yaml::Null) {
            return Ok((None, EntryMetadata::default()));
        }
        return Err(WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: format!("{context} must be a mapping"),
        });
    };
    let slug = data.iter().find_map(|(key, value)| match key {
        Yaml::String(key) if key == "slug" => Some(value),
        _ => None,
    });
    let slug = match slug {
        None => None,
        Some(value) if yaml_slug_is_falsey(value) => None,
        Some(Yaml::String(value)) => Some(value.clone()),
        Some(value) => {
            return Err(WorkspaceError::UnsupportedSlug(format!(
                "truthy {context} slug in {} has unsupported type {value:?}; refusing to guess an Astro ID",
                path.display()
            )));
        }
    };
    Ok((slug, metadata_from_document(data)))
}

fn metadata_from_document(data: &yaml_rust2::yaml::Hash) -> EntryMetadata {
    let mut fields = BTreeMap::new();
    for (key, value) in data.iter().take(MAX_METADATA_FIELDS) {
        let Yaml::String(key) = key else {
            continue;
        };
        let Some(value) = meta_value(value) else {
            continue;
        };
        fields.insert(key.clone(), value);
    }
    EntryMetadata {
        fields,
        modified_unix_millis: None,
    }
}

fn meta_value(value: &Yaml) -> Option<MetaValue> {
    match value {
        Yaml::String(text) => Some(MetaValue::String(text.clone())),
        Yaml::Boolean(flag) => Some(MetaValue::Boolean(*flag)),
        Yaml::Integer(number) => Some(MetaValue::Integer(*number)),
        Yaml::Real(text) => text.parse::<f64>().ok().map(MetaValue::Real),
        _ => None,
    }
}

fn metadata_modified(path: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn read_data_file(path: &Path) -> Result<String, WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file = File::open(&canonical).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() || metadata.len() > MAX_DATA_BYTES as u64 {
        return Err(WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: format!("YAML data entry exceeds {MAX_DATA_BYTES} bytes"),
        });
    }
    let mut bytes = Vec::new();
    file.take(MAX_DATA_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| WorkspaceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > MAX_DATA_BYTES {
        return Err(WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: format!("YAML data entry exceeds {MAX_DATA_BYTES} bytes"),
        });
    }
    String::from_utf8(bytes).map_err(|error| WorkspaceError::InvalidFrontmatter {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}

fn yaml_slug_is_falsey(value: &Yaml) -> bool {
    match value {
        Yaml::BadValue | Yaml::Null | Yaml::Boolean(false) => true,
        Yaml::Integer(value) => *value == 0,
        Yaml::Real(value) => {
            value.eq_ignore_ascii_case(".nan")
                || value.parse::<f64>().is_ok_and(|number| number == 0.0)
        }
        Yaml::String(value) => value.is_empty(),
        Yaml::Array(_) | Yaml::Hash(_) | Yaml::Alias(_) | Yaml::Boolean(true) => false,
    }
}

#[derive(Clone, Copy)]
enum YamlMappingPosition {
    Key,
    Value { root_slug: bool },
}

enum YamlContainer {
    Mapping {
        position: YamlMappingPosition,
        root: bool,
    },
    Sequence,
}

fn validate_yaml_events(
    path: &Path,
    source: &str,
    cancellation: &Cancellation,
) -> Result<(), WorkspaceError> {
    let mut parser = Parser::new_from_str(source);
    let mut stack = Vec::new();
    let mut documents = 0_usize;
    let mut events = 0_usize;

    loop {
        events += 1;
        if events > MAX_YAML_EVENTS {
            return Err(WorkspaceError::InvalidFrontmatter {
                path: path.to_path_buf(),
                message: format!("YAML frontmatter exceeds {MAX_YAML_EVENTS} parser events"),
            });
        }
        if events.is_multiple_of(1024) {
            check_cancelled(cancellation)?;
        }
        let (event, _) =
            parser
                .next_token()
                .map_err(|error| WorkspaceError::InvalidFrontmatter {
                    path: path.to_path_buf(),
                    message: error.to_string(),
                })?;

        match event {
            Event::StreamEnd => break,
            Event::DocumentStart => {
                documents += 1;
                if documents > 1 {
                    return invalid_yaml(path, "multiple YAML documents are unsupported");
                }
            }
            Event::Alias(_) => return invalid_yaml(path, "YAML anchor/alias is unsupported"),
            Event::Scalar(value, style, anchor, tag) => {
                reject_yaml_attributes(path, anchor, tag.as_ref())?;
                if let Some(YamlContainer::Mapping { position, root }) = stack.last_mut() {
                    match *position {
                        YamlMappingPosition::Key => {
                            if style == TScalarStyle::Plain && value == "<<" {
                                return invalid_yaml(path, "YAML merge keys are unsupported");
                            }
                            *position = YamlMappingPosition::Value {
                                root_slug: *root && value == "slug",
                            };
                        }
                        YamlMappingPosition::Value { root_slug } => {
                            if root_slug && style == TScalarStyle::Plain {
                                if is_js_yaml_timestamp(&value) {
                                    return Err(WorkspaceError::UnsupportedSlug(format!(
                                        "implicit YAML timestamp slug in {} has JS Date semantics; refusing to interpret it as a string",
                                        path.display()
                                    )));
                                }
                                if is_ambiguous_plain_slug(&value) {
                                    return Err(WorkspaceError::UnsupportedSlug(format!(
                                        "ambiguous plain YAML slug in {} resolves differently in JS YAML; quote the slug to preserve string semantics",
                                        path.display()
                                    )));
                                }
                            }
                            *position = YamlMappingPosition::Key;
                        }
                    }
                }
            }
            Event::SequenceStart(anchor, tag) => {
                reject_yaml_attributes(path, anchor, tag.as_ref())?;
                reject_complex_mapping_key(path, &stack)?;
                stack.push(YamlContainer::Sequence);
            }
            Event::MappingStart(anchor, tag) => {
                reject_yaml_attributes(path, anchor, tag.as_ref())?;
                reject_complex_mapping_key(path, &stack)?;
                stack.push(YamlContainer::Mapping {
                    position: YamlMappingPosition::Key,
                    root: stack.is_empty(),
                });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                if stack.pop().is_none() {
                    return invalid_yaml(path, "unbalanced YAML container events");
                }
                if let Some(YamlContainer::Mapping { position, .. }) = stack.last_mut() {
                    *position = YamlMappingPosition::Key;
                }
            }
            Event::Nothing | Event::StreamStart | Event::DocumentEnd => {}
        }
    }

    if !stack.is_empty() {
        return invalid_yaml(path, "unbalanced YAML container events");
    }
    Ok(())
}

fn reject_yaml_attributes(
    path: &Path,
    anchor: usize,
    tag: Option<&yaml_rust2::parser::Tag>,
) -> Result<(), WorkspaceError> {
    if anchor != 0 {
        return invalid_yaml(path, "YAML anchor/alias is unsupported");
    }
    if tag.is_some() {
        return invalid_yaml(path, "YAML tags are unsupported");
    }
    Ok(())
}

fn reject_complex_mapping_key(path: &Path, stack: &[YamlContainer]) -> Result<(), WorkspaceError> {
    if matches!(
        stack.last(),
        Some(YamlContainer::Mapping {
            position: YamlMappingPosition::Key,
            ..
        })
    ) {
        return invalid_yaml(path, "complex YAML mapping keys are unsupported");
    }
    Ok(())
}

fn is_js_yaml_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() == 10
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..].iter().all(u8::is_ascii_digit)
    {
        return true;
    }

    let mut index = 0_usize;
    if consume_digits(bytes, &mut index, 4, 4) != 4 || bytes.get(index) != Some(&b'-') {
        return false;
    }
    index += 1;
    if consume_digits(bytes, &mut index, 1, 2) == 0 || bytes.get(index) != Some(&b'-') {
        return false;
    }
    index += 1;
    if consume_digits(bytes, &mut index, 1, 2) == 0 {
        return false;
    }

    if matches!(bytes.get(index), Some(b'T' | b't')) {
        index += 1;
    } else {
        let separator_start = index;
        while matches!(bytes.get(index), Some(b' ' | b'\t')) {
            index += 1;
        }
        if index == separator_start {
            return false;
        }
    }

    if consume_digits(bytes, &mut index, 1, 2) == 0 || bytes.get(index) != Some(&b':') {
        return false;
    }
    index += 1;
    if consume_digits(bytes, &mut index, 2, 2) != 2 || bytes.get(index) != Some(&b':') {
        return false;
    }
    index += 1;
    if consume_digits(bytes, &mut index, 2, 2) != 2 {
        return false;
    }

    if bytes.get(index) == Some(&b'.') {
        index += 1;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
    }
    while matches!(bytes.get(index), Some(b' ' | b'\t')) {
        index += 1;
    }
    if index == bytes.len() || (bytes.get(index) == Some(&b'Z') && index + 1 == bytes.len()) {
        return true;
    }
    if !matches!(bytes.get(index), Some(b'+' | b'-')) {
        return false;
    }
    index += 1;
    if consume_digits(bytes, &mut index, 1, 2) == 0 {
        return false;
    }
    if bytes.get(index) == Some(&b':') {
        index += 1;
        if consume_digits(bytes, &mut index, 2, 2) != 2 {
            return false;
        }
    }
    index == bytes.len()
}

fn is_ambiguous_plain_slug(value: &str) -> bool {
    if !matches!(Yaml::from_str(value), Yaml::String(_)) {
        return false;
    }
    if value.eq_ignore_ascii_case("null") {
        return true;
    }
    let bytes = value.as_bytes();
    match bytes.first() {
        Some(byte) if byte.is_ascii_digit() => true,
        Some(b'+' | b'-' | b'.') => bytes.get(1).is_some_and(u8::is_ascii_digit),
        _ => false,
    }
}

fn consume_digits(bytes: &[u8], index: &mut usize, min: usize, max: usize) -> usize {
    let start = *index;
    while *index - start < max && bytes.get(*index).is_some_and(u8::is_ascii_digit) {
        *index += 1;
    }
    let consumed = *index - start;
    if consumed < min {
        *index = start;
        0
    } else {
        consumed
    }
}

fn invalid_yaml<T>(path: &Path, message: &str) -> Result<T, WorkspaceError> {
    Err(WorkspaceError::InvalidFrontmatter {
        path: path.to_path_buf(),
        message: message.into(),
    })
}

fn read_frontmatter(path: &Path) -> Result<Option<String>, WorkspaceError> {
    let canonical = fs::canonicalize(path).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let file = File::open(&canonical).map_err(|source| WorkspaceError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let Some((mut first, mut bytes_read)) = read_line_bounded(&mut reader, 0, path)? else {
        return Ok(None);
    };
    first = first.strip_prefix('\u{feff}').unwrap_or(&first).to_owned();
    while first.trim().is_empty() {
        let Some((line, consumed)) = read_line_bounded(&mut reader, bytes_read, path)? else {
            return Ok(None);
        };
        bytes_read += consumed;
        first = line;
    }
    match first.as_str() {
        "---" => {}
        "+++" => {
            return Err(WorkspaceError::UnsupportedFrontmatter {
                path: path.to_path_buf(),
                message: "TOML frontmatter delimiters (+++) are unsupported".into(),
            });
        }
        line if line.starts_with("---") || line.starts_with("+++") => {
            return Err(WorkspaceError::UnsupportedFrontmatter {
                path: path.to_path_buf(),
                message: "frontmatter delimiters must be exactly --- or +++".into(),
            });
        }
        _ => return Ok(None),
    }
    let mut frontmatter = Vec::new();
    loop {
        let Some((line, consumed)) = read_line_bounded(&mut reader, bytes_read, path)? else {
            return Err(WorkspaceError::InvalidFrontmatter {
                path: path.to_path_buf(),
                message: "frontmatter is missing a closing delimiter".into(),
            });
        };
        bytes_read += consumed;
        if line == "---" {
            break;
        }
        frontmatter.extend_from_slice(line.as_bytes());
        frontmatter.push(b'\n');
    }
    String::from_utf8(frontmatter)
        .map(Some)
        .map_err(|error| WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

fn read_line_bounded(
    reader: &mut BufReader<File>,
    bytes_read: usize,
    path: &Path,
) -> Result<Option<(String, usize)>, WorkspaceError> {
    let remaining = MAX_FRONTMATTER_BYTES.saturating_sub(bytes_read);
    let mut bytes = Vec::new();
    reader
        .take(remaining as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|source| WorkspaceError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() > remaining {
        return Err(WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: format!("frontmatter exceeds {MAX_FRONTMATTER_BYTES} bytes"),
        });
    }
    let consumed = bytes.len();
    if consumed == 0 {
        return Ok(None);
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes)
        .map(|line| Some((line, consumed)))
        .map_err(|error| WorkspaceError::InvalidFrontmatter {
            path: path.to_path_buf(),
            message: error.to_string(),
        })
}

fn default_logical_id(relative_path: &Path) -> String {
    let without_extension = relative_path.with_extension("");
    let slug = without_extension
        .components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(github_slug(&segment.to_string_lossy())),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");
    slug.strip_suffix("/index").unwrap_or(&slug).to_owned()
}

fn github_slug(value: &str) -> String {
    value
        .to_lowercase()
        .chars()
        .filter(|character| slug_character_is_allowed(*character))
        .map(|character| if character == ' ' { '-' } else { character })
        .collect()
}

fn slug_character_is_allowed(character: char) -> bool {
    let value = character as u32;
    GITHUB_SLUGGER_ALLOWED
        .binary_search_by(|(start, end)| {
            if value < *start {
                std::cmp::Ordering::Greater
            } else if value > *end {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}
