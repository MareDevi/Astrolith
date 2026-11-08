//! Project-scoped Git operations through the user's installed Git executable.
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Repository discovery and command failure.
#[derive(Debug)]
pub enum GitError {
    /// Git executable is missing or cannot start.
    Unavailable(io::Error),
    /// The selected directory is not inside a Git worktree.
    NotRepository,
    /// Git command failed with its exit code and stderr.
    Command { code: Option<i32>, stderr: String },
    /// Git emitted malformed repository state.
    InvalidOutput(String),
    /// Requested operation cannot proceed with the supplied input or state.
    InvalidOperation(String),
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(error) => write!(f, "Git is unavailable: {error}"),
            Self::NotRepository => f.write_str("This project is not a Git repository."),
            Self::Command { stderr, .. } => write!(f, "Git command failed: {}", stderr.trim()),
            Self::InvalidOutput(message) => write!(f, "Git returned invalid output: {message}"),
            Self::InvalidOperation(message) => write!(f, "Git operation unavailable: {message}"),
        }
    }
}

impl std::error::Error for GitError {}

/// One changed path from `git status --porcelain=v2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedPath {
    /// Path relative to the repository root.
    pub path: PathBuf,
    /// Index status code, such as `M` or `A`.
    pub index_status: char,
    /// Worktree status code, such as `M` or `D`.
    pub worktree_status: char,
    /// Whether this path is untracked.
    pub untracked: bool,
    /// Whether this path has merge conflicts.
    pub conflicted: bool,
    /// Previous path for a rename or copy.
    pub original_path: Option<PathBuf>,
}

/// Current local repository state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// Canonical working-tree root used to resolve changed paths.
    pub repository_root: PathBuf,
    /// Current branch name, or `None` for detached HEAD/unborn state.
    pub branch: Option<String>,
    /// Current commit short object identifier.
    pub head: Option<String>,
    /// Commits ahead of upstream.
    pub ahead: usize,
    /// Commits behind upstream.
    pub behind: usize,
    /// Whether the current branch has an upstream.
    pub has_upstream: bool,
    /// Changed tracked and untracked paths.
    pub paths: Vec<ChangedPath>,
    /// Local branch names.
    pub branches: Vec<String>,
    /// Configured remote names.
    pub remotes: Vec<String>,
}

/// Unified diff for one repository-relative path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDiff {
    /// Patch text returned by Git.
    pub patch: String,
}

/// Git repository rooted at a canonical working-tree directory.
#[derive(Clone, Debug)]
pub struct GitRepository {
    root: PathBuf,
}

impl GitRepository {
    /// Discover a repository containing `path` and retain its worktree root.
    pub fn discover(path: &Path) -> Result<Self, GitError> {
        let output = run_at(
            path,
            [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        )?;
        if !output.status.success() {
            return Err(GitError::NotRepository);
        }
        let root = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        if root.as_os_str().is_empty() {
            return Err(GitError::InvalidOutput("missing worktree root".into()));
        }
        Ok(Self { root })
    }

    /// Repository root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run<I, S>(&self, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = run_at(&self.root, args)?;
        if output.status.success() {
            Ok(output)
        } else {
            Err(GitError::Command {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    /// Read branch, HEAD, upstream counts, and path status.
    pub fn status(&self) -> Result<GitStatus, GitError> {
        let output = self.run([
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
        ])?;
        let mut status = parse_status(&output.stdout)?;
        status.repository_root = self.root.clone();
        status.branches = self.branches()?;
        status.remotes = self.remotes()?;
        Ok(status)
    }

    /// Read the unstaged patch for one path.
    pub fn diff(&self, path: &Path) -> Result<GitDiff, GitError> {
        let path = relative_path(path)?;
        let output = self.run(with_path(
            &["diff", "--no-ext-diff", "--no-color", "--"],
            path,
        ))?;
        if !output.stdout.is_empty() {
            return Ok(GitDiff {
                patch: String::from_utf8_lossy(&output.stdout).into_owned(),
            });
        }
        let tracked = self.run_allow_code(
            with_path(&["ls-files", "--error-unmatch", "--"], path),
            &[1],
        )?;
        if tracked.status.success() {
            return Ok(GitDiff {
                patch: String::new(),
            });
        }
        // `git diff` omits untracked files. Ask Git to render one without
        // modifying the index; exit code 1 means a non-empty diff.
        let output = self.run_allow_code(
            with_path(
                &[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-color",
                    "--",
                    "/dev/null",
                ],
                path,
            ),
            &[1],
        )?;
        Ok(GitDiff {
            patch: String::from_utf8_lossy(&output.stdout).into_owned(),
        })
    }

    /// Read the staged patch for one path.
    pub fn staged_diff(&self, path: &Path) -> Result<GitDiff, GitError> {
        let path = relative_path(path)?;
        let output = self.run(with_path(
            &["diff", "--cached", "--no-ext-diff", "--no-color", "--"],
            path,
        ))?;
        Ok(GitDiff {
            patch: String::from_utf8_lossy(&output.stdout).into_owned(),
        })
    }

    /// Stage one path without interpreting it as a Git option.
    pub fn stage(&self, path: &Path) -> Result<(), GitError> {
        let path = relative_path(path)?;
        self.run(with_path(&["add", "--"], path)).map(drop)
    }

    /// Remove one path from the index while preserving its working-tree file.
    pub fn unstage(&self, path: &Path) -> Result<(), GitError> {
        let path = relative_path(path)?;
        let restore = self.run_allow_code(with_path(&["restore", "--staged", "--"], path), &[1]);
        if restore.as_ref().is_ok_and(|output| output.status.success()) {
            return Ok(());
        }
        // An unborn branch has no HEAD for `restore`; remove only its index
        // entry, leaving the worktree file in place.
        self.run(with_path(&["rm", "--cached", "-f", "--"], path))
            .map(drop)
    }

    /// Commit the current index with the supplied message.
    pub fn commit(&self, message: &str) -> Result<(), GitError> {
        if message.trim().is_empty() {
            return Err(GitError::InvalidOperation(
                "commit message cannot be empty".into(),
            ));
        }
        self.run(["commit", "-m", message]).map(drop)
    }

    /// Local branch names.
    pub fn branches(&self) -> Result<Vec<String>, GitError> {
        let output = self.run(["for-each-ref", "--format=%(refname:short)", "refs/heads"])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Configured remote names.
    pub fn remotes(&self) -> Result<Vec<String>, GitError> {
        let output = self.run(["remote"])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    }

    /// Switch to an existing local branch.
    pub fn checkout(&self, branch: &str) -> Result<(), GitError> {
        if branch.is_empty() || branch.starts_with('-') || branch.contains('\0') {
            return Err(GitError::InvalidOperation("invalid branch name".into()));
        }
        self.run(["switch", "--", branch]).map(drop)
    }

    /// Fetch all configured remotes.
    pub fn fetch(&self) -> Result<(), GitError> {
        self.run(["fetch", "--all", "--prune"]).map(drop)
    }

    /// Push the current branch to its configured upstream.
    pub fn push(&self) -> Result<(), GitError> {
        let upstream = self.run_allow_code(
            [
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{upstream}",
            ],
            &[128],
        )?;
        if upstream.status.success() {
            return self.run(["push"]).map(drop);
        }
        let remotes = self.remotes()?;
        match remotes.as_slice() {
            [remote] => self
                .run(["push", "--set-upstream", remote, "HEAD"])
                .map(drop),
            [] => Err(GitError::InvalidOperation(
                "configure a remote before pushing".into(),
            )),
            _ => Err(GitError::InvalidOperation(
                "set an upstream before pushing to multiple remotes".into(),
            )),
        }
    }

    /// Fast-forward the current branch from its configured upstream.
    pub fn pull(&self) -> Result<(), GitError> {
        self.run(["pull", "--ff-only"]).map(drop)
    }

    fn run_allow_code<I, S>(&self, args: I, accepted: &[i32]) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S> + Clone,
        S: AsRef<OsStr>,
    {
        let output = run_at(&self.root, args)?;
        if output.status.success()
            || output
                .status
                .code()
                .is_some_and(|code| accepted.contains(&code))
        {
            Ok(output)
        } else {
            Err(GitError::Command {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }
}

fn relative_path(path: &Path) -> Result<&OsStr, GitError> {
    use std::path::Component;
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(GitError::InvalidOperation(
            "Git path must stay inside the repository".into(),
        ));
    }
    Ok(path.as_os_str())
}

fn with_path(args: &[&str], path: &OsStr) -> Vec<std::ffi::OsString> {
    args.iter()
        .map(std::ffi::OsString::from)
        .chain(std::iter::once(path.to_owned()))
        .collect()
}

fn run_at<I, S>(path: &Path, args: I) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(GitError::Unavailable)
}

fn parse_status(bytes: &[u8]) -> Result<GitStatus, GitError> {
    let mut status = GitStatus::default();
    let records = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty());
    let mut records = records.peekable();
    while let Some(record) = records.next() {
        let record = String::from_utf8_lossy(record);
        if let Some(value) = record.strip_prefix("# branch.head ") {
            if value != "(detached)" {
                status.branch = Some(value.to_owned());
            }
        } else if let Some(value) = record.strip_prefix("# branch.oid ") {
            if value != "(initial)" {
                status.head = Some(value.to_owned());
            }
        } else if let Some(value) = record.strip_prefix("# branch.upstream ") {
            status.has_upstream = !value.is_empty();
        } else if let Some(value) = record.strip_prefix("# branch.ab ") {
            let mut counts = value.split_ascii_whitespace();
            status.ahead = counts
                .next()
                .and_then(|value| value.strip_prefix('+'))
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| GitError::InvalidOutput("invalid ahead count".into()))?;
            status.behind = counts
                .next()
                .and_then(|value| value.strip_prefix('-'))
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| GitError::InvalidOutput("invalid behind count".into()))?;
        } else if record.starts_with("1 ") || record.starts_with("2 ") || record.starts_with("u ") {
            let kind = record.as_bytes()[0];
            let fields = record
                .splitn(
                    if kind == b'2' {
                        10
                    } else if kind == b'u' {
                        11
                    } else {
                        9
                    },
                    ' ',
                )
                .collect::<Vec<_>>();
            let (xy, path_index) = match kind {
                b'1' if fields.len() == 9 => (fields[1], 8),
                b'2' if fields.len() == 10 => (fields[1], 9),
                b'u' if fields.len() == 11 => (fields[1], 10),
                _ => return Err(GitError::InvalidOutput("malformed path record".into())),
            };
            let mut xy = xy.chars();
            let index_status = xy.next().unwrap_or(' ');
            let worktree_status = xy.next().unwrap_or(' ');
            let conflicted = kind == b'u'
                || matches!(
                    (index_status, worktree_status),
                    ('D', 'D' | 'U') | ('A' | 'U', 'D' | 'U')
                );
            let original_path = if kind == b'2' {
                records
                    .next()
                    .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
            } else {
                None
            };
            status.paths.push(ChangedPath {
                path: PathBuf::from(fields[path_index]),
                index_status,
                worktree_status,
                untracked: false,
                conflicted,
                original_path,
            });
        } else if let Some(path) = record.strip_prefix("? ") {
            status.paths.push(ChangedPath {
                path: PathBuf::from(path),
                index_status: '?',
                worktree_status: '?',
                untracked: true,
                conflicted: false,
                original_path: None,
            });
        }
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(path: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Astrolith Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Astrolith Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn status_parser_reads_branch_counts_and_paths_with_spaces() {
        let output = b"# branch.oid abc123\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\x001 M. N... 100644 100644 100644 a b file with spaces.rs\0? untracked file\0";
        let status = parse_status(output).unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.head.as_deref(), Some("abc123"));
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.paths[0].path, PathBuf::from("file with spaces.rs"));
        assert!(status.paths[1].untracked);
    }

    #[test]
    fn status_parser_reads_rename_source_as_separate_record() {
        let output = b"2 R. N... 100644 100644 100644 a b R100 new name.rs\0old name.rs\0";
        let status = parse_status(output).unwrap();
        assert_eq!(status.paths[0].path, PathBuf::from("new name.rs"));
        assert_eq!(
            status.paths[0].original_path,
            Some(PathBuf::from("old name.rs"))
        );
    }

    #[test]
    fn repository_supports_status_diff_staging_commit_branch_and_remote() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("work tree");
        std::fs::create_dir(&root).unwrap();
        git(&root, &["init", "--initial-branch=main"]);
        git(&root, &["config", "user.name", "Astrolith Test"]);
        git(&root, &["config", "user.email", "test@example.invalid"]);
        let repository = GitRepository::discover(&root).unwrap();

        std::fs::write(root.join("new file.txt"), "first\n").unwrap();
        assert!(repository.status().unwrap().paths[0].untracked);
        assert!(
            repository
                .diff(Path::new("new file.txt"))
                .unwrap()
                .patch
                .contains("+first")
        );
        repository.stage(Path::new("new file.txt")).unwrap();
        assert!(
            !repository
                .staged_diff(Path::new("new file.txt"))
                .unwrap()
                .patch
                .is_empty()
        );
        repository.commit("Add first file").unwrap();
        assert!(
            repository
                .diff(Path::new("new file.txt"))
                .unwrap()
                .patch
                .is_empty()
        );

        std::fs::write(root.join("new file.txt"), "second\n").unwrap();
        repository.stage(Path::new("new file.txt")).unwrap();
        repository.unstage(Path::new("new file.txt")).unwrap();
        assert!(!repository.status().unwrap().paths[0].untracked);
        assert_eq!(
            std::fs::read_to_string(root.join("new file.txt")).unwrap(),
            "second\n"
        );
        assert_eq!(
            repository.commit("  ").unwrap_err().to_string(),
            "Git operation unavailable: commit message cannot be empty"
        );

        git(&root, &["branch", "feature/test"]);
        assert!(
            repository
                .branches()
                .unwrap()
                .contains(&"feature/test".to_owned())
        );
        repository.checkout("feature/test").unwrap();
        assert_eq!(
            repository.status().unwrap().branch.as_deref(),
            Some("feature/test")
        );

        let remote = directory.path().join("remote.git");
        Command::new("git")
            .args(["init", "--bare", remote.to_str().unwrap()])
            .output()
            .unwrap();
        git(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        repository.push().unwrap();
        repository.fetch().unwrap();
        repository.push().unwrap();
        let status = repository.status().unwrap();
        assert!(status.has_upstream);
        assert_eq!(status.remotes, ["origin"]);

        assert!(repository.diff(Path::new("../outside")).is_err());
    }
}
