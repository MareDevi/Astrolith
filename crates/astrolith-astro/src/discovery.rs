//! Bounded synchronous inspection. Call only from the application's blocking lane.
use crate::config::{parse_content_config, parse_project_config};
use astrolith_core::{CollectionId, ProjectId};
use std::{
    fs,
    io::{self, Read},
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Exact releases whose loader semantics are checked by the fixture task.
pub const SUPPORTED_ASTRO_VERSIONS: &[&str] = &["5.18.2", "7.4.4"];
/// Cooperative cancellation shared by discovery and index services.
#[derive(Clone, Default, Debug)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), AstroError> {
        if self.0.load(Ordering::Acquire) {
            Err(AstroError("Operation cancelled".into()))
        } else {
            Ok(())
        }
    }
}
#[derive(Debug)]
pub struct AstroError(String);
impl std::fmt::Display for AstroError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AstroError {}
impl From<io::Error> for AstroError {
    fn from(value: io::Error) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveryRequest {
    project: ProjectId,
    generation: u64,
    root: PathBuf,
    cancellation: Cancellation,
}
impl DiscoveryRequest {
    pub fn new(
        project: ProjectId,
        generation: u64,
        root: PathBuf,
        cancellation: Cancellation,
    ) -> Self {
        Self {
            project,
            generation,
            root,
            cancellation,
        }
    }
    pub fn project(&self) -> &ProjectId {
        &self.project
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }
}
#[derive(Clone, Debug)]
pub struct ProjectMetadata {
    root: PathBuf,
    requirement: String,
    installed: Option<String>,
}
impl ProjectMetadata {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn requirement(&self) -> &str {
        &self.requirement
    }
    pub fn installed_version(&self) -> Option<&str> {
        self.installed.as_deref()
    }
}
#[derive(Clone, Debug)]
pub struct CollectionDescriptor {
    id: CollectionId,
    base: PathBuf,
    pattern: String,
    config: PathBuf,
}
impl CollectionDescriptor {
    pub fn id(&self) -> &CollectionId {
        &self.id
    }
    pub fn base(&self) -> &Path {
        &self.base
    }
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
    pub fn config(&self) -> &Path {
        &self.config
    }
}
#[derive(Clone, Debug)]
pub struct ProjectManifest {
    metadata: ProjectMetadata,
    project: ProjectId,
    generation: u64,
    collections: Vec<CollectionDescriptor>,
}
impl ProjectManifest {
    pub fn root(&self) -> &Path {
        self.metadata.root()
    }
    pub fn metadata(&self) -> &ProjectMetadata {
        &self.metadata
    }
    pub fn project(&self) -> &ProjectId {
        &self.project
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn collections(&self) -> &[CollectionDescriptor] {
        &self.collections
    }
}
#[derive(Clone, Debug)]
pub enum DiscoveryOutcome {
    Ready(ProjectManifest),
    Unsupported {
        metadata: ProjectMetadata,
        diagnostics: Vec<String>,
    },
}

/// Reads a regular file without accepting unbounded growth after metadata inspection.
/// Callers must separately enforce root containment for editable/configuration paths.
pub fn read_bounded(path: &Path, limit: usize) -> io::Result<String> {
    if !fs::metadata(path)?.is_file() {
        return Err(io::Error::other("Choose a regular file"));
    }
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Choose a regular file"));
    }
    read_utf8_bounded(file, limit)
}
fn read_utf8_bounded(reader: impl Read, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::other("File exceeds supported size"));
    }
    String::from_utf8(bytes).map_err(|_| io::Error::other("Choose a UTF-8 file"))
}
/// Canonicalize an explicitly relative project path, rejecting escape and aliases.
pub fn contained_path(root: &Path, relative: &Path) -> io::Result<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|p| matches!(p, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(io::Error::other("Path must stay within project root"));
    }
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(root) {
        return Err(io::Error::other("Symlink escapes project root"));
    }
    Ok(path)
}
fn candidate(root: &Path, directory: &Path, name: &str) -> Result<Option<PathBuf>, AstroError> {
    let mut found = Vec::new();
    for ext in ["js", "mjs", "ts", "mts", "cts", "cjs"] {
        let path = directory.join(format!("{name}.{ext}"));
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                if matches!(ext, "mts" | "cts" | "cjs") {
                    return Err(AstroError(format!(
                        "Unsupported config extension: {}",
                        path.display()
                    )));
                }
                let canonical = path.canonicalize()?;
                if !canonical.starts_with(root) {
                    return Err(AstroError("Config symlink escapes project root".into()));
                }
                found.push(canonical);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if found.len() > 1 {
        return Err(AstroError(format!("Ambiguous {name} configuration")));
    }
    Ok(found.pop())
}
fn json(path: &Path) -> Result<serde_json::Value, AstroError> {
    serde_json::from_str(&read_bounded(path, 1024 * 1024)?)
        .map_err(|error| AstroError(format!("Invalid package metadata: {error}")))
}
/// Inspect a selected root without evaluating project JavaScript or walking content.
pub fn inspect_project(request: &DiscoveryRequest) -> Result<DiscoveryOutcome, AstroError> {
    let cancellation = request.cancellation();
    cancellation.check()?;
    let root = request.root.canonicalize()?;
    if !root.is_dir() {
        return Err(AstroError("Choose a project directory".into()));
    }
    let package_path = contained_path(&root, Path::new("package.json"))?;
    let package = json(&package_path)?;
    if !package.is_object() {
        return Err(AstroError("package.json must be an object".into()));
    }
    let mut requirements = Vec::new();
    for section in ["dependencies", "devDependencies"] {
        if let Some(dependencies) = package.get(section) {
            if !dependencies.is_object() {
                return Err(AstroError(format!("{section} must be an object")));
            }
            if let Some(value) = dependencies.get("astro") {
                let value = value
                    .as_str()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| {
                        AstroError("Astro dependency must be a nonempty string".into())
                    })?;
                requirements.push(value.to_owned());
            }
        }
    }
    let Some(requirement) = requirements.first().cloned() else {
        return Err(AstroError("No Astro dependency in this project".into()));
    };
    if requirements.iter().any(|v| v != &requirement) {
        return Err(AstroError("Conflicting Astro dependencies".into()));
    }
    cancellation.check()?;
    let installed_path = root.join("node_modules/astro/package.json");
    let installed = match json(&installed_path) {
        Ok(value) => value
            .get("version")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        Err(_) => None,
    };
    let metadata = ProjectMetadata {
        root: root.clone(),
        requirement,
        installed,
    };
    let unsupported = |message: String| DiscoveryOutcome::Unsupported {
        metadata: metadata.clone(),
        diagnostics: vec![message],
    };
    if !metadata
        .installed_version()
        .is_some_and(|v| SUPPORTED_ASTRO_VERSIONS.contains(&v))
    {
        return Ok(unsupported("Install a fixture-verified project-local Astro release; unsupported or missing installed version".into()));
    }
    let astro_config = match candidate(&root, &root, "astro.config") {
        Ok(path) => path,
        Err(error) => {
            if error.0.contains("symlink") {
                return Err(error);
            }
            return Ok(unsupported(error.to_string()));
        }
    };
    let (src_dir, mdx_integration) = if let Some(path) = astro_config {
        match parse_project_config(&read_bounded(&path, 1024 * 1024)?) {
            Ok(config) => (config.src_dir, config.mdx),
            Err(error) => return Ok(unsupported(error)),
        }
    } else {
        ("src".into(), false)
    };
    cancellation.check()?;
    let src = match contained_path(&root, Path::new(&src_dir)) {
        Ok(path) if path.is_dir() => path,
        _ => {
            return Ok(unsupported(
                "Unsupported or missing source directory".into(),
            ));
        }
    };
    let modern = match candidate(&root, &src, "content.config") {
        Ok(path) => path,
        Err(error) => {
            if error.0.contains("symlink") {
                return Err(error);
            }
            return Ok(unsupported(error.to_string()));
        }
    };
    let legacy = match candidate(&root, &src.join("content"), "config") {
        Ok(path) => path,
        Err(error) => return Ok(unsupported(error.to_string())),
    };
    if legacy.is_some() {
        return Ok(unsupported(
            "Legacy collection config is unsupported; use modern content.config".into(),
        ));
    }
    let Some(config) = modern else {
        return Ok(unsupported("No modern content configuration".into()));
    };
    let parsed = match parse_content_config(&read_bounded(&config, 1024 * 1024)?) {
        Ok(parsed) => parsed,
        Err(error) => return Ok(unsupported(error)),
    };
    if parsed
        .iter()
        .any(|collection| collection.pattern().contains("mdx"))
    {
        if !mdx_integration {
            return Ok(unsupported(
                "MDX patterns require a configured fixture-verified @astrojs/mdx integration"
                    .into(),
            ));
        }
        let expected = match metadata.installed_version() {
            Some("5.18.2") => "4.3.14",
            Some("7.4.4") => "8.1.2",
            _ => return Ok(unsupported("Unsupported Astro/MDX version pair".into())),
        };
        let mdx = json(&root.join("node_modules/@astrojs/mdx/package.json")).ok();
        if mdx
            .as_ref()
            .and_then(|value| value.get("version"))
            .and_then(|value| value.as_str())
            != Some(expected)
        {
            return Ok(unsupported(format!(
                "MDX patterns require installed @astrojs/mdx {expected} for this Astro release"
            )));
        }
    }
    let mut collections = Vec::new();
    for item in parsed {
        cancellation.check()?;
        let base = match contained_path(&root, Path::new(item.base())) {
            Ok(path) if path.is_dir() => path,
            _ => {
                return Ok(unsupported(format!(
                    "Invalid local collection base: {}",
                    item.base()
                )));
            }
        };
        let id =
            CollectionId::new(item.collection()).map_err(|error| AstroError(error.to_string()))?;
        collections.push(CollectionDescriptor {
            id,
            base,
            pattern: item.pattern().into(),
            config: config.clone(),
        });
    }
    cancellation.check()?;
    Ok(DiscoveryOutcome::Ready(ProjectManifest {
        metadata,
        project: request.project.clone(),
        generation: request.generation,
        collections,
    }))
}

#[cfg(test)]
mod bounded_read_tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn growth_after_initial_read_still_hits_the_byte_cap() {
        struct GrowingFile {
            reader: fs::File,
            writer: fs::File,
            grew: bool,
        }
        impl Read for GrowingFile {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                if !self.grew {
                    self.grew = true;
                    let count = self.reader.read(&mut buffer[..1])?;
                    self.writer.write_all(b"01234567890123456789")?;
                    self.writer.flush()?;
                    Ok(count)
                } else {
                    self.reader.read(buffer)
                }
            }
        }
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), b"a").unwrap();
        let initial_length = fs::metadata(temp.path()).unwrap().len();
        let reader = GrowingFile {
            reader: fs::File::open(temp.path()).unwrap(),
            writer: fs::OpenOptions::new()
                .append(true)
                .open(temp.path())
                .unwrap(),
            grew: false,
        };
        assert_eq!(initial_length, 1);
        let error = read_utf8_bounded(reader, 8).unwrap_err();
        assert!(error.to_string().contains("size"));
        assert_eq!(fs::metadata(temp.path()).unwrap().len(), 21);
    }
}
