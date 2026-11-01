//! Writer chrome: Markdown presentation, chrome auto-hide and footer counts.
//!
//! The pinned editor paints only the plain Markdown source, so the writer's
//! presentation comes from tracked *text decorations* built from this module's
//! own line analysis. Nothing here reads state or performs I/O: the analysis is
//! a pure function of the visible text, and the policies are pure state
//! machines, so both are covered by unit tests.
//!
//! Two capabilities the pinned facade does not expose are recorded here so a
//! reader is not misled by the code:
//!
//! - `HighlightStyle` carries colour, weight, style, background, underline,
//!   strike-through and opacity only — no font family and no glyph offset. Code
//!   spans and blocks therefore take the writer's monospace face as a whole
//!   (the editor is already monospace) plus an explicit background, and heading
//!   markers cannot be hung into the margin: when the pane is wide enough they
//!   are de-emphasised instead of shifted.
//! - Syntax colouring inside code is suppressed by *not emitting* any
//!   prose token there; the analysis never colours a marker, heading, emphasis
//!   or quote inside an inline code span or a fenced block.

use std::ops::Range;
use std::path::PathBuf;

use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::TextDecoration;
use gpui_kit::{FontStyle, FontWeight, HighlightStyle, Hsla};

/// One span of the writer's Markdown presentation.
///
/// Spans never overlap: a marker and the text it wraps are adjacent, and a code
/// span replaces whatever would otherwise have been analysed inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProseToken {
    /// Markdown punctuation: heading hashes, emphasis markers, brackets and
    /// parentheses, quote marks and list bullets.
    Marker,
    /// Heading text.
    Heading,
    /// An inline code span, backticks included.
    CodeSpan,
    /// A fenced code block, fences included, one span per line.
    CodeBlock,
    /// Blockquote content, after its marker.
    Quote,
    /// Bold text.
    Bold,
    /// Italic text.
    Italic,
}

/// Analyse `text` into non-overlapping presentation spans, in document order.
pub(crate) fn analyze_prose(text: &str) -> Vec<(Range<usize>, ProseToken)> {
    let mut spans = Vec::new();
    let mut in_fence = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.strip_suffix('\n').unwrap_or(line);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let line_start = offset;
        offset += line.len();
        if content.is_empty() {
            continue;
        }
        let indent = content.len() - content.trim_start_matches(' ').len();
        let rest = &content[indent..];
        if indent <= 3 && (rest.starts_with("```") || rest.starts_with("~~~")) {
            // Fence lines belong to the block and carry no other tokens.
            spans.push((
                line_start..line_start + content.len(),
                ProseToken::CodeBlock,
            ));
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            spans.push((
                line_start..line_start + content.len(),
                ProseToken::CodeBlock,
            ));
            continue;
        }
        if let Some(markers) = heading_markers(rest) {
            let marker_end = indent + markers;
            spans.push((
                line_start + indent..line_start + marker_end,
                ProseToken::Marker,
            ));
            if marker_end < content.len() {
                spans.push((
                    line_start + marker_end..line_start + content.len(),
                    ProseToken::Heading,
                ));
            }
            continue;
        }
        let mut body_start = indent;
        let mut depth = 0;
        while let Some(quote) = rest_and_quote(&content[body_start..]) {
            let quote_end = body_start + quote;
            spans.push((
                line_start + body_start..line_start + quote_end,
                ProseToken::Marker,
            ));
            body_start = quote_end;
            depth += 1;
            if depth > 8 {
                break;
            }
        }
        let body = &content[body_start..];
        if body_start < content.len() {
            if let Some(bullet) = list_marker(body) {
                spans.push((
                    line_start + body_start..line_start + body_start + bullet,
                    ProseToken::Marker,
                ));
                body_start += bullet;
            }
            inline_spans(
                &content[body_start..],
                line_start + body_start,
                if depth > 0 {
                    ProseToken::Quote
                } else {
                    ProseToken::Marker
                },
                &mut spans,
            );
        }
    }
    spans.sort_by_key(|(range, _)| (range.start, range.end));
    spans
}

/// Heading hashes plus their separator, when the line is a heading.
fn heading_markers(rest: &str) -> Option<usize> {
    let hashes = rest.bytes().take_while(|byte| *byte == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    match rest.as_bytes().get(hashes) {
        None => Some(hashes),
        Some(b' ' | b'\t') => {
            let separator = rest[hashes..]
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count();
            Some(hashes + separator)
        }
        Some(_) => None,
    }
}

/// Length of one `>` quote marker and its separating space.
fn rest_and_quote(text: &str) -> Option<usize> {
    let rest = text.strip_prefix('>')?;
    let spaces = rest.len() - rest.trim_start_matches(' ').len();
    Some(1 + spaces)
}

/// Length of a `-`, `*`, `+` or `1.` list marker and its space.
fn list_marker(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let bullet = match bytes.first()? {
        b'-' | b'*' | b'+' => 1,
        byte if byte.is_ascii_digit() => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if bytes.get(digits) == Some(&b'.') {
                digits + 1
            } else {
                return None;
            }
        }
        _ => return None,
    };
    match bytes.get(bullet) {
        Some(b' ') => Some(bullet + 1),
        _ => None,
    }
}

/// Analyse one line body, given the token to use for prose markers.
fn inline_spans(
    body: &str,
    base: usize,
    prose: ProseToken,
    spans: &mut Vec<(Range<usize>, ProseToken)>,
) {
    let bytes = body.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'`' {
            let run = bytes[index..]
                .iter()
                .take_while(|byte| **byte == b'`')
                .count();
            let Some(close) =
                find_run(&bytes[index + run..], b'`', run).map(|close| index + run + close)
            else {
                break;
            };
            spans.push((base + index..base + close + run, ProseToken::CodeSpan));
            index = close + run;
            continue;
        }
        match bytes[index] {
            b'*' | b'_' => {
                let marker = bytes[index];
                let run = bytes[index..]
                    .iter()
                    .take_while(|byte| **byte == marker)
                    .count();
                if run > 2 {
                    spans.push((base + index..base + index + run, prose));
                    index += run;
                    continue;
                }
                if let Some(close) =
                    find_run(&bytes[index + run..], marker, run).map(|close| index + run + close)
                    && close > index + run
                {
                    let token = if run == 2 {
                        ProseToken::Bold
                    } else {
                        ProseToken::Italic
                    };
                    spans.push((base + index..base + index + run, prose));
                    spans.push((base + index + run..base + close, token));
                    spans.push((base + close..base + close + run, prose));
                    index = close + run;
                    continue;
                }
                index += run;
            }
            b'[' | b']' | b'(' | b')' => {
                spans.push((base + index..base + index + 1, prose));
                index += 1;
            }
            _ => index += 1,
        }
    }
}

/// Offset of the next run of exactly `width` `marker` bytes, or `None`.
fn find_run(bytes: &[u8], marker: u8, width: usize) -> Option<usize> {
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == marker {
            let run = bytes[index..]
                .iter()
                .take_while(|byte| **byte == marker)
                .count();
            if run == width {
                return Some(index);
            }
            index += run;
        } else {
            index += 1;
        }
    }
    None
}

/// Colours for one theme's writer presentation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProsePalette {
    /// Markers, de-emphasised against the prose.
    pub marker: Hsla,
    /// Heading text colour, the same at every level.
    pub heading: Hsla,
    /// Code foreground.
    pub code: Hsla,
    /// Code background.
    pub code_background: Hsla,
    /// Blockquote foreground.
    pub quote: Hsla,
}

impl ProsePalette {
    /// Read the palette from the active component theme.
    pub(crate) fn current(cx: &gpui_kit::App) -> Self {
        let theme = cx.theme();
        Self {
            marker: theme.muted_foreground,
            heading: theme.foreground,
            code: theme.foreground,
            code_background: theme.button,
            quote: theme.muted_foreground,
        }
    }
}

/// Turn analysed spans into one decoration layer.
///
/// `hanging` is true when the pane is wide enough for hanging heading markers;
/// the pinned facade cannot move glyphs, so a hanging pane lowers the marker
/// opacity further instead of shifting it into the margin.
pub(crate) fn prose_decorations(
    spans: &[(Range<usize>, ProseToken)],
    palette: &ProsePalette,
    hanging: bool,
) -> Vec<TextDecoration> {
    spans
        .iter()
        .filter(|(range, _)| !range.is_empty())
        .map(|(range, token)| {
            let style = match token {
                ProseToken::Marker => HighlightStyle {
                    color: Some(palette.marker),
                    fade_out: Some(if hanging { 0.5 } else { 0.25 }),
                    ..HighlightStyle::default()
                },
                ProseToken::Heading => HighlightStyle {
                    color: Some(palette.heading),
                    font_weight: Some(FontWeight::BOLD),
                    ..HighlightStyle::default()
                },
                ProseToken::CodeSpan | ProseToken::CodeBlock => HighlightStyle {
                    color: Some(palette.code),
                    background_color: Some(palette.code_background),
                    ..HighlightStyle::default()
                },
                ProseToken::Quote => HighlightStyle {
                    color: Some(palette.quote),
                    font_style: Some(FontStyle::Italic),
                    ..HighlightStyle::default()
                },
                ProseToken::Bold => HighlightStyle {
                    font_weight: Some(FontWeight::BOLD),
                    ..HighlightStyle::default()
                },
                ProseToken::Italic => HighlightStyle {
                    font_style: Some(FontStyle::Italic),
                    ..HighlightStyle::default()
                },
            };
            TextDecoration::new((*range).clone(), style)
        })
        .collect()
}

/// Whether the pane is wide enough to hang heading markers at its text edges.
pub(crate) fn hanging_markers(pane_width: f32) -> bool {
    pane_width >= 760.
}

/// Typed-character accounting for the chrome auto-hide policy.
///
/// The baseline is keyed by the active document and measured in characters, so
/// switching documents, growing the text with a formatting command or a link
/// paste, and inserting a multi-byte character cannot satisfy the policy.
#[derive(Clone, Debug, Default)]
pub(crate) struct TypedCharacters {
    last: Option<(PathBuf, usize)>,
}

impl TypedCharacters {
    /// Characters the user typed into `path` since the previous observation.
    ///
    /// `programmatic` marks text this session wrote itself: the baseline moves
    /// without counting anything, so formatting and pasting stay invisible.
    pub(crate) fn observe(&mut self, path: PathBuf, visible: &str, programmatic: bool) -> usize {
        let characters = visible.chars().count();
        let typed = match (&self.last, programmatic) {
            (Some((last_path, last_characters)), false)
                if last_path == &path && characters > *last_characters =>
            {
                characters - last_characters
            }
            _ => 0,
        };
        self.last = Some((path, characters));
        typed
    }

    /// Drop the baseline, so the next observation starts a fresh count.
    ///
    /// Called when no document is active: a document reopened after that may
    /// have been reloaded from disk, so its previous length means nothing.
    pub(crate) fn forget(&mut self) {
        self.last = None;
    }
}

/// Chrome auto-hide after continuous typing with both sidebars closed.
///
/// The writer hides its title bar once four characters have been typed while
/// neither sidebar is open, and a hover at the window's top edge restores it.
/// The state is explicit so the policy is testable without a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChromeVisibility {
    typed: u32,
    hidden: bool,
}

/// Typed characters that hide the chrome.
pub(crate) const AUTO_HIDE_AFTER: u32 = 4;

impl ChromeVisibility {
    /// Record `characters` typed since the last change.
    ///
    /// Returns whether the chrome should be hidden now. Nothing hides while a
    /// sidebar is open, and non-typing edits (zero or negative growth) never
    /// accumulate.
    pub(crate) fn type_characters(
        &mut self,
        characters: usize,
        both_sidebars_closed: bool,
    ) -> bool {
        if !both_sidebars_closed {
            self.typed = 0;
            self.hidden = false;
            return false;
        }
        self.typed = self
            .typed
            .saturating_add(characters.min(u32::MAX as usize) as u32);
        if self.typed >= AUTO_HIDE_AFTER {
            self.hidden = true;
        }
        self.hidden
    }

    /// Restore the chrome; returns whether it was hidden.
    pub(crate) fn restore(&mut self) -> bool {
        self.typed = 0;
        std::mem::replace(&mut self.hidden, false)
    }

    /// Track sidebar visibility; opening either sidebar always restores.
    pub(crate) fn set_sidebars(&mut self, both_sidebars_closed: bool) {
        if !both_sidebars_closed {
            self.typed = 0;
            self.hidden = false;
        }
    }

    /// Whether the title bar is currently hidden.
    pub(crate) fn is_hidden(&self) -> bool {
        self.hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<(String, ProseToken)> {
        analyze_prose(text)
            .into_iter()
            .map(|(range, token)| (text[range].to_owned(), token))
            .collect()
    }

    fn assert_non_overlapping(text: &str) {
        let spans = analyze_prose(text);
        for pair in spans.windows(2) {
            assert!(
                pair[0].0.end <= pair[1].0.start,
                "overlapping spans in {text:?}: {:?} then {:?}",
                pair[0].0,
                pair[1].0
            );
        }
        for (range, _) in &spans {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn dims_markdown_markers_and_styles_the_text_they_wrap() {
        assert_eq!(
            tokens("## Title\n"),
            [
                ("## ".to_owned(), ProseToken::Marker),
                ("Title".to_owned(), ProseToken::Heading),
            ]
        );
        assert_eq!(
            tokens("a **bold** and *soft* word"),
            [
                ("**".to_owned(), ProseToken::Marker),
                ("bold".to_owned(), ProseToken::Bold),
                ("**".to_owned(), ProseToken::Marker),
                ("*".to_owned(), ProseToken::Marker),
                ("soft".to_owned(), ProseToken::Italic),
                ("*".to_owned(), ProseToken::Marker),
            ]
        );
    }

    #[test]
    fn headings_render_uniformly_at_every_level() {
        for level in 1..=4 {
            let text = format!("{} Heading\n", "#".repeat(level));
            let spans = analyze_prose(&text);
            let heading = spans
                .iter()
                .find(|(_, token)| *token == ProseToken::Heading)
                .expect("the heading text is present");
            assert_eq!(&text[heading.0.clone()], "Heading");
            let decorations = prose_decorations(&spans, &palette(), true);
            let heading_style = decorations
                .iter()
                .zip(&spans)
                .find(|(_, (_, token))| *token == ProseToken::Heading)
                .map(|(decoration, _)| decoration.style)
                .unwrap();
            assert_eq!(heading_style.font_weight, Some(FontWeight::BOLD));
        }
    }

    #[test]
    fn code_spans_and_blocks_take_a_background_and_no_prose_tokens() {
        let text = "use `a **b**` here\n\n```rust\n# not a heading\n> not a quote\n```\n";
        let spans = analyze_prose(text);
        assert_non_overlapping(text);
        for (range, token) in &spans {
            if matches!(token, ProseToken::CodeSpan | ProseToken::CodeBlock) {
                continue;
            }
            for code in spans
                .iter()
                .filter(|(_, token)| matches!(token, ProseToken::CodeSpan | ProseToken::CodeBlock))
            {
                assert!(
                    range.end <= code.0.start || code.0.end <= range.start,
                    "prose token {:?} overlaps code {:?}",
                    range,
                    code.0
                );
            }
        }
        let decorations = prose_decorations(&spans, &palette(), false);
        assert_eq!(decorations.len(), spans.len());
        for ((_, token), decoration) in spans.iter().zip(&decorations) {
            let code = matches!(token, ProseToken::CodeSpan | ProseToken::CodeBlock);
            assert_eq!(
                decoration.style.background_color == Some(palette().code_background),
                code,
                "only code takes the code background: {token:?}"
            );
        }
        assert!(tokens(text).contains(&("```rust".to_owned(), ProseToken::CodeBlock)));
        assert!(tokens(text).contains(&("# not a heading".to_owned(), ProseToken::CodeBlock)));
    }

    #[test]
    fn quotes_and_list_markers_are_presentation_only() {
        assert_eq!(
            tokens("> quoted *words*\n"),
            [
                ("> ".to_owned(), ProseToken::Marker),
                ("*".to_owned(), ProseToken::Quote),
                ("words".to_owned(), ProseToken::Italic),
                ("*".to_owned(), ProseToken::Quote),
            ]
        );
        assert_eq!(tokens("- item\n"), [("- ".to_owned(), ProseToken::Marker),]);
    }

    #[test]
    fn link_punctuation_is_a_marker_and_the_anchor_stays_text() {
        assert_eq!(
            tokens("[label](https://x.dev)\n"),
            [
                ("[".to_owned(), ProseToken::Marker),
                ("]".to_owned(), ProseToken::Marker),
                ("(".to_owned(), ProseToken::Marker),
                (")".to_owned(), ProseToken::Marker),
            ]
        );
    }

    #[test]
    fn unicode_and_crlf_lines_keep_byte_boundaries() {
        let text = "## Ünïcode 🦀\r\n\r\nquote: *déjà*\r\n";
        assert_non_overlapping(text);
        assert!(tokens(text).contains(&("Ünïcode 🦀".to_owned(), ProseToken::Heading)));
        assert!(tokens(text).contains(&("déjà".to_owned(), ProseToken::Italic)));
    }

    #[test]
    fn chrome_hides_only_after_four_typed_characters_with_sidebars_closed() {
        let mut chrome = ChromeVisibility::default();
        assert!(!chrome.type_characters(3, true));
        assert!(!chrome.is_hidden());
        assert!(chrome.type_characters(1, true));
        assert!(chrome.is_hidden());

        let mut chrome = ChromeVisibility::default();
        for _ in 0..8 {
            assert!(!chrome.type_characters(1, false));
        }
        assert!(!chrome.is_hidden());

        // Opening a sidebar while hidden restores the chrome.
        let mut chrome = ChromeVisibility::default();
        assert!(chrome.type_characters(AUTO_HIDE_AFTER as usize, true));
        chrome.set_sidebars(false);
        assert!(!chrome.is_hidden());
        // The counter restarts, so one more character cannot hide it again.
        assert!(!chrome.type_characters(1, false));
    }

    #[test]
    fn hovering_the_top_edge_restores_the_chrome_once() {
        let mut chrome = ChromeVisibility::default();
        chrome.type_characters(AUTO_HIDE_AFTER as usize, true);
        assert!(chrome.restore());
        assert!(!chrome.is_hidden());
        assert!(!chrome.restore());
        assert!(!chrome.type_characters(3, true));
    }

    #[test]
    fn non_typing_edits_do_not_hide_the_chrome() {
        let mut chrome = ChromeVisibility::default();
        for _ in 0..8 {
            assert!(!chrome.type_characters(0, true));
        }
        assert!(!chrome.is_hidden());
    }

    #[test]
    fn typed_characters_skip_commands_and_document_switches() {
        let mut typed = TypedCharacters::default();
        assert_eq!(typed.observe("a.md".into(), "one", false), 0);
        assert_eq!(typed.observe("a.md".into(), "one two", false), 4);
        // A formatting command grows the text without typing: the baseline
        // moves and the policy stays where it was.
        let mut chrome = ChromeVisibility::default();
        assert!(!chrome.type_characters(typed.observe("a.md".into(), "**one** two", true), true));
        assert!(
            !chrome.type_characters(typed.observe("a.md".into(), "**one** two  ", true), true),
            "programmatic growth never hides the chrome"
        );
        // The next real keystroke is one character, not the whole rewrite.
        assert!(
            !chrome.type_characters(typed.observe("a.md".into(), "**one** two  x", false), true)
        );

        // Closing the document drops the baseline, so reopening a reloaded file
        // does not count its whole new length as typing.
        let mut typed = TypedCharacters::default();
        assert_eq!(typed.observe("a.md".into(), "short", false), 0);
        typed.forget();
        assert_eq!(typed.observe("a.md".into(), &"long ".repeat(20), false), 0);

        // Switching documents starts from a fresh baseline.
        let mut typed = TypedCharacters::default();
        assert_eq!(typed.observe("a.md".into(), "short", false), 0);
        assert_eq!(
            typed.observe("b.md".into(), &"long document ".repeat(20), false),
            0
        );
        assert_eq!(
            typed.observe(
                "b.md".into(),
                &format!("{}x", "long document ".repeat(20)),
                false
            ),
            1
        );
    }

    #[test]
    fn typed_characters_count_cjk_as_one_character_each() {
        let mut typed = TypedCharacters::default();
        let mut chrome = ChromeVisibility::default();
        let mut text = String::new();
        assert_eq!(typed.observe("a.md".into(), &text, false), 0);
        // Three bytes per character: byte counting would hide on the second
        // keystroke and is not what "characters" means.
        let mut hidden = false;
        for character in ['日', '本', '語', '文'] {
            text.push(character);
            hidden = chrome.type_characters(typed.observe("a.md".into(), &text, false), true);
            assert!(!hidden || text.chars().count() == 4);
        }
        assert!(hidden);
    }

    #[test]
    fn hanging_markers_need_a_wide_pane() {
        assert!(hanging_markers(1200.));
        assert!(!hanging_markers(640.));
    }

    fn palette() -> ProsePalette {
        ProsePalette {
            marker: gpui_kit::rgb(0x666666).into(),
            heading: gpui_kit::rgb(0x111111).into(),
            code: gpui_kit::rgb(0x222222).into(),
            code_background: gpui_kit::rgb(0xeeeeee).into(),
            quote: gpui_kit::rgb(0x444444).into(),
        }
    }
}
