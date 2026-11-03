//! Tag and wrapping-scaffold generation with tracked placeholder ranges.

use std::ops::Range;

use super::{Component, PropKind};

/// A value supplied by the builder form.
#[derive(Clone, Debug, PartialEq)]
pub enum SnippetValue {
    /// A quoted string, enum value or date.
    String(String),
    /// A numeric literal, preserved as source text.
    Number(String),
    Boolean(bool),
    /// A string or number array literal.
    List(Vec<String>),
    /// A planned import identifier or `getEntry` constant.
    Identifier(String),
}

/// A selected hydration directive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientDirectiveValue {
    pub name: String,
    pub value: Option<String>,
}

/// One insertion request.
#[derive(Clone, Debug)]
pub struct TagRequest<'a> {
    pub component: &'a Component,
    pub values: Vec<(String, SnippetValue)>,
    pub content: Option<String>,
    pub client_directive: Option<ClientDirectiveValue>,
}

/// What a tracked placeholder fills.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaceholderKind {
    /// An empty required prop value.
    RequiredProp(String),
    /// The wrapped child content.
    Content,
}

/// A tracked range in the generated text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnippetPlaceholder {
    pub range: Range<usize>,
    pub kind: PlaceholderKind,
}

/// Generated source plus placeholder ranges relative to that text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snippet {
    pub text: String,
    pub placeholders: Vec<SnippetPlaceholder>,
}

/// A tag generation failure. Callers fail closed and insert nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnippetError {
    ContentUnsupported,
    DirectiveUnsupported { name: String },
    DirectiveValueMissing { name: String },
    InvalidValue { prop: String },
    InvalidIdentifier { prop: String },
}

impl std::fmt::Display for SnippetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContentUnsupported => {
                formatter.write_str("This component does not accept wrapped content")
            }
            Self::DirectiveUnsupported { name } => {
                write!(formatter, "`{name}` is not supported by this component")
            }
            Self::DirectiveValueMissing { name } => {
                write!(formatter, "`{name}` requires a value")
            }
            Self::InvalidValue { prop } => {
                write!(formatter, "`{prop}` has an invalid value")
            }
            Self::InvalidIdentifier { prop } => {
                write!(formatter, "`{prop}` resolved to an invalid identifier")
            }
        }
    }
}

impl std::error::Error for SnippetError {}

/// Generate one self-closing tag or a wrapping scaffold.
pub fn generate_tag(request: &TagRequest<'_>) -> Result<Snippet, SnippetError> {
    let component = request.component;
    let name = &component.name;
    // `<` plus the tag name precede every attribute in the final text.
    let attribute_base = 1 + name.len();
    let mut attributes = String::new();
    let mut placeholders = Vec::new();

    for prop in &component.props {
        let Some(value) = request
            .values
            .iter()
            .find(|(name, _)| name == &prop.name)
            .map(|(_, value)| value)
        else {
            if prop.required {
                push_required_placeholder(
                    &mut attributes,
                    &mut placeholders,
                    attribute_base,
                    &prop.name,
                );
            }
            continue;
        };
        if prop.required && matches!(value, SnippetValue::String(text) if text.is_empty()) {
            push_required_placeholder(
                &mut attributes,
                &mut placeholders,
                attribute_base,
                &prop.name,
            );
            continue;
        }
        render_value(&mut attributes, prop, value)?;
    }

    if let Some(directive) = &request.client_directive {
        let supported = component
            .client_directives
            .iter()
            .find(|candidate| candidate.name == directive.name)
            .ok_or_else(|| SnippetError::DirectiveUnsupported {
                name: directive.name.clone(),
            })?;
        if supported.requires_value && directive.value.as_deref().is_none_or(str::is_empty) {
            return Err(SnippetError::DirectiveValueMissing {
                name: directive.name.clone(),
            });
        }
        attributes.push(' ');
        attributes.push_str(&directive.name);
        if let Some(value) = &directive.value {
            attributes.push_str("=\"");
            attributes.push_str(&escape_attribute(value));
            attributes.push('"');
        }
    }

    let content = request
        .content
        .as_deref()
        .filter(|content| !content.trim().is_empty());
    if content.is_some() && !component.has_default_slot() {
        return Err(SnippetError::ContentUnsupported);
    }

    let mut text = format!("<{name}{attributes}");
    if component.has_default_slot() {
        text.push_str(">\n  ");
        let placeholder_start = text.len();
        match content {
            Some(content) => {
                text.push_str(&indent_content(content));
                text.push('\n');
            }
            None => {
                text.push('\n');
                placeholders.push(SnippetPlaceholder {
                    range: placeholder_start..placeholder_start,
                    kind: PlaceholderKind::Content,
                });
            }
        }
        text.push_str(&format!("</{name}>"));
    } else {
        text.push_str(" />");
    }
    Ok(Snippet { text, placeholders })
}

fn push_required_placeholder(
    attributes: &mut String,
    placeholders: &mut Vec<SnippetPlaceholder>,
    base: usize,
    name: &str,
) {
    attributes.push(' ');
    attributes.push_str(name);
    attributes.push_str("=\"");
    let start = base + attributes.len();
    attributes.push('"');
    placeholders.push(SnippetPlaceholder {
        range: start..start,
        kind: PlaceholderKind::RequiredProp(name.to_owned()),
    });
}

fn render_value(
    attributes: &mut String,
    prop: &super::Prop,
    value: &SnippetValue,
) -> Result<(), SnippetError> {
    attributes.push(' ');
    attributes.push_str(&prop.name);
    match value {
        SnippetValue::String(text) => {
            attributes.push_str("=\"");
            attributes.push_str(&escape_attribute(text));
            attributes.push('"');
        }
        SnippetValue::Number(number) => {
            validate_number(number).ok_or_else(|| SnippetError::InvalidValue {
                prop: prop.name.clone(),
            })?;
            attributes.push_str("={");
            attributes.push_str(number);
            attributes.push('}');
        }
        SnippetValue::Boolean(true) => {}
        SnippetValue::Boolean(false) => attributes.push_str("={false}"),
        SnippetValue::List(values) => {
            attributes.push_str("={[");
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    attributes.push_str(", ");
                }
                if matches!(prop.kind, PropKind::NumberArray) {
                    validate_number(value).ok_or_else(|| SnippetError::InvalidValue {
                        prop: prop.name.clone(),
                    })?;
                    attributes.push_str(value);
                } else {
                    attributes.push('"');
                    attributes.push_str(&escape_js_string(value));
                    attributes.push('"');
                }
            }
            attributes.push_str("]}");
        }
        SnippetValue::Identifier(identifier) => {
            if !is_identifier(identifier) {
                return Err(SnippetError::InvalidIdentifier {
                    prop: prop.name.clone(),
                });
            }
            attributes.push_str("={");
            attributes.push_str(identifier);
            attributes.push('}');
        }
    }
    Ok(())
}

fn validate_number(text: &str) -> Option<()> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    text.parse::<f64>().ok().map(|_| ())
}

fn is_identifier(text: &str) -> bool {
    let mut characters = text.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || matches!(first, '_' | '$'))
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '$'))
}

fn indent_content(content: &str) -> String {
    let mut text = String::with_capacity(content.len());
    let mut rest = content;
    let mut first = true;
    while !rest.is_empty() {
        let (line, remainder) = match rest.find('\n') {
            Some(index) => (&rest[..index], &rest[index + 1..]),
            None => (rest, ""),
        };
        if !first {
            text.push('\n');
            text.push_str("  ");
        }
        text.push_str(line);
        first = false;
        rest = remainder;
    }
    text
}

fn escape_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;")
        .replace('\n', "&#10;")
}

fn escape_js_string(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            other => escaped.push(other),
        }
    }
    escaped
}
