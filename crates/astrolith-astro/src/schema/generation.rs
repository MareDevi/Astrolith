//! R1 schema generation: run a project-local `astro sync`, then load and
//! normalize the schemas it emits under `.astro/collections/`.
//!
//! This is the first Astro service that executes project tooling. It is
//! intentionally narrow: it resolves the installed Astro CLI entry, invokes it
//! with `node <entry> sync` in the project root, bounds the run by a timeout and
//! the shared [`Cancellation`](crate::discovery::Cancellation), caps captured
//! output, and never trusts the generated files without normalizing them.
//!
//! Process-tree cleanup (grandchildren such as esbuild) is the M05 process
//! lifecycle responsibility; here only the direct child is killed on
//! timeout/cancellation.

use std::{
    fs,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::Value;

use crate::{
    config::parse_project_config,
    discovery::{Cancellation, read_bounded},
    schema::model::CollectionSchema,
    schema::parser::{SchemaError, normalize_schema},
};

use super::helpers::enhance_astro_fields;

/// Default wall-clock bound for one `astro sync` run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Captured stdout/stderr is truncated to this many bytes per stream.
pub const MAX_TOOL_OUTPUT: usize = 1024 * 1024;
/// Upper bound on an individual generated schema file.
pub const MAX_SCHEMA_FILE: usize = 1024 * 1024;
/// Upper bound on how many collection schema files a project may expose.
const MAX_COLLECTION_SCHEMAS: usize = 512;

/// A request to generate and load a project's collection schemas.
#[derive(Clone, Debug)]
pub struct SchemaGenerationRequest {
    root: PathBuf,
    cancellation: Cancellation,
    node: PathBuf,
    timeout: Duration,
}

impl SchemaGenerationRequest {
    /// Create a request that runs `node` from `PATH`.
    pub fn new(root: impl Into<PathBuf>, cancellation: Cancellation) -> Self {
        Self {
            root: root.into(),
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

    /// Override the generation deadline.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// One collection's normalized schema, with the file it came from.
#[derive(Clone, Debug)]
pub struct CollectionSchemaEntry {
    pub collection: String,
    pub schema: CollectionSchema,
    pub source: PathBuf,
}

/// The result of generating or loading a project's collection schemas.
#[derive(Clone, Debug, Default)]
pub struct SchemaGenerationReport {
    pub schemas: Vec<CollectionSchemaEntry>,
    pub diagnostics: Vec<String>,
}

/// Generate schemas by running `astro sync`, then load whatever it emitted.
///
/// A non-zero `astro sync` exit is not treated as fatal when schema files were
/// still produced (Astro writes schema files before content validation), but it
/// is reported as a diagnostic.
pub fn generate_collection_schemas(
    request: &SchemaGenerationRequest,
) -> Result<SchemaGenerationReport, SchemaError> {
    let entry = resolve_astro_entry(&request.root)?;
    let run = run_astro_sync(request, &entry)?;

    let mut report = load_generated_schemas(&request.root, &request.cancellation)?;
    if !run.success {
        report.diagnostics.push(format!(
            "astro sync exited with {}: {}",
            run.status,
            excerpt(&run.stderr)
        ));
    }
    if report.schemas.is_empty() {
        let detail = excerpt(&run.stderr);
        return Err(SchemaError::new(if detail.is_empty() {
            "astro sync produced no collection schemas".to_owned()
        } else {
            format!("astro sync produced no collection schemas: {detail}")
        }));
    }
    Ok(report)
}

/// Load and normalize already-generated schemas without running the tool.
pub fn load_generated_schemas(
    root: &Path,
    cancellation: &Cancellation,
) -> Result<SchemaGenerationReport, SchemaError> {
    cancellation
        .check()
        .map_err(|error| SchemaError::new(error.to_string()))?;
    let files = discover_schema_files(root)?;
    let mut report = SchemaGenerationReport::default();
    for file in files {
        cancellation
            .check()
            .map_err(|error| SchemaError::new(error.to_string()))?;
        let Some(collection) = schema_collection_name(&file) else {
            continue;
        };
        let text = match read_bounded(&file, MAX_SCHEMA_FILE) {
            Ok(text) => text,
            Err(error) => {
                report
                    .diagnostics
                    .push(format!("{collection}: unreadable schema ({error})"));
                continue;
            }
        };
        let value: Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                report
                    .diagnostics
                    .push(format!("{collection}: invalid schema JSON ({error})"));
                continue;
            }
        };
        match normalize_schema(&value) {
            Ok(schema) => report.schemas.push(CollectionSchemaEntry {
                collection,
                schema,
                source: file,
            }),
            Err(error) => report.diagnostics.push(format!("{collection}: {error}")),
        }
    }
    apply_astro_helpers(root, &mut report);
    report
        .schemas
        .sort_by(|a, b| a.collection.cmp(&b.collection));
    Ok(report)
}

/// Refine generated schemas with Astro `image()`/`reference()` helper semantics.
///
/// The generated JSON Schema loses these helpers (images look like strings and
/// references like unsupported unions), so the project's own content config
/// source is read and each generated collection is passed through
/// [`enhance_astro_fields`]. A missing or unreadable config is non-fatal: the
/// normalized schemas are still returned, with a diagnostic explaining why the
/// helper pass was skipped. Malformed configs likewise only add diagnostics.
fn apply_astro_helpers(root: &Path, report: &mut SchemaGenerationReport) {
    let source = match resolve_content_config_source(root) {
        Ok(Some(source)) => source,
        Ok(None) => {
            if !report.schemas.is_empty() {
                report
                    .diagnostics
                    .push("Astro helper resolution skipped: content config not found".to_owned());
            }
            return;
        }
        Err(error) => {
            report
                .diagnostics
                .push(format!("Astro helper resolution skipped: {error}"));
            return;
        }
    };

    let mut messages = Vec::new();
    for entry in &mut report.schemas {
        for diagnostic in enhance_astro_fields(&mut entry.schema, &source) {
            messages.push(format!(
                "{}: {}: {}",
                entry.collection, diagnostic.path, diagnostic.message
            ));
        }
    }
    for message in messages {
        if !report.diagnostics.contains(&message) {
            report.diagnostics.push(message);
        }
    }
}

/// Read the project's content config source, honouring the configured `srcDir`.
fn resolve_content_config_source(root: &Path) -> Result<Option<String>, String> {
    let mut directories = Vec::new();
    if let Some(directory) = safe_join(root, &astro_src_dir(root)) {
        directories.push(directory);
    }
    directories.push(root.to_path_buf());

    for directory in directories {
        if let Some(path) = find_config_file(&directory, "content.config") {
            return read_bounded(&path, MAX_SCHEMA_FILE)
                .map(Some)
                .map_err(|error| format!("cannot read {}: {error}", path.display()));
        }
    }
    Ok(None)
}

fn astro_src_dir(root: &Path) -> String {
    if let Some(path) = find_config_file(root, "astro.config")
        && let Ok(text) = read_bounded(&path, MAX_SCHEMA_FILE)
        && let Ok(config) = parse_project_config(&text)
    {
        return config.src_dir;
    }
    "src".to_owned()
}

fn find_config_file(directory: &Path, name: &str) -> Option<PathBuf> {
    for extension in ["ts", "mts", "js", "mjs", "cts", "cjs"] {
        let path = directory.join(format!("{name}.{extension}"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    if relative.is_empty() {
        return Some(root.to_path_buf());
    }
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return None;
    }
    Some(root.join(relative_path))
}

/// Whether the generated schemas are missing or older than the content config.
///
/// A missing `.astro/collections` directory, no schema files, a missing content
/// config, or *any* schema older than `content.config` all mean regenerate.
pub fn schemas_are_stale(root: &Path, content_config: &Path) -> io::Result<bool> {
    let config_modified = match fs::metadata(content_config) {
        Ok(metadata) => metadata.modified()?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(error),
    };
    let directory = root.join(".astro/collections");
    if !directory.is_dir() {
        return Ok(true);
    }
    let mut any = false;
    for entry in fs::read_dir(&directory)? {
        let path = entry?.path();
        if !is_schema_file(&path) {
            continue;
        }
        any = true;
        // A partial sync can leave one collection old while another is new; any
        // schema older than the config means the whole set must be regenerated.
        if fs::metadata(&path)?.modified()? < config_modified {
            return Ok(true);
        }
    }
    Ok(!any)
}

/// Resolve the installed Astro CLI JavaScript entry inside the project.
pub fn resolve_astro_entry(root: &Path) -> Result<PathBuf, SchemaError> {
    let package = root.join("node_modules/astro/package.json");
    if !package.is_file() {
        return Err(SchemaError::new(
            "Astro is not installed in this project (node_modules/astro is missing)",
        ));
    }
    let text = read_bounded(&package, MAX_SCHEMA_FILE).map_err(|error| {
        SchemaError::new(format!("Cannot read Astro package metadata: {error}"))
    })?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| SchemaError::new(format!("Invalid Astro package metadata: {error}")))?;
    let bin = value
        .get("bin")
        .and_then(|bin| match bin {
            Value::String(path) => Some(path.as_str()),
            Value::Object(map) => map.get("astro").and_then(Value::as_str),
            _ => None,
        })
        .ok_or_else(|| SchemaError::new("Astro package does not declare an `astro` binary"))?;

    let package_dir = package
        .parent()
        .ok_or_else(|| SchemaError::new("Astro package path has no parent"))?;
    let entry = package_dir.join(bin).canonicalize().map_err(|error| {
        SchemaError::new(format!("Cannot resolve the Astro CLI entry: {error}"))
    })?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| SchemaError::new(format!("Cannot resolve project root: {error}")))?;
    if !entry.starts_with(&canonical_root) || !entry.is_file() {
        return Err(SchemaError::new(
            "Astro CLI entry escapes the project or is not a file",
        ));
    }
    Ok(entry)
}

/// Outcome of one bounded Astro CLI invocation.
pub(crate) struct ToolRun {
    pub(crate) success: bool,
    pub(crate) status: String,
    pub(crate) stderr: Vec<u8>,
}

fn run_astro_sync(request: &SchemaGenerationRequest, entry: &Path) -> Result<ToolRun, SchemaError> {
    run_astro_tool(
        &request.root,
        &request.node,
        entry,
        &["sync"],
        &request.cancellation,
        request.timeout,
        "Astro schema generation",
    )
}

/// Run the project-local Astro CLI entry with the given arguments under a hard
/// deadline and cooperative cancellation, capturing bounded stderr.
///
/// Shared by schema generation and Astro tooling validation so both use the same
/// process, timeout, cancellation and output bounds. The caller resolves the
/// executable entry first (see [`resolve_astro_entry`]).
pub(crate) fn run_astro_tool(
    root: &Path,
    node: &Path,
    entry: &Path,
    args: &[&str],
    cancellation: &Cancellation,
    timeout: Duration,
    label: &str,
) -> Result<ToolRun, SchemaError> {
    cancellation
        .check()
        .map_err(|error| SchemaError::new(error.to_string()))?;

    // Capture stderr into an anonymous temp file instead of a pipe. A lingering
    // grandchild that inherits the descriptor can then never block a drain, so
    // the deadline stays a hard bound even if a child outlives `node`.
    let mut stderr_file = tempfile::tempfile()
        .map_err(|error| SchemaError::new(format!("Cannot create tool output buffer: {error}")))?;
    let stderr_for_child = stderr_file
        .try_clone()
        .map_err(|error| SchemaError::new(format!("Cannot clone tool output buffer: {error}")))?;

    let mut child = Command::new(node)
        .arg(entry)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_for_child))
        .spawn()
        .map_err(|error| {
            SchemaError::new(format!(
                "Failed to start Astro tooling (`{}`): {error}",
                node.display()
            ))
        })?;

    let started = Instant::now();
    let status = loop {
        if cancellation.check().is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SchemaError::new(format!("{label} cancelled")));
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SchemaError::new(format!(
                    "{label} timed out after {}s",
                    timeout.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SchemaError::new(format!(
                    "Failed while waiting for Astro tooling: {error}"
                )));
            }
        }
    };

    let stderr = read_capped(&mut stderr_file).unwrap_or_default();
    Ok(ToolRun {
        success: status.success(),
        status: status.to_string(),
        stderr,
    })
}

fn read_capped(file: &mut fs::File) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut kept = Vec::new();
    file.take(MAX_TOOL_OUTPUT as u64).read_to_end(&mut kept)?;
    Ok(kept)
}

fn discover_schema_files(root: &Path) -> Result<Vec<PathBuf>, SchemaError> {
    let directory = root.join(".astro/collections");
    if !directory.is_dir() {
        return Err(SchemaError::new(
            "astro sync did not produce `.astro/collections`",
        ));
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| SchemaError::new(format!("Cannot read generated schemas: {error}")))?
    {
        let path = entry
            .map_err(|error| SchemaError::new(format!("Cannot read generated schema: {error}")))?
            .path();
        if is_schema_file(&path) {
            files.push(path);
            if files.len() > MAX_COLLECTION_SCHEMAS {
                return Err(SchemaError::new(format!(
                    "too many generated collection schemas (more than {MAX_COLLECTION_SCHEMAS})"
                )));
            }
        }
    }
    files.sort();
    Ok(files)
}

fn is_schema_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".schema.json"))
}

fn schema_collection_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_suffix(".schema.json"))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

fn excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    const LIMIT: usize = 2000;
    if trimmed.chars().count() <= LIMIT {
        trimmed.to_owned()
    } else {
        let mut shortened: String = trimmed.chars().take(LIMIT).collect();
        shortened.push('…');
        shortened
    }
}

#[cfg(test)]
#[path = "generation_tests.rs"]
mod tests;
