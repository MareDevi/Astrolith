//! Typed, validated entry lifecycle operations.
//!
//! Every request carries the project generation and a canonical project-relative
//! source, so a stale UI result can never mutate a different project state. All
//! filesystem work is synchronous and must run on the application's blocking
//! lane. Operations validate names, extensions, collisions, containment and
//! symlinks; deletion goes to the desktop trash only, with no permanent-delete
//! fallback.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use astrolith_astro::schema::{CollectionSchema, FieldKind};
use astrolith_core::{
    CollectionId, SourcePath,
    preferences::{FieldMappings, FileExtension},
};
use serde_json::Value;

use crate::list::{DEFAULT_DATE_FIELD, DEFAULT_TITLE_FIELD};

/// Maximum `-N` collision suffixes attempted by create and duplicate.
pub const MAX_NAME_ATTEMPTS: usize = 1000;

/// Canonical project root plus the generation its requests must match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectScope {
    root: PathBuf,
    generation: u64,
}

impl ProjectScope {
    /// Construct a scope for one project generation.
    pub fn new(root: impl Into<PathBuf>, generation: u64) -> Self {
        Self {
            root: root.into(),
            generation,
        }
    }

    /// Return the project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the project generation this scope belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// One scalar frontmatter value written at creation.
#[derive(Clone, Debug, PartialEq)]
pub enum ScalarValue {
    /// A quoted YAML string.
    String(String),
    /// A YAML integer.
    Integer(i64),
    /// A YAML floating-point number.
    Real(f64),
    /// A YAML boolean.
    Boolean(bool),
    /// A sequence of quoted YAML strings, written as a flow sequence.
    List(Vec<String>),
}

/// One required frontmatter field with its generated value.
#[derive(Clone, Debug, PartialEq)]
pub struct CreationField {
    /// Exact frontmatter key.
    pub name: String,
    /// Value written for that key.
    pub value: ScalarValue,
}

/// How the new entry's file stem is chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewEntryName {
    /// The default date stem; a collision adds `-1`, `-2`, …
    Date(String),
    /// A user-supplied stem; a collision is refused.
    Explicit(String),
}

/// One required field the UI must ask for before anything is written.
#[derive(Clone, Debug, PartialEq)]
pub struct CreationInput {
    /// Exact frontmatter key.
    pub name: String,
    /// Control family to render.
    pub kind: CreationInputKind,
}

/// The control family for one pre-write creation field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationInputKind {
    /// Free text.
    Text,
    /// A floating-point number.
    Number,
    /// An integer.
    Integer,
    /// A boolean.
    Boolean,
    /// A date scalar prefilled with today.
    Date,
    /// One of a fixed set of string values.
    Enum(Vec<String>),
    /// An Astro `image()` path. Creation accepts an in-project path; copying an
    /// external file is the asset service's job.
    Image,
    /// A comma-separated list of in-project image paths.
    ImageList,
    /// An Astro `reference()` entry id from one or more collections.
    Reference {
        /// Target collections, in schema declaration order.
        collections: Vec<String>,
    },
    /// A comma-separated list of reference entry ids.
    ReferenceList {
        /// Target collections, in schema declaration order.
        collections: Vec<String>,
    },
    /// A comma-separated list of string tags.
    Tags,
}

/// Everything creation can decide before the user is asked anything.
#[derive(Clone, Debug, PartialEq)]
pub struct CreationPlan {
    /// Required fields with representable generated values.
    pub fields: Vec<CreationField>,
    /// Required fields the UI must collect before writing.
    pub inputs: Vec<CreationInput>,
    /// Required fields that need the typed schema form.
    pub unsupported: Vec<String>,
}

impl CreationPlan {
    /// Whether the file can be written without showing creation fields.
    pub fn writes_immediately(&self) -> bool {
        self.inputs.is_empty() && self.unsupported.is_empty()
    }
}

/// Create one new entry in a resolved collection directory.
#[derive(Clone, Debug, PartialEq)]
pub struct CreateRequest {
    /// Project generation the request was built against.
    pub generation: u64,
    /// Collection the entry will belong to.
    pub collection: CollectionId,
    /// Project-relative directory that must already exist; `None` is the root.
    pub directory: Option<SourcePath>,
    /// File extension chosen by the resolved preference.
    pub extension: FileExtension,
    /// Default or explicit file stem.
    pub name: NewEntryName,
    /// Frontmatter fields to write, in order.
    pub fields: Vec<CreationField>,
}

/// Duplicate one existing entry beside itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateRequest {
    /// Project generation the request was built against.
    pub generation: u64,
    /// Canonical source to copy.
    pub source: SourcePath,
}

/// Rename or move one entry without rewriting its content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveRequest {
    /// Project generation the request was built against.
    pub generation: u64,
    /// Canonical source being moved.
    pub source: SourcePath,
    /// Canonical destination, project-relative.
    pub destination: SourcePath,
}

/// Move one entry to the desktop trash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashRequest {
    /// Project generation the request was built against.
    pub generation: u64,
    /// Canonical source being trashed.
    pub source: SourcePath,
}

/// Locate the directory that contains one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealRequest {
    /// Project generation the request was built against.
    pub generation: u64,
    /// Canonical source whose containing directory is revealed.
    pub source: SourcePath,
}

/// Absolute or project-relative path text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathForm {
    /// Canonical absolute path.
    Absolute,
    /// Project-relative path with `/` separators.
    Relative,
}

/// Read one entry's path for the clipboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopyPathRequest {
    /// Project generation the request was built against.
    pub generation: u64,
    /// Canonical source whose path is copied.
    pub source: SourcePath,
    /// Requested form.
    pub form: PathForm,
}

/// A created entry and its canonical source identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedEntry {
    source: SourcePath,
    absolute: PathBuf,
}

impl CreatedEntry {
    /// Canonical project-relative source.
    pub fn source(&self) -> &SourcePath {
        &self.source
    }

    /// Absolute path of the written file.
    pub fn absolute(&self) -> &Path {
        &self.absolute
    }
}

/// A duplicated entry and its new canonical source identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicatedEntry {
    from: SourcePath,
    source: SourcePath,
    absolute: PathBuf,
}

impl DuplicatedEntry {
    /// Source that was copied.
    pub fn from(&self) -> &SourcePath {
        &self.from
    }

    /// New canonical source.
    pub fn source(&self) -> &SourcePath {
        &self.source
    }

    /// Absolute path of the copy.
    pub fn absolute(&self) -> &Path {
        &self.absolute
    }
}

/// A moved entry and both canonical source identities.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovedEntry {
    from: SourcePath,
    to: SourcePath,
    absolute: PathBuf,
}

impl MovedEntry {
    /// Previous canonical source.
    pub fn from(&self) -> &SourcePath {
        &self.from
    }

    /// New canonical source.
    pub fn to(&self) -> &SourcePath {
        &self.to
    }

    /// Absolute path of the moved file.
    pub fn absolute(&self) -> &Path {
        &self.absolute
    }
}

/// A trashed entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashedEntry {
    source: SourcePath,
}

impl TrashedEntry {
    /// Canonical source that was moved to the trash.
    pub fn source(&self) -> &SourcePath {
        &self.source
    }
}

/// The directory containing one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevealTarget {
    directory: PathBuf,
}

impl RevealTarget {
    /// Directory to open in the file manager.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

/// Path text copied for one entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopiedPath {
    text: String,
    form: PathForm,
}

impl CopiedPath {
    /// Path text placed on the clipboard.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Form that was requested.
    pub fn form(&self) -> PathForm {
        self.form
    }
}

/// A refused or failed entry operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationError {
    /// The request was built for a different project generation.
    StaleGeneration {
        /// Generation carried by the request.
        request: u64,
        /// Generation currently published.
        current: u64,
    },
    /// A file name or path segment is unsafe.
    InvalidName(String),
    /// The extension is not a supported content extension.
    UnsupportedExtension(String),
    /// The destination already exists.
    Collision(PathBuf),
    /// The path resolves outside the project root.
    OutsideProject(PathBuf),
    /// A symlink appeared where the operation requires a real path.
    Symlink(PathBuf),
    /// The source is not a regular file.
    NotAFile(PathBuf),
    /// The path is not a directory.
    NotADirectory(PathBuf),
    /// The source or directory does not exist.
    Missing(PathBuf),
    /// A filesystem operation failed.
    Io {
        /// Affected path.
        path: PathBuf,
        /// Operating-system message.
        message: String,
    },
    /// The desktop trash refused the file.
    TrashFailed {
        /// Affected path.
        path: PathBuf,
        /// Backend message.
        message: String,
    },
    /// The copy did not preserve the source byte length.
    CopyMismatch(PathBuf),
    /// Every `-N` collision suffix was taken.
    NameAttemptsExhausted,
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StaleGeneration { request, current } => write!(
                formatter,
                "the project changed (request generation {request}, current {current})"
            ),
            Self::InvalidName(name) => write!(formatter, "invalid file name {name:?}"),
            Self::UnsupportedExtension(extension) => {
                write!(formatter, "unsupported entry extension .{extension}")
            }
            Self::Collision(path) => {
                write!(formatter, "an entry already exists at {}", path.display())
            }
            Self::OutsideProject(path) => {
                write!(formatter, "{} is outside the project", path.display())
            }
            Self::Symlink(path) => {
                write!(formatter, "symlinks are not allowed: {}", path.display())
            }
            Self::NotAFile(path) => write!(formatter, "{} is not a regular file", path.display()),
            Self::NotADirectory(path) => {
                write!(formatter, "{} is not a directory", path.display())
            }
            Self::Missing(path) => write!(formatter, "{} does not exist", path.display()),
            Self::Io { path, message } => write!(formatter, "{}: {message}", path.display()),
            Self::TrashFailed { path, message } => {
                write!(formatter, "could not trash {}: {message}", path.display())
            }
            Self::CopyMismatch(path) => {
                write!(
                    formatter,
                    "the copy at {} is not byte-identical",
                    path.display()
                )
            }
            Self::NameAttemptsExhausted => {
                write!(
                    formatter,
                    "no free file name after {MAX_NAME_ATTEMPTS} attempts"
                )
            }
        }
    }
}

impl std::error::Error for OperationError {}

/// Move one path to the desktop trash.
///
/// Implementations must never fall back to permanent deletion.
pub trait TrashBackend: Send + Sync {
    /// Move `path` to the desktop trash.
    fn trash(&self, path: &Path) -> Result<(), String>;
}

/// The platform trash implementation.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemTrash;

impl TrashBackend for SystemTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        trash::delete(path).map_err(|error| error.to_string())
    }
}

/// Create one new entry.
pub fn create(
    scope: &ProjectScope,
    request: &CreateRequest,
) -> Result<CreatedEntry, OperationError> {
    let root = check_generation(scope, request.generation)?;
    let directory = match &request.directory {
        Some(directory) => resolve_directory(&root, directory)?,
        None => root.clone(),
    };
    let extension = request.extension.as_str();
    let stem = match &request.name {
        NewEntryName::Date(date) => {
            validate_date_stem(date)?;
            date.clone()
        }
        NewEntryName::Explicit(name) => {
            validate_file_name(name)?;
            name.clone()
        }
    };
    let target = match &request.name {
        NewEntryName::Date(_) => unique_create_path(&directory, &stem, extension)?,
        NewEntryName::Explicit(_) => {
            let target = directory.join(format!("{stem}.{extension}"));
            refuse_existing(&target)?;
            target
        }
    };
    let content = render_frontmatter(&request.fields)?;
    write_new_file(&target, content.as_bytes())?;
    let source = source_of(&root, &target)?;
    tracing::info!(
        path = %target.display(),
        collection = request.collection.as_str(),
        "created a collection entry"
    );
    Ok(CreatedEntry {
        source,
        absolute: target,
    })
}

/// Duplicate one entry beside itself, preserving its bytes.
pub fn duplicate(
    scope: &ProjectScope,
    request: &DuplicateRequest,
) -> Result<DuplicatedEntry, OperationError> {
    let root = check_generation(scope, request.generation)?;
    let source = resolve_existing_source(&root, &request.source)?;
    let parent = source
        .parent()
        .ok_or_else(|| OperationError::OutsideProject(source.clone()))?;
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| OperationError::InvalidName(source.display().to_string()))?;
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| OperationError::InvalidName(source.display().to_string()))?;
    let target = {
        let mut attempts = 0;
        loop {
            let target = unique_copy_path(parent, stem, extension)?;
            match copy_new_file(&source, &target) {
                Ok(()) => break target,
                Err(OperationError::Collision(_)) if attempts + 1 < MAX_NAME_ATTEMPTS => {
                    attempts += 1;
                }
                Err(error) => return Err(error),
            }
        }
    };
    let original = fs::metadata(&source).map_err(|error| io_error(&source, error))?;
    let copied = fs::metadata(&target).map_err(|error| io_error(&target, error))?;
    if original.len() != copied.len() {
        let _ = fs::remove_file(&target);
        return Err(OperationError::CopyMismatch(target));
    }
    let destination = source_of(&root, &target)?;
    tracing::info!(
        from = %request.source.as_str(),
        to = %destination.as_str(),
        "duplicated a collection entry"
    );
    Ok(DuplicatedEntry {
        from: request.source.clone(),
        source: destination,
        absolute: target,
    })
}

/// Rename or move one entry without touching its content.
pub fn rename(scope: &ProjectScope, request: &MoveRequest) -> Result<MovedEntry, OperationError> {
    let root = check_generation(scope, request.generation)?;
    let source = resolve_existing_source(&root, &request.source)?;
    let destination = resolve_destination(&root, &request.destination)?;
    if source == destination {
        return Ok(MovedEntry {
            from: request.source.clone(),
            to: request.destination.clone(),
            absolute: source,
        });
    }
    refuse_existing(&destination)?;
    // `refuse_existing` plus `rename` is check-then-act: a destination created
    // by another process in between would be replaced on Unix. Closing that
    // window needs `renameat2(RENAME_NOREPLACE)`, which std does not expose.
    fs::rename(&source, &destination).map_err(|error| io_error(&destination, error))?;
    let moved = source_of(&root, &destination)?;
    tracing::info!(
        from = %request.source.as_str(),
        to = %moved.as_str(),
        "renamed a collection entry"
    );
    Ok(MovedEntry {
        from: request.source.clone(),
        to: moved,
        absolute: destination,
    })
}

/// Move one entry to the desktop trash.
pub fn trash(
    scope: &ProjectScope,
    request: &TrashRequest,
    backend: &dyn TrashBackend,
) -> Result<TrashedEntry, OperationError> {
    let root = check_generation(scope, request.generation)?;
    let source = resolve_existing_source(&root, &request.source)?;
    backend
        .trash(&source)
        .map_err(|message| OperationError::TrashFailed {
            path: source.clone(),
            message,
        })?;
    tracing::info!(source = %request.source.as_str(), "moved a collection entry to the trash");
    Ok(TrashedEntry {
        source: request.source.clone(),
    })
}

/// Locate the directory that contains one entry.
pub fn reveal_target(
    scope: &ProjectScope,
    request: &RevealRequest,
) -> Result<RevealTarget, OperationError> {
    let root = check_generation(scope, request.generation)?;
    let lexical = lexical_source(&root, &request.source)?;
    let parent = lexical
        .parent()
        .ok_or_else(|| OperationError::OutsideProject(lexical.clone()))?;
    let directory = fs::canonicalize(parent).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => OperationError::Missing(parent.to_path_buf()),
        _ => io_error(parent, error),
    })?;
    if !directory.starts_with(&root) {
        return Err(OperationError::OutsideProject(directory));
    }
    if !directory.is_dir() {
        return Err(OperationError::NotADirectory(directory));
    }
    Ok(RevealTarget { directory })
}

/// Produce the absolute or project-relative path text for one entry.
pub fn copy_path(
    scope: &ProjectScope,
    request: &CopyPathRequest,
) -> Result<CopiedPath, OperationError> {
    let root = check_generation(scope, request.generation)?;
    let text = match request.form {
        PathForm::Relative => request.source.as_str().to_owned(),
        PathForm::Absolute => {
            let absolute = lexical_source(&root, &request.source)?;
            absolute.to_string_lossy().into_owned()
        }
    };
    Ok(CopiedPath {
        text,
        form: request.form,
    })
}

/// Build the required-field creation plan for one collection schema.
///
/// Only required fields are considered. A required field is representable when
/// it is the mapped title (a generated `New <Singular>`), a date (today), a
/// schema literal, or carries a scalar schema default. Everything else is
/// reported so the UI can collect it or defer to the typed schema form.
pub fn creation_plan(
    schema: Option<&CollectionSchema>,
    collection: &str,
    mappings: &FieldMappings,
    today: &str,
) -> CreationPlan {
    let Some(schema) = schema else {
        return CreationPlan {
            fields: Vec::new(),
            inputs: Vec::new(),
            unsupported: Vec::new(),
        };
    };
    let title_field = mappings.title.as_deref().unwrap_or(DEFAULT_TITLE_FIELD);
    let date_field = mappings.date.as_deref().unwrap_or(DEFAULT_DATE_FIELD);
    let generated_title = format!("New {}", singular(collection));
    let mut plan = CreationPlan {
        fields: Vec::new(),
        inputs: Vec::new(),
        unsupported: Vec::new(),
    };
    for field in &schema.fields {
        if !field.required || plan.fields.iter().any(|entry| entry.name == field.name) {
            continue;
        }
        if field.name == title_field && matches!(scalar_kind(&field.kind), Some(ScalarKind::Text)) {
            plan.fields.push(CreationField {
                name: field.name.clone(),
                value: ScalarValue::String(generated_title.clone()),
            });
            continue;
        }
        if field.name == date_field || matches!(scalar_kind(&field.kind), Some(ScalarKind::Date)) {
            plan.fields.push(CreationField {
                name: field.name.clone(),
                value: ScalarValue::String(today.to_owned()),
            });
            continue;
        }
        if let Some(value) = literal_value(&field.kind).or(field.default.as_ref()) {
            match json_scalar(value) {
                Some(value) => {
                    plan.fields.push(CreationField {
                        name: field.name.clone(),
                        value,
                    });
                    continue;
                }
                None => {
                    plan.unsupported.push(field.name.clone());
                    continue;
                }
            }
        }
        match typed_input(&field.kind) {
            Some(kind) => plan.inputs.push(CreationInput {
                name: field.name.clone(),
                kind,
            }),
            None => match scalar_kind(&field.kind) {
                Some(ScalarKind::Text) => plan.inputs.push(CreationInput {
                    name: field.name.clone(),
                    kind: CreationInputKind::Text,
                }),
                Some(ScalarKind::Number) => plan.inputs.push(CreationInput {
                    name: field.name.clone(),
                    kind: CreationInputKind::Number,
                }),
                Some(ScalarKind::Integer) => plan.inputs.push(CreationInput {
                    name: field.name.clone(),
                    kind: CreationInputKind::Integer,
                }),
                Some(ScalarKind::Boolean) => plan.inputs.push(CreationInput {
                    name: field.name.clone(),
                    kind: CreationInputKind::Boolean,
                }),
                Some(ScalarKind::Date) => plan.inputs.push(CreationInput {
                    name: field.name.clone(),
                    kind: CreationInputKind::Date,
                }),
                Some(ScalarKind::Enum(values)) => plan.inputs.push(CreationInput {
                    name: field.name.clone(),
                    kind: CreationInputKind::Enum(values),
                }),
                None => plan.unsupported.push(field.name.clone()),
            },
        }
    }
    plan
}

/// Typed creation controls for schema kinds that need more than a plain scalar.
fn typed_input(kind: &FieldKind) -> Option<CreationInputKind> {
    match kind {
        FieldKind::Image => Some(CreationInputKind::Image),
        FieldKind::Reference { collections } => Some(CreationInputKind::Reference {
            collections: collections.clone(),
        }),
        FieldKind::Array { items } => match items.as_ref() {
            FieldKind::String { .. } | FieldKind::Enum { .. } => Some(CreationInputKind::Tags),
            FieldKind::Image => Some(CreationInputKind::ImageList),
            FieldKind::Reference { collections } => Some(CreationInputKind::ReferenceList {
                collections: collections.clone(),
            }),
            FieldKind::Nullable { inner } => typed_input(inner).map(|kind| match kind {
                CreationInputKind::Image => CreationInputKind::ImageList,
                CreationInputKind::Reference { collections } => {
                    CreationInputKind::ReferenceList { collections }
                }
                other => other,
            }),
            _ => None,
        },
        FieldKind::Nullable { inner } => typed_input(inner),
        _ => None,
    }
}

/// Render one creation value as a raw string for a pre-write form.
pub fn input_text(input: &CreationInput, today: &str) -> String {
    match &input.kind {
        CreationInputKind::Date => today.to_owned(),
        CreationInputKind::Enum(values) => values.first().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Parse one pre-write form value against the requested control family.
pub fn parse_input(input: &CreationInput, raw: &str) -> Result<ScalarValue, String> {
    let raw = raw.trim();
    match &input.kind {
        CreationInputKind::Text => Ok(ScalarValue::String(raw.to_owned())),
        CreationInputKind::Number => raw
            .parse::<f64>()
            .map(ScalarValue::Real)
            .map_err(|_| format!("{} must be a number", input.name)),
        CreationInputKind::Integer => raw
            .parse::<i64>()
            .map(ScalarValue::Integer)
            .map_err(|_| format!("{} must be an integer", input.name)),
        CreationInputKind::Boolean => match raw.to_ascii_lowercase().as_str() {
            "true" => Ok(ScalarValue::Boolean(true)),
            "false" => Ok(ScalarValue::Boolean(false)),
            _ => Err(format!("{} must be true or false", input.name)),
        },
        CreationInputKind::Date => {
            if crate::list::parse_date(raw).is_some() {
                Ok(ScalarValue::String(raw.to_owned()))
            } else {
                Err(format!("{} must be a date", input.name))
            }
        }
        CreationInputKind::Enum(values) => {
            if values.iter().any(|value| value == raw) {
                Ok(ScalarValue::String(raw.to_owned()))
            } else {
                Err(format!(
                    "{} must be one of {}",
                    input.name,
                    values.join(", ")
                ))
            }
        }
        CreationInputKind::Image => {
            if raw.is_empty() {
                Err(format!("{} needs an image path", input.name))
            } else if raw.chars().any(char::is_control) {
                Err(format!("{} contains an unsupported character", input.name))
            } else {
                Ok(ScalarValue::String(raw.to_owned()))
            }
        }
        CreationInputKind::ImageList => Ok(ScalarValue::List(split_list(raw))),
        CreationInputKind::Reference { .. } => {
            if raw.is_empty() {
                Err(format!("{} needs a reference id", input.name))
            } else {
                Ok(ScalarValue::String(raw.to_owned()))
            }
        }
        CreationInputKind::ReferenceList { .. } => Ok(ScalarValue::List(split_list(raw))),
        CreationInputKind::Tags => Ok(ScalarValue::List(split_list(raw))),
    }
}

/// Split a comma-separated creation list into non-empty trimmed values.
fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Validate one file stem or name segment without touching the filesystem.
pub fn validate_file_name(name: &str) -> Result<(), OperationError> {
    let invalid = || OperationError::InvalidName(name.to_owned());
    if name.is_empty() || name == "." || name == ".." {
        return Err(invalid());
    }
    if name.len() > 255 {
        return Err(invalid());
    }
    if name.starts_with('.') {
        return Err(invalid());
    }
    if name.ends_with(['.', ' ']) {
        return Err(invalid());
    }
    if name
        .chars()
        .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':'))
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_date_stem(date: &str) -> Result<(), OperationError> {
    if crate::list::parse_date(date).is_some() && date.len() == 10 {
        Ok(())
    } else {
        Err(OperationError::InvalidName(date.to_owned()))
    }
}

fn check_generation(scope: &ProjectScope, generation: u64) -> Result<PathBuf, OperationError> {
    if scope.generation != generation {
        return Err(OperationError::StaleGeneration {
            request: generation,
            current: scope.generation,
        });
    }
    canonical_root(scope)
}

fn canonical_root(scope: &ProjectScope) -> Result<PathBuf, OperationError> {
    let root = fs::canonicalize(scope.root()).map_err(|error| io_error(scope.root(), error))?;
    if !root.is_dir() {
        return Err(OperationError::Missing(root));
    }
    Ok(root)
}

fn resolve_directory(root: &Path, relative: &SourcePath) -> Result<PathBuf, OperationError> {
    let lexical = lexical_source(root, relative)?;
    let canonical = fs::canonicalize(&lexical).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => OperationError::Missing(lexical.clone()),
        _ => io_error(&lexical, error),
    })?;
    if !canonical.starts_with(root) {
        return Err(OperationError::OutsideProject(canonical));
    }
    if !canonical.is_dir() {
        return Err(OperationError::NotADirectory(canonical));
    }
    Ok(canonical)
}

fn resolve_existing_source(root: &Path, source: &SourcePath) -> Result<PathBuf, OperationError> {
    let lexical = lexical_source(root, source)?;
    let metadata = fs::symlink_metadata(&lexical).map_err(|error| match error.kind() {
        io::ErrorKind::NotFound => OperationError::Missing(lexical.clone()),
        _ => io_error(&lexical, error),
    })?;
    if metadata.file_type().is_symlink() {
        return Err(OperationError::Symlink(lexical));
    }
    if !metadata.is_file() {
        return Err(OperationError::NotAFile(lexical));
    }
    let canonical = fs::canonicalize(&lexical).map_err(|error| io_error(&lexical, error))?;
    if !canonical.starts_with(root) {
        return Err(OperationError::OutsideProject(canonical));
    }
    Ok(canonical)
}

fn resolve_destination(root: &Path, destination: &SourcePath) -> Result<PathBuf, OperationError> {
    let text = destination.as_str();
    let (parent, name) = match text.rsplit_once('/') {
        Some((parent, name)) => (
            Some(
                SourcePath::new(parent)
                    .map_err(|_| OperationError::InvalidName(text.to_owned()))?,
            ),
            name,
        ),
        None => (None, text),
    };
    validate_file_name(name)?;
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .ok_or_else(|| OperationError::InvalidName(name.to_owned()))?;
    if !matches!(extension, "md" | "mdx") {
        return Err(OperationError::UnsupportedExtension(extension.to_owned()));
    }
    let parent = match parent {
        Some(parent) => resolve_directory(root, &parent)?,
        None => root.to_path_buf(),
    };
    Ok(parent.join(name))
}

/// Join a validated relative source onto the root, rejecting symlink components.
fn lexical_source(root: &Path, source: &SourcePath) -> Result<PathBuf, OperationError> {
    let mut path = root.to_path_buf();
    for segment in source.as_str().split('/') {
        validate_file_name(segment)?;
        path.push(segment);
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && metadata.file_type().is_symlink()
        {
            return Err(OperationError::Symlink(path));
        }
    }
    Ok(path)
}

fn source_of(root: &Path, path: &Path) -> Result<SourcePath, OperationError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| OperationError::OutsideProject(path.to_path_buf()))?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(OperationError::InvalidName(path.display().to_string()));
        };
        let segment = segment
            .to_str()
            .ok_or_else(|| OperationError::InvalidName(path.display().to_string()))?;
        segments.push(segment);
    }
    SourcePath::new(segments.join("/"))
        .map_err(|_| OperationError::InvalidName(path.display().to_string()))
}

fn refuse_existing(path: &Path) -> Result<(), OperationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(OperationError::Symlink(path.into()))
        }
        Ok(_) => Err(OperationError::Collision(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(path, error)),
    }
}

fn unique_create_path(
    directory: &Path,
    stem: &str,
    extension: &str,
) -> Result<PathBuf, OperationError> {
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let name = if attempt == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem}-{attempt}.{extension}")
        };
        let path = directory.join(name);
        if path_is_free(&path)? {
            return Ok(path);
        }
    }
    Err(OperationError::NameAttemptsExhausted)
}

fn unique_copy_path(
    directory: &Path,
    stem: &str,
    extension: &str,
) -> Result<PathBuf, OperationError> {
    for attempt in 0..MAX_NAME_ATTEMPTS {
        let name = if attempt == 0 {
            format!("{stem}-copy.{extension}")
        } else {
            format!("{stem}-copy-{}.{extension}", attempt + 1)
        };
        let path = directory.join(name);
        if path_is_free(&path)? {
            return Ok(path);
        }
    }
    Err(OperationError::NameAttemptsExhausted)
}

fn path_is_free(path: &Path) -> Result<bool, OperationError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(io_error(path, error)),
    }
}

/// Copy `source` to a destination that must not already exist.
///
/// `create_new` makes the existence check atomic, so another process cannot be
/// silently overwritten between the name search and the write.
fn copy_new_file(source: &Path, destination: &Path) -> Result<(), OperationError> {
    let mut input = fs::File::open(source).map_err(|error| io_error(source, error))?;
    let metadata = input.metadata().map_err(|error| io_error(source, error))?;
    if !metadata.is_file() {
        return Err(OperationError::NotAFile(source.to_path_buf()));
    }
    let mut output = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            return Err(OperationError::Collision(destination.to_path_buf()));
        }
        Err(error) => return Err(io_error(destination, error)),
    };
    let copied = io::copy(&mut input, &mut output).and_then(|_| output.sync_all());
    drop(output);
    if let Err(error) = copied {
        let _ = fs::remove_file(destination);
        return Err(io_error(destination, error));
    }
    if let Err(error) = fs::set_permissions(destination, metadata.permissions()) {
        let _ = fs::remove_file(destination);
        return Err(io_error(destination, error));
    }
    Ok(())
}

fn write_new_file(path: &Path, content: &[u8]) -> Result<(), OperationError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => OperationError::Collision(path.to_path_buf()),
            _ => io_error(path, error),
        })?;
    file.write_all(content)
        .map_err(|error| io_error(path, error))
}

fn render_frontmatter(fields: &[CreationField]) -> Result<String, OperationError> {
    if fields.is_empty() {
        return Ok(String::new());
    }
    let mut text = String::from("---\n");
    for field in fields {
        validate_field_name(&field.name)?;
        text.push_str(&field.name);
        text.push_str(": ");
        text.push_str(&render_scalar(&field.value)?);
        text.push('\n');
    }
    text.push_str("---\n\n");
    Ok(text)
}

fn validate_field_name(name: &str) -> Result<(), OperationError> {
    let mut characters = name.chars();
    let valid = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'));
    if valid {
        Ok(())
    } else {
        Err(OperationError::InvalidName(name.to_owned()))
    }
}

fn render_scalar(value: &ScalarValue) -> Result<String, OperationError> {
    Ok(match value {
        ScalarValue::String(text) => {
            if text.chars().any(char::is_control) {
                return Err(OperationError::InvalidName(text.clone()));
            }
            format!("'{}'", text.replace('\'', "''"))
        }
        ScalarValue::Integer(value) => value.to_string(),
        ScalarValue::Real(value) => {
            if !value.is_finite() {
                return Err(OperationError::InvalidName(value.to_string()));
            }
            let mut text = value.to_string();
            if !text.contains(['.', 'e', 'E']) {
                text.push_str(".0");
            }
            text
        }
        ScalarValue::Boolean(value) => value.to_string(),
        ScalarValue::List(values) => {
            let mut rendered = String::from("[");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    rendered.push_str(", ");
                }
                if value.chars().any(char::is_control) {
                    return Err(OperationError::InvalidName(value.clone()));
                }
                rendered.push('\'');
                rendered.push_str(&value.replace('\'', "''"));
                rendered.push('\'');
            }
            rendered.push(']');
            rendered
        }
    })
}

enum ScalarKind {
    Text,
    Number,
    Integer,
    Boolean,
    Date,
    Enum(Vec<String>),
}

fn scalar_kind(kind: &FieldKind) -> Option<ScalarKind> {
    match kind {
        FieldKind::String { .. } => Some(ScalarKind::Text),
        FieldKind::Number => Some(ScalarKind::Number),
        FieldKind::Integer => Some(ScalarKind::Integer),
        FieldKind::Boolean => Some(ScalarKind::Boolean),
        FieldKind::Date => Some(ScalarKind::Date),
        FieldKind::Enum { values } => {
            let values = values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()?;
            Some(ScalarKind::Enum(values))
        }
        FieldKind::Literal { value } => value.is_string().then_some(ScalarKind::Text),
        FieldKind::Nullable { inner } => scalar_kind(inner),
        _ => None,
    }
}

fn literal_value(kind: &FieldKind) -> Option<&Value> {
    match kind {
        FieldKind::Literal { value } => Some(value),
        FieldKind::Nullable { inner } => literal_value(inner),
        _ => None,
    }
}

fn json_scalar(value: &Value) -> Option<ScalarValue> {
    match value {
        Value::String(text) => Some(ScalarValue::String(text.clone())),
        Value::Bool(flag) => Some(ScalarValue::Boolean(*flag)),
        Value::Number(number) => number
            .as_i64()
            .map(ScalarValue::Integer)
            .or_else(|| number.as_f64().map(ScalarValue::Real)),
        _ => None,
    }
}

/// Title-cased singular form of a collection name for generated titles.
fn singular(collection: &str) -> String {
    let word = collection
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(collection)
        .trim();
    let singular = if let Some(stem) = word.strip_suffix("ies") {
        format!("{stem}y")
    } else if word.len() > 1 && word.ends_with('s') && !word.ends_with("ss") {
        word[..word.len() - 1].to_owned()
    } else {
        word.to_owned()
    };
    let mut characters = singular.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => "Entry".to_owned(),
    }
}

fn io_error(path: &Path, error: io::Error) -> OperationError {
    OperationError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}
