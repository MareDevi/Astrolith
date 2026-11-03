//! Snippet composition for the component builder.
//!
//! Tag text and placeholder ranges come from the P9 generator; this module adds
//! the two decisions the editor owns: Starlight wrapper prefill and the blank
//! lines a wrapping component needs when it is inserted on an empty line. The
//! import plan is built here too, so the generated tag already uses the
//! identifiers that plan resolves.

use std::{collections::BTreeMap, ops::Range, path::Path};

use astrolith_astro::components::{
    ClientDirectiveValue, Component, CorroboratedImport, ImportPlan, ImportPlanRequest,
    PlannedValue, PropKind, Snippet, SnippetPlaceholder, SnippetValue, TagRequest, generate_tag,
    plan_imports,
};

use super::form::PlannedFieldValue;

/// Everything an insertion needs, measured at confirmation time.
pub(crate) struct InsertionContext<'a> {
    pub component: &'a Component,
    /// Form values before planned identifiers resolve.
    pub values: Vec<(String, SnippetValue)>,
    pub planned: Vec<PlannedFieldValue>,
    pub content: Option<String>,
    pub client_directive: Option<ClientDirectiveValue>,
    pub root: &'a Path,
    pub document: &'a Path,
    pub source: &'a str,
    pub imports_enabled: bool,
    pub current_imports: &'a [CorroboratedImport],
    pub other_imports: &'a [CorroboratedImport],
    /// Indexed catalog, so scaffold children resolve to real components.
    pub catalog: &'a [Component],
    pub visible: &'a str,
    pub selection: Range<usize>,
}

/// A tag and its import plan, ready for the insertion step.
#[derive(Clone, Debug)]
pub(crate) struct PreparedInsertion {
    pub tag: Snippet,
    pub plan: ImportPlan,
}

impl PreparedInsertion {
    /// Whether the plan edits the complete source.
    pub(crate) fn changes_imports(&self, source: &str) -> bool {
        self.plan.source() != source
    }
}

/// Plan imports and generate the tag from the same values.
pub(crate) fn prepare(context: &InsertionContext<'_>) -> Result<PreparedInsertion, String> {
    let planned: Vec<PlannedValue> = context
        .planned
        .iter()
        .map(|value| match value {
            PlannedFieldValue::Image { prop, path } => PlannedValue::Image {
                prop: prop.clone(),
                path: path.clone(),
            },
            PlannedFieldValue::Entry {
                prop,
                collection,
                id,
            } => PlannedValue::Entry {
                prop: prop.clone(),
                collection: collection.clone(),
                id: id.clone(),
            },
        })
        .collect();
    let extras = scaffold_components(
        context.component,
        context.content.as_deref(),
        context.catalog,
    );
    let plan = plan_imports(&ImportPlanRequest {
        source: context.source,
        root: context.root,
        document: context.document,
        component: context.component,
        extra_components: &extras,
        values: &planned,
        enabled: context.imports_enabled,
        current_collection_imports: context.current_imports,
        other_collection_imports: context.other_imports,
    })
    .map_err(|error| error.to_string())?;

    let values = resolve_identifiers(
        context.component,
        context.values.clone(),
        plan.identifiers(),
    );
    let mut tag = generate_tag(&TagRequest {
        component: context.component,
        values,
        content: context.content.clone(),
        client_directive: context.client_directive.clone(),
    })
    .map_err(|error| error.to_string())?;
    if context.component.has_default_slot() {
        surround_blank_lines(&mut tag, context.visible, &context.selection);
    }
    Ok(PreparedInsertion { tag, plan })
}

/// Replace image and entry values with the identifiers the plan resolved.
fn resolve_identifiers(
    component: &Component,
    values: Vec<(String, SnippetValue)>,
    identifiers: &BTreeMap<String, String>,
) -> Vec<(String, SnippetValue)> {
    values
        .into_iter()
        .map(|(name, value)| {
            let planned_kind = component.prop(&name).is_some_and(|prop| {
                matches!(
                    prop.kind,
                    PropKind::Image | PropKind::CollectionEntry { .. }
                )
            });
            match identifiers.get(&name) {
                Some(identifier) if planned_kind => {
                    (name, SnippetValue::Identifier(identifier.clone()))
                }
                _ => (name, value),
            }
        })
        .collect()
}

/// Give a wrapping component its own lines when the caret is on an empty line.
///
/// Upstream inserts a slot-bearing component with a blank line before and after
/// so Markdown can be written inside it. Anywhere else the tag is inserted
/// exactly at the selection.
pub(crate) fn surround_blank_lines(tag: &mut Snippet, visible: &str, selection: &Range<usize>) {
    if selection.start != selection.end || selection.end > visible.len() {
        return;
    }
    let caret = selection.start;
    let line_start = visible[..caret].rfind('\n').map_or(0, |index| index + 1);
    let line_end = visible[caret..]
        .find('\n')
        .map_or(visible.len(), |index| caret + index);
    let empty_line = visible
        .get(line_start..line_end)
        .is_some_and(|line| line.trim().is_empty());
    if !empty_line {
        return;
    }
    let eol = if visible.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let prefix = eol;
    let text = format!("{prefix}{}{prefix}", tag.text);
    let shift = prefix.len();
    tag.placeholders = tag
        .placeholders
        .iter()
        .map(|placeholder| SnippetPlaceholder {
            range: placeholder.range.start + shift..placeholder.range.end + shift,
            kind: placeholder.kind.clone(),
        })
        .collect();
    tag.text = text;
}

/// The package whose wrappers receive generated child scaffolds.
const STARLIGHT_COMPONENTS: &str = "@astrojs/starlight/components";

/// A Starlight wrapper's generated children.
pub(crate) struct StarlightPrefill {
    /// Child content the form starts with.
    pub(crate) content: &'static str,
    /// Catalog components that content references by name.
    children: &'static [&'static str],
}

/// Claim the child content a Starlight wrapper expects.
///
/// Only the wrappers the parity plan names are pre-filled, and only when the
/// component really is the Starlight export: a project component that happens
/// to be called `Tabs` keeps the plain empty content placeholder.
pub(crate) fn starlight_prefill(component: &Component) -> Option<StarlightPrefill> {
    if !component.has_default_slot() || component.package_specifier() != Some(STARLIGHT_COMPONENTS)
    {
        return None;
    }
    let (content, children) = match component.name.as_str() {
        "Tabs" => (
            "<TabItem label=\"First\">\nFirst tab content\n</TabItem>\n\n<TabItem label=\"Second\">\nSecond tab content\n</TabItem>",
            &["TabItem"][..],
        ),
        "CardGrid" => (
            "<Card title=\"First card\">\nFirst card body\n</Card>\n\n<Card title=\"Second card\">\nSecond card body\n</Card>",
            &["Card"][..],
        ),
        "Steps" => (
            "<ol>\n  <li>First step</li>\n  <li>Second step</li>\n</ol>",
            &[][..],
        ),
        "FileTree" => (
            "<ul>\n  <li><code>src/</code>\n    <ul>\n      <li><code>index.astro</code></li>\n    </ul>\n  </li>\n</ul>",
            &[][..],
        ),
        _ => return None,
    };
    Some(StarlightPrefill { content, children })
}

/// The indexed catalog components a scaffold's content actually references.
///
/// Children the user deleted from the scaffold are left out, so an edited
/// prefill never imports a component the tag does not use.
fn scaffold_components(
    component: &Component,
    content: Option<&str>,
    catalog: &[Component],
) -> Vec<Component> {
    let (Some(prefill), Some(content)) = (starlight_prefill(component), content) else {
        return Vec::new();
    };
    prefill
        .children
        .iter()
        .filter(|name| references_tag(content, name))
        .filter_map(|name| {
            catalog
                .iter()
                .find(|candidate| {
                    candidate.name == **name
                        && candidate.package_specifier() == component.package_specifier()
                })
                .cloned()
        })
        .collect()
}

/// Whether `content` opens or closes a `<name>` tag.
fn references_tag(content: &str, name: &str) -> bool {
    content.match_indices('<').any(|(index, _)| {
        let rest = &content[index + 1..];
        rest.strip_prefix(name).is_some_and(|after| {
            after
                .chars()
                .next()
                .is_some_and(|next| next == '>' || next == '/' || next.is_whitespace())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_astro::components::{ComponentIndexRequest, PropKind, inspect_components};
    use std::path::{Path, PathBuf};

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../astrolith-astro/tests/fixtures/components/project")
    }

    fn index() -> astrolith_astro::components::ComponentIndex {
        inspect_components(&ComponentIndexRequest::new(
            fixture_root(),
            3,
            astrolith_astro::Cancellation::default(),
        ))
        .expect("fixture index")
    }

    fn component<'a>(
        index: &'a astrolith_astro::components::ComponentIndex,
        name: &str,
        package: bool,
    ) -> &'a Component {
        index
            .components()
            .iter()
            .find(|component| {
                component.name == name && component.package_specifier().is_some() == package
            })
            .unwrap_or_else(|| panic!("component {name}"))
    }

    const SOURCE: &str = "---\ntitle: Insert\n---\n\n# Insert\n";

    fn context<'a>(
        component: &'a Component,
        values: Vec<(String, SnippetValue)>,
        content: Option<String>,
        enabled: bool,
        visible: &'a str,
        selection: Range<usize>,
    ) -> InsertionContext<'a> {
        InsertionContext {
            component,
            values,
            planned: Vec::new(),
            content,
            client_directive: None,
            root: Path::new("/does/not/exist"),
            document: Path::new("/does/not/exist/post.mdx"),
            source: SOURCE,
            imports_enabled: enabled,
            current_imports: &[],
            other_imports: &[],
            catalog: &[],
            visible,
            selection,
        }
    }

    #[test]
    fn tag_only_insertion_is_planned_without_imports_when_disabled() {
        let index = index();
        let callout = component(&index, "Callout", false);
        let prepared = prepare(&context(
            callout,
            vec![("title".into(), SnippetValue::String("Hello".into()))],
            None,
            false,
            "Body text",
            0..0,
        ))
        .unwrap();
        assert_eq!(prepared.plan.source(), SOURCE);
        assert!(!prepared.changes_imports(SOURCE));
        assert!(prepared.tag.text.starts_with("<Callout title=\"Hello\""));
    }

    #[test]
    fn planned_imports_resolve_image_and_entry_identifiers() {
        let root = tempfile::tempdir().unwrap();
        let document = root.path().join("src/content/blog/post.mdx");
        std::fs::create_dir_all(document.parent().unwrap()).unwrap();
        std::fs::write(&document, SOURCE).unwrap();
        std::fs::create_dir_all(root.path().join("src/assets")).unwrap();
        std::fs::write(root.path().join("src/assets/cover.png"), b"png").unwrap();

        // A component file inside the temporary project, so the planner's
        // containment check exercises the real path rules.
        let component = Component {
            name: "Callout".into(),
            framework: astrolith_astro::components::Framework::Astro,
            location: astrolith_astro::components::ComponentLocation::Project {
                source: root.path().join("src/components/Callout.astro"),
                relative_path: PathBuf::from("src/components/Callout.astro"),
            },
            description: None,
            props: vec![
                astrolith_astro::components::Prop {
                    name: "title".into(),
                    kind: PropKind::String,
                    required: true,
                    default: None,
                    description: None,
                },
                astrolith_astro::components::Prop {
                    name: "cover".into(),
                    kind: PropKind::Image,
                    required: false,
                    default: None,
                    description: None,
                },
                astrolith_astro::components::Prop {
                    name: "related".into(),
                    kind: PropKind::CollectionEntry {
                        collection: "blog".into(),
                    },
                    required: false,
                    default: None,
                    description: None,
                },
            ],
            slots: Vec::new(),
            client_directives: Vec::new(),
        };
        std::fs::create_dir_all(root.path().join("src/components")).unwrap();
        std::fs::write(
            root.path().join("src/components/Callout.astro"),
            "---\ninterface Props { title: string }\n---\n<section><slot /></section>\n",
        )
        .unwrap();
        let mut context = context(
            &component,
            vec![
                ("title".into(), SnippetValue::String("Hello".into())),
                (
                    "cover".into(),
                    SnippetValue::String("../../assets/cover.png".into()),
                ),
                ("related".into(), SnippetValue::String("first-post".into())),
            ],
            None,
            true,
            "Body text",
            0..0,
        );
        context.root = root.path();
        context.document = &document;
        context.planned = vec![
            PlannedFieldValue::Image {
                prop: "cover".into(),
                path: "../../assets/cover.png".into(),
            },
            PlannedFieldValue::Entry {
                prop: "related".into(),
                collection: "blog".into(),
                id: "first-post".into(),
            },
        ];
        let prepared = prepare(&context).unwrap();
        assert!(prepared.changes_imports(SOURCE));
        assert!(prepared.plan.source().contains("import Callout from"));
        assert!(
            prepared
                .plan
                .source()
                .contains("export const relatedEntry = await getEntry(\"blog\", \"first-post\")")
        );
        assert!(prepared.tag.text.contains("cover={cover}"));
        assert!(prepared.tag.text.contains("related={relatedEntry}"));

        // The plan is tied to the source it was computed against.
        assert!(prepared.plan.revalidate(SOURCE).is_ok());
        let changed = format!("{SOURCE}x");
        assert!(prepared.plan.revalidate(&changed).is_err());
    }

    #[test]
    fn wrapping_scaffolds_take_their_own_blank_lines() {
        let index = index();
        let callout = component(&index, "Callout", false);
        let visible = "Before\n\n\nAfter";
        // The caret sits on the empty line between the two blank lines.
        let prepared = prepare(&context(
            callout,
            vec![("title".into(), SnippetValue::String("T".into()))],
            Some("Wrapped".into()),
            false,
            visible,
            7..7,
        ))
        .unwrap();
        assert!(prepared.tag.text.starts_with("\n<Callout"));
        assert!(prepared.tag.text.ends_with("</Callout>\n"));
        assert_eq!(prepared.tag.text.matches('\n').count(), 4);

        // A caret inside text inserts inline.
        let inline = prepare(&context(
            callout,
            vec![("title".into(), SnippetValue::String("T".into()))],
            Some("Wrapped".into()),
            false,
            visible,
            2..2,
        ))
        .unwrap();
        assert!(inline.tag.text.starts_with("<Callout"));
    }

    #[test]
    fn starlight_wrappers_prefill_their_children() {
        let index = index();
        let tabs = component(&index, "Tabs", true);
        assert!(tabs.has_default_slot());
        let prefill = starlight_prefill(tabs).unwrap();
        assert_eq!(prefill.content.matches("<TabItem").count(), 2);
        let prepared = prepare(&context(
            tabs,
            Vec::new(),
            Some(prefill.content.to_owned()),
            false,
            "",
            0..0,
        ))
        .unwrap();
        assert!(prepared.tag.text.starts_with("\n<Tabs>"));
        assert_eq!(prepared.tag.text.matches("<TabItem").count(), 2);

        for name in ["CardGrid", "Steps", "FileTree"] {
            let wrapper = component(&index, name, true);
            assert!(
                starlight_prefill(wrapper).is_some(),
                "{name} must prefill children"
            );
        }
        let aside = component(&index, "Aside", true);
        assert!(starlight_prefill(aside).is_none());
        let callout = component(&index, "Callout", false);
        assert!(starlight_prefill(callout).is_none());

        // The prefill belongs to the Starlight export, never to a project
        // component that shares its name.
        let impostor = Component {
            name: "Tabs".into(),
            framework: astrolith_astro::components::Framework::Astro,
            location: astrolith_astro::components::ComponentLocation::Project {
                source: fixture_root().join("src/components/Tabs.astro"),
                relative_path: PathBuf::from("src/components/Tabs.astro"),
            },
            description: None,
            props: Vec::new(),
            slots: tabs.slots.clone(),
            client_directives: Vec::new(),
        };
        assert!(starlight_prefill(&impostor).is_none());
    }

    /// The fixture document the import-planning assertions insert into.
    fn fixture_document() -> (PathBuf, PathBuf, String) {
        let root = fixture_root();
        let document = root.join("src/content/blog/plain.mdx");
        let source = std::fs::read_to_string(&document).unwrap();
        (root, document, source)
    }

    #[test]
    fn starlight_scaffold_plans_its_children_in_the_same_import() {
        let index = index();
        let (root, document, source) = fixture_document();
        let tabs = component(&index, "Tabs", true);
        let prefill = starlight_prefill(tabs).unwrap();
        let mut tabs_context = context(
            tabs,
            Vec::new(),
            Some(prefill.content.to_owned()),
            true,
            "",
            0..0,
        );
        tabs_context.root = &root;
        tabs_context.document = &document;
        tabs_context.source = &source;
        tabs_context.catalog = index.components();
        let prepared = prepare(&tabs_context).unwrap();
        assert_eq!(
            prepared.plan.source(),
            concat!(
                "---\ntitle: Plain imports\n---\n\n",
                "import Existing from \"../../components/Callout.astro\";\n",
                "import { formatLabel } from \"../../lib/format\";\n",
                "import { Tabs, TabItem } from \"@astrojs/starlight/components\";\n",
                "\n# Plain imports\n\n",
                "<Existing title=\"Keeps existing bytes\" />\n",
            )
        );
        assert_eq!(prepared.tag.text.matches("<TabItem").count(), 2);

        let grid = component(&index, "CardGrid", true);
        let prefill = starlight_prefill(grid).unwrap();
        let mut grid_context = context(
            grid,
            Vec::new(),
            Some(prefill.content.to_owned()),
            true,
            "",
            0..0,
        );
        grid_context.root = &root;
        grid_context.document = &document;
        grid_context.source = &source;
        grid_context.catalog = index.components();
        let prepared = prepare(&grid_context).unwrap();
        assert_eq!(
            prepared.plan.source(),
            concat!(
                "---\ntitle: Plain imports\n---\n\n",
                "import Existing from \"../../components/Callout.astro\";\n",
                "import { formatLabel } from \"../../lib/format\";\n",
                "import { CardGrid, Card } from \"@astrojs/starlight/components\";\n",
                "\n# Plain imports\n\n",
                "<Existing title=\"Keeps existing bytes\" />\n",
            )
        );
        assert_eq!(prepared.tag.text.matches("<Card ").count(), 2);
    }

    #[test]
    fn deleted_scaffold_children_are_not_imported() {
        let index = index();
        let (root, document, source) = fixture_document();
        let tabs = component(&index, "Tabs", true);
        let mut context = context(
            tabs,
            Vec::new(),
            Some("Plain body\n".to_owned()),
            true,
            "",
            0..0,
        );
        context.root = &root;
        context.document = &document;
        context.source = &source;
        context.catalog = index.components();
        let prepared = prepare(&context).unwrap();
        assert_eq!(
            prepared.plan.source(),
            concat!(
                "---\ntitle: Plain imports\n---\n\n",
                "import Existing from \"../../components/Callout.astro\";\n",
                "import { formatLabel } from \"../../lib/format\";\n",
                "import { Tabs } from \"@astrojs/starlight/components\";\n",
                "\n# Plain imports\n\n",
                "<Existing title=\"Keeps existing bytes\" />\n",
            )
        );
    }

    #[test]
    fn own_line_surround_shifts_placeholder_ranges() {
        let mut tag = Snippet {
            text: "<A>".into(),
            placeholders: vec![SnippetPlaceholder {
                range: 2..2,
                kind: astrolith_astro::components::PlaceholderKind::Content,
            }],
        };
        surround_blank_lines(&mut tag, "\n\n", &(1..1));
        assert_eq!(tag.text, "\n<A>\n");
        assert_eq!(tag.placeholders[0].range, 3..3);
    }
}
