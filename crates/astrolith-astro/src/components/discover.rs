//! Bounded recursive component discovery and catalog loading.

use std::{
    collections::HashSet,
    fs,
    path::{Component as PathComponent, Path, PathBuf},
};

use crate::discovery::{Cancellation, contained_path, read_bounded};

use super::{
    Component, ComponentLocation, Framework,
    props::analyze_component,
    starlight::{PackageDeclaration, package_declaration, starlight_catalog},
};

/// The default project-relative component directory.
pub const DEFAULT_COMPONENT_DIRECTORY: &str = "src/components";
/// Upper bound on indexed project component files.
const MAX_COMPONENT_FILES: usize = 2048;
/// Upper bound on component directory depth.
const MAX_COMPONENT_DEPTH: usize = 32;
/// Upper bound on visited component directories.
const MAX_COMPONENT_DIRECTORIES: usize = 1024;
/// Upper bound on entries read from one directory.
const MAX_DIRECTORY_ENTRIES: usize = 8192;
/// Upper bound on one component source file.
const MAX_COMPONENT_BYTES: usize = 256 * 1024;
/// Upper bound on package.json.
const MAX_PACKAGE_BYTES: usize = 1024 * 1024;

/// One bounded component index request, carrying the caller's generation.
#[derive(Clone, Debug)]
pub struct ComponentIndexRequest {
    root: PathBuf,
    components_directory: PathBuf,
    generation: u64,
    cancellation: Cancellation,
}

impl ComponentIndexRequest {
    /// A request for the conventional `src/components` directory.
    pub fn new(root: impl Into<PathBuf>, generation: u64, cancellation: Cancellation) -> Self {
        Self {
            root: root.into(),
            components_directory: PathBuf::from(DEFAULT_COMPONENT_DIRECTORY),
            generation,
            cancellation,
        }
    }

    /// Override the resolved, project-relative component directory.
    pub fn with_components_directory(mut self, directory: impl Into<PathBuf>) -> Self {
        self.components_directory = directory.into();
        self
    }

    /// The project generation this request belongs to.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The configured project-relative component directory.
    pub fn components_directory(&self) -> &Path {
        &self.components_directory
    }
}

/// A completed component index for one project generation.
#[derive(Clone, Debug, Default)]
pub struct ComponentIndex {
    generation: u64,
    components: Vec<Component>,
    diagnostics: Vec<String>,
    starlight: bool,
    auto_import: bool,
}

impl ComponentIndex {
    /// The generation this index was built for.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Project components first, then package components.
    pub fn components(&self) -> &[Component] {
        &self.components
    }

    /// Non-fatal discovery and parsing problems.
    pub fn diagnostics(&self) -> &[String] {
        &self.diagnostics
    }

    /// Whether the built-in Starlight catalog was included.
    pub fn starlight(&self) -> bool {
        self.starlight
    }

    /// Whether the project declares `astro-auto-import`.
    pub fn auto_import(&self) -> bool {
        self.auto_import
    }

    /// Fail closed when a completed result belongs to a superseded generation.
    pub fn accept_for_generation(self, generation: u64) -> Result<Self, ComponentIndexError> {
        if self.generation == generation {
            Ok(self)
        } else {
            Err(ComponentIndexError::StaleGeneration {
                result: self.generation,
                current: generation,
            })
        }
    }
}

/// A fatal component index failure.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentIndexError {
    Cancelled,
    InvalidRoot(String),
    StaleGeneration { result: u64, current: u64 },
}

impl std::fmt::Display for ComponentIndexError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("Component discovery was cancelled"),
            Self::InvalidRoot(message) => formatter.write_str(message),
            Self::StaleGeneration { result, current } => write!(
                formatter,
                "component index generation {result} was superseded by {current}"
            ),
        }
    }
}

impl std::error::Error for ComponentIndexError {}

/// Discover project components and load the Starlight catalog when declared.
pub fn inspect_components(
    request: &ComponentIndexRequest,
) -> Result<ComponentIndex, ComponentIndexError> {
    let check = || {
        request
            .cancellation
            .check()
            .map_err(|_| ComponentIndexError::Cancelled)
    };
    check()?;
    let root = request.root.canonicalize().map_err(|error| {
        ComponentIndexError::InvalidRoot(format!("Invalid project root: {error}"))
    })?;
    if !root.is_dir() {
        return Err(ComponentIndexError::InvalidRoot(
            "Choose a project directory".into(),
        ));
    }

    let mut diagnostics = Vec::new();
    let declaration = read_package_declaration(&root, &mut diagnostics);
    let mut components = Vec::new();
    let mut index = ComponentIndex {
        generation: request.generation,
        components: Vec::new(),
        diagnostics,
        starlight: declaration.starlight,
        auto_import: declaration.auto_import,
    };
    if declaration.auto_import {
        index.diagnostics.push(
            "Project declares astro-auto-import; components may already be available without imports."
                .into(),
        );
    }

    match resolve_component_directory(&root, &request.components_directory) {
        Ok(Some(directory)) => {
            walk_components(
                &root,
                &directory,
                &request.cancellation,
                &mut components,
                &mut index.diagnostics,
            )?;
        }
        Ok(None) => index.diagnostics.push(format!(
            "Component directory not found: {}",
            request.components_directory.display()
        )),
        Err(message) => return Err(ComponentIndexError::InvalidRoot(message)),
    }

    components.sort_by_key(Component::id);
    if declaration.starlight {
        match starlight_catalog() {
            Ok(catalog) => components.extend(catalog),
            Err(message) => index
                .diagnostics
                .push(format!("Starlight catalog unavailable: {message}")),
        }
    }

    index.components = components;
    Ok(index)
}

fn resolve_component_directory(root: &Path, relative: &Path) -> Result<Option<PathBuf>, String> {
    if relative.as_os_str().is_empty() {
        return Ok(None);
    }
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                PathComponent::ParentDir | PathComponent::Prefix(_)
            )
        })
    {
        return Err("Component directory must stay within project root".into());
    }
    match contained_path(root, relative) {
        Ok(path) if path.is_dir() => Ok(Some(path)),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Invalid component directory: {error}")),
    }
}

fn read_package_declaration(root: &Path, diagnostics: &mut Vec<String>) -> PackageDeclaration {
    let path = match contained_path(root, Path::new("package.json")) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PackageDeclaration::default();
        }
        Err(error) => {
            diagnostics.push(format!("Invalid package.json: {error}"));
            return PackageDeclaration::default();
        }
    };
    match read_bounded(&path, MAX_PACKAGE_BYTES) {
        Ok(source) => match package_declaration(&source) {
            Ok(declaration) => declaration,
            Err(message) => {
                diagnostics.push(message);
                PackageDeclaration::default()
            }
        },
        Err(error) => {
            diagnostics.push(format!("Invalid package.json: {error}"));
            PackageDeclaration::default()
        }
    }
}

fn walk_components(
    root: &Path,
    directory: &Path,
    cancellation: &Cancellation,
    components: &mut Vec<Component>,
    diagnostics: &mut Vec<String>,
) -> Result<(), ComponentIndexError> {
    let mut visited = HashSet::new();
    visited.insert(directory.to_path_buf());
    let mut visited_files = HashSet::new();
    let mut pending = vec![(directory.to_path_buf(), 0_usize)];
    let mut visited_directories = 0_usize;

    while let Some((directory, depth)) = pending.pop() {
        cancellation
            .check()
            .map_err(|_| ComponentIndexError::Cancelled)?;
        visited_directories += 1;
        if visited_directories > MAX_COMPONENT_DIRECTORIES {
            diagnostics.push(format!(
                "Stopped component indexing after {MAX_COMPONENT_DIRECTORIES} directories"
            ));
            return Ok(());
        }
        if depth > MAX_COMPONENT_DEPTH {
            diagnostics.push(format!(
                "Skipped component directory deeper than {MAX_COMPONENT_DEPTH}: {}",
                directory.display()
            ));
            continue;
        }
        let read = match fs::read_dir(&directory) {
            Ok(read) => read,
            Err(error) => {
                diagnostics.push(format!(
                    "Could not read component directory {}: {error}",
                    directory.display()
                ));
                continue;
            }
        };
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in read {
            if entries.len() >= MAX_DIRECTORY_ENTRIES {
                truncated = true;
                break;
            }
            cancellation
                .check()
                .map_err(|_| ComponentIndexError::Cancelled)?;
            match entry {
                Ok(entry) => entries.push(entry),
                Err(error) => diagnostics.push(format!(
                    "Could not read a component entry in {}: {error}",
                    directory.display()
                )),
            }
        }
        if truncated {
            diagnostics.push(format!(
                "Stopped reading {} after {MAX_DIRECTORY_ENTRIES} entries",
                directory.display()
            ));
        }
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            if components.len() >= MAX_COMPONENT_FILES {
                diagnostics.push(format!(
                    "Stopped component indexing after {MAX_COMPONENT_FILES} files"
                ));
                return Ok(());
            }
            let path = entry.path();
            if let Err(error) = fs::symlink_metadata(&path) {
                diagnostics.push(format!("Skipped {}: {error}", path.display()));
                continue;
            }
            let canonical = match path.canonicalize() {
                Ok(canonical) => canonical,
                Err(error) => {
                    diagnostics.push(format!("Skipped {}: {error}", path.display()));
                    continue;
                }
            };
            if !canonical.starts_with(root) {
                diagnostics.push(format!(
                    "Skipped component path escaping the project root: {}",
                    path.display()
                ));
                continue;
            }
            let file_type = match fs::metadata(&canonical) {
                Ok(metadata) => metadata,
                Err(error) => {
                    diagnostics.push(format!("Skipped {}: {error}", canonical.display()));
                    continue;
                }
            };
            if file_type.is_dir() {
                if visited.insert(canonical.clone()) {
                    pending.push((canonical, depth + 1));
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if !visited_files.insert(canonical.clone()) {
                continue;
            }
            let Some(framework) = Framework::from_path(&canonical) else {
                continue;
            };
            let Some(stem) = canonical.file_stem().and_then(|stem| stem.to_str()) else {
                diagnostics.push(format!(
                    "Skipped a component with a non-UTF-8 filename: {}",
                    canonical.display()
                ));
                continue;
            };
            if stem == "index" {
                continue;
            }
            let relative_path = canonical
                .strip_prefix(root)
                .unwrap_or(&canonical)
                .to_path_buf();
            let source = match read_bounded(&canonical, MAX_COMPONENT_BYTES) {
                Ok(source) => source,
                Err(error) => {
                    diagnostics.push(format!(
                        "Listed {} without props: {error}",
                        relative_path.display()
                    ));
                    components.push(Component {
                        name: stem.to_owned(),
                        framework,
                        location: ComponentLocation::Project {
                            source: canonical,
                            relative_path,
                        },
                        description: None,
                        props: Vec::new(),
                        slots: Vec::new(),
                        client_directives: framework.client_directives().to_vec(),
                    });
                    continue;
                }
            };
            let analysis = analyze_component(framework, &source, stem, &canonical, root);
            diagnostics.extend(
                analysis
                    .diagnostics
                    .into_iter()
                    .map(|message| format!("{}: {message}", relative_path.display())),
            );
            components.push(Component {
                name: stem.to_owned(),
                framework,
                location: ComponentLocation::Project {
                    source: canonical,
                    relative_path,
                },
                description: analysis.description,
                props: analysis.props,
                slots: analysis.slots,
                client_directives: framework.client_directives().to_vec(),
            });
        }
    }
    Ok(())
}
