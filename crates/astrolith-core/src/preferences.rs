//! Scoped, versioned preferences and the project registry.
//!
//! Persistence lives in the desktop application; this module owns the exact
//! on-disk shape and the single inheritance resolver used by every consumer.
//! A missing optional value means inheritance. `Some(false)`, `Some(0.0)` and
//! other explicitly stored values are never mistaken for absence.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Version stored in `global.json`.
pub const GLOBAL_PREFERENCES_VERSION: u32 = 3;
/// Version stored in every per-project preferences file.
pub const PROJECT_PREFERENCES_VERSION: u32 = 2;
/// Version stored in `projects.json`.
pub const PROJECT_REGISTRY_VERSION: u32 = 1;

/// Appearance preference; [`Self::System`] remains distinct from its resolved mode.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    /// Force the light palette.
    Light,
    /// Force the dark palette.
    Dark,
    /// Follow the operating system appearance.
    #[default]
    System,
}

/// Named colour family used in the application interface.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ColorScheme {
    /// Catppuccin Latte.
    #[default]
    CatppuccinLatte,
    /// Catppuccin Frappé.
    CatppuccinFrappe,
    /// Catppuccin Macchiato.
    CatppuccinMacchiato,
    /// Catppuccin Mocha.
    CatppuccinMocha,
    /// Nord.
    Nord,
    /// Nord Light.
    NordLight,
    /// Tokyo Night.
    TokyoNight,
    /// Tokyo Night Storm.
    TokyoNightStorm,
    /// Tokyo Night Moon.
    TokyoNightMoon,
    /// Tokyo Day.
    TokyoDay,
    /// Gruvbox Light.
    GruvboxLight,
    /// Gruvbox Dark.
    GruvboxDark,
}

/// Workspace presentation mode.
///
/// The mode changes layout policy and chrome only; document identity, editor
/// state, history and services stay with the open project.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceMode {
    /// Writing: centered prose with chrome hidden by default.
    #[default]
    Writing,
    /// Build: source, navigation and development tools.
    Build,
    /// Preview: the project's real page through its presentation backend.
    Preview,
}

impl WorkspaceMode {
    /// Every mode in cycling order.
    pub const ALL: [Self; 3] = [Self::Writing, Self::Build, Self::Preview];

    /// Short label used by the toolbar, menus and command palette.
    pub fn label(self) -> &'static str {
        match self {
            Self::Writing => "Writing",
            Self::Build => "Build",
            Self::Preview => "Preview",
        }
    }

    /// The next mode in the writing → build → preview cycle.
    pub fn next(self) -> Self {
        match self {
            Self::Writing => Self::Build,
            Self::Build => Self::Preview,
            Self::Preview => Self::Writing,
        }
    }
}

/// Default extension for newly created content entries.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "lowercase")]
pub enum FileExtension {
    /// Markdown, `.md`.
    #[default]
    Md,
    /// MDX, `.mdx`.
    Mdx,
}

impl FileExtension {
    /// Extension without the leading dot, as stored in file names.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Mdx => "mdx",
        }
    }

    /// Extension including the leading dot.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Md => ".md",
            Self::Mdx => ".mdx",
        }
    }
}

/// How generated image paths are written.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum ImagePathMode {
    /// Resolve from the document location.
    #[default]
    Relative,
    /// Resolve from the project root.
    Absolute,
}

/// Valid idle autosave delays in seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(into = "u64", try_from = "u64")]
pub enum AutosaveDelay {
    /// One second.
    One,
    /// Two seconds; the default.
    #[default]
    Two,
    /// Five seconds.
    Five,
    /// Ten seconds.
    Ten,
}

impl AutosaveDelay {
    /// Delay in whole seconds.
    pub fn seconds(self) -> u64 {
        match self {
            Self::One => 1,
            Self::Two => 2,
            Self::Five => 5,
            Self::Ten => 10,
        }
    }

    /// Parse a supported delay, rejecting every other value.
    pub fn from_seconds(seconds: u64) -> Option<Self> {
        match seconds {
            1 => Some(Self::One),
            2 => Some(Self::Two),
            5 => Some(Self::Five),
            10 => Some(Self::Ten),
            _ => None,
        }
    }
}

impl From<AutosaveDelay> for u64 {
    fn from(delay: AutosaveDelay) -> Self {
        delay.seconds()
    }
}

impl TryFrom<u64> for AutosaveDelay {
    type Error = String;

    fn try_from(seconds: u64) -> Result<Self, Self::Error> {
        Self::from_seconds(seconds)
            .ok_or_else(|| format!("autosave delay must be 1, 2, 5 or 10 seconds, not {seconds}"))
    }
}

/// Persisted dock layout in logical pixels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelLayout {
    /// Whether the left sidebar is open.
    pub sidebar_visible: bool,
    /// Whether the bottom preview dock is open.
    pub preview_visible: bool,
    /// Left sidebar width in logical pixels.
    pub sidebar_width: f32,
    /// Bottom preview height in logical pixels.
    pub preview_height: f32,
}

impl Default for PanelLayout {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            preview_visible: false,
            sidebar_width: 240.,
            preview_height: 160.,
        }
    }
}

impl PanelLayout {
    /// Clamp stored logical sizes so the document keeps usable space.
    pub fn clamp(&mut self, viewport_width: f32) {
        let maximum = ((viewport_width - 320.) / 2.).clamp(160., 400.);
        if !self.sidebar_width.is_finite() {
            self.sidebar_width = 240.;
        }
        self.sidebar_width = self.sidebar_width.clamp(160., maximum);
        if !self.preview_height.is_finite() {
            self.preview_height = 160.;
        }
        self.preview_height = self.preview_height.clamp(120., 400.);
    }

    /// Writing starts as a blank page: every tool is closed.
    fn writing() -> Self {
        Self {
            sidebar_visible: false,
            preview_visible: false,
            sidebar_width: 320.,
            preview_height: 160.,
        }
    }

    /// Build starts with the project library open and tools closed.
    fn build() -> Self {
        Self {
            sidebar_visible: true,
            preview_visible: false,
            sidebar_width: 224.,
            preview_height: 160.,
        }
    }

    /// Preview reads the document; the Astro preview dock starts closed.
    fn preview() -> Self {
        Self {
            sidebar_visible: false,
            preview_visible: false,
            sidebar_width: 224.,
            preview_height: 160.,
        }
    }
}

/// Layout policy for each workspace mode.
///
/// Modes own separate geometry and visibility so switching never reuses the
/// previous mode's composition. Inspector visibility is transient and is
/// never persisted through this type.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeLayouts {
    /// Writing layout.
    pub writing: PanelLayout,
    /// Build layout.
    pub build: PanelLayout,
    /// Preview layout.
    pub preview: PanelLayout,
}

impl Default for ModeLayouts {
    fn default() -> Self {
        Self {
            writing: PanelLayout::writing(),
            build: PanelLayout::build(),
            preview: PanelLayout::preview(),
        }
    }
}

impl ModeLayouts {
    /// Layout for one mode.
    pub fn get(&self, mode: WorkspaceMode) -> &PanelLayout {
        match mode {
            WorkspaceMode::Writing => &self.writing,
            WorkspaceMode::Build => &self.build,
            WorkspaceMode::Preview => &self.preview,
        }
    }

    /// Mutable layout for one mode.
    pub fn get_mut(&mut self, mode: WorkspaceMode) -> &mut PanelLayout {
        match mode {
            WorkspaceMode::Writing => &mut self.writing,
            WorkspaceMode::Build => &mut self.build,
            WorkspaceMode::Preview => &mut self.preview,
        }
    }

    /// Replace one mode's layout.
    pub fn set(&mut self, mode: WorkspaceMode, layout: PanelLayout) {
        *self.get_mut(mode) = layout;
    }

    /// Clamp every mode against one viewport width.
    pub fn clamp(&mut self, viewport_width: f32) {
        for mode in WorkspaceMode::ALL {
            self.get_mut(mode).clamp(viewport_width);
        }
    }

    /// Overlay the fields a project explicitly configured.
    pub fn resolve_with(&self, overrides: &ModeLayoutOverrides) -> Self {
        let mut resolved = self.clone();
        for mode in WorkspaceMode::ALL {
            if let Some(layout) = overrides.get(mode) {
                resolved.set(mode, layout.clone());
            }
        }
        resolved
    }
}

/// Per-mode layout overrides stored in a project scope.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModeLayoutOverrides {
    /// Writing override, when the project recorded one.
    pub writing: Option<PanelLayout>,
    /// Build override, when the project recorded one.
    pub build: Option<PanelLayout>,
    /// Preview override, when the project recorded one.
    pub preview: Option<PanelLayout>,
}

impl ModeLayoutOverrides {
    /// The override for one mode.
    pub fn get(&self, mode: WorkspaceMode) -> Option<&PanelLayout> {
        match mode {
            WorkspaceMode::Writing => self.writing.as_ref(),
            WorkspaceMode::Build => self.build.as_ref(),
            WorkspaceMode::Preview => self.preview.as_ref(),
        }
    }

    /// Replace one mode's override.
    pub fn set(&mut self, mode: WorkspaceMode, layout: Option<PanelLayout>) {
        match mode {
            WorkspaceMode::Writing => self.writing = layout,
            WorkspaceMode::Build => self.build = layout,
            WorkspaceMode::Preview => self.preview = layout,
        }
    }
}

/// Optional heading colors, one per theme.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeadingColors {
    /// Heading color used by the light palette; `None` keeps the theme token.
    pub light: Option<String>,
    /// Heading color used by the dark palette; `None` keeps the theme token.
    pub dark: Option<String>,
}

/// Copyedit category identifiers; the five categories are the complete set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum CopyeditCategory {
    /// Nouns.
    Noun,
    /// Verbs.
    Verb,
    /// Adjectives.
    Adjective,
    /// Adverbs.
    Adverb,
    /// Conjunctions.
    Conjunction,
}

impl CopyeditCategory {
    /// Every category in stable display order.
    pub const ALL: [Self; 5] = [
        Self::Noun,
        Self::Verb,
        Self::Adjective,
        Self::Adverb,
        Self::Conjunction,
    ];
}

/// One persisted color per copyedit category.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyeditColors {
    /// Noun highlight color.
    pub noun: String,
    /// Verb highlight color.
    pub verb: String,
    /// Adjective highlight color.
    pub adjective: String,
    /// Adverb highlight color.
    pub adverb: String,
    /// Conjunction highlight color.
    pub conjunction: String,
}

impl Default for CopyeditColors {
    fn default() -> Self {
        Self {
            noun: "#4ec9b0".into(),
            verb: "#569cd6".into(),
            adjective: "#dcdcaa".into(),
            adverb: "#c586c0".into(),
            conjunction: "#ce9178".into(),
        }
    }
}

impl CopyeditColors {
    /// Color for one category.
    pub fn color(&self, category: CopyeditCategory) -> &str {
        match category {
            CopyeditCategory::Noun => &self.noun,
            CopyeditCategory::Verb => &self.verb,
            CopyeditCategory::Adjective => &self.adjective,
            CopyeditCategory::Adverb => &self.adverb,
            CopyeditCategory::Conjunction => &self.conjunction,
        }
    }
}

/// Copyedit toggles and colors. All categories start disabled, matching the
/// persisted upstream default; the display layer owns the translated labels.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CopyeditPreferences {
    /// Highlight nouns.
    pub nouns: bool,
    /// Highlight verbs.
    pub verbs: bool,
    /// Highlight adjectives.
    pub adjectives: bool,
    /// Highlight adverbs.
    pub adverbs: bool,
    /// Highlight conjunctions.
    pub conjunctions: bool,
    /// Per-category colors.
    pub colors: CopyeditColors,
}

impl CopyeditPreferences {
    /// Whether one category is enabled.
    pub fn enabled(&self, category: CopyeditCategory) -> bool {
        match category {
            CopyeditCategory::Noun => self.nouns,
            CopyeditCategory::Verb => self.verbs,
            CopyeditCategory::Adjective => self.adjectives,
            CopyeditCategory::Adverb => self.adverbs,
            CopyeditCategory::Conjunction => self.conjunctions,
        }
    }

    /// Enable or disable one category without touching the others.
    pub fn set_enabled(&mut self, category: CopyeditCategory, enabled: bool) {
        match category {
            CopyeditCategory::Noun => self.nouns = enabled,
            CopyeditCategory::Verb => self.verbs = enabled,
            CopyeditCategory::Adjective => self.adjectives = enabled,
            CopyeditCategory::Adverb => self.adverbs = enabled,
            CopyeditCategory::Conjunction => self.conjunctions = enabled,
        }
    }

    /// True only when every category is enabled.
    pub fn all_enabled(&self) -> bool {
        CopyeditCategory::ALL
            .iter()
            .all(|category| self.enabled(*category))
    }

    /// Set every category to the same state.
    pub fn set_all(&mut self, enabled: bool) {
        for category in CopyeditCategory::ALL {
            self.set_enabled(category, enabled);
        }
    }
}

/// IDE launch command as argv; arguments are never interpreted by a shell.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdeCommand {
    /// Executable path or program name.
    pub executable: String,
    /// Arguments supporting `{project}`, `{file}`, `{line}` and `{column}`.
    pub arguments: Vec<String>,
}

impl IdeCommand {
    /// True when an executable is configured.
    pub fn is_configured(&self) -> bool {
        !self.executable.trim().is_empty()
    }
}

/// Where a preview URL is presented.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Hash, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum PreviewPresentation {
    /// The user's browser, opened through the desktop opener.
    #[default]
    External,
    /// A WebView embedded in the workspace split.
    Embedded,
}

/// Preferred browser as argv; the executable is started without a shell.
///
/// An empty executable means the platform's default opener. Every argument may
/// contain `{url}`; when no argument does, the URL is appended as the final
/// argument.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserCommand {
    /// Executable path or program name.
    pub executable: String,
    /// Arguments; `{url}` is replaced with the preview URL.
    pub arguments: Vec<String>,
}

impl BrowserCommand {
    /// True when a browser executable is configured.
    pub fn is_configured(&self) -> bool {
        !self.executable.trim().is_empty()
    }

    /// Render the argv for diagnostics.
    pub fn display(&self) -> String {
        let mut rendered = self.executable.clone();
        for argument in &self.arguments {
            rendered.push(' ');
            rendered.push_str(argument);
        }
        rendered
    }
}

/// Preview presentation, preferred browser and entry behaviour.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewPreferences {
    /// Which surface presents the resolved URL.
    pub presentation: PreviewPresentation,
    /// Preferred external browser argv; empty uses the desktop opener.
    pub browser: BrowserCommand,
    /// Open the active entry once when the user explicitly enters Preview.
    pub open_on_enter: bool,
}

impl Default for PreviewPreferences {
    fn default() -> Self {
        Self {
            presentation: PreviewPresentation::External,
            browser: BrowserCommand::default(),
            open_on_enter: false,
        }
    }
}

/// Persisted main-window geometry, restored on the next launch.
///
/// Values are logical pixels in desktop coordinates; the caller enforces the
/// 1000×700 minimum when restoring.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeometry {
    /// Desktop x origin.
    pub x: f32,
    /// Desktop y origin.
    pub y: f32,
    /// Window width in logical pixels.
    pub width: f32,
    /// Window height in logical pixels.
    pub height: f32,
    /// Whether the window was maximized; the bounds remain the restore size.
    pub maximized: bool,
}

impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            width: 0.,
            height: 0.,
            maximized: false,
        }
    }
}

/// Application-wide preference scope. Persisted as `global.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GlobalPreferencesV2 {
    /// Envelope version; always [`GLOBAL_PREFERENCES_VERSION`] when written.
    pub version: u32,
    /// Light, dark or system appearance.
    pub theme: ThemePreference,
    /// Named colour palette.
    pub color_scheme: ColorScheme,
    /// Interface language selected by the user (`en`, `zh-CN` or `ja`).
    pub language: String,
    /// Mode restored when no project records a last mode.
    pub default_mode: WorkspaceMode,
    /// Global per-mode layout defaults; project overrides win per mode.
    pub layouts: ModeLayouts,
    /// Last main-window geometry; absent opens the platform default.
    pub window: Option<WindowGeometry>,
    /// Optional light and dark heading colors.
    pub heading_colors: HeadingColors,
    /// Interface font family; empty uses the platform UI font.
    pub ui_font_family: String,
    /// Interface base font size in logical pixels; controls the rem scale.
    pub ui_font_size: f32,
    /// Shared corner radius in logical pixels for panels and components.
    pub panel_radius: f32,
    /// Document font family; empty uses the mock's sans face.
    pub prose_font_family: String,
    /// Document font size in logical pixels.
    pub editor_font_size: f32,
    /// Build source editor family; empty keeps the platform monospace family.
    pub build_font_family: String,
    /// Build source editor font size in logical pixels.
    pub build_font_size: f32,
    /// Preview document family; empty uses the preferred sans family.
    pub preview_font_family: String,
    /// Preview document font size in logical pixels.
    pub preview_font_size: f32,
    /// Document line height as a multiple of the font size.
    pub editor_line_height: f32,
    /// Page width as a percentage of the editor pane.
    pub page_width_percent: f32,
    /// Dim every sentence except the cursor sentence.
    pub focus_mode: bool,
    /// Keep the cursor line vertically centred.
    pub typewriter_mode: bool,
    /// Idle autosave delay.
    pub autosave_delay: AutosaveDelay,
    /// Extension used for new entries unless a narrower scope overrides it.
    pub default_extension: FileExtension,
    /// IDE executable and argv template.
    pub ide: IdeCommand,
    /// Copyedit toggles and colors.
    pub copyedit: CopyeditPreferences,
    /// Preview presentation, preferred browser and entry behaviour.
    pub preview: PreviewPreferences,
    /// Release version the user chose to skip in the update check.
    pub skipped_update_version: Option<String>,
}

impl Default for GlobalPreferencesV2 {
    fn default() -> Self {
        Self {
            version: GLOBAL_PREFERENCES_VERSION,
            theme: ThemePreference::System,
            color_scheme: ColorScheme::default(),
            language: "en".to_owned(),
            default_mode: WorkspaceMode::default(),
            layouts: ModeLayouts::default(),
            window: None,
            heading_colors: HeadingColors::default(),
            ui_font_family: String::new(),
            ui_font_size: 18.,
            panel_radius: 10.,
            prose_font_family: String::new(),
            editor_font_size: 16.,
            build_font_family: String::new(),
            build_font_size: 14.,
            preview_font_family: String::new(),
            preview_font_size: 16.,
            editor_line_height: 2.,
            page_width_percent: 64.,
            focus_mode: false,
            typewriter_mode: false,
            autosave_delay: AutosaveDelay::default(),
            default_extension: FileExtension::default(),
            ide: IdeCommand::default(),
            copyedit: CopyeditPreferences::default(),
            preview: PreviewPreferences::default(),
            skipped_update_version: None,
        }
    }
}

/// Frontmatter field-name mappings. Each absent value inherits the next scope.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FieldMappings {
    /// Field mapped to the entry title.
    pub title: Option<String>,
    /// Field mapped to the publication date.
    pub date: Option<String>,
    /// Field mapped to the description.
    pub description: Option<String>,
    /// Field mapped to the draft flag.
    pub draft: Option<String>,
}

/// Project-scope preferences. Persisted as `projects/<project-id>.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectPreferencesV1 {
    /// Envelope version; always [`PROJECT_PREFERENCES_VERSION`] when written.
    pub version: u32,
    /// Registry identity of the owning project.
    pub project_id: String,
    /// Canonical project root at the last open.
    pub path: String,
    /// Mode restored for this project before the global default.
    pub last_mode: Option<WorkspaceMode>,
    /// Layout overrides for this project; absent modes inherit global values.
    pub layout_overrides: ModeLayoutOverrides,
    /// Project-relative source of the entry reopened on launch.
    pub last_entry: Option<String>,
    /// Content directory override.
    pub content_directory: Option<String>,
    /// Assets directory override.
    pub assets_directory: Option<String>,
    /// MDX component directory override; project-only by design.
    pub mdx_component_directory: Option<String>,
    /// Relative or project-root image paths; absent inherits the global default.
    pub image_path_mode: Option<ImagePathMode>,
    /// New-entry extension override.
    pub default_extension: Option<FileExtension>,
    /// Frontmatter field mappings.
    pub field_mappings: FieldMappings,
    /// Experimental MDX import insertion; `Some(false)` is an explicit choice.
    pub insert_mdx_imports: Option<bool>,
    /// Per-collection overrides keyed by collection identity.
    pub collections: BTreeMap<String, CollectionPreferencesV1>,
}

impl Default for ProjectPreferencesV1 {
    fn default() -> Self {
        Self {
            version: PROJECT_PREFERENCES_VERSION,
            project_id: String::new(),
            path: String::new(),
            last_mode: None,
            layout_overrides: ModeLayoutOverrides::default(),
            last_entry: None,
            content_directory: None,
            assets_directory: None,
            mdx_component_directory: None,
            image_path_mode: None,
            default_extension: None,
            field_mappings: FieldMappings::default(),
            insert_mdx_imports: None,
            collections: BTreeMap::new(),
        }
    }
}

impl ProjectPreferencesV1 {
    /// Create the empty preferences for a newly registered project.
    pub fn for_project(project_id: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            project_id: project_id.into(),
            path: path.into(),
            ..Self::default()
        }
    }

    /// Collection override by exact identity, when configured.
    pub fn collection(&self, collection: &str) -> Option<&CollectionPreferencesV1> {
        self.collections.get(collection)
    }
}

/// Collection-scope overrides stored inside the owning project file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectionPreferencesV1 {
    /// Envelope version; always [`PROJECT_PREFERENCES_VERSION`] when written.
    pub version: u32,
    /// Content directory override.
    pub content_directory: Option<String>,
    /// Assets directory override.
    pub assets_directory: Option<String>,
    /// Relative or project-root image paths; absent inherits the project then global value.
    pub image_path_mode: Option<ImagePathMode>,
    /// New-entry extension override.
    pub default_extension: Option<FileExtension>,
    /// Frontmatter field mappings overriding the project mapping per field.
    pub field_mappings: FieldMappings,
    /// URL pattern for Content Finder links; `{slug}` is URL-encoded.
    pub content_finder_url_pattern: Option<String>,
}

impl Default for CollectionPreferencesV1 {
    fn default() -> Self {
        Self {
            version: PROJECT_PREFERENCES_VERSION,
            content_directory: None,
            assets_directory: None,
            image_path_mode: None,
            default_extension: None,
            field_mappings: FieldMappings::default(),
            content_finder_url_pattern: None,
        }
    }
}

/// Immutable result of one resolution pass. Consumers never merge scopes again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPreferences {
    /// Appearance preference.
    pub theme: ThemePreference,
    /// Mode used when this project has no recorded last mode.
    pub default_mode: WorkspaceMode,
    /// Mode restored for the open project, if it recorded one.
    pub last_mode: Option<WorkspaceMode>,
    /// Effective layout for every mode.
    pub layouts: ModeLayouts,
    /// Heading colors.
    pub heading_colors: HeadingColors,
    /// Interface font family; empty uses the platform UI font.
    pub ui_font_family: String,
    /// Interface base font size in logical pixels.
    pub ui_font_size: f32,
    /// Document font family; empty uses the mock's sans face.
    pub prose_font_family: String,
    /// Document font size.
    pub editor_font_size: f32,
    /// Build source editor family; empty keeps the platform monospace family.
    pub build_font_family: String,
    /// Build source editor font size.
    pub build_font_size: f32,
    /// Preview document family; empty uses the preferred sans family.
    pub preview_font_family: String,
    /// Preview document font size.
    pub preview_font_size: f32,
    /// Document line height as a multiple of the font size.
    pub editor_line_height: f32,
    /// Page width as a percentage of the editor pane.
    pub page_width_percent: f32,
    /// Focus and typewriter presentation.
    pub focus_mode: bool,
    /// Keep the cursor line vertically centred.
    pub typewriter_mode: bool,
    /// Idle autosave delay.
    pub autosave_delay: AutosaveDelay,
    /// Effective new-entry extension.
    pub default_extension: FileExtension,
    /// IDE argv template.
    pub ide: IdeCommand,
    /// Copyedit toggles and colors.
    pub copyedit: CopyeditPreferences,
    /// Preview presentation, preferred browser and entry behaviour.
    pub preview: PreviewPreferences,
    /// Effective content directory, if configured.
    pub content_directory: Option<String>,
    /// Effective assets directory, if configured.
    pub assets_directory: Option<String>,
    /// Effective MDX component directory, if configured.
    pub mdx_component_directory: Option<String>,
    /// Effective image path mode.
    pub image_path_mode: ImagePathMode,
    /// Effective frontmatter mappings, largest scope first.
    pub field_mappings: FieldMappings,
    /// Effective MDX import insertion choice, if the project configured one.
    pub insert_mdx_imports: Option<bool>,
    /// Effective Content Finder URL pattern.
    pub content_finder_url_pattern: Option<String>,
    /// Entry to reopen for the project.
    pub last_entry: Option<String>,
}

/// Resolve collection over project over global, once.
pub fn resolve(
    global: &GlobalPreferencesV2,
    project: Option<&ProjectPreferencesV1>,
    collection: Option<&CollectionPreferencesV1>,
) -> ResolvedPreferences {
    let project_field = |pick: fn(&FieldMappings) -> &Option<String>| {
        project.and_then(|project| pick(&project.field_mappings).clone())
    };
    let collection_field = |pick: fn(&FieldMappings) -> &Option<String>| {
        collection
            .and_then(|collection| pick(&collection.field_mappings).clone())
            .or_else(|| project_field(pick))
    };
    ResolvedPreferences {
        theme: global.theme,
        default_mode: global.default_mode,
        last_mode: project.and_then(|project| project.last_mode),
        layouts: project
            .map(|project| global.layouts.resolve_with(&project.layout_overrides))
            .unwrap_or_else(|| global.layouts.clone()),
        heading_colors: global.heading_colors.clone(),
        ui_font_family: global.ui_font_family.clone(),
        ui_font_size: global.ui_font_size,
        prose_font_family: global.prose_font_family.clone(),
        editor_font_size: global.editor_font_size,
        build_font_family: global.build_font_family.clone(),
        build_font_size: global.build_font_size,
        preview_font_family: global.preview_font_family.clone(),
        preview_font_size: global.preview_font_size,
        editor_line_height: global.editor_line_height,
        page_width_percent: global.page_width_percent,
        focus_mode: global.focus_mode,
        typewriter_mode: global.typewriter_mode,
        autosave_delay: global.autosave_delay,
        default_extension: collection
            .and_then(|collection| collection.default_extension)
            .or_else(|| project.and_then(|project| project.default_extension))
            .unwrap_or(global.default_extension),
        ide: global.ide.clone(),
        copyedit: global.copyedit.clone(),
        preview: global.preview.clone(),
        content_directory: collection
            .and_then(|collection| collection.content_directory.clone())
            .or_else(|| project.and_then(|project| project.content_directory.clone())),
        assets_directory: collection
            .and_then(|collection| collection.assets_directory.clone())
            .or_else(|| project.and_then(|project| project.assets_directory.clone())),
        mdx_component_directory: project
            .and_then(|project| project.mdx_component_directory.clone()),
        image_path_mode: collection
            .and_then(|collection| collection.image_path_mode)
            .or_else(|| project.and_then(|project| project.image_path_mode))
            .unwrap_or(ImagePathMode::Relative),
        field_mappings: FieldMappings {
            title: collection_field(|mappings| &mappings.title),
            date: collection_field(|mappings| &mappings.date),
            description: collection_field(|mappings| &mappings.description),
            draft: collection_field(|mappings| &mappings.draft),
        },
        insert_mdx_imports: project.and_then(|project| project.insert_mdx_imports),
        content_finder_url_pattern: collection
            .and_then(|collection| collection.content_finder_url_pattern.clone()),
        last_entry: project.and_then(|project| project.last_entry.clone()),
    }
}

/// One project registered by the application.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectRecord {
    /// Stable registry identity; preserved across a moved-path rebind.
    pub id: String,
    /// Canonical project root.
    pub path: String,
    /// User-facing name, derived from the package name or directory name.
    pub display_name: String,
    /// Declared package name; required for moved-path rebinding.
    pub package_name: Option<String>,
    /// Unix timestamp in seconds of the most recent open.
    pub last_opened_at: u64,
}

/// Registry of recently opened projects. Persisted as `projects.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectRegistry {
    /// Envelope version; always [`PROJECT_REGISTRY_VERSION`] when written.
    pub version: u32,
    /// Registered projects in insertion order; use [`Self::recent`] for display.
    pub projects: Vec<ProjectRecord>,
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self {
            version: PROJECT_REGISTRY_VERSION,
            projects: Vec::new(),
        }
    }
}

/// Outcome of recording an open project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectOpen {
    /// Stable registry identity.
    pub id: String,
    /// Canonical path now recorded.
    pub path: String,
    /// Display name now recorded.
    pub display_name: String,
    /// Package name now recorded.
    pub package_name: Option<String>,
    /// Previous path when this open rebound a moved project.
    pub rebound_from: Option<String>,
}

impl ProjectRegistry {
    /// Record an open, rebinding a moved project only when its recorded path is
    /// gone and exactly one registered project shares the package name.
    pub fn record_open(
        &mut self,
        path: &str,
        package_name: Option<&str>,
        opened_at: u64,
        old_path_exists: impl Fn(&str) -> bool,
    ) -> ProjectOpen {
        let path = normalize_path(path);
        let package_name = package_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        if let Some(index) = self
            .projects
            .iter()
            .position(|project| normalize_path(&project.path) == path)
        {
            let project = &mut self.projects[index];
            project.path = path.clone();
            project.last_opened_at = opened_at;
            if let Some(package) = &package_name {
                project.package_name = Some(package.clone());
                if project.display_name.trim().is_empty() {
                    project.display_name = display_name_for(package, &path);
                }
            }
            return ProjectOpen {
                id: project.id.clone(),
                path,
                display_name: project.display_name.clone(),
                package_name: project.package_name.clone(),
                rebound_from: None,
            };
        }
        let rebound = package_name.as_ref().and_then(|package| {
            let candidates: Vec<usize> = self
                .projects
                .iter()
                .enumerate()
                .filter(|(_, project)| {
                    project.package_name.as_deref() == Some(package)
                        && normalize_path(&project.path) != path
                        && !old_path_exists(&project.path)
                })
                .map(|(index, _)| index)
                .collect();
            match candidates.as_slice() {
                [index] => Some(*index),
                _ => None,
            }
        });
        if let Some(index) = rebound {
            let project = &mut self.projects[index];
            let previous = std::mem::replace(&mut project.path, path.clone());
            project.last_opened_at = opened_at;
            return ProjectOpen {
                id: project.id.clone(),
                path,
                display_name: project.display_name.clone(),
                package_name: project.package_name.clone(),
                rebound_from: Some(previous),
            };
        }
        let display_name = display_name_for(package_name.as_deref().unwrap_or_default(), &path);
        let mut id = project_id(package_name.as_deref().unwrap_or(&display_name), &path);
        let mut collision = 1;
        while self.projects.iter().any(|project| project.id == id) {
            id = format!("{id}-{collision}");
            collision += 1;
        }
        self.projects.push(ProjectRecord {
            id: id.clone(),
            path: path.clone(),
            display_name: display_name.clone(),
            package_name: package_name.clone(),
            last_opened_at: opened_at,
        });
        ProjectOpen {
            id,
            path,
            display_name,
            package_name,
            rebound_from: None,
        }
    }

    /// Registered project by canonical path.
    pub fn get_by_path(&self, path: &str) -> Option<&ProjectRecord> {
        let path = normalize_path(path);
        self.projects
            .iter()
            .find(|project| normalize_path(&project.path) == path)
    }

    /// Projects ordered by most recently opened, then identity.
    pub fn recent(&self) -> Vec<&ProjectRecord> {
        let mut projects: Vec<&ProjectRecord> = self.projects.iter().collect();
        projects.sort_by(|left, right| {
            right
                .last_opened_at
                .cmp(&left.last_opened_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        projects
    }

    /// The registered project that owns `path`, matched by longest ancestor.
    ///
    /// A deep link or startup file must resolve to exactly one registered root;
    /// a path outside every root has no owner.
    pub fn owning_project(&self, path: &str) -> Option<&ProjectRecord> {
        let path = normalize_path(path);
        self.projects
            .iter()
            .filter(|project| path_is_within(&normalize_path(&project.path), &path))
            .max_by_key(|project| normalize_path(&project.path).len())
    }

    /// Remove one entry from the recent list; returns whether it existed.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.projects.len();
        self.projects.retain(|project| project.id != id);
        self.projects.len() != before
    }
}

/// Whether `path` is `root` itself or lies beneath it at a component boundary.
fn path_is_within(root: &str, path: &str) -> bool {
    if cfg!(windows) {
        Path::new(&path.to_lowercase()).starts_with(Path::new(&root.to_lowercase()))
    } else {
        Path::new(path).starts_with(root)
    }
}

/// Stable identity from a display/package name plus a hash of the canonical path.
///
/// The hash keeps simultaneous same-name projects distinct. A moved project
/// keeps its existing identity through the registry rather than regenerating it.
pub fn project_id(name: &str, path: &str) -> String {
    let slug = slugify(name);
    let hash = fnv1a_64(normalize_path(path).as_bytes());
    format!("{slug}-{hash:016x}")
}

fn display_name_for(package_name: &str, path: &str) -> String {
    if !package_name.trim().is_empty() {
        return package_name.to_owned();
    }
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "project".into())
}

fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if separator && !slug.is_empty() {
                slug.push('-');
            }
            separator = false;
            slug.push(character.to_ascii_lowercase());
            if slug.len() >= 48 {
                break;
            }
        } else {
            separator = true;
        }
    }
    if slug.is_empty() {
        "project".into()
    } else {
        slug
    }
}

fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Lexically normalize a path for registry comparisons.
///
/// Storage still uses the filesystem-canonical path supplied by the store; this
/// only removes `.`/`..` components and duplicate separators so equivalent
/// spellings compare equal.
pub fn normalize_path(path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => normalized.push(component.as_os_str()),
            },
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copyedit_defaults_to_disabled_and_tracks_each_category() {
        let mut copyedit = CopyeditPreferences::default();
        assert!(!copyedit.all_enabled());
        for category in CopyeditCategory::ALL {
            assert!(!copyedit.enabled(category));
        }

        copyedit.set_enabled(CopyeditCategory::Verb, true);
        assert!(copyedit.enabled(CopyeditCategory::Verb));
        assert!(!copyedit.enabled(CopyeditCategory::Noun));
        assert!(!copyedit.all_enabled());

        copyedit.set_all(true);
        assert!(copyedit.all_enabled());
        for category in CopyeditCategory::ALL {
            assert!(copyedit.enabled(category));
        }

        copyedit.set_all(false);
        assert!(!copyedit.all_enabled());
    }

    #[test]
    fn skipped_update_version_is_optional_and_survives_round_trips() {
        let mut stored =
            serde_json::to_value(GlobalPreferencesV2::default()).expect("serialize preferences");
        assert!(
            stored.get("skipped_update_version").is_some(),
            "the field must be written"
        );
        stored
            .as_object_mut()
            .expect("preferences object")
            .remove("skipped_update_version");
        let loaded: GlobalPreferencesV2 =
            serde_json::from_value(stored).expect("older files omit the field");
        assert_eq!(loaded.skipped_update_version, None);

        let chosen = GlobalPreferencesV2 {
            skipped_update_version: Some("1.2.3".into()),
            ..GlobalPreferencesV2::default()
        };
        let reloaded: GlobalPreferencesV2 =
            serde_json::from_value(serde_json::to_value(chosen).expect("serialize"))
                .expect("deserialize");
        assert_eq!(reloaded.skipped_update_version.as_deref(), Some("1.2.3"));
    }

    #[test]
    fn explicit_false_and_zero_survive_inheritance() {
        let global = GlobalPreferencesV2::default();
        let project = ProjectPreferencesV1 {
            insert_mdx_imports: Some(false),
            last_entry: Some("blog/first.md".into()),
            layout_overrides: ModeLayoutOverrides {
                writing: Some(PanelLayout {
                    sidebar_width: 0.,
                    preview_visible: true,
                    ..PanelLayout::default()
                }),
                ..ModeLayoutOverrides::default()
            },
            ..ProjectPreferencesV1::default()
        };
        let resolved = resolve(&global, Some(&project), None);
        assert_eq!(resolved.insert_mdx_imports, Some(false));
        assert_eq!(resolved.layouts.writing.sidebar_width, 0.);
        assert!(resolved.layouts.writing.preview_visible);
        assert_eq!(resolved.layouts.build, ModeLayouts::default().build);
        assert_eq!(resolved.last_entry.as_deref(), Some("blog/first.md"));
    }

    #[test]
    fn workspace_modes_default_to_a_blank_page_and_resolve_one_mode_at_a_time() {
        let defaults = GlobalPreferencesV2::default();
        assert_eq!(defaults.default_mode, WorkspaceMode::Writing);
        assert!(!defaults.layouts.writing.sidebar_visible);
        assert!(!defaults.layouts.writing.preview_visible);
        assert!(defaults.layouts.build.sidebar_visible);
        assert!(
            !defaults.layouts.preview.preview_visible,
            "Preview opens on the rendered document, not the Astro preview dock"
        );

        let global = GlobalPreferencesV2 {
            default_mode: WorkspaceMode::Preview,
            ..GlobalPreferencesV2::default()
        };
        let project = ProjectPreferencesV1 {
            last_mode: Some(WorkspaceMode::Build),
            layout_overrides: ModeLayoutOverrides {
                build: Some(PanelLayout {
                    sidebar_visible: false,
                    sidebar_width: 310.,
                    ..PanelLayout::default()
                }),
                ..ModeLayoutOverrides::default()
            },
            ..ProjectPreferencesV1::default()
        };
        let resolved = resolve(&global, Some(&project), None);
        assert_eq!(resolved.default_mode, WorkspaceMode::Preview);
        assert_eq!(resolved.last_mode, Some(WorkspaceMode::Build));
        assert_eq!(resolved.layouts.build.sidebar_width, 310.);
        assert!(!resolved.layouts.build.sidebar_visible);
        assert_eq!(
            resolved.layouts.writing,
            ModeLayouts::default().writing,
            "unoverridden modes still inherit the global layout"
        );
    }

    #[test]
    fn collection_mappings_and_directories_override_project_per_field() {
        let global = GlobalPreferencesV2 {
            default_extension: FileExtension::Md,
            ..GlobalPreferencesV2::default()
        };
        let project = ProjectPreferencesV1 {
            content_directory: Some("src/project".into()),
            assets_directory: Some("src/assets".into()),
            image_path_mode: Some(ImagePathMode::Absolute),
            default_extension: Some(FileExtension::Mdx),
            field_mappings: FieldMappings {
                title: Some("title".into()),
                date: Some("pubDate".into()),
                description: Some("summary".into()),
                draft: Some("draft".into()),
            },
            ..ProjectPreferencesV1::default()
        };
        let collection = CollectionPreferencesV1 {
            content_directory: Some("src/collection".into()),
            image_path_mode: Some(ImagePathMode::Relative),
            field_mappings: FieldMappings {
                title: Some("name".into()),
                ..FieldMappings::default()
            },
            ..CollectionPreferencesV1::default()
        };
        let inherited = resolve(&global, Some(&project), None);
        assert_eq!(inherited.default_extension, FileExtension::Mdx);
        assert_eq!(inherited.image_path_mode, ImagePathMode::Absolute);
        assert_eq!(inherited.content_directory.as_deref(), Some("src/project"));
        assert_eq!(inherited.field_mappings.title.as_deref(), Some("title"));

        let overridden = resolve(&global, Some(&project), Some(&collection));
        assert_eq!(overridden.default_extension, FileExtension::Mdx);
        assert_eq!(overridden.image_path_mode, ImagePathMode::Relative);
        assert_eq!(
            overridden.content_directory.as_deref(),
            Some("src/collection")
        );
        assert_eq!(overridden.assets_directory.as_deref(), Some("src/assets"));
        assert_eq!(overridden.field_mappings.title.as_deref(), Some("name"));
        assert_eq!(
            overridden.field_mappings.date.as_deref(),
            Some("pubDate"),
            "an absent collection mapping inherits the project value"
        );
    }

    #[test]
    fn copyedit_preferences_round_trip_through_json() {
        let mut global = GlobalPreferencesV2::default();
        global.copyedit.set_enabled(CopyeditCategory::Verb, true);
        global.copyedit.colors.verb = "#123456".into();
        let encoded = serde_json::to_string(&global).unwrap();
        let decoded: GlobalPreferencesV2 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, global);
        assert!(decoded.copyedit.enabled(CopyeditCategory::Verb));
        assert!(!decoded.copyedit.all_enabled());
        assert_eq!(
            decoded.copyedit.colors.color(CopyeditCategory::Verb),
            "#123456"
        );
    }

    #[test]
    fn preview_preferences_default_to_external_and_round_trip() {
        let defaults = PreviewPreferences::default();
        assert_eq!(defaults.presentation, PreviewPresentation::External);
        assert!(!defaults.browser.is_configured());
        assert!(!defaults.open_on_enter);

        let mut global = GlobalPreferencesV2::default();
        global.preview.presentation = PreviewPresentation::Embedded;
        global.preview.browser = BrowserCommand {
            executable: "firefox".into(),
            arguments: vec!["--new-window".into(), "{url}".into()],
        };
        global.preview.open_on_enter = true;
        let encoded = serde_json::to_string(&global).unwrap();
        let decoded: GlobalPreferencesV2 = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.preview, global.preview);

        let resolved = resolve(&decoded, None, None);
        assert_eq!(resolved.preview, global.preview);
    }

    #[test]
    fn path_normalization_matches_equivalent_spellings() {
        assert_eq!(
            normalize_path("/projects/astro/./fixture/"),
            "/projects/astro/fixture"
        );
        assert_eq!(
            normalize_path("/projects/astro/fixture/../fixture"),
            "/projects/astro/fixture"
        );
        assert_eq!(normalize_path("/projects/"), "/projects");
    }

    #[test]
    fn moved_project_rebinds_only_when_the_old_path_is_missing() {
        let mut registry = ProjectRegistry::default();
        let first = registry.record_open("/projects/one/astro", Some("astro-site"), 10, |_| true);
        let moved = registry.record_open("/projects/two/astro", Some("astro-site"), 20, |_| false);
        assert_eq!(moved.id, first.id);
        assert_eq!(moved.rebound_from.as_deref(), Some("/projects/one/astro"));
        assert_eq!(registry.projects.len(), 1);
        assert_eq!(registry.projects[0].path, "/projects/two/astro");

        let clone = registry.record_open("/projects/three/astro", Some("astro-site"), 30, |_| true);
        assert_ne!(clone.id, first.id);
        assert_eq!(clone.rebound_from, None);
        assert_eq!(registry.projects.len(), 2);

        let unrelated =
            registry.record_open("/projects/four/astro", Some("another-site"), 40, |_| false);
        assert_ne!(unrelated.id, first.id);
        assert_eq!(unrelated.rebound_from, None);
        assert_eq!(registry.projects.len(), 3);
    }

    #[test]
    fn owning_project_matches_the_longest_registered_ancestor() {
        let mut registry = ProjectRegistry::default();
        registry.record_open("/projects/site", Some("site"), 1, |_| true);
        registry.record_open("/projects/site/packages/nested", Some("nested"), 2, |_| {
            true
        });
        assert_eq!(
            registry
                .owning_project("/projects/site/src/content/posts/a.md")
                .map(|project| project.id.as_str()),
            Some(registry.projects[0].id.as_str())
        );
        assert_eq!(
            registry
                .owning_project("/projects/site/packages/nested/src/a.md")
                .map(|project| project.id.as_str()),
            Some(registry.projects[1].id.as_str())
        );
        assert!(registry.owning_project("/elsewhere/a.md").is_none());
        assert!(
            registry
                .owning_project("/projects/site-other/a.md")
                .is_none(),
            "a shared string prefix is not an ancestor"
        );
    }

    #[test]
    fn removing_a_recent_project_keeps_the_rest_ordered() {
        let mut registry = ProjectRegistry::default();
        let first = registry.record_open("/projects/one", Some("one"), 1, |_| true);
        let second = registry.record_open("/projects/two", Some("two"), 2, |_| true);
        assert_eq!(registry.recent().first().unwrap().id, second.id);
        assert!(registry.remove(&first.id));
        assert!(!registry.remove(&first.id));
        assert_eq!(registry.recent().len(), 1);
        assert_eq!(registry.recent()[0].id, second.id);
    }

    #[test]
    fn theme_round_trips_and_retired_palette_fields_are_ignored() {
        let global = GlobalPreferencesV2 {
            theme: ThemePreference::Dark,
            ..GlobalPreferencesV2::default()
        };
        let encoded = serde_json::to_value(&global).unwrap();
        let decoded: GlobalPreferencesV2 = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.theme, ThemePreference::Dark);

        let retired: GlobalPreferencesV2 =
            serde_json::from_str(r#"{"version":2,"light_theme":"nord_light","dark_theme":"ink"}"#)
                .unwrap();
        assert_eq!(retired.theme, ThemePreference::System);
        assert_eq!(retired.ui_font_family, "");
        assert_eq!(retired.ui_font_size, 18.);
        assert_eq!(retired.prose_font_family, "");
        assert_eq!(retired.editor_font_size, 16.);
        assert_eq!(retired.editor_line_height, 2.);
        assert_eq!(retired.page_width_percent, 64.);
        assert!(!retired.focus_mode);
        assert!(!retired.typewriter_mode);
    }

    #[test]
    fn writing_typography_and_modes_resolve_from_the_global_scope() {
        let global = GlobalPreferencesV2 {
            prose_font_family: "Inter Display".into(),
            editor_font_size: 19.,
            editor_line_height: 1.75,
            page_width_percent: 70.,
            focus_mode: true,
            typewriter_mode: true,
            ..GlobalPreferencesV2::default()
        };
        let resolved = resolve(&global, None, None);
        assert_eq!(resolved.prose_font_family, "Inter Display");
        assert_eq!(resolved.editor_font_size, 19.);
        assert_eq!(resolved.editor_line_height, 1.75);
        assert_eq!(resolved.page_width_percent, 70.);
        assert!(resolved.focus_mode);
        assert!(resolved.typewriter_mode);

        let decoded: GlobalPreferencesV2 =
            serde_json::from_str(&serde_json::to_string(&global).unwrap()).unwrap();
        assert_eq!(decoded.prose_font_family, "Inter Display");
        assert_eq!(decoded.editor_line_height, 1.75);
        assert!(decoded.focus_mode);
        assert!(decoded.typewriter_mode);
    }

    #[test]
    fn window_geometry_round_trips_through_the_global_envelope() {
        let global = GlobalPreferencesV2 {
            window: Some(WindowGeometry {
                x: 120.,
                y: 80.,
                width: 1280.,
                height: 800.,
                maximized: true,
            }),
            ..GlobalPreferencesV2::default()
        };
        let decoded: GlobalPreferencesV2 =
            serde_json::from_str(&serde_json::to_string(&global).unwrap()).unwrap();
        assert_eq!(decoded.window, global.window);
        let bare: GlobalPreferencesV2 = serde_json::from_str("{\"version\":2}").unwrap();
        assert_eq!(bare.window, None);
    }

    #[test]
    fn same_name_projects_keep_distinct_stable_ids() {
        let mut registry = ProjectRegistry::default();
        let left = registry.record_open("/projects/left/astro", Some("astro-site"), 1, |_| true);
        let right = registry.record_open("/projects/right/astro", Some("astro-site"), 2, |_| true);
        assert_ne!(left.id, right.id);
        assert_eq!(left.display_name, right.display_name);
        let reopened =
            registry.record_open("/projects/left/astro", Some("astro-site"), 3, |_| true);
        assert_eq!(reopened.id, left.id);
        assert_eq!(registry.projects.len(), 2);
        assert_eq!(registry.recent()[0].id, left.id);
        assert_eq!(
            registry.get_by_path("/projects/right/astro").unwrap().id,
            right.id
        );
    }
}
