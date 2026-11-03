//! Static MDX component index, prop model and insertion planning.
//!
//! Everything here is local, bounded static analysis. Project files are read
//! and parsed; nothing is imported, evaluated or executed. Discovery carries
//! the caller's generation ticket so a superseded walk can be dropped before
//! publication.

mod discover;
mod imports;
mod props;
mod snippet;
mod starlight;

pub use discover::{
    ComponentIndex, ComponentIndexError, ComponentIndexRequest, inspect_components,
};
pub use imports::{
    CorroboratedImport, ImportError, ImportPlan, ImportPlanRequest, PlannedValue, plan_imports,
};
pub use snippet::{
    ClientDirectiveValue, PlaceholderKind, Snippet, SnippetError, SnippetPlaceholder, SnippetValue,
    TagRequest, generate_tag,
};
pub use starlight::{STARLIGHT_CATALOG_VERSION, starlight_catalog};

use std::path::{Path, PathBuf};

/// A component framework the builder can insert.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Framework {
    Astro,
    React,
    Vue,
    Svelte,
}

impl Framework {
    /// Detect a framework from a component file extension.
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "astro" => Some(Self::Astro),
            "tsx" | "jsx" => Some(Self::React),
            "vue" => Some(Self::Vue),
            "svelte" => Some(Self::Svelte),
            _ => None,
        }
    }

    /// A short badge label for the palette.
    pub fn label(self) -> &'static str {
        match self {
            Self::Astro => "Astro",
            Self::React => "React",
            Self::Vue => "Vue",
            Self::Svelte => "Svelte",
        }
    }

    /// Whether Astro accepts `client:*` directives on this component.
    pub fn client_directives(self) -> &'static [ClientDirective] {
        match self {
            Self::Astro => &[],
            Self::React | Self::Vue | Self::Svelte => &CLIENT_DIRECTIVES,
        }
    }
}

/// One `client:*` hydration directive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientDirective {
    /// Attribute name, for example `client:load`.
    pub name: &'static str,
    /// Whether the directive requires a value (`client:only`, `client:media`).
    pub requires_value: bool,
}

/// The hydration directives Astro supports for framework components.
pub const CLIENT_DIRECTIVES: [ClientDirective; 5] = [
    ClientDirective {
        name: "client:load",
        requires_value: false,
    },
    ClientDirective {
        name: "client:idle",
        requires_value: false,
    },
    ClientDirective {
        name: "client:visible",
        requires_value: false,
    },
    ClientDirective {
        name: "client:media",
        requires_value: true,
    },
    ClientDirective {
        name: "client:only",
        requires_value: true,
    },
];

/// A prop control family derived from the declared TypeScript type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PropKind {
    String,
    Number,
    Boolean,
    Date,
    StringArray,
    NumberArray,
    /// An `ImageMetadata` value; the form uses the asset service.
    Image,
    /// A `CollectionEntry<'name'>` reference.
    CollectionEntry {
        collection: String,
    },
    /// A string literal union or a string-valued TypeScript enum.
    Enum {
        values: Vec<String>,
    },
}

impl PropKind {
    /// Whether the value is rendered as a quoted attribute.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::String | Self::Date | Self::Enum { .. })
    }

    /// Whether the value is rendered as a JavaScript expression.
    pub fn is_expression(&self) -> bool {
        !self.is_text()
    }
}

/// A literal default discovered by static analysis.
#[derive(Clone, Debug, PartialEq)]
pub enum PropDefault {
    String(String),
    Number(String),
    Boolean(bool),
    List(Vec<String>),
}

/// One component prop.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    pub name: String,
    pub kind: PropKind,
    pub required: bool,
    pub default: Option<PropDefault>,
    pub description: Option<String>,
}

impl Prop {
    /// A prop is optional when it is marked optional or has a default.
    pub fn optional(&self) -> bool {
        !self.required
    }
}

/// A named or default content slot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Slot {
    /// `None` is the default slot.
    pub name: Option<String>,
}

/// Where a component comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentLocation {
    /// A project file, with canonical source and project-relative path.
    Project {
        source: PathBuf,
        relative_path: PathBuf,
    },
    /// A package export.
    Package { specifier: String },
}

/// One index entry. Duplicate names are intentionally separate entries.
#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub name: String,
    pub framework: Framework,
    pub location: ComponentLocation,
    pub description: Option<String>,
    pub props: Vec<Prop>,
    pub slots: Vec<Slot>,
    pub client_directives: Vec<ClientDirective>,
}

impl Component {
    /// Stable identity for the palette and tests.
    pub fn id(&self) -> String {
        match &self.location {
            ComponentLocation::Project { relative_path, .. } => {
                format!("project:{}", relative_path.display())
            }
            ComponentLocation::Package { specifier } => {
                format!("package:{specifier}#{}", self.name)
            }
        }
    }

    /// Whether the component accepts wrapped child content.
    pub fn has_default_slot(&self) -> bool {
        self.slots.iter().any(|slot| slot.name.is_none())
    }

    /// The declared prop with this name.
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|prop| prop.name == name)
    }

    /// The package specifier for a package component.
    pub fn package_specifier(&self) -> Option<&str> {
        match &self.location {
            ComponentLocation::Project { .. } => None,
            ComponentLocation::Package { specifier } => Some(specifier),
        }
    }

    /// The canonical project source for a project component.
    pub fn project_source(&self) -> Option<&Path> {
        match &self.location {
            ComponentLocation::Project { source, .. } => Some(source),
            ComponentLocation::Package { .. } => None,
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
