//! The preferences sheet and one pane per preference scope.
//!
//! Panes mirror the upstream scope exactly: General and Advanced always, Project
//! Settings and Collections only while a project is open. The sheet owns a draft
//! of every editable value; Apply validates all panes and emits one
//! [`PreferenceEdits`] bundle, Cancel and Escape drop the draft without touching
//! the live preferences.

pub(crate) mod advanced;
pub(crate) mod appearance;
pub(crate) mod collection;
pub(crate) mod preview;
pub(crate) mod project;
#[cfg(test)]
mod tests;
pub(crate) mod workspace;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use astrolith_astro::schema::{CollectionSchema, FieldKind};
use astrolith_core::preferences::{
    AutosaveDelay, BrowserCommand, CollectionPreferencesV1, FieldMappings, FileExtension,
    GlobalPreferencesV2, HeadingColors, IdeCommand, ImagePathMode, ModeLayouts, PreviewPreferences,
    PreviewPresentation, ProjectPreferencesV1, ThemePreference, WorkspaceMode,
};
use gpui_kit::component::{
    ActiveTheme, Disableable, Selectable, Sizable, StyledExt, WindowExt,
    button::{Button, ButtonVariants},
    input::{InputEvent, InputState, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
    scroll::ScrollableElement,
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, SharedString, Subscription, TestSupportExt, Window, div, prelude::*, px,
};

use crate::theme;

/// One preferences pane; the set matches the upstream scopes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pane {
    /// Theme, interface and document typography.
    Appearance,
    /// Default workspace mode, per-mode layouts and workspace behaviour.
    Workspace,
    /// Preview presentation, browser argv and entry behaviour.
    Preview,
    /// Preferences of the open project.
    Project,
    /// Per-collection overrides.
    Collections,
    /// Versions, the preferences folder and Reset All.
    Advanced,
}

impl Pane {
    /// Display order in the sheet's navigation.
    pub(crate) const ALL: [Self; 6] = [
        Self::Appearance,
        Self::Workspace,
        Self::Preview,
        Self::Project,
        Self::Collections,
        Self::Advanced,
    ];

    /// Navigation label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Workspace => "Workspace",
            Self::Preview => "Preview",
            Self::Project => "Project Settings",
            Self::Collections => "Collections",
            Self::Advanced => "Advanced",
        }
    }

    /// One-line explanation shown above the pane body.
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Appearance => "Theme, interface type and document typography.",
            Self::Workspace => "Startup mode, per-mode layouts and writing behaviour.",
            Self::Preview => "How the live site opens and which browser to use.",
            Self::Project => "Defaults for this project's content workflow.",
            Self::Collections => "Directories, field mappings and style per collection.",
            Self::Advanced => "Paths, integrations and reset actions.",
        }
    }

    /// Stable element identifier, used by tests and accessibility clients.
    pub(crate) fn element_id(self) -> &'static str {
        match self {
            Self::Appearance => "preferences-pane-appearance",
            Self::Workspace => "preferences-pane-workspace",
            Self::Preview => "preferences-pane-preview",
            Self::Project => "preferences-pane-project",
            Self::Collections => "preferences-pane-collections",
            Self::Advanced => "preferences-pane-advanced",
        }
    }

    /// Project-scoped panes exist only while a project is open.
    pub(crate) fn available(self, has_project: bool) -> bool {
        has_project
            || matches!(
                self,
                Self::Appearance | Self::Workspace | Self::Preview | Self::Advanced
            )
    }
}

/// One collection and the generated schema used to validate its mappings.
#[derive(Clone)]
pub(crate) struct CollectionEntry {
    /// Collection identity.
    pub name: String,
    /// Normalized schema when the generation service produced one.
    pub schema: Option<Arc<CollectionSchema>>,
}

/// Snapshots and project data the sheet needs when it opens.
#[derive(Clone)]
pub(crate) struct PreferencesContext {
    /// Global scope as persisted.
    pub global: GlobalPreferencesV2,
    /// Global layout defaults, including changes made while the sheet is open.
    pub global_layouts: ModeLayouts,
    /// Open project scope; `None` hides the project and collection panes.
    pub project: Option<ProjectPreferencesV1>,
    /// Project collections with their schemas.
    pub collections: Vec<CollectionEntry>,
    /// The project declares `astro-auto-import`.
    pub auto_import: bool,
    /// Preferences folder, when it can be resolved.
    pub preferences_folder: Option<PathBuf>,
    /// Latest persistence warning, if any.
    pub warning: Option<String>,
    /// Why the embedded WebView cannot be offered on this host/build.
    pub embedded_unavailable_reason: Option<String>,
}

/// Per-mode layout resets requested by the draft.
///
/// Layouts are edited directly in the workspace, so the sheet never carries a
/// stale copy of the live geometry. Apply folds only these explicit resets into
/// the authoritative global defaults and the open project's overrides.
#[derive(Clone, Debug, Default)]
pub(crate) struct LayoutResets {
    /// Modes whose global default returns to the design default.
    pub global: BTreeSet<WorkspaceMode>,
    /// Drop every layout override recorded by the open project.
    pub project: bool,
}

/// One validated Apply: both scopes are replaced together or not at all.
#[derive(Clone)]
pub(crate) struct PreferenceEdits {
    /// Global scope with every General change applied.
    pub global: GlobalPreferencesV2,
    /// Project scope and its collection overrides, when a project is open.
    pub project: Option<ProjectPreferencesV1>,
    /// Layout resets the draft recorded.
    pub layout_resets: LayoutResets,
}

/// What the sheet asks the shell to do.
pub(crate) enum PreferencesEvent {
    /// Validate every pane and replace both scopes.
    Apply(Box<PreferenceEdits>),
    /// Delete preferences and the registry, then restart.
    ResetAll,
    /// Dismiss without changes.
    Cancel,
    /// Open the bundled user guide.
    OpenUserGuide,
    /// Open the generated keyboard-shortcut reference.
    OpenKeyboardShortcuts,
}

impl EventEmitter<PreferencesEvent> for PreferencesSheet {}

/// Every field that can carry an inline error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FieldId {
    IdeExecutable,
    IdeArguments,
    PreviewBrowserExecutable,
    PreviewBrowserArguments,
    HeadingLight,
    HeadingDark,
    UiFontFamily,
    UiFontSize,
    ProseFontFamily,
    FontSize,
    LineHeight,
    PageWidth,
    ProjectContent,
    ProjectAssets,
    ProjectMdx,
    ProjectTitle,
    ProjectDate,
    ProjectDescription,
    ProjectDraft,
    CollectionContent,
    CollectionAssets,
    CollectionTitle,
    CollectionDate,
    CollectionDescription,
    CollectionDraft,
    CollectionUrlPattern,
}

/// Optional decoration around one preference field row.
pub(crate) struct FieldExtras {
    pub field: Option<FieldId>,
    pub hint: Option<&'static str>,
    pub trailing: Option<AnyElement>,
}

/// Text controls the sheet keeps alive across pane switches and dialog rebuilds.
struct Inputs {
    ide_executable: Entity<InputState>,
    ide_arguments: Entity<TextareaState>,
    preview_browser_executable: Entity<InputState>,
    preview_browser_arguments: Entity<TextareaState>,
    heading_light: Entity<InputState>,
    heading_dark: Entity<InputState>,
    ui_font_family: Entity<InputState>,
    ui_font_size: Entity<InputState>,
    prose_font_family: Entity<InputState>,
    font_size: Entity<InputState>,
    line_height: Entity<InputState>,
    page_width_percent: Entity<InputState>,
    project_content: Entity<InputState>,
    project_assets: Entity<InputState>,
    project_mdx: Entity<InputState>,
    project_title: Entity<InputState>,
    project_date: Entity<InputState>,
    project_description: Entity<InputState>,
    project_draft: Entity<InputState>,
    collection_content: Entity<InputState>,
    collection_assets: Entity<InputState>,
    collection_title: Entity<InputState>,
    collection_date: Entity<InputState>,
    collection_description: Entity<InputState>,
    collection_draft: Entity<InputState>,
    collection_url_pattern: Entity<InputState>,
}

/// The preferences sheet view.
pub(crate) struct PreferencesSheet {
    context: Option<PreferencesContext>,
    pane: Pane,
    theme: ThemePreference,
    default_mode: WorkspaceMode,
    layout_resets: LayoutResets,
    global_extension: FileExtension,
    autosave: AutosaveDelay,
    focus_mode: bool,
    typewriter_mode: bool,
    preview_presentation: PreviewPresentation,
    preview_open_on_enter: bool,
    project_image_mode: Option<ImagePathMode>,
    project_extension: Option<FileExtension>,
    project_insert_imports: Option<bool>,
    collection_index: Option<usize>,
    collection_image_mode: Option<ImagePathMode>,
    collection_extension: Option<FileExtension>,
    inputs: Inputs,
    errors: BTreeMap<FieldId, SharedString>,
    notice: Option<SharedString>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl PreferencesSheet {
    /// Create an empty sheet; [`Self::open`] installs the snapshots.
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        macro_rules! input {
            ($placeholder:expr) => {{
                let input = cx.new(|cx| InputState::new(window, cx).placeholder($placeholder));
                subscriptions.push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.refresh_errors(cx);
                    }
                }));
                input
            }};
        }
        let ide_arguments = cx.new(|cx| TextareaState::new(window, cx));
        subscriptions.push(
            cx.subscribe(&ide_arguments, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refresh_errors(cx);
                }
            }),
        );
        Self {
            context: None,
            pane: Pane::Appearance,
            theme: ThemePreference::default(),
            default_mode: WorkspaceMode::default(),
            layout_resets: LayoutResets::default(),
            global_extension: FileExtension::default(),
            autosave: AutosaveDelay::default(),
            focus_mode: false,
            typewriter_mode: false,
            preview_presentation: PreviewPresentation::default(),
            preview_open_on_enter: false,
            project_image_mode: None,
            project_extension: None,
            project_insert_imports: None,
            collection_index: None,
            collection_image_mode: None,
            collection_extension: None,
            inputs: Inputs {
                ide_executable: input!("hx or code"),
                ide_arguments,
                preview_browser_executable: input!("Empty uses the system browser"),
                preview_browser_arguments: {
                    let input = cx.new(|cx| TextareaState::new(window, cx));
                    subscriptions.push(cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.refresh_errors(cx);
                        }
                    }));
                    input
                },
                heading_light: input!("#1f2328"),
                heading_dark: input!("#e6edf3"),
                ui_font_family: input!("System UI font"),
                ui_font_size: input!(format_number(GlobalPreferencesV2::default().ui_font_size)),
                prose_font_family: input!("Inter / system sans"),
                font_size: input!("16"),
                line_height: input!("2"),
                page_width_percent: input!("64"),
                project_content: input!("Inherit the Astro content directory"),
                project_assets: input!("Inherit the configured assets directory"),
                project_mdx: input!("Inherit the MDX component directory"),
                project_title: input!("title"),
                project_date: input!("date"),
                project_description: input!("description"),
                project_draft: input!("draft"),
                collection_content: input!("Inherit the project content directory"),
                collection_assets: input!("Inherit the project assets directory"),
                collection_title: input!("Inherit the project mapping"),
                collection_date: input!("Inherit the project mapping"),
                collection_description: input!("Inherit the project mapping"),
                collection_draft: input!("Inherit the project mapping"),
                collection_url_pattern: input!("/blog/{slug}/"),
            },
            errors: BTreeMap::new(),
            notice: None,
            focus: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
        }
    }

    /// Install the current snapshots and draft every editable value.
    pub(crate) fn open(
        &mut self,
        context: PreferencesContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = context.project.clone();
        let global = context.global.clone();
        self.theme = global.theme;
        self.default_mode = global.default_mode;
        self.layout_resets = LayoutResets::default();
        self.global_extension = global.default_extension;
        self.autosave = global.autosave_delay;
        self.focus_mode = global.focus_mode;
        self.typewriter_mode = global.typewriter_mode;
        self.preview_presentation = global.preview.presentation;
        self.preview_open_on_enter = global.preview.open_on_enter;
        self.set_text(FieldId::IdeExecutable, &global.ide.executable, window, cx);
        self.set_text(
            FieldId::IdeArguments,
            &global.ide.arguments.join("\n"),
            window,
            cx,
        );
        self.set_text(
            FieldId::PreviewBrowserExecutable,
            &global.preview.browser.executable,
            window,
            cx,
        );
        self.set_text(
            FieldId::PreviewBrowserArguments,
            &global.preview.browser.arguments.join("\n"),
            window,
            cx,
        );
        self.set_text(
            FieldId::HeadingLight,
            global.heading_colors.light.as_deref().unwrap_or_default(),
            window,
            cx,
        );
        self.set_text(
            FieldId::HeadingDark,
            global.heading_colors.dark.as_deref().unwrap_or_default(),
            window,
            cx,
        );
        self.set_text(FieldId::UiFontFamily, &global.ui_font_family, window, cx);
        self.set_text(
            FieldId::UiFontSize,
            &format_number(global.ui_font_size),
            window,
            cx,
        );
        self.set_text(
            FieldId::ProseFontFamily,
            &global.prose_font_family,
            window,
            cx,
        );
        self.set_text(
            FieldId::FontSize,
            &format_number(global.editor_font_size),
            window,
            cx,
        );
        self.set_text(
            FieldId::LineHeight,
            &format_number(global.editor_line_height),
            window,
            cx,
        );
        self.set_text(
            FieldId::PageWidth,
            &format_number(global.page_width_percent),
            window,
            cx,
        );

        let project_preferences = project.clone().unwrap_or_default();
        self.set_text(
            FieldId::ProjectContent,
            project_preferences
                .content_directory
                .as_deref()
                .unwrap_or_default(),
            window,
            cx,
        );
        self.set_text(
            FieldId::ProjectAssets,
            project_preferences
                .assets_directory
                .as_deref()
                .unwrap_or_default(),
            window,
            cx,
        );
        self.set_text(
            FieldId::ProjectMdx,
            project_preferences
                .mdx_component_directory
                .as_deref()
                .unwrap_or_default(),
            window,
            cx,
        );
        for (field, value) in [
            (
                FieldId::ProjectTitle,
                &project_preferences.field_mappings.title,
            ),
            (
                FieldId::ProjectDate,
                &project_preferences.field_mappings.date,
            ),
            (
                FieldId::ProjectDescription,
                &project_preferences.field_mappings.description,
            ),
            (
                FieldId::ProjectDraft,
                &project_preferences.field_mappings.draft,
            ),
        ] {
            self.set_text(field, value.as_deref().unwrap_or_default(), window, cx);
        }
        self.set_placeholder(FieldId::ProjectTitle, "title", window, cx);
        self.set_placeholder(FieldId::ProjectDate, "date", window, cx);
        self.set_placeholder(FieldId::ProjectDescription, "description", window, cx);
        self.set_placeholder(FieldId::ProjectDraft, "draft", window, cx);
        self.project_image_mode = project_preferences.image_path_mode;
        self.project_extension = project_preferences.default_extension;
        self.project_insert_imports = project_preferences.insert_mdx_imports;
        let first_collection = (!context.collections.is_empty()).then_some(0);
        self.collection_index = first_collection;
        self.pane = Pane::Appearance;
        self.notice = context.warning.clone().map(SharedString::from);
        self.context = Some(context);
        self.select_collection(first_collection, window, cx);
        cx.notify();
    }

    /// Drop the draft; the shell closes the sheet.
    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        self.context = None;
        self.errors.clear();
        cx.notify();
    }

    /// Whether the Apply button is blocked by an inline validation error.
    pub(crate) fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// The current draft, when every field validates. Used by Apply and tests.
    pub(crate) fn edits(&self, cx: &App) -> Option<PreferenceEdits> {
        if !self.errors.is_empty() || self.context.is_none() {
            return None;
        }
        let context = self.context.as_ref()?;
        let mut global = context.global.clone();
        global.theme = self.theme;
        global.default_mode = self.default_mode;
        global.default_extension = self.global_extension;
        global.autosave_delay = self.autosave;
        global.heading_colors = HeadingColors {
            light: heading_color(&self.text(FieldId::HeadingLight, cx)),
            dark: heading_color(&self.text(FieldId::HeadingDark, cx)),
        };
        global.ui_font_family = self.text(FieldId::UiFontFamily, cx).trim().to_owned();
        global.ui_font_size = self
            .text(FieldId::UiFontSize, cx)
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|size| size.is_finite() && (8.0..=28.0).contains(size))
            .unwrap_or(global.ui_font_size);
        global.prose_font_family = self.text(FieldId::ProseFontFamily, cx).trim().to_owned();
        global.editor_font_size = self
            .text(FieldId::FontSize, cx)
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|size| size.is_finite() && (1.0..=30.0).contains(size))
            .unwrap_or(global.editor_font_size);
        global.editor_line_height = self
            .text(FieldId::LineHeight, cx)
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|height| height.is_finite() && (1.2..=3.0).contains(height))
            .unwrap_or(global.editor_line_height);
        global.focus_mode = self.focus_mode;
        global.typewriter_mode = self.typewriter_mode;
        global.preview = PreviewPreferences {
            presentation: self.preview_presentation,
            browser: browser_command(
                &self.text(FieldId::PreviewBrowserExecutable, cx),
                &self.text(FieldId::PreviewBrowserArguments, cx),
            )
            .ok()?,
            open_on_enter: self.preview_open_on_enter,
        };
        global.page_width_percent = self
            .text(FieldId::PageWidth, cx)
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|width| width.is_finite() && (30.0..=100.0).contains(width))
            .unwrap_or(global.page_width_percent);
        global.ide = ide_command(
            &self.text(FieldId::IdeExecutable, cx),
            &self.text(FieldId::IdeArguments, cx),
        )?;

        let project = context.project.as_ref().map(|preferences| {
            let mut preferences = preferences.clone();
            preferences.content_directory = directory(&self.text(FieldId::ProjectContent, cx));
            preferences.assets_directory = directory(&self.text(FieldId::ProjectAssets, cx));
            preferences.mdx_component_directory = directory(&self.text(FieldId::ProjectMdx, cx));
            preferences.image_path_mode = self.project_image_mode;
            preferences.default_extension = self.project_extension;
            preferences.insert_mdx_imports = self.project_insert_imports;
            preferences.field_mappings = self.project_mappings(cx);
            if let Some(entry) = self.collection_entry() {
                let existing = preferences.collections.remove(&entry.name);
                let mut overrides = existing.clone().unwrap_or_default();
                overrides.content_directory = directory(&self.text(FieldId::CollectionContent, cx));
                overrides.assets_directory = directory(&self.text(FieldId::CollectionAssets, cx));
                overrides.image_path_mode = self.collection_image_mode;
                overrides.default_extension = self.collection_extension;
                overrides.field_mappings = self.collection_mappings(cx);
                overrides.content_finder_url_pattern =
                    url_pattern(&self.text(FieldId::CollectionUrlPattern, cx));
                if existing.is_some() || !collection_is_empty(&overrides) {
                    preferences
                        .collections
                        .insert(entry.name.clone(), overrides);
                }
            }
            preferences
        });
        Some(PreferenceEdits {
            global,
            project,
            layout_resets: self.layout_resets.clone(),
        })
    }

    /// Reset one control to its absent/default value, leaving every sibling alone.
    pub(crate) fn reset_field(
        &mut self,
        field: FieldId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match field {
            FieldId::UiFontFamily => self.set_text(field, "", window, cx),
            FieldId::UiFontSize => self.set_text(
                field,
                &format_number(GlobalPreferencesV2::default().ui_font_size),
                window,
                cx,
            ),
            FieldId::ProseFontFamily => self.set_text(field, "", window, cx),
            FieldId::FontSize => self.set_text(
                field,
                &format_number(GlobalPreferencesV2::default().editor_font_size),
                window,
                cx,
            ),
            FieldId::LineHeight => self.set_text(
                field,
                &format_number(GlobalPreferencesV2::default().editor_line_height),
                window,
                cx,
            ),
            FieldId::PageWidth => self.set_text(
                field,
                &format_number(GlobalPreferencesV2::default().page_width_percent),
                window,
                cx,
            ),
            FieldId::PreviewBrowserExecutable | FieldId::PreviewBrowserArguments => {
                self.set_text(field, "", window, cx)
            }
            FieldId::CollectionUrlPattern
            | FieldId::HeadingLight
            | FieldId::HeadingDark
            | FieldId::IdeExecutable
            | FieldId::IdeArguments
            | FieldId::ProjectContent
            | FieldId::ProjectAssets
            | FieldId::ProjectMdx
            | FieldId::ProjectTitle
            | FieldId::ProjectDate
            | FieldId::ProjectDescription
            | FieldId::ProjectDraft
            | FieldId::CollectionContent
            | FieldId::CollectionAssets
            | FieldId::CollectionTitle
            | FieldId::CollectionDate
            | FieldId::CollectionDescription
            | FieldId::CollectionDraft => self.set_text(field, "", window, cx),
        }
        self.refresh_errors(cx);
    }

    /// Switch to a pane; project panes are refused without a project.
    pub(crate) fn set_pane(&mut self, pane: Pane, cx: &mut Context<Self>) {
        let has_project = self
            .context
            .as_ref()
            .is_some_and(|context| context.project.is_some());
        if pane.available(has_project) {
            self.pane = pane;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn visible_panes(&self) -> Vec<Pane> {
        let has_project = self
            .context
            .as_ref()
            .is_some_and(|context| context.project.is_some());
        Pane::ALL
            .into_iter()
            .filter(|pane| pane.available(has_project))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn active_pane(&self) -> Pane {
        self.pane
    }

    #[cfg(test)]
    pub(crate) fn set_pane_for_tests(&mut self, pane: Pane, cx: &mut Context<Self>) {
        self.set_pane(pane, cx);
    }

    #[cfg(test)]
    pub(crate) fn set_text_for_tests(
        &mut self,
        field: FieldId,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_text(field, value, window, cx);
        self.refresh_errors(cx);
    }

    #[cfg(test)]
    pub(crate) fn project_insert_imports(&self) -> Option<bool> {
        self.project_insert_imports
    }

    #[cfg(test)]
    pub(crate) fn context_auto_import(&self) -> bool {
        self.context
            .as_ref()
            .is_some_and(|context| context.auto_import)
    }

    #[cfg(test)]
    pub(crate) fn set_default_mode_for_tests(
        &mut self,
        mode: WorkspaceMode,
        cx: &mut Context<Self>,
    ) {
        self.default_mode = mode;
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn toggle_layout_reset_for_tests(
        &mut self,
        mode: WorkspaceMode,
        cx: &mut Context<Self>,
    ) {
        if !self.layout_resets.global.remove(&mode) {
            self.layout_resets.global.insert(mode);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn set_project_layout_reset_for_tests(
        &mut self,
        reset: bool,
        cx: &mut Context<Self>,
    ) {
        self.layout_resets.project = reset;
        cx.notify();
    }

    fn collection_entry(&self) -> Option<CollectionEntry> {
        let context = self.context.as_ref()?;
        let index = self.collection_index?;
        context.collections.get(index).cloned()
    }

    fn select_collection(
        &mut self,
        index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.context.as_ref() else {
            return;
        };
        let Some(entry) = index
            .and_then(|index| context.collections.get(index))
            .cloned()
        else {
            self.collection_index = None;
            self.collection_image_mode = None;
            self.collection_extension = None;
            for field in [
                FieldId::CollectionContent,
                FieldId::CollectionAssets,
                FieldId::CollectionTitle,
                FieldId::CollectionDate,
                FieldId::CollectionDescription,
                FieldId::CollectionDraft,
                FieldId::CollectionUrlPattern,
            ] {
                self.set_text(field, "", window, cx);
            }
            self.refresh_errors(cx);
            return;
        };
        let existing = context
            .project
            .as_ref()
            .and_then(|project| project.collections.get(&entry.name).cloned());
        let mut overrides = existing.unwrap_or_default();
        self.collection_index = index;
        self.collection_image_mode = overrides.image_path_mode.take();
        self.collection_extension = overrides.default_extension.take();
        self.set_text(
            FieldId::CollectionContent,
            overrides.content_directory.as_deref().unwrap_or_default(),
            window,
            cx,
        );
        self.set_text(
            FieldId::CollectionAssets,
            overrides.assets_directory.as_deref().unwrap_or_default(),
            window,
            cx,
        );
        for (field, value) in [
            (FieldId::CollectionTitle, &overrides.field_mappings.title),
            (FieldId::CollectionDate, &overrides.field_mappings.date),
            (
                FieldId::CollectionDescription,
                &overrides.field_mappings.description,
            ),
            (FieldId::CollectionDraft, &overrides.field_mappings.draft),
        ] {
            self.set_text(field, value.as_deref().unwrap_or_default(), window, cx);
        }
        self.set_text(
            FieldId::CollectionUrlPattern,
            overrides
                .content_finder_url_pattern
                .as_deref()
                .unwrap_or_default(),
            window,
            cx,
        );
        // Inherited values stay visible as placeholders, never as stored text.
        let project_mappings = context
            .project
            .as_ref()
            .map(|project| project.field_mappings.clone())
            .unwrap_or_default();
        self.set_placeholder(
            FieldId::CollectionTitle,
            project_mappings.title.as_deref().unwrap_or("title"),
            window,
            cx,
        );
        self.set_placeholder(
            FieldId::CollectionDate,
            project_mappings.date.as_deref().unwrap_or("date"),
            window,
            cx,
        );
        self.set_placeholder(
            FieldId::CollectionDescription,
            project_mappings
                .description
                .as_deref()
                .unwrap_or("description"),
            window,
            cx,
        );
        self.set_placeholder(
            FieldId::CollectionDraft,
            project_mappings.draft.as_deref().unwrap_or("draft"),
            window,
            cx,
        );
        self.refresh_errors(cx);
        cx.notify();
    }

    fn project_mappings(&self, cx: &App) -> FieldMappings {
        FieldMappings {
            title: mapping(&self.text(FieldId::ProjectTitle, cx)),
            date: mapping(&self.text(FieldId::ProjectDate, cx)),
            description: mapping(&self.text(FieldId::ProjectDescription, cx)),
            draft: mapping(&self.text(FieldId::ProjectDraft, cx)),
        }
    }

    fn collection_mappings(&self, cx: &App) -> FieldMappings {
        FieldMappings {
            title: mapping(&self.text(FieldId::CollectionTitle, cx)),
            date: mapping(&self.text(FieldId::CollectionDate, cx)),
            description: mapping(&self.text(FieldId::CollectionDescription, cx)),
            draft: mapping(&self.text(FieldId::CollectionDraft, cx)),
        }
    }

    fn text(&self, field: FieldId, cx: &App) -> String {
        let value = match field {
            FieldId::IdeExecutable => self.inputs.ide_executable.read(cx).value(),
            FieldId::IdeArguments => self.inputs.ide_arguments.read(cx).value(),
            FieldId::PreviewBrowserExecutable => {
                self.inputs.preview_browser_executable.read(cx).value()
            }
            FieldId::PreviewBrowserArguments => {
                self.inputs.preview_browser_arguments.read(cx).value()
            }
            FieldId::HeadingLight => self.inputs.heading_light.read(cx).value(),
            FieldId::HeadingDark => self.inputs.heading_dark.read(cx).value(),
            FieldId::UiFontFamily => self.inputs.ui_font_family.read(cx).value(),
            FieldId::UiFontSize => self.inputs.ui_font_size.read(cx).value(),
            FieldId::ProseFontFamily => self.inputs.prose_font_family.read(cx).value(),
            FieldId::FontSize => self.inputs.font_size.read(cx).value(),
            FieldId::LineHeight => self.inputs.line_height.read(cx).value(),
            FieldId::PageWidth => self.inputs.page_width_percent.read(cx).value(),
            FieldId::ProjectContent => self.inputs.project_content.read(cx).value(),
            FieldId::ProjectAssets => self.inputs.project_assets.read(cx).value(),
            FieldId::ProjectMdx => self.inputs.project_mdx.read(cx).value(),
            FieldId::ProjectTitle => self.inputs.project_title.read(cx).value(),
            FieldId::ProjectDate => self.inputs.project_date.read(cx).value(),
            FieldId::ProjectDescription => self.inputs.project_description.read(cx).value(),
            FieldId::ProjectDraft => self.inputs.project_draft.read(cx).value(),
            FieldId::CollectionContent => self.inputs.collection_content.read(cx).value(),
            FieldId::CollectionAssets => self.inputs.collection_assets.read(cx).value(),
            FieldId::CollectionTitle => self.inputs.collection_title.read(cx).value(),
            FieldId::CollectionDate => self.inputs.collection_date.read(cx).value(),
            FieldId::CollectionDescription => self.inputs.collection_description.read(cx).value(),
            FieldId::CollectionDraft => self.inputs.collection_draft.read(cx).value(),
            FieldId::CollectionUrlPattern => self.inputs.collection_url_pattern.read(cx).value(),
        };
        value.to_string()
    }

    fn set_text(&self, field: FieldId, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        match field {
            FieldId::IdeExecutable => self
                .inputs
                .ide_executable
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::IdeArguments => self
                .inputs
                .ide_arguments
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::PreviewBrowserExecutable => self
                .inputs
                .preview_browser_executable
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::PreviewBrowserArguments => self
                .inputs
                .preview_browser_arguments
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::HeadingLight => self
                .inputs
                .heading_light
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::HeadingDark => self
                .inputs
                .heading_dark
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::UiFontFamily => self
                .inputs
                .ui_font_family
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::UiFontSize => self
                .inputs
                .ui_font_size
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProseFontFamily => self
                .inputs
                .prose_font_family
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::FontSize => self
                .inputs
                .font_size
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::LineHeight => self
                .inputs
                .line_height
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::PageWidth => self
                .inputs
                .page_width_percent
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectContent => self
                .inputs
                .project_content
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectAssets => self
                .inputs
                .project_assets
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectMdx => self
                .inputs
                .project_mdx
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectTitle => self
                .inputs
                .project_title
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectDate => self
                .inputs
                .project_date
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectDescription => self
                .inputs
                .project_description
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::ProjectDraft => self
                .inputs
                .project_draft
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionContent => self
                .inputs
                .collection_content
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionAssets => self
                .inputs
                .collection_assets
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionTitle => self
                .inputs
                .collection_title
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionDate => self
                .inputs
                .collection_date
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionDescription => self
                .inputs
                .collection_description
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionDraft => self
                .inputs
                .collection_draft
                .update(cx, |input, cx| input.set_value(value, window, cx)),
            FieldId::CollectionUrlPattern => self
                .inputs
                .collection_url_pattern
                .update(cx, |input, cx| input.set_value(value, window, cx)),
        }
    }

    fn set_placeholder(
        &self,
        field: FieldId,
        placeholder: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match field {
            FieldId::ProjectTitle => self.inputs.project_title.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            }),
            FieldId::ProjectDate => self.inputs.project_date.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            }),
            FieldId::ProjectDescription => {
                self.inputs.project_description.update(cx, |input, cx| {
                    input.set_placeholder(placeholder, window, cx)
                })
            }
            FieldId::ProjectDraft => self.inputs.project_draft.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            }),
            FieldId::CollectionTitle => self.inputs.collection_title.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            }),
            FieldId::CollectionDate => self.inputs.collection_date.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            }),
            FieldId::CollectionDescription => {
                self.inputs.collection_description.update(cx, |input, cx| {
                    input.set_placeholder(placeholder, window, cx)
                })
            }
            FieldId::CollectionDraft => self.inputs.collection_draft.update(cx, |input, cx| {
                input.set_placeholder(placeholder, window, cx)
            }),
            _ => {}
        }
    }

    fn refresh_errors(&mut self, cx: &mut Context<Self>) {
        self.errors = self.validate(cx);
        cx.notify();
    }

    /// Validate every visible and hidden field, so switching panes never hides
    /// a value that would fail on Apply.
    fn validate(&self, cx: &App) -> BTreeMap<FieldId, SharedString> {
        let mut errors = BTreeMap::new();
        let mut insert = |field: FieldId, message: String| {
            errors.insert(field, SharedString::from(message));
        };
        if let Err(message) = heading_color_value(&self.text(FieldId::HeadingLight, cx)) {
            insert(FieldId::HeadingLight, message);
        }
        if let Err(message) = heading_color_value(&self.text(FieldId::HeadingDark, cx)) {
            insert(FieldId::HeadingDark, message);
        }
        let ui_font_family = self.text(FieldId::UiFontFamily, cx);
        if !theme::font_family_available(cx, &ui_font_family) {
            insert(
                FieldId::UiFontFamily,
                format!(
                    "“{}” is not an installed font family.",
                    ui_font_family.trim()
                ),
            );
        }
        if let Err(message) = ui_font_size_value(&self.text(FieldId::UiFontSize, cx)) {
            insert(FieldId::UiFontSize, message);
        }
        let prose_font_family = self.text(FieldId::ProseFontFamily, cx);
        if !theme::font_family_available(cx, &prose_font_family) {
            insert(
                FieldId::ProseFontFamily,
                format!(
                    "“{}” is not an installed font family.",
                    prose_font_family.trim()
                ),
            );
        }
        if let Err(message) = font_size_value(&self.text(FieldId::FontSize, cx)) {
            insert(FieldId::FontSize, message);
        }
        if let Err(message) = line_height_value(&self.text(FieldId::LineHeight, cx)) {
            insert(FieldId::LineHeight, message);
        }
        if let Err(message) = page_width_value(&self.text(FieldId::PageWidth, cx)) {
            insert(FieldId::PageWidth, message);
        }
        if let Err(message) = ide_command_value(
            &self.text(FieldId::IdeExecutable, cx),
            &self.text(FieldId::IdeArguments, cx),
        ) {
            insert(FieldId::IdeExecutable, message);
        }
        if let Err(message) = browser_command(
            &self.text(FieldId::PreviewBrowserExecutable, cx),
            &self.text(FieldId::PreviewBrowserArguments, cx),
        ) {
            insert(FieldId::PreviewBrowserExecutable, message);
        }
        let Some(context) = self.context.as_ref() else {
            return errors;
        };
        if context.project.is_none() {
            return errors;
        }
        for (field, label) in [
            (FieldId::ProjectContent, "Content"),
            (FieldId::ProjectAssets, "Assets"),
            (FieldId::ProjectMdx, "MDX components"),
        ] {
            if let Err(message) = directory_value(&self.text(field, cx)) {
                insert(field, format!("{label}: {message}"));
            }
        }
        let schemas: Vec<&CollectionSchema> = context
            .collections
            .iter()
            .filter_map(|entry| entry.schema.as_deref())
            .collect();
        for (field, role) in [
            (FieldId::ProjectTitle, MappingRole::Title),
            (FieldId::ProjectDate, MappingRole::Date),
            (FieldId::ProjectDescription, MappingRole::Description),
            (FieldId::ProjectDraft, MappingRole::Draft),
        ] {
            if let Err(message) = mapping_value(role, &self.text(field, cx), &schemas) {
                insert(field, message);
            }
        }
        if let Some(entry) = self.collection_entry() {
            for (field, name) in [
                (FieldId::CollectionContent, "Content directory"),
                (FieldId::CollectionAssets, "Assets directory"),
            ] {
                if let Err(message) = directory_value(&self.text(field, cx)) {
                    insert(field, format!("{name}: {message}"));
                }
            }
            let schemas: Vec<&CollectionSchema> = entry.schema.as_deref().into_iter().collect();
            for (field, role) in [
                (FieldId::CollectionTitle, MappingRole::Title),
                (FieldId::CollectionDate, MappingRole::Date),
                (FieldId::CollectionDescription, MappingRole::Description),
                (FieldId::CollectionDraft, MappingRole::Draft),
            ] {
                if let Err(message) = mapping_value(role, &self.text(field, cx), &schemas) {
                    insert(field, message);
                }
            }
            if let Err(message) = url_pattern_value(&self.text(FieldId::CollectionUrlPattern, cx)) {
                insert(FieldId::CollectionUrlPattern, message);
            }
        }
        errors
    }

    fn error(&self, field: FieldId) -> Option<SharedString> {
        self.errors.get(&field).cloned()
    }
}

impl Focusable for PreferencesSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl PreferencesSheet {
    /// The text control for one field, ready to render.
    pub(super) fn control(&self, field: FieldId) -> AnyElement {
        match field {
            FieldId::IdeArguments => {
                gpui_kit::component::input::Textarea::new(&self.inputs.ide_arguments)
                    .h(px(72.))
                    .into_any_element()
            }
            FieldId::IdeExecutable => {
                gpui_kit::component::input::Input::new(&self.inputs.ide_executable)
                    .into_any_element()
            }
            FieldId::PreviewBrowserExecutable => {
                gpui_kit::component::input::Input::new(&self.inputs.preview_browser_executable)
                    .into_any_element()
            }
            FieldId::PreviewBrowserArguments => {
                gpui_kit::component::input::Textarea::new(&self.inputs.preview_browser_arguments)
                    .h(px(72.))
                    .into_any_element()
            }
            FieldId::HeadingLight => {
                gpui_kit::component::input::Input::new(&self.inputs.heading_light)
                    .into_any_element()
            }
            FieldId::HeadingDark => {
                gpui_kit::component::input::Input::new(&self.inputs.heading_dark).into_any_element()
            }
            FieldId::UiFontFamily => {
                gpui_kit::component::input::Input::new(&self.inputs.ui_font_family)
                    .into_any_element()
            }
            FieldId::UiFontSize => {
                gpui_kit::component::input::Input::new(&self.inputs.ui_font_size).into_any_element()
            }
            FieldId::ProseFontFamily => {
                gpui_kit::component::input::Input::new(&self.inputs.prose_font_family)
                    .into_any_element()
            }
            FieldId::FontSize => {
                gpui_kit::component::input::Input::new(&self.inputs.font_size).into_any_element()
            }
            FieldId::LineHeight => {
                gpui_kit::component::input::Input::new(&self.inputs.line_height).into_any_element()
            }
            FieldId::PageWidth => {
                gpui_kit::component::input::Input::new(&self.inputs.page_width_percent)
                    .into_any_element()
            }
            FieldId::ProjectContent => {
                gpui_kit::component::input::Input::new(&self.inputs.project_content)
                    .into_any_element()
            }
            FieldId::ProjectAssets => {
                gpui_kit::component::input::Input::new(&self.inputs.project_assets)
                    .into_any_element()
            }
            FieldId::ProjectMdx => {
                gpui_kit::component::input::Input::new(&self.inputs.project_mdx).into_any_element()
            }
            FieldId::ProjectTitle => {
                gpui_kit::component::input::Input::new(&self.inputs.project_title)
                    .into_any_element()
            }
            FieldId::ProjectDate => {
                gpui_kit::component::input::Input::new(&self.inputs.project_date).into_any_element()
            }
            FieldId::ProjectDescription => {
                gpui_kit::component::input::Input::new(&self.inputs.project_description)
                    .into_any_element()
            }
            FieldId::ProjectDraft => {
                gpui_kit::component::input::Input::new(&self.inputs.project_draft)
                    .into_any_element()
            }
            FieldId::CollectionContent => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_content)
                    .into_any_element()
            }
            FieldId::CollectionAssets => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_assets)
                    .into_any_element()
            }
            FieldId::CollectionTitle => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_title)
                    .into_any_element()
            }
            FieldId::CollectionDate => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_date)
                    .into_any_element()
            }
            FieldId::CollectionDescription => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_description)
                    .into_any_element()
            }
            FieldId::CollectionDraft => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_draft)
                    .into_any_element()
            }
            FieldId::CollectionUrlPattern => {
                gpui_kit::component::input::Input::new(&self.inputs.collection_url_pattern)
                    .into_any_element()
            }
        }
    }

    /// A reset control that removes only this field's value.
    pub(super) fn reset_button(
        &self,
        field: FieldId,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new(id)
            .xsmall()
            .ghost()
            .label("Reset")
            .on_click(cx.listener(move |this, _, window, cx| this.reset_field(field, window, cx)))
            .into_any_element()
    }

    /// A row with a visible label, its control, inline error and optional help.
    pub(super) fn field(
        &self,
        id: &'static str,
        label: &'static str,
        control: AnyElement,
        extras: FieldExtras,
        cx: &App,
    ) -> AnyElement {
        let FieldExtras {
            field,
            hint,
            trailing,
        } = extras;
        let error = field.and_then(|field| self.error(field));
        div()
            .id(id)
            .test_support()
            .v_flex()
            .w_full()
            .gap_2()
            .child(
                div()
                    .h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .min_w_0()
                            .text_base()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .children(trailing),
            )
            .child(control)
            .when_some(hint, |column, hint| {
                column.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
            })
            .when_some(error, |column, error| {
                column.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .into_any_element()
    }

    /// One titled group of related settings.
    pub(super) fn section(
        &self,
        title: &'static str,
        hint: Option<&'static str>,
        children: Vec<AnyElement>,
        cx: &App,
    ) -> AnyElement {
        div()
            .v_flex()
            .w_full()
            .gap_4()
            .p_4()
            .rounded_lg()
            .bg(cx.theme().group_box)
            .border_1()
            .border_color(cx.theme().border.opacity(0.55))
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(
                        div()
                            .font_family(crate::theme::display_font(cx))
                            .text_base()
                            .font_semibold()
                            .child(title),
                    )
                    .when_some(hint, |column, hint| {
                        column.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(hint),
                        )
                    }),
            )
            .children(children)
            .into_any_element()
    }
}

impl Render for PreferencesSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self
            .context
            .as_ref()
            .is_some_and(|context| context.project.is_some());
        if !self.pane.available(has_project) {
            self.pane = Pane::Appearance;
        }
        let mut root = div()
            .v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus);
        if let Some(notice) = self.notice.clone() {
            root = root.child(
                div()
                    .px_6()
                    .pt_4()
                    .text_sm()
                    .text_color(cx.theme().warning)
                    .child(notice),
            );
        }
        // The dialog title already names the sheet; the navigation starts
        // directly with the pane list so it cannot appear twice.
        let navigation_width =
            px((190. * (f32::from(cx.theme().font_size) / 14.)).clamp(180., 300.));
        let navigation = div()
            .v_flex()
            .w(navigation_width)
            .flex_shrink_0()
            .gap_1()
            .p_4()
            .pr_3()
            .border_r_1()
            .border_color(cx.theme().border.opacity(0.6))
            .children(
                Pane::ALL
                    .into_iter()
                    .filter(|pane| pane.available(has_project))
                    .map(|pane| {
                        let selected = self.pane == pane;
                        Button::new(pane.element_id())
                            .ghost()
                            .w_full()
                            .justify_start()
                            .selected(selected)
                            .label(pane.label())
                            .tab_index(0)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.set_pane(pane, cx);
                            }))
                    }),
            )
            .child(
                div()
                    .v_flex()
                    .w_full()
                    .gap_1()
                    .pt_4()
                    .child(
                        div()
                            .w_full()
                            .border_t_1()
                            .border_color(cx.theme().border.opacity(0.6)),
                    )
                    .child(
                        Button::new("preferences-user-guide")
                            .ghost()
                            .w_full()
                            .justify_start()
                            .label("User guide")
                            .tab_index(0)
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(PreferencesEvent::OpenUserGuide);
                            })),
                    )
                    .child(
                        Button::new("preferences-shortcuts")
                            .ghost()
                            .w_full()
                            .justify_start()
                            .label("Keyboard shortcuts")
                            .tab_index(0)
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(PreferencesEvent::OpenKeyboardShortcuts);
                            })),
                    ),
            );
        let body = match self.pane {
            Pane::Appearance => self.appearance_pane(cx).into_any_element(),
            Pane::Workspace => self.workspace_pane(cx).into_any_element(),
            Pane::Preview => self.preview_pane(cx).into_any_element(),
            Pane::Project => self.project_pane(cx).into_any_element(),
            Pane::Collections => self.collection_pane(cx).into_any_element(),
            Pane::Advanced => self.advanced_pane(cx).into_any_element(),
        };
        let apply_disabled = self.has_errors();
        let pane = self.pane;
        let header = div()
            .h_flex()
            .w_full()
            .items_start()
            .justify_between()
            .gap_4()
            .px_6()
            .pt_5()
            .pb_4()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.6))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .font_family(crate::theme::display_font(cx))
                            .text_lg()
                            .font_semibold()
                            .child(pane.label()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(pane.description()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(if apply_disabled {
                                "Fix the highlighted values before applying."
                            } else {
                                "Changes apply immediately and persist atomically."
                            }),
                    ),
            )
            .child(
                div()
                    .h_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .child(
                        Button::new("preferences-cancel")
                            .label("Cancel")
                            .tab_index(1)
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.emit(PreferencesEvent::Cancel);
                                this.cancel(cx);
                                window.close_dialog(cx);
                            })),
                    )
                    .child(
                        Button::new("preferences-apply")
                            .primary()
                            .label("Apply")
                            .tab_index(1)
                            .disabled(apply_disabled)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let Some(edits) = this.edits(cx) else {
                                    this.refresh_errors(cx);
                                    return;
                                };
                                cx.emit(PreferencesEvent::Apply(Box::new(edits)));
                                window.close_dialog(cx);
                            })),
                    ),
            );
        root.child(
            div()
                .h_flex()
                .flex_1()
                .min_h_0()
                .items_stretch()
                .child(navigation)
                .child(
                    div()
                        .v_flex()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(header)
                        .child(
                            div()
                                .id("preferences-body")
                                .test_support()
                                .v_flex()
                                .flex_1()
                                .min_h_0()
                                .gap_4()
                                .px_6()
                                .py_5()
                                .overflow_y_scrollbar()
                                .child(body),
                        ),
                ),
        )
    }
}

// ---------------------------------------------------------------------------
// Pure validation used by the sheet. Every function takes user text and returns
// either the value to store or the inline message shown under the control.
// ---------------------------------------------------------------------------

/// Role a frontmatter field mapping assigns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MappingRole {
    /// Entry title.
    Title,
    /// Publication date.
    Date,
    /// Entry description.
    Description,
    /// Draft flag.
    Draft,
}

impl MappingRole {
    /// The schema kind this role accepts.
    pub(crate) fn accepts(self, kind: &FieldKind) -> bool {
        let kind = match kind {
            FieldKind::Nullable { inner } => inner.as_ref(),
            kind => kind,
        };
        match self {
            Self::Title | Self::Description => matches!(
                kind,
                FieldKind::String { .. } | FieldKind::Enum { .. } | FieldKind::Literal { .. }
            ),
            Self::Date => matches!(kind, FieldKind::Date | FieldKind::String { .. }),
            Self::Draft => matches!(kind, FieldKind::Boolean),
        }
    }

    fn expected(self) -> &'static str {
        match self {
            Self::Title => "a string field for the title",
            Self::Date => "a date field for the publication date",
            Self::Description => "a string field for the description",
            Self::Draft => "a boolean field for the draft flag",
        }
    }
}

/// Validate a project-relative directory override; blank means inherit.
pub(crate) fn directory_value(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().any(char::is_control) {
        return Err("Directories cannot contain control characters.".into());
    }
    if value == "~" || value.starts_with("~/") {
        return Err("Use a project-relative directory, not a home-directory path.".into());
    }
    let path = std::path::Path::new(value);
    if path.is_absolute() {
        return Err("Use a project-relative directory, not an absolute path.".into());
    }
    let mut normalized = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(segment) => normalized.push(segment),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                return Err("Directories must stay inside the project root.".into());
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err("Use a project-relative directory, not an absolute path.".into());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("Enter a directory name or leave the field empty to inherit.".into());
    }
    Ok(Some(normalized.to_string_lossy().replace('\\', "/")))
}

/// Validate the editor font size.
pub(crate) fn font_size_value(value: &str) -> Result<f32, String> {
    let value = value.trim();
    let Ok(size) = value.parse::<f32>() else {
        return Err("Enter a font size between 1 and 30.".into());
    };
    if !size.is_finite() || !(1.0..=30.0).contains(&size) {
        return Err("Font size must be between 1 and 30.".into());
    }
    Ok(size)
}

/// Validate the document line height as a multiple of the font size.
pub(crate) fn line_height_value(value: &str) -> Result<f32, String> {
    let value = value.trim();
    let Ok(height) = value.parse::<f32>() else {
        return Err("Enter a line height between 1.2 and 3.0.".into());
    };
    if !height.is_finite() || !(1.2..=3.0).contains(&height) {
        return Err("Line height must be between 1.2 and 3.0.".into());
    }
    Ok(height)
}

/// Validate the editor's prose column width as a percentage of the pane.
pub(crate) fn page_width_value(value: &str) -> Result<f32, String> {
    let value = value.trim();
    let Ok(width) = value.parse::<f32>() else {
        return Err("Enter a page width between 30 and 100 percent.".into());
    };
    if !width.is_finite() || !(30.0..=100.0).contains(&width) {
        return Err("Page width must be between 30 and 100 percent.".into());
    }
    Ok(width)
}

/// Validate the interface font size.
pub(crate) fn ui_font_size_value(value: &str) -> Result<f32, String> {
    let value = value.trim();
    let Ok(size) = value.parse::<f32>() else {
        return Err("Enter an interface size between 8 and 28.".into());
    };
    if !size.is_finite() || !(8.0..=28.0).contains(&size) {
        return Err("Interface size must be between 8 and 28.".into());
    }
    Ok(size)
}

/// Validate one heading colour; blank keeps the theme token.
pub(crate) fn heading_color_value(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    theme::color_from_hex(value)
        .map(|_| Some(value.to_ascii_lowercase()))
        .ok_or_else(|| "Enter a colour as #rrggbb or #rrggbbaa.".into())
}

/// Validate an IDE command; argv is never interpreted by a shell.
pub(crate) fn ide_command_value(executable: &str, arguments: &str) -> Result<IdeCommand, String> {
    let executable = executable.trim();
    if executable.chars().any(char::is_control) {
        return Err("The IDE executable cannot contain control characters.".into());
    }
    let arguments: Vec<String> = arguments
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if executable.is_empty() {
        if arguments.is_empty() {
            return Ok(IdeCommand::default());
        }
        return Err("Enter the IDE executable before its arguments.".into());
    }
    for argument in &arguments {
        if argument.chars().any(char::is_control) {
            return Err("IDE arguments cannot contain control characters.".into());
        }
        let mut rest = argument.as_str();
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                return Err("IDE arguments have an unclosed placeholder.".into());
            };
            let name = &after[..close];
            if !matches!(name, "project" | "file" | "line" | "column") {
                return Err(format!(
                    "Unknown IDE placeholder {{{name}}}; use {{project}}, {{file}}, {{line}} or {{column}}."
                ));
            }
            rest = &after[close + 1..];
        }
        if rest.contains('}') {
            return Err("IDE arguments have an unmatched closing brace.".into());
        }
    }
    Ok(IdeCommand {
        executable: executable.to_owned(),
        arguments,
    })
}

/// Validate a Content Finder URL pattern; blank keeps relative links.
pub(crate) fn url_pattern_value(value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("The URL pattern cannot contain spaces or control characters.".into());
    }
    if !value.contains("{slug}") {
        return Err("The URL pattern must contain {slug}.".into());
    }
    let without_slug = value.replace("{slug}", "");
    if without_slug.contains('{') || without_slug.contains('}') {
        return Err("Only the {slug} placeholder is supported.".into());
    }
    if !(value.starts_with('/') || value.starts_with("http://") || value.starts_with("https://")) {
        return Err("Start the pattern with / or with http:// or https://.".into());
    }
    Ok(Some(value.to_owned()))
}

/// Validate one field mapping against zero or more collection schemas.
pub(crate) fn mapping_value(
    role: MappingRole,
    value: &str,
    schemas: &[&CollectionSchema],
) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("Field names cannot contain spaces or control characters.".into());
    }
    if schemas.is_empty() {
        return Ok(Some(value.to_owned()));
    }
    let mut found = false;
    for schema in schemas {
        for field in &schema.fields {
            if field.name != value {
                continue;
            }
            found = true;
            if role.accepts(&field.kind) {
                return Ok(Some(value.to_owned()));
            }
        }
    }
    if found {
        Err(format!("“{value}” is not {}.", role.expected()))
    } else {
        Err(format!(
            "No collection schema declares a field named “{value}”."
        ))
    }
}

// ---------------------------------------------------------------------------
// Small helpers shared by the panes.
// ---------------------------------------------------------------------------

pub(super) fn directory(value: &str) -> Option<String> {
    directory_value(value).ok().flatten()
}

pub(super) fn mapping(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(super) fn heading_color(value: &str) -> Option<String> {
    heading_color_value(value).ok().flatten()
}

pub(super) fn url_pattern(value: &str) -> Option<String> {
    url_pattern_value(value).ok().flatten()
}

pub(super) fn ide_command(executable: &str, arguments: &str) -> Option<IdeCommand> {
    ide_command_value(executable, arguments).ok()
}

/// Parse the preferred browser argv. An empty executable inherits the system
/// opener and therefore must not carry arguments.
pub(super) fn browser_command(executable: &str, arguments: &str) -> Result<BrowserCommand, String> {
    let executable = executable.trim();
    if executable.chars().any(char::is_control) {
        return Err("The browser executable cannot contain control characters.".into());
    }
    let arguments: Vec<String> = arguments
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    if executable.is_empty() {
        if arguments.is_empty() {
            return Ok(BrowserCommand::default());
        }
        return Err("Enter the browser executable before its arguments.".into());
    }
    for argument in &arguments {
        if argument.chars().any(char::is_control) {
            return Err("Browser arguments cannot contain control characters.".into());
        }
        let mut rest = argument.as_str();
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let Some(close) = after.find('}') else {
                return Err("Browser arguments have an unclosed placeholder.".into());
            };
            let name = &after[..close];
            if name != "url" {
                return Err(format!(
                    "Unknown browser placeholder {{{name}}}; use {{url}}."
                ));
            }
            rest = &after[close + 1..];
        }
        if rest.contains('}') {
            return Err("Browser arguments have an unmatched closing brace.".into());
        }
    }
    Ok(BrowserCommand {
        executable: executable.to_owned(),
        arguments,
    })
}

pub(super) fn format_number(size: f32) -> String {
    if size.fract() == 0. {
        format!("{}", size as i64)
    } else {
        format!("{size}")
    }
}

fn collection_is_empty(preferences: &CollectionPreferencesV1) -> bool {
    preferences.content_directory.is_none()
        && preferences.assets_directory.is_none()
        && preferences.image_path_mode.is_none()
        && preferences.default_extension.is_none()
        && preferences.content_finder_url_pattern.is_none()
        && preferences.field_mappings == FieldMappings::default()
}
