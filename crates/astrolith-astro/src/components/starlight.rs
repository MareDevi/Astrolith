//! Versioned built-in Starlight catalog and package declaration detection.

use serde_json::Value;

use super::{Component, ComponentLocation, Framework, Prop, PropDefault, PropKind, Slot};

/// The catalog version frozen into this build.
pub const STARLIGHT_CATALOG_VERSION: &str = "0.42";
/// The package that declares the catalog.
const STARLIGHT_PACKAGE: &str = "@astrojs/starlight";
/// The package that globally provides MDX components.
const AUTO_IMPORT_PACKAGE: &str = "astro-auto-import";
/// Frozen assets, versioned by filename.
const STARLIGHT_CATALOG_JSON: &str = include_str!("../../assets/starlight-0.42.json");

/// Relevant package.json declaration state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PackageDeclaration {
    pub(crate) starlight: bool,
    pub(crate) auto_import: bool,
}

/// Read the declarations that change component indexing from package.json.
pub(crate) fn package_declaration(source: &str) -> Result<PackageDeclaration, String> {
    let package: Value =
        serde_json::from_str(source).map_err(|error| format!("Invalid package.json: {error}"))?;
    let package = package
        .as_object()
        .ok_or_else(|| "package.json must be an object".to_owned())?;
    let mut declaration = PackageDeclaration::default();
    for section in ["dependencies", "devDependencies"] {
        let Some(dependencies) = package.get(section) else {
            continue;
        };
        let dependencies = dependencies
            .as_object()
            .ok_or_else(|| format!("{section} must be an object"))?;
        declaration.starlight |= dependencies.contains_key(STARLIGHT_PACKAGE);
        declaration.auto_import |= dependencies.contains_key(AUTO_IMPORT_PACKAGE);
    }
    Ok(declaration)
}

/// Load the frozen Starlight catalog.
pub fn starlight_catalog() -> Result<Vec<Component>, String> {
    let value: Value = serde_json::from_str(STARLIGHT_CATALOG_JSON)
        .map_err(|error| format!("Invalid embedded catalog: {error}"))?;
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| "Embedded catalog has no version".to_owned())?;
    if version != STARLIGHT_CATALOG_VERSION {
        return Err(format!(
            "Embedded catalog version {version} does not match {STARLIGHT_CATALOG_VERSION}"
        ));
    }
    let package = value
        .get("package")
        .and_then(Value::as_str)
        .ok_or_else(|| "Embedded catalog has no package".to_owned())?;
    if package != STARLIGHT_PACKAGE {
        return Err(format!("Embedded catalog package is {package}"));
    }
    let specifier = value
        .get("specifier")
        .and_then(Value::as_str)
        .ok_or_else(|| "Embedded catalog has no import specifier".to_owned())?;
    let components = value
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| "Embedded catalog has no components".to_owned())?;

    let mut catalog = Vec::with_capacity(components.len());
    for component in components {
        let name = component
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "Catalog component has no name".to_owned())?;
        let description = component
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let slots = component
            .get("slots")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|slot| {
                let name = slot.as_str().unwrap_or_default();
                Slot {
                    name: (name != "default").then(|| name.to_owned()),
                }
            })
            .collect();
        let props = component
            .get("props")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(catalog_prop)
            .collect::<Result<Vec<_>, String>>()?;
        catalog.push(Component {
            name: name.to_owned(),
            framework: Framework::Astro,
            location: ComponentLocation::Package {
                specifier: specifier.to_owned(),
            },
            description,
            props,
            slots,
            client_directives: Vec::new(),
        });
    }
    Ok(catalog)
}

fn catalog_prop(value: &Value) -> Result<Prop, String> {
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "Catalog prop has no name".to_owned())?;
    let kind = match value.get("kind") {
        Some(Value::String(kind)) => match kind.as_str() {
            "string" => PropKind::String,
            "number" => PropKind::Number,
            "boolean" => PropKind::Boolean,
            "date" => PropKind::Date,
            "string-array" => PropKind::StringArray,
            "number-array" => PropKind::NumberArray,
            "image" => PropKind::Image,
            other => return Err(format!("Unknown catalog prop kind `{other}`")),
        },
        Some(Value::Object(kind)) => {
            if let Some(values) = kind.get("enum").and_then(Value::as_array) {
                PropKind::Enum {
                    values: values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                }
            } else if let Some(collection) = kind.get("collection-entry").and_then(Value::as_str) {
                PropKind::CollectionEntry {
                    collection: collection.to_owned(),
                }
            } else {
                return Err(format!("Unknown catalog prop kind for `{name}`"));
            }
        }
        _ => return Err(format!("Catalog prop `{name}` has no kind")),
    };
    let default = value.get("default").and_then(|default| match default {
        Value::Object(object) => {
            if let Some(text) = object.get("string").and_then(Value::as_str) {
                Some(PropDefault::String(text.to_owned()))
            } else if let Some(text) = object.get("number").and_then(Value::as_str) {
                Some(PropDefault::Number(text.to_owned()))
            } else if let Some(boolean) = object.get("boolean").and_then(Value::as_bool) {
                Some(PropDefault::Boolean(boolean))
            } else {
                object.get("list").and_then(Value::as_array).map(|values| {
                    PropDefault::List(
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect(),
                    )
                })
            }
        }
        _ => None,
    });
    Ok(Prop {
        name: name.to_owned(),
        kind,
        required: value
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        default,
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}
