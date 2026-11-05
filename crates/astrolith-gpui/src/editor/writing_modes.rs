//! Preferences consumed by the visible writing surface.

use gpui_kit::SharedString;

/// Which writing modes are active for the whole writer surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WritingModes {
    /// Dim every sentence except the cursor sentence.
    pub focus: bool,
    /// Keep the cursor line vertically centred.
    pub typewriter: bool,
}

/// Resolved document presentation: face, size, line spacing and page width.
///
/// The values come from preferences and are used as written; the pane no
/// longer scales type by its width, so an explicit size or line height means
/// the same thing in every window. The page width is a percentage of the
/// editor pane, clamped so extremely narrow or wide panes stay readable.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProseStyle {
    /// Document face.
    pub font_family: SharedString,
    /// Font size in logical pixels.
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Prose column width as a percentage of the editor pane.
    pub page_width_percent: f32,
}

impl ProseStyle {
    pub(crate) fn new(
        font_family: SharedString,
        font_size: f32,
        line_height: f32,
        page_width_percent: f32,
    ) -> Self {
        Self {
            font_family,
            font_size: font_size.clamp(1., 30.),
            line_height: line_height.clamp(1.2, 3.),
            page_width_percent: page_width_percent.clamp(30., 100.),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prose_style_keeps_configured_values_and_clamps_corrupt_ones() {
        let style = ProseStyle::new("Inter".into(), 16., 2., 64.);
        assert_eq!(style.font_size, 16.);
        assert_eq!(style.line_height, 2.);
        assert_eq!(style.page_width_percent, 64.);

        let clamped = ProseStyle::new("Inter".into(), 0., 9., 10_000.);
        assert_eq!(clamped.font_size, 1.);
        assert_eq!(clamped.line_height, 3.);
        assert_eq!(clamped.page_width_percent, 100.);
    }
}
