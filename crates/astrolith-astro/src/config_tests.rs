use super::{CollectionConfig, parse_content_config, parse_project_config, parse_src_dir};

const CONTENT_IMPORTS: &str = r#"
import { defineCollection, z } from "astro:content";
import { glob } from "astro/loaders";
"#;

fn content_source(body: &str) -> String {
    format!("{CONTENT_IMPORTS}\n{body}")
}

fn collection<'a>(items: &'a [CollectionConfig], name: &str) -> &'a CollectionConfig {
    items
        .iter()
        .find(|item| item.collection() == name)
        .unwrap_or_else(|| panic!("missing collection {name}"))
}

#[test]
fn accepts_an_empty_literal_collections_export() {
    let source = content_source("export const collections = {};");

    assert_eq!(parse_content_config(&source).unwrap(), Vec::new());
}

#[test]
fn accepts_direct_collections_with_verified_import_aliases() {
    let source = r#"
        import { defineCollection as collection, z } from "astro:content";
        import { glob as find } from "astro/loaders";

        export const collections = {
            articles: collection({
                loader: find({ base: "content/articles", pattern: "**/*.md" }),
                schema: z.object({ title: z.string() }),
            }),
            "field-notes": collection({
                loader: find({ base: "content/notes", pattern: "**/*.{md,mdx}" }),
            }),
        };
    "#;

    let parsed = parse_content_config(source).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(collection(&parsed, "articles").base(), "content/articles");
    assert_eq!(
        collection(&parsed, "articles").pattern().declared(),
        ["**/*.md"]
    );
    assert_eq!(collection(&parsed, "field-notes").base(), "content/notes");
    assert_eq!(
        collection(&parsed, "field-notes").pattern().declared(),
        ["**/*.{md,mdx}"]
    );
}

#[test]
fn accepts_local_const_references_and_preserves_exported_names() {
    let source = content_source(
        r#"
        const localArticles = defineCollection({
            loader: glob({ base: "src/content/articles", pattern: "**/*.mdx" }),
        });

        export const collections = {
            posts: localArticles,
            updates: localArticles,
        };
    "#,
    );

    let parsed = parse_content_config(&source).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(collection(&parsed, "posts").base(), "src/content/articles");
    assert_eq!(
        collection(&parsed, "updates").base(),
        "src/content/articles"
    );
    assert_eq!(
        collection(&parsed, "posts").pattern().declared(),
        ["**/*.mdx"]
    );
}

#[test]
fn accepts_shorthand_collection_references() {
    let source = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        });
        export const collections = { posts };
    "#,
    );

    let parsed = parse_content_config(&source).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].collection(), "posts");
    assert_eq!(parsed[0].base(), "content/posts");
}

#[test]
fn ignores_fake_configuration_in_comments_and_schema_strings() {
    let source = content_source(
        r#"
        // export const collections = { leaked: defineCollection({ loader: glob({
        //   base: "outside", pattern: "**/*.md" }) }) };
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            schema: z.object({
                example: z.string().default("export const collections = { fake: 1 }"),
                note: z.string().default("loader: glob({ base: 'outside' })"),
            }),
        });
        export const collections = { posts };
    "#,
    );

    let parsed = parse_content_config(&source).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].base(), "content/posts");
}

#[test]
fn does_not_treat_configuration_text_inside_a_string_as_code() {
    let source = r#"
        const decoy = `import { defineCollection } from "astro:content";
        import { glob } from "astro/loaders";
        export const collections = { fake: defineCollection({ loader: glob({
          base: "not-real", pattern: "**/*.md" }) }) };`;
    "#;

    assert!(parse_content_config(source).is_err());
}

#[test]
fn rejects_syntax_errors_and_unknown_ast_nodes() {
    assert!(parse_content_config(&content_source("export const collections = {")).is_err());
    assert!(
        parse_content_config(&content_source(
            "export const collections = Object.freeze({ posts: defineCollection({}) });",
        ))
        .is_err()
    );
}

#[test]
fn rejects_missing_or_wrong_imports() {
    let missing_define = r#"
        import { glob } from "astro/loaders";
        export const collections = { posts: defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        }) };
    "#;
    let wrong_source = r#"
        import { defineCollection } from "somewhere-else";
        import { glob } from "astro/loaders";
        export const collections = { posts: defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        }) };
    "#;
    let unrecognized_alias = content_source(
        r#"
        export const collections = { posts: define_collection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        }) };
    "#,
    );

    assert!(parse_content_config(missing_define).is_err());
    assert!(parse_content_config(wrong_source).is_err());
    assert!(parse_content_config(&unrecognized_alias).is_err());
}

#[test]
fn rejects_shadowed_import_bindings() {
    let source = content_source(
        r#"
        const glob = 1;
        export const collections = {};
    "#,
    );

    assert!(parse_content_config(&source).is_err());
}

#[test]
fn rejects_reassignment_and_top_level_side_effects() {
    let reassignment = content_source(
        r#"
        glob = () => {};
        export const collections = {};
    "#,
    );
    let side_effect = content_source(
        r#"
        console.log("loading");
        export const collections = {};
    "#,
    );

    assert!(parse_content_config(&reassignment).is_err());
    assert!(parse_content_config(&side_effect).is_err());
}

#[test]
fn rejects_spreads_computed_keys_and_duplicate_keys() {
    let spread = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        });
        const extra = { posts };
        export const collections = { ...extra };
    "#,
    );
    let computed = content_source(
        r#"
        const name = "posts";
        export const collections = { [name]: defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        }) };
    "#,
    );
    let duplicate = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        });
        export const collections = { posts, "posts": posts };
    "#,
    );

    assert!(parse_content_config(&spread).is_err());
    assert!(parse_content_config(&computed).is_err());
    assert!(parse_content_config(&duplicate).is_err());
}

#[test]
fn rejects_duplicate_keys_in_collection_and_glob_objects() {
    let collection_duplicate = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            schema: z.object({}),
            schema: z.object({ title: z.string() }),
        });
        export const collections = { posts };
    "#,
    );
    let glob_duplicate = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({
                base: "content/posts",
                pattern: "**/*.md",
                pattern: "**/*.mdx",
            }),
        });
        export const collections = { posts };
    "#,
    );

    assert!(parse_content_config(&collection_duplicate).is_err());
    assert!(parse_content_config(&glob_duplicate).is_err());
}

#[test]
fn rejects_unknown_or_unsupported_loader_options() {
    let generate_id = content_source(
        r#"
        export const collections = { posts: defineCollection({
            loader: glob({
                base: "content/posts",
                pattern: "**/*.md",
                generateId: ({ entry }) => entry,
            }),
        }) };
    "#,
    );
    let unknown_config_key = content_source(
        r#"
        export const collections = { posts: defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            type: "content",
        }) };
    "#,
    );

    assert!(parse_content_config(&generate_id).is_err());
    assert!(parse_content_config(&unknown_config_key).is_err());
}

#[test]
fn rejects_unsupported_patterns_and_dynamic_paths() {
    let unsupported_pattern = content_source(
        r#"
        export const collections = { posts: defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.json" }),
        }) };
    "#,
    );
    let dynamic_base = content_source(
        r#"
        const base = "content/posts";
        export const collections = { posts: defineCollection({
            loader: glob({ base, pattern: "**/*.md" }),
        }) };
    "#,
    );
    let dynamic_pattern = content_source(
        r#"
        export const collections = { posts: defineCollection({
            loader: glob({ base: "content/posts", pattern: prefix + "*.md" }),
        }) };
    "#,
    );

    assert!(parse_content_config(&unsupported_pattern).is_err());
    assert!(parse_content_config(&dynamic_base).is_err());
    assert!(parse_content_config(&dynamic_pattern).is_err());
}

#[test]
fn rejects_custom_remote_and_file_loaders() {
    let custom_loader = content_source(
        r#"
        import { file } from "astro/loaders";
        export const collections = { posts: defineCollection({
            loader: file("content/posts.json"),
        }) };
    "#,
    );
    let remote_loader = content_source(
        r#"
        export const collections = { posts: makeLoader({
            base: "content/posts", pattern: "**/*.md",
        }) };
    "#,
    );

    assert!(parse_content_config(&custom_loader).is_err());
    assert!(parse_content_config(&remote_loader).is_err());
}

#[test]
fn schema_must_not_mutate_known_bindings() {
    let source = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            schema: glob,
        });
        export const collections = { posts };
    "#,
    );
    let assigned = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            schema: z.object({ title: z.string() }),
        });
        glob = () => {};
        export const collections = { posts };
    "#,
    );

    assert!(parse_content_config(&source).is_err());
    assert!(parse_content_config(&assigned).is_err());
}

#[test]
fn defaults_src_dir_and_accepts_an_explicit_literal_with_inert_unknown_keys() {
    let default_source = r#"
        import { defineConfig } from "astro/config";
        export default defineConfig({ site: "https://example.com" });
    "#;
    let explicit_source = r#"
        import { defineConfig as configure } from "astro/config";
        export default configure({
            site: "https://example.com",
            markdown: { gfm: true, remarkPlugins: [] },
            srcDir: "app",
        });
    "#;

    assert_eq!(parse_src_dir(default_source).unwrap(), "src");
    assert_eq!(parse_src_dir(explicit_source).unwrap(), "app");
}

#[test]
fn project_config_reports_src_dir_and_verified_mdx_default_imports() {
    let default_import = r#"
        import mdx from "@astrojs/mdx";
        import { defineConfig } from "astro/config";
        export default defineConfig({ srcDir: "app", integrations: [mdx()] });
    "#;
    let named_default_alias = r#"
        import { default as withMdx } from "@astrojs/mdx";
        import { defineConfig as configure } from "astro/config";
        export default configure({ integrations: [withMdx()] });
    "#;
    let mdx_import_not_used = r#"
        import mdx from "@astrojs/mdx";
        import { defineConfig } from "astro/config";
        export default defineConfig({ srcDir: "content" });
    "#;

    let direct = parse_project_config(default_import).unwrap();
    assert_eq!(direct.src_dir, "app");
    assert!(direct.mdx);

    let aliased = parse_project_config(named_default_alias).unwrap();
    assert_eq!(aliased.src_dir, "src");
    assert!(aliased.mdx);

    let unused = parse_project_config(mdx_import_not_used).unwrap();
    assert_eq!(unused.src_dir, "content");
    assert!(!unused.mdx);
    assert_eq!(parse_src_dir(default_import).unwrap(), direct.src_dir);
}

#[test]
fn rejects_mdx_options_and_spreads_but_ignores_unverified_integrations() {
    let options = r#"
        import mdx from "@astrojs/mdx";
        import { defineConfig } from "astro/config";
        export default defineConfig({ integrations: [mdx({})] });
    "#;
    let spread = r#"
        import mdx from "@astrojs/mdx";
        import { defineConfig } from "astro/config";
        export default defineConfig({ integrations: [...[mdx()]] });
    "#;
    for source in [options, spread] {
        assert!(
            parse_project_config(source).is_err(),
            "accepted unsafe integrations: {source}"
        );
    }

    let type_only = r#"
        import type mdx from "@astrojs/mdx";
        import { defineConfig } from "astro/config";
        export default defineConfig({ integrations: [mdx()] });
    "#;
    let named_import = r#"
        import { mdx } from "@astrojs/mdx";
        import { defineConfig } from "astro/config";
        export default defineConfig({ integrations: [mdx()] });
    "#;
    let unknown = r#"
        import integration from "some-integration";
        import { defineConfig } from "astro/config";
        export default defineConfig({ integrations: [integration({ option: true })] });
    "#;
    for source in [type_only, named_import, unknown] {
        assert!(
            !parse_project_config(source).unwrap().mdx,
            "unverified integration factory was treated as MDX: {source}"
        );
    }
}

#[test]
fn ignores_dynamic_non_content_settings_used_by_real_projects() {
    let source = r#"
        import { defineConfig } from "astro/config";
        import cloudflare from "@astrojs/cloudflare";
        import svelte from "@astrojs/svelte";
        import mdx from "@astrojs/mdx";
        import siteConfig from "./site.config";
        import reading from "./src/lib/reading";
        export default defineConfig({
            adapter: cloudflare({ imageService: "compile" }),
            site: "https://example.com",
            i18n: { ...siteConfig.i18n, routing: { prefixDefaultLocale: false } },
            markdown: {
                remarkPlugins: [[reading, { singleTilde: false }]],
                rehypePlugins: [reading],
            },
            vite: { plugins: [reading()] },
            integrations: [svelte(), mdx(), reading()],
            fonts: [{ name: "Serif", provider: "google" }],
        });
    "#;

    let config = parse_project_config(source).unwrap();
    assert_eq!(config.src_dir, "src");
    assert!(config.mdx);
}

#[test]
fn rejects_legacy_and_experimental_astro_config_fields() {
    let legacy = r#"
        import { defineConfig } from "astro/config";
        export default defineConfig({ legacy: { collectionsBackwardsCompat: true } });
    "#;
    let experimental = r#"
        import { defineConfig } from "astro/config";
        export default defineConfig({ experimental: { legacy: { collections: true } } });
    "#;

    for source in [legacy, experimental] {
        assert!(
            parse_project_config(source).is_err(),
            "accepted legacy-affecting Astro config: {source}"
        );
        assert!(parse_src_dir(source).is_err());
    }
}

#[test]
fn rejects_excessively_deep_typescript_ast_before_recursive_config_helpers() {
    let nested = format!("{}0{}", "[".repeat(129), "]".repeat(129));
    let source = format!(
        r#"import {{ defineConfig }} from "astro/config"; export default defineConfig({{ site: {nested} }});"#
    );
    let error = parse_project_config(&source).unwrap_err();
    assert!(
        error.contains("AST exceeds supported depth"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn does_not_read_src_dir_from_comments_or_strings() {
    let decoy = r#"
        const text = `import { defineConfig } from "astro/config";
        export default defineConfig({ srcDir: "not-real" });`;
    "#;
    let real = r#"
        // export default defineConfig({ srcDir: "commented-out" });
        import { defineConfig } from "astro/config";
        export default defineConfig({
            site: "export default defineConfig({ srcDir: 'string-only' })",
        });
    "#;

    assert!(parse_src_dir(decoy).is_err());
    assert_eq!(parse_src_dir(real).unwrap(), "src");
}

#[test]
fn rejects_unsafe_or_unverifiable_astro_config() {
    let missing_import = r#"export default defineConfig({ srcDir: "app" });"#;
    let wrong_import = r#"
        import { defineConfig } from "other/config";
        export default defineConfig({ srcDir: "app" });
    "#;
    let shadowed = r#"
        import { defineConfig } from "astro/config";
        const defineConfig = () => ({});
        export default defineConfig({ srcDir: "app" });
    "#;
    let top_level_effect = r#"
        import { defineConfig } from "astro/config";
        initialize();
        export default defineConfig({ srcDir: "app" });
    "#;

    for source in [missing_import, wrong_import, shadowed, top_level_effect] {
        assert!(parse_src_dir(source).is_err(), "accepted: {source}");
    }
}

#[test]
fn rejects_root_spreads_dynamic_src_dir_and_mutating_unknown_values() {
    let prefix = r#"import { defineConfig } from "astro/config";
        const dynamic = ".";
    "#;
    let root = format!("{prefix} export default defineConfig({{ root: \".\", srcDir: \"app\" }});");
    let dynamic_src = format!("{prefix} export default defineConfig({{ srcDir: dynamic }});");
    let spread =
        format!("{prefix} export default defineConfig({{ ...options, srcDir: \"app\" }});");
    let duplicate =
        format!("{prefix} export default defineConfig({{ srcDir: \"app\", srcDir: \"other\" }});");
    let mutating_unknown = format!(
        "{prefix} export default defineConfig({{ config: (() => {{ srcDir = \"other\"; }})(), srcDir: \"app\" }});"
    );

    for source in [root, dynamic_src, spread, duplicate, mutating_unknown] {
        assert!(parse_src_dir(&source).is_err(), "accepted: {source}");
    }
}

#[test]
fn rejects_absolute_or_empty_src_dir_literals() {
    let prefix = r#"import { defineConfig } from "astro/config";"#;
    for value in ["", "/outside", "..\\outside"] {
        let source = format!("{prefix} export default defineConfig({{ srcDir: {value:?} }});");
        assert!(parse_src_dir(&source).is_err(), "accepted: {source}");
    }
}

#[test]
fn parses_checked_in_astro_fixture_configs_and_rejects_custom_id_oracles() {
    for (name, astro_config, content_config, oracle_config, expected) in [
        (
            "astro5",
            include_str!("../../../fixtures/astro5/astro.config.mjs"),
            include_str!("../../../fixtures/astro5/workspace/content.config.mjs"),
            include_str!("../../../fixtures/astro5/oracle/content.config.mjs"),
            3,
        ),
        (
            "astro6",
            include_str!("../../../fixtures/astro6/astro.config.mjs"),
            include_str!("../../../fixtures/astro6/workspace/content.config.mjs"),
            include_str!("../../../fixtures/astro6/oracle/content.config.mjs"),
            6,
        ),
        (
            "astro7",
            include_str!("../../../fixtures/astro7/astro.config.mjs"),
            include_str!("../../../fixtures/astro7/workspace/content.config.mjs"),
            include_str!("../../../fixtures/astro7/oracle/content.config.mjs"),
            3,
        ),
    ] {
        assert_eq!(
            parse_src_dir(astro_config).unwrap(),
            "./workspace",
            "{name}"
        );

        let parsed = parse_content_config(content_config).unwrap();
        assert_eq!(parsed.len(), expected, "{name}");
        assert_eq!(collection(&parsed, "docs").base(), "./content/docs");
        assert_eq!(
            collection(&parsed, "archive").pattern().declared(),
            ["**/*.{md,mdx}"]
        );

        assert!(
            parse_content_config(oracle_config).is_err(),
            "{name} custom-ID oracle config must remain unsupported"
        );
    }
}

#[test]
fn accepts_array_patterns_with_negative_globs() {
    let source = content_source(
        r#"
        const posts = defineCollection({
            loader: glob({
                base: "content/posts",
                pattern: ["**/*.{md,mdx}", "!**/drafts/**", "!**/hidden.md"],
            }),
        });
        export const collections = { posts };
    "#,
    );

    let parsed = parse_content_config(&source).unwrap();
    assert_eq!(
        collection(&parsed, "posts").pattern().declared(),
        ["**/*.{md,mdx}", "!**/drafts/**", "!**/hidden.md"]
    );
}

#[test]
fn rejects_empty_mixed_or_dynamic_pattern_arrays() {
    for pattern in [
        "[]",
        r#"["**/drafts/**"]"#,
        r#"["**/*.md", "**/*.json"]"#,
        r#"["**/*.md", ...extra]"#,
        r#"[prefix + "*.md"]"#,
        r#"[["**/*.md"]]"#,
    ] {
        let source = content_source(&format!(
            r#"
            const posts = defineCollection({{
                loader: glob({{ base: "content/posts", pattern: {pattern} }}),
            }});
            export const collections = {{ posts }};
        "#
        ));
        assert!(
            parse_content_config(&source).is_err(),
            "accepted pattern {pattern}"
        );
    }
}

#[test]
fn rejects_proto_keys_in_every_interpreted_object() {
    let exported_name = content_source(
        r#"
        export const collections = {
            "__proto__": defineCollection({
                loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            }),
        };
    "#,
    );
    let collection_option = content_source(
        r#"
        export const collections = { posts: defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            "__proto__": {},
        }) };
    "#,
    );
    let loader_option = content_source(
        r#"
        export const collections = { posts: defineCollection({
            loader: glob({
                base: "content/posts",
                pattern: "**/*.md",
                "__proto__": {},
            }),
        }) };
    "#,
    );
    let astro_config = r#"
        import { defineConfig } from "astro/config";
        export default defineConfig({
            srcDir: "app",
            markdown: { "__proto__": { polluted: true } },
        });
    "#;
    let top_level_astro_config = r#"
        import { defineConfig } from "astro/config";
        export default defineConfig({ srcDir: "app", "__proto__": {} });
    "#;

    for source in [exported_name, collection_option, loader_option] {
        assert!(
            parse_content_config(&source).is_err(),
            "accepted __proto__ content configuration: {source}"
        );
    }
    for source in [astro_config, top_level_astro_config] {
        assert!(
            parse_src_dir(source).is_err(),
            "accepted __proto__ Astro configuration: {source}"
        );
    }
}

#[test]
fn rejects_forward_const_references_from_the_collections_export() {
    let source = content_source(
        r#"
        export const collections = { posts };
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
        });
    "#,
    );

    let error = parse_content_config(&source).unwrap_err();
    assert!(
        error.contains("initialized before"),
        "unexpected diagnostic: {error}"
    );
}

#[test]
fn local_const_schemas_remain_an_explicit_unsupported_limit() {
    let source = content_source(
        r#"
        const postSchema = z.object({ title: z.string() });
        const posts = defineCollection({
            loader: glob({ base: "content/posts", pattern: "**/*.md" }),
            schema: postSchema,
        });
        export const collections = { posts };
    "#,
    );

    let error = parse_content_config(&source).unwrap_err();
    assert!(
        error.contains("local const schema"),
        "unexpected diagnostic: {error}"
    );
}
