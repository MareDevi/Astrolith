//! Bounded project file tree for the Build file navigator.
//!
//! The walk is a best-effort snapshot: dependency, output and generated
//! directories are skipped, symlinks are never followed, and depth, inspected
//! entry and node counts are bounded so a large or hostile project cannot stall
//! the navigator. Unreadable subdirectories are skipped; an unreadable project
//! root is reported so the UI can explain the empty tree.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Directories that never belong in the project tree, matched by exact name.
pub const SKIPPED_DIRECTORIES: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".astro",
    ".cache",
    ".turbo",
    ".next",
    ".vercel",
    ".netlify",
    ".output",
    ".svelte-kit",
];

/// Maximum directory depth below the project root.
const MAX_DEPTH: usize = 16;
/// Maximum directory entries inspected by one scan.
const MAX_INSPECTED: usize = 20_000;
/// Maximum files and directories reported by one scan.
const MAX_NODES: usize = 5_000;

/// One file or directory in the project tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    /// File or directory name, without its parent.
    pub name: String,
    /// Project-relative path using `/` separators.
    pub relative: String,
    /// Whether this node is a directory with `children`.
    pub directory: bool,
    /// Directory contents; empty for files.
    pub children: Vec<ProjectFile>,
}

/// One bounded project tree snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFileTree {
    /// Root children, directories first and then files, both name-sorted.
    pub entries: Vec<ProjectFile>,
    /// Whether a bound stopped the walk before the whole project was listed.
    pub truncated: bool,
}

/// List the project files below `root`.
///
/// `root` must be the canonical project root. Paths are relative to it, which
/// keeps the tree independent of where the project lives on disk.
pub fn scan_project_files(root: &Path) -> io::Result<ProjectFileTree> {
    let mut state = ScanState::default();
    let mut entries = Vec::new();
    walk(root, "", 0, &mut entries, &mut state)?;
    Ok(ProjectFileTree {
        entries,
        truncated: state.truncated,
    })
}

#[derive(Default)]
struct ScanState {
    inspected: usize,
    produced: usize,
    truncated: bool,
}

fn walk(
    directory: &Path,
    relative: &str,
    depth: usize,
    out: &mut Vec<ProjectFile>,
    state: &mut ScanState,
) -> io::Result<()> {
    if depth > MAX_DEPTH || state.produced >= MAX_NODES {
        state.truncated = true;
        return Ok(());
    }
    let read_dir = match fs::read_dir(directory) {
        Ok(read_dir) => read_dir,
        // A permission error below the root hides that subtree, not the project.
        Err(error) if depth > 0 => {
            tracing::debug!(
                path = %directory.display(),
                %error,
                "skipping an unreadable project directory"
            );
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut directories = Vec::new();
    let mut files = Vec::new();
    for entry in read_dir {
        if state.inspected >= MAX_INSPECTED || state.produced >= MAX_NODES {
            state.truncated = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        state.inspected += 1;
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_relative = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        if file_type.is_dir() {
            if SKIPPED_DIRECTORIES.contains(&name.as_str()) {
                continue;
            }
            let mut children = Vec::new();
            walk(
                &entry.path(),
                &child_relative,
                depth + 1,
                &mut children,
                state,
            )?;
            state.produced += 1;
            directories.push(ProjectFile {
                name,
                relative: child_relative,
                directory: true,
                children,
            });
        } else if file_type.is_file() {
            state.produced += 1;
            files.push(ProjectFile {
                name,
                relative: child_relative,
                directory: false,
                children: Vec::new(),
            });
        }
    }
    directories.sort_by(|left, right| compare_names(&left.name, &right.name));
    files.sort_by(|left, right| compare_names(&left.name, &right.name));
    out.extend(directories);
    out.extend(files);
    Ok(())
}

/// Case-insensitive name order with a byte tie-break, so the tree is stable.
fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    left.to_lowercase()
        .cmp(&right.to_lowercase())
        .then_with(|| left.cmp(right))
}

/// Absolute path of one tree node under `root`.
pub fn absolute_path(root: &Path, node: &ProjectFile) -> PathBuf {
    node.relative
        .split('/')
        .fold(root.to_path_buf(), |path, segment| path.join(segment))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_names(entries: &[ProjectFile]) -> Vec<(&str, bool)> {
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.directory))
            .collect()
    }

    #[test]
    fn scan_orders_directories_first_and_skips_dependencies_and_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for path in [
            "src/content/blog/a.md",
            "src/components/Card.astro",
            "src/pages/index.astro",
            ".github/workflows/ci.yml",
            "node_modules/pkg/index.js",
            "dist/index.html",
            ".git/config",
            ".astro/types.d.ts",
        ] {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::write(root.join(".gitignore"), "dist\n").unwrap();

        let tree = scan_project_files(root).unwrap();
        assert!(!tree.truncated);
        assert_eq!(
            tree_names(&tree.entries),
            vec![
                (".github", true),
                ("src", true),
                (".gitignore", false),
                ("package.json", false),
            ]
        );
        let src = &tree.entries[1];
        assert_eq!(
            tree_names(&src.children),
            vec![("components", true), ("content", true), ("pages", true)]
        );
        let content = src
            .children
            .iter()
            .find(|entry| entry.name == "content")
            .unwrap();
        assert_eq!(content.children[0].relative, "src/content/blog");
        assert_eq!(
            content.children[0].children[0].relative,
            "src/content/blog/a.md"
        );
        assert!(absolute_path(root, &content.children[0].children[0]).is_file());
    }

    #[cfg(unix)]
    #[test]
    fn scan_never_follows_symlinked_files_or_directories() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/real.astro"), "").unwrap();
        std::os::unix::fs::symlink(root.join("src"), root.join("linked")).unwrap();
        std::os::unix::fs::symlink(root.join("src/real.astro"), root.join("alias.astro")).unwrap();

        let tree = scan_project_files(root).unwrap();
        assert_eq!(tree_names(&tree.entries), vec![("src", true)]);
        assert_eq!(
            tree_names(&tree.entries[0].children),
            vec![("real.astro", false)]
        );
    }

    #[test]
    fn scan_reports_an_unreadable_root() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");
        assert!(scan_project_files(&missing).is_err());
    }
}
