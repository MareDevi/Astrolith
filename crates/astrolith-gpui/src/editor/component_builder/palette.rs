//! Component palette ordering, fuzzy search and row summaries.
//!
//! The palette is a pure projection of the static component index: project
//! components first, then one group per package specifier, each row carrying
//! the framework badge, required-prop summary, description and wrap state the
//! builder renders.

use astrolith_astro::components::{Component, ComponentLocation};
use gpui_kit::SharedString;

/// Where a palette row sits in the grouped list.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum ComponentGroup {
    /// A component from the open project.
    Project,
    /// A component exported by a declared package.
    Package(String),
}

impl ComponentGroup {
    /// The header the palette shows before a group's rows.
    pub(crate) fn label(&self) -> SharedString {
        match self {
            Self::Project => tr!("editor.palette.project_group").into(),
            Self::Package(specifier) => specifier.clone().into(),
        }
    }
}

/// The requirement summary a palette row shows under its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Requirement {
    /// Required prop names, rendered as a comma-separated list.
    Props(Vec<String>),
    /// The number of optional props when nothing is required.
    Optional(usize),
    /// The component takes no props at all.
    None,
}

impl Requirement {
    /// The translated line under the row's name.
    pub(crate) fn label(&self) -> SharedString {
        match self {
            Self::Props(names) => tr!("editor.palette.requires", names = names.join(", ")).into(),
            Self::Optional(count) => tr!("editor.palette.optional_count", count = count).into(),
            Self::None => tr!("editor.palette.no_props").into(),
        }
    }
}

/// One row the palette can show.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaletteEntry {
    pub component: Component,
    pub group: ComponentGroup,
    /// Numeric group rank; project is always 0 and package groups follow.
    pub group_rank: u16,
    /// Framework badge label.
    pub badge: &'static str,
    /// Required props, optional count or no props.
    pub requirement: Requirement,
    /// Whether the component accepts wrapped content.
    pub wraps: bool,
}

impl PaletteEntry {
    /// The first line of the component's doc comment, for a compact row.
    pub(crate) fn description(&self) -> Option<&str> {
        self.component
            .description
            .as_deref()
            .map(str::trim)
            .filter(|description| !description.is_empty())
    }
}

/// All indexed components in palette order.
#[derive(Clone, Debug, Default)]
pub(crate) struct Palette {
    entries: Vec<PaletteEntry>,
}

impl Palette {
    /// Group an index for display: project components first, then packages.
    pub(crate) fn new(components: &[Component]) -> Self {
        let project = components
            .iter()
            .enumerate()
            .filter(|(_, component)| {
                matches!(component.location, ComponentLocation::Project { .. })
            })
            .map(|(index, component)| (index, component, ComponentGroup::Project))
            .collect::<Vec<_>>();
        let mut packages: std::collections::BTreeMap<String, Vec<(usize, &Component)>> =
            std::collections::BTreeMap::new();
        for (index, component) in components.iter().enumerate() {
            if let ComponentLocation::Package { specifier } = &component.location {
                packages
                    .entry(specifier.clone())
                    .or_default()
                    .push((index, component));
            }
        }
        let mut ordered: Vec<(usize, &Component, ComponentGroup)> = Vec::new();
        let mut project = project;
        project.sort_by(|(left_index, left, _), (right_index, right, _)| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id().cmp(&right.id()))
                .then_with(|| left_index.cmp(right_index))
        });
        ordered.extend(project);
        for (specifier, mut members) in packages {
            members.sort_by(|(left_index, left), (right_index, right)| {
                left.name
                    .cmp(&right.name)
                    .then_with(|| left.id().cmp(&right.id()))
                    .then_with(|| left_index.cmp(right_index))
            });
            for (index, component) in members {
                ordered.push((index, component, ComponentGroup::Package(specifier.clone())));
            }
        }
        let entries = ordered
            .into_iter()
            .map(|(_index, component, group)| PaletteEntry {
                group,
                group_rank: 0,
                badge: component.framework.label(),
                requirement: requirement(component),
                wraps: component.has_default_slot(),
                component: component.clone(),
            })
            .collect::<Vec<_>>();
        // Stable ranks: every distinct group after Project increments.
        let mut rank = 0u16;
        let mut previous: Option<ComponentGroup> = None;
        let entries = entries
            .into_iter()
            .map(|mut entry| {
                if previous.as_ref() != Some(&entry.group) {
                    previous = Some(entry.group.clone());
                    if entry.group != ComponentGroup::Project {
                        rank = rank.saturating_add(1);
                    }
                }
                entry.group_rank = rank;
                entry
            })
            .collect();
        Self { entries }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn get(&self, position: usize) -> Option<&PaletteEntry> {
        self.entries.get(position)
    }

    /// Fuzzy-search positions into [`Self::entries`].
    ///
    /// An empty query keeps palette order. Results stay grouped — project rows
    /// before package rows — and within a group rank exact and prefix matches
    /// above substrings above loose subsequences.
    pub(crate) fn search(&self, query: &str) -> Vec<usize> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let mut hits: Vec<(usize, u32)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(position, entry)| {
                fuzzy_score(&query, &entry.component.name).map(|score| (position, score))
            })
            .collect();
        hits.sort_by(
            |(left_position, left_score), (right_position, right_score)| {
                self.entries[*left_position]
                    .group_rank
                    .cmp(&self.entries[*right_position].group_rank)
                    .then_with(|| left_score.cmp(right_score))
                    .then_with(|| {
                        self.entries[*left_position]
                            .component
                            .name
                            .cmp(&self.entries[*right_position].component.name)
                    })
                    .then_with(|| left_position.cmp(right_position))
            },
        );
        hits.into_iter().map(|(position, _)| position).collect()
    }

    /// Group headers for a hit sequence.
    ///
    /// The first hit of each group carries that group's label and the rest
    /// carry none, so the palette can print one header per group without
    /// inventing non-selectable rows.
    pub(crate) fn headers(&self, hits: &[usize]) -> Vec<Option<SharedString>> {
        let mut previous = None;
        hits.iter()
            .map(|position| {
                let entry = self.get(*position)?;
                let header = (previous != Some(entry.group_rank)).then(|| entry.group.label());
                previous = Some(entry.group_rank);
                header
            })
            .collect()
    }
}

/// The required-prop summary shown under a palette row's name.
pub(crate) fn requirement(component: &Component) -> Requirement {
    let required: Vec<String> = component
        .props
        .iter()
        .filter(|prop| prop.required)
        .map(|prop| prop.name.clone())
        .collect();
    if !required.is_empty() {
        return Requirement::Props(required);
    }
    let optional = component.props.len();
    if optional > 0 {
        return Requirement::Optional(optional);
    }
    Requirement::None
}

/// Rank `query` against `name`; lower scores match more closely.
fn fuzzy_score(query: &str, name: &str) -> Option<u32> {
    let name = name.to_lowercase();
    if name == query {
        return Some(0);
    }
    if name.starts_with(query) {
        return Some(1);
    }
    if let Some(position) = name.find(query) {
        return Some(2 + position as u32);
    }
    let mut cursor = 0;
    let mut gaps = 0u32;
    for character in query.chars() {
        let rest = name.get(cursor..)?;
        let offset = rest.find(character)?;
        gaps += u32::try_from(offset).ok()?;
        cursor += offset + character.len_utf8();
    }
    Some(100 + gaps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_astro::components::{
        ComponentLocation, Framework, Prop, PropDefault, PropKind, Slot,
    };
    use std::path::PathBuf;

    fn component(
        name: &str,
        framework: Framework,
        location: ComponentLocation,
        props: Vec<Prop>,
        wraps: bool,
    ) -> Component {
        Component {
            name: name.into(),
            framework,
            location,
            description: Some(format!("{name} description")),
            props,
            slots: if wraps {
                vec![Slot { name: None }]
            } else {
                Vec::new()
            },
            client_directives: framework.client_directives().to_vec(),
        }
    }

    fn prop(name: &str, required: bool) -> Prop {
        Prop {
            name: name.into(),
            kind: PropKind::String,
            required,
            default: None,
            description: None,
        }
    }

    fn project_component(name: &str, props: Vec<Prop>) -> Component {
        component(
            name,
            Framework::Astro,
            ComponentLocation::Project {
                source: PathBuf::from(format!("/project/src/components/{name}.astro")),
                relative_path: PathBuf::from(format!("src/components/{name}.astro")),
            },
            props,
            false,
        )
    }

    fn package_component(specifier: &str, name: &str, props: Vec<Prop>) -> Component {
        component(
            name,
            Framework::Astro,
            ComponentLocation::Package {
                specifier: specifier.into(),
            },
            props,
            false,
        )
    }

    #[test]
    fn project_components_come_first_then_package_groups() {
        let components = vec![
            package_component("@astrojs/starlight/components", "Tabs", Vec::new()),
            project_component("Zebra", Vec::new()),
            package_component("@astrojs/starlight/components", "Card", Vec::new()),
            project_component("Alpha", Vec::new()),
            package_component("@example/ui", "Widget", Vec::new()),
        ];
        let palette = Palette::new(&components);
        let names: Vec<&str> = palette
            .entries
            .iter()
            .map(|entry| entry.component.name.as_str())
            .collect();
        assert_eq!(names, ["Alpha", "Zebra", "Card", "Tabs", "Widget"]);
        assert_eq!(palette.entries[0].group, ComponentGroup::Project);
        assert_eq!(
            palette.entries[2].group,
            ComponentGroup::Package("@astrojs/starlight/components".into())
        );
        assert_eq!(
            palette.entries[4].group,
            ComponentGroup::Package("@example/ui".into())
        );
        assert_ne!(palette.entries[2].group, palette.entries[1].group);
        assert_eq!(palette.entries[3].group, palette.entries[2].group);
    }

    #[test]
    fn headers_label_each_group_once() {
        let components = vec![
            package_component("@astrojs/starlight/components", "Tabs", Vec::new()),
            project_component("Alpha", Vec::new()),
            project_component("Beta", Vec::new()),
            package_component("@astrojs/starlight/components", "Card", Vec::new()),
            package_component("@example/ui", "Widget", Vec::new()),
        ];
        let palette = Palette::new(&components);
        let headers = palette.headers(&palette.search(""));
        assert_eq!(
            headers,
            vec![
                Some("Project components".into()),
                None,
                Some("@astrojs/starlight/components".into()),
                None,
                Some("@example/ui".into()),
            ]
        );
        // A search that drops the project group labels the first hit's group.
        assert_eq!(
            palette.headers(&palette.search("bet")),
            [Some("Project components".into())]
        );
        assert_eq!(
            palette.headers(&palette.search("card")),
            [Some("@astrojs/starlight/components".into())]
        );
    }

    #[test]
    fn search_ranks_within_project_before_package_groups() {
        let components = vec![
            package_component("@astrojs/starlight/components", "CardGrid", Vec::new()),
            project_component("Cart", Vec::new()),
            project_component("Card", Vec::new()),
            package_component("@astrojs/starlight/components", "Card", Vec::new()),
        ];
        let palette = Palette::new(&components);
        let hits = palette.search("car");
        let names: Vec<&str> = hits
            .iter()
            .map(|position| palette.entries[*position].component.name.as_str())
            .collect();
        // Exact project name first, then the project prefix match, then the
        // package group. A loose subsequence would sort after both.
        assert_eq!(names, ["Card", "Cart", "Card", "CardGrid"]);
        let card_grid = palette.search("cag");
        assert_eq!(card_grid.len(), 1);
        assert_eq!(palette.entries[card_grid[0]].component.name, "CardGrid");
        assert!(palette.search("zzz").is_empty());
        assert_eq!(palette.search("  ").len(), palette.entries.len());
    }

    #[test]
    fn summaries_name_required_props_or_count_optionals() {
        let required = project_component(
            "BookCard",
            vec![
                prop("title", true),
                prop("author", true),
                prop("rating", false),
            ],
        );
        assert_eq!(
            requirement(&required),
            Requirement::Props(vec!["title".into(), "author".into()])
        );
        let optional =
            project_component("Optional", vec![prop("tone", false), prop("count", false)]);
        assert_eq!(requirement(&optional), Requirement::Optional(2));
        assert_eq!(
            requirement(&project_component("Bare", Vec::new())),
            Requirement::None
        );

        let mut defaulted = project_component("Defaulted", vec![prop("tone", false)]);
        defaulted.props[0].default = Some(PropDefault::String("tip".into()));
        let entry = &Palette::new(&[defaulted]).entries[0];
        assert_eq!(entry.badge, "Astro");
        assert_eq!(entry.requirement, Requirement::Optional(1));
        assert_eq!(entry.description(), Some("Defaulted description"));
    }
}
