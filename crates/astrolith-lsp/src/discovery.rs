//! Project-local language server discovery.
//!
//! Astro projects install their language servers as development dependencies,
//! so `node_modules/.bin` is the primary location. Ancestor directories are
//! searched too (workspace hoisting), then `PATH` as a last resort. Nothing is
//! installed or downloaded on the user's behalf.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::language::LanguageId;

/// A resolved server command ready to spawn for one project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSpec {
    /// Stable server identity reported in logs.
    pub server_id: &'static str,
    /// Executable to run.
    pub command: PathBuf,
    /// Arguments, already including the server's fixed `--stdio`.
    pub args: Vec<OsString>,
    /// Canonical project root; also the server's workspace folder.
    pub root: PathBuf,
    /// Language this server was resolved for.
    pub language: LanguageId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryError {
    /// The language has no language server (for example Markdown).
    Unsupported(LanguageId),
    /// The project root could not be canonicalized.
    InvalidRoot { root: PathBuf, message: String },
    /// No server executable was found in the project or on `PATH`.
    NotFound {
        language: LanguageId,
        binaries: &'static [&'static str],
    },
}

impl std::fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(language) => {
                write!(formatter, "no language server is defined for {language:?}")
            }
            Self::InvalidRoot { root, message } => write!(
                formatter,
                "could not resolve the project root {}: {message}",
                root.display()
            ),
            Self::NotFound { language, binaries } => write!(
                formatter,
                "no {:?} language server found (looked for {})",
                language,
                binaries.join(", ")
            ),
        }
    }
}

impl std::error::Error for DiscoveryError {}

/// Find and describe the server for `language` in `root`.
pub fn discover(root: &Path, language: LanguageId) -> Result<ServerSpec, DiscoveryError> {
    let server = language
        .server()
        .ok_or(DiscoveryError::Unsupported(language))?;
    let root = root
        .canonicalize()
        .map_err(|error| DiscoveryError::InvalidRoot {
            root: root.to_path_buf(),
            message: error.to_string(),
        })?;

    for directory in root.ancestors() {
        let bin = directory.join("node_modules").join(".bin");
        for name in server.binaries {
            if let Some(path) = find_executable(&bin, name) {
                return Ok(spec(server.id, path, server.args, root, language));
            }
        }
    }

    let paths: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    for name in server.binaries {
        if let Some(path) = find_in_paths(name, &paths) {
            return Ok(spec(server.id, path, server.args, root, language));
        }
    }

    Err(DiscoveryError::NotFound {
        language,
        binaries: server.binaries,
    })
}

fn spec(
    id: &'static str,
    command: PathBuf,
    args: &'static [&'static str],
    root: PathBuf,
    language: LanguageId,
) -> ServerSpec {
    #[cfg(windows)]
    {
        let is_script = command
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            });
        if is_script {
            let mut resolved: Vec<OsString> = vec![OsString::from("/C"), command.into()];
            resolved.extend(args.iter().map(OsString::from));
            return ServerSpec {
                server_id: id,
                command: PathBuf::from("cmd"),
                args: resolved,
                root,
                language,
            };
        }
    }
    ServerSpec {
        server_id: id,
        command,
        args: args.iter().map(OsString::from).collect(),
        root,
        language,
    }
}

/// Look for an executable in one `node_modules/.bin` directory.
fn find_executable(directory: &Path, name: &str) -> Option<PathBuf> {
    for candidate in candidates(name) {
        let path = directory.join(candidate);
        if is_executable(&path) {
            return Some(path);
        }
    }
    None
}

fn find_in_paths(name: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    for candidate in candidates(name) {
        for directory in paths {
            let path = directory.join(&candidate);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

fn candidates(name: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            format!("{name}.cmd"),
            format!("{name}.exe"),
            format!("{name}.bat"),
            name.to_owned(),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![name.to_owned()]
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(not(unix))]
    fn executable(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn finds_a_project_local_astro_server() {
        let project = tempfile::tempdir().unwrap();
        let bin = project.path().join("node_modules/.bin/astro-ls");
        executable(&bin);
        let spec = discover(project.path(), LanguageId::Astro).unwrap();
        assert_eq!(spec.server_id, "astro");
        assert_eq!(spec.command, bin);
        assert_eq!(spec.args, [OsString::from("--stdio")]);
        assert_eq!(spec.root, project.path().canonicalize().unwrap());
    }

    #[test]
    fn finds_a_hoisted_server_above_the_project() {
        let workspace = tempfile::tempdir().unwrap();
        let bin = workspace
            .path()
            .join("node_modules/.bin/typescript-language-server");
        executable(&bin);
        let project = workspace.path().join("apps/site");
        fs::create_dir_all(&project).unwrap();
        let spec = discover(&project, LanguageId::TypeScript).unwrap();
        assert_eq!(spec.command, bin);
    }

    #[test]
    fn reports_a_missing_server_with_its_candidates() {
        let project = tempfile::tempdir().unwrap();
        let error = discover(project.path(), LanguageId::Svelte).unwrap_err();
        assert_eq!(
            error,
            DiscoveryError::NotFound {
                language: LanguageId::Svelte,
                binaries: &["svelteserver"],
            }
        );
        assert!(error.to_string().contains("svelteserver"));
    }

    #[test]
    fn reports_languages_without_a_server() {
        let project = tempfile::tempdir().unwrap();
        assert_eq!(
            discover(project.path(), LanguageId::Markdown),
            Err(DiscoveryError::Unsupported(LanguageId::Markdown))
        );
    }

    #[test]
    fn explains_an_unreadable_root() {
        let missing = std::env::temp_dir().join("astrolith-lsp-missing-root");
        let error = discover(&missing, LanguageId::Astro).unwrap_err();
        assert!(matches!(error, DiscoveryError::InvalidRoot { .. }));
    }

    #[test]
    fn finds_an_executable_in_explicit_paths() {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("astro-ls");
        executable(&bin);
        assert_eq!(
            find_in_paths("astro-ls", &[directory.path().to_path_buf()]),
            Some(bin)
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_non_executable_file() {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join("astro-ls");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&bin, "").unwrap();
        assert!(find_in_paths("astro-ls", &[directory.path().to_path_buf()]).is_none());
    }
}
