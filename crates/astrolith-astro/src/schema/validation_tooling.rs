//! Astro tooling validation: a second, deliberately separate operation.
//!
//! Local checks in [`super::validation`] only understand the normalized schema.
//! Astro itself validates frontmatter with the real Zod schema — including
//! transforms and coercions such as `z.coerce.date()`, image path resolution and
//! default application — and loads every entry of the project. This operation
//! runs the project-local Astro CLI (`astro sync`, the documented content
//! validation path) and maps its result onto revision-tagged diagnostics.
//!
//! What it does *not* prove: `astro sync` validates on-disk content, not the
//! unsaved in-memory buffer; referenced-entry existence is only checked when
//! `getEntry()`/`getEntries()` run at request time; and `.astro`/TypeScript type
//! errors require `astro check`, not this command. Those limits are deliberate
//! and mean one passing tool run must not be treated as proof of every content
//! validation requirement.
//!
//! Results attach to the [`AstroValidationReport::revision`] supplied in the
//! request. Callers reject a stale report with
//! [`AstroValidationReport::reject_if_stale`] or [`accept_for_revision`] after
//! the document revision advances.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::discovery::Cancellation;
use crate::schema::generation::{DEFAULT_TIMEOUT, resolve_astro_entry, run_astro_tool};
use crate::schema::validation::{ValidationDiagnostic, ValidationSeverity, ValidationSource};

/// A request to validate a project's content with its own Astro tooling.
#[derive(Clone, Debug)]
pub struct AstroValidationRequest {
    root: PathBuf,
    revision: u64,
    cancellation: Cancellation,
    node: PathBuf,
    timeout: Duration,
}

impl AstroValidationRequest {
    /// Create a request for `root` that attaches results to `revision`.
    pub fn new(root: impl Into<PathBuf>, revision: u64, cancellation: Cancellation) -> Self {
        Self {
            root: root.into(),
            revision,
            cancellation,
            node: PathBuf::from("node"),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the Node executable (used by tests and non-PATH environments).
    pub fn with_node(mut self, node: impl Into<PathBuf>) -> Self {
        self.node = node.into();
        self
    }

    /// Override the tooling deadline.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The document revision the result will be attached to.
    pub fn revision(&self) -> u64 {
        self.revision
    }
}

/// Overall result of one Astro tooling validation run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AstroValidationStatus {
    /// Astro accepted the content (tool exited zero).
    Passed,
    /// Astro reported errors (tool exited non-zero).
    Failed,
    /// The project-local Astro CLI could not be resolved; validation is unknown.
    Unavailable,
}

/// Revision-tagged diagnostics produced by Astro tooling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AstroValidationReport {
    revision: u64,
    status: AstroValidationStatus,
    diagnostics: Vec<ValidationDiagnostic>,
    tool_status: Option<String>,
}

impl AstroValidationReport {
    /// The document revision this report was produced for.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn status(&self) -> AstroValidationStatus {
        self.status
    }

    /// Whether Astro accepted the content.
    pub fn passed(&self) -> bool {
        self.status == AstroValidationStatus::Passed
    }

    pub fn diagnostics(&self) -> &[ValidationDiagnostic] {
        &self.diagnostics
    }

    /// Raw process exit description, when a tool actually ran.
    pub fn tool_status(&self) -> Option<&str> {
        self.tool_status.as_deref()
    }

    /// Whether this report still describes `current_revision`.
    pub fn is_current_for(&self, current_revision: u64) -> bool {
        self.revision == current_revision
    }

    /// Reject the report if the document advanced past its revision.
    ///
    /// Results are attached to the revision they were requested for. Once the
    /// document revision changes (the user typed again) the tooling result is
    /// obsolete and must not overwrite newer validation feedback.
    pub fn reject_if_stale(&self, current_revision: u64) -> Result<(), ValidationError> {
        if self.is_current_for(current_revision) {
            Ok(())
        } else {
            Err(ValidationError::Stale {
                result_revision: self.revision,
                current_revision,
            })
        }
    }
}

/// Consume a report only if it is still current for `current_revision`.
pub fn accept_for_revision(
    report: AstroValidationReport,
    current_revision: u64,
) -> Result<AstroValidationReport, ValidationError> {
    report.reject_if_stale(current_revision)?;
    Ok(report)
}

/// Failure to run Astro tooling validation at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// The report's revision no longer matches the document.
    Stale {
        result_revision: u64,
        current_revision: u64,
    },
    /// The tool could not run (start failure, timeout or cancellation).
    Tooling(String),
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale {
                result_revision,
                current_revision,
            } => write!(
                formatter,
                "validation result is stale: produced for revision {result_revision}, current revision is {current_revision}"
            ),
            Self::Tooling(message) => {
                write!(formatter, "Astro tooling validation failed: {message}")
            }
        }
    }
}

impl std::error::Error for ValidationError {}

impl From<crate::schema::SchemaError> for ValidationError {
    fn from(error: crate::schema::SchemaError) -> Self {
        Self::Tooling(error.to_string())
    }
}

/// Run the project-local Astro CLI and map its result to revision-tagged
/// diagnostics.
///
/// Intended to run off the UI thread; use [`validate_with_astro_tooling_async`]
/// from an async context. Missing Astro yields an `Unavailable` report rather
/// than an error so callers can surface it as a diagnostic. Timeouts and
/// cancellation are returned as [`ValidationError::Tooling`].
pub fn validate_with_astro_tooling(
    request: &AstroValidationRequest,
) -> Result<AstroValidationReport, ValidationError> {
    request
        .cancellation
        .check()
        .map_err(|error| ValidationError::Tooling(error.to_string()))?;

    let entry = match resolve_astro_entry(&request.root) {
        Ok(entry) => entry,
        Err(error) => {
            return Ok(AstroValidationReport {
                revision: request.revision,
                status: AstroValidationStatus::Unavailable,
                diagnostics: vec![tooling_diagnostic(
                    "<project>",
                    format!("Astro tooling unavailable: {error}"),
                )],
                tool_status: None,
            });
        }
    };

    let run = run_astro_tool(
        &request.root,
        &request.node,
        &entry,
        &["sync"],
        &request.cancellation,
        request.timeout,
        "Astro content validation",
    )?;

    let mut diagnostics = if run.success {
        Vec::new()
    } else {
        parse_tool_diagnostics(&run.stderr, &request.root)
    };
    if !run.success && diagnostics.is_empty() {
        diagnostics.push(tooling_diagnostic(
            "<project>",
            format!("astro sync exited with {}", run.status),
        ));
    }
    let status = if run.success {
        AstroValidationStatus::Passed
    } else {
        AstroValidationStatus::Failed
    };
    Ok(AstroValidationReport {
        revision: request.revision,
        status,
        diagnostics,
        tool_status: Some(run.status),
    })
}

/// Async wrapper that runs validation on the blocking lane.
pub async fn validate_with_astro_tooling_async(
    request: AstroValidationRequest,
) -> Result<AstroValidationReport, ValidationError> {
    tokio::task::spawn_blocking(move || validate_with_astro_tooling(&request))
        .await
        .map_err(|error| ValidationError::Tooling(format!("validation task failed: {error}")))?
}

fn tooling_diagnostic(path: impl Into<String>, message: impl Into<String>) -> ValidationDiagnostic {
    // Tooling diagnostics share the struct but always come from Astro.
    let mut diagnostic = ValidationDiagnostic::tooling(path, message);
    diagnostic.source = ValidationSource::AstroTooling;
    diagnostic.severity = ValidationSeverity::Error;
    diagnostic
}

/// Extract structured errors from Astro's stderr.
///
/// Astro prints blocks like:
/// ```text
/// [InvalidContentEntryDataError] docs → slug data does not match collection schema.
///   Location:
///     /project/content/docs/intro.md:1:2
/// ```
/// Each header line becomes one diagnostic; a following `Location:` line becomes
/// its project-relative path.
fn parse_tool_diagnostics(stderr: &[u8], root: &Path) -> Vec<ValidationDiagnostic> {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text.lines().collect();
    let mut diagnostics = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let header = lines[index].trim();
        if is_error_header(header) {
            let mut path = "<project>".to_owned();
            let mut scan = index + 1;
            while scan < lines.len() && !is_error_header(lines[scan].trim()) {
                if lines[scan].trim() == "Location:" {
                    let mut location = scan + 1;
                    while location < lines.len() && lines[location].trim().is_empty() {
                        location += 1;
                    }
                    if let Some(found) = lines.get(location) {
                        path = relative_location(found.trim(), root);
                    }
                    break;
                }
                scan += 1;
            }
            diagnostics.push(tooling_diagnostic(path, header.to_owned()));
            index = scan;
        } else {
            index += 1;
        }
    }
    diagnostics
}

fn is_error_header(line: &str) -> bool {
    line.starts_with('[') && line.contains(']') && line.to_ascii_lowercase().contains("error")
}

fn relative_location(location: &str, root: &Path) -> String {
    // Drop a trailing `:line:column` (and any surrounding punctuation).
    let without_coords = location
        .rsplitn(3, ':')
        .nth(2)
        .filter(|prefix| !prefix.is_empty())
        .unwrap_or(location)
        .trim_end_matches([':', ' ']);

    let root_text = root.to_string_lossy();
    let relative = without_coords
        .strip_prefix(root_text.as_ref())
        .map(|rest| rest.trim_start_matches(['/', '\\']))
        .unwrap_or(without_coords);
    if relative.is_empty() {
        "<project>".to_owned()
    } else {
        relative.replace('\\', "/")
    }
}

#[cfg(test)]
#[path = "validation_tooling_tests.rs"]
mod tests;
