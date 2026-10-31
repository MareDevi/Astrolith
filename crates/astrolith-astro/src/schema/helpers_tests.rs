use serde_json::{Value, json};

use super::{CollectionSchema, Field, FieldKind, enhance_astro_fields, normalize_schema};

fn normalize(value: Value) -> CollectionSchema {
    normalize_schema(&value).expect("schema normalizes")
}

fn field<'a>(schema: &'a CollectionSchema, name: &str) -> &'a Field {
    schema
        .fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("missing field `{name}`"))
}

fn nested<'a>(field: &'a Field, name: &str) -> &'a Field {
    let FieldKind::Object { fields } = &field.kind else {
        panic!("field `{}` is not an object: {:?}", field.name, field.kind);
    };
    fields
        .iter()
        .find(|field| field.name == name)
        .unwrap_or_else(|| panic!("missing nested field `{name}`"))
}

/// JSON Schema shape Astro emits for `reference(...)`: a heterogeneous union of
/// a string and two objects, which alone normalizes to `Unsupported`.
fn reference_json() -> Value {
    json!({
        "anyOf": [
            { "type": "string" },
            {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "collection": { "type": "string" }
                },
                "required": ["id", "collection"]
            },
            {
                "type": "object",
                "properties": {
                    "slug": { "type": "string" },
                    "collection": { "type": "string" }
                },
                "required": ["slug", "collection"]
            }
        ]
    })
}

const DIRECT_CONFIG: &str = r#"
import { defineCollection, reference } from "astro:content";
import { z } from "astro/zod";
import { glob } from "astro/loaders";

const docs = defineCollection({
  loader: glob({ base: "./content/docs", pattern: "**/*.md" }),
  schema: ({ image }) => z.object({
    title: z.string(),
    cover: image(),
    author: reference("authors"),
  }),
});

export const collections = { docs };
"#;

#[test]
fn resolves_direct_image_and_reference_helpers() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "cover": { "type": "string" },
            "author": reference_json()
        },
        "required": ["title", "cover", "author"]
    }));
    // Sanity: a bare reference is unsupported and diagnosed before enhancement.
    assert!(matches!(
        field(&schema, "author").kind,
        FieldKind::Unsupported { .. }
    ));
    assert_eq!(schema.diagnostics.len(), 1);

    let diagnostics = enhance_astro_fields(&mut schema, DIRECT_CONFIG);

    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );
    assert_eq!(
        field(&schema, "title").kind,
        FieldKind::String { format: None }
    );
    assert_eq!(field(&schema, "cover").kind, FieldKind::Image);
    assert_eq!(
        field(&schema, "author").kind,
        FieldKind::Reference {
            collections: vec!["authors".to_owned()]
        }
    );
    // The stale "union not supported" diagnostic is cleared once resolved.
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn resolves_nested_object_and_array_helpers() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "gallery": { "type": "array", "items": { "type": "string" } },
            "related": { "type": "array", "items": reference_json() },
            "meta": {
                "type": "object",
                "properties": {
                    "avatar": { "type": "string" },
                    "links": { "type": "array", "items": reference_json() }
                },
                "required": ["avatar", "links"]
            }
        },
        "required": ["title"]
    }));

    let config = r#"
import { defineCollection, reference } from "astro:content";
import { z } from "astro/zod";

const docs = defineCollection({
  schema: z.object({
    title: z.string(),
    gallery: z.array(image()),
    related: z.array(reference("posts")),
    meta: z.object({
      avatar: image(),
      links: z.array(reference("links")),
    }),
  }),
});
export const collections = { docs };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );

    assert_eq!(
        field(&schema, "gallery").kind,
        FieldKind::Array {
            items: Box::new(FieldKind::Image)
        }
    );
    assert_eq!(
        field(&schema, "related").kind,
        FieldKind::Array {
            items: Box::new(FieldKind::Reference {
                collections: vec!["posts".to_owned()]
            })
        }
    );
    let meta = field(&schema, "meta");
    assert_eq!(nested(meta, "avatar").kind, FieldKind::Image);
    assert_eq!(
        nested(meta, "links").kind,
        FieldKind::Array {
            items: Box::new(FieldKind::Reference {
                collections: vec!["links".to_owned()]
            })
        }
    );
    assert!(schema.diagnostics.is_empty());
}

#[test]
fn wraps_nullable_and_preserves_optional_requiredness() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": {
            "cover": { "type": ["string", "null"] },
            "author": reference_json()
        },
        "required": ["cover"]
    }));

    let config = r#"
const docs = defineCollection({
  schema: z.object({
    cover: image().nullable(),
    author: reference("authors").optional(),
  }),
});
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );

    assert_eq!(
        field(&schema, "cover").kind,
        FieldKind::Nullable {
            inner: Box::new(FieldKind::Image)
        }
    );
    assert!(field(&schema, "cover").required);
    assert_eq!(
        field(&schema, "author").kind,
        FieldKind::Reference {
            collections: vec!["authors".to_owned()]
        }
    );
    assert!(!field(&schema, "author").required);
}

#[test]
fn honors_imported_aliases_for_helpers() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": {
            "cover": { "type": "string" },
            "document": reference_json()
        }
    }));

    let config = r#"
import { defineCollection, image as picture, reference as ref } from "astro:content";

const docs = defineCollection({
  schema: z.object({
    cover: picture(),
    document: ref("documents"),
  }),
});
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );
    assert_eq!(field(&schema, "cover").kind, FieldKind::Image);
    assert_eq!(
        field(&schema, "document").kind,
        FieldKind::Reference {
            collections: vec!["documents".to_owned()]
        }
    );
}

#[test]
fn resolves_helpers_from_a_local_const_schema() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "cover": { "type": "string" } }
    }));

    let config = r#"
const docsSchema = ({ image }) => z.object({ cover: image() });
const docs = defineCollection({ schema: docsSchema });
export const collections = { docs };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );
    assert_eq!(field(&schema, "cover").kind, FieldKind::Image);
}

#[test]
fn selects_the_collection_whose_fields_match_the_schema() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": {
            "title": { "type": "string" },
            "cover": { "type": "string" }
        }
    }));

    // Two schema-bearing collections both define `cover`; only the one whose
    // field set matches the target schema (title + cover) may refine it.
    let config = r#"
const docs = defineCollection({
  schema: z.object({ title: z.string(), cover: image() }),
});
const notes = defineCollection({
  schema: z.object({ name: z.string(), cover: reference("gallery") }),
});
export const collections = { docs, notes };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );
    assert_eq!(field(&schema, "cover").kind, FieldKind::Image);
}

#[test]
fn malformed_config_is_a_non_fatal_diagnostic_and_leaves_schema_unchanged() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "cover": { "type": "string" } }
    }));

    let diagnostics = enhance_astro_fields(
        &mut schema,
        "const docs = defineCollection({ schema: z.object({ cover: image( })",
    );

    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, "<root>");
    assert!(diagnostics[0].message.contains("invalid TypeScript syntax"));
    // Never silently coerce: the field keeps its pre-enhancement kind.
    assert_eq!(
        field(&schema, "cover").kind,
        FieldKind::String { format: None }
    );
    assert_eq!(schema.diagnostics, diagnostics);
}

#[test]
fn non_literal_reference_argument_is_diagnosed_not_coerced() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "author": reference_json() }
    }));

    let config = r#"
const docs = defineCollection({
  schema: z.object({ author: reference(collectionName) }),
});
export const collections = { docs };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, "author");
    assert!(diagnostics[0].message.contains("string literal"));
    // The unresolved union stays unsupported rather than becoming a string.
    assert!(matches!(
        field(&schema, "author").kind,
        FieldKind::Unsupported { .. }
    ));
}

#[test]
fn reference_without_arguments_is_diagnosed() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "author": reference_json() }
    }));

    let config = r#"
const docs = defineCollection({
  schema: z.object({ author: reference() }),
});
export const collections = { docs };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, "author");
    assert!(
        diagnostics[0]
            .message
            .contains("requires a collection name")
    );
    assert!(matches!(
        field(&schema, "author").kind,
        FieldKind::Unsupported { .. }
    ));
}

#[test]
fn a_schema_without_helpers_is_unchanged() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "title": { "type": "string" } }
    }));
    let before = schema.clone();

    let config = r#"
const docs = defineCollection({ schema: z.object({ title: z.string() }) });
export const collections = { docs };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(diagnostics.is_empty());
    assert_eq!(schema, before);
}

#[test]
fn conflicting_helpers_across_collections_are_diagnosed_and_left_unrefined() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "cover": { "type": "string" } }
    }));

    // Identical field sets, so selection cannot disambiguate; the conflicting
    // helpers must not be guessed.
    let config = r#"
const docs = defineCollection({ schema: z.object({ cover: image() }) });
const notes = defineCollection({ schema: z.object({ cover: reference("gallery") }) });
export const collections = { docs, notes };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].path, "cover");
    assert!(diagnostics[0].message.contains("ambiguous"));
    assert_eq!(
        field(&schema, "cover").kind,
        FieldKind::String { format: None }
    );
}

#[test]
fn comma_separated_reference_collections_are_preserved() {
    let mut schema = normalize(json!({
        "type": "object",
        "properties": { "related": reference_json() }
    }));

    let config = r#"
const docs = defineCollection({
  schema: z.object({ related: reference("blog", "authors") }),
});
export const collections = { docs };
"#;

    let diagnostics = enhance_astro_fields(&mut schema, config);
    assert!(
        diagnostics.is_empty(),
        "unexpected diagnostics: {diagnostics:?}"
    );
    assert_eq!(
        field(&schema, "related").kind,
        FieldKind::Reference {
            collections: vec!["blog".to_owned(), "authors".to_owned()]
        }
    );
}
