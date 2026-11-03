//! Complete-source import planning for component insertion.
//!
//! Plans are computed against the exact complete source handed in, edit only
//! the ESM prelude, and never reformat unrelated imports. Callers must re-plan
//! (or reject a stale plan) at insertion time.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component as PathComponent, Path, PathBuf},
};

use tree_sitter::Parser;

use super::{Component, ComponentLocation};

/// Upper bound on complete MDX source accepted by the planner.
const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
/// Upper bound on the parsed ESM prelude.
const MAX_PRELUDE_BYTES: usize = 256 * 1024;

/// An import observed in another collection entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorroboratedImport {
    /// Local binding name.
    pub name: String,
    /// Module specifier as written in that document.
    pub specifier: String,
    /// Canonical source of the document that contains it.
    pub document: PathBuf,
}

/// A prop value that changes imports.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlannedValue {
    /// An image path from the asset picker.
    Image { prop: String, path: String },
    /// A collection entry selected for a `CollectionEntry` prop.
    Entry {
        prop: String,
        collection: String,
        id: String,
    },
}

/// One import planning request over complete source.
#[derive(Clone, Debug)]
pub struct ImportPlanRequest<'a> {
    pub source: &'a str,
    pub root: &'a Path,
    pub document: &'a Path,
    pub component: &'a Component,
    pub values: &'a [PlannedValue],
    /// Per-project opt-in; off leaves the source untouched.
    pub enabled: bool,
    pub current_collection_imports: &'a [CorroboratedImport],
    pub other_collection_imports: &'a [CorroboratedImport],
}

/// A successful complete-source plan.
#[derive(Clone, Debug)]
pub struct ImportPlan {
    source_hash: u64,
    source: String,
    identifiers: BTreeMap<String, String>,
    warnings: Vec<String>,
}

impl ImportPlan {
    /// Complete source after the planned import edits.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Prop name to identifier/constant expression resolved by this plan.
    pub fn identifiers(&self) -> &BTreeMap<String, String> {
        &self.identifiers
    }

    /// Non-fatal planning notes.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Whether the complete source changed since the plan was built.
    pub fn is_stale(&self, current: &str) -> bool {
        self.source_hash != hash_source(current)
    }

    /// Fail closed for a stale plan before any source is written.
    pub fn revalidate(&self, current: &str) -> Result<(), ImportError> {
        if self.is_stale(current) {
            Err(ImportError::StaleSource)
        } else {
            Ok(())
        }
    }
}

/// A fatal import planning failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportError {
    SourceTooLarge,
    Unparseable(String),
    StaleSource,
    NameOccupied(String),
    OutsideProject(String),
    ImageMissing(String),
    LoaderCollision,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceTooLarge => formatter.write_str("MDX source is too large to plan imports"),
            Self::Unparseable(message) => formatter.write_str(message),
            Self::StaleSource => {
                formatter.write_str("The document changed; the import plan is stale")
            }
            Self::NameOccupied(name) => write!(
                formatter,
                "The name `{name}` is already bound to a different value"
            ),
            Self::OutsideProject(path) => {
                write!(formatter, "`{path}` is outside the project")
            }
            Self::ImageMissing(path) => write!(formatter, "Image `{path}` was not found"),
            Self::LoaderCollision => {
                formatter.write_str("`getEntry` is already bound to a different value")
            }
        }
    }
}

impl std::error::Error for ImportError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImportedName {
    Default,
    Named,
    Namespace,
}

#[derive(Clone, Debug)]
struct ImportBinding {
    module: String,
    kind: ImportedName,
    imported: Option<String>,
    type_only: bool,
}

#[derive(Clone, Debug)]
struct Binding {
    import: Option<ImportBinding>,
}

#[derive(Clone, Debug)]
struct ImportStatement {
    end: usize,
    module: String,
    quote: char,
    semicolon: bool,
    type_only: bool,
    default_binding: Option<String>,
    named: Option<NamedImports>,
    namespace_binding: Option<String>,
}

#[derive(Clone, Debug)]
struct NamedImports {
    start: usize,
    end: usize,
    names: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
struct ImportStyle {
    quote: char,
    semicolon: bool,
    multiline: bool,
    indent: String,
}

#[derive(Clone, Debug)]
struct SourceModel {
    bindings: BTreeMap<String, Binding>,
    declarations: BTreeSet<String>,
    imports: Vec<ImportStatement>,
    style: ImportStyle,
    insert_at: usize,
    eol: &'static str,
    separate_block: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PlannedImportKind {
    Default,
    Named,
}

#[derive(Clone, Debug)]
struct PlannedImport {
    module: String,
    kind: PlannedImportKind,
    binding: String,
}

/// Plan complete-source import edits for one component insertion.
pub fn plan_imports(request: &ImportPlanRequest<'_>) -> Result<ImportPlan, ImportError> {
    let source_hash = hash_source(request.source);
    if !request.enabled {
        return Ok(ImportPlan {
            source_hash,
            source: request.source.to_owned(),
            identifiers: BTreeMap::new(),
            warnings: Vec::new(),
        });
    }
    if request.source.len() > MAX_SOURCE_BYTES {
        return Err(ImportError::SourceTooLarge);
    }
    let root = request
        .root
        .canonicalize()
        .map_err(|_| ImportError::OutsideProject(request.root.display().to_string()))?;
    let document = request
        .document
        .canonicalize()
        .unwrap_or_else(|_| request.document.to_path_buf());
    if !document.starts_with(&root) {
        return Err(ImportError::OutsideProject(document.display().to_string()));
    }

    let mut warnings = Vec::new();
    if !is_importable_name(&request.component.name) {
        warnings.push(format!(
            "`{}` does not start with an uppercase letter; no import is added",
            request.component.name
        ));
        return Ok(ImportPlan {
            source_hash,
            source: request.source.to_owned(),
            identifiers: BTreeMap::new(),
            warnings,
        });
    }

    let model = parse_source(request.source)?;
    let mut used = model.bindings.keys().cloned().collect::<BTreeSet<_>>();
    used.extend(model.declarations.iter().cloned());
    let mut planned = Vec::new();
    let mut identifiers = BTreeMap::new();
    let mut image_bindings = BTreeMap::new();
    let mut loader = "getEntry".to_owned();

    plan_component_import(
        request,
        &document,
        &model,
        &mut used,
        &mut planned,
        &mut warnings,
    )?;

    for value in request.values {
        match value {
            PlannedValue::Image { prop, path } => {
                plan_image(
                    prop,
                    path,
                    &document,
                    &root,
                    &model,
                    &mut used,
                    &mut planned,
                    &mut identifiers,
                    &mut image_bindings,
                )?;
            }
            PlannedValue::Entry { prop, .. } => {
                plan_entry(
                    prop,
                    &model,
                    &mut used,
                    &mut planned,
                    &mut identifiers,
                    &mut loader,
                )?;
            }
        }
    }

    let mut edits = Vec::new();
    let mut new_imports: Vec<PlannedImport> = Vec::new();
    let mut merge_names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen_imports = BTreeSet::new();
    for import in planned {
        let key = (
            import.module.clone(),
            import.kind == PlannedImportKind::Named,
            import.binding.clone(),
        );
        if !seen_imports.insert(key) {
            continue;
        }
        if import.kind == PlannedImportKind::Named {
            if model.imports.iter().any(|statement| {
                statement.module == import.module
                    && statement.named.is_some()
                    && !statement.type_only
            }) {
                merge_names
                    .entry(import.module.clone())
                    .or_default()
                    .push(import.binding);
                continue;
            }
        } else if model.imports.iter().any(|statement| {
            statement.module == import.module
                && statement.default_binding.as_deref() == Some(import.binding.as_str())
        }) {
            continue;
        }
        new_imports.push(import);
    }
    for (module, names) in merge_names {
        let Some(statement) = model.imports.iter().find(|statement| {
            statement.module == module && statement.named.is_some() && !statement.type_only
        }) else {
            continue;
        };
        let named = statement.named.as_ref().expect("checked above");
        if let Some((offset, text)) = merge_named_import(request.source, named, &names, model.eol) {
            edits.push((offset, text));
        }
    }

    // Named additions for the same module share one new statement.
    let mut grouped: Vec<PlannedImport> = Vec::new();
    for import in new_imports {
        if let Some(existing) = grouped.iter_mut().find(|existing| {
            import.kind == PlannedImportKind::Named
                && existing.kind == PlannedImportKind::Named
                && existing.module == import.module
        }) {
            if !existing
                .binding
                .split(", ")
                .any(|name| name == import.binding)
            {
                existing.binding.push_str(", ");
                existing.binding.push_str(&import.binding);
            }
            continue;
        }
        grouped.push(import);
    }
    let mut insertion = String::new();
    for import in &grouped {
        let quote = model.style.quote;
        let semicolon = if model.style.semicolon { ";" } else { "" };
        match import.kind {
            PlannedImportKind::Default => {
                insertion.push_str(&format!(
                    "import {} from {quote}{}{quote}{semicolon}{}",
                    import.binding, import.module, model.eol
                ));
            }
            PlannedImportKind::Named => {
                if model.style.multiline {
                    insertion.push_str(&format!("import {{{}", model.eol));
                    for (index, name) in import.binding.split(", ").enumerate() {
                        if index > 0 {
                            insertion.push_str(model.eol);
                        }
                        insertion.push_str(&model.style.indent);
                        insertion.push_str(name);
                        insertion.push(',');
                    }
                    insertion.push_str(&format!(
                        "{}}} from {quote}{}{quote}{semicolon}{}",
                        model.eol, import.module, model.eol
                    ));
                } else {
                    insertion.push_str(&format!(
                        "import {{ {} }} from {quote}{}{quote}{semicolon}{}",
                        import.binding, import.module, model.eol
                    ));
                }
            }
        }
    }

    let consts = entry_constants(request.values, &identifiers, &model, &loader);
    insertion.push_str(&consts);
    if model.separate_block && !insertion.is_empty() {
        insertion.push_str(model.eol);
    }

    if !insertion.is_empty() {
        let prefix = if model.insert_at > 0 && !request.source[..model.insert_at].ends_with('\n') {
            model.eol
        } else {
            ""
        };
        let text = format!("{prefix}{insertion}");
        edits.push((model.insert_at, text));
    }

    edits.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    let mut planned_source = request.source.to_owned();
    for (offset, text) in edits {
        planned_source.insert_str(offset, &text);
    }
    Ok(ImportPlan {
        source_hash,
        source: planned_source,
        identifiers,
        warnings,
    })
}

fn plan_component_import(
    request: &ImportPlanRequest<'_>,
    document: &Path,
    model: &SourceModel,
    used: &mut BTreeSet<String>,
    planned: &mut Vec<PlannedImport>,
    warnings: &mut Vec<String>,
) -> Result<(), ImportError> {
    let component = request.component;
    let name = component.name.clone();
    if let Some(binding) = model.bindings.get(&name) {
        if binding_matches_component(binding, component, document) {
            return Ok(());
        }
        return Err(ImportError::NameOccupied(name));
    }
    if model.declarations.contains(&name) {
        return Err(ImportError::NameOccupied(name));
    }
    used.insert(name.clone());
    match &component.location {
        ComponentLocation::Package { specifier } => {
            // Importing the same export under another local name leaves the
            // component name unbound, so a second import cannot be planned.
            if model.imports.iter().any(|statement| {
                statement.module == *specifier
                    && statement.named.as_ref().is_some_and(|named| {
                        named
                            .names
                            .iter()
                            .any(|(imported, local)| imported == &name && local != &name)
                    })
            }) {
                return Err(ImportError::NameOccupied(name));
            }
            planned.push(PlannedImport {
                module: specifier.clone(),
                kind: PlannedImportKind::Named,
                binding: name,
            });
        }
        ComponentLocation::Project { source, .. } => {
            let specifier = reused_specifier(
                component,
                document,
                request.current_collection_imports,
                request.other_collection_imports,
            )
            .or_else(|| {
                relative_specifier(document, source)
                    .map_err(|message| {
                        warnings.push(message);
                    })
                    .ok()
            })
            .ok_or_else(|| ImportError::OutsideProject(source.display().to_string()))?;
            planned.push(PlannedImport {
                module: specifier,
                kind: PlannedImportKind::Default,
                binding: name,
            });
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn plan_image(
    prop: &str,
    raw: &str,
    document: &Path,
    root: &Path,
    model: &SourceModel,
    used: &mut BTreeSet<String>,
    planned: &mut Vec<PlannedImport>,
    identifiers: &mut BTreeMap<String, String>,
    images: &mut BTreeMap<PathBuf, String>,
) -> Result<(), ImportError> {
    let resolution = resolve_image(document, root, raw)?;
    let ResolvedImage::Project { source: image } = resolution else {
        return Ok(());
    };
    let specifier = relative_specifier(document, &image).map_err(ImportError::OutsideProject)?;
    // Reuse an existing default import for the same module.
    if let Some(binding) = model
        .imports
        .iter()
        .find(|statement| statement.module == specifier)
        .and_then(|statement| statement.default_binding.as_ref())
    {
        identifiers.insert(prop.to_owned(), binding.clone());
        images.insert(image, binding.clone());
        return Ok(());
    }
    // Two spellings of the same image share one identifier.
    if let Some(existing) = images.get(&image) {
        identifiers.insert(prop.to_owned(), existing.clone());
        return Ok(());
    }
    let base = image_identifier(&image);
    let identifier = unique_identifier(&base, used);
    used.insert(identifier.clone());
    planned.push(PlannedImport {
        module: specifier,
        kind: PlannedImportKind::Default,
        binding: identifier.clone(),
    });
    images.insert(image, identifier.clone());
    identifiers.insert(prop.to_owned(), identifier);
    Ok(())
}

fn plan_entry(
    prop: &str,
    model: &SourceModel,
    used: &mut BTreeSet<String>,
    planned: &mut Vec<PlannedImport>,
    identifiers: &mut BTreeMap<String, String>,
    loader: &mut String,
) -> Result<(), ImportError> {
    match model.bindings.get(loader.as_str()) {
        Some(Binding {
            import: Some(binding),
            ..
        }) if !binding.type_only
            && binding.module == "astro:content"
            && binding.kind == ImportedName::Named
            && binding.imported.as_deref() == Some("getEntry") => {}
        Some(_) => return Err(ImportError::LoaderCollision),
        None => {
            // `getEntry` imported under an alias is still the same loader.
            match model.bindings.iter().find_map(|(local, binding)| {
                let import = binding.import.as_ref()?;
                (!import.type_only
                    && import.module == "astro:content"
                    && import.kind == ImportedName::Named
                    && import.imported.as_deref() == Some("getEntry"))
                .then(|| local.clone())
            }) {
                Some(local) => {
                    loader.clear();
                    loader.push_str(&local);
                }
                None => {
                    if model.declarations.contains(loader.as_str()) {
                        return Err(ImportError::LoaderCollision);
                    }
                    planned.push(PlannedImport {
                        module: "astro:content".into(),
                        kind: PlannedImportKind::Named,
                        binding: loader.clone(),
                    });
                    used.insert(loader.clone());
                }
            }
        }
    }
    let base = camel_identifier(prop);
    let base = if base.is_empty() {
        "entry".to_owned()
    } else {
        format!("{base}Entry")
    };
    let constant = unique_identifier(&base, used);
    used.insert(constant.clone());
    identifiers.insert(prop.to_owned(), constant);
    Ok(())
}

fn entry_constants(
    values: &[PlannedValue],
    identifiers: &BTreeMap<String, String>,
    model: &SourceModel,
    loader: &str,
) -> String {
    let mut constants = String::new();
    for value in values {
        let PlannedValue::Entry {
            prop,
            collection,
            id,
        } = value
        else {
            continue;
        };
        let Some(constant) = identifiers.get(prop) else {
            continue;
        };
        let quote = model.style.quote;
        let semicolon = if model.style.semicolon { ";" } else { "" };
        constants.push_str(&format!(
            "export const {constant} = await {loader}({quote}{}{quote}, {quote}{}{quote}){semicolon}{}",
            escape_js(collection, quote),
            escape_js(id, quote),
            model.eol
        ));
    }
    constants
}

enum ResolvedImage {
    Project { source: PathBuf },
    Public,
    External,
}

fn resolve_image(document: &Path, root: &Path, raw: &str) -> Result<ResolvedImage, ImportError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ImportError::ImageMissing(raw.to_owned()));
    }
    if raw.starts_with("http://")
        || raw.starts_with("https://")
        || raw.starts_with("//")
        || raw.starts_with("data:")
    {
        return Ok(ResolvedImage::External);
    }
    let public_root = root.join("public").canonicalize().ok();
    let candidate = if let Some(relative) = raw.strip_prefix('/') {
        // Absolute-mode frontmatter stores a project asset as `/src/...`;
        // a page references a public file as `/banner.png`. Resolve inside the
        // project first, then the public directory, then a real host path.
        if relative.is_empty() {
            return Err(ImportError::ImageMissing(raw.to_owned()));
        }
        normalize(&root.join(relative))
            .canonicalize()
            .or_else(|_| normalize(&root.join("public").join(relative)).canonicalize())
            .or_else(|_| Path::new(raw).canonicalize())
            .map_err(|_| ImportError::ImageMissing(raw.to_owned()))?
    } else {
        let from_document = document
            .parent()
            .map(|parent| normalize(&parent.join(raw)))
            .filter(|candidate| candidate.is_file());
        let candidate = match from_document {
            Some(candidate) => candidate,
            None => normalize(&root.join(raw)),
        };
        candidate
            .canonicalize()
            .map_err(|_| ImportError::ImageMissing(raw.to_owned()))?
    };
    if !candidate.starts_with(root) {
        return Err(ImportError::OutsideProject(raw.to_owned()));
    }
    if public_root
        .as_ref()
        .is_some_and(|public| candidate.starts_with(public))
    {
        return Ok(ResolvedImage::Public);
    }
    Ok(ResolvedImage::Project { source: candidate })
}

fn binding_matches_component(binding: &Binding, component: &Component, document: &Path) -> bool {
    let Some(import) = &binding.import else {
        return false;
    };
    if import.kind == ImportedName::Namespace || import.type_only {
        return false;
    }
    match &component.location {
        ComponentLocation::Project { source, .. } => {
            if import.kind != ImportedName::Default {
                return false;
            }
            if import.module.starts_with('.') {
                resolve_module(document, &import.module).is_some_and(|resolved| resolved == *source)
            } else {
                alias_targets_component(&import.module, component)
            }
        }
        ComponentLocation::Package { specifier } => {
            import.module == *specifier
                && import.kind == ImportedName::Named
                && import.imported.as_deref() == Some(component.name.as_str())
        }
    }
}

/// Whether a non-relative specifier clearly names the component's file.
fn alias_targets_component(specifier: &str, component: &Component) -> bool {
    let ComponentLocation::Project { relative_path, .. } = &component.location else {
        return false;
    };
    let specifier = specifier.split(['?', '#']).next().unwrap_or(specifier);
    let segments = specifier
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let relative = relative_path
        .components()
        .filter_map(|component| match component {
            PathComponent::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let matches_tail = |segments: &[&str]| {
        !segments.is_empty()
            && segments.len() <= relative.len()
            && relative[relative.len() - segments.len()..]
                .iter()
                .zip(segments)
                .all(|(relative, specifier)| relative.as_str() == *specifier)
    };
    // Either the specifier spells the project-relative path, or an alias-style
    // prefix replaces the leading project directories.
    matches_tail(&segments)
        || (segments.len() >= 2
            && matches!(segments[0].chars().next(), Some('@' | '~' | '#' | '$'))
            && matches_tail(&segments[1..]))
}

fn reused_specifier(
    component: &Component,
    document: &Path,
    current: &[CorroboratedImport],
    other: &[CorroboratedImport],
) -> Option<String> {
    let ComponentLocation::Project { source, .. } = &component.location else {
        return None;
    };
    for candidate in current.iter().chain(other) {
        if candidate.name != component.name {
            continue;
        }
        if candidate.specifier.starts_with('.') {
            if resolve_module(document, &candidate.specifier)
                .is_some_and(|resolved| resolved == *source)
            {
                return Some(candidate.specifier.clone());
            }
            continue;
        }
        // A non-relative specifier is only reusable when it clearly names
        // this component's file.
        if alias_targets_component(&candidate.specifier, component) {
            return Some(candidate.specifier.clone());
        }
    }
    None
}

fn parse_source(source: &str) -> Result<SourceModel, ImportError> {
    let body_start = body_start(source)?;
    let end = source.len().min(body_start + MAX_PRELUDE_BYTES);
    let eol = if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TSX.into())
        .map_err(|error| {
            ImportError::Unparseable(format!("Could not load the MDX parser: {error}"))
        })?;
    let tree = parser
        .parse(&source[body_start..end], None)
        .ok_or_else(|| ImportError::Unparseable("Could not parse the MDX import prelude".into()))?;
    let mut model = SourceModel {
        bindings: BTreeMap::new(),
        declarations: BTreeSet::new(),
        imports: Vec::new(),
        style: ImportStyle {
            quote: '"',
            semicolon: true,
            multiline: false,
            indent: "  ".into(),
        },
        insert_at: body_start,
        eol,
        separate_block: false,
    };
    let mut cursor = tree.root_node().walk();
    let mut first_import = true;
    let mut last_import_end = None;
    for child in tree.root_node().named_children(&mut cursor) {
        let kind = child.kind();
        if kind == "comment" {
            continue;
        }
        if !matches!(kind, "import_statement" | "export_statement") {
            break;
        }
        if child.has_error() {
            return Err(ImportError::Unparseable(
                "The MDX import prelude is malformed".into(),
            ));
        }
        if kind == "import_statement" {
            let statement = parse_import(child, body_start, source)?;
            if first_import {
                model.style.quote = statement.quote;
                model.style.semicolon = statement.semicolon;
                if let Some(named) = &statement.named
                    && source[named.start + 1..named.end - 1].contains('\n')
                {
                    model.style.multiline = true;
                    if let Some((imported, _)) = named.names.first()
                        && let Some(offset) = source[named.start..named.end].find(imported.as_str())
                    {
                        let position = named.start + offset;
                        let line_start = source[..position]
                            .rfind('\n')
                            .map_or(named.start, |index| index + 1);
                        model.style.indent = source[line_start..position].to_owned();
                    }
                }
                first_import = false;
            }
            collect_import_bindings(&statement, &mut model.bindings)?;
            last_import_end = Some(statement.end);
            model.imports.push(statement);
        } else {
            collect_declarations(child, body_start, source, &mut model.declarations);
        }
    }
    if let Some(end) = last_import_end {
        let line_end = source[end..]
            .find('\n')
            .map(|offset| end + offset + 1)
            .unwrap_or(source.len());
        let rest = source[end..line_end].trim();
        // A trailing comment belongs to the import it follows.
        model.insert_at = if rest.is_empty() || rest.starts_with("//") || rest.starts_with("/*") {
            line_end
        } else {
            end
        };
    } else {
        let mut insert = body_start;
        while insert < source.len()
            && matches!(source.as_bytes()[insert], b' ' | b'\t' | b'\r' | b'\n')
        {
            insert += 1;
        }
        model.insert_at = insert;
        model.separate_block = insert < source.len();
    }
    Ok(model)
}

fn body_start(source: &str) -> Result<usize, ImportError> {
    let start = usize::from(source.starts_with('\u{feff}')) * 3;
    let Some(first_end) = source[start..].find('\n').map(|offset| start + offset) else {
        return Ok(0);
    };
    let first = source[start..first_end].trim_end_matches('\r').trim_end();
    if first != "---" {
        return Ok(0);
    }
    let mut cursor = first_end + 1;
    while cursor <= source.len() {
        let line_end = source[cursor..]
            .find('\n')
            .map(|offset| cursor + offset)
            .unwrap_or(source.len());
        if source[cursor..line_end].trim_end_matches('\r').trim_end() == "---" {
            return Ok((line_end + 1).min(source.len()));
        }
        if line_end == source.len() {
            break;
        }
        cursor = line_end + 1;
    }
    Err(ImportError::Unparseable(
        "The document has unfinished frontmatter".into(),
    ))
}

fn parse_import(
    node: tree_sitter::Node<'_>,
    offset: usize,
    source: &str,
) -> Result<ImportStatement, ImportError> {
    let start = offset + node.start_byte();
    let end = offset + node.end_byte();
    let module_node = node.child_by_field_name("source").ok_or_else(|| {
        ImportError::Unparseable("Side-effect-only imports are unsupported".into())
    })?;
    let module_text = &source[offset + module_node.start_byte()..offset + module_node.end_byte()];
    let quote = module_text.chars().next().unwrap_or('"');
    let module = module_text.trim_matches(['"', '\'']).to_owned();
    let text = source[start..end].trim_end();
    let semicolon = text.ends_with(';');
    let mut type_only = false;
    let mut child_cursor = node.walk();
    for child in node.children(&mut child_cursor) {
        if !child.is_named() && child.kind() == "type" {
            type_only = true;
            break;
        }
    }
    let mut default_binding = None;
    let mut named = None;
    let mut namespace_binding = None;
    let mut clause_cursor = node.walk();
    for clause in node.named_children(&mut clause_cursor) {
        if clause.kind() != "import_clause" {
            continue;
        }
        let mut item_cursor = clause.walk();
        for item in clause.named_children(&mut item_cursor) {
            match item.kind() {
                "identifier" => {
                    default_binding = Some(
                        source[offset + item.start_byte()..offset + item.end_byte()].to_owned(),
                    );
                }
                "namespace_import" => {
                    namespace_binding = item.named_child(0).map(|identifier| {
                        source[offset + identifier.start_byte()..offset + identifier.end_byte()]
                            .to_owned()
                    });
                }
                "named_imports" => {
                    let mut names = Vec::new();
                    let mut specifier_cursor = item.walk();
                    for specifier in item.named_children(&mut specifier_cursor) {
                        if specifier.kind() != "import_specifier" {
                            continue;
                        }
                        let Some(name_node) = specifier.child_by_field_name("name") else {
                            continue;
                        };
                        let imported = source
                            [offset + name_node.start_byte()..offset + name_node.end_byte()]
                            .trim_matches(['"', '\''])
                            .to_owned();
                        let local = specifier
                            .child_by_field_name("alias")
                            .map(|alias| {
                                source[offset + alias.start_byte()..offset + alias.end_byte()]
                                    .to_owned()
                            })
                            .unwrap_or_else(|| imported.clone());
                        names.push((imported, local));
                    }
                    named = Some(NamedImports {
                        start: offset + item.start_byte(),
                        end: offset + item.end_byte(),
                        names,
                    });
                }
                _ => {}
            }
        }
    }
    Ok(ImportStatement {
        end,
        module,
        quote,
        semicolon,
        type_only,
        default_binding,
        named,
        namespace_binding,
    })
}

fn collect_import_bindings(
    statement: &ImportStatement,
    bindings: &mut BTreeMap<String, Binding>,
) -> Result<(), ImportError> {
    if let Some(binding) = &statement.default_binding {
        bindings.insert(
            binding.clone(),
            Binding {
                import: Some(ImportBinding {
                    module: statement.module.clone(),
                    kind: ImportedName::Default,
                    imported: None,
                    type_only: statement.type_only,
                }),
            },
        );
    }
    if let Some(binding) = &statement.namespace_binding {
        bindings.insert(
            binding.clone(),
            Binding {
                import: Some(ImportBinding {
                    module: statement.module.clone(),
                    kind: ImportedName::Namespace,
                    imported: None,
                    type_only: statement.type_only,
                }),
            },
        );
    }
    if let Some(named) = &statement.named {
        for (imported, local) in &named.names {
            bindings.insert(
                local.clone(),
                Binding {
                    import: Some(ImportBinding {
                        module: statement.module.clone(),
                        kind: ImportedName::Named,
                        imported: Some(imported.clone()),
                        type_only: statement.type_only,
                    }),
                },
            );
        }
    }
    Ok(())
}

fn collect_declarations(
    statement: tree_sitter::Node<'_>,
    offset: usize,
    source: &str,
    declarations: &mut BTreeSet<String>,
) {
    let Some(declaration) = statement.child_by_field_name("declaration").or_else(|| {
        let mut cursor = statement.walk();
        statement
            .named_children(&mut cursor)
            .into_iter()
            .find(|child| child.kind() != "export_clause")
    }) else {
        return;
    };
    if declaration.kind() == "lexical_declaration" || declaration.kind() == "variable_declaration" {
        let mut cursor = declaration.walk();
        for declarator in declaration.named_children(&mut cursor) {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            if let Some(name) = declarator.child_by_field_name("name")
                && name.kind() == "identifier"
            {
                declarations.insert(
                    source[offset + name.start_byte()..offset + name.end_byte()].to_owned(),
                );
            }
        }
        return;
    }
    if let Some(name) = declaration.child_by_field_name("name")
        && matches!(name.kind(), "identifier" | "type_identifier")
    {
        declarations
            .insert(source[offset + name.start_byte()..offset + name.end_byte()].to_owned());
    }
}

fn merge_named_import(
    source: &str,
    named: &NamedImports,
    additions: &[String],
    eol: &str,
) -> Option<(usize, String)> {
    let additions = additions
        .iter()
        .filter(|addition| !named.names.iter().any(|(_, local)| local == *addition))
        .cloned()
        .collect::<Vec<_>>();
    if additions.is_empty() {
        return None;
    }
    let inner_start = named.start + 1;
    let inner_end = named.end.saturating_sub(1);
    let inner = &source[inner_start..inner_end];
    if inner.trim().is_empty() {
        return Some((inner_start, format!(" {} ", additions.join(", "))));
    }
    if inner.contains('\n') {
        let first_name = named.names.first()?;
        let name_start = source[named.start..named.end].find(&first_name.0)? + named.start;
        let line_start = source[..name_start]
            .rfind('\n')
            .map_or(named.start, |index| index + 1);
        let indent = &source[line_start..name_start];
        let indent = if indent.trim().is_empty() {
            indent
        } else {
            "  "
        };
        let trailing_comma = inner.trim_end().ends_with(',');
        if trailing_comma {
            let close_start = source[..inner_end]
                .rfind('\n')
                .map_or(inner_end, |index| index + 1);
            let mut text = String::new();
            for addition in &additions {
                text.push_str(indent);
                text.push_str(addition);
                text.push(',');
                text.push_str(eol);
            }
            Some((close_start, text))
        } else {
            let last = named.names.last()?.1.clone();
            let last_start = source[named.start..named.end].rfind(&last)? + named.start;
            let offset = last_start + last.len();
            let mut text = String::new();
            for addition in &additions {
                text.push(',');
                text.push_str(eol);
                text.push_str(indent);
                text.push_str(addition);
            }
            Some((offset, text))
        }
    } else {
        let trimmed_end = inner_start + inner.trim_end().len();
        let text = if inner.trim_end().ends_with(',') {
            format!(" {},", additions.join(", "))
        } else {
            format!(", {}", additions.join(", "))
        };
        Some((trimmed_end, text))
    }
}

fn resolve_module(document: &Path, specifier: &str) -> Option<PathBuf> {
    if !specifier.starts_with('.') {
        return None;
    }
    let base = document.parent()?.join(specifier);
    let mut candidates = vec![base.clone()];
    if base.extension().is_none() {
        for extension in ["ts", "tsx", "js", "jsx", "vue", "svelte", "astro"] {
            candidates.push(base.with_extension(extension));
        }
    }
    for candidate in candidates {
        if let Ok(canonical) = candidate.canonicalize() {
            return Some(canonical);
        }
    }
    Some(normalize(&base))
}

fn relative_specifier(document: &Path, target: &Path) -> Result<String, String> {
    let document = document
        .canonicalize()
        .unwrap_or_else(|_| document.to_path_buf());
    let target = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let from = document
        .parent()
        .ok_or_else(|| "The document has no parent directory".to_owned())?;
    let from = normalize(from);
    let target = normalize(&target);
    let from_parts = absolute_parts(&from);
    let target_parts = absolute_parts(&target);
    let mut common = 0;
    while common < from_parts.len()
        && common < target_parts.len()
        && from_parts[common] == target_parts[common]
    {
        common += 1;
    }
    if common == 0 {
        return Err("The target is outside the document root".into());
    }
    let mut parts = Vec::new();
    for _ in common..from_parts.len() {
        parts.push("..".to_owned());
    }
    for component in &target_parts[common..] {
        parts.push(component.clone());
    }
    let joined = parts.join("/");
    if joined.starts_with('.') {
        Ok(joined)
    } else {
        Ok(format!("./{joined}"))
    }
}

fn absolute_parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            PathComponent::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<PathComponent<'_>> = Vec::new();
    for component in path.components() {
        match component {
            PathComponent::CurDir => {}
            PathComponent::ParentDir => {
                if matches!(parts.last(), Some(PathComponent::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            other => parts.push(other),
        }
    }
    let mut normalized = PathBuf::new();
    for component in parts {
        normalized.push(component.as_os_str());
    }
    normalized
}

fn is_importable_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
}

fn camel_identifier(raw: &str) -> String {
    let mut words = Vec::new();
    let mut current = String::new();
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() {
            current.push(character);
        } else if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    let Some(first) = words.first() else {
        return String::new();
    };
    let mut identifier = first.to_ascii_lowercase();
    for word in &words[1..] {
        let mut characters = word.chars();
        if let Some(first) = characters.next() {
            identifier.push(first.to_ascii_uppercase());
            identifier.extend(characters.map(|character| character.to_ascii_lowercase()));
        }
    }
    if identifier.starts_with(|character: char| character.is_ascii_digit()) {
        identifier.insert_str(0, "image");
    }
    if is_reserved(&identifier) {
        identifier.push_str("Value");
    }
    identifier
}

fn image_identifier(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("image");
    let identifier = camel_identifier(stem);
    if identifier.is_empty() {
        "image".into()
    } else {
        identifier
    }
}

fn unique_identifier(base: &str, used: &BTreeSet<String>) -> String {
    if !used.contains(base) {
        return base.to_owned();
    }
    for suffix in 2..=10_000 {
        let candidate = format!("{base}{suffix}");
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    let mut suffix = 1;
    loop {
        let candidate = if suffix == 1 {
            format!("{base}Extra")
        } else {
            format!("{base}Extra{suffix}")
        };
        if !used.contains(&candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    )
}

fn escape_js(text: &str, quote: char) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '"' if quote == '"' => escaped.push_str("\\\""),
            '\'' if quote == '\'' => escaped.push_str("\\'"),
            other => escaped.push(other),
        }
    }
    escaped
}

fn hash_source(source: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in source.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
