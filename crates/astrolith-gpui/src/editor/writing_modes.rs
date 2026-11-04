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
use gpui_kit::{App, Bounds, Entity, HighlightStyle, Pixels, Point, px};

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

/// Prose size and line spacing for one pane width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProseTypography {
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
}

/// Narrowest prose size factor: a narrow pane shrinks the text.
const NARROW: f32 = 0.88;
/// Widest prose size factor: a wide pane grows the text.
const WIDE: f32 = 1.12;

/// Scale prose typography with the document pane's width.
///
/// A narrow pane trades size for looser lines, a wide pane does the opposite,
/// and both stay inside a readable band around the configured base size.
pub(crate) fn prose_typography(pane_width: f32, base_font_size: f32) -> ProseTypography {
    let base = base_font_size.clamp(12., 32.);
    let (factor, ratio) = if pane_width <= 640. {
        (NARROW, 1.85)
    } else if pane_width <= 960. {
        let t = (pane_width - 640.) / 320.;
        (NARROW + (1. - NARROW) * t, 1.85 - 0.1 * t)
    } else if pane_width <= 1400. {
        let t = (pane_width - 960.) / 440.;
        (1. + (WIDE - 1.) * t, 1.75 - 0.1 * t)
    } else {
        (WIDE, 1.65)
    };
    ProseTypography {
        font_size: ((base * factor) * 2.).round() / 2.,
        line_height: (ratio * 100.).round() / 100.,
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
    fn prose_typography_grows_with_the_pane_and_tightens_its_lines() {
        let narrow = prose_typography(480., 18.);
        let middle = prose_typography(800., 18.);
        let wide = prose_typography(1600., 18.);
        assert!(narrow.font_size < middle.font_size);
        assert!(middle.font_size < wide.font_size);
        assert!(narrow.line_height > middle.line_height);
        assert!(middle.line_height > wide.line_height);
        assert_eq!(wide.font_size, 20.);
        assert_eq!(narrow.font_size, 16.);
    }

    #[test]
    fn prose_typography_clamps_the_configured_base_size() {
        assert_eq!(prose_typography(1200., 2.).font_size, 13.);
        assert_eq!(prose_typography(1200., 400.).font_size, 34.);
    }
}
