use crate::{ModelError, validate_identity};

/// Stable opaque project identity, assigned by its owning workspace service.
///
/// This is not a filesystem path or a canonicalization operation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(String);

impl ProjectId {
    /// Preserve the supplied identity exactly, rejecting blank/control input.
    pub fn new(value: impl Into<String>) -> Result<Self, ModelError> {
        let value = value.into();
        validate_identity(&value)?;
        Ok(Self(value))
    }

    /// Return the original identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
