//! Typed component-builder form state.
//!
//! The form mirrors the component's declared props: required props are always
//! part of the tag, optional props stay out until they are given a value or
//! deliberately included, and declared defaults are shown beside the label
//! rather than inserted silently. Values are rendered by the P9 snippet
//! generator, so this module owns no MDX syntax of its own.

use std::collections::BTreeMap;

use astrolith_astro::components::{
    ClientDirectiveValue, Component, PlannedValue, Prop, PropDefault, PropKind, Snippet,
    SnippetError, SnippetValue, TagRequest, generate_tag, provisional_identifiers,
};

/// What a field's control needs to render.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PropControl {
    Text,
    Number,
    /// Boolean props with no meaningful "empty" value.
    Boolean,
    Date,
    /// A string union or string enum.
    Enum {
        options: Vec<String>,
    },
    /// A string or number array, entered as a comma-separated list.
    List,
    /// An `ImageMetadata` value: a project path or asset.
    Image,
    /// A `CollectionEntry<'name'>` value from one collection.
    Entry {
        collection: String,
    },
}

impl PropControl {
    pub(crate) fn of(prop: &Prop) -> Self {
        match &prop.kind {
            PropKind::String => Self::Text,
            PropKind::Number => Self::Number,
            PropKind::Boolean => Self::Boolean,
            PropKind::Date => Self::Date,
            PropKind::StringArray | PropKind::NumberArray => Self::List,
            PropKind::Image => Self::Image,
            PropKind::CollectionEntry { collection } => Self::Entry {
                collection: collection.clone(),
            },
            PropKind::Enum { values } => Self::Enum {
                options: values.clone(),
            },
        }
    }

    /// Whether the control is a text field the form keeps an `InputState` for.
    pub(crate) fn is_textual(&self) -> bool {
        matches!(
            self,
            Self::Text | Self::Number | Self::Date | Self::List | Self::Image | Self::Entry { .. }
        )
    }
}

/// One editable prop.
#[derive(Clone, Debug)]
pub(crate) struct PropField {
    pub prop: Prop,
    pub control: PropControl,
    text: String,
    flag: bool,
    included: bool,
    /// Options for a collection-entry field, supplied from the index.
    pub entries: Vec<(String, String)>,
}

impl PropField {
    /// Whether the field contributes an attribute to the tag.
    pub(crate) fn included(&self) -> bool {
        self.included
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn flag(&self) -> bool {
        self.flag
    }

    /// The declared default, shown beside the label.
    pub(crate) fn default_label(&self) -> Option<String> {
        self.prop.default.as_ref().map(default_text)
    }

    /// Whether an empty required field will receive the caret.
    pub(crate) fn is_blank_required(&self) -> bool {
        self.prop.required && self.snippet_value().is_none_or(|value| empty_value(&value))
    }

    /// One rendered value, or `None` when the prop is omitted.
    fn snippet_value(&self) -> Option<SnippetValue> {
        if !self.included && !self.prop.required {
            return None;
        }
        match &self.control {
            PropControl::Boolean => Some(SnippetValue::Boolean(self.flag)),
            PropControl::Enum { .. } => {
                if self.text.is_empty() {
                    self.prop
                        .required
                        .then(|| SnippetValue::String(String::new()))
                } else {
                    Some(SnippetValue::String(self.text.clone()))
                }
            }
            PropControl::Number => {
                let text = self.text.trim();
                if text.is_empty() {
                    None
                } else {
                    Some(SnippetValue::Number(text.to_owned()))
                }
            }
            PropControl::List => {
                let values: Vec<String> = self
                    .text
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect();
                if values.is_empty() {
                    None
                } else {
                    Some(SnippetValue::List(values))
                }
            }
            PropControl::Text
            | PropControl::Date
            | PropControl::Image
            | PropControl::Entry { .. } => {
                if self.text.is_empty() && self.prop.required {
                    Some(SnippetValue::String(String::new()))
                } else if self.text.is_empty() {
                    None
                } else {
                    Some(SnippetValue::String(self.text.clone()))
                }
            }
        }
    }
}

fn empty_value(value: &SnippetValue) -> bool {
    matches!(value, SnippetValue::String(text) if text.is_empty())
}

fn default_text(default: &PropDefault) -> String {
    match default {
        PropDefault::String(value) => value.clone(),
        PropDefault::Number(value) => value.clone(),
        PropDefault::Boolean(value) => {
            if *value {
                "true".into()
            } else {
                "false".into()
            }
        }
        PropDefault::List(values) => values.join(", "),
    }
}

/// The whole form for one component.
#[derive(Clone, Debug)]
pub(crate) struct FormState {
    component: Component,
    fields: Vec<PropField>,
    content: String,
    directive: Option<String>,
    directive_value: String,
}

impl FormState {
    /// Build the form with declared defaults shown and optional props excluded.
    pub(crate) fn new(component: &Component, content: Option<String>) -> Self {
        let fields = component
            .props
            .iter()
            .map(|prop| {
                let text = prop.default.as_ref().map(default_text).unwrap_or_default();
                let flag = match prop.default {
                    // A required boolean reads as present by default; changing
                    // it to false writes `={false}`.
                    Some(PropDefault::Boolean(value)) => value,
                    _ => prop.required,
                };
                PropField {
                    control: PropControl::of(prop),
                    prop: prop.clone(),
                    text,
                    flag,
                    included: prop.required,
                    entries: Vec::new(),
                }
            })
            .collect();
        Self {
            component: component.clone(),
            fields,
            content: content.unwrap_or_default(),
            directive: None,
            directive_value: String::new(),
        }
    }

    pub(crate) fn component(&self) -> &Component {
        &self.component
    }

    pub(crate) fn fields(&self) -> &[PropField] {
        &self.fields
    }

    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    pub(crate) fn directive(&self) -> Option<&str> {
        self.directive.as_deref()
    }

    pub(crate) fn directive_value(&self) -> &str {
        &self.directive_value
    }

    /// Attach the indexed options for one collection-entry prop.
    pub(crate) fn set_entry_options(&mut self, collection: &str, options: Vec<(String, String)>) {
        for field in &mut self.fields {
            if matches!(&field.control, PropControl::Entry { collection: name } if name == collection)
            {
                field.entries = options.clone();
            }
        }
    }

    pub(crate) fn set_text(&mut self, index: usize, text: impl Into<String>) {
        let text = text.into();
        let Some(field) = self.fields.get_mut(index) else {
            return;
        };
        let empty = text.trim().is_empty();
        field.text = text;
        // Typing a value includes an optional prop; clearing it omits it again.
        if !field.prop.required {
            field.included = !empty;
        }
    }

    pub(crate) fn set_flag(&mut self, index: usize, flag: bool) {
        let Some(field) = self.fields.get_mut(index) else {
            return;
        };
        field.flag = flag;
        if !field.prop.required {
            field.included = true;
        }
    }

    pub(crate) fn set_included(&mut self, index: usize, included: bool) {
        let Some(field) = self.fields.get_mut(index) else {
            return;
        };
        if !field.prop.required {
            field.included = included;
        }
    }

    pub(crate) fn set_content(&mut self, content: impl Into<String>) {
        self.content = content.into();
    }

    pub(crate) fn set_directive(&mut self, directive: Option<String>) {
        self.directive = directive;
        self.directive_value.clear();
    }

    pub(crate) fn set_directive_value(&mut self, value: impl Into<String>) {
        self.directive_value = value.into();
    }

    /// Whether every optional field is currently included.
    pub(crate) fn all_optional_included(&self) -> bool {
        self.fields
            .iter()
            .filter(|field| !field.prop.required)
            .all(|field| field.included)
    }

    /// Toggle every optional field at once; returns the new state.
    ///
    /// Optional fields without a usable value still stay out of the tag: the
    /// toggle includes controls, it never invents empty attributes.
    pub(crate) fn toggle_all_optional(&mut self) -> bool {
        let include = !self.all_optional_included();
        for field in &mut self.fields {
            if !field.prop.required {
                field.included = include;
            }
        }
        include
    }

    /// The index of the first required prop that will receive the caret.
    ///
    /// Only textual controls can hold the caret; a blank required boolean or
    /// enum keeps its own control and the caret goes to the next text field.
    pub(crate) fn first_blank_required(&self) -> Option<usize> {
        self.fields
            .iter()
            .position(|field| field.control.is_textual() && field.is_blank_required())
    }

    /// Values for the snippet generator, before planned identifiers resolve.
    pub(crate) fn values(&self) -> Vec<(String, SnippetValue)> {
        self.fields
            .iter()
            .filter_map(|field| {
                field
                    .snippet_value()
                    .map(|value| (field.prop.name.clone(), value))
            })
            .collect()
    }

    /// Image and entry values that change the import plan.
    pub(crate) fn planned_values(&self) -> Vec<PlannedFieldValue> {
        self.fields
            .iter()
            .filter_map(|field| {
                if !field.included && !field.prop.required {
                    return None;
                }
                if field.text.trim().is_empty() {
                    return None;
                }
                match &field.control {
                    PropControl::Image => Some(PlannedFieldValue::Image {
                        prop: field.prop.name.clone(),
                        path: field.text.clone(),
                    }),
                    PropControl::Entry { collection } => Some(PlannedFieldValue::Entry {
                        prop: field.prop.name.clone(),
                        collection: collection.clone(),
                        id: field.text.clone(),
                    }),
                    _ => None,
                }
            })
            .collect()
    }

    /// The selected hydration directive, if any.
    pub(crate) fn client_directive(&self) -> Option<ClientDirectiveValue> {
        let name = self.directive.clone()?;
        let requires_value = self
            .component
            .client_directives
            .iter()
            .find(|directive| directive.name == name)
            .is_some_and(|directive| directive.requires_value);
        Some(ClientDirectiveValue {
            name,
            value: requires_value.then(|| self.directive_value.clone()),
        })
    }

    /// Generate the tag the current form values produce.
    ///
    /// With imports on, image and entry values render as the identifier shape
    /// the plan will resolve, so the preview and the insertion agree. The names
    /// are provisional: only the plan can pick a collision suffix, and it runs
    /// against the live source at insertion time.
    pub(crate) fn preview(&self, imports: bool) -> Result<Snippet, SnippetError> {
        let identifiers = if imports {
            let planned: Vec<PlannedValue> = self
                .planned_values()
                .iter()
                .map(PlannedFieldValue::planned_value)
                .collect();
            provisional_identifiers(&planned)
        } else {
            BTreeMap::new()
        };
        let values = self
            .values()
            .into_iter()
            .map(|(name, value)| {
                let planned = identifiers
                    .get(&name)
                    .filter(|_| {
                        self.fields.iter().any(|field| {
                            field.prop.name == name
                                && matches!(
                                    field.control,
                                    PropControl::Image | PropControl::Entry { .. }
                                )
                        })
                    })
                    .cloned();
                match planned {
                    Some(identifier) => (name, SnippetValue::Identifier(identifier)),
                    None => (name, value),
                }
            })
            .collect();
        generate_tag(&TagRequest {
            component: &self.component,
            values,
            content: (!self.content.trim().is_empty()).then(|| self.content.clone()),
            client_directive: self.client_directive(),
        })
    }
}

/// A form value that changes the import plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PlannedFieldValue {
    Image {
        prop: String,
        path: String,
    },
    Entry {
        prop: String,
        collection: String,
        id: String,
    },
}

impl PlannedFieldValue {
    /// The planner's view of this value.
    pub(crate) fn planned_value(&self) -> PlannedValue {
        match self {
            Self::Image { prop, path } => PlannedValue::Image {
                prop: prop.clone(),
                path: path.clone(),
            },
            Self::Entry {
                prop,
                collection,
                id,
            } => PlannedValue::Entry {
                prop: prop.clone(),
                collection: collection.clone(),
                id: id.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_astro::components::{
        ClientDirective, ComponentLocation, Framework, PropKind, Slot,
    };
    use std::path::PathBuf;

    fn component(props: Vec<Prop>, wraps: bool) -> Component {
        Component {
            name: "BookCard".into(),
            framework: Framework::React,
            location: ComponentLocation::Project {
                source: PathBuf::from("/project/src/components/BookCard.tsx"),
                relative_path: PathBuf::from("src/components/BookCard.tsx"),
            },
            description: Some("A book card".into()),
            props,
            slots: if wraps {
                vec![Slot { name: None }]
            } else {
                Vec::new()
            },
            client_directives: vec![
                ClientDirective {
                    name: "client:load",
                    requires_value: false,
                },
                ClientDirective {
                    name: "client:only",
                    requires_value: true,
                },
            ],
        }
    }

    fn prop(name: &str, kind: PropKind, required: bool) -> Prop {
        Prop {
            name: name.into(),
            kind,
            required,
            default: None,
            description: None,
        }
    }

    fn sample() -> Component {
        let mut format = prop(
            "format",
            PropKind::Enum {
                values: vec!["hardback".into(), "paperback".into()],
            },
            false,
        );
        format.default = Some(PropDefault::String("paperback".into()));
        let mut rating = prop("rating", PropKind::Number, false);
        rating.default = Some(PropDefault::Number("3".into()));
        let mut spoilers = prop("spoilers", PropKind::Boolean, false);
        spoilers.default = Some(PropDefault::Boolean(true));
        component(
            vec![
                prop("title", PropKind::String, true),
                prop("author", PropKind::String, true),
                prop("cover", PropKind::Image, false),
                format,
                rating,
                spoilers,
                prop("tags", PropKind::StringArray, false),
                prop(
                    "review",
                    PropKind::CollectionEntry {
                        collection: "books".into(),
                    },
                    false,
                ),
            ],
            true,
        )
    }

    #[test]
    fn defaults_are_shown_and_optional_props_start_excluded() {
        let form = FormState::new(&sample(), None);
        let title = &form.fields()[0];
        assert!(title.included(), "required props are always included");
        assert_eq!(title.default_label(), None);
        let format = &form.fields()[3];
        assert!(!format.included());
        assert_eq!(format.text(), "paperback");
        assert_eq!(format.default_label().as_deref(), Some("paperback"));
        let spoilers = &form.fields()[5];
        assert!(!spoilers.included());
        assert_eq!(spoilers.default_label().as_deref(), Some("true"));
        assert!(form.fields()[6].default_label().is_none());
    }

    #[test]
    fn required_blanks_and_optional_values_shape_the_tag() {
        let mut form = FormState::new(&sample(), None);
        assert_eq!(form.first_blank_required(), Some(0));
        let preview = form.preview(false).unwrap();
        assert!(preview.text.ends_with(">\n  \n</BookCard>"));
        assert!(preview.text.contains("title=\"\""));
        assert!(preview.text.contains("author=\"\""));
        // Both blank required props and the empty content are tracked.
        assert_eq!(preview.placeholders.len(), 3);
        assert!(matches!(
            preview.placeholders[0].kind,
            astrolith_astro::components::PlaceholderKind::RequiredProp(_)
        ));

        form.set_text(0, "Atomic Habits");
        form.set_text(6, "habits, productivity");
        form.set_text(4, "4");
        let preview = form.preview(false).unwrap();
        assert!(preview.text.contains("title=\"Atomic Habits\""));
        assert!(preview.text.contains("author=\"\""));
        assert!(preview.text.contains("rating={4}"));
        assert!(
            preview
                .text
                .contains("tags={[\"habits\", \"productivity\"]}")
        );
        // Untouched optional props stay omitted, including their defaults.
        assert!(!preview.text.contains("format="));
        assert!(!preview.text.contains("spoilers"));
        assert_eq!(form.first_blank_required(), Some(1));
    }

    #[test]
    fn toggle_all_optional_includes_value_bearing_fields_and_back() {
        let mut form = FormState::new(&sample(), None);
        form.set_text(0, "T");
        assert!(!form.all_optional_included());
        assert!(form.toggle_all_optional());
        assert!(form.all_optional_included());
        let preview = form.preview(false).unwrap();
        assert!(preview.text.contains("format=\"paperback\""));
        assert!(preview.text.contains("rating={3}"));
        assert!(preview.text.contains("spoilers"));
        // An empty optional text prop still contributes nothing.
        assert!(!preview.text.contains("cover="));
        assert!(!preview.text.contains("tags="));
        assert!(!form.toggle_all_optional());
        assert!(!form.all_optional_included());
        let preview = form.preview(false).unwrap();
        assert!(!preview.text.contains("format="));
    }

    #[test]
    fn directive_and_identifier_values_reach_the_preview() {
        let mut form = FormState::new(&sample(), None);
        form.set_text(0, "T");
        form.set_text(1, "A");
        form.set_text(2, "../../assets/cover.png");
        form.set_text(7, "atomic-habits");
        form.set_directive(Some("client:only".into()));
        assert!(form.preview(false).is_err(), "client:only needs a value");
        form.set_directive_value("react");
        // With imports on, image and entry values render as the identifiers the
        // plan resolves, so preview and insertion agree.
        let preview = form.preview(true).unwrap();
        assert!(preview.text.contains("cover={cover}"));
        assert!(preview.text.contains("review={reviewEntry}"));
        assert!(preview.text.contains("client:only=\"react\""));
        // Without imports, the raw path and id render as strings.
        let preview = form.preview(false).unwrap();
        assert!(preview.text.contains("client:only=\"react\""));
        assert!(preview.text.contains("cover=\"../../assets/cover.png\""));
        assert!(preview.text.contains("review=\"atomic-habits\""));
    }

    #[test]
    fn blank_required_enum_does_not_take_the_caret() {
        let component = component(
            vec![
                prop(
                    "format",
                    PropKind::Enum {
                        values: vec!["hardback".into()],
                    },
                    true,
                ),
                prop("title", PropKind::String, true),
            ],
            false,
        );
        let form = FormState::new(&component, None);
        assert_eq!(
            form.first_blank_required(),
            Some(1),
            "only a textual control can receive the caret"
        );
    }
}
