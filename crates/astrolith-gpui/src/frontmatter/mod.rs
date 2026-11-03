//! Schema-driven frontmatter forms for the Inspector panel.
//!
//! [`fields`] maps normalized [`FieldKind`](astrolith_astro::schema::FieldKind)
//! values and parsed document shapes to native controls, orders them exactly as
//! the panel requires and applies edits through the lossless
//! [`ParsedDocument`] patch contract. [`FrontmatterEditor`] is the GUI-free model
//! that holds one parsed document, its controls and its validation feedback; the
//! [`FrontmatterForm`] GPUI entity renders that model with GPUI Kit controls.
//!
//! The editor never touches the filesystem: parsing, editing and serialization
//! all operate on in-memory text. Persisting the result is the shell's save
//! coordinator's job, so only supported edits ever reach disk.

pub mod fields;

use std::{path::PathBuf, sync::Arc};

use astrolith_astro::schema::{
    CollectionSchema, FieldKind, ValidationDiagnostic, validate_frontmatter,
};
use astrolith_core::preferences::{FieldMappings, ImagePathMode};
use astrolith_document::{
    DocumentError, FieldShape, FrontmatterField, FrontmatterValue, ParsedDocument, parse_document,
};
use astrolith_workspace::assets::{
    AssetCopyRequest, AssetLocation, asset_path, copy_asset, project_relative_path,
};
use chrono::NaiveDate;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    calendar::Date,
    checkbox::Checkbox,
    date_picker::{DatePicker, DatePickerEvent, DatePickerState, DateTime},
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, EventEmitter, IntoElement, ObjectFit,
    PathPromptOptions, Render, SharedString, Subscription, Window, div, img, prelude::*, px,
};

use crate::editor::image_preview::{self, PreviewSource};
use crate::runtime;
use serde_json::{Map, Value};

pub use fields::{
    ControlKind, FieldControl, FieldEditError, FieldRole, FrontmatterEdit, ReferenceCatalog,
    control_label, slugify,
};

/// The GUI-free form model for one document's frontmatter.
///
/// Holds the parsed document, the resolved control list and the latest local
/// validation diagnostics. Editing a value records a lossless patch, re-parses
/// the re-serialized text (so subsequent reads see the edited value) and
/// recomputes diagnostics. A collection without a schema still gets a form
/// built from the fields that exist in the document.
#[derive(Clone, Debug)]
pub struct FrontmatterEditor {
    schema: Option<Arc<CollectionSchema>>,
    mappings: FieldMappings,
    controls: Vec<FieldControl>,
    document: ParsedDocument,
    text: String,
    diagnostics: Vec<ValidationDiagnostic>,
}

impl FrontmatterEditor {
    /// Build a form model for `text`.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the document frontmatter is malformed or
    /// uses a construct outside the lossless subset (for example anchors, aliases
    /// or tags). The caller must surface that error instead of guessing.
    pub fn new(
        schema: Option<Arc<CollectionSchema>>,
        mappings: FieldMappings,
        text: String,
    ) -> Result<Self, DocumentError> {
        let document = parse_document(&text)?;
        let controls = fields::build_controls(schema.as_deref(), &mappings, document.fields());
        let mut editor = Self {
            schema,
            mappings,
            controls,
            document,
            text,
            diagnostics: Vec::new(),
        };
        editor.revalidate();
        Ok(editor)
    }

    /// Return the resolved controls in render order.
    pub fn controls(&self) -> &[FieldControl] {
        &self.controls
    }

    /// Return the current full document text (frontmatter plus body).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Return the current local validation diagnostics.
    pub fn diagnostics(&self) -> &[ValidationDiagnostic] {
        &self.diagnostics
    }

    /// Return the control for a dotted path, if any.
    pub fn control(&self, path: &str) -> Option<&FieldControl> {
        self.controls.iter().find(|control| control.path == path)
    }

    /// Return the current typed scalar value at `path`, if readable.
    pub fn value(&self, path: &str) -> Option<FrontmatterValue> {
        self.document.field(path).cloned()
    }

    /// Return the current sequence items at `path` (empty when absent).
    pub fn items(&self, path: &str) -> Vec<FrontmatterValue> {
        self.document
            .field_info(path)
            .and_then(|info| info.items.clone())
            .unwrap_or_default()
    }

    /// Return the exact raw YAML text for a container field, if any.
    pub fn raw(&self, path: &str) -> Option<&str> {
        self.document
            .field_info(path)
            .and_then(|info| info.raw.as_deref())
    }

    /// Return the diagnostics whose path matches `path` (leaf or descendant).
    pub fn diagnostics_for(&self, path: &str) -> impl Iterator<Item = &ValidationDiagnostic> {
        let prefix = format!("{path}.");
        self.diagnostics.iter().filter(move |diagnostic| {
            diagnostic.path == path || diagnostic.path.starts_with(&prefix)
        })
    }

    /// Apply a raw string edit to the field at `path`.
    ///
    /// Returns the new full document text on success. Read-only fields return
    /// their explicit refusal reason; unparsable input returns a value error.
    pub fn set_value(&mut self, path: &str, raw: &str) -> Result<String, String> {
        let control = self
            .control(path)
            .ok_or_else(|| format!("unknown frontmatter field `{path}`"))?
            .clone();
        if !control.editable {
            return Err(control
                .refusal
                .unwrap_or_else(|| "this field is read-only".to_owned()));
        }
        if matches!(
            control.kind,
            ControlKind::Tags | ControlKind::Reference { multiple: true, .. }
        ) {
            // A list control must never be replaced by one scalar; items are
            // added and removed individually through `add_item`/`remove_item`.
            return Err("list fields are edited item by item".to_owned());
        }
        let mut document = self.document.clone();
        match &control.kind {
            ControlKind::RawYaml { .. } => {
                fields::apply_raw_edit(&mut document, path, raw)
                    .map_err(|error| error.to_string())?;
            }
            _ => {
                fields::apply_scalar_edit(&mut document, path, &control.kind, raw, control.present)
                    .map_err(|error| error.to_string())?
            }
        }
        self.commit(document)
    }

    /// Add one string/reference item to a sequence field, inserting the field
    /// when it is absent.
    pub fn add_item(&mut self, path: &str, raw: &str) -> Result<String, String> {
        let control = self
            .control(path)
            .ok_or_else(|| format!("unknown frontmatter field `{path}`"))?
            .clone();
        if !control.editable
            || !matches!(
                control.kind,
                ControlKind::Tags | ControlKind::Reference { multiple: true, .. }
            )
        {
            return Err(control
                .refusal
                .unwrap_or_else(|| "this field does not accept list items".to_owned()));
        }
        let value = FrontmatterValue::String(raw.to_owned());
        let mut document = self.document.clone();
        if control.present {
            let index = self.items(path).len();
            document
                .insert_sequence_item(path, index, value)
                .map_err(|error| error.to_string())?;
        } else {
            document
                .insert_sequence(path, &[value])
                .map_err(|error| error.to_string())?;
        }
        self.commit(document)
    }

    /// Remove one sequence item.
    pub fn remove_item(&mut self, path: &str, index: usize) -> Result<String, String> {
        let mut document = self.document.clone();
        document
            .remove_sequence_item(path, index)
            .map_err(|error| error.to_string())?;
        self.commit(document)
    }

    /// Remove an optional field.
    pub fn remove_field(&mut self, path: &str) -> Result<String, String> {
        let control = self
            .control(path)
            .ok_or_else(|| format!("unknown frontmatter field `{path}`"))?
            .clone();
        if control.required {
            return Err("a required field cannot be removed".to_owned());
        }
        let mut document = self.document.clone();
        document
            .remove_field(path)
            .map_err(|error| error.to_string())?;
        self.commit(document)
    }

    fn commit(&mut self, document: ParsedDocument) -> Result<String, String> {
        let text = document.serialize().map_err(|error| error.to_string())?;
        let parsed = parse_document(&text).map_err(|error| error.to_string())?;
        self.document = parsed;
        self.text = text.clone();
        self.controls = fields::build_controls(
            self.schema.as_deref(),
            &self.mappings,
            self.document.fields(),
        );
        self.revalidate();
        Ok(text)
    }

    fn revalidate(&mut self) {
        let values = self.read_values();
        let Some(schema) = self.schema.as_deref() else {
            self.diagnostics.clear();
            return;
        };
        let present: std::collections::BTreeSet<&str> = self
            .document
            .fields()
            .iter()
            .map(|info| info.path.as_str())
            .collect();
        self.diagnostics = validate_frontmatter(schema, &values)
            .into_iter()
            .filter(|diagnostic| {
                // A field that exists with an unreadable shape is not missing.
                !(diagnostic.message == "required field is missing"
                    && present.contains(diagnostic.path.as_str()))
            })
            .collect();
    }

    /// Build a JSON object of the readable document values for validation.
    fn read_values(&self) -> Value {
        let mut root = Map::new();
        for info in self.document.fields() {
            let Some(value) = readable_value(info) else {
                continue;
            };
            insert_path(&mut root, &info.path, value);
        }
        Value::Object(root)
    }
}

fn readable_value(info: &FrontmatterField) -> Option<Value> {
    match info.shape {
        FieldShape::Scalar => info.value.as_ref().map(fields::frontmatter_to_json),
        FieldShape::Null => Some(Value::Null),
        FieldShape::Mapping => Some(Value::Object(Map::new())),
        FieldShape::Sequence => info
            .items
            .as_ref()
            .map(|items| Value::Array(items.iter().map(fields::frontmatter_to_json).collect())),
        FieldShape::Flow | FieldShape::BlockScalar => None,
    }
}

fn insert_path(root: &mut Map<String, Value>, path: &str, value: Value) {
    let mut segments = path.split('.').peekable();
    let mut current = root;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            if value != Value::Object(Map::new()) || !current.contains_key(segment) {
                current.insert(segment.to_owned(), value);
            }
            return;
        }
        let entry = current
            .entry(segment.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        current = entry.as_object_mut().expect("object inserted above");
    }
}

/// Capitalized presentation of a control family.
fn kind_display(label: &'static str) -> &'static str {
    match label {
        "text" => "Text",
        "number" => "Number",
        "integer" => "Integer",
        "boolean" => "Boolean",
        "date" => "Date",
        "image" => "Image",
        "enum" => "Choice",
        "reference" => "Reference",
        "reference list" => "References",
        "tags" => "Tags",
        "object" => "Group",
        "raw yaml" => "YAML",
        "literal" => "Fixed",
        "read-only" => "Read only",
        other => other,
    }
}

/// Short label for a schema leaf kind (used for tests and diagnostics).
pub fn kind_label(kind: &FieldKind) -> &'static str {
    match kind {
        FieldKind::String { .. } => "string",
        FieldKind::Number => "number",
        FieldKind::Integer => "integer",
        FieldKind::Boolean => "boolean",
        FieldKind::Date => "date",
        FieldKind::Image => "image",
        FieldKind::Reference { .. } => "reference",
        FieldKind::Enum { .. } => "enum",
        FieldKind::Literal { .. } => "literal",
        FieldKind::Object { .. } => "object",
        FieldKind::Array { .. } => "array",
        FieldKind::Record { .. } => "record",
        FieldKind::Nullable { .. } => "nullable",
        FieldKind::Unsupported { .. } => "unsupported",
    }
}

/// One editable row: the control plus whatever input entity it needs.
struct RowEditor {
    control: FieldControl,
    input: Option<Entity<InputState>>,
    textarea: Option<Entity<TextareaState>>,
    date: Option<Entity<DatePickerState>>,
    add_input: Option<Entity<InputState>>,
    /// Presentation strings are built when the form loads, not per frame.
    label: SharedString,
    path: SharedString,
    kind_label: &'static str,
    hierarchy: usize,
    is_object: bool,
    read_only_label: Option<SharedString>,
    description: Option<SharedString>,
    hint: Option<SharedString>,
    aria_label: SharedString,
    /// (display, raw) pairs for enum controls.
    enum_values: Vec<(SharedString, SharedString)>,
    /// Options for reference controls, ordered by label.
    reference_options: Vec<(SharedString, SharedString)>,
    /// Current sequence items as (true index, display string) pairs (tags and
    /// reference lists). Null items are hidden but keep their position, so chip
    /// removal never targets the wrong item.
    items: Vec<(usize, SharedString)>,
    /// Current raw YAML text for container fields.
    raw: Option<SharedString>,
    is_title: bool,
    _subscriptions: Vec<Subscription>,
}

/// Asset context shared by every image row in one render.
struct RowImageContext {
    root: Option<PathBuf>,
    source: Option<String>,
    assets: Option<AssetLocation>,
    path_mode: ImagePathMode,
}

/// Everything the form needs to render one document.
pub struct FrontmatterDocument {
    /// Generated collection schema, or `None` for a schema-less collection.
    pub schema: Option<Arc<CollectionSchema>>,
    /// Resolved field mappings for the collection.
    pub mappings: FieldMappings,
    /// Reference options for every schema reference target.
    pub references: Arc<ReferenceCatalog>,
    /// Canonical project root used to validate in-project image paths.
    pub root: Option<PathBuf>,
    /// Project-relative source of the document holding this frontmatter.
    pub source: Option<String>,
    /// Resolved assets directory for external image copies.
    pub assets: Option<astrolith_workspace::assets::AssetLocation>,
    /// Relative or project-root generated image paths.
    pub path_mode: astrolith_core::preferences::ImagePathMode,
    /// Complete document text, including unsaved edits.
    pub text: String,
}

/// A GPUI entity that renders a [`FrontmatterEditor`] with native controls.
pub struct FrontmatterForm {
    editor: Option<FrontmatterEditor>,
    rows: Vec<RowEditor>,
    references: Arc<ReferenceCatalog>,
    root: Option<PathBuf>,
    source: Option<String>,
    assets: Option<astrolith_workspace::assets::AssetLocation>,
    path_mode: astrolith_core::preferences::ImagePathMode,
    load_error: Option<SharedString>,
    notice: Option<SharedString>,
}

impl EventEmitter<FrontmatterEdit> for FrontmatterForm {}

impl FrontmatterForm {
    /// Create an empty form showing a placeholder.
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            editor: None,
            rows: Vec::new(),
            references: Arc::new(ReferenceCatalog::new()),
            root: None,
            source: None,
            assets: None,
            path_mode: astrolith_core::preferences::ImagePathMode::Relative,
            load_error: None,
            notice: None,
        }
    }

    /// Return whether a document is currently loaded.
    pub fn has_document(&self) -> bool {
        self.editor.is_some()
    }

    /// Return the current editor model, if any.
    pub fn editor(&self) -> Option<&FrontmatterEditor> {
        self.editor.as_ref()
    }

    /// Load one document into the form.
    pub fn set_document(
        &mut self,
        document: FrontmatterDocument,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.references = document.references;
        self.root = document.root;
        self.source = document.source;
        self.assets = document.assets;
        self.path_mode = document.path_mode;
        self.notice = None;
        match FrontmatterEditor::new(document.schema, document.mappings, document.text) {
            Ok(editor) => {
                self.load_error = None;
                self.rows = build_rows(&editor, &self.references, window, cx);
                self.editor = Some(editor);
            }
            Err(error) => {
                // Malformed or unsupported frontmatter stays in the source
                // editor; the panel shows the explicit refusal reason.
                self.load_error = Some(SharedString::from(error.to_string()));
                self.editor = None;
                self.rows.clear();
            }
        }
        cx.notify();
    }

    /// Clear the form back to its placeholder.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.rows.clear();
        self.load_error = None;
        self.notice = None;
        cx.notify();
    }

    /// Apply a raw edit at `path` and emit the document edit.
    pub fn apply_value(&mut self, path: &str, raw: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(control) = editor.control(path).cloned() else {
            return;
        };
        if !control.editable {
            return;
        }
        if matches!(control.kind, ControlKind::RawYaml { .. }) {
            match editor.set_value(path, raw) {
                Ok(_) => {
                    cx.emit(FrontmatterEdit::Raw {
                        path: path.to_owned(),
                        raw: raw.to_owned(),
                    });
                    self.sync_rows(cx);
                }
                Err(error) => self.notice = Some(SharedString::from(error)),
            }
            cx.notify();
            return;
        }
        let Ok(value) = fields::parse_value(&control.kind, raw) else {
            return;
        };
        if let Err(error) = editor.set_value(path, raw) {
            self.notice = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        cx.emit(FrontmatterEdit::Scalar {
            path: path.to_owned(),
            value,
            present: control.present,
        });
        self.sync_rows(cx);
        cx.notify();
    }

    /// Add one tag/reference item and emit the edit.
    pub fn apply_add_item(&mut self, path: &str, raw: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(control) = editor.control(path).cloned() else {
            return;
        };
        if !control.editable {
            return;
        }
        let index = editor.items(path).len();
        if let Err(error) = editor.add_item(path, raw) {
            self.notice = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        cx.emit(FrontmatterEdit::AddItem {
            path: path.to_owned(),
            index,
            value: FrontmatterValue::String(raw.to_owned()),
        });
        self.sync_rows(cx);
        cx.notify();
    }

    /// Remove one sequence item and emit the edit.
    pub fn apply_remove_item(&mut self, path: &str, index: usize, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if let Err(error) = editor.remove_item(path, index) {
            self.notice = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        cx.emit(FrontmatterEdit::RemoveItem {
            path: path.to_owned(),
            index,
        });
        self.sync_rows(cx);
        cx.notify();
    }

    /// Remove an optional field and emit the edit.
    pub fn apply_remove_field(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if let Err(error) = editor.remove_field(path) {
            self.notice = Some(SharedString::from(error));
            cx.notify();
            return;
        }
        cx.emit(FrontmatterEdit::Remove {
            path: path.to_owned(),
        });
        self.sync_rows(cx);
        cx.notify();
    }

    /// Copy the slugified title to the clipboard only.
    ///
    /// No document text and no file changes; the value comes from the current
    /// form buffer even when the file has never been saved.
    pub fn copy_slug(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let Some(control) = editor
            .controls()
            .iter()
            .find(|control| control.role == Some(FieldRole::Title))
        else {
            return;
        };
        let title = match editor.value(&control.path) {
            Some(FrontmatterValue::String(text)) => text,
            Some(FrontmatterValue::Integer(value)) => value.to_string(),
            Some(FrontmatterValue::Float(value)) => value.to_string(),
            _ => String::new(),
        };
        let slug = slugify(&title);
        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(slug));
        self.notice = Some(SharedString::from("Slug copied to the clipboard"));
        cx.notify();
    }

    /// Apply a project-relative image path through a normal scalar edit.
    fn set_image_path(&mut self, path: &str, value: String, cx: &mut Context<Self>) {
        self.apply_value(path, &value, cx);
    }

    fn sync_rows(&mut self, _cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            self.rows.clear();
            return;
        };
        for row in &mut self.rows {
            let Some(control) = editor.control(&row.path) else {
                continue;
            };
            row.control.present = control.present;
            row.control.removable = control.removable;
            row.control.editable = control.editable;
            row.control.refusal = control.refusal.clone();
            row.items = sequence_display(editor, &row.path);
            row.raw = editor.raw(&row.path).map(SharedString::from);
            row.read_only_label = read_only_label(&row.control, row.is_object);
        }
    }

    /// Return the row control paths currently rendered (test/diagnostic helper).
    pub fn control_paths(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.control.path.clone())
            .collect()
    }

    /// Return the visible row kinds in order (test/diagnostic helper).
    pub fn control_kinds(&self) -> Vec<&'static str> {
        self.rows
            .iter()
            .map(|row| row.control.control_label())
            .collect()
    }
}

fn sequence_display(editor: &FrontmatterEditor, path: &str) -> Vec<(usize, SharedString)> {
    editor
        .items(path)
        .into_iter()
        .enumerate()
        .filter_map(|(index, value)| match value {
            FrontmatterValue::Null => None,
            FrontmatterValue::Boolean(flag) => Some((index, SharedString::from(flag.to_string()))),
            FrontmatterValue::Integer(value) => {
                Some((index, SharedString::from(value.to_string())))
            }
            FrontmatterValue::Float(value) => Some((index, SharedString::from(value.to_string()))),
            FrontmatterValue::String(text) => Some((index, SharedString::from(text))),
        })
        .collect()
}

fn read_only_label(control: &FieldControl, is_object: bool) -> Option<SharedString> {
    if control.editable || is_object {
        return None;
    }
    let refusal = control.refusal.as_deref().unwrap_or("read-only field");
    Some(SharedString::from(format!("Read-only · {refusal}")))
}

fn build_rows(
    editor: &FrontmatterEditor,
    references: &ReferenceCatalog,
    window: &mut Window,
    cx: &mut Context<FrontmatterForm>,
) -> Vec<RowEditor> {
    let mut rows = Vec::new();
    for control in editor.controls() {
        let is_object = control.kind == ControlKind::Object;
        let needs_text = control.editable
            && matches!(
                control.kind,
                ControlKind::Text { .. }
                    | ControlKind::Number
                    | ControlKind::Integer
                    | ControlKind::Image
            );
        let is_description = control.role == Some(FieldRole::Description);
        let mut subscriptions = Vec::new();
        let mut textarea = None;
        let mut input = None;
        let mut date = None;
        let mut add_input = None;
        if needs_text {
            let initial = editor
                .value(&control.path)
                .map(scalar_display)
                .unwrap_or_default();
            if is_description {
                let state = cx.new(|cx| TextareaState::new(window, cx).default_value(initial));
                let path = control.path.clone();
                subscriptions.push(cx.subscribe(
                    &state,
                    move |this, area, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            let raw = area.read(cx).value().to_string();
                            this.apply_value(&path, &raw, cx);
                        }
                    },
                ));
                textarea = Some(state);
            } else {
                let state = cx.new(|cx| InputState::new(window, cx).default_value(initial));
                let path = control.path.clone();
                subscriptions.push(cx.subscribe(
                    &state,
                    move |this, field, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            let raw = field.read(cx).value().to_string();
                            this.apply_value(&path, &raw, cx);
                        }
                    },
                ));
                input = Some(state);
            }
        } else if matches!(control.kind, ControlKind::RawYaml { .. }) && control.editable {
            let initial = editor.raw(&control.path).unwrap_or_default().to_owned();
            let state = cx.new(|cx| TextareaState::new(window, cx).default_value(initial));
            let path = control.path.clone();
            subscriptions.push(
                cx.subscribe(&state, move |this, area, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let raw = area.read(cx).value().to_string();
                        this.apply_value(&path, &raw, cx);
                    }
                }),
            );
            textarea = Some(state);
        } else if control.kind == ControlKind::Date {
            let initial = editor.value(&control.path).and_then(|value| match value {
                FrontmatterValue::String(text) => NaiveDate::parse_from_str(
                    text.split(['T', ' ']).next().unwrap_or(&text),
                    "%Y-%m-%d",
                )
                .ok(),
                _ => None,
            });
            let state = cx.new(|cx| {
                let mut state = DatePickerState::new(window, cx);
                if let Some(initial) = initial {
                    state.set_date(Date::Single(Some(initial)), window, cx);
                }
                state
            });
            let path = control.path.clone();
            subscriptions.push(cx.subscribe(
                &state,
                move |this, _picker, event: &DatePickerEvent, cx| {
                    if let DatePickerEvent::Change(DateTime::Single(Some(value))) = event {
                        this.apply_value(&path, &value.format("%Y-%m-%d").to_string(), cx);
                    }
                },
            ));
            date = Some(state);
        }
        if matches!(control.kind, ControlKind::Tags) && control.editable {
            let state = cx.new(|cx| InputState::new(window, cx));
            let path = control.path.clone();
            subscriptions.push(
                cx.subscribe(&state, move |this, field, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        let raw = field.read(cx).value().trim().to_owned();
                        if !raw.is_empty() {
                            this.apply_add_item(&path, &raw, cx);
                        }
                    }
                }),
            );
            add_input = Some(state);
        }
        let enum_values = match &control.kind {
            ControlKind::Enum { values } => values
                .iter()
                .map(|value| {
                    let display = enum_display(value);
                    (
                        SharedString::from(display.clone()),
                        SharedString::from(display),
                    )
                })
                .collect(),
            _ => Vec::new(),
        };
        let reference_options = match &control.kind {
            ControlKind::Reference { collections, .. } => {
                reference_options(references, collections)
            }
            _ => Vec::new(),
        };
        rows.push(RowEditor {
            label: SharedString::from(control.name.clone()),
            aria_label: SharedString::from(format!("frontmatter field {}", control.path)),
            path: SharedString::from(control.path.clone()),
            kind_label: kind_display(control_label(&control.kind)),
            hierarchy: control.path.matches('.').count(),
            is_object,
            read_only_label: read_only_label(control, is_object),
            description: control.description.clone().map(SharedString::from),
            hint: control.hint().map(SharedString::from),
            enum_values,
            reference_options,
            items: sequence_display(editor, &control.path),
            raw: editor.raw(&control.path).map(SharedString::from),
            is_title: control.role == Some(FieldRole::Title),
            input,
            textarea,
            date,
            add_input,
            control: control.clone(),
            _subscriptions: subscriptions,
        });
    }
    rows
}

fn reference_options(
    catalog: &ReferenceCatalog,
    collections: &[String],
) -> Vec<(SharedString, SharedString)> {
    let mut options: Vec<(SharedString, SharedString)> = Vec::new();
    for collection in collections {
        for option in catalog.get(collection).into_iter().flatten() {
            let label = SharedString::from(option.label.clone());
            let value = SharedString::from(option.value.clone());
            if !options.iter().any(|(_, existing)| existing == &value) {
                options.push((label, value));
            }
        }
    }
    options
}

fn scalar_display(value: FrontmatterValue) -> String {
    match value {
        FrontmatterValue::Null => String::new(),
        FrontmatterValue::Boolean(flag) => flag.to_string(),
        FrontmatterValue::Integer(integer) => integer.to_string(),
        FrontmatterValue::Float(number) => number.to_string(),
        FrontmatterValue::String(text) => text,
    }
}

impl Render for FrontmatterForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = self.load_error.clone() {
            return div()
                .id("frontmatter-form")
                .v_flex()
                .gap_2()
                .p_4()
                .child(
                    div()
                        .h_flex()
                        .items_center()
                        .gap_2()
                        .text_sm()
                        .child(Icon::new(IconName::TriangleAlert))
                        .child("Frontmatter cannot be edited safely"),
                )
                .child(div().text_xs().text_color(cx.theme().danger).child(error))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Edit the raw source instead; the document is never rewritten."),
                );
        }
        let Some(editor) = self.editor.as_ref() else {
            return div()
                .id("frontmatter-form")
                .v_flex()
                .items_center()
                .justify_center()
                .gap_2()
                .p_6()
                .text_center()
                .child(
                    Icon::new(IconName::SlidersHorizontal)
                        .size_6()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(div().text_sm().child("No frontmatter form"))
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Choose an entry to see its details."),
                );
        };
        let notice = self.notice.clone();
        let images = RowImageContext {
            root: self.root.clone(),
            source: self.source.clone(),
            assets: self.assets.clone(),
            path_mode: self.path_mode,
        };
        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.rows.len());
        for (index, row) in self.rows.iter().enumerate() {
            rows.push(render_row(index, row, editor, &images, cx, window).into_any_element());
        }
        div()
            .id("frontmatter-form")
            .v_flex()
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().background)
            .child(
                div()
                    .h_flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .pt_4()
                    .pb_3()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.55))
                    .child(
                        div()
                            .v_flex()
                            .gap_0p5()
                            .child(
                                div()
                                    .font_family(crate::theme::display_font(cx))
                                    .text_base()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child("Details"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(SharedString::from(format!(
                                        "{} fields",
                                        editor.controls().len()
                                    ))),
                            ),
                    ),
            )
            .when_some(notice, |form, notice| {
                form.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(notice),
                )
            })
            .child(div().v_flex().gap_5().p_4().children(rows))
    }
}

fn render_row(
    index: usize,
    row: &RowEditor,
    editor: &FrontmatterEditor,
    images: &RowImageContext,
    cx: &mut Context<FrontmatterForm>,
    window: &Window,
) -> impl IntoElement {
    let _ = window;
    let control = &row.control;
    let mut diagnostics = String::new();
    for diagnostic in editor.diagnostics_for(&row.path) {
        use std::fmt::Write as _;
        if !diagnostics.is_empty() {
            diagnostics.push_str(" · ");
        }
        let _ = write!(
            diagnostics,
            "{:?}: {}",
            diagnostic.severity, diagnostic.message
        );
    }
    let header = div()
        .h_flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .text_xs()
                .when(control.required, |label| {
                    label.child(div().text_color(cx.theme().warning).child("*"))
                })
                .font_family(crate::theme::display_font(cx))
                .text_sm()
                .child(row.label.clone()),
        )
        .child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(row.kind_label),
                )
                .when(row.control.removable, |group| {
                    let path = row.path.clone();
                    group.child(
                        Button::new(("frontmatter-remove", index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip("Remove this optional field")
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.apply_remove_field(&path, cx);
                            })),
                    )
                }),
        );

    let mut row_view = div()
        .id(("frontmatter-field", index))
        .v_flex()
        .gap_1p5()
        .when(row.hierarchy > 0, |view| {
            view.pl(px((row.hierarchy * 12) as f32))
        })
        .child(header);

    if let Some(description) = row.description.clone() {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(description),
        );
    }

    if row.is_object {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(row.label.clone()),
        );
    } else if matches!(control.kind, ControlKind::Image) {
        // The image control owns its path input, so it must render before the
        // generic text input built for it.
        row_view = row_view.child(image_control(index, row, images, cx));
    } else if matches!(
        control.kind,
        ControlKind::Reference {
            multiple: false,
            ..
        }
    ) {
        row_view = row_view.child(reference_dropdown(index, row, editor, cx));
    } else if let Some(area) = row.textarea.as_ref() {
        row_view = row_view.child(
            Textarea::new(area)
                .small()
                .h(px(96.))
                .aria_label(row.aria_label.clone()),
        );
    } else if let Some(input) = row.input.as_ref() {
        let field = Input::new(input).small().aria_label(row.aria_label.clone());
        let field = if row.is_title { field.large() } else { field };
        row_view = row_view.child(field);
    } else if let Some(picker) = row.date.as_ref() {
        row_view = row_view.child(DatePicker::new(picker).small());
    } else if matches!(control.kind, ControlKind::Boolean) {
        let boolean_value = matches!(
            editor.value(&row.path),
            Some(FrontmatterValue::Boolean(true))
        );
        let toggle_path = row.path.clone();
        row_view = row_view.child(
            Checkbox::new(("frontmatter-bool", index))
                .checked(boolean_value)
                .label(if boolean_value { "true" } else { "false" })
                .on_click(cx.listener(move |this, checked: &bool, _window, cx| {
                    let raw = if *checked { "true" } else { "false" };
                    this.apply_value(&toggle_path, raw, cx);
                })),
        );
    } else if !row.enum_values.is_empty() {
        row_view = row_view.child(enum_dropdown(index, row, editor, cx));
    } else if matches!(control.kind, ControlKind::Reference { multiple: true, .. }) {
        row_view = row_view.child(reference_list(index, row, cx));
    } else if matches!(control.kind, ControlKind::Tags) {
        row_view = row_view.child(tag_control(index, row, cx));
    } else if let Some(label) = row.read_only_label.clone() {
        row_view = row_view.child(
            div()
                .h_flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(Icon::new(IconName::Lock).xsmall())
                .child(label),
        );
    }

    if row.is_title {
        let actions = div().h_flex().items_center().gap_2().pt_1().child(
            Button::new(("frontmatter-copy-slug", index))
                .ghost()
                .xsmall()
                .label("Copy slug")
                .tooltip("Slugify the title and copy it to the clipboard")
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.copy_slug(cx);
                })),
        );
        row_view = row_view.child(actions);
    }

    if let Some(hint) = row.hint.clone() {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(hint),
        );
    }
    if !diagnostics.is_empty() {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(SharedString::from(diagnostics)),
        );
    }
    row_view
}

/// The label shown on an enum or reference dropdown.
///
/// The current document value is shown even when the schema no longer lists it,
/// so an out-of-schema value is visible instead of looking unset.
fn selected_option_label(options: &[(SharedString, SharedString)], current: &str) -> SharedString {
    options
        .iter()
        .find(|(_, value)| value.as_ref() == current)
        .map(|(label, _)| label.clone())
        .unwrap_or_else(|| {
            if current.is_empty() {
                SharedString::from("Select…")
            } else {
                SharedString::from(current.to_owned())
            }
        })
}

fn enum_dropdown(
    index: usize,
    row: &RowEditor,
    editor: &FrontmatterEditor,
    cx: &mut Context<FrontmatterForm>,
) -> impl IntoElement {
    let current = editor
        .value(&row.path)
        .map(scalar_display)
        .unwrap_or_default();
    let label = selected_option_label(&row.enum_values, &current);
    let values = row.enum_values.clone();
    let path = row.path.to_string();
    let this = cx.weak_entity();
    Button::new(("frontmatter-enum", index))
        .xsmall()
        .outline()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _window, _cx| {
            let mut menu = menu;
            for (display, raw) in &values {
                let path = path.clone();
                let raw = raw.to_string();
                let this = this.clone();
                menu = menu.item(PopupMenuItem::new(display.clone()).on_click(
                    move |_, _window, cx: &mut App| {
                        let raw = raw.clone();
                        let path = path.clone();
                        let _ = this.update(cx, |form, cx| form.apply_value(&path, &raw, cx));
                    },
                ));
            }
            menu
        })
}

fn reference_dropdown(
    index: usize,
    row: &RowEditor,
    editor: &FrontmatterEditor,
    cx: &mut Context<FrontmatterForm>,
) -> impl IntoElement {
    let current = editor
        .value(&row.path)
        .map(scalar_display)
        .unwrap_or_default();
    let label = selected_option_label(&row.reference_options, &current);
    let options = row.reference_options.clone();
    let path = row.path.to_string();
    let this = cx.weak_entity();
    Button::new(("frontmatter-reference", index))
        .xsmall()
        .outline()
        .label(label)
        .dropdown_caret(true)
        .dropdown_menu(move |menu, _window, _cx| {
            let mut menu = menu;
            if options.is_empty() {
                return menu.item(PopupMenuItem::label("No entries"));
            }
            for (display, value) in &options {
                let path = path.clone();
                let value = value.to_string();
                let this = this.clone();
                menu = menu.item(PopupMenuItem::new(display.clone()).on_click(
                    move |_, _window, cx: &mut App| {
                        let value = value.clone();
                        let path = path.clone();
                        let _ = this.update(cx, |form, cx| form.apply_value(&path, &value, cx));
                    },
                ));
            }
            menu
        })
}

fn reference_list(
    index: usize,
    row: &RowEditor,
    cx: &mut Context<FrontmatterForm>,
) -> impl IntoElement {
    let path = row.path.to_string();
    let this = cx.weak_entity();
    let options = row.reference_options.clone();
    let mut view = div().v_flex().gap_1();
    if !row.items.is_empty() {
        let chips = row
            .items
            .iter()
            .map(|(item_index, item)| {
                let path = path.clone();
                let item_index = *item_index;
                div()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .text_xs()
                    .child(item.clone())
                    .child(
                        Button::new(("frontmatter-chip-remove", index * 64 + item_index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip("Remove item")
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.apply_remove_item(&path, item_index, cx);
                            })),
                    )
            })
            .collect::<Vec<_>>();
        view = view.child(div().h_flex().flex_wrap().gap_1().children(chips));
    }
    view.child(
        Button::new(("frontmatter-reference-add", index))
            .xsmall()
            .outline()
            .label("Add reference")
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _window, _cx| {
                if options.is_empty() {
                    return menu.item(PopupMenuItem::label("No entries"));
                }
                let mut menu = menu;
                for (display, value) in &options {
                    let path = path.clone();
                    let value = value.to_string();
                    let this = this.clone();
                    menu = menu.item(PopupMenuItem::new(display.clone()).on_click(
                        move |_, _window, cx: &mut App| {
                            let value = value.clone();
                            let path = path.clone();
                            let _ =
                                this.update(cx, |form, cx| form.apply_add_item(&path, &value, cx));
                        },
                    ));
                }
                menu
            }),
    )
}

fn tag_control(
    index: usize,
    row: &RowEditor,
    cx: &mut Context<FrontmatterForm>,
) -> impl IntoElement {
    let path = row.path.to_string();
    let mut view = div().v_flex().gap_1();
    if !row.items.is_empty() {
        let chips = row
            .items
            .iter()
            .map(|(item_index, item)| {
                let path = path.clone();
                let item_index = *item_index;
                div()
                    .h_flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .bg(cx.theme().muted)
                    .text_xs()
                    .child(item.clone())
                    .child(
                        Button::new(("frontmatter-chip-remove", index * 64 + item_index))
                            .ghost()
                            .xsmall()
                            .icon(IconName::X)
                            .tooltip("Remove tag")
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                this.apply_remove_item(&path, item_index, cx);
                            })),
                    )
            })
            .collect::<Vec<_>>();
        view = view.child(div().h_flex().flex_wrap().gap_1().children(chips));
    }
    if let Some(input) = row.add_input.as_ref() {
        view = view.child(
            Input::new(input)
                .small()
                .aria_label(SharedString::from(format!("add tag to {}", row.path))),
        );
    }
    view
}

/// Resolve one image picker result into the frontmatter value.
///
/// A file already inside the project is referenced in place; an external file
/// is copied through the asset service first. Both outcomes use the configured
/// relative or project-root path mode, and the manual path input is never
/// validated here.
pub(crate) fn resolve_image_pick(
    root: Option<PathBuf>,
    source: Option<String>,
    assets: Option<AssetLocation>,
    path_mode: ImagePathMode,
    selected: PathBuf,
) -> Result<String, String> {
    let root = root.ok_or_else(|| "The project root is unavailable.".to_owned())?;
    let source = source.ok_or_else(|| "This document has no project source.".to_owned())?;
    let canonical_root = std::fs::canonicalize(&root).map_err(|error| error.to_string())?;
    let canonical = std::fs::canonicalize(&selected).map_err(|error| error.to_string())?;
    if canonical.starts_with(&canonical_root) {
        let relative = project_relative_path(&canonical_root, &canonical)
            .map_err(|error| error.to_string())?;
        return Ok(asset_path(path_mode, &source, &relative));
    }
    let location =
        assets.ok_or_else(|| "External images need a resolved assets directory.".to_owned())?;
    let date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let asset = copy_asset(
        &location,
        &AssetCopyRequest {
            source: selected,
            date,
            document: source,
            path_mode,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(asset.path().to_owned())
}

fn image_control(
    index: usize,
    row: &RowEditor,
    images: &RowImageContext,
    cx: &mut Context<FrontmatterForm>,
) -> impl IntoElement {
    let root = images.root.clone();
    let source = images.source.clone();
    let assets = images.assets.clone();
    let path_mode = images.path_mode;
    let path = row.path.to_string();
    let value = row
        .input
        .as_ref()
        .map(|input| input.read(cx).value().to_string())
        .unwrap_or_default();
    // Lexical containment only: the pinned image loader reads the file off the
    // render path, and a refused path simply has no thumbnail.
    let thumbnail = match (root.as_ref(), source.as_deref()) {
        (Some(root), Some(source)) if !value.is_empty() => {
            let document = root.join(source);
            match image_preview::resolve_preview(root, &document, &value) {
                Some(PreviewSource::Local(path)) => Some(path),
                _ => None,
            }
        }
        _ => None,
    };
    let mut preview = div()
        .h_flex()
        .items_center()
        .justify_center()
        .size(px(56.))
        .rounded_md()
        .overflow_hidden()
        .border_1()
        .border_color(cx.theme().window_border)
        .bg(cx.theme().muted);
    preview = match thumbnail {
        Some(path) => preview.child(img(path).size(px(48.)).object_fit(ObjectFit::Contain)),
        None => preview.child(
            Icon::new(IconName::Image)
                .small()
                .text_color(cx.theme().muted_foreground),
        ),
    };
    if let Some(input) = row.input.as_ref() {
        preview = div().h_flex().items_center().gap_2().child(preview).child(
            div().flex_1().child(
                Input::new(input)
                    .small()
                    .aria_label(SharedString::from(format!("image path for {}", row.path))),
            ),
        );
    }
    let mut actions = div().h_flex().gap_2();
    let pick_path = path.clone();
    let pick_root = root.clone();
    let pick_source = source.clone();
    let pick_assets = assets.clone();
    actions = actions.child(
        Button::new(("frontmatter-image-pick", index))
            .xsmall()
            .outline()
            .label("Choose…")
            .on_click(cx.listener(move |this, _, window, cx| {
                let _ = this;
                let path = pick_path.clone();
                let root = pick_root.clone();
                let source = pick_source.clone();
                let assets = pick_assets.clone();
                let expected_source = source.clone();
                let paths = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Choose an image".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    let Ok(Ok(Some(paths))) = paths.await else {
                        return;
                    };
                    let Some(selected) = paths.into_iter().next() else {
                        return;
                    };
                    let work = runtime::blocking(cx, move || {
                        resolve_image_pick(
                            root.clone(),
                            source.clone(),
                            assets.clone(),
                            path_mode,
                            selected,
                        )
                    });
                    let result = work.await.and_then(|result| result);
                    let _ = this.update(cx, |form, cx| {
                        if form.source != expected_source {
                            return;
                        }
                        match result {
                            Ok(value) => {
                                form.set_image_path(&path, value, cx);
                                form.notice = Some(SharedString::from("Image path updated"));
                            }
                            Err(error) => {
                                form.notice = Some(SharedString::from(format!(
                                    "Could not copy the image: {error}"
                                )));
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            })),
    );
    if row.control.present {
        let path = path.clone();
        actions = actions.child(
            Button::new(("frontmatter-image-clear", index))
                .xsmall()
                .ghost()
                .label("Clear")
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.apply_remove_field(&path, cx);
                })),
        );
    }
    div().v_flex().gap_2().child(preview).child(actions)
}

fn enum_display(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Null => "null".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
