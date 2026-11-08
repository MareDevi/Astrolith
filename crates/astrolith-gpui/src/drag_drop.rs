//! Shell-level drag-and-drop policy.
//!
//! The editor already owns asset drops. The shell accepts exactly one directory
//! to open as a project or one Markdown/MDX entry that belongs to the open
//! project. Mixed or multiple drops are refused with a visible message.

use std::path::{Path, PathBuf};

use crate::url_scheme::is_content_entry;

/// What one shell drop should do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DropTarget {
    /// Open this directory as a project.
    Project(PathBuf),
    /// Open this in-project Markdown or MDX entry.
    Entry(PathBuf),
}

/// Classify a shell drop.
///
/// `project_root` is the canonical root of the open project, if any. An entry
/// drop must stay inside it.
pub(crate) fn classify(
    paths: &[PathBuf],
    project_root: Option<&Path>,
) -> Result<DropTarget, String> {
    let [path] = paths else {
        return Err(if paths.is_empty() {
            tr!("services.drop.nothing")
        } else {
            tr!("services.drop.exactly_one")
        });
    };
    if path.is_dir() {
        return Ok(DropTarget::Project(path.clone()));
    }
    if !is_content_entry(path) {
        return Err(tr!("services.drop.unsupported_entry"));
    }
    let Some(root) = project_root else {
        return Err(tr!("services.drop.project_required"));
    };
    if !path.starts_with(root) {
        return Err(tr!("services.drop.outside_project", path = path.display()));
    }
    Ok(DropTarget::Entry(path.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_directory_or_in_project_entry_is_accepted() {
        let directory = tempfile::tempdir().unwrap();
        let project = directory.path().join("site");
        std::fs::create_dir(&project).unwrap();
        let entry = project.join("src/content/a.mdx");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, "# a\n").unwrap();
        let outside = directory.path().join("b.md");
        std::fs::write(&outside, "# b\n").unwrap();

        assert_eq!(
            classify(std::slice::from_ref(&project), None).unwrap(),
            DropTarget::Project(project.clone())
        );
        assert_eq!(
            classify(std::slice::from_ref(&entry), Some(&project)).unwrap(),
            DropTarget::Entry(entry.clone())
        );
        assert!(classify(&[entry.clone(), project.clone()], Some(&project)).is_err());
        assert!(classify(std::slice::from_ref(&entry), None).is_err());
        assert!(classify(&[outside], Some(&project)).is_err());
        assert!(classify(&[], Some(&project)).is_err());

        let other = project.join("notes.txt");
        std::fs::write(&other, "text").unwrap();
        assert!(classify(&[other], Some(&project)).is_err());
    }
}
