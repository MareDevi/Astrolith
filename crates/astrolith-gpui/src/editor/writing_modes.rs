//! Writing modes: prose typography, focus dimming and typewriter centering.
//!
//! Everything the writer needs from the editor's *geometry* is read through
//! public accessors (`cursor_layout`, `input_bounds`, `scroll_offset`,
//! `line_height`) and written back through `set_scroll_offset`, which the
//! editor defers to the next layout and clamps itself. The calculations are
//! pure functions so the typewriter formula is unit-tested without a window.
//!
//! Cost note: presentation analysis runs once per text change, not per frame.
//! It is a linear byte scan, the same order as the P4 whole-visible diff the
//! session already performs on every change, and it is skipped entirely when
//! neither the text nor the active mode changed.

use astrolith_document::{cursor_sentence, other_sentences};
use gpui_kit::component::input::{EditorState, TextDecoration, TextDecorationCollection};
use gpui_kit::{App, Bounds, Entity, HighlightStyle, Pixels, Point, SharedString, px};

use crate::editor::writer_chrome::{
    ProsePalette, analyze_prose, hanging_markers, prose_decorations,
};

/// Which writing modes are active for the whole writer surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WritingModes {
    /// Dim every sentence except the cursor sentence.
    pub focus: bool,
    /// Keep the cursor line vertically centred.
    pub typewriter: bool,
}

/// Resolved document presentation: face, size, line spacing and measure.
///
/// The values come from preferences and are used as written; the pane no
/// longer scales type by its width, so an explicit size or line height means
/// the same thing in every window. Out-of-range values from a hand-edited file
/// are clamped rather than trusted by the renderer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProseStyle {
    /// Document face.
    pub font_family: SharedString,
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Widest prose column in logical pixels.
    pub measure: f32,
}

impl ProseStyle {
    pub(crate) fn new(
        font_family: SharedString,
        font_size: f32,
        line_height: f32,
        measure: f32,
    ) -> Self {
        Self {
            font_family,
            font_size: font_size.clamp(1., 30.),
            line_height: line_height.clamp(1.2, 3.),
            measure: measure.clamp(480., 1200.),
        }
    }
}

/// Scroll offset that puts the cursor line at the viewport's vertical centre.
///
/// `cursor` and `viewport` are window-space bounds from the editor's last
/// layout and `current` is the live scroll offset, so the caret's position in
/// content coordinates is recoverable. The result is handed to
/// `set_scroll_offset`, which clamps it to the scrollable range after the next
/// layout — so a target above the first line or below the last is safe.
pub(crate) fn centered_scroll_offset(
    cursor: Bounds<Pixels>,
    viewport: Bounds<Pixels>,
    line_height: Pixels,
    current: Point<Pixels>,
) -> Point<Pixels> {
    let caret_in_content =
        f32::from(cursor.origin.y) - f32::from(viewport.origin.y) + f32::from(current.y);
    let centre = (f32::from(viewport.size.height) - f32::from(line_height)) / 2.;
    Point::new(current.x, px(caret_in_content - centre))
}

/// Per-session presentation: the decoration layers and the typewriter request.
pub(crate) struct SessionPresentation {
    /// Writer prose layer built from the Markdown analysis.
    prose: Option<TextDecorationCollection>,
    /// Focus-mode dimming layer.
    focus: Option<TextDecorationCollection>,
    /// What the prose layer was last built from: the text fingerprint, the
    /// rounded pane width, the hanging-marker class and the palette.
    prose_key: Option<(u64, i32, bool, ProsePalette)>,
    /// The (text length, cursor, mode) the focus layer was last built from.
    focus_key: Option<(usize, usize, bool)>,
    /// The last typewriter offset this session asked for, so a repeated
    /// geometry read does not request the same scroll again.
    typewriter_request: Option<Point<Pixels>>,
    /// Active writing modes.
    pub modes: WritingModes,
}

impl SessionPresentation {
    pub(crate) fn new(modes: WritingModes) -> Self {
        Self {
            prose: None,
            focus: None,
            prose_key: None,
            focus_key: None,
            typewriter_request: None,
            modes,
        }
    }

    /// Rebuild the prose layer when the text, the pane width class or the
    /// theme changed; leave it alone otherwise.
    pub(crate) fn refresh_prose(
        &mut self,
        editor: &Entity<EditorState>,
        visible: &str,
        text_key: u64,
        cx: &mut App,
    ) {
        let width = f32::from(editor.read(cx).input_bounds().size.width);
        let hanging = hanging_markers(width);
        let palette = ProsePalette::current(cx);
        // Same-length replacements and pure pane resizes must rebuild too, so the
        // key carries the text content (hashed once per text change by the
        // session) and the rounded pane width, not its length.
        let key = (text_key, width.round() as i32, hanging, palette);
        if self.prose_key.as_ref() == Some(&key) {
            return;
        }
        let spans = analyze_prose(visible);
        let decorations = prose_decorations(&spans, &palette, hanging);
        self.prose = Some(match self.prose.take() {
            Some(collection) => {
                collection.set(decorations, cx);
                collection
            }
            None => editor.update(cx, |editor, cx| {
                editor.create_decorations_collection(decorations, cx)
            }),
        });
        self.prose_key = Some(key);
    }

    /// Rebuild the focus layer so only the cursor sentence stays undimmed.
    pub(crate) fn refresh_focus(
        &mut self,
        editor: &Entity<EditorState>,
        visible: &str,
        cx: &mut App,
    ) {
        let cursor = editor.read(cx).cursor().min(visible.len());
        // A caret blink notifies without moving; compare the cheap inputs first so
        // sentence detection only runs when the cursor sentence can have changed.
        let key = (visible.len(), cursor, self.modes.focus);
        if self.focus_key == Some(key) {
            return;
        }
        self.focus_key = Some(key);
        let decorations: Vec<TextDecoration> = if self.modes.focus {
            match cursor_sentence(visible, cursor) {
                Some(sentence) => other_sentences(visible, sentence.start)
                    .into_iter()
                    .map(|range| TextDecoration::new(range, dim_style()))
                    .collect(),
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        if decorations.is_empty() && self.focus.is_none() {
            return;
        }
        self.focus = Some(match self.focus.take() {
            Some(collection) => {
                collection.set(decorations, cx);
                collection
            }
            None => editor.update(cx, |editor, cx| {
                editor.create_decorations_collection(decorations, cx)
            }),
        });
    }

    /// Ask the editor to centre the cursor line, once per distinct offset.
    pub(crate) fn centre_cursor(&mut self, editor: &Entity<EditorState>, cx: &mut App) {
        if !self.modes.typewriter {
            self.typewriter_request = None;
            return;
        }
        let state = editor.read(cx);
        let (Some((cursor, line_height)), Some(text_bounds)) =
            (state.cursor_layout(), state.text_bounds())
        else {
            return;
        };
        let target =
            centered_scroll_offset(cursor, text_bounds, line_height, state.scroll_offset());
        if self.typewriter_request == Some(target) {
            return;
        }
        self.typewriter_request = Some(target);
        editor.update(cx, |editor, cx| editor.set_scroll_offset(target, cx));
    }

    /// Apply the modes to the editor; typewriter mode reserves scroll room.
    pub(crate) fn set_modes(
        &mut self,
        editor: &Entity<EditorState>,
        modes: WritingModes,
        window: &mut gpui_kit::Window,
        cx: &mut App,
    ) {
        if self.modes == modes {
            return;
        }
        if self.modes.typewriter != modes.typewriter {
            // Centring the last line needs room below it.
            let rows = modes.typewriter.then_some(1_000);
            editor.update(cx, |editor, cx| {
                editor.set_scroll_beyond_last_line(rows, window, cx)
            });
            self.typewriter_request = None;
        }
        self.modes = modes;
        // The mode change must change the focus layer even for identical text.
        self.focus_key = None;
    }

    /// Rebuild both layers after a mode change.
    /// Remove writer decorations when the editor shows complete source.
    pub(crate) fn clear_layers(&mut self, cx: &mut App) {
        if self.prose_key.is_none() && self.focus_key.is_none() {
            return;
        }
        if let Some(collection) = self.prose.as_mut() {
            collection.set(Vec::new(), cx);
        }
        if let Some(collection) = self.focus.as_mut() {
            collection.set(Vec::new(), cx);
        }
        self.prose_key = None;
        self.focus_key = None;
        self.typewriter_request = None;
    }

    pub(crate) fn invalidate(&mut self) {
        self.prose_key = None;
        self.focus_key = None;
    }

    /// The dimmed ranges currently tracked by the focus layer.
    #[cfg(test)]
    pub(crate) fn focus_ranges(&self, cx: &App) -> Vec<std::ops::Range<usize>> {
        self.focus
            .as_ref()
            .map(|collection| collection.get_ranges(cx))
            .unwrap_or_default()
    }

    /// The decorated ranges currently tracked by the prose layer.
    #[cfg(test)]
    pub(crate) fn prose_ranges(&self, cx: &App) -> Vec<std::ops::Range<usize>> {
        self.prose
            .as_ref()
            .map(|collection| collection.get_ranges(cx))
            .unwrap_or_default()
    }
}

/// Dimming style for everything but the cursor sentence.
fn dim_style() -> HighlightStyle {
    HighlightStyle {
        fade_out: Some(0.65),
        ..HighlightStyle::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Bounds, point, px, size};

    fn viewport(height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(100.)),
            size: size(px(600.), px(height)),
        }
    }

    fn cursor(y: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(10.), px(y)),
            size: size(px(2.), px(20.)),
        }
    }

    #[test]
    fn typewriter_offset_centres_the_cursor_line() {
        // Caret 400 px into the content, 400 px viewport, 20 px lines:
        // the caret line's top lands 190 px below the viewport top.
        let target =
            centered_scroll_offset(cursor(500.), viewport(400.), px(20.), point(px(0.), px(0.)));
        assert_eq!(f32::from(target.y), 210.);
        assert_eq!(f32::from(target.x), 0.);
    }

    #[test]
    fn typewriter_offset_accounts_for_the_current_scroll() {
        // Already scrolled by 100 px, so the caret is 500 px into the content.
        let target = centered_scroll_offset(
            cursor(500.),
            viewport(400.),
            px(20.),
            point(px(0.), px(100.)),
        );
        assert_eq!(f32::from(target.y), 310.);
    }

    #[test]
    fn typewriter_offset_may_be_negative_near_the_first_lines() {
        // The editor clamps the deferred offset; the formula still asks for the
        // centre so the first lines can sit in the middle of a tall viewport.
        let target =
            centered_scroll_offset(cursor(120.), viewport(400.), px(20.), point(px(0.), px(0.)));
        assert_eq!(f32::from(target.y), -170.);
    }

    #[test]
    fn prose_style_keeps_configured_values_and_clamps_corrupt_ones() {
        let style = ProseStyle::new("Inter".into(), 16., 2., 768.);
        assert_eq!(style.font_size, 16.);
        assert_eq!(style.line_height, 2.);
        assert_eq!(style.measure, 768.);

        let clamped = ProseStyle::new("Inter".into(), 0., 9., 10_000.);
        assert_eq!(clamped.font_size, 1.);
        assert_eq!(clamped.line_height, 3.);
        assert_eq!(clamped.measure, 1200.);
    }
}
