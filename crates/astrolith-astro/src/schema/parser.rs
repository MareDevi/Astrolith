//! Normalizes Astro-generated JSON Schema into the GUI-independent model.
//!
//! The supported Astro releases emit two different shapes for the same Zod
//! schema: Astro 5 wraps a single named draft-07 definition behind a top-level
//! `$ref`, while Astro 7 emits draft 2020-12 with the object inline. This parser
//! accepts both and produces one dialect-independent tree. It never evaluates
//! project code and requires no filesystem access.

use serde_json::{Map, Value};

use crate::schema::model::{CollectionSchema, Field, FieldKind, SchemaDiagnostic, SchemaDialect};

/// Error raised when an entire schema cannot be interpreted.
#[derive(Debug)]
pub struct SchemaError(String);

impl SchemaError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemaError {}

/// Normalize a generated Astro collection schema.
///
/// `source` is the parsed content of an `.astro/collections/<name>.schema.json`
/// file. Unsupported individual fields become [`FieldKind::Unsupported`] with an
/// accompanying diagnostic so that other fields remain editable; only a
/// structurally invalid document is an error.
pub fn normalize_schema(source: &Value) -> Result<CollectionSchema, SchemaError> {
    let object = source
        .as_object()
        .ok_or_else(|| SchemaError::new("collection schema must be a JSON object"))?;
    let dialect = detect_dialect(object);
    let definitions = object.get("definitions").and_then(Value::as_object);
    let root = resolve_root(object, definitions)?;

    if let Some(kind) = root.get("type").and_then(Value::as_str) {
        if kind != "object" {
            return Err(SchemaError::new(format!(
                "collection schema must describe an object, found `{kind}`"
            )));
        }
    } else if !root.contains_key("properties") {
        return Err(SchemaError::new(
            "collection schema must describe an object with `properties`",
        ));
    }

    let mut diagnostics = Vec::new();
    let fields = collect_fields(root, definitions, "", &mut diagnostics);
    Ok(CollectionSchema {
        dialect,
        fields,
        diagnostics,
    })
}

fn detect_dialect(object: &Map<String, Value>) -> SchemaDialect {
    match object.get("$schema").and_then(Value::as_str) {
        Some(schema) if schema.contains("draft-07") => SchemaDialect::Draft7,
        Some(schema) if schema.contains("2020-12") => SchemaDialect::Draft202012,
        _ if object.contains_key("definitions") => SchemaDialect::Draft7,
        _ => SchemaDialect::Unknown,
    }
}

fn resolve_root<'a>(
    object: &'a Map<String, Value>,
    definitions: Option<&'a Map<String, Value>>,
) -> Result<&'a Map<String, Value>, SchemaError> {
    let Some(reference) = object.get("$ref").and_then(Value::as_str) else {
        return Ok(object);
    };
    let target = resolve_definition(reference, definitions)
        .ok_or_else(|| SchemaError::new(format!("unresolved schema reference `{reference}`")))?;
    target
        .as_object()
        .ok_or_else(|| SchemaError::new(format!("schema reference `{reference}` is not an object")))
}

/// Resolve a local `#/definitions/<name>` reference against the definition map.
fn resolve_definition<'a>(
    reference: &str,
    definitions: Option<&'a Map<String, Value>>,
) -> Option<&'a Value> {
    let name = reference.strip_prefix("#/definitions/")?;
    definitions?.get(&unescape_pointer(name))
}

fn unescape_pointer(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

fn collect_fields(
    object: &Map<String, Value>,
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> Vec<Field> {
    let Some(Value::Object(properties)) = object.get("properties") else {
        return Vec::new();
    };
    let required: Vec<&str> = object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    let mut fields = Vec::new();
    for (name, value) in properties {
        if name == "$schema" {
            continue;
        }
        let field_path = child_path(path, name);
        let default = value.get("default").cloned();
        let kind = normalize_kind(value, definitions, &field_path, diagnostics);
        fields.push(Field {
            name: name.clone(),
            required: required.contains(&name.as_str()),
            default,
            kind,
        });
    }
    // `serde_json` map ordering depends on whether the `preserve_order` feature
    // is unified in by another workspace crate, so sort for deterministic output.
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    fields
}

fn normalize_kind(
    value: &Value,
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> FieldKind {
    let Some(object) = value.as_object() else {
        return unsupported(path, "schema value is not an object", diagnostics);
    };

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return match resolve_definition(reference, definitions) {
            Some(target) => normalize_kind(target, definitions, path, diagnostics),
            None => unsupported(
                path,
                format!("unresolved schema reference `{reference}`"),
                diagnostics,
            ),
        };
    }

    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        return FieldKind::Enum {
            values: values.clone(),
        };
    }

    if let Some(constant) = object.get("const") {
        return FieldKind::Literal {
            value: constant.clone(),
        };
    }

    if let Some(branches) = object.get("anyOf").and_then(Value::as_array) {
        return normalize_combination(branches, definitions, path, diagnostics, "anyOf");
    }
    if let Some(branches) = object.get("oneOf").and_then(Value::as_array) {
        return normalize_combination(branches, definitions, path, diagnostics, "oneOf");
    }

    match object.get("type") {
        Some(Value::String(kind)) => kind_for_type(kind, object, definitions, path, diagnostics),
        Some(Value::Array(types)) => {
            normalize_type_array(types, object, definitions, path, diagnostics)
        }
        Some(_) => unsupported(path, "`type` must be a string or array", diagnostics),
        None => {
            if object.contains_key("properties") || object.contains_key("additionalProperties") {
                object_kind(object, definitions, path, diagnostics)
            } else {
                unsupported(path, "schema has no `type`", diagnostics)
            }
        }
    }
}

fn kind_for_type(
    kind: &str,
    object: &Map<String, Value>,
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> FieldKind {
    match kind {
        "string" => {
            let format = object.get("format").and_then(Value::as_str);
            if matches!(format, Some("date-time" | "date")) {
                FieldKind::Date
            } else {
                FieldKind::String {
                    format: format.map(str::to_owned),
                }
            }
        }
        "number" => FieldKind::Number,
        "integer" => FieldKind::Integer,
        "boolean" => FieldKind::Boolean,
        "object" => object_kind(object, definitions, path, diagnostics),
        "array" => array_kind(object, definitions, path, diagnostics),
        other => unsupported(
            path,
            format!("unsupported field type `{other}`"),
            diagnostics,
        ),
    }
}

fn object_kind(
    object: &Map<String, Value>,
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> FieldKind {
    if object.get("properties").is_some_and(Value::is_object) {
        return FieldKind::Object {
            fields: collect_fields(object, definitions, path, diagnostics),
        };
    }
    if let Some(value_schema) = object.get("additionalProperties")
        && value_schema.is_object()
    {
        return FieldKind::Record {
            value: Box::new(normalize_kind(value_schema, definitions, path, diagnostics)),
        };
    }
    unsupported(
        path,
        "open object without a value schema is not supported",
        diagnostics,
    )
}

fn array_kind(
    object: &Map<String, Value>,
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> FieldKind {
    match object.get("items") {
        Some(items) if items.is_object() => FieldKind::Array {
            items: Box::new(normalize_kind(items, definitions, path, diagnostics)),
        },
        Some(_) => unsupported(path, "tuple arrays are not supported", diagnostics),
        None => unsupported(path, "array items are unspecified", diagnostics),
    }
}

fn normalize_type_array(
    types: &[Value],
    object: &Map<String, Value>,
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> FieldKind {
    let mut names = Vec::with_capacity(types.len());
    for entry in types {
        match entry.as_str() {
            Some(name) => names.push(name),
            None => return unsupported(path, "`type` entries must be strings", diagnostics),
        }
    }

    if names.contains(&"null") {
        let mut non_null: Vec<&str> = names.iter().copied().filter(|&n| n != "null").collect();
        if non_null.len() == 1 {
            let single = non_null.pop().expect("one non-null type");
            let mut narrowed = object.clone();
            narrowed.insert("type".to_owned(), Value::String(single.to_owned()));
            return FieldKind::Nullable {
                inner: Box::new(kind_for_type(
                    single,
                    &narrowed,
                    definitions,
                    path,
                    diagnostics,
                )),
            };
        }
        return unsupported(
            path,
            format!(
                "nullable union of {} is not supported",
                non_null.join(" | ")
            ),
            diagnostics,
        );
    }

    unsupported(
        path,
        format!(
            "union of {} is not supported for editing",
            names.join(" | ")
        ),
        diagnostics,
    )
}

fn normalize_combination(
    branches: &[Value],
    definitions: Option<&Map<String, Value>>,
    path: &str,
    diagnostics: &mut Vec<SchemaDiagnostic>,
    keyword: &str,
) -> FieldKind {
    if let [single] = branches {
        return normalize_kind(single, definitions, path, diagnostics);
    }

    if branches.iter().all(is_date_branch) {
        return FieldKind::Date;
    }

    let non_null: Vec<&Value> = branches
        .iter()
        .filter(|branch| !is_null_branch(branch))
        .collect();
    if non_null.len() == 1 && non_null.len() != branches.len() {
        return FieldKind::Nullable {
            inner: Box::new(normalize_kind(non_null[0], definitions, path, diagnostics)),
        };
    }

    unsupported(
        path,
        format!("`{keyword}` union is not supported for editing"),
        diagnostics,
    )
}

fn is_date_branch(branch: &Value) -> bool {
    let Some(object) = branch.as_object() else {
        return false;
    };
    let kind = object.get("type").and_then(Value::as_str);
    let format = object.get("format").and_then(Value::as_str);
    matches!(
        (kind, format),
        (Some("string"), Some("date-time" | "date")) | (Some("integer"), Some("unix-time"))
    )
}

fn is_null_branch(branch: &Value) -> bool {
    match branch.get("type") {
        Some(Value::String(kind)) => kind == "null",
        Some(Value::Array(types)) => {
            types.len() == 1 && types.first().and_then(Value::as_str) == Some("null")
        }
        _ => false,
    }
}

fn unsupported(
    path: &str,
    message: impl Into<String>,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) -> FieldKind {
    let message = message.into();
    diagnostics.push(SchemaDiagnostic {
        path: display_path(path),
        message: message.clone(),
    });
    FieldKind::Unsupported { reason: message }
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
