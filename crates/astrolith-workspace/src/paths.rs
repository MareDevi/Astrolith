//! Project-root policy: the system and credential directories that must never
//! be opened as a project.
//!
//! The predicate is pure so discovery, the project registry and drop handling
//! share exactly one rule. Canonicalization happens in
//! [`validate_project_root`], which every open path uses before any watcher or
//! scan starts.

use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

/// Absolute system directories that cannot contain a user project.
pub const SYSTEM_ROOTS: &[&str] = &[
    "usr", "etc", "bin", "sbin", "lib", "lib32", "lib64", "libx32", "boot", "dev", "proc", "sys",
    "run", "root",
];

/// Home-relative credential directories that are never projects.
pub const CREDENTIAL_DIRECTORIES: &[&str] = &[".ssh", ".aws", ".docker", ".gnupg", ".kube"];

/// Why a directory cannot be opened as a project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockedRoot {
    /// The filesystem root itself.
    FilesystemRoot,
    /// The user's home directory itself.
    HomeDirectory,
    /// A system directory such as `/usr` or `/etc`.
    SystemDirectory,
    /// A credential directory inside the home directory, such as `~/.ssh`.
    CredentialDirectory,
}

impl BlockedRoot {
    /// User-facing explanation shown before an open is refused.
    pub fn message(self) -> &'static str {
        match self {
            Self::FilesystemRoot => {
                "The filesystem root cannot be opened as a project. Choose the project folder instead."
            }
            Self::HomeDirectory => {
                "The home directory cannot be opened as a project. Choose the project folder instead."
            }
            Self::SystemDirectory => {
                "System directories such as /usr and /etc cannot be opened as projects."
            }
            Self::CredentialDirectory => {
                "Credential directories such as ~/.ssh and ~/.aws cannot be opened as projects."
            }
        }
    }
}

/// The current user's home directory from the process environment.
pub fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|home| !home.is_empty()))
        .map(PathBuf::from)
}

/// Refuse a system, credential, root or home directory as a project root.
///
/// `path` must be canonical; callers canonicalize first. The check is lexical
/// so it never touches the filesystem.
pub fn blocked_root(path: &Path, home: Option<&Path>) -> Option<BlockedRoot> {
    let components: Vec<Component<'_>> = path.components().collect();
    if components.is_empty() {
        return Some(BlockedRoot::FilesystemRoot);
    }
    let root_prefix = match components.first() {
        Some(Component::RootDir) => Some(Component::RootDir),
        Some(Component::Prefix(prefix)) => Some(Component::Prefix(*prefix)),
        _ => None,
    };
    if components.len() == 1 && root_prefix.is_some() {
        return Some(BlockedRoot::FilesystemRoot);
    }
    if let Some(home) = home {
        if same_path(path, home) {
            return Some(BlockedRoot::HomeDirectory);
        }
        for name in CREDENTIAL_DIRECTORIES {
            if path.starts_with(home.join(name)) {
                return Some(BlockedRoot::CredentialDirectory);
            }
        }
    }
    let names: Vec<String> = components
        .iter()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    if let Some(first) = names.first()
        && SYSTEM_ROOTS.contains(&first.as_str())
        && root_prefix.is_some()
    {
        return Some(BlockedRoot::SystemDirectory);
    }
    // Windows equivalents such as `C:\Windows` and `C:\Program Files`.
    if root_prefix.is_some()
        && names.first().is_some_and(|first| {
            matches!(
                first.as_str(),
                "windows" | "program files" | "program files (x86)" | "programdata"
            )
        })
    {
        return Some(BlockedRoot::SystemDirectory);
    }
    None
}

/// Canonicalize and validate one directory as an openable project root.
pub fn validate_project_root(
    path: &Path,
    home: Option<&Path>,
) -> Result<PathBuf, ProjectRootError> {
    let canonical = fs::canonicalize(path).map_err(|error| ProjectRootError::Unreadable {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if !canonical.is_dir() {
        return Err(ProjectRootError::NotADirectory(canonical));
    }
    if let Some(blocked) = blocked_root(&canonical, home) {
        return Err(ProjectRootError::Blocked(blocked));
    }
    Ok(canonical)
}

/// A refused or unreadable project root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectRootError {
    /// The directory could not be resolved.
    Unreadable {
        /// The requested path.
        path: PathBuf,
        /// The operating-system message.
        message: String,
    },
    /// The path resolved to something other than a directory.
    NotADirectory(PathBuf),
    /// The directory is protected by [`blocked_root`].
    Blocked(BlockedRoot),
}

impl ProjectRootError {
    /// User-facing explanation of the refusal.
    pub fn message(&self) -> String {
        match self {
            Self::Unreadable { path, message } => {
                format!("Could not open {}: {message}", path.display())
            }
            Self::NotADirectory(path) => {
                format!("Choose a project directory, not {}.", path.display())
            }
            Self::Blocked(blocked) => blocked.message().to_owned(),
        }
    }
}

impl std::fmt::Display for ProjectRootError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for ProjectRootError {}

impl From<io::Error> for ProjectRootError {
    fn from(error: io::Error) -> Self {
        Self::Unreadable {
            path: PathBuf::new(),
            message: error.to_string(),
        }
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_and_credential_directories_are_refused() {
        let home = Path::new("/home/author");
        for path in ["/", "/usr", "/usr/local", "/etc", "/etc/nginx"] {
            assert!(
                matches!(
                    blocked_root(Path::new(path), Some(home)),
                    Some(BlockedRoot::FilesystemRoot | BlockedRoot::SystemDirectory)
                ),
                "{path}"
            );
        }
        for path in [
            "/home/author",
            "/home/author/.ssh",
            "/home/author/.ssh/keys",
            "/home/author/.aws",
            "/home/author/.docker",
        ] {
            assert!(
                matches!(
                    blocked_root(Path::new(path), Some(home)),
                    Some(BlockedRoot::HomeDirectory | BlockedRoot::CredentialDirectory)
                ),
                "{path}"
            );
        }
        assert_eq!(
            blocked_root(Path::new("/home/author/site"), Some(home)),
            None
        );
        assert_eq!(
            blocked_root(Path::new("/home/author/.sshrc/site"), Some(home)),
            None,
            "a prefix that only starts like a credential folder is not one"
        );
    }

    #[test]
    fn validation_canonicalizes_and_reports_non_directories() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("site");
        fs::create_dir(&nested).unwrap();
        let file = directory.path().join("note.md");
        fs::write(&file, "# note\n").unwrap();
        assert_eq!(
            validate_project_root(&nested, None).unwrap(),
            nested.canonicalize().unwrap()
        );
        assert!(matches!(
            validate_project_root(&file, None),
            Err(ProjectRootError::NotADirectory(_))
        ));
        assert!(validate_project_root(&directory.path().join("missing"), None).is_err());
    }
}
