use std::collections::{HashMap, HashSet};

use tree_sitter::{Node, Parser, Tree};

use crate::pattern::CollectionPattern;

const DEFAULT_SRC_DIR: &str = "src";
const MAX_AST_NODES: usize = 100_000;
const MAX_AST_DEPTH: usize = 128;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectConfig {
    pub src_dir: String,
    pub mdx: bool,
    pub default_locale: Option<String>,
    pub prefix_default_locale: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionConfig {
    collection: String,
    base: Option<String>,
    pattern: Option<CollectionPattern>,
    file: Option<String>,
}

impl CollectionConfig {
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Glob loader base directory, when this collection uses `glob()`.
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// Glob loader pattern, when this collection uses `glob()`.
    pub fn pattern(&self) -> Option<&CollectionPattern> {
        self.pattern.as_ref()
    }

    /// Literal project-relative JSON file loaded by `file()`, when this is a
    /// data collection.
    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    /// Whether this collection is loaded from a single JSON file.
    pub fn is_data(&self) -> bool {
        self.file.is_some()
    }
}

#[derive(Default)]
struct ImportSet {
    bindings: HashSet<String>,
    define_collection_aliases: Vec<String>,
    glob_aliases: Vec<String>,
    file_aliases: Vec<String>,
    define_config_aliases: Vec<String>,
    mdx_default_aliases: Vec<String>,
}

pub fn parse_content_config(source: &str) -> Result<Vec<CollectionConfig>, String> {
    let tree = parse_tree(source)?;
    let root = tree.root_node();
    reject_mutations(root, source)?;

    let imports = collect_imports(root, source)?;
    let define_collection_alias = required_alias(
        &imports.define_collection_aliases,
        "defineCollection from astro:content",
    )?;
    let glob_alias = optional_alias(&imports.glob_aliases, "glob from astro/loaders")?;
    let file_alias = optional_alias(&imports.file_aliases, "file from astro/loaders")?;
    if glob_alias.is_none() && file_alias.is_none() {
        return Err("missing verified import of glob or file from astro/loaders".into());
    }

    let mut declarations = HashMap::new();
    let mut declaration_positions = HashMap::new();
    let mut declaration_order = Vec::new();
    let mut collections_object = None;

    for statement in named_children(root) {
        if is_comment(statement) {
            continue;
        }

        match statement.kind() {
            "import_statement" => {}
            "lexical_declaration" => {
                for (name, value) in const_declarators(statement, source)? {
                    reject_shadowing(&imports, &declarations, &name)?;
                    if declarations.insert(name.clone(), value).is_some() {
                        return Err(format!("duplicate top-level binding `{name}`"));
                    }
                    declaration_positions.insert(name.clone(), value.start_byte());
                    declaration_order.push(name);
                }
            }
            "export_statement" => {
                if collections_object.is_some() {
                    return Err("multiple exported collections declarations are unsupported".into());
                }
                if imports.bindings.contains("collections") {
                    return Err("`collections` shadows a verified import binding".into());
                }
                collections_object = Some(exported_collections_object(statement, source)?);
            }
            kind => {
                return Err(format!(
                    "top-level `{kind}` statements are unsupported in content config"
                ));
            }
        }
    }

    let collections_object = collections_object
        .ok_or_else(|| "content config must export `const collections`".to_owned())?;
    let mut known_bindings = HashSet::from([define_collection_alias.to_owned()]);
    known_bindings.extend(glob_alias.map(str::to_owned));
    known_bindings.extend(file_alias.map(str::to_owned));
    known_bindings.extend(declarations.keys().cloned());

    let mut parsed_declarations = HashMap::new();
    for name in declaration_order {
        let value = declarations
            .get(&name)
            .copied()
            .ok_or_else(|| format!("missing declaration for `{name}`"))?;
        let target = direct_identifier_call_target(value, source)?;
        if target != Some(define_collection_alias) {
            return Err(format!(
                "top-level local const `{name}` is unsupported; local const schemas and other non-defineCollection declarations are unsupported"
            ));
        }
        let config = parse_collection_call(
            value,
            source,
            define_collection_alias,
            glob_alias,
            file_alias,
            &known_bindings,
        )
        .map_err(|error| format!("collection `{name}`: {error}"))?;
        parsed_declarations.insert(name, config);
    }

    parse_collections_export(
        collections_object,
        &ExportContext {
            source,
            define_collection_alias,
            glob_alias,
            file_alias,
            known_bindings: &known_bindings,
        },
        &parsed_declarations,
        &declaration_positions,
    )
}

pub fn parse_project_config(source: &str) -> Result<ProjectConfig, String> {
    let tree = parse_tree(source)?;
    let root = tree.root_node();
    reject_mutations(root, source)?;

    let imports = collect_imports(root, source)?;
    let mut default_export = None;

    for statement in named_children(root) {
        if is_comment(statement) {
            continue;
        }

        match statement.kind() {
            "import_statement" => {}
            "export_statement" => {
                let value = statement.child_by_field_name("value").ok_or_else(|| {
                    "only `export default defineConfig(...)` is supported".to_owned()
                })?;
                if default_export.replace(value).is_some() {
                    return Err("multiple default exports are unsupported".into());
                }
            }
            kind => {
                return Err(format!(
                    "top-level `{kind}` statements are unsupported in Astro config"
                ));
            }
        }
    }

    let Some(default_value) = default_export else {
        return Ok(ProjectConfig {
            src_dir: DEFAULT_SRC_DIR.to_owned(),
            mdx: false,
            default_locale: None,
            prefix_default_locale: false,
        });
    };
    let define_config_alias = required_alias(
        &imports.define_config_aliases,
        "defineConfig from astro/config",
    )?;
    let config_object =
        direct_call_object(default_value, source, define_config_alias, "Astro config")?;
    reject_nested_proto_keys(config_object, source)?;
    let pairs = strict_object_pairs(config_object, source, "Astro config")?;
    let mut src_dir = None;
    let mut mdx = false;
    let mut default_locale = None;
    let mut prefix_default_locale = false;

    for (key, value) in pairs {
        match key.as_str() {
            "root" => return Err("`root` in Astro config is unsupported".into()),
            "legacy" | "experimental" => {
                return Err(format!(
                    "`{key}` in Astro config may change content discovery and is unsupported"
                ));
            }
            "srcDir" => {
                let literal = literal_string(value, source, "srcDir")?;
                validate_project_relative_path(&literal, "srcDir")?;
                if src_dir.replace(literal).is_some() {
                    return Err("duplicate `srcDir` key".into());
                }
            }
            "integrations" => {
                mdx = validate_integrations(value, source, &imports.mdx_default_aliases)?;
            }
            "i18n" => {
                (default_locale, prefix_default_locale) = parse_i18n_config(value, source)?;
            }
            // Other Astro settings (adapter, site, markdown, vite, fonts, ...)
            // cannot change which files a `glob()` collection selects, so their
            // values are intentionally not inspected or evaluated.
            _ => {}
        }
    }

    Ok(ProjectConfig {
        src_dir: src_dir.unwrap_or_else(|| DEFAULT_SRC_DIR.to_owned()),
        mdx,
        default_locale,
        prefix_default_locale,
    })
}

fn parse_i18n_config(value: Node<'_>, source: &str) -> Result<(Option<String>, bool), String> {
    let object = value;
    if object.kind() != "object" {
        return Err("`i18n` in Astro config must be a static object".into());
    }
    let mut default_locale = None;
    let mut prefix_default_locale = false;
    for (key, value) in static_pairs_with_spreads(object, source, "i18n")? {
        match key.as_str() {
            "defaultLocale" => {
                default_locale = Some(literal_string(value, source, "defaultLocale")?)
            }
            "routing" => {
                let routing = value;
                if routing.kind() != "object" {
                    return Err("`i18n.routing` must be a static object".into());
                }
                for (routing_key, routing_value) in
                    static_pairs_with_spreads(routing, source, "i18n.routing")?
                {
                    if routing_key == "prefixDefaultLocale" {
                        let text = node_text(routing_value, source, "prefixDefaultLocale")?;
                        prefix_default_locale = match text {
                            "true" => true,
                            "false" => false,
                            _ => {
                                return Err(
                                    "`prefixDefaultLocale` must be a boolean literal".into()
                                );
                            }
                        };
                    }
                }
            }
            _ => {}
        }
    }
    Ok((default_locale, prefix_default_locale))
}

fn static_pairs_with_spreads<'tree>(
    object: Node<'tree>,
    source: &str,
    context: &str,
) -> Result<Vec<(String, Node<'tree>)>, String> {
    let mut pairs = Vec::new();
    for item in named_children(object) {
        if is_comment(item) || item.kind() == "spread_element" {
            continue;
        }
        if item.kind() != "pair" {
            return Err(format!(
                "{context} contains unsupported member `{}`",
                item.kind()
            ));
        }
        let key = item
            .child_by_field_name("key")
            .ok_or_else(|| format!("{context} member has no key"))?;
        let value = item
            .child_by_field_name("value")
            .ok_or_else(|| format!("{context} member has no value"))?;
        pairs.push((property_name(key, source, context)?, value));
    }
    Ok(pairs)
}

pub fn parse_src_dir(source: &str) -> Result<String, String> {
    parse_project_config(source).map(|config| config.src_dir)
}

fn parse_tree(source: &str) -> Result<Tree, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|error| format!("failed to load TypeScript parser: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "TypeScript parser returned no syntax tree".to_owned())?;
    if tree.root_node().has_error() {
        return Err("invalid TypeScript syntax".into());
    }
    validate_ast_bounds(tree.root_node())?;
    Ok(tree)
}

fn validate_ast_bounds(root: Node<'_>) -> Result<(), String> {
    let mut pending = vec![(root, 0_usize)];
    let mut node_count = 0_usize;

    while let Some((node, depth)) = pending.pop() {
        node_count += 1;
        if node_count > MAX_AST_NODES {
            return Err(format!(
                "TypeScript AST exceeds supported node count of {MAX_AST_NODES}"
            ));
        }
        if depth > MAX_AST_DEPTH {
            return Err(format!(
                "TypeScript AST exceeds supported depth of {MAX_AST_DEPTH}"
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

fn collect_imports(root: Node<'_>, source: &str) -> Result<ImportSet, String> {
    let mut imports = ImportSet::default();

    for statement in named_children(root) {
        if statement.kind() != "import_statement" {
            continue;
        }
        let source_node = statement
            .child_by_field_name("source")
            .ok_or_else(|| "side-effect imports are unsupported".to_owned())?;
        let module = literal_string(source_node, source, "import source")?;
        let clause = named_children(statement)
            .into_iter()
            .find(|child| child.kind() == "import_clause")
            .ok_or_else(|| "side-effect imports are unsupported".to_owned())?;
        let statement_is_type_only = has_direct_token(statement, "type");
        let defines_content_names = module == "astro:content";
        let defines_loader_names = module == "astro/loaders";

        for item in named_children(clause) {
            match item.kind() {
                "named_imports" => {
                    for specifier in named_children(item) {
                        if is_comment(specifier) {
                            continue;
                        }
                        if specifier.kind() != "import_specifier" {
                            return Err(format!(
                                "unsupported import clause `{}`",
                                specifier.kind()
                            ));
                        }

                        let imported_node = specifier
                            .child_by_field_name("name")
                            .ok_or_else(|| "import specifier has no name".to_owned())?;
                        let imported = node_text(imported_node, source, "import name")?.to_owned();
                        let local_node = specifier
                            .child_by_field_name("alias")
                            .unwrap_or(imported_node);
                        if local_node.kind() != "identifier" {
                            return Err("import aliases must be identifiers".into());
                        }
                        let local = node_text(local_node, source, "import alias")?.to_owned();
                        let type_only =
                            statement_is_type_only || has_direct_token(specifier, "type");
                        insert_import_binding(&mut imports, &local)?;

                        if !type_only && defines_content_names && imported == "defineCollection" {
                            imports.define_collection_aliases.push(local.clone());
                        }
                        if !type_only && defines_loader_names && imported == "glob" {
                            imports.glob_aliases.push(local.clone());
                        }
                        if !type_only && defines_loader_names && imported == "file" {
                            imports.file_aliases.push(local.clone());
                        }
                        if !type_only && module == "astro/config" && imported == "defineConfig" {
                            imports.define_config_aliases.push(local.clone());
                        }
                        if !type_only && module == "@astrojs/mdx" && imported == "default" {
                            imports.mdx_default_aliases.push(local.clone());
                        }
                    }
                }
                "identifier" => {
                    if defines_content_names || defines_loader_names {
                        return Err(format!("default imports from `{module}` are unsupported"));
                    }
                    let local = node_text(item, source, "default import")?.to_owned();
                    insert_import_binding(&mut imports, &local)?;
                    if !statement_is_type_only && module == "@astrojs/mdx" {
                        imports.mdx_default_aliases.push(local);
                    }
                }
                "namespace_import" => {
                    if defines_content_names || defines_loader_names {
                        return Err(format!("namespace imports from `{module}` are unsupported"));
                    }
                    let identifier = named_children(item)
                        .into_iter()
                        .find(|child| child.kind() == "identifier")
                        .ok_or_else(|| "namespace import has no identifier".to_owned())?;
                    let local = node_text(identifier, source, "namespace import")?.to_owned();
                    insert_import_binding(&mut imports, &local)?;
                }
                kind => return Err(format!("unsupported import clause `{kind}`")),
            }
        }
    }

    Ok(imports)
}

fn insert_import_binding(imports: &mut ImportSet, local: &str) -> Result<(), String> {
    if !imports.bindings.insert(local.to_owned()) {
        return Err(format!("duplicate or shadowed import binding `{local}`"));
    }
    Ok(())
}

fn required_alias<'a>(aliases: &'a [String], description: &str) -> Result<&'a str, String> {
    match aliases {
        [alias] => Ok(alias),
        [] => Err(format!("missing verified import of {description}")),
        _ => Err(format!(
            "multiple verified imports of {description} are unsupported"
        )),
    }
}

fn optional_alias<'a>(aliases: &'a [String], description: &str) -> Result<Option<&'a str>, String> {
    match aliases {
        [] => Ok(None),
        [alias] => Ok(Some(alias)),
        _ => Err(format!(
            "multiple verified imports of {description} are unsupported"
        )),
    }
}

fn reject_shadowing(
    imports: &ImportSet,
    declarations: &HashMap<String, Node<'_>>,
    name: &str,
) -> Result<(), String> {
    if imports.bindings.contains(name) || declarations.contains_key(name) {
        return Err(format!(
            "`{name}` shadows or redeclares an existing binding"
        ));
    }
    Ok(())
}

fn const_declarators<'tree>(
    declaration: Node<'tree>,
    source: &str,
) -> Result<Vec<(String, Node<'tree>)>, String> {
    let kind = declaration
        .child_by_field_name("kind")
        .ok_or_else(|| "declaration has no kind".to_owned())?;
    if node_text(kind, source, "declaration kind")? != "const" {
        return Err("only top-level `const` declarations are supported".into());
    }

    let mut declarations = Vec::new();
    for declarator in named_children(declaration) {
        if is_comment(declarator) {
            continue;
        }
        if declarator.kind() != "variable_declarator" {
            return Err(format!(
                "unsupported declaration node `{}`",
                declarator.kind()
            ));
        }
        if declarator.child_by_field_name("type").is_some() {
            return Err("typed collection declarations are unsupported".into());
        }
        let name_node = declarator
            .child_by_field_name("name")
            .ok_or_else(|| "const declaration has no name".to_owned())?;
        if name_node.kind() != "identifier" {
            return Err("destructured const declarations are unsupported".into());
        }
        let name = node_text(name_node, source, "declaration name")?.to_owned();
        let value = declarator
            .child_by_field_name("value")
            .ok_or_else(|| format!("const `{name}` has no initializer"))?;
        declarations.push((name, value));
    }

    if declarations.is_empty() {
        return Err("empty const declaration".into());
    }
    Ok(declarations)
}

fn exported_collections_object<'tree>(
    statement: Node<'tree>,
    source: &str,
) -> Result<Node<'tree>, String> {
    let declaration = statement
        .child_by_field_name("declaration")
        .ok_or_else(|| "only `export const collections = {...}` is supported".to_owned())?;
    if declaration.kind() != "lexical_declaration" {
        return Err("collection export must be a const declaration".into());
    }
    let declarators = const_declarators(declaration, source)?;
    let [(name, value)] = declarators.as_slice() else {
        return Err("collection export must declare exactly `collections`".into());
    };
    if name != "collections" {
        return Err(format!(
            "unsupported export `{name}`; expected `collections`"
        ));
    }
    if value.kind() != "object" {
        return Err("exported `collections` must be a literal object".into());
    }
    Ok(*value)
}

fn parse_collection_call(
    node: Node<'_>,
    source: &str,
    define_collection_alias: &str,
    glob_alias: Option<&str>,
    file_alias: Option<&str>,
    known_bindings: &HashSet<String>,
) -> Result<CollectionConfig, String> {
    let config_object = direct_call_object(
        node,
        source,
        define_collection_alias,
        "defineCollection call",
    )?;
    let pairs = strict_object_pairs(config_object, source, "defineCollection options")?;
    let mut loader = None;
    let mut schema = None;

    for (key, value) in pairs {
        match key.as_str() {
            "loader" => loader = Some(value),
            "schema" => schema = Some(value),
            _ => {
                return Err(format!("unsupported defineCollection option `{key}`"));
            }
        }
    }

    let loader = loader.ok_or_else(|| "defineCollection is missing `loader`".to_owned())?;
    if let Some(schema) = schema {
        validate_schema(schema, source, known_bindings)?;
    }

    let target = direct_identifier_call_target(loader, source)?.ok_or_else(|| {
        "defineCollection loader must be a direct `glob(...)` or `file(...)` call".to_owned()
    })?;
    if Some(target) == glob_alias {
        return parse_glob_loader(loader, source, target);
    }
    if Some(target) == file_alias {
        return parse_file_loader(loader, source, target);
    }
    Err("defineCollection loader must use the verified `glob` or `file` import".into())
}

fn parse_glob_loader(
    loader: Node<'_>,
    source: &str,
    glob_alias: &str,
) -> Result<CollectionConfig, String> {
    let glob_object = direct_call_object(loader, source, glob_alias, "glob loader call")?;
    let glob_pairs = strict_object_pairs(glob_object, source, "glob loader options")?;
    let mut base = None;
    let mut pattern = None;

    for (key, value) in glob_pairs {
        match key.as_str() {
            "base" => base = Some(value),
            "pattern" => pattern = Some(value),
            _ => return Err(format!("unsupported glob loader option `{key}`")),
        }
    }

    let base = literal_string(
        base.ok_or_else(|| "glob loader is missing `base`".to_owned())?,
        source,
        "glob base",
    )?;
    validate_project_relative_path(&base, "glob base")?;
    let pattern = literal_pattern(
        pattern.ok_or_else(|| "glob loader is missing `pattern`".to_owned())?,
        source,
        "glob pattern",
    )?;

    Ok(CollectionConfig {
        collection: String::new(),
        base: Some(base),
        pattern: Some(pattern),
        file: None,
    })
}

fn parse_file_loader(
    loader: Node<'_>,
    source: &str,
    file_alias: &str,
) -> Result<CollectionConfig, String> {
    let path = direct_call_string_argument(loader, source, file_alias, "file loader call")?;
    validate_project_relative_path(&path, "file loader path")?;
    if !path.to_ascii_lowercase().ends_with(".json") {
        return Err(format!(
            "file loader path `{path}` must reference a JSON (`.json`) file"
        ));
    }
    Ok(CollectionConfig {
        collection: String::new(),
        base: None,
        pattern: None,
        file: Some(path),
    })
}

/// The verified loader aliases and bindings one exported collection may use.
struct ExportContext<'a> {
    source: &'a str,
    define_collection_alias: &'a str,
    glob_alias: Option<&'a str>,
    file_alias: Option<&'a str>,
    known_bindings: &'a HashSet<String>,
}

fn parse_collections_export(
    object: Node<'_>,
    context: &ExportContext<'_>,
    declarations: &HashMap<String, CollectionConfig>,
    declaration_positions: &HashMap<String, usize>,
) -> Result<Vec<CollectionConfig>, String> {
    let source = context.source;
    let mut collections = Vec::new();
    let mut names = HashSet::new();

    for item in named_children(object) {
        if is_comment(item) {
            continue;
        }
        let (name, value) = match item.kind() {
            "pair" => {
                let key = item
                    .child_by_field_name("key")
                    .ok_or_else(|| "collection entry has no key".to_owned())?;
                let value = item
                    .child_by_field_name("value")
                    .ok_or_else(|| "collection entry has no value".to_owned())?;
                (property_name(key, source, "collection name")?, value)
            }
            "shorthand_property_identifier" => {
                let name = node_text(item, source, "collection name")?.to_owned();
                ensure_declared_before_export(&name, object.start_byte(), declaration_positions)?;
                let value = declarations
                    .get(&name)
                    .map(|_| item)
                    .ok_or_else(|| format!("unknown collection reference `{name}`"))?;
                (name, value)
            }
            kind => {
                return Err(format!("unsupported exported collections member `{kind}`"));
            }
        };

        reject_proto_key(&name, "exported collection names")?;
        if name.is_empty() {
            return Err("collection names must not be empty".into());
        }
        if !names.insert(name.clone()) {
            return Err(format!("duplicate exported collection `{name}`"));
        }

        let mut config = match value.kind() {
            "shorthand_property_identifier" => declarations
                .get(&name)
                .cloned()
                .ok_or_else(|| format!("unknown collection reference `{name}`"))?,
            "identifier" => {
                let reference = node_text(value, source, "collection reference")?;
                ensure_declared_before_export(
                    reference,
                    object.start_byte(),
                    declaration_positions,
                )?;
                declarations
                    .get(reference)
                    .cloned()
                    .ok_or_else(|| format!("unknown collection reference `{reference}`"))?
            }
            "call_expression" => parse_collection_call(
                value,
                source,
                context.define_collection_alias,
                context.glob_alias,
                context.file_alias,
                context.known_bindings,
            )?,
            kind => {
                return Err(format!(
                    "collection value must be a defineCollection call or local const reference, found `{kind}`"
                ));
            }
        };
        config.collection = name;
        collections.push(config);
    }

    Ok(collections)
}

fn direct_call_object<'tree>(
    call: Node<'tree>,
    source: &str,
    expected_function: &str,
    context: &str,
) -> Result<Node<'tree>, String> {
    if call.kind() != "call_expression" {
        return Err(format!("{context} must be a direct call expression"));
    }
    let function = call
        .child_by_field_name("function")
        .ok_or_else(|| format!("{context} has no function"))?;
    if function.kind() != "identifier" {
        return Err(format!("{context} must use a verified identifier"));
    }
    let function_name = node_text(function, source, "call target")?;
    if function_name != expected_function {
        return Err(format!(
            "{context} must call the verified `{expected_function}` import"
        ));
    }
    let arguments = call
        .child_by_field_name("arguments")
        .ok_or_else(|| format!("{context} has no arguments"))?;
    if arguments.kind() != "arguments" {
        return Err(format!("{context} must use parenthesized arguments"));
    }
    let arguments: Vec<_> = named_children(arguments)
        .into_iter()
        .filter(|argument| !is_comment(*argument))
        .collect();
    let [argument] = arguments.as_slice() else {
        return Err(format!("{context} must have exactly one object argument"));
    };
    if argument.kind() != "object" {
        return Err(format!("{context} argument must be a literal object"));
    }
    Ok(*argument)
}

fn direct_call_string_argument(
    call: Node<'_>,
    source: &str,
    expected_function: &str,
    context: &str,
) -> Result<String, String> {
    if call.kind() != "call_expression" {
        return Err(format!("{context} must be a direct call expression"));
    }
    let function = call
        .child_by_field_name("function")
        .ok_or_else(|| format!("{context} has no function"))?;
    if function.kind() != "identifier" {
        return Err(format!("{context} must use a verified identifier"));
    }
    let function_name = node_text(function, source, "call target")?;
    if function_name != expected_function {
        return Err(format!(
            "{context} must call the verified `{expected_function}` import"
        ));
    }
    let arguments = call
        .child_by_field_name("arguments")
        .ok_or_else(|| format!("{context} has no arguments"))?;
    if arguments.kind() != "arguments" {
        return Err(format!("{context} must use parenthesized arguments"));
    }
    let arguments: Vec<_> = named_children(arguments)
        .into_iter()
        .filter(|argument| !is_comment(*argument))
        .collect();
    let [argument] = arguments.as_slice() else {
        return Err(format!(
            "{context} must have exactly one literal string argument"
        ));
    };
    literal_string(*argument, source, context)
}

fn strict_object_pairs<'tree>(
    object: Node<'tree>,
    source: &str,
    context: &str,
) -> Result<Vec<(String, Node<'tree>)>, String> {
    if object.kind() != "object" {
        return Err(format!("{context} must be a literal object"));
    }
    let mut pairs = Vec::new();
    let mut keys = HashSet::new();

    for item in named_children(object) {
        if is_comment(item) {
            continue;
        }
        if item.kind() != "pair" {
            return Err(format!(
                "{context} contains unsupported member `{}`",
                item.kind()
            ));
        }
        let key_node = item
            .child_by_field_name("key")
            .ok_or_else(|| format!("{context} member has no key"))?;
        let value = item
            .child_by_field_name("value")
            .ok_or_else(|| format!("{context} member has no value"))?;
        let key = property_name(key_node, source, context)?;
        if !keys.insert(key.clone()) {
            return Err(format!("duplicate `{key}` key in {context}"));
        }
        pairs.push((key, value));
    }
    Ok(pairs)
}

fn property_name(node: Node<'_>, source: &str, context: &str) -> Result<String, String> {
    let name = match node.kind() {
        "property_identifier" => Ok(node_text(node, source, context)?.to_owned()),
        "string" => literal_string(node, source, context),
        "computed_property_name" => Err(format!("computed keys are unsupported in {context}")),
        kind => Err(format!("unsupported {context} key `{kind}`")),
    }?;
    reject_proto_key(&name, context)?;
    Ok(name)
}

/// Reject prototype keys even inside settings whose values are not interpreted.
fn reject_nested_proto_keys(root: Node<'_>, source: &str) -> Result<(), String> {
    for node in descendants_including(root) {
        if node.kind() != "pair" {
            continue;
        }
        let Some(key) = node.child_by_field_name("key") else {
            continue;
        };
        let name = match key.kind() {
            "property_identifier" => node_text(key, source, "config key").ok().map(str::to_owned),
            "string" => literal_string(key, source, "config key").ok(),
            _ => None,
        };
        if name.as_deref() == Some("__proto__") {
            return Err("`__proto__` keys are unsupported in Astro config".into());
        }
    }
    Ok(())
}

fn reject_proto_key(name: &str, context: &str) -> Result<(), String> {
    if name == "__proto__" {
        return Err(format!("`__proto__` keys are unsupported in {context}"));
    }
    Ok(())
}

fn literal_pattern(
    node: Node<'_>,
    source: &str,
    context: &str,
) -> Result<CollectionPattern, String> {
    let entries = match node.kind() {
        "string" => vec![literal_string(node, source, context)?],
        "array" => {
            let mut entries = Vec::new();
            for item in named_children(node) {
                if is_comment(item) {
                    continue;
                }
                if item.kind() == "spread_element" {
                    return Err(format!("spreads are unsupported in {context}"));
                }
                entries.push(literal_string(item, source, context)?);
            }
            entries
        }
        kind => {
            return Err(format!(
                "{context} must be a literal string or array of strings, found `{kind}`"
            ));
        }
    };
    CollectionPattern::new(entries).map_err(|error| format!("{context}: {error}"))
}

fn literal_string(node: Node<'_>, source: &str, context: &str) -> Result<String, String> {
    if node.kind() != "string" {
        return Err(format!("{context} must be a literal string"));
    }
    let raw = node_text(node, source, context)?;
    let bytes = raw.as_bytes();
    if bytes.len() < 2
        || !matches!(bytes[0], b'\'' | b'"')
        || bytes.last().copied() != Some(bytes[0])
    {
        return Err(format!("{context} must use a quoted literal string"));
    }
    let value = &raw[1..raw.len() - 1];
    if value.contains('\\') {
        return Err(format!("escaped strings are unsupported for {context}"));
    }
    if value.contains(['\n', '\r']) {
        return Err(format!("multiline strings are unsupported for {context}"));
    }
    Ok(value.to_owned())
}

fn validate_project_relative_path(value: &str, context: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{context} must not be empty"));
    }
    let bytes = value.as_bytes();
    if value.starts_with(['/', '\\'])
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || value.starts_with("file:")
    {
        return Err(format!("{context} must be project-relative"));
    }
    if value.split(['/', '\\']).any(|component| component == "..") {
        return Err(format!("{context} must not contain parent traversal"));
    }
    Ok(())
}

fn validate_schema(
    schema: Node<'_>,
    source: &str,
    known_bindings: &HashSet<String>,
) -> Result<(), String> {
    for node in descendants_including(schema) {
        if node.kind() == "identifier" {
            let identifier = node_text(node, source, "schema identifier")?;
            if known_bindings.contains(identifier) {
                return Err(format!(
                    "schema must not reference known config binding `{identifier}`"
                ));
            }
        }
    }
    Ok(())
}

fn direct_identifier_call_target<'a>(
    node: Node<'_>,
    source: &'a str,
) -> Result<Option<&'a str>, String> {
    if node.kind() != "call_expression" {
        return Ok(None);
    }
    let Some(function) = node.child_by_field_name("function") else {
        return Ok(None);
    };
    if function.kind() != "identifier" {
        return Ok(None);
    }
    node_text(function, source, "call target").map(Some)
}

fn ensure_declared_before_export(
    name: &str,
    export_offset: usize,
    declaration_positions: &HashMap<String, usize>,
) -> Result<(), String> {
    let declaration_offset = declaration_positions
        .get(name)
        .copied()
        .ok_or_else(|| format!("unknown collection reference `{name}`"))?;
    if declaration_offset >= export_offset {
        return Err(format!(
            "collection `{name}` must be initialized before the collections export"
        ));
    }
    Ok(())
}

fn validate_integrations(
    value: Node<'_>,
    source: &str,
    mdx_default_aliases: &[String],
) -> Result<bool, String> {
    if value.kind() != "array" {
        return Err("`integrations` must be a literal array".into());
    }

    let mut found_mdx = false;
    for integration in named_children(value) {
        if is_comment(integration) {
            continue;
        }
        if integration.kind() == "spread_element" {
            return Err("spreads are unsupported in `integrations`".into());
        }
        // Non-MDX integrations do not change glob selection or entry IDs, so
        // their factories and options are intentionally not interpreted.
        if integration.kind() != "call_expression" {
            continue;
        }
        let Some(function) = integration.child_by_field_name("function") else {
            continue;
        };
        if function.kind() != "identifier" {
            continue;
        }
        let function_name = node_text(function, source, "integration call target")?;
        if !mdx_default_aliases
            .iter()
            .any(|alias| alias == function_name)
        {
            continue;
        }
        if integration.child_by_field_name("type_arguments").is_some()
            || has_direct_token(integration, "?.")
        {
            return Err(
                "generic or optional @astrojs/mdx integration calls are unsupported".into(),
            );
        }
        let arguments = integration
            .child_by_field_name("arguments")
            .ok_or_else(|| "integration call has no arguments".to_owned())?;
        let arguments: Vec<_> = named_children(arguments)
            .into_iter()
            .filter(|argument| !is_comment(*argument))
            .collect();
        if !arguments.is_empty() {
            return Err(
                "`@astrojs/mdx` integration options are unsupported; use the verified empty call `mdx()`"
                    .into(),
            );
        }
        if found_mdx {
            return Err("duplicate `@astrojs/mdx` integrations are unsupported".into());
        }
        found_mdx = true;
    }
    Ok(found_mdx)
}

fn reject_mutations(root: Node<'_>, source: &str) -> Result<(), String> {
    for node in descendants_including(root) {
        if matches!(
            node.kind(),
            "assignment_expression"
                | "augmented_assignment_expression"
                | "update_expression"
                | "delete_expression"
        ) {
            let text = node_text(node, source, "mutation expression")?;
            return Err(format!("mutating expressions are unsupported: `{text}`"));
        }
    }
    Ok(())
}

fn has_direct_token(node: Node<'_>, token_kind: &str) -> bool {
    (0..node.child_count()).any(|index| {
        node.child(index as u32)
            .is_some_and(|child| !child.is_named() && child.kind() == token_kind)
    })
}

fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    (0..node.named_child_count())
        .filter_map(|index| node.named_child(index as u32))
        .collect()
}

fn descendants_including(node: Node<'_>) -> Vec<Node<'_>> {
    let mut descendants = Vec::new();
    let mut pending = vec![node];
    while let Some(current) = pending.pop() {
        descendants.push(current);
        pending.extend(named_children(current));
    }
    descendants
}

fn node_text<'a>(node: Node<'_>, source: &'a str, context: &str) -> Result<&'a str, String> {
    node.utf8_text(source.as_bytes())
        .map_err(|error| format!("invalid UTF-8 in {context}: {error}"))
}

fn is_comment(node: Node<'_>) -> bool {
    node.kind() == "comment"
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
