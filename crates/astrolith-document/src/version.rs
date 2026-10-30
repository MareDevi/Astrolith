//! Disk baseline used for conflict-aware saving.

/// The exact bytes a document had on disk when it was loaded.
///
/// A save is applied only if the file still matches this baseline, so an
/// external edit that happened after load produces a conflict instead of being
/// silently overwritten.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskBaseline {
    content: String,
}

impl DiskBaseline {
    /// Capture a baseline from previously loaded source text.
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }

    /// Borrow the baseline bytes.
    pub fn as_bytes(&self) -> &[u8] {
        self.content.as_bytes()
    }

    /// Borrow the baseline as text.
    pub fn as_str(&self) -> &str {
        &self.content
    }

    /// Report whether `current` disk bytes still equal this baseline.
    pub fn matches(&self, current: &[u8]) -> bool {
        self.content.as_bytes() == current
    }
}
