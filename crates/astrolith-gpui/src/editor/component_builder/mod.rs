//! The MDX component builder: palette, typed form and insertion request.
//!
//! The palette and form are pure state ([`palette`], [`form`]); this module owns
//! the GPUI surface. Insertion itself stays in the project: the builder emits
//! the chosen component and values, and the project re-plans imports against the
//! live complete source before touching the document.

pub(crate) mod form;
pub(crate) mod palette;
pub(crate) mod snippets;

use std::{path::PathBuf, sync::Arc};

use astrolith_astro::components::{ClientDirectiveValue, Component, ComponentIndex, SnippetValue};
use astrolith_core::preferences::ImagePathMode;
use astrolith_workspace::assets::AssetLocation;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Disableable, IndexPath, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    command::{Command, CommandItem, CommandState},
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    PathPromptOptions, Render, SharedString, Subscription, Window, div, prelude::*, px,
};

use crate::frontmatter::{ReferenceCatalog, resolve_image_pick};
use crate::runtime;

use self::form::{FormState, PlannedFieldValue, PropControl};
use self::palette::Palette;
use self::snippets::starlight_prefill;

/// Indexed components and the services the form needs, resolved by the project.
#[derive(Clone)]
pub(crate) struct BuilderContext {
    pub index: Arc<ComponentIndex>,
    /// The project's import-insertion opt-in; never mutated here.
    pub imports_enabled: bool,
    pub references: ReferenceCatalog,
    pub images: Option<ImagePickContext>,
    /// The index is still being built; the palette shows a loading state.
    pub loading: bool,
    /// Why the last index attempt failed, if it did.
    pub error: Option<String>,
}

/// The parts of the image service the builder's picker needs.
#[derive(Clone)]
pub(crate) struct ImagePickContext {
    pub root: PathBuf,
    pub source: String,
    pub assets: Option<AssetLocation>,
    pub path_mode: ImagePathMode,
}

/// Everything the project needs to insert one chosen component.
#[derive(Clone)]
pub(crate) struct ComponentSelection {
    pub component: Component,
    pub values: Vec<(String, SnippetValue)>,
    pub planned: Vec<PlannedFieldValue>,
    pub content: Option<String>,
    pub client_directive: Option<ClientDirectiveValue>,
    /// Import insertion is requested for this insertion and allowed by the
    /// project setting.
    pub imports_enabled: bool,
}

/// What the builder asks the shell to do.
pub(crate) enum ComponentBuilderEvent {
    Insert(ComponentSelection),
}

impl EventEmitter<ComponentBuilderEvent> for ComponentBuilder {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Stage {
    Palette,
    Form,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BuilderShortcut {
    Back,
    Insert,
    ToggleOptional,
}

pub(crate) struct ComponentBuilder {
    context: Option<BuilderContext>,
    palette: Palette,
    query: String,
    hits: Vec<usize>,
    stage: Stage,
    form: Option<FormState>,
    /// One input entity per form field; `None` for non-textual controls.
    inputs: Vec<Option<Entity<InputState>>>,
    content_input: Option<Entity<TextareaState>>,
    directive_input: Option<Entity<InputState>>,
    /// Whether this insertion should add imports (project opt-in still wins).
    add_import: bool,
    search: Entity<CommandState>,
    notice: Option<SharedString>,
    error: Option<SharedString>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _keys: Subscription,
}

impl ComponentBuilder {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| CommandState::new(window, cx));
        let focus = cx.focus_handle();
        let builder_weak = cx.weak_entity();
        let keys = cx.intercept_keystrokes({
            let weak = builder_weak;
            move |event, window, cx| {
                let Some(shortcut) = builder_shortcut(&event.keystroke) else {
                    return;
                };
                let handled = weak
                    .update(cx, |builder, cx| {
                        builder.handle_shortcut(shortcut, window, cx)
                    })
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            }
        });
        Self {
            context: None,
            palette: Palette::default(),
            query: String::new(),
            hits: Vec::new(),
            stage: Stage::Palette,
            form: None,
            inputs: Vec::new(),
            content_input: None,
            directive_input: None,
            add_import: false,
            search,
            notice: None,
            error: None,
            focus,
            _subscriptions: Vec::new(),
            _keys: keys,
        }
    }

    /// Install a freshly indexed context and return to the palette.
    pub(crate) fn open(
        &mut self,
        context: BuilderContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.palette = Palette::new(context.index.components());
        self.add_import = context.imports_enabled;
        self.notice = palette_notice(&context);
        self.error = None;
        self.context = Some(context);
        self.clear_form();
        self.stage = Stage::Palette;
        self.query.clear();
        self.hits = self.palette.search("");
        self.search
            .update(cx, |state, cx| state.set_query("", window, cx));
        cx.notify();
    }

    /// Whether the builder holds a project context, so its dialog is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.context.is_some()
    }

    /// Focus the palette search, so the builder opens keyboard-ready.
    pub(crate) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.focus(window, cx));
    }

    /// Re-snapshot the project context after it republishes the component index.
    ///
    /// A builder opened while the index is still building starts with an empty
    /// palette; this replaces it with the published one without touching an open
    /// form. Returns whether anything the palette shows changed.
    pub(crate) fn refresh(&mut self, context: BuilderContext, cx: &mut Context<Self>) -> bool {
        let Some(current) = &self.context else {
            return false;
        };
        let changed = !Arc::ptr_eq(&current.index, &context.index)
            || current.loading != context.loading
            || current.error != context.error;
        self.context = Some(context);
        if !changed {
            return false;
        }
        if self.stage == Stage::Palette {
            let context = self.context.as_ref().expect("just installed");
            self.palette = Palette::new(context.index.components());
            self.notice = palette_notice(context);
            self.hits = self.palette.search(&self.query);
            cx.notify();
        }
        changed
    }

    /// Close the form and forget the component context.
    pub(crate) fn close(&mut self, cx: &mut Context<Self>) {
        self.context = None;
        self.palette = Palette::default();
        self.hits.clear();
        self.clear_form();
        self.stage = Stage::Palette;
        self.error = None;
        cx.notify();
    }

    /// The auto-import reminder, when the project declares the package.
    pub(crate) fn auto_import_warning(&self) -> Option<&'static str> {
        auto_import_warning(self.context.as_ref())
    }

    /// Index the current query; called from the search callback.
    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.hits = self.palette.search(&self.query);
        cx.notify();
    }

    fn handle_shortcut(
        &mut self,
        shortcut: BuilderShortcut,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.stage != Stage::Form || !self.focus.contains_focused(window, cx) {
            return false;
        }
        match shortcut {
            BuilderShortcut::Back => self.back(window, cx),
            BuilderShortcut::Insert => self.confirm(cx),
            BuilderShortcut::ToggleOptional => {
                if let Some(form) = &mut self.form {
                    form.toggle_all_optional();
                }
                cx.notify();
            }
        }
        true
    }

    fn open_form(&mut self, position: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self
            .hits
            .get(position)
            .and_then(|position| self.palette.get(*position))
        else {
            return;
        };
        let component = entry.component.clone();
        let content = starlight_prefill(&component).map(|prefill| prefill.content.to_owned());
        let mut form = FormState::new(&component, content);
        if let Some(context) = &self.context {
            let collections: Vec<String> = form
                .fields()
                .iter()
                .filter_map(|field| match PropControl::of(&field.prop) {
                    PropControl::Entry { collection } => Some(collection),
                    _ => None,
                })
                .collect();
            for collection in collections {
                let options = context
                    .references
                    .get(&collection)
                    .map(|options| {
                        options
                            .iter()
                            .map(|option| (option.label.clone(), option.value.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                form.set_entry_options(&collection, options);
            }
        }
        self.inputs = form
            .fields()
            .iter()
            .enumerate()
            .map(|(index, field)| {
                if !field.control.is_textual() {
                    return None;
                }
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .default_value(field.text().to_owned())
                        .placeholder(field_placeholder(field))
                });
                let text_index = index;
                self._subscriptions.push(cx.subscribe(
                    &input,
                    move |this, input, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            let value = input.read(cx).value().to_string();
                            if let Some(form) = &mut this.form {
                                form.set_text(text_index, value);
                            }
                            cx.notify();
                        }
                    },
                ));
                Some(input)
            })
            .collect();
        self.content_input = component.has_default_slot().then(|| {
            let content = form.content().to_owned();
            let input = cx.new(|cx| {
                TextareaState::new(window, cx)
                    .default_value(content)
                    .placeholder("Child content")
            });
            self._subscriptions.push(cx.subscribe(
                &input,
                move |this, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let value = input.read(cx).value().to_string();
                        if let Some(form) = &mut this.form {
                            form.set_content(value);
                        }
                        cx.notify();
                    }
                },
            ));
            input
        });
        self.directive_input = None;
        self.form = Some(form);
        self.stage = Stage::Form;
        self.error = None;
        let first_blank = self.form.as_ref().and_then(FormState::first_blank_required);
        let focus_index = first_blank.or_else(|| self.inputs.iter().position(Option::is_some));
        if let Some(input) = focus_index.and_then(|index| self.inputs.get(index).cloned().flatten())
        {
            input.update(cx, |input, cx| input.focus(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
        cx.notify();
    }

    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_form();
        self.stage = Stage::Palette;
        self.error = None;
        self.search
            .update(cx, |search, cx| search.focus(window, cx));
        cx.notify();
    }

    fn clear_form(&mut self) {
        self.form = None;
        self.inputs.clear();
        self.content_input = None;
        self.directive_input = None;
        self._subscriptions.clear();
    }

    /// Validate the form and ask the shell to insert it.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let (Some(context), Some(form)) = (self.context.as_ref(), self.form.as_ref()) else {
            self.error = Some(SharedString::from(
                "Choose a component before inserting it.",
            ));
            return;
        };
        let imports = self.add_import && context.imports_enabled;
        if let Err(error) = form.preview(imports) {
            self.error = Some(SharedString::from(error.to_string()));
            cx.notify();
            return;
        }
        let selection = ComponentSelection {
            component: form.component().clone(),
            values: form.values(),
            planned: form.planned_values(),
            content: (!form.content().trim().is_empty()).then(|| form.content().to_owned()),
            client_directive: form.client_directive(),
            imports_enabled: imports,
        };
        self.error = None;
        cx.emit(ComponentBuilderEvent::Insert(selection));
        cx.notify();
    }

    /// Surface an insertion failure without losing the form.
    pub(crate) fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn set_field_included(
        &mut self,
        index: usize,
        included: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.set_included(index, included);
        if included {
            // Re-including a field restores its displayed default or value.
            let Some(field) = form.fields().get(index) else {
                return;
            };
            let text = field.text().to_owned();
            if let Some(Some(input)) = self.inputs.get(index) {
                input.update(cx, |input, cx| input.set_value(text, window, cx));
            }
        }
        cx.notify();
    }

    fn set_entry(
        &mut self,
        index: usize,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.set_text(index, value.clone());
        if let Some(Some(input)) = self.inputs.get(index) {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn toggle_all_optional(&mut self, cx: &mut Context<Self>) {
        if let Some(form) = &mut self.form {
            form.toggle_all_optional();
            cx.notify();
        }
    }

    fn set_directive(
        &mut self,
        directive: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(form) = &mut self.form else {
            return;
        };
        form.set_directive(directive);
        let requires_value =
            form.component().client_directives.iter().any(|candidate| {
                Some(candidate.name) == form.directive() && candidate.requires_value
            });
        if requires_value {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(form.directive_value().to_owned())
                    .placeholder("Directive value")
            });
            self._subscriptions.push(cx.subscribe(
                &input,
                move |this, input, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let value = input.read(cx).value().to_string();
                        if let Some(form) = &mut this.form {
                            form.set_directive_value(value);
                        }
                        cx.notify();
                    }
                },
            ));
            input.update(cx, |input, cx| input.focus(window, cx));
            self.directive_input = Some(input);
        } else {
            self.directive_input = None;
        }
        cx.notify();
    }

    fn pick_image(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(images) = self
            .context
            .as_ref()
            .and_then(|context| context.images.clone())
        else {
            self.error = Some(SharedString::from(
                "This document has no project context for image picks.",
            ));
            cx.notify();
            return;
        };
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose an image".into()),
        });
        let weak = cx.weak_entity();
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(selected) = paths.into_iter().next() else {
                return;
            };
            let work = runtime::blocking(cx, move || {
                resolve_image_pick(
                    Some(images.root.clone()),
                    Some(images.source.clone()),
                    images.assets.clone(),
                    images.path_mode,
                    selected,
                )
            });
            let result = work.await.and_then(|result| result);
            let _ = weak.update_in(cx, |builder, window, cx| match result {
                Ok(value) => builder.set_entry(index, value, window, cx),
                Err(error) => {
                    builder.error = Some(SharedString::from(error));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Whether this insertion will add imports: the project opt-in and the
    /// form's own switch must both allow it.
    fn imports_for_insertion(&self) -> bool {
        self.add_import
            && self
                .context
                .as_ref()
                .is_some_and(|context| context.imports_enabled)
    }

    fn preview_text(&self) -> Result<String, String> {
        match &self.form {
            Some(form) => form
                .preview(self.imports_for_insertion())
                .map(|snippet| snippet.text)
                .map_err(|error| error.to_string()),
            None => Err("Choose a component before previewing a tag.".to_owned()),
        }
    }
}

/// Read-only views of the builder state for integration tests.
#[cfg(test)]
impl ComponentBuilder {
    /// Visible palette rows: name, package specifier (or `None` for a project
    /// component) and the required-prop summary.
    pub(crate) fn palette_rows(&self) -> Vec<(String, Option<String>, String)> {
        self.hits
            .iter()
            .filter_map(|position| self.palette.get(*position))
            .map(|entry| {
                let package = match &entry.group {
                    palette::ComponentGroup::Project => None,
                    palette::ComponentGroup::Package(specifier) => Some(specifier.clone()),
                };
                (
                    entry.component.name.clone(),
                    package,
                    entry.requirement.clone(),
                )
            })
            .collect()
    }

    pub(crate) fn stage_is_form(&self) -> bool {
        self.stage == Stage::Form
    }

    /// Whether the palette is still waiting for the project's index.
    pub(crate) fn is_loading(&self) -> bool {
        self.context.as_ref().is_some_and(|context| context.loading)
    }

    pub(crate) fn preview_tag(&self) -> Result<String, String> {
        self.preview_text()
    }
}

fn auto_import_warning(context: Option<&BuilderContext>) -> Option<&'static str> {
    let context = context?;
    context.index.auto_import().then_some(
        "This project declares astro-auto-import. Imports are still added only when you add them here.",
    )
}

/// The one line above the palette that explains an empty or pending index.
fn palette_notice(context: &BuilderContext) -> Option<SharedString> {
    if context.loading {
        Some(SharedString::from("Indexing components…"))
    } else if let Some(error) = &context.error {
        Some(SharedString::from(format!(
            "Components unavailable: {error}"
        )))
    } else {
        auto_import_warning(Some(context)).map(SharedString::from)
    }
}

fn field_placeholder(field: &form::PropField) -> SharedString {
    match &field.control {
        PropControl::Number => "Number".into(),
        PropControl::List => "Comma-separated values".into(),
        PropControl::Image => "Project image path".into(),
        PropControl::Entry { .. } => "Entry id".into(),
        PropControl::Date => "YYYY-MM-DD".into(),
        _ => field.prop.name.clone().into(),
    }
}

/// Platform-correct secondary modifier for Ctrl/Cmd+Enter.
fn secondary_enter(keystroke: &gpui_kit::Keystroke) -> bool {
    if keystroke.key != "enter" {
        return false;
    }
    let modifiers = keystroke.modifiers;
    if cfg!(target_os = "macos") {
        modifiers.platform && !modifiers.control
    } else {
        modifiers.control && !modifiers.platform
    }
}

fn builder_shortcut(keystroke: &gpui_kit::Keystroke) -> Option<BuilderShortcut> {
    if secondary_enter(keystroke) {
        return Some(BuilderShortcut::Insert);
    }
    if keystroke.key == "escape" {
        return Some(BuilderShortcut::Back);
    }
    if keystroke.key == "o" && keystroke.modifiers.alt && !keystroke.modifiers.control {
        return Some(BuilderShortcut::ToggleOptional);
    }
    None
}

impl Focusable for ComponentBuilder {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ComponentBuilder {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        let stage = self.stage;
        let mut root = div()
            .v_flex()
            .gap_3()
            .p_3()
            .track_focus(&self.focus)
            .when_some(self.notice.clone(), |root, notice| {
                root.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(notice),
                )
            });
        if stage == Stage::Palette {
            root = root.child(self.palette_view(&weak, window, cx));
        } else {
            root = root.child(self.form_view(&weak, cx));
        }
        root
    }
}

impl ComponentBuilder {
    fn palette_view(
        &self,
        weak: &gpui_kit::WeakEntity<Self>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let headers = self.palette.headers(&self.hits);
        let rows: Vec<PaletteRow> = self
            .hits
            .iter()
            .zip(headers)
            .filter_map(|(position, header)| {
                let entry = self.palette.get(*position)?;
                Some(PaletteRow {
                    header: header.map(str::to_owned),
                    name: entry.component.name.clone(),
                    badge: entry.badge,
                    requirement: entry.requirement.clone(),
                    description: entry.description().map(str::to_owned),
                    wraps: entry.wraps,
                })
            })
            .collect();
        let hits = self.hits.clone();
        let query_weak = weak.clone();
        let confirm_weak = weak.clone();
        let mut view = div().v_flex().gap_2();
        if self.palette.is_empty() {
            view = view.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        if self.context.as_ref().is_some_and(|context| context.loading) {
                            "Indexing components…"
                        } else {
                            "No components were found in this project."
                        },
                    ),
            );
        }
        view.child(
            Command::new(&self.search)
                .bordered(false)
                .placeholder("Search components by name…")
                .filterable(false)
                .on_query(move |query, _, cx| {
                    let query = query.to_owned();
                    let weak = query_weak.clone();
                    cx.defer(move |cx| {
                        let _ = weak.update(cx, |builder, cx| builder.set_query(query, cx));
                    });
                })
                .items(rows.into_iter().map(|row| {
                    CommandItem::new()
                        .label(row.name.clone())
                        .icon(IconName::FileText)
                        .child(move |_, cx| {
                            div()
                                .v_flex()
                                .w_full()
                                .gap_0p5()
                                .when_some(row.header.clone(), |column, header| {
                                    column.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(header),
                                    )
                                })
                                .child(
                                    div()
                                        .h_flex()
                                        .items_center()
                                        .gap_2()
                                        .child(div().text_sm().child(row.name.clone()))
                                        .child(badge(row.badge, cx))
                                        .when(row.wraps, |line| {
                                            line.child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("wraps content"),
                                            )
                                        }),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(row.requirement.clone()),
                                )
                                .when_some(row.description.clone(), |column, description| {
                                    column.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(description),
                                    )
                                })
                        })
                }))
                .empty(|_, _, cx| {
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No matching components")
                })
                .on_confirm(move |index: IndexPath, window, cx| {
                    let position = index.row;
                    if hits.get(position).is_none() {
                        return;
                    }
                    let _ = confirm_weak
                        .update(cx, |builder, cx| builder.open_form(position, window, cx));
                }),
        )
    }

    fn form_view(
        &self,
        weak: &gpui_kit::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(form) = self.form.as_ref() else {
            return div().into_any_element();
        };
        let component = form.component();
        let mut view = div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child({
                        let back = weak.clone();
                        Button::new("component-builder-back")
                            .ghost()
                            .xsmall()
                            .label("Back")
                            .on_click(move |_, window, cx| {
                                let _ = back.update(cx, |builder, cx| builder.back(window, cx));
                            })
                    })
                    .child(hint("Esc", cx))
                    .child(div().flex_1())
                    .child(div().text_sm().child(component.name.clone()))
                    .child(badge(component.framework.label(), cx))
                    .when(component.has_default_slot(), |row| {
                        row.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("wraps content"),
                        )
                    }),
            )
            .when_some(component.description.clone(), |view, description| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description),
                )
            });
        if let Some(warning) = self.auto_import_warning() {
            view = view.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(warning),
            );
        }
        let fields_view = self.fields_view(weak, cx);
        view = view.child(fields_view);
        view.into_any_element()
    }

    fn fields_view(
        &self,
        weak: &gpui_kit::WeakEntity<Self>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let Some(form) = self.form.as_ref() else {
            return div().into_any_element();
        };
        let mut fields = div().v_flex().gap_2();
        for (index, field) in form.fields().iter().enumerate() {
            let required = field.prop.required;
            let mut label = div()
                .h_flex()
                .items_center()
                .gap_1()
                .child(div().text_xs().child(field.prop.name.clone()))
                .when(required, |label| {
                    label.child(div().text_xs().text_color(cx.theme().red).child("*"))
                });
            if let Some(default) = field.default_label() {
                label = label.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("default: {default}")),
                );
            }
            if !required {
                let toggle = weak.clone();
                let included = field.included();
                label = label.child(div().flex_1()).child(
                    Checkbox::new(("component-field-include", index))
                        .checked(included)
                        .label("Add")
                        .on_click(move |checked: &bool, window, cx| {
                            let _ = toggle.update(cx, |builder, cx| {
                                builder.set_field_included(index, *checked, window, cx)
                            });
                        }),
                );
            }
            let mut row = div()
                .v_flex()
                .gap_1()
                .when(required, |row| row.opacity(1.))
                .child(label);
            if let Some(description) = &field.prop.description {
                row = row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.clone()),
                );
            }
            row = row.child(match &field.control {
                PropControl::Boolean => {
                    let toggle = weak.clone();
                    let flag = field.flag();
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Checkbox::new(("component-field-bool", index))
                                .checked(flag)
                                .label(if flag { "true" } else { "false" })
                                .on_click(move |checked: &bool, _window, cx| {
                                    let _ = toggle.update(cx, |builder, cx| {
                                        if let Some(form) = &mut builder.form {
                                            form.set_flag(index, *checked);
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                        .into_any_element()
                }
                PropControl::Enum { options } => {
                    let selected = if field.text().is_empty() {
                        "Choose…".to_owned()
                    } else {
                        field.text().to_owned()
                    };
                    let options = options.clone();
                    let pick = weak.clone();
                    Button::new(("component-field-enum", index))
                        .xsmall()
                        .outline()
                        .label(selected)
                        .dropdown_caret(true)
                        .dropdown_menu(move |menu, _, _| {
                            let mut menu = menu;
                            if !required {
                                let clear = pick.clone();
                                menu = menu.item(PopupMenuItem::new("Clear").on_click(
                                    move |_, window, cx| {
                                        let _ = clear.update(cx, |builder, cx| {
                                            if let Some(form) = &mut builder.form {
                                                form.set_text(index, "");
                                            }
                                            if let Some(Some(input)) = builder.inputs.get(index) {
                                                input.update(cx, |input, cx| {
                                                    input.set_value("", window, cx)
                                                });
                                            }
                                            cx.notify();
                                        });
                                    },
                                ));
                            }
                            for option in &options {
                                let pick = pick.clone();
                                let value = option.clone();
                                menu = menu.item(PopupMenuItem::new(option.clone()).on_click(
                                    move |_, window, cx| {
                                        let _ = pick.update(cx, |builder, cx| {
                                            builder.set_entry(index, value.clone(), window, cx)
                                        });
                                    },
                                ));
                            }
                            menu
                        })
                        .into_any_element()
                }
                PropControl::Entry { collection } => {
                    let label = if field.text().is_empty() {
                        format!("Choose a {collection} entry…")
                    } else {
                        field.text().to_owned()
                    };
                    let options = field.entries.clone();
                    let pick = weak.clone();
                    let text_input = self.inputs.get(index).cloned().flatten();
                    div()
                        .v_flex()
                        .gap_1()
                        .when_some(text_input, |column, input| {
                            column.child(Input::new(&input).small())
                        })
                        .child(
                            Button::new(("component-field-entry", index))
                                .xsmall()
                                .outline()
                                .label(label)
                                .dropdown_caret(true)
                                .dropdown_menu(move |menu, _, _| {
                                    let mut menu = menu;
                                    if options.is_empty() {
                                        return menu.item(PopupMenuItem::label("No entries"));
                                    }
                                    for (label, value) in &options {
                                        let pick = pick.clone();
                                        let value = value.clone();
                                        menu =
                                            menu.item(PopupMenuItem::new(label.clone()).on_click(
                                                move |_, window, cx| {
                                                    let _ = pick.update(cx, |builder, cx| {
                                                        builder.set_entry(
                                                            index,
                                                            value.clone(),
                                                            window,
                                                            cx,
                                                        )
                                                    });
                                                },
                                            ));
                                    }
                                    menu
                                }),
                        )
                        .into_any_element()
                }
                PropControl::Image => {
                    let pick = weak.clone();
                    let text_input = self.inputs.get(index).cloned().flatten();
                    div()
                        .v_flex()
                        .gap_1()
                        .when_some(text_input, |column, input| {
                            column.child(Input::new(&input).small())
                        })
                        .child(
                            Button::new(("component-field-image", index))
                                .xsmall()
                                .outline()
                                .label("Choose image…")
                                .on_click(move |_, window, cx| {
                                    let _ = pick.update(cx, |builder, cx| {
                                        builder.pick_image(index, window, cx)
                                    });
                                }),
                        )
                        .into_any_element()
                }
                _ => match self.inputs.get(index).cloned().flatten() {
                    Some(input) => Input::new(&input).small().into_any_element(),
                    None => div().into_any_element(),
                },
            });
            fields = fields.child(row);
        }
        // Client directives apply to hydrated framework components only.
        if !form.component().client_directives.is_empty() {
            let directives = form.component().client_directives.clone();
            let selected = form.directive().map(str::to_owned);
            let pick = weak.clone();
            fields = fields.child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_xs().child("client directive"))
                    .child(
                        Button::new("component-field-directive")
                            .xsmall()
                            .outline()
                            .label(selected.clone().unwrap_or_else(|| "None".into()))
                            .dropdown_caret(true)
                            .dropdown_menu(move |menu, _, _| {
                                let mut menu = menu;
                                let clear = pick.clone();
                                menu = menu.item(PopupMenuItem::new("None").on_click(
                                    move |_, window, cx| {
                                        let _ = clear.update(cx, |builder, cx| {
                                            builder.set_directive(None, window, cx)
                                        });
                                    },
                                ));
                                for directive in &directives {
                                    let pick = pick.clone();
                                    let name = directive.name.to_owned();
                                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(
                                        move |_, window, cx| {
                                            let _ = pick.update(cx, |builder, cx| {
                                                builder.set_directive(
                                                    Some(name.clone()),
                                                    window,
                                                    cx,
                                                )
                                            });
                                        },
                                    ));
                                }
                                menu
                            }),
                    )
                    .when_some(self.directive_input.clone(), |column, input| {
                        column.child(Input::new(&input).small())
                    }),
            );
        }
        if let Some(content) = self.content_input.clone() {
            fields = fields.child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_xs().child("child content"))
                    .child(Textarea::new(&content).small().h(px(96.))),
            );
        }
        // Optional-prop shortcut and the import switch.
        fields = fields.child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child({
                    let toggle = weak.clone();
                    Button::new("component-field-optional")
                        .ghost()
                        .xsmall()
                        .label(if form.all_optional_included() {
                            "Exclude optional props"
                        } else {
                            "Include optional props"
                        })
                        .on_click(move |_, _, cx| {
                            let _ =
                                toggle.update(cx, |builder, cx| builder.toggle_all_optional(cx));
                        })
                })
                .child(hint("Alt+O", cx)),
        );
        if let Some(context) = &self.context {
            fields = fields.child(if context.imports_enabled {
                let toggle = weak.clone();
                let checked = self.add_import;
                div()
                    .h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Checkbox::new("component-field-import")
                            .checked(checked)
                            .label("Add import for this insertion")
                            .on_click(move |checked: &bool, _, cx| {
                                let _ = toggle.update(cx, |builder, cx| {
                                    builder.add_import = *checked;
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Off leaves the import untouched."),
                    )
                    .into_any_element()
            } else {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Import insertion is off for this project.")
                    .into_any_element()
            });
        }
        // Preview plus the commit actions.
        let preview = self.preview_text();
        let preview_failed = preview.is_err();
        let provisional = self.imports_for_insertion()
            && form.fields().iter().any(|field| {
                matches!(
                    field.control,
                    PropControl::Image | PropControl::Entry { .. }
                )
            });
        fields = fields.child(
            div()
                .v_flex()
                .gap_1()
                .child(div().text_xs().child("Preview"))
                .child(match preview {
                    Ok(text) => div()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().group_box)
                        .text_xs()
                        .child(text)
                        .into_any_element(),
                    Err(error) => div()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().group_box)
                        .text_xs()
                        .text_color(cx.theme().red)
                        .child(error)
                        .into_any_element(),
                })
                .when(provisional, |view| {
                    view.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                "Image and entry identifiers are provisional; the import plan settles the final names when you insert.",
                            ),
                    )
                }),
        );
        if let Some(error) = self.error.clone() {
            fields = fields.child(div().text_xs().text_color(cx.theme().red).child(error));
        }
        fields = fields.child(
            div()
                .h_flex()
                .items_center()
                .gap_2()
                .child({
                    let insert = weak.clone();
                    Button::new("component-field-insert")
                        .primary()
                        .small()
                        .label("Insert")
                        .disabled(preview_failed)
                        .on_click(move |_, _, cx| {
                            let _ = insert.update(cx, |builder, cx| builder.confirm(cx));
                        })
                })
                .child(hint("Ctrl+Enter", cx)),
        );
        fields.into_any_element()
    }
}

struct PaletteRow {
    /// Group header, shown only on the first row of each group.
    header: Option<String>,
    name: String,
    badge: &'static str,
    requirement: String,
    description: Option<String>,
    wraps: bool,
}

fn hint(text: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_1()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

fn badge(label: &'static str, cx: &App) -> impl IntoElement {
    div()
        .px_1p5()
        .py_0p5()
        .rounded(cx.theme().radius)
        .bg(cx.theme().muted)
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(label)
}
