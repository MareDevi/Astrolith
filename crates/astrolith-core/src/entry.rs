use crate::{CollectionId, ModelError, validate_identity};

/// Logical entry identity within one project, independent of its source file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId {
    collection: CollectionId,
    id: String,
}

impl EntryId {
    /// Construct a collection-scoped identity without slug or path normalization.
    ///
    /// Custom loader IDs may contain separators and punctuation. Never join an
    /// entry ID to a filesystem root; use its explicit [`Entry::source`] mapping.
    pub fn new(collection: impl Into<String>, id: impl Into<String>) -> Result<Self, ModelError> {
        let collection = CollectionId::new(collection)?;
        let id = id.into();
        validate_identity(&id)?;
        Ok(Self { collection, id })
    }

    /// Return the collection portion of this identity.
    pub fn collection(&self) -> &CollectionId {
        &self.collection
    }

    /// Return the exact loader-provided logical ID.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// Lexically validated UTF-8 source path relative to a project root.
///
/// Uses `/` separators on every platform. Rejects empty, dot, parent, absolute,
/// drive-qualified, backslash and control-containing paths, plus segments ending
/// in dots or spaces (Windows aliases), rather than silently normalizing them.
/// Colons are intentionally unsupported, including valid POSIX filenames, to
/// exclude Windows drive syntax and alternate data stream syntax consistently.
/// This does not prove filesystem containment: discovery and
/// I/O services must verify symlinks and the actual target against their root.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourcePath(String);

impl SourcePath {
    /// Validate a relative source path without touching the filesystem.
    pub fn new(path: impl Into<String>) -> Result<Self, ModelError> {
        let path = path.into();
        if path.chars().any(char::is_control)
            || path.contains(['\\', ':'])
            || path
                .split('/')
                .any(|part| part.is_empty() || part.ends_with(['.', ' ']))
        {
            return Err(ModelError::InvalidSourcePath);
        }
        Ok(Self(path))
    }

    /// Return the exact relative path, using `/` separators.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Explicit mapping from a logical entry to an editable source file.
///
/// Two collections may map different identities to the same source path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    id: EntryId,
    source: SourcePath,
}

impl Entry {
    /// Map an identity to its independently discovered source path.
    pub fn new(id: EntryId, source: SourcePath) -> Self {
        Self { id, source }
    }

    /// Return the logical collection-scoped identity.
    pub fn id(&self) -> &EntryId {
        &self.id
    }

    /// Return the source mapping; never infer it from the logical ID.
    pub fn source(&self) -> &SourcePath {
        &self.source
    }
}
