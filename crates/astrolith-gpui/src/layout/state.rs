use serde::{Deserialize, Serialize};

#[derive(Clone, Copy)]
pub(crate) enum PanelKind {
    Sidebar,
    Inspector,
    Preview,
}

/// Preference snapshot; live dock state owns geometry while the window is open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LayoutState {
    pub sidebar_visible: bool,
    pub inspector_visible: bool,
    pub preview_visible: bool,
    pub sidebar_width: f32,
    pub inspector_width: f32,
}

impl Default for LayoutState {
    fn default() -> Self {
        Self {
            sidebar_visible: true,
            inspector_visible: true,
            preview_visible: false,
            sidebar_width: 240.,
            inspector_width: 260.,
        }
    }
}

impl LayoutState {
    /// Stored logical pixels cross the native Dock API boundary here.
    pub fn clamp(&mut self, viewport_width: f32) {
        let maximum = ((viewport_width - 320.) / 2.).clamp(160., 400.);
        for width in [&mut self.sidebar_width, &mut self.inspector_width] {
            if !width.is_finite() {
                *width = 240.;
            }
            *width = width.clamp(160., maximum);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_widths_leave_room_for_document_and_reject_nonfinite_values() {
        let mut layout = LayoutState {
            sidebar_width: f32::INFINITY,
            inspector_width: -20.,
            ..Default::default()
        };
        layout.clamp(800.);
        assert_eq!(layout.sidebar_width, 240.);
        assert_eq!(layout.inspector_width, 160.);
        layout.sidebar_width = 10000.;
        layout.inspector_width = 10000.;
        layout.clamp(800.);
        assert!(layout.sidebar_width + layout.inspector_width <= 480.);
    }

    #[test]
    fn panel_visibility_flags_are_independent_and_widths_survive() {
        let mut layout = LayoutState {
            sidebar_visible: false,
            ..LayoutState::default()
        };
        assert!(!layout.sidebar_visible);
        assert!(layout.inspector_visible);
        assert!(!layout.preview_visible);
        assert_eq!(layout.sidebar_width, 240.);
        layout.preview_visible = true;
        layout.inspector_visible = false;
        assert!(layout.preview_visible);
        assert!(!layout.inspector_visible);
    }
}
