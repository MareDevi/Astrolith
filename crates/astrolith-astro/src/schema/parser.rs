//! Normalizes Astro-generated JSON Schema into the GUI-independent model.
//!
//! The supported Astro releases emit two different shapes for the same Zod
//! schema: Astro 5 wraps a single named draft-07 definition behind a top-level
//! `$ref`, while Astro 7 emits draft 2020-12 with the object inline. This parser
//! accepts both and produces one dialect-independent tree. It never evaluates
//! project code and requires no filesystem access.

use serde_json::{Map, Value};

use crate::schema::model::{CollectionSchema, Field, FieldKind, SchemaDiagnostic, SchemaDialect};

/// Bound on schema nesting so a crafted document cannot exhaust the stack.
const MAX_SCHEMA_DEPTH: usize = 64;

/// Error raised when an entire schema cannot be interpreted.
#[derive(Debug)]
pub struct SchemaError(String);

impl SchemaError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
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
    let definitions = object
        .get("definitions")
        .and_then(Value::as_object)
        .or_else(|| object.get("$defs").and_then(Value::as_object));
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

    let mut normalizer = Normalizer::new(definitions);
    let fields = normalizer.collect_fields(root, "", 0);
    normalizer.sort_diagnostics();
    Ok(CollectionSchema {
        dialect,
        fields,
        diagnostics: normalizer.diagnostics,
    })
}

fn detect_dialect(object: &Map<String, Value>) -> SchemaDialect {
    match object.get("$schema").and_then(Value::as_str) {
        Some(schema) if schema.contains("draft-07") => SchemaDialect::Draft7,
        Some(schema) if schema.contains("2020-12") => SchemaDialect::Draft202012,
        _ if object.contains_key("$defs") => SchemaDialect::Draft202012,
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

/// Resolve a local `#/definitions/<name>` or `#/$defs/<name>` reference.
fn resolve_definition<'a>(
    reference: &str,
    definitions: Option<&'a Map<String, Value>>,
) -> Option<&'a Value> {
    let name = reference
        .strip_prefix("#/definitions/")
        .or_else(|| reference.strip_prefix("#/$defs/"))?;
    definitions?.get(&unescape_pointer(name))
}

fn unescape_pointer(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Mutable traversal state: definitions, accumulated diagnostics, and the
/// reference stack used to detect cycles.
struct Normalizer<'a> {
    definitions: Option<&'a Map<String, Value>>,
    diagnostics: Vec<SchemaDiagnostic>,
    active_refs: Vec<String>,
}

impl<'a> Normalizer<'a> {
    fn new(definitions: Option<&'a Map<String, Value>>) -> Self {
        Self {
            definitions,
            diagnostics: Vec::new(),
            active_refs: Vec::new(),
        }
    }

    fn sort_diagnostics(&mut self) {
        self.diagnostics.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.message.cmp(&right.message))
        });
    }

    fn diagnostic(&mut self, path: &str, message: impl Into<String>) {
        self.diagnostics.push(SchemaDiagnostic {
            path: display_path(path),
            message: message.into(),
        });
    }

    fn unsupported(&mut self, path: &str, message: impl Into<String>) -> FieldKind {
        let message = message.into();
        self.diagnostic(path, message.clone());
        FieldKind::Unsupported { reason: message }
    }

    fn collect_fields(
        &mut self,
        object: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> Vec<Field> {
        if depth > MAX_SCHEMA_DEPTH {
            self.diagnostic(path, depth_message());
            return Vec::new();
        }
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
            let kind = self.kind(value, &field_path, depth + 1);
            fields.push(Field {
                name: name.clone(),
                required: required.contains(&name.as_str()),
                default,
                kind,
            });
        }
        // `serde_json` map ordering depends on whether the `preserve_order`
        // feature is unified in by another workspace crate, so sort.
        fields.sort_by(|left, right| left.name.cmp(&right.name));
        fields
    }

    fn kind(&mut self, value: &Value, path: &str, depth: usize) -> FieldKind {
        if depth > MAX_SCHEMA_DEPTH {
            return self.unsupported(path, depth_message());
        }
        let Some(object) = value.as_object() else {
            return self.unsupported(path, "schema value is not an object");
        };

        if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
            if self.active_refs.iter().any(|active| active == reference) {
                return self.unsupported(path, format!("cyclic schema reference `{reference}`"));
            }
            let Some(target) = resolve_definition(reference, self.definitions) else {
                return self
                    .unsupported(path, format!("unresolved schema reference `{reference}`"));
            };
            self.active_refs.push(reference.to_owned());
            let result = self.kind(target, path, depth + 1);
            self.active_refs.pop();
            return result;
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
            return self.combination(branches, path, depth, "anyOf");
        }
        if let Some(branches) = object.get("oneOf").and_then(Value::as_array) {
            return self.combination(branches, path, depth, "oneOf");
        }

        match object.get("type") {
            Some(Value::String(kind)) => self.kind_for_type(kind, object, path, depth),
            Some(Value::Array(types)) => self.type_array(types, object, path, depth),
            Some(_) => self.unsupported(path, "`type` must be a string or array"),
            None => {
                if object.contains_key("properties") || object.contains_key("additionalProperties")
                {
                    self.object_kind(object, path, depth)
                } else {
                    self.unsupported(path, "schema has no `type`")
                }
            }
        }
    }

    fn kind_for_type(
        &mut self,
        kind: &str,
        object: &Map<String, Value>,
        path: &str,
        depth: usize,
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
            "integer" if object.get("format").and_then(Value::as_str) == Some("unix-time") => {
                FieldKind::Date
            }
            "number" => FieldKind::Number,
            "integer" => FieldKind::Integer,
            "boolean" => FieldKind::Boolean,
            "object" => self.object_kind(object, path, depth),
            "array" => self.array_kind(object, path, depth),
            "null" => self.unsupported(path, "null-only fields are not supported"),
            other => self.unsupported(path, format!("unsupported field type `{other}`")),
        }
    }

    fn object_kind(&mut self, object: &Map<String, Value>, path: &str, depth: usize) -> FieldKind {
        if object.get("properties").is_some_and(Value::is_object) {
            return FieldKind::Object {
                fields: self.collect_fields(object, path, depth + 1),
            };
        }
        if let Some(value_schema) = object.get("additionalProperties")
            && value_schema.is_object()
        {
            return FieldKind::Record {
                value: Box::new(self.kind(value_schema, path, depth + 1)),
            };
        }
        self.unsupported(path, "open object without a value schema is not supported")
    }

    fn array_kind(&mut self, object: &Map<String, Value>, path: &str, depth: usize) -> FieldKind {
        match object.get("items") {
            Some(items) if items.is_object() => {
                let item_path = format!("{path}[]");
                FieldKind::Array {
                    items: Box::new(self.kind(items, &item_path, depth + 1)),
                }
            }
            Some(Value::Array(_)) => self.unsupported(path, "tuple arrays are not supported"),
            Some(_) => self.unsupported(path, "array `items` must be a single schema"),
            None => self.unsupported(path, "array items are unspecified"),
        }
    }

    fn type_array(
        &mut self,
        types: &[Value],
        object: &Map<String, Value>,
        path: &str,
        depth: usize,
    ) -> FieldKind {
        let mut names = Vec::with_capacity(types.len());
        for entry in types {
            match entry.as_str() {
                Some(name) => names.push(name),
                None => return self.unsupported(path, "`type` entries must be strings"),
            }
        }

        if names == ["null"] {
            return self.unsupported(path, "null-only fields are not supported");
        }

        if names.contains(&"null") {
            let non_null: Vec<&str> = names.iter().copied().filter(|&n| n != "null").collect();
            if non_null.len() == 1 {
                let single = non_null[0];
                let mut narrowed = object.clone();
                narrowed.insert("type".to_owned(), Value::String(single.to_owned()));
                return FieldKind::Nullable {
                    inner: Box::new(self.kind_for_type(single, &narrowed, path, depth + 1)),
                };
            }
            return self.unsupported(
                path,
                format!(
                    "nullable union of {} is not supported",
                    non_null.join(" | ")
                ),
            );
        }

        self.unsupported(
            path,
            format!(
                "union of {} is not supported for editing",
                names.join(" | ")
            ),
        )
    }

    fn combination(
        &mut self,
        branches: &[Value],
        path: &str,
        depth: usize,
        keyword: &str,
    ) -> FieldKind {
        if let [single] = branches {
            return self.kind(single, path, depth + 1);
        }
        if branches.is_empty() {
            return self.unsupported(path, format!("`{keyword}` must contain a schema"));
        }

        let non_null: Vec<&Value> = branches
            .iter()
            .filter(|branch| !is_null_branch(branch))
            .collect();
        if !non_null.is_empty() && non_null.iter().all(|branch| is_date_branch(branch)) {
            let date = FieldKind::Date;
            return if non_null.len() != branches.len() {
                FieldKind::Nullable {
                    inner: Box::new(date),
                }
            } else {
                date
            };
        }
        if non_null.len() == 1 && non_null.len() != branches.len() {
            return FieldKind::Nullable {
                inner: Box::new(self.kind(non_null[0], path, depth + 1)),
            };
        }

        self.unsupported(
            path,
            format!("`{keyword}` union is not supported for editing"),
        )
    }
}

fn depth_message() -> String {
    format!("schema nesting exceeds the supported depth of {MAX_SCHEMA_DEPTH}")
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
