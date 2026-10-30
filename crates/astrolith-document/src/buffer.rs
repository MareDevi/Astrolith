use astrolith_core::{ProjectId, SourcePath};

/// Monotonic document revision, starting at zero.
///
/// Values advance only when current text changes. Once a buffer reaches
/// [`Self::MAX`], further changed text is rejected instead of wrapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentRevision(u64);

impl DocumentRevision {
    /// The initial revision of every newly created buffer.
    pub const INITIAL: Self = Self(0);
    /// The largest representable document revision.
    pub const MAX: Self = Self(u64::MAX);

    /// Construct a revision from a persisted or transferred value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the underlying revision value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Return the next revision, or an error when this is [`Self::MAX`].
    pub fn checked_next(self) -> Result<Self, DocumentError> {
        self.0
            .checked_add(1)
            .map(Self)
            .ok_or(DocumentError::RevisionOverflow)
    }
}

/// Invalid document state transition or frontmatter problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentError {
    /// Changed text would advance a revision beyond [`u64::MAX`].
    RevisionOverflow,
    /// Frontmatter delimiters or YAML content are malformed.
    InvalidFrontmatter,
    /// The frontmatter uses a construct outside the supported lossless subset.
    UnsupportedFrontmatter {
        /// Why the construct is unsupported.
        reason: String,
    },
    /// A requested field path does not exist in the frontmatter.
    UnknownField {
        /// The dotted field path that was requested.
        path: String,
    },
    /// A field cannot be patched in place without risking data loss.
    UnsupportedPatch {
        /// The dotted field path that was requested.
        path: String,
        /// Why the patch is refused.
        reason: String,
    },
    /// A filesystem or save operation failed.
    Io {
        /// Human-readable description of the failure.
        message: String,
    },
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionOverflow => formatter.write_str("document revision overflowed"),
            Self::InvalidFrontmatter => formatter.write_str("invalid or unterminated frontmatter"),
            Self::UnsupportedFrontmatter { reason } => {
                write!(formatter, "unsupported frontmatter: {reason}")
            }
            Self::UnknownField { path } => {
                write!(formatter, "unknown frontmatter field `{path}`")
            }
            Self::UnsupportedPatch { path, reason } => {
                write!(formatter, "unsupported patch for `{path}`: {reason}")
            }
            Self::Io { message } => write!(formatter, "document I/O failed: {message}"),
        }
    }
}

impl std::error::Error for DocumentError {}

/// Project and canonical source identity for one editable document.
///
/// `project_generation` rejects results produced before a project switch, and
/// [`SourcePath`] is the canonical project-relative source identity supplied by
/// workspace discovery. This type performs no filesystem canonicalization.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DocumentIdentity {
    project: ProjectId,
    project_generation: u64,
    canonical_source: SourcePath,
}

impl DocumentIdentity {
    /// Construct an identity from already validated project and source IDs.
    pub fn new(project: ProjectId, project_generation: u64, canonical_source: SourcePath) -> Self {
        Self {
            project,
            project_generation,
            canonical_source,
        }
    }

    /// Return the workspace project identity.
    pub fn project(&self) -> &ProjectId {
        &self.project
    }

    /// Return the project generation that produced this identity.
    pub fn project_generation(&self) -> u64 {
        self.project_generation
    }

    /// Return the canonical project-relative source path.
    pub fn canonical_source(&self) -> &SourcePath {
        &self.canonical_source
    }
}

/// Identity captured before asynchronous load or parse work begins.
///
/// A result is applicable only while all identity fields and the expected
/// revision still match its retained document buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentLoadTicket {
    identity: DocumentIdentity,
    expected_revision: DocumentRevision,
}

impl DocumentLoadTicket {
    /// Construct a ticket for an independently produced load or parse result.
    pub fn new(identity: DocumentIdentity, expected_revision: DocumentRevision) -> Self {
        Self {
            identity,
            expected_revision,
        }
    }

    /// Return the captured project and source identity.
    pub fn identity(&self) -> &DocumentIdentity {
        &self.identity
    }

    /// Return the revision the buffer must still have for this result to apply.
    pub fn expected_revision(&self) -> DocumentRevision {
        self.expected_revision
    }
}

/// Retained source baseline and current editable text for one document.
#[derive(Clone, Debug)]
pub struct DocumentBuffer {
    identity: DocumentIdentity,
    original_text: String,
    current_text: String,
    revision: DocumentRevision,
}

impl DocumentBuffer {
    /// Create a clean buffer at revision zero from loaded source text.
    pub fn new(identity: DocumentIdentity, source: String) -> Self {
        Self {
            identity,
            original_text: source.clone(),
            current_text: source,
            revision: DocumentRevision::INITIAL,
        }
    }

    /// Create a buffer whose baseline is the on-disk source but whose current
    /// text differs (used when restoring recovered unsaved content).
    pub fn from_unsaved(identity: DocumentIdentity, original: String, current: String) -> Self {
        Self {
            identity,
            original_text: original,
            current_text: current,
            revision: DocumentRevision::INITIAL,
        }
    }

    /// Return the project and source identity retained by this buffer.
    pub fn identity(&self) -> &DocumentIdentity {
        &self.identity
    }

    /// Return the source baseline used for dirty-state comparison.
    pub fn original_text(&self) -> &str {
        &self.original_text
    }

    /// Return the current editable text.
    pub fn current_text(&self) -> &str {
        &self.current_text
    }

    /// Return the current content revision.
    pub fn revision(&self) -> DocumentRevision {
        self.revision
    }

    /// Report whether current text differs from the retained source baseline.
    pub fn is_dirty(&self) -> bool {
        self.current_text != self.original_text
    }

    /// Replace current text and advance the revision only when content changed.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::RevisionOverflow`] without changing the buffer
    /// when changed text would exceed [`DocumentRevision::MAX`].
    pub fn apply_text(&mut self, text: String) -> Result<DocumentRevision, DocumentError> {
        if self.current_text == text {
            return Ok(self.revision);
        }

        let next_revision = self.revision.checked_next()?;
        self.current_text = text;
        self.revision = next_revision;
        Ok(next_revision)
    }

    /// Record a successful save of `saved_revision`'s content.
    ///
    /// The buffer becomes clean only when the saved revision is still the current
    /// revision and `persisted` matches the current text. If the user edited after
    /// the snapshot (a newer revision exists), the buffer stays dirty so the newer
    /// edit is never silently marked saved. Returns whether the buffer was marked
    /// clean.
    pub fn confirm_saved(&mut self, saved_revision: DocumentRevision, persisted: &str) -> bool {
        if self.revision == saved_revision && self.current_text == persisted {
            self.original_text = persisted.to_owned();
            true
        } else {
            false
        }
    }

    /// Capture the identity and revision needed to validate asynchronous work.
    pub fn load_ticket(&self) -> DocumentLoadTicket {
        DocumentLoadTicket::new(self.identity.clone(), self.revision)
    }

    /// Report whether a load or parse result still targets current buffer state.
    pub fn accepts(&self, ticket: &DocumentLoadTicket) -> bool {
        self.identity == ticket.identity && self.revision == ticket.expected_revision
    }

    /// Apply a clean source refresh only when its ticket still matches.
    ///
    /// A stale ticket or dirty buffer returns `false` and leaves all buffer state
    /// unchanged. An applicable refresh returns `true`, replaces the baseline and
    /// current text, preserves clean state, and advances the revision when the
    /// source changed.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::RevisionOverflow`] without changing the buffer
    /// when changed source would exceed [`DocumentRevision::MAX`].
    pub fn refresh_from_load(
        &mut self,
        ticket: &DocumentLoadTicket,
        source: String,
    ) -> Result<bool, DocumentError> {
        if !self.accepts(ticket) || self.is_dirty() {
            return Ok(false);
        }
        if self.current_text == source {
            return Ok(true);
        }

        let next_revision = self.revision.checked_next()?;
        self.original_text = source.clone();
        self.current_text = source;
        self.revision = next_revision;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use astrolith_core::{ProjectId, SourcePath};

    use super::{
        DocumentBuffer, DocumentError, DocumentIdentity, DocumentLoadTicket, DocumentRevision,
    };

    const MDX_SOURCE: &str = concat!(
        "---\r\n",
        "title: \"你好, Astro\"\r\n",
        "draft: false\r\n",
        "---\r\n",
        "import Card from '../../components/Card.astro';\r\n",
        "\r\n",
        "# 見出し\r\n",
        "\r\n",
        "<Card title={\"保持\"}>本文</Card>\r\n",
    );

    fn identity() -> DocumentIdentity {
        DocumentIdentity::new(
            ProjectId::new("project-a").expect("fixture project ID must be valid"),
            7,
            SourcePath::new("src/content/posts/你好.mdx")
                .expect("fixture source path must be valid"),
        )
    }

    #[test]
    fn unchanged_unicode_mdx_crlf_source_stays_clean_at_the_initial_revision() {
        let mut buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());

        assert_eq!(DocumentRevision::INITIAL.get(), 0);
        assert_eq!(buffer.original_text(), MDX_SOURCE);
        assert_eq!(buffer.current_text(), MDX_SOURCE);
        assert!(!buffer.is_dirty());

        let revision = buffer
            .apply_text(MDX_SOURCE.to_owned())
            .expect("an unchanged replacement cannot overflow");

        assert_eq!(revision, DocumentRevision::INITIAL);
        assert_eq!(buffer.original_text(), MDX_SOURCE);
        assert_eq!(buffer.current_text(), MDX_SOURCE);
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn undo_to_the_original_stays_clean_and_redo_advances_revision() {
        let edited = MDX_SOURCE.replace("draft: false", "draft: true");
        let mut buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());

        let first_edit = buffer
            .apply_text(edited.clone())
            .expect("editing below MAX must succeed");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.current_text(), edited);

        let undo = buffer
            .apply_text(MDX_SOURCE.to_owned())
            .expect("undoing below MAX must succeed");
        assert!(undo > first_edit);
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.current_text(), MDX_SOURCE);

        let redo = buffer
            .apply_text(edited.clone())
            .expect("redoing below MAX must succeed");
        assert!(redo > undo);
        assert!(buffer.is_dirty());
        assert_eq!(buffer.current_text(), edited);
    }

    #[test]
    fn changed_text_at_max_revision_is_rejected_without_mutating_the_buffer() {
        let mut buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());
        buffer.revision = DocumentRevision::MAX;

        let unchanged = buffer
            .apply_text(MDX_SOURCE.to_owned())
            .expect("an unchanged replacement must not overflow");
        assert_eq!(unchanged, DocumentRevision::MAX);

        let result = buffer.apply_text(format!("{MDX_SOURCE}\r\nchanged"));

        assert_eq!(result, Err(DocumentError::RevisionOverflow));
        assert_eq!(buffer.current_text(), MDX_SOURCE);
        assert_eq!(buffer.original_text(), MDX_SOURCE);
        assert_eq!(buffer.revision(), DocumentRevision::MAX);
        assert!(!buffer.is_dirty());
    }

    #[test]
    fn stale_load_refresh_cannot_overwrite_an_edit() {
        let ticket = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned()).load_ticket();
        let edited = MDX_SOURCE.replace("本文", "編集済み");
        let mut buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());
        buffer
            .apply_text(edited.clone())
            .expect("editing below MAX must succeed");

        let refreshed = buffer
            .refresh_from_load(&ticket, "disk replacement".to_owned())
            .expect("a stale refresh must not overflow");

        assert!(!refreshed);
        assert_eq!(buffer.original_text(), MDX_SOURCE);
        assert_eq!(buffer.current_text(), edited);
        assert!(buffer.is_dirty());
    }

    #[test]
    fn dirty_buffer_refuses_refresh_even_with_its_current_ticket() {
        let edited = MDX_SOURCE.replace("本文", "編集済み");
        let mut buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());
        let edited_revision = buffer
            .apply_text(edited.clone())
            .expect("editing below MAX must succeed");
        let ticket = buffer.load_ticket();

        let refreshed = buffer
            .refresh_from_load(&ticket, "disk replacement".to_owned())
            .expect("a rejected refresh must not overflow");

        assert!(!refreshed);
        assert_eq!(buffer.original_text(), MDX_SOURCE);
        assert_eq!(buffer.current_text(), edited);
        assert_eq!(buffer.revision(), edited_revision);
        assert!(buffer.is_dirty());
    }

    #[test]
    fn load_ticket_requires_matching_project_generation_source_and_revision() {
        let buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());
        let ticket = buffer.load_ticket();

        assert!(buffer.accepts(&ticket));
        assert!(
            !buffer.accepts(&DocumentLoadTicket::new(
                DocumentIdentity::new(
                    ProjectId::new("project-b").expect("fixture project ID must be valid"),
                    7,
                    SourcePath::new("src/content/posts/你好.mdx")
                        .expect("fixture source path must be valid"),
                ),
                DocumentRevision::INITIAL,
            ))
        );
        assert!(
            !buffer.accepts(&DocumentLoadTicket::new(
                DocumentIdentity::new(
                    ProjectId::new("project-a").expect("fixture project ID must be valid"),
                    8,
                    SourcePath::new("src/content/posts/你好.mdx")
                        .expect("fixture source path must be valid"),
                ),
                DocumentRevision::INITIAL,
            ))
        );
        assert!(
            !buffer.accepts(&DocumentLoadTicket::new(
                DocumentIdentity::new(
                    ProjectId::new("project-a").expect("fixture project ID must be valid"),
                    7,
                    SourcePath::new("src/content/posts/another.mdx")
                        .expect("fixture source path must be valid"),
                ),
                DocumentRevision::INITIAL,
            ))
        );
        assert!(!buffer.accepts(&DocumentLoadTicket::new(
            identity(),
            DocumentRevision::new(1),
        )));
    }

    #[test]
    fn current_load_refresh_replaces_a_clean_source_and_remains_clean() {
        let mut buffer = DocumentBuffer::new(identity(), MDX_SOURCE.to_owned());
        let ticket = buffer.load_ticket();
        let replacement = MDX_SOURCE.replace("draft: false", "draft: true");

        let refreshed = buffer
            .refresh_from_load(&ticket, replacement.clone())
            .expect("refreshing below MAX must succeed");

        assert!(refreshed);
        assert_eq!(buffer.original_text(), replacement);
        assert_eq!(buffer.current_text(), replacement);
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.revision(), DocumentRevision::new(1));
        assert!(!buffer.accepts(&ticket));
    }
}
