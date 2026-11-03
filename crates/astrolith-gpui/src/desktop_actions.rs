//! Desktop actions that leave the application: IDE launch and reveal.
//!
//! The IDE command is an argv template with `{project}`, `{file}`, `{line}`
//! and `{column}` placeholders. Arguments are never passed through a shell, and
//! every path must be absolute, inside its project and free of control
//! characters.

use std::{path::Path, process::Stdio};

use astrolith_core::preferences::IdeCommand;

/// One IDE launch target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IdeTarget {
    /// Canonical project root.
    pub project: std::path::PathBuf,
    /// Canonical entry path for file commands.
    pub file: Option<std::path::PathBuf>,
    /// One-based line for file commands.
    pub line: Option<u32>,
    /// One-based column for file commands.
    pub column: Option<u32>,
}

/// Expand one validated IDE command into executable and arguments.
pub(crate) fn ide_argv(ide: &IdeCommand, target: &IdeTarget) -> Result<Vec<String>, String> {
    let executable = ide.executable.trim();
    if executable.is_empty() {
        return Err("No IDE is configured in Preferences.".to_owned());
    }
    if executable.chars().any(|character| character.is_control()) {
        return Err("The configured IDE command contains control characters.".to_owned());
    }
    validate_path(&target.project)?;
    if let Some(file) = &target.file {
        validate_path(file)?;
        if !file.starts_with(&target.project) {
            return Err("The file is outside the project and was not sent to the IDE.".to_owned());
        }
        if !matches!(
            file.extension().and_then(|extension| extension.to_str()),
            Some("md" | "mdx")
        ) {
            return Err("Only Markdown and MDX entries can be opened in the IDE.".to_owned());
        }
    }
    let project = target.project.to_string_lossy();
    let file = target.file.as_ref().map(|file| file.to_string_lossy());
    let line = target.line.unwrap_or(1).to_string();
    let column = target.column.unwrap_or(1).to_string();
    let mut argv = Vec::with_capacity(ide.arguments.len() + 1);
    argv.push(executable.to_owned());
    for argument in &ide.arguments {
        if argument.contains('\0') || argument.chars().any(|character| character.is_control()) {
            return Err("The configured IDE argument contains control characters.".to_owned());
        }
        let mut expanded = argument.replace("{project}", &project);
        if expanded.contains("{file}") {
            let Some(file) = file.as_ref() else {
                return Err(
                    "The IDE command uses {file}, but this action has no file selected.".to_owned(),
                );
            };
            expanded = expanded.replace("{file}", file.as_ref());
        }
        expanded = expanded.replace("{line}", &line);
        expanded = expanded.replace("{column}", &column);
        argv.push(expanded);
    }
    Ok(argv)
}

fn validate_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Only absolute project paths can be sent to the IDE.".to_owned());
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("The IDE path contains a parent-directory segment.".to_owned());
    }
    if path.to_string_lossy().chars().any(char::is_control) {
        return Err("The IDE path contains control characters.".to_owned());
    }
    Ok(())
}

/// Spawn the configured IDE without a shell.
pub(crate) async fn spawn_ide(mut argv: Vec<String>) -> Result<(), String> {
    if argv.is_empty() {
        return Err("No IDE is configured in Preferences.".to_owned());
    }
    let executable = argv.remove(0);
    let child = tokio::process::Command::new(&executable)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start the IDE (`{executable}`): {error}"))?;
    // The IDE is long-lived; reap it in the background so it is never a zombie.
    tokio::spawn(async move {
        let mut child = child;
        let _ = child.wait().await;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ide(arguments: &[&str]) -> IdeCommand {
        IdeCommand {
            executable: "code".into(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).into())
                .collect(),
        }
    }

    fn target() -> IdeTarget {
        IdeTarget {
            project: PathBuf::from("/projects/site"),
            file: Some(PathBuf::from("/projects/site/src/content/a.md")),
            line: Some(12),
            column: Some(4),
        }
    }

    #[test]
    fn placeholders_expand_without_a_shell() {
        let argv = ide_argv(
            &ide(&["--goto", "{file}:{line}:{column}", "{project}"]),
            &target(),
        )
        .unwrap();
        assert_eq!(
            argv,
            [
                "code",
                "--goto",
                "/projects/site/src/content/a.md:12:4",
                "/projects/site"
            ]
        );
        let argv = ide_argv(&ide(&["{project}"]), &target()).unwrap();
        assert_eq!(argv, ["code", "/projects/site"]);
    }

    #[test]
    fn untrusted_paths_and_missing_files_are_rejected() {
        let mut outside = target();
        outside.file = Some(PathBuf::from("/elsewhere/a.md"));
        assert!(ide_argv(&ide(&["{file}"]), &outside).is_err());

        let mut traversal = target();
        traversal.file = Some(PathBuf::from("/projects/site/../secret.md"));
        assert!(ide_argv(&ide(&["{file}"]), &traversal).is_err());

        let mut relative = target();
        relative.project = PathBuf::from("site");
        assert!(ide_argv(&ide(&["{project}"]), &relative).is_err());

        let mut text = target();
        text.file = Some(PathBuf::from("/projects/site/notes.txt"));
        assert!(ide_argv(&ide(&["{file}"]), &text).is_err());

        assert!(ide_argv(&ide(&["{file}"]), &target()).is_ok());
        let project_only = IdeTarget {
            file: None,
            ..target()
        };
        assert!(ide_argv(&ide(&["{file}"]), &project_only).is_err());
        assert!(
            ide_argv(&ide(&["--goto", "{file}:{line}"]), &project_only)
                .unwrap_err()
                .contains("{file}")
        );
    }

    #[test]
    fn an_empty_or_control_configured_command_is_refused() {
        let mut command = ide(&["{project}"]);
        command.executable = "  ".into();
        assert!(ide_argv(&command, &target()).is_err());
        let mut command = ide(&["{project}"]);
        command.arguments = vec!["bad\nargument".into()];
        assert!(ide_argv(&command, &target()).is_err());
    }
}
