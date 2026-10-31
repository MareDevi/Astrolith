//! Schema-driven frontmatter forms for the Inspector panel.
//!
//! [`fields`] maps normalized [`FieldKind`](astrolith_astro::schema::FieldKind)
//! values to native controls and applies edits through the lossless
//! [`ParsedDocument`] patch contract. [`FrontmatterEditor`] is the GUI-free model
//! that holds one parsed document, its controls and its validation feedback; the
//! [`FrontmatterForm`] GPUI entity renders that model with GPUI Kit controls.
//!
//! The editor never touches the filesystem: parsing, editing and serialization
//! all operate on in-memory text. Persisting the result is the shell's save
//! coordinator's job, so only supported scalar edits ever reach disk.

pub mod fields;

use std::{collections::BTreeSet, sync::Arc};

use astrolith_astro::schema::{
    CollectionSchema, FieldKind, ValidationDiagnostic, validate_frontmatter,
};
use astrolith_document::{DocumentError, FrontmatterValue, ParsedDocument, parse_document};
use gpui_kit::component::{
    ActiveTheme, Sizable, StyledExt,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    input::{Editor, EditorState, InputEvent},
};
use gpui_kit::{
    AppContext, Context, Entity, EventEmitter, IntoElement, Render, SharedString, Subscription,
    Window, div, prelude::*, px,
};
use serde_json::{Map, Value};

pub use fields::{ControlKind, FieldControl, FieldEditError, control_label};

/// The GUI-free form model for one document's frontmatter.
///
/// Holds the parsed document, the resolved control list and the latest local
/// validation diagnostics. Editing a value records a lossless patch, re-parses
/// the re-serialized text (so subsequent reads see the edited value) and
/// recomputes diagnostics.
#[derive(Clone, Debug)]
pub struct FrontmatterEditor {
    schema: Arc<CollectionSchema>,
    controls: Vec<FieldControl>,
    document: ParsedDocument,
    text: String,
    diagnostics: Vec<ValidationDiagnostic>,
    manageable: BTreeSet<String>,
}

impl FrontmatterEditor {
    /// Build a form model for `text` against `schema`.
    ///
    /// # Errors
    ///
    /// Returns a [`DocumentError`] when the document frontmatter is malformed or
    /// uses a construct outside the lossless subset (for example block sequences).
    pub fn new(schema: Arc<CollectionSchema>, text: String) -> Result<Self, DocumentError> {
        let document = parse_document(&text)?;
        let controls = fields::build_controls(&schema.fields);
        let manageable = manageable_paths(&controls);
        let mut editor = Self {
            schema,
            controls,
            document,
            text,
            diagnostics: Vec::new(),
            manageable,
        };
        editor.revalidate();
        Ok(editor)
    }

    /// Return the schema backing this form.
    pub fn schema(&self) -> &CollectionSchema {
        &self.schema
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

    /// Return the current typed value at `path`, if readable.
    pub fn value(&self, path: &str) -> Option<FrontmatterValue> {
        self.document.field(path).cloned()
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
        let mut document = self.document.clone();
        fields::apply_edit(&mut document, path, &control.kind, raw)
            .map_err(|error| error.to_string())?;
        let text = document.serialize().map_err(|error| error.to_string())?;
        let parsed = parse_document(&text).map_err(|error| error.to_string())?;
        self.document = parsed;
        self.text = text.clone();
        self.revalidate();
        Ok(text)
    }

    fn revalidate(&mut self) {
        let values = self.read_values();
        let diagnostics = validate_frontmatter(&self.schema, &values);
        self.diagnostics = diagnostics
            .into_iter()
            .filter(|diagnostic| {
                diagnostic.path == "<root>" || self.manageable.contains(&diagnostic.path)
            })
            .collect();
    }

    /// Build a JSON object of the readable scalar leaves.
    fn read_values(&self) -> Value {
        let mut root = Map::new();
        for control in &self.controls {
            if !control.editable {
                continue;
            }
            let Some(value) = self.document.field(&control.path) else {
                continue;
            };
            insert_path(&mut root, &control.path, fields::frontmatter_to_json(value));
        }
        Value::Object(root)
    }
}

/// Paths for which local validation is authoritative: editable scalar leaves and
/// object containers we can introspect. Read-only containers (arrays, records,
/// unsupported) are excluded so an unreadable value never yields a false
/// "required field is missing".
fn manageable_paths(controls: &[FieldControl]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for control in controls {
        let introspectable = match &control.kind {
            ControlKind::Object => true,
            ControlKind::Array | ControlKind::Record | ControlKind::Unsupported { .. } => false,
            _ => control.editable,
        };
        if introspectable {
            paths.insert(control.path.clone());
        }
    }
    paths
}

fn insert_path(root: &mut Map<String, Value>, path: &str, value: Value) {
    let mut segments = path.split('.').peekable();
    let mut current = root;
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            current.insert(segment.to_owned(), value);
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

/// Short label for a schema leaf kind (used for tests and the mapping report).
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

/// Emitted when the form produces a new full document text from an edit.
#[derive(Clone, Debug)]
pub struct FrontmatterChanged {
    /// The complete re-serialized document text to persist through the session.
    pub text: String,
}

/// One editable row: the control plus, for text-like controls, a retained editor
/// whose changes drive the model.
struct RowEditor {
    control: FieldControl,
    input: Option<Entity<EditorState>>,
    _subscription: Option<Subscription>,
}

/// A GPUI entity that renders a [`FrontmatterEditor`] with native controls.
///
/// Text-like fields use GPUI Kit [`Editor`]s, booleans a [`Checkbox`], enums one
/// [`Button`] per allowed value, object containers a heading plus their children,
/// and read-only fields an explicit refusal. Edits update the model and emit
/// [`FrontmatterChanged`] with the new full document text; the shell applies that
/// text through the save coordinator.
pub struct FrontmatterForm {
    editor: Option<FrontmatterEditor>,
    rows: Vec<RowEditor>,
}

impl EventEmitter<FrontmatterChanged> for FrontmatterForm {}

impl FrontmatterForm {
    /// Create an empty form showing a placeholder.
    pub fn new(_cx: &mut Context<Self>) -> Self {
        Self {
            editor: None,
            rows: Vec::new(),
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

    /// Load a schema and document text into the form, rebuilding its rows.
    pub fn set_document(
        &mut self,
        schema: Arc<CollectionSchema>,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match FrontmatterEditor::new(schema, text) {
            Ok(editor) => {
                self.rows = build_rows(&editor, window, cx);
                self.editor = Some(editor);
            }
            Err(_) => {
                // Malformed frontmatter stays in the source editor; the form
                // shows nothing rather than guessing at a lossy parse.
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
        cx.notify();
    }

    /// Apply a raw edit at `path`, update the model and emit the new text.
    pub fn apply_value(&mut self, path: &str, raw: &str, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if let Ok(text) = editor.set_value(path, raw) {
            cx.emit(FrontmatterChanged { text });
            cx.notify();
        }
    }

    /// Return the row control paths currently rendered (test/diagnostic helper).
    pub fn control_paths(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.control.path.clone())
            .collect()
    }
}

fn build_rows(
    editor: &FrontmatterEditor,
    window: &mut Window,
    cx: &mut Context<FrontmatterForm>,
) -> Vec<RowEditor> {
    let mut rows = Vec::new();
    for control in editor.controls() {
        let needs_text_input = control.editable
            && !matches!(
                control.kind,
                ControlKind::Boolean | ControlKind::Enum { .. }
            );
        let (input, subscription) = if needs_text_input {
            let initial = editor
                .value(&control.path)
                .map(scalar_display)
                .unwrap_or_default();
            let state = cx.new(|cx| {
                EditorState::new(window, cx)
                    .language("markdown")
                    .default_value(initial)
            });
            let path = control.path.clone();
            let subscription =
                cx.subscribe(&state, move |this, editor_state, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        let raw = editor_state.read(cx).text().to_string();
                        this.apply_value(&path, &raw, cx);
                    }
                });
            (Some(state), Some(subscription))
        } else {
            (None, None)
        };
        rows.push(RowEditor {
            control: control.clone(),
            input,
            _subscription: subscription,
        });
    }
    rows
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor) = self.editor.as_ref() else {
            return div()
                .id("frontmatter-form")
                .p_3()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("No frontmatter schema is available for this document yet.");
        };
        let rows = self
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| render_row(index, row, editor, cx))
            .collect::<Vec<_>>();
        div()
            .id("frontmatter-form")
            .v_flex()
            .gap_2()
            .p_3()
            .size_full()
            .overflow_y_scroll()
            .child(div().text_sm().child(SharedString::from(format!(
                "Frontmatter · {} field(s)",
                editor.controls().len()
            ))))
            .children(rows)
    }
}

fn render_row(
    index: usize,
    row: &RowEditor,
    editor: &FrontmatterEditor,
    cx: &Context<FrontmatterForm>,
) -> impl IntoElement {
    let control = &row.control;
    let nested = control.path.matches('.').count();
    let label = if control.required {
        format!("{} *", control.name)
    } else {
        control.name.clone()
    };
    let diagnostics: Vec<String> = editor
        .diagnostics_for(&control.path)
        .map(|diagnostic| format!("{:?}: {}", diagnostic.severity, diagnostic.message))
        .collect();
    let is_object = control.kind == ControlKind::Object;
    let boolean_value = matches!(
        editor.value(&control.path),
        Some(FrontmatterValue::Boolean(true))
    );
    let enum_values = match &control.kind {
        ControlKind::Enum { values } => values.clone(),
        _ => Vec::new(),
    };
    let path = control.path.clone();
    let refusal = control
        .refusal
        .clone()
        .unwrap_or_else(|| "read-only field".to_owned());

    let mut row_view = div()
        .id(("frontmatter-field", index))
        .v_flex()
        .gap_1()
        .pl(px((nested * 12) as f32))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(label)),
        );

    if is_object {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(SharedString::from(format!("{}.", control.name))),
        );
    } else if let Some(input) = row.input.as_ref() {
        row_view = row_view.child(
            Editor::new(input)
                .bordered(true)
                .aria_label(format!("frontmatter field {}", control.path)),
        );
    } else if matches!(control.kind, ControlKind::Boolean) {
        let toggle_path = path.clone();
        row_view = row_view.child(
            Checkbox::new(format!("frontmatter-bool-{index}"))
                .checked(boolean_value)
                .label(if boolean_value { "true" } else { "false" })
                .on_click(cx.listener(move |this, checked: &bool, _window, cx| {
                    let raw = if *checked { "true" } else { "false" };
                    this.apply_value(&toggle_path, raw, cx);
                })),
        );
    } else if !enum_values.is_empty() {
        let buttons = enum_values
            .iter()
            .enumerate()
            .map(|(value_index, value)| {
                let raw = enum_display(value);
                let selected = editor
                    .value(&path)
                    .map(|current| scalar_display(current) == raw)
                    .unwrap_or(false);
                let enum_path = path.clone();
                Button::new(format!("frontmatter-enum-{index}-{value_index}"))
                    .small()
                    .when(!selected, |button| button.ghost())
                    .label(raw.clone())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.apply_value(&enum_path, &raw, cx);
                    }))
            })
            .collect::<Vec<_>>();
        row_view = row_view.child(div().h_flex().flex_wrap().gap_1().children(buttons));
    } else {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().warning)
                .child(SharedString::from(format!("Read-only · {refusal}"))),
        );
    }

    if !diagnostics.is_empty() {
        row_view = row_view.child(
            div()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(SharedString::from(diagnostics.join(" · "))),
        );
    }
    row_view
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
