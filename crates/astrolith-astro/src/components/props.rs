//! Bounded static prop extraction for Astro, React, Vue and Svelte files.
//!
//! The analysis reads the framework's script/frontmatter block with the pinned
//! Tree-sitter TypeScript grammar, follows relative type imports inside the
//! project, and never imports or evaluates the analysed module. A malformed
//! block still yields a component with no props.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
};

use tree_sitter::{Node, Parser, Tree};

use crate::discovery::read_bounded;

use super::{Framework, Prop, PropDefault, PropKind, Slot};

/// Upper bound on AST nodes inspected in one component or type file.
const MAX_AST_NODES: usize = 100_000;
/// Upper bound on AST depth.
const MAX_AST_DEPTH: usize = 128;
/// Upper bound on a parsed component script/frontmatter block.
const MAX_SCRIPT_BYTES: usize = 256 * 1024;
/// Upper bound on one relative type import file.
const MAX_TYPE_FILE_BYTES: usize = 256 * 1024;
/// Upper bound on relative type files followed for one component.
const MAX_TYPE_IMPORTS: usize = 8;

#[derive(Clone, Debug, Default)]
pub(crate) struct Analysis {
    pub(crate) description: Option<String>,
    pub(crate) props: Vec<Prop>,
    pub(crate) slots: Vec<Slot>,
    pub(crate) diagnostics: Vec<String>,
}

#[derive(Clone, Copy)]
struct ScriptRegion<'a> {
    text: &'a str,
    offset: usize,
}

#[derive(Clone, Debug, Default)]
struct TypeCatalog {
    records: BTreeMap<String, TypeRecord>,
}

#[derive(Clone, Debug)]
enum TypeRecord {
    Object {
        members: Vec<TypeMember>,
        bases: Vec<String>,
    },
    Enum(Vec<String>),
    Alias(String),
}

#[derive(Clone, Debug)]
struct TypeMember {
    name: String,
    type_text: String,
    optional: bool,
    doc: Option<String>,
}

#[derive(Clone, Debug)]
struct ClassifiedType {
    kind: PropKind,
    nullable: bool,
}

/// Analyse one component file. This never fails: a parse error is a diagnostic.
pub(crate) fn analyze_component(
    framework: Framework,
    source: &str,
    name: &str,
    path: &Path,
    project_root: &Path,
) -> Analysis {
    let mut analysis = Analysis::default();
    let (script, markup): (Option<ScriptRegion<'_>>, &str) = match framework {
        Framework::Astro => {
            let frontmatter = astro_frontmatter(source);
            let markup = frontmatter
                .map(|region| {
                    &source[astro_markup_start(source, region.offset + region.text.len())..]
                })
                .unwrap_or(source);
            (frontmatter, markup)
        }
        Framework::React => (
            Some(ScriptRegion {
                text: source,
                offset: 0,
            }),
            source,
        ),
        Framework::Vue => {
            let script = find_script(source, &["defineProps", "withDefaults"]);
            (script, source)
        }
        Framework::Svelte => {
            let script = find_script(source, &["export let", "$props"]);
            (script, source)
        }
    };

    let Some(script) = script else {
        analysis.slots = scan_slots(markup);
        analysis.description = leading_doc_comment(source);
        return analysis;
    };
    if script.text.len() > MAX_SCRIPT_BYTES {
        analysis.diagnostics.push(format!(
            "Component script exceeds {MAX_SCRIPT_BYTES} bytes; listed without props"
        ));
        analysis.slots = scan_slots(markup);
        return analysis;
    }
    let language = match framework {
        Framework::React => tree_sitter_typescript::LANGUAGE_TSX,
        _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    };
    let tree = match parse_tree(script.text, language.into()) {
        Ok(tree) => tree,
        Err(message) => {
            analysis.diagnostics.push(message);
            analysis.slots = scan_slots(markup);
            analysis.description = leading_doc_comment(script.text);
            return analysis;
        }
    };

    analysis.description = leading_doc_comment(script.text);
    let root = tree.root_node();
    let mut catalog = collect_catalog(root, script.text);
    extend_with_relative_type_imports(&mut catalog, root, script.text, path, project_root);

    let mut children_as_slot = false;
    let props = match framework {
        Framework::Astro => astro_props(root, script.text, &catalog),
        Framework::React => {
            let (props, children) = react_props(root, script.text, name, &catalog);
            children_as_slot = children;
            props
        }
        Framework::Vue => vue_props(root, script.text, &catalog),
        Framework::Svelte => {
            let (props, children) = svelte_props(root, script.text, &catalog);
            children_as_slot = children;
            props
        }
    };
    analysis.props = props;
    analysis.slots = scan_slots(markup);
    if children_as_slot && !analysis.slots.iter().any(|slot| slot.name.is_none()) {
        analysis.slots.insert(0, Slot { name: None });
    }
    analysis
}

fn parse_tree(source: &str, language: tree_sitter::Language) -> Result<Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|error| format!("failed to load the TypeScript parser: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "TypeScript parser returned no syntax tree".to_owned())?;
    if tree.root_node().has_error() {
        return Err("Could not parse the component script; listed without props".into());
    }
    validate_ast_bounds(tree.root_node())?;
    Ok(tree)
}

fn validate_ast_bounds(root: Node<'_>) -> Result<(), String> {
    let mut pending = vec![(root, 0_usize)];
    let mut count = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        count += 1;
        if count > MAX_AST_NODES {
            return Err(format!(
                "TypeScript AST exceeds the supported node count of {MAX_AST_NODES}"
            ));
        }
        if depth > MAX_AST_DEPTH {
            return Err(format!(
                "TypeScript AST exceeds the supported depth of {MAX_AST_DEPTH}"
            ));
        }
        for index in (0..node.child_count() as u32).rev() {
            if let Some(child) = node.child(index) {
                pending.push((child, depth + 1));
            }
        }
    }
    Ok(())
}

fn astro_frontmatter(source: &str) -> Option<ScriptRegion<'_>> {
    let start = usize::from(source.starts_with('\u{feff}')) * 3;
    let first_end = source[start..].find('\n').map(|index| start + index)?;
    let first = source[start..first_end].trim_end_matches('\r').trim_end();
    if first != "---" {
        return None;
    }
    let body_start = first_end + 1;
    let mut cursor = body_start;
    while cursor <= source.len() {
        let line_end = source[cursor..]
            .find('\n')
            .map(|index| cursor + index)
            .unwrap_or(source.len());
        let line = source[cursor..line_end].trim_end_matches('\r');
        if line.trim_end() == "---" {
            return Some(ScriptRegion {
                text: &source[body_start..cursor],
                offset: body_start,
            });
        }
        if line_end == source.len() {
            break;
        }
        cursor = line_end + 1;
    }
    None
}

fn astro_markup_start(source: &str, delimiter: usize) -> usize {
    source[delimiter..]
        .find('\n')
        .map(|offset| delimiter + offset + 1)
        .unwrap_or(source.len())
}

fn find_script<'a>(source: &'a str, preferred: &[&str]) -> Option<ScriptRegion<'a>> {
    let lower = source.to_ascii_lowercase();
    let mut first = None;
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<script") {
        let open = cursor + offset;
        let after = open + "<script".len();
        let boundary = lower[after..].chars().next();
        if !boundary.is_some_and(|character| {
            character.is_ascii_whitespace() || matches!(character, '>' | '/')
        }) {
            cursor = after;
            continue;
        }
        let Some(tag_end) = lower[after..].find('>').map(|index| after + index + 1) else {
            break;
        };
        let Some(close) = lower[tag_end..].find("</script") else {
            break;
        };
        let content_end = tag_end + close;
        let region = ScriptRegion {
            text: &source[tag_end..content_end],
            offset: tag_end,
        };
        if preferred.iter().any(|needle| region.text.contains(needle)) {
            return Some(region);
        }
        first.get_or_insert(region);
        cursor = lower[content_end..]
            .find('>')
            .map(|index| content_end + index + 1)
            .unwrap_or(source.len());
    }
    first
}

fn leading_doc_comment(source: &str) -> Option<String> {
    let start = source.find("/**")?;
    let end = source[start + 3..].find("*/")? + start + 3;
    let cleaned = clean_doc(&source[start..end + 2])?;
    (!cleaned.is_empty()).then_some(cleaned)
}

fn clean_doc(comment: &str) -> Option<String> {
    let body = comment.strip_prefix("/**")?.strip_suffix("*/")?;
    let mut lines = Vec::new();
    for line in body.lines() {
        let line = line.trim().strip_prefix('*').unwrap_or(line.trim()).trim();
        if line.is_empty() || line.starts_with('@') {
            continue;
        }
        lines.push(line);
    }
    (!lines.is_empty()).then(|| lines.join(" "))
}

fn collect_catalog(root: Node<'_>, source: &str) -> TypeCatalog {
    let mut catalog = TypeCatalog::default();
    for child in named_children(root) {
        let declaration = match child.kind() {
            "export_statement" => child
                .child_by_field_name("declaration")
                .or_else(|| named_children(child).first().copied()),
            _ => Some(child),
        };
        let Some(declaration) = declaration else {
            continue;
        };
        match declaration.kind() {
            "interface_declaration" => {
                let Some(name) = field_text(declaration, "name", source) else {
                    continue;
                };
                let mut bases = Vec::new();
                if let Some(extends) = named_children(declaration)
                    .into_iter()
                    .find(|child| child.kind() == "extends_type_clause")
                {
                    for base in named_children(extends) {
                        if let Some(text) = node_text(base, source) {
                            bases.push(text.trim().to_owned());
                        }
                    }
                }
                let members = declaration
                    .child_by_field_name("body")
                    .map(|body| object_members(body, source))
                    .unwrap_or_default();
                catalog
                    .records
                    .insert(name, TypeRecord::Object { members, bases });
            }
            "type_alias_declaration" => {
                let Some(name) = field_text(declaration, "name", source) else {
                    continue;
                };
                let Some(value) = declaration.child_by_field_name("value") else {
                    continue;
                };
                let record = match value.kind() {
                    "object_type" => TypeRecord::Object {
                        members: object_members(value, source),
                        bases: Vec::new(),
                    },
                    "intersection_type" => {
                        let mut members = Vec::new();
                        let mut bases = Vec::new();
                        for part in named_children(value) {
                            match part.kind() {
                                "object_type" => members.extend(object_members(part, source)),
                                _ => {
                                    if let Some(text) = node_text(part, source) {
                                        bases.push(text.trim().to_owned());
                                    }
                                }
                            }
                        }
                        TypeRecord::Object { members, bases }
                    }
                    _ => TypeRecord::Alias(node_text(value, source).unwrap_or_default().to_owned()),
                };
                catalog.records.insert(name, record);
            }
            "enum_declaration" => {
                let Some(name) = field_text(declaration, "name", source) else {
                    continue;
                };
                let values = declaration
                    .child_by_field_name("body")
                    .map(|body| enum_values(body, source))
                    .unwrap_or_default();
                catalog.records.insert(name, TypeRecord::Enum(values));
            }
            _ => {}
        }
    }
    catalog
}

fn object_members(body: Node<'_>, source: &str) -> Vec<TypeMember> {
    let mut members = Vec::new();
    for child in named_children(body) {
        if child.kind() != "property_signature" {
            continue;
        }
        let Some(name_node) = child.child_by_field_name("name") else {
            continue;
        };
        let Some(name) = node_text(name_node, source).map(unquote_name) else {
            continue;
        };
        let type_text = child
            .child_by_field_name("type")
            .and_then(|annotation| node_text(annotation, source))
            .map(|text| text.trim_start_matches(':').trim().to_owned())
            .unwrap_or_default();
        let optional = child.child_by_field_name("type").is_some_and(|annotation| {
            source
                .get(name_node.end_byte()..annotation.start_byte())
                .is_some_and(|between| between.contains('?'))
        });
        let doc = child
            .prev_named_sibling()
            .filter(|sibling| sibling.kind() == "comment")
            .and_then(|comment| node_text(comment, source))
            .and_then(clean_doc);
        members.push(TypeMember {
            name,
            type_text,
            optional,
            doc,
        });
    }
    members
}

fn enum_values(body: Node<'_>, source: &str) -> Vec<String> {
    let mut values = Vec::new();
    for child in named_children(body) {
        match child.kind() {
            "enum_assignment" => {
                let value = child
                    .child_by_field_name("value")
                    .and_then(|value| node_text(value, source))
                    .map(|text| unquote_name(text.trim()))
                    .unwrap_or_default();
                if !value.is_empty() {
                    values.push(value);
                }
            }
            "property_identifier" => {
                if let Some(name) = node_text(child, source) {
                    values.push(name.to_owned());
                }
            }
            _ => {}
        }
    }
    values
}

fn extend_with_relative_type_imports(
    catalog: &mut TypeCatalog,
    root: Node<'_>,
    source: &str,
    file: &Path,
    project_root: &Path,
) {
    let mut followed = 0_usize;
    let mut visited = HashSet::new();
    visited.insert(file.to_path_buf());
    for statement in named_children(root) {
        if statement.kind() != "import_statement" || followed >= MAX_TYPE_IMPORTS {
            continue;
        }
        let Some(specifier) = statement
            .child_by_field_name("source")
            .and_then(|module| node_text(module, source))
            .map(unquote_name)
        else {
            continue;
        };
        if !specifier.starts_with('.') {
            continue;
        }
        let Some(imported_file) = resolve_relative_type_file(file, &specifier, project_root) else {
            continue;
        };
        if !visited.insert(imported_file.clone()) {
            continue;
        }
        let Ok(imported_source) = read_bounded(&imported_file, MAX_TYPE_FILE_BYTES) else {
            continue;
        };
        let language = match imported_file
            .extension()
            .and_then(|extension| extension.to_str())
        {
            Some("tsx" | "jsx") => tree_sitter_typescript::LANGUAGE_TSX,
            _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        };
        let Ok(tree) = parse_tree(&imported_source, language.into()) else {
            continue;
        };
        let imported = collect_catalog(tree.root_node(), &imported_source);
        let Some(clause) = named_children(statement)
            .into_iter()
            .find(|child| child.kind() == "import_clause")
        else {
            continue;
        };
        for item in named_children(clause) {
            if item.kind() != "named_imports" {
                continue;
            }
            for specifier_node in named_children(item) {
                if specifier_node.kind() != "import_specifier" {
                    continue;
                }
                let Some(imported_name) = specifier_node
                    .child_by_field_name("name")
                    .and_then(|name| node_text(name, source))
                    .map(unquote_name)
                else {
                    continue;
                };
                let local_name = specifier_node
                    .child_by_field_name("alias")
                    .and_then(|alias| node_text(alias, source))
                    .unwrap_or(&imported_name)
                    .to_owned();
                if let Some(record) = imported.records.get(&imported_name) {
                    catalog
                        .records
                        .entry(local_name)
                        .or_insert_with(|| record.clone());
                }
            }
        }
        followed += 1;
    }
}

fn resolve_relative_type_file(file: &Path, specifier: &str, root: &Path) -> Option<PathBuf> {
    if specifier.contains('\0') {
        return None;
    }
    let base = file.parent()?.join(specifier);
    let candidates = if base.extension().is_some() {
        vec![base]
    } else {
        vec![
            base.with_extension("ts"),
            base.with_extension("tsx"),
            base.with_extension("d.ts"),
            base.with_extension("js"),
            base.with_extension("jsx"),
            base.join("index.ts"),
            base.join("index.tsx"),
            base.join("index.d.ts"),
            base.join("index.js"),
        ]
    };
    for candidate in candidates {
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if !canonical.starts_with(root) || !canonical.is_file() {
            continue;
        }
        return Some(canonical);
    }
    None
}

fn astro_props(root: Node<'_>, source: &str, catalog: &TypeCatalog) -> Vec<Prop> {
    let (names, defaults) = astro_defaults(root, source);
    let mut members = catalog
        .records
        .get("Props")
        .map(|record| members_for_record(record, catalog, 0))
        .unwrap_or_default();
    for name in &names {
        if !members.iter().any(|member| &member.name == name) {
            members.push(TypeMember {
                name: name.clone(),
                type_text: String::new(),
                optional: false,
                doc: None,
            });
        }
    }
    build_props(&members, &defaults, catalog, None)
}

fn astro_defaults(root: Node<'_>, source: &str) -> (Vec<String>, BTreeMap<String, PropDefault>) {
    let mut names = Vec::new();
    let mut defaults = BTreeMap::new();
    for child in named_children(root) {
        if child.kind() != "lexical_declaration" {
            continue;
        }
        for declarator in named_children(child) {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let is_astro_props = declarator
                .child_by_field_name("value")
                .and_then(|value| node_text(value, source))
                .is_some_and(|text| text.trim() == "Astro.props");
            if !is_astro_props {
                continue;
            }
            if let Some(pattern) = declarator.child_by_field_name("name") {
                let (found_names, found_defaults) = pattern_info(pattern, source);
                for name in found_names {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                defaults.extend(found_defaults);
            }
        }
    }
    (names, defaults)
}

fn react_props(
    root: Node<'_>,
    source: &str,
    name: &str,
    catalog: &TypeCatalog,
) -> (Vec<Prop>, bool) {
    let Some((parameters, type_text)) = react_parameters(root, source, name) else {
        return (Vec::new(), false);
    };
    let mut names = Vec::new();
    let mut defaults = BTreeMap::new();
    let mut has_children_pattern = false;
    if let Some(parameter) = parameters.into_iter().next() {
        let pattern = parameter
            .child_by_field_name("pattern")
            .or_else(|| parameter.child_by_field_name("name"))
            .or(Some(parameter));
        if let Some(pattern) = pattern
            && pattern.kind() == "object_pattern"
        {
            let (found_names, found_defaults) = pattern_info(pattern, source);
            has_children_pattern = found_names.iter().any(|name| name == "children");
            for name in found_names {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            defaults.extend(found_defaults);
        }
    }
    let (mut members, children_from_type) = match type_text {
        Some(text) => type_members(&text, catalog),
        None => (Vec::new(), false),
    };
    for name in &names {
        if !members.iter().any(|member| &member.name == name) {
            members.push(TypeMember {
                name: name.clone(),
                type_text: String::new(),
                optional: false,
                doc: None,
            });
        }
    }
    let children = children_from_type || has_children_pattern;
    (
        build_props(&members, &defaults, catalog, Some("children")),
        children,
    )
}

fn react_parameters<'tree>(
    root: Node<'tree>,
    source: &str,
    name: &str,
) -> Option<(Vec<Node<'tree>>, Option<String>)> {
    let mut declarations = Vec::new();
    for child in named_children(root) {
        let declaration = if child.kind() == "export_statement" {
            child.child_by_field_name("declaration")
        } else {
            Some(child)
        };
        let Some(declaration) = declaration else {
            continue;
        };
        if declaration.kind() == "lexical_declaration" {
            declarations.extend(
                named_children(declaration)
                    .into_iter()
                    .filter(|item| item.kind() == "variable_declarator"),
            );
        } else {
            declarations.push(declaration);
        }
    }
    for child in declarations {
        if child.kind() == "function_declaration" {
            if field_text(child, "name", source).as_deref() == Some(name) {
                return child.child_by_field_name("parameters").map(|parameters| {
                    let params = named_children(parameters);
                    let type_text = parameter_type(&params, source);
                    (params, type_text)
                });
            }
            continue;
        }
        if child.kind() != "variable_declarator" {
            continue;
        }
        if field_text(child, "name", source).as_deref() != Some(name) {
            continue;
        }
        let Some(value) = child.child_by_field_name("value") else {
            continue;
        };
        if !matches!(value.kind(), "arrow_function" | "function_expression") {
            continue;
        }
        let parameters = value
            .child_by_field_name("parameters")
            .map(named_children)
            .or_else(|| {
                value
                    .child_by_field_name("parameter")
                    .map(|parameter| vec![parameter])
            })?;
        let fc_type = child
            .child_by_field_name("type")
            .and_then(|annotation| node_text(annotation, source))
            .and_then(fc_type_argument);
        let type_text = parameter_type(&parameters, source).or(fc_type);
        return Some((parameters, type_text));
    }
    None
}

fn parameter_type(parameters: &[Node<'_>], source: &str) -> Option<String> {
    let parameter = parameters.first()?;
    let annotation = parameter.child_by_field_name("type")?;
    let text = node_text(annotation, source)?;
    let text = text.trim().trim_start_matches(':').trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn fc_type_argument(text: &str) -> Option<String> {
    let text = text.trim().trim_start_matches(':').trim();
    let inner = text
        .strip_prefix("React.FC<")
        .or_else(|| text.strip_prefix("FC<"))
        .or_else(|| text.strip_prefix("React.FunctionComponent<"))
        .or_else(|| text.strip_prefix("FunctionComponent<"))?;
    let inner = inner.strip_suffix('>')?.trim();
    if inner.is_empty() {
        return None;
    }
    Some(inner.to_owned())
}

fn vue_props(root: Node<'_>, source: &str, catalog: &TypeCatalog) -> Vec<Prop> {
    let mut defaults = BTreeMap::new();
    let mut type_text = None;
    if let Some(call) = find_call(root, source, "withDefaults")
        && let Some(arguments) = call.child_by_field_name("arguments")
    {
        let args = named_children(arguments);
        if let Some(props_call) = args.first() {
            type_text = define_props_type(props_call, source);
        }
        if let Some(defaults_object) = args.get(1) {
            defaults = object_defaults(*defaults_object, source);
        }
    }
    if type_text.is_none()
        && let Some(call) = find_call(root, source, "defineProps")
    {
        type_text = define_props_type(&call, source);
    }
    if let Some(type_text) = type_text {
        let members = match type_text {
            TypeText::Inline(node) => object_members(node, source),
            TypeText::Reference(name) => catalog
                .records
                .get(&name)
                .map(|record| members_for_record(record, catalog, 0))
                .unwrap_or_default(),
        };
        return build_props(&members, &defaults, catalog, None);
    }
    let Some(call) = find_call(root, source, "defineProps") else {
        return Vec::new();
    };
    let Some((members, runtime_defaults)) = runtime_props(&call, source) else {
        return Vec::new();
    };
    for (name, default) in runtime_defaults {
        defaults.entry(name).or_insert(default);
    }
    build_props(&members, &defaults, catalog, None)
}

fn runtime_props(
    call: &Node<'_>,
    source: &str,
) -> Option<(Vec<TypeMember>, BTreeMap<String, PropDefault>)> {
    let arguments = call.child_by_field_name("arguments")?;
    let object = named_children(arguments).into_iter().next()?;
    if object.kind() != "object" {
        return None;
    }
    let mut members = Vec::new();
    let mut defaults = BTreeMap::new();
    for pair in named_children(object) {
        if pair.kind() != "pair" {
            continue;
        }
        let Some(name) = pair
            .child_by_field_name("key")
            .and_then(|key| node_text(key, source))
            .map(unquote_name)
        else {
            continue;
        };
        let Some(value) = pair.child_by_field_name("value") else {
            continue;
        };
        let doc = pair
            .prev_named_sibling()
            .filter(|sibling| sibling.kind() == "comment")
            .and_then(|comment| node_text(comment, source))
            .and_then(clean_doc);
        let mut type_text = "string".to_owned();
        let mut optional = true;
        if value.kind() == "object" {
            for option in named_children(value) {
                if option.kind() != "pair" {
                    continue;
                }
                let Some(key) = option
                    .child_by_field_name("key")
                    .and_then(|key| node_text(key, source))
                else {
                    continue;
                };
                let Some(option_value) = option.child_by_field_name("value") else {
                    continue;
                };
                match key.trim() {
                    "type" => {
                        type_text = runtime_type_text(option_value, source);
                    }
                    "required" => {
                        optional = !node_text(option_value, source)
                            .is_some_and(|text| text.trim() == "true");
                    }
                    "default" => {
                        if let Some(default) = default_from_expression(option_value, source) {
                            defaults.insert(name.clone(), default);
                        }
                    }
                    _ => {}
                }
            }
        } else {
            type_text = runtime_type_text(value, source);
        }
        members.push(TypeMember {
            name,
            type_text,
            optional,
            doc,
        });
    }
    Some((members, defaults))
}

fn runtime_type_text(node: Node<'_>, source: &str) -> String {
    match node.kind() {
        "identifier" | "type_identifier" => match node_text(node, source).unwrap_or_default() {
            "String" => "string",
            "Number" => "number",
            "Boolean" => "boolean",
            "Date" => "Date",
            "Array" => "string[]",
            _ => "string",
        }
        .to_owned(),
        "array" => "string[]".to_owned(),
        _ => "string".to_owned(),
    }
}

enum TypeText<'tree> {
    Inline(Node<'tree>),
    Reference(String),
}

fn define_props_type<'tree>(call: &Node<'tree>, source: &str) -> Option<TypeText<'tree>> {
    let function = call.child_by_field_name("function")?;
    if node_text(function, source)?.trim() != "defineProps" {
        return None;
    }
    let arguments = call.child_by_field_name("type_arguments")?;
    let first = named_children(arguments).into_iter().next()?;
    if first.kind() == "object_type" {
        return Some(TypeText::Inline(first));
    }
    let name = node_text(first, source)?.trim().to_owned();
    (!name.is_empty()).then_some(TypeText::Reference(name))
}

fn find_call<'tree>(node: Node<'tree>, source: &str, name: &str) -> Option<Node<'tree>> {
    let mut pending = vec![node];
    while let Some(node) = pending.pop() {
        if node.kind() == "call_expression"
            && node
                .child_by_field_name("function")
                .and_then(|function| node_text(function, source))
                .is_some_and(|function| function.trim() == name)
        {
            return Some(node);
        }
        for child in named_children(node) {
            pending.push(child);
        }
    }
    None
}

fn svelte_props(root: Node<'_>, source: &str, catalog: &TypeCatalog) -> (Vec<Prop>, bool) {
    let mut props = Vec::new();
    let mut children = false;
    let mut names = Vec::new();
    let mut defaults = BTreeMap::new();
    let mut members = Vec::new();

    // Svelte 5: `let { a, b = 1 }: Props = $props();`
    for child in named_children(root) {
        if child.kind() != "lexical_declaration" {
            continue;
        }
        for declarator in named_children(child) {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let is_props_call = declarator
                .child_by_field_name("value")
                .and_then(|value| node_text(value, source))
                .is_some_and(|text| text.trim() == "$props()");
            if !is_props_call {
                continue;
            }
            let Some(pattern) = declarator.child_by_field_name("name") else {
                continue;
            };
            if pattern.kind() == "object_pattern" {
                let (found_names, found_defaults) = pattern_info(pattern, source);
                children = found_names.iter().any(|name| name == "children");
                for name in found_names {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                defaults.extend(found_defaults);
            }
            if let Some(type_text) = declarator
                .child_by_field_name("type")
                .and_then(|annotation| node_text(annotation, source))
            {
                let (found_members, children_from_type) = type_members(type_text, catalog);
                members = found_members;
                children |= children_from_type;
            }
        }
    }
    if !members.is_empty() || !names.is_empty() {
        for name in &names {
            if !members.iter().any(|member| &member.name == name) {
                members.push(TypeMember {
                    name: name.clone(),
                    type_text: String::new(),
                    optional: false,
                    doc: None,
                });
            }
        }
        return (
            build_props(&members, &defaults, catalog, Some("children")),
            children,
        );
    }

    // Svelte 3/4: `export let prop: type = default;`
    for child in named_children(root) {
        let declaration = if child.kind() == "export_statement" {
            child
                .child_by_field_name("declaration")
                .or_else(|| named_children(child).first().copied())
        } else {
            continue;
        };
        let Some(declaration) = declaration else {
            continue;
        };
        if declaration.kind() != "lexical_declaration" {
            continue;
        }
        for declarator in named_children(declaration) {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let Some(pattern) = declarator.child_by_field_name("name") else {
                continue;
            };
            if pattern.kind() != "identifier" {
                continue;
            }
            let Some(name) = node_text(pattern, source) else {
                continue;
            };
            let type_text = declarator
                .child_by_field_name("type")
                .and_then(|annotation| node_text(annotation, source))
                .map(|text| text.trim_start_matches(':').trim().to_owned())
                .unwrap_or_else(|| "string".into());
            let default = declarator
                .child_by_field_name("value")
                .and_then(|value| default_from_expression(value, source));
            let classified = classify_type(&type_text, catalog, 0);
            let doc = child
                .prev_named_sibling()
                .filter(|sibling| sibling.kind() == "comment")
                .and_then(|comment| node_text(comment, source))
                .and_then(clean_doc);
            props.push(Prop {
                name: name.to_owned(),
                kind: classified.kind,
                required: !(classified.nullable || default.is_some()),
                default,
                description: doc,
            });
        }
    }
    (props, children)
}

fn build_props(
    members: &[TypeMember],
    defaults: &BTreeMap<String, PropDefault>,
    catalog: &TypeCatalog,
    skip: Option<&str>,
) -> Vec<Prop> {
    let mut props = Vec::new();
    let mut seen = BTreeSet::new();
    for member in members {
        if skip == Some(member.name.as_str()) || !seen.insert(member.name.clone()) {
            continue;
        }
        let classified = if member.type_text.is_empty() {
            ClassifiedType {
                kind: defaults
                    .get(&member.name)
                    .map(kind_for_default)
                    .unwrap_or(PropKind::String),
                nullable: false,
            }
        } else {
            classify_type(&member.type_text, catalog, 0)
        };
        let default = defaults.get(&member.name).cloned();
        props.push(Prop {
            name: member.name.clone(),
            kind: classified.kind,
            required: !(member.optional || classified.nullable || default.is_some()),
            default,
            description: member.doc.clone(),
        });
    }
    // Destructured values with no declaration still deserve a control.
    for (name, default) in defaults {
        if skip == Some(name.as_str()) || seen.contains(name) {
            continue;
        }
        props.push(Prop {
            name: name.clone(),
            kind: kind_for_default(default),
            required: false,
            default: Some(default.clone()),
            description: None,
        });
    }
    props
}

fn members_for_record(record: &TypeRecord, catalog: &TypeCatalog, depth: usize) -> Vec<TypeMember> {
    if depth > 8 {
        return Vec::new();
    }
    match record {
        TypeRecord::Object { members, bases } => {
            let mut merged = Vec::new();
            for base in bases {
                if let Some(base_record) = catalog.records.get(base) {
                    merged.extend(members_for_record(base_record, catalog, depth + 1));
                }
            }
            for member in members {
                if let Some(existing) = merged.iter_mut().find(|item| item.name == member.name) {
                    *existing = member.clone();
                } else {
                    merged.push(member.clone());
                }
            }
            merged
        }
        TypeRecord::Enum(_) | TypeRecord::Alias(_) => Vec::new(),
    }
}

fn type_members(type_text: &str, catalog: &TypeCatalog) -> (Vec<TypeMember>, bool) {
    let text = type_text.trim().trim_start_matches(':').trim();
    if text.starts_with('{') {
        let synthetic = format!("type __Props = {text};");
        let members = parse_tree(
            &synthetic,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        )
        .ok()
        .and_then(|tree| {
            named_children(tree.root_node())
                .into_iter()
                .find(|child| child.kind() == "type_alias_declaration")
                .and_then(|alias| alias.child_by_field_name("value"))
                .map(|value| object_members(value, &synthetic))
        })
        .unwrap_or_default();
        let children = members.iter().any(|member| member.name == "children");
        return (members, children);
    }
    match catalog.records.get(text) {
        Some(record) => {
            let members = members_for_record(record, catalog, 0);
            let children = members.iter().any(|member| member.name == "children");
            (members, children)
        }
        None => (Vec::new(), false),
    }
}

fn classify_type(text: &str, catalog: &TypeCatalog, depth: usize) -> ClassifiedType {
    if depth > 8 {
        return ClassifiedType {
            kind: PropKind::String,
            nullable: false,
        };
    }
    let text = trim_type(text);
    let parts = split_union(text);
    if parts.len() > 1 {
        let mut literals = Vec::new();
        let mut others = Vec::new();
        let mut nullable = false;
        for part in &parts {
            let part = trim_type(part);
            if part == "null" || part == "undefined" {
                nullable = true;
            } else if let Some(value) = string_literal(part) {
                literals.push(value);
            } else {
                others.push(part.to_owned());
            }
        }
        if others.is_empty() && !literals.is_empty() {
            return ClassifiedType {
                kind: PropKind::Enum { values: literals },
                nullable,
            };
        }
        if others.len() == 1 {
            let mut classified = classify_type(&others[0], catalog, depth + 1);
            classified.nullable |= nullable;
            return classified;
        }
        return ClassifiedType {
            kind: PropKind::String,
            nullable,
        };
    }
    let text = parts.first().map(|part| trim_type(part)).unwrap_or(text);
    if text.is_empty() || text == "any" || text == "unknown" {
        return ClassifiedType {
            kind: PropKind::String,
            nullable: false,
        };
    }
    if let Some(inner) = text.strip_suffix("[]") {
        let inner = trim_type(inner);
        let kind = match inner {
            "string" | "String" => PropKind::StringArray,
            "number" | "Number" => PropKind::NumberArray,
            _ => PropKind::String,
        };
        return ClassifiedType {
            kind,
            nullable: false,
        };
    }
    for prefix in ["Array<", "ReadonlyArray<"] {
        if let Some(inner) = text
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_suffix('>'))
        {
            let kind = match trim_type(inner) {
                "string" | "String" => PropKind::StringArray,
                "number" | "Number" => PropKind::NumberArray,
                _ => PropKind::String,
            };
            return ClassifiedType {
                kind,
                nullable: false,
            };
        }
    }
    if let Some(inner) = text
        .strip_prefix("CollectionEntry<")
        .and_then(|rest| rest.strip_suffix('>'))
    {
        let collection = split_union(inner)
            .into_iter()
            .find_map(|part| string_literal(trim_type(part)))
            .unwrap_or_else(|| trim_type(inner).to_owned());
        return ClassifiedType {
            kind: PropKind::CollectionEntry { collection },
            nullable: false,
        };
    }
    match text {
        "string" | "String" => ClassifiedType {
            kind: PropKind::String,
            nullable: false,
        },
        "number" | "Number" => ClassifiedType {
            kind: PropKind::Number,
            nullable: false,
        },
        "boolean" | "Boolean" => ClassifiedType {
            kind: PropKind::Boolean,
            nullable: false,
        },
        "Date" => ClassifiedType {
            kind: PropKind::Date,
            nullable: false,
        },
        "ImageMetadata" => ClassifiedType {
            kind: PropKind::Image,
            nullable: false,
        },
        _ => {
            if let Some(value) = string_literal(text) {
                return ClassifiedType {
                    kind: PropKind::Enum {
                        values: vec![value],
                    },
                    nullable: false,
                };
            }
            match catalog.records.get(text) {
                Some(TypeRecord::Enum(values)) => ClassifiedType {
                    kind: PropKind::Enum {
                        values: values.clone(),
                    },
                    nullable: false,
                },
                Some(TypeRecord::Alias(alias)) => classify_type(alias, catalog, depth + 1),
                Some(TypeRecord::Object { .. }) => ClassifiedType {
                    kind: PropKind::String,
                    nullable: false,
                },
                None => ClassifiedType {
                    kind: PropKind::String,
                    nullable: false,
                },
            }
        }
    }
}

fn trim_type(text: &str) -> &str {
    let mut text = text.trim();
    while text.starts_with('(') && text.ends_with(')') {
        text = text[1..text.len() - 1].trim();
    }
    text
}

fn split_union(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0_i32;
    let mut quote = None;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if byte == active && (index == 0 || bytes[index - 1] != b'\\') {
                quote = None;
            }
            index += 1;
            continue;
        }
        match byte {
            b'\'' | b'"' | b'`' => quote = Some(byte),
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' | b')' | b']' | b'}' => depth -= 1,
            b'|' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
        index += 1;
    }
    parts.push(&text[start..]);
    parts
}

fn string_literal(text: &str) -> Option<String> {
    let text = text.trim();
    let quote = text.chars().next()?;
    if !matches!(quote, '\'' | '"') || !text.ends_with(quote) || text.len() < 2 {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    if inner.contains(quote) {
        return None;
    }
    Some(inner.to_owned())
}

fn kind_for_default(default: &PropDefault) -> PropKind {
    match default {
        PropDefault::String(_) => PropKind::String,
        PropDefault::Number(_) => PropKind::Number,
        PropDefault::Boolean(_) => PropKind::Boolean,
        PropDefault::List(values) => {
            let all_numbers =
                !values.is_empty() && values.iter().all(|value| value.parse::<f64>().is_ok());
            if all_numbers {
                PropKind::NumberArray
            } else {
                PropKind::StringArray
            }
        }
    }
}

fn pattern_info(pattern: Node<'_>, source: &str) -> (Vec<String>, BTreeMap<String, PropDefault>) {
    let mut names = Vec::new();
    let mut defaults = BTreeMap::new();
    for child in named_children(pattern) {
        match child.kind() {
            "shorthand_property_identifier_pattern" => {
                if let Some(name) = node_text(child, source)
                    && !names.iter().any(|existing| existing == name)
                {
                    names.push(name.to_owned());
                }
            }
            "object_assignment_pattern" => {
                let Some(name) = child
                    .child_by_field_name("left")
                    .and_then(|left| node_text(left, source))
                else {
                    continue;
                };
                if !names.iter().any(|existing| existing == name) {
                    names.push(name.to_owned());
                }
                if let Some(value) = child
                    .child_by_field_name("right")
                    .and_then(|right| default_from_expression(right, source))
                {
                    defaults.insert(name.to_owned(), value);
                }
            }
            "pair_pattern" => {
                let Some(name) = child
                    .child_by_field_name("key")
                    .and_then(|key| node_text(key, source))
                    .map(unquote_name)
                else {
                    continue;
                };
                let default =
                    child
                        .child_by_field_name("value")
                        .and_then(|value| match value.kind() {
                            "assignment_pattern" => value
                                .child_by_field_name("right")
                                .and_then(|right| default_from_expression(right, source)),
                            _ => None,
                        });
                if !names.contains(&name) {
                    names.push(name.clone());
                }
                if let Some(default) = default {
                    defaults.insert(name, default);
                }
            }
            _ => {}
        }
    }
    (names, defaults)
}

fn object_defaults(object: Node<'_>, source: &str) -> BTreeMap<String, PropDefault> {
    let mut defaults = BTreeMap::new();
    for child in named_children(object) {
        if child.kind() != "pair" {
            continue;
        }
        let Some(name) = child
            .child_by_field_name("key")
            .and_then(|key| node_text(key, source))
            .map(unquote_name)
        else {
            continue;
        };
        if let Some(value) = child
            .child_by_field_name("value")
            .and_then(|value| default_from_expression(value, source))
        {
            defaults.insert(name, value);
        }
    }
    defaults
}

fn default_from_expression(node: Node<'_>, source: &str) -> Option<PropDefault> {
    match node.kind() {
        "string" => {
            let text = node_text(node, source)?.trim();
            string_literal(text).map(PropDefault::String)
        }
        "template_string" => {
            let text = node_text(node, source)?.trim();
            text.strip_prefix('`')
                .and_then(|text| text.strip_suffix('`'))
                .filter(|text| !text.contains("${"))
                .map(|text| PropDefault::String(text.to_owned()))
        }
        "number" => node_text(node, source).map(|text| PropDefault::Number(text.to_owned())),
        "true" => Some(PropDefault::Boolean(true)),
        "false" => Some(PropDefault::Boolean(false)),
        "unary_expression" => {
            let text = node_text(node, source)?.trim();
            text.strip_prefix('-')
                .and_then(|rest| rest.trim().parse::<f64>().ok())
                .map(|_| PropDefault::Number(text.to_owned()))
        }
        "array" => {
            let mut values = Vec::new();
            for child in named_children(node) {
                match child.kind() {
                    "string" => {
                        let text = node_text(child, source)?.trim();
                        values.push(string_literal(text)?);
                    }
                    "number" => values.push(node_text(child, source)?.to_owned()),
                    _ => return None,
                }
            }
            Some(PropDefault::List(values))
        }
        "parenthesized_expression" => {
            let child = named_children(node).into_iter().next()?;
            default_from_expression(child, source)
        }
        "arrow_function" => {
            let body = node.child_by_field_name("body")?;
            default_from_expression(body, source)
        }
        "as_expression" => {
            let child = node
                .child_by_field_name("left")
                .or_else(|| named_children(node).into_iter().next())?;
            default_from_expression(child, source)
        }
        _ => None,
    }
}

fn scan_slots(markup: &str) -> Vec<Slot> {
    let lower = markup.to_ascii_lowercase();
    let mut skip = Vec::new();
    for (tag, close) in [("<script", "</script>"), ("<!--", "-->")] {
        let mut cursor = 0;
        while let Some(offset) = lower[cursor..].find(tag) {
            let start = cursor + offset;
            let Some(end_offset) = lower[start..].find(close) else {
                break;
            };
            let end = start + end_offset + close.len();
            skip.push(start..end);
            cursor = end;
        }
    }
    let mut slots = Vec::new();
    let mut default_slot = false;
    let mut names = BTreeSet::new();
    let mut cursor = 0;
    while let Some(offset) = lower[cursor..].find("<slot") {
        let start = cursor + offset;
        let after = start + "<slot".len();
        let boundary = lower[after..].chars().next();
        if !boundary.is_some_and(|character| {
            character.is_ascii_whitespace() || matches!(character, '>' | '/')
        }) {
            cursor = after;
            continue;
        }
        if skip.iter().any(|range| range.contains(&start)) {
            cursor = after;
            continue;
        }
        let Some(end_offset) = lower[after..].find('>') else {
            break;
        };
        let end = after + end_offset;
        let tag = &markup[after..end];
        match slot_name(tag) {
            Some(name) => {
                if names.insert(name.clone()) {
                    slots.push(Slot { name: Some(name) });
                }
            }
            None => default_slot = true,
        }
        cursor = end + 1;
    }
    if default_slot {
        slots.insert(0, Slot { name: None });
    }
    slots
}

fn slot_name(tag: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search = 0;
    let position = loop {
        let found = search + lower[search..].find("name")?;
        let before_ok = found == 0
            || tag[..found]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after = tag[found + "name".len()..].trim_start();
        if before_ok && after.starts_with('=') {
            break found;
        }
        search = found + "name".len();
    };
    let rest = &tag[position + "name".len()..];
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if !matches!(quote, '\'' | '"') {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    let name = &rest[1..end];
    (!name.is_empty()).then(|| name.to_owned())
}

fn unquote_name(text: &str) -> String {
    string_literal(text).unwrap_or_else(|| text.trim().to_owned())
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).collect()
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> Option<&'a str> {
    source.get(node.start_byte()..node.end_byte())
}

fn field_text(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|child| node_text(child, source))
        .map(|text| text.trim().trim_matches(['"', '\'']).to_owned())
}
