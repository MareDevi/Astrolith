//! Astro helper resolution on top of the normalized collection schema.
//!
//! Astro content configs describe some fields with helpers that the generated
//! JSON Schema cannot faithfully represent:
//!
//! - `image()` emits `z.string().transform(...)`, so the JSON Schema is a plain
//!   string. Without this pass a form would treat an image as free text.
//! - `reference("collection")` emits a heterogeneous union of a string and two
//!   objects, so the JSON Schema alone normalizes to
//!   [`FieldKind::Unsupported`].
//!
//! This module parses the *content config source* with the vendored
//! tree-sitter TypeScript grammar and refines matching fields into
//! [`FieldKind::Image`] and [`FieldKind::Reference`]. It never executes project
//! code and never touches the filesystem. Helper-parse failures are returned as
//! non-fatal [`SchemaDiagnostic`]s; a field is never silently coerced to a
//! string.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use tree_sitter::{Node, Parser};

use crate::schema::model::{CollectionSchema, Field, FieldKind, SchemaDiagnostic};

/// Bound on the TypeScript AST so a crafted config cannot exhaust the stack.
const MAX_AST_NODES: usize = 100_000;
/// Bound on TypeScript AST depth.
const MAX_AST_DEPTH: usize = 128;

/// Zod chain methods that wrap the receiver without changing the schema shape
/// enough to lose the helper beneath them.
const TRANSPARENT_METHODS: &[&str] = &[
    "optional",
    "nullable",
    "default",
    "describe",
    "catch",
    "brand",
    "readonly",
    "nullish",
    "refine",
    "superRefine",
    "transform",
    "pipe",
];

/// Methods that keep an object schema's shape when applied at the top level.
const SHAPE_PRESERVING_METHODS: &[&str] =
    &["strict", "strip", "passthrough", "partial", "required"];

fn is_shape_preserving_method(method: &str) -> bool {
    TRANSPARENT_METHODS.contains(&method) || SHAPE_PRESERVING_METHODS.contains(&method)
}

/// A helper recovered from the content config for one field path.
#[derive(Clone, Debug, PartialEq, Eq)]
enum HelperKind {
    Image,
    Reference(Vec<String>),
}

impl HelperKind {
    fn to_field_kind(&self) -> FieldKind {
        match self {
            HelperKind::Image => FieldKind::Image,
            HelperKind::Reference(collections) => FieldKind::Reference {
                collections: collections.clone(),
            },
        }
    }
}

/// One `defineCollection` schema expression recovered from the config.
#[derive(Default)]
struct CollectedCollection {
    /// Top-level field names of the collection's outer `z.object({...})`.
    top_names: BTreeSet<String>,
    /// Field path (e.g. `cover`, `author.avatar`, `gallery[]`) to helper.
    helpers: BTreeMap<String, HelperKind>,
    /// Non-fatal problems found while parsing this expression.
    diagnostics: Vec<SchemaDiagnostic>,
}

/// Resolve `image()` and `reference()` helpers into the normalized schema.
///
/// Mutates `schema` so image fields become [`FieldKind::Image`] and reference
/// fields become [`FieldKind::Reference`]. Stale diagnostics that described the
/// now-resolved field (for example the union warning a bare `reference()`
/// produced) are removed. Newly discovered problems are appended to
/// `schema.diagnostics` and returned so callers can surface them too.
///
/// The returned diagnostics are sorted and de-duplicated by `(path, message)`.
/// A config that cannot be parsed leaves `schema` untouched and returns one
/// root diagnostic.
pub fn enhance_astro_fields(
    schema: &mut CollectionSchema,
    config_source: &str,
) -> Vec<SchemaDiagnostic> {
    let mut diagnostics = apply_helpers(schema, config_source);
    diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.message.cmp(&right.message))
    });
    diagnostics.dedup();
    for diagnostic in &diagnostics {
        if !schema.diagnostics.contains(diagnostic) {
            schema.diagnostics.push(diagnostic.clone());
        }
    }
    schema.diagnostics.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.message.cmp(&right.message))
    });
    diagnostics
}

fn apply_helpers(schema: &mut CollectionSchema, config_source: &str) -> Vec<SchemaDiagnostic> {
    let collections = match parse_content_config_schemas(config_source) {
        Ok(collections) => collections,
        Err(message) => {
            return vec![SchemaDiagnostic {
                path: "<root>".to_owned(),
                message,
            }];
        }
    };

    // Always surface parse diagnostics from every collection, even ones we do
    // not apply, so a malformed sibling schema does not disappear silently.
    let mut diagnostics: Vec<SchemaDiagnostic> = collections
        .iter()
        .flat_map(|collection| collection.diagnostics.iter().cloned())
        .collect();

    if collections.is_empty() && !schema.fields.is_empty() {
        diagnostics.push(SchemaDiagnostic {
            path: "<root>".to_owned(),
            message:
                "content config declares no inline collection schema; Astro helpers could not be resolved"
                    .to_owned(),
        });
        return diagnostics;
    }

    let top_level: BTreeSet<String> = schema
        .fields
        .iter()
        .map(|field| field.name.clone())
        .collect();
    let selected = select_collections(&collections, &top_level);

    // Merge helper assignments; conflicting helpers at one path are ambiguous
    // and left unrefined rather than guessed.
    let mut merged: BTreeMap<String, HelperKind> = BTreeMap::new();
    let mut conflicted: BTreeSet<String> = BTreeSet::new();
    for collection in selected {
        for (path, helper) in &collection.helpers {
            match merged.get(path) {
                None => {
                    merged.insert(path.clone(), helper.clone());
                }
                Some(existing) if existing == helper => {}
                Some(_) => {
                    merged.remove(path);
                    if conflicted.insert(path.clone()) {
                        diagnostics.push(SchemaDiagnostic {
                            path: display_path(path),
                            message: "Astro helper for this field is ambiguous across collections"
                                .to_owned(),
                        });
                    }
                }
            }
        }
    }

    let mut applied: BTreeSet<String> = BTreeSet::new();
    schema.fields = std::mem::take(&mut schema.fields)
        .into_iter()
        .map(|field| refine_field(field, "", &merged, &mut applied))
        .collect();

    // Drop diagnostics that described a field we have now resolved.
    schema
        .diagnostics
        .retain(|diagnostic| !applied.contains(&diagnostic.path));

    diagnostics
}

fn select_collections<'a>(
    collections: &'a [CollectedCollection],
    top_level: &BTreeSet<String>,
) -> Vec<&'a CollectedCollection> {
    let matching: Vec<&CollectedCollection> = collections
        .iter()
        .filter(|collection| !collection.top_names.is_empty() && &collection.top_names == top_level)
        .collect();
    if matching.is_empty() {
        collections.iter().collect()
    } else {
        matching
    }
}

fn refine_field(
    field: Field,
    parent: &str,
    helpers: &BTreeMap<String, HelperKind>,
    applied: &mut BTreeSet<String>,
) -> Field {
    let path = child_path(parent, &field.name);
    Field {
        kind: refine_kind(field.kind, &path, helpers, applied),
        ..field
    }
}

fn refine_kind(
    kind: FieldKind,
    path: &str,
    helpers: &BTreeMap<String, HelperKind>,
    applied: &mut BTreeSet<String>,
) -> FieldKind {
    match kind {
        FieldKind::Nullable { inner } => FieldKind::Nullable {
            inner: Box::new(refine_kind(*inner, path, helpers, applied)),
        },
        FieldKind::Array { items } => FieldKind::Array {
            items: Box::new(refine_kind(*items, &format!("{path}[]"), helpers, applied)),
        },
        FieldKind::Object { fields } => {
            if let Some(helper) = helpers.get(path) {
                applied.insert(path.to_owned());
                return helper.to_field_kind();
            }
            FieldKind::Object {
                fields: fields
                    .into_iter()
                    .map(|field| refine_field(field, path, helpers, applied))
                    .collect(),
            }
        }
        FieldKind::Record { value } => FieldKind::Record {
            value: Box::new(refine_kind(*value, path, helpers, applied)),
        },
        other => match helpers.get(path) {
            Some(helper) => {
                applied.insert(path.to_owned());
                helper.to_field_kind()
            }
            None => other,
        },
    }
}

/// Parse the content config and recover every `defineCollection` schema.
fn parse_content_config_schemas(source: &str) -> Result<Vec<CollectedCollection>, String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|error| format!("failed to load TypeScript parser: {error}"))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| "TypeScript parser returned no syntax tree".to_owned())?;
    let root = tree.root_node();
    if root.has_error() {
        return Err("invalid TypeScript syntax in content config".to_owned());
    }
    validate_ast_bounds(root)?;

    let names = collect_import_names(root, source);
    let consts = collect_local_consts(root, source);

    let mut collections = Vec::new();
    for call in descendants_including(root) {
        if call.kind() != "call_expression" {
            continue;
        }
        let Some(function) = call.child_by_field_name("function") else {
            continue;
        };
        if function.kind() != "identifier" {
            continue;
        }
        let Ok(target) = node_text(function, source, "defineCollection call") else {
            continue;
        };
        if !names.define_collections.contains(target) {
            continue;
        }
        let Some(arguments) = call.child_by_field_name("arguments") else {
            continue;
        };
        let Some(config_object) = first_named_argument(arguments) else {
            continue;
        };
        if config_object.kind() != "object" {
            continue;
        }
        let Some(schema_pair) = find_object_pair(config_object, source, "schema") else {
            continue;
        };
        let Some(schema_value) = schema_pair.child_by_field_name("value") else {
            continue;
        };
        let mut collection = CollectedCollection::default();
        match resolve_schema_expression(schema_value, source, &consts, &mut 0) {
            Some(expression) => collect_collection(expression, source, &names, &mut collection),
            None => collection.diagnostics.push(SchemaDiagnostic {
                path: "<root>".to_owned(),
                message: "collection schema is not an inline `z.object(...)` expression; Astro helpers could not be resolved"
                    .to_owned(),
            }),
        }
        collections.push(collection);
    }
    Ok(collections)
}

fn collect_collection(
    expression: Node<'_>,
    source: &str,
    names: &ImportNames,
    collection: &mut CollectedCollection,
) {
    let Some(object) = object_literal_of_object_call(expression, source) else {
        collection.diagnostics.push(SchemaDiagnostic {
            path: "<root>".to_owned(),
            message: "collection schema is not a `z.object(...)` literal; Astro helpers could not be resolved"
                .to_owned(),
        });
        return;
    };
    for pair in object_pairs(object) {
        if let Some(key) = pair_key_text(pair, source) {
            collection.top_names.insert(key);
        }
    }
    walk_fields(object, "", source, names, collection);
}

fn walk_fields(
    object: Node<'_>,
    prefix: &str,
    source: &str,
    names: &ImportNames,
    collection: &mut CollectedCollection,
) {
    for pair in object_pairs(object) {
        let Some(key) = pair_key_text(pair, source) else {
            continue;
        };
        let Some(value) = pair.child_by_field_name("value") else {
            continue;
        };
        let path = child_path(prefix, &key);
        walk_expression(value, &path, source, names, collection);
    }
}

fn walk_expression(
    node: Node<'_>,
    path: &str,
    source: &str,
    names: &ImportNames,
    collection: &mut CollectedCollection,
) {
    let node = unwrap_parenthesized(node);
    match node.kind() {
        "call_expression" => walk_call(node, path, source, names, collection),
        "object" => walk_fields(node, path, source, names, collection),
        _ => {}
    }
}

fn walk_call(
    node: Node<'_>,
    path: &str,
    source: &str,
    names: &ImportNames,
    collection: &mut CollectedCollection,
) {
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let arguments = node.child_by_field_name("arguments");

    if function.kind() == "identifier" {
        let Ok(name) = node_text(function, source, "helper call") else {
            return;
        };
        if names.image.contains(name) {
            record_helper(collection, path, HelperKind::Image);
        } else if names.reference.contains(name) {
            record_reference(collection, path, arguments, source);
        }
        return;
    }

    if function.kind() != "member_expression" {
        return;
    }
    let property = function
        .child_by_field_name("property")
        .and_then(|property| node_text(property, source, "method name").ok());
    let receiver = function.child_by_field_name("object");

    match property {
        Some("object") => {
            let Some(argument) = arguments.and_then(first_named_argument) else {
                return;
            };
            if argument.kind() == "object" {
                walk_fields(argument, path, source, names, collection);
            }
        }
        Some("array") => {
            if let Some(argument) = arguments.and_then(first_named_argument) {
                walk_expression(argument, &format!("{path}[]"), source, names, collection);
            }
        }
        Some(method) if TRANSPARENT_METHODS.contains(&method) => {
            if let Some(receiver) = receiver {
                walk_expression(receiver, path, source, names, collection);
            }
        }
        _ => {}
    }
}

fn record_helper(collection: &mut CollectedCollection, path: &str, helper: HelperKind) {
    match collection.helpers.get(path) {
        None => {
            collection.helpers.insert(path.to_owned(), helper);
        }
        Some(existing) if *existing == helper => {}
        Some(_) => collection.diagnostics.push(SchemaDiagnostic {
            path: display_path(path),
            message: "conflicting Astro helpers declared for the same field".to_owned(),
        }),
    }
}

fn record_reference(
    collection: &mut CollectedCollection,
    path: &str,
    arguments: Option<Node<'_>>,
    source: &str,
) {
    let Some(arguments) = arguments else {
        collection.diagnostics.push(SchemaDiagnostic {
            path: display_path(path),
            message: "reference() requires a collection name".to_owned(),
        });
        return;
    };
    let arguments: Vec<Node<'_>> = named_children(arguments)
        .into_iter()
        .filter(|argument| !is_comment(*argument))
        .collect();
    if arguments.is_empty() {
        collection.diagnostics.push(SchemaDiagnostic {
            path: display_path(path),
            message: "reference() requires a collection name".to_owned(),
        });
        return;
    }

    let mut collections = Vec::new();
    for argument in arguments {
        if !collect_reference_names(argument, source, &mut collections) {
            collection.diagnostics.push(SchemaDiagnostic {
                path: display_path(path),
                message: "reference() collection name must be a string literal".to_owned(),
            });
            return;
        }
    }
    if collections.is_empty() {
        collection.diagnostics.push(SchemaDiagnostic {
            path: display_path(path),
            message: "reference() requires a collection name".to_owned(),
        });
        return;
    }
    let mut seen = HashSet::new();
    collections.retain(|name| seen.insert(name.clone()));
    record_helper(collection, path, HelperKind::Reference(collections));
}

/// Collect one reference argument, which may be a string or an array of strings.
fn collect_reference_names(node: Node<'_>, source: &str, out: &mut Vec<String>) -> bool {
    match node.kind() {
        "string" => match string_literal_value(node, source) {
            Some(value) => {
                out.push(value);
                true
            }
            None => false,
        },
        "array" => {
            let mut all_literals = true;
            for element in named_children(node) {
                if is_comment(element) {
                    continue;
                }
                if !collect_reference_names(element, source, out) {
                    all_literals = false;
                }
            }
            all_literals
        }
        _ => false,
    }
}

// -- Import and binding collection -----------------------------------------

#[derive(Default)]
struct ImportNames {
    define_collections: HashSet<String>,
    image: HashSet<String>,
    reference: HashSet<String>,
}

fn collect_import_names(root: Node<'_>, source: &str) -> ImportNames {
    let mut names = ImportNames {
        define_collections: HashSet::from(["defineCollection".to_owned()]),
        image: HashSet::from(["image".to_owned()]),
        reference: HashSet::from(["reference".to_owned()]),
    };

    for statement in named_children(root) {
        if statement.kind() != "import_statement" {
            continue;
        }
        let Some(module) = statement
            .child_by_field_name("source")
            .and_then(|source_node| string_literal_value(source_node, source))
        else {
            continue;
        };
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
            for specifier in named_children(item) {
                if specifier.kind() != "import_specifier" {
                    continue;
                }
                let Some(imported_node) = specifier.child_by_field_name("name") else {
                    continue;
                };
                let Ok(imported) = node_text(imported_node, source, "import name") else {
                    continue;
                };
                let local_node = specifier
                    .child_by_field_name("alias")
                    .unwrap_or(imported_node);
                let Ok(local) = node_text(local_node, source, "import alias") else {
                    continue;
                };
                if module == "astro:content" {
                    match imported {
                        "defineCollection" => {
                            names.define_collections.insert(local.to_owned());
                        }
                        "image" => {
                            names.image.insert(local.to_owned());
                        }
                        "reference" => {
                            names.reference.insert(local.to_owned());
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    names
}

fn collect_local_consts<'tree>(root: Node<'tree>, source: &str) -> HashMap<String, Node<'tree>> {
    let mut consts = HashMap::new();
    for statement in named_children(root) {
        if statement.kind() != "lexical_declaration" {
            continue;
        }
        for declarator in named_children(statement) {
            if declarator.kind() != "variable_declarator" {
                continue;
            }
            let Some(name) = declarator.child_by_field_name("name") else {
                continue;
            };
            let Some(value) = declarator.child_by_field_name("value") else {
                continue;
            };
            if name.kind() != "identifier" {
                continue;
            }
            if let Ok(name) = node_text(name, source, "binding name") {
                consts.insert(name.to_owned(), value);
            }
        }
    }
    consts
}

/// Follow identifiers, arrow/function bodies, parenthesized and `as` wrappers
/// down to the actual schema expression.
fn resolve_schema_expression<'tree>(
    node: Node<'tree>,
    source: &str,
    consts: &HashMap<String, Node<'tree>>,
    depth: &mut usize,
) -> Option<Node<'tree>> {
    if *depth > MAX_AST_DEPTH {
        return None;
    }
    *depth += 1;
    let node = unwrap_parenthesized(node);
    match node.kind() {
        "identifier" => {
            let name = node_text(node, source, "schema binding").ok()?;
            let target = *consts.get(name)?;
            resolve_schema_expression(target, source, consts, depth)
        }
        "arrow_function" | "function_expression" => {
            let body = node.child_by_field_name("body")?;
            if body.kind() == "statement_block" {
                let statement = descendants_including(body)
                    .into_iter()
                    .find(|child| child.kind() == "return_statement")?;
                let expression = named_children(statement)
                    .into_iter()
                    .find(|child| !is_comment(*child))?;
                resolve_schema_expression(expression, source, consts, depth)
            } else {
                resolve_schema_expression(body, source, consts, depth)
            }
        }
        "as_expression" | "satisfies_expression" => {
            let expression = node
                .child_by_field_name("expression")
                .or_else(|| named_children(node).into_iter().next())?;
            resolve_schema_expression(expression, source, consts, depth)
        }
        _ => Some(node),
    }
}

// -- Node helpers -----------------------------------------------------------

fn object_literal_of_object_call<'tree>(node: Node<'tree>, source: &str) -> Option<Node<'tree>> {
    if node.kind() != "call_expression" {
        return None;
    }
    let function = node.child_by_field_name("function")?;
    if function.kind() != "member_expression" {
        return None;
    }
    let property = function.child_by_field_name("property")?;
    let property_text = node_text(property, source, "method name").ok()?;
    if property_text != "object" {
        // Unwrap shape-preserving wrappers such as `.strict()` or `.optional()`
        // so the object literal beneath them is still found.
        if is_shape_preserving_method(property_text) {
            let receiver = function.child_by_field_name("object")?;
            return object_literal_of_object_call(receiver, source);
        }
        return None;
    }
    let arguments = node.child_by_field_name("arguments")?;
    let argument = first_named_argument(arguments)?;
    if argument.kind() == "object" {
        Some(argument)
    } else {
        None
    }
}

fn object_pairs(object: Node<'_>) -> Vec<Node<'_>> {
    named_children(object)
        .into_iter()
        .filter(|child| child.kind() == "pair")
        .collect()
}

fn find_object_pair<'tree>(object: Node<'tree>, source: &str, name: &str) -> Option<Node<'tree>> {
    object_pairs(object).into_iter().find(|pair| {
        pair_key_text(*pair, source)
            .map(|key| key == name)
            .unwrap_or(false)
    })
}

fn pair_key_text(pair: Node<'_>, source: &str) -> Option<String> {
    let key = pair.child_by_field_name("key")?;
    let text = node_text(key, source, "object key").ok()?;
    Some(strip_quotes(text).to_owned())
}

fn first_named_argument(arguments: Node<'_>) -> Option<Node<'_>> {
    named_children(arguments)
        .into_iter()
        .find(|argument| !is_comment(*argument))
}

fn unwrap_parenthesized(node: Node<'_>) -> Node<'_> {
    let mut current = node;
    while current.kind() == "parenthesized_expression" {
        match named_children(current).into_iter().next() {
            Some(inner) => current = inner,
            None => break,
        }
    }
    current
}

fn string_literal_value(node: Node<'_>, source: &str) -> Option<String> {
    let raw = node_text(node, source, "string literal").ok()?;
    match node.kind() {
        "string" | "template_string" => {}
        _ => return None,
    }
    let quote = raw.chars().next()?;
    if !matches!(quote, '"' | '\'' | '`') {
        return None;
    }
    let inner = raw.get(1..raw.len().checked_sub(1)?)?;
    if inner.contains('\\') || (node.kind() == "template_string" && inner.contains("${")) {
        return None;
    }
    Some(inner.to_owned())
}

fn strip_quotes(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if matches!(first, b'"' | b'\'' | b'`') && first == last {
            return &text[1..text.len() - 1];
        }
    }
    text
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
        for child in named_children(node) {
            pending.push((child, depth + 1));
        }
    }
    Ok(())
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

fn display_path(path: &str) -> String {
    if path.is_empty() {
        "<root>".to_owned()
    } else {
        path.to_owned()
    }
}

fn child_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}.{name}")
    }
}
