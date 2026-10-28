use crate::{ModelError, validate_identity};

/// Case-sensitive collection name within one project.
///
/// Cross-project registries must additionally key by [`crate::ProjectId`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CollectionId(String);

impl CollectionId {
    /// Preserve the name exactly, rejecting blank/control input.
    pub fn new(name: impl Into<String>) -> Result<Self, ModelError> {
        let name = name.into();
        validate_identity(&name)?;
        Ok(Self(name))
    }

    /// Return the original collection name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
