//! Startup and `astrolith://` deep-link routing.
//!
//! Every startup path, deep link and single-instance hand-off is parsed here
//! into one validated [`StartupTarget`], then resolved against the project
//! registry. Content-entry membership is checked after the project index is
//! published, so a link can never open an arbitrary file.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use astrolith_core::preferences::ProjectRecord;
use astrolith_workspace::paths::blocked_root;
use url::Url;

/// URL scheme registered for Astrolith (packaging registers it in P14).
pub(crate) const SCHEME: &str = "astrolith";

/// One validated startup request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StartupTarget {
    /// Open this directory as a project.
    Project(PathBuf),
    /// Open the registered project that owns this Markdown or MDX file.
    Entry(PathBuf),
}

/// Arguments and the first problem found while reading them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StartupRequest {
    /// The first valid target; later arguments are ignored.
    pub target: Option<StartupTarget>,
    /// Why an explicitly supplied argument could not be used.
    pub problem: Option<String>,
}

impl StartupRequest {
    /// Whether launch arguments were supplied at all.
    pub fn has_arguments(&self) -> bool {
        self.target.is_some() || self.problem.is_some()
    }
}

/// Where one startup target routes after the registry lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Open this project root.
    Project(PathBuf),
    /// Open this project root, then this in-project entry.
    Entry {
        /// Canonical project root.
        root: PathBuf,
        /// Canonical entry path.
        entry: PathBuf,
    },
    /// The target cannot be routed; the message is user-facing.
    Invalid(String),
}

/// Parse launch arguments (without the program name).
///
/// Flags such as `--spike=editor` are ignored; the first positional path or
/// `astrolith://open?path=` URL wins.
pub(crate) fn parse_startup(arguments: impl IntoIterator<Item = OsString>) -> StartupRequest {
    let mut request = StartupRequest::default();
    for argument in arguments {
        let Some(text) = argument.to_str() else {
            request.problem.get_or_insert_with(|| {
                "A launch argument is not valid UTF-8 and was ignored.".to_owned()
            });
            continue;
        };
        if text.starts_with('-') {
            continue;
        }
        if text.starts_with("astrolith:") {
            match parse_deep_link(text) {
                Ok(target) => {
                    request.target = Some(target);
                    return request;
                }
                Err(problem) => {
                    request.problem = Some(problem);
                    return request;
                }
            }
        }
        let path = if Path::new(text).is_absolute() {
            PathBuf::from(text)
        } else {
            std::env::current_dir()
                .map(|directory| directory.join(text))
                .unwrap_or_else(|_| PathBuf::from(text))
        };
        match target_from_path(&path) {
            Ok(target) => {
                request.target = Some(target);
                return request;
            }
            Err(problem) => {
                request.problem = Some(problem);
                return request;
            }
        }
    }
    request
}

/// Parse `astrolith://open?path=<absolute path>`.
pub(crate) fn parse_deep_link(raw: &str) -> Result<StartupTarget, String> {
    let url = Url::parse(raw).map_err(|error| format!("Invalid Astrolith link: {error}"))?;
    if url.scheme() != SCHEME {
        return Err(format!(
            "Only {SCHEME}:// links are supported, not {}://.",
            url.scheme()
        ));
    }
    if !url
        .host_str()
        .is_some_and(|host| host.eq_ignore_ascii_case("open"))
    {
        return Err("An Astrolith link must use the `open` host.".to_owned());
    }
    let path = url
        .query_pairs()
        .find(|(name, _)| name == "path")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "The Astrolith link has no `path` parameter.".to_owned())?;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("The Astrolith link path must be absolute.".to_owned());
    }
    target_from_path(&path)
}

fn target_from_path(path: &Path) -> Result<StartupTarget, String> {
    if path.is_dir() {
        return Ok(StartupTarget::Project(path.to_path_buf()));
    }
    if path.is_file() {
        if is_content_entry(path) {
            return Ok(StartupTarget::Entry(path.to_path_buf()));
        }
        return Err(format!(
            "{} is not a Markdown or MDX entry.",
            path.display()
        ));
    }
    Err(format!("{} does not exist.", path.display()))
}

/// Whether a path names a Markdown or MDX entry.
pub(crate) fn is_content_entry(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("mdx")
        })
}

/// Resolve a target against the registered projects.
///
/// File targets require a registered owning project; directory targets are
/// refused for protected system and credential directories.
pub(crate) fn route(
    target: &StartupTarget,
    projects: &[ProjectRecord],
    home: Option<&Path>,
) -> Route {
    match target {
        StartupTarget::Project(path) => {
            if let Some(blocked) = blocked_root(path, home) {
                return Route::Invalid(blocked.message().to_owned());
            }
            Route::Project(path.clone())
        }
        StartupTarget::Entry(path) => {
            if !is_content_entry(path) {
                return Route::Invalid(format!(
                    "{} is not a Markdown or MDX entry.",
                    path.display()
                ));
            }
            // Registry roots are canonical; canonicalize the spelling the
            // launch supplied so a symlinked or relative path still matches.
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let Some(record) = projects
                .iter()
                .filter(|record| {
                    let root = Path::new(&record.path);
                    path.starts_with(root)
                })
                .max_by_key(|record| record.path.len())
            else {
                return Route::Invalid(format!(
                    "{} is not inside a project Astrolith has opened. Open the project once, then try again.",
                    path.display()
                ));
            };
            if let Some(blocked) = blocked_root(Path::new(&record.path), home) {
                return Route::Invalid(blocked.message().to_owned());
            }
            Route::Entry {
                root: PathBuf::from(&record.path),
                entry: path.clone(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, path: &str) -> ProjectRecord {
        ProjectRecord {
            id: id.into(),
            path: path.into(),
            display_name: id.into(),
            package_name: Some(id.into()),
            last_opened_at: 1,
        }
    }

    #[test]
    fn a_relative_startup_argument_becomes_absolute() {
        let directory = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        std::fs::write(directory.path().join("note.md"), "# note\n").unwrap();
        let relative = directory
            .path()
            .strip_prefix(std::env::current_dir().unwrap())
            .unwrap()
            .join("note.md");
        let request = parse_startup([relative.into_os_string()]);
        match request.target {
            Some(StartupTarget::Entry(path)) => assert!(path.is_absolute(), "{path:?}"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_entry_spelling_routes_to_the_registered_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("site");
        std::fs::create_dir_all(root.join("src/content")).unwrap();
        let entry = root.join("src/content/a.md");
        std::fs::write(&entry, "# a\n").unwrap();
        let link = directory.path().join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let record = record("site", root.to_str().unwrap());
        match route(
            &StartupTarget::Entry(link.join("src/content/a.md")),
            &[record],
            None,
        ) {
            Route::Entry {
                root: matched_root,
                entry: matched_entry,
            } => {
                assert_eq!(matched_root, root);
                assert_eq!(matched_entry, std::fs::canonicalize(&entry).unwrap());
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_deep_link_requires_the_scheme_host_and_absolute_path() {
        let directory = tempfile::tempdir().unwrap();
        let encoded = directory.path().to_string_lossy().replace('/', "%2F");
        let target = parse_deep_link(&format!("astrolith://open?path={encoded}")).unwrap();
        assert_eq!(
            target,
            StartupTarget::Project(directory.path().to_path_buf())
        );
        assert!(parse_deep_link("https://open?path=/tmp/site").is_err());
        assert!(parse_deep_link("astrolith://other?path=/tmp/site").is_err());
        assert!(parse_deep_link("astrolith://open").is_err());
        assert!(parse_deep_link("astrolith://open?path=relative.md").is_err());
    }

    #[test]
    fn startup_arguments_accept_flags_a_directory_and_markdown_only() {
        let directory = tempfile::tempdir().unwrap();
        let markdown = directory.path().join("note.md");
        std::fs::write(&markdown, "# note\n").unwrap();
        let other = directory.path().join("note.txt");
        std::fs::write(&other, "text").unwrap();

        let request = parse_startup([
            OsString::from("--spike=editor"),
            OsString::from(directory.path().as_os_str()),
        ]);
        assert_eq!(
            request.target,
            Some(StartupTarget::Project(directory.path().to_path_buf()))
        );
        let request = parse_startup([OsString::from(markdown.as_os_str())]);
        assert_eq!(request.target, Some(StartupTarget::Entry(markdown.clone())));
        let request = parse_startup([OsString::from(other.as_os_str())]);
        assert!(request.target.is_none());
        assert!(request.problem.unwrap().contains("not a Markdown"));
    }

    #[test]
    fn entry_targets_route_through_the_longest_registered_project() {
        let projects = [
            record("site", "/projects/site"),
            record("nested", "/projects/site/packages/nested"),
        ];
        assert_eq!(
            route(
                &StartupTarget::Entry(PathBuf::from(
                    "/projects/site/packages/nested/src/content/a.mdx"
                )),
                &projects,
                None
            ),
            Route::Entry {
                root: PathBuf::from("/projects/site/packages/nested"),
                entry: PathBuf::from("/projects/site/packages/nested/src/content/a.mdx"),
            }
        );
        assert_eq!(
            route(
                &StartupTarget::Entry(PathBuf::from("/projects/site/src/content/a.md")),
                &projects,
                None
            ),
            Route::Entry {
                root: PathBuf::from("/projects/site"),
                entry: PathBuf::from("/projects/site/src/content/a.md"),
            }
        );
        assert!(matches!(
            route(
                &StartupTarget::Entry(PathBuf::from("/elsewhere/a.md")),
                &projects,
                None
            ),
            Route::Invalid(_)
        ));
    }

    #[test]
    fn blocked_project_targets_are_refused() {
        assert!(matches!(
            route(&StartupTarget::Project(PathBuf::from("/usr")), &[], None),
            Route::Invalid(_)
        ));
        assert!(matches!(
            route(
                &StartupTarget::Project(PathBuf::from("/home/author/.ssh")),
                &[],
                Some(Path::new("/home/author"))
            ),
            Route::Invalid(_)
        ));
    }
}
