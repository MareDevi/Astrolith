/// Preference snapshot; live dock state owns geometry while the window is open.
pub(crate) use astrolith_core::preferences::PanelLayout as LayoutState;

#[derive(Clone, Copy)]
pub(crate) enum PanelKind {
    Sidebar,
    Inspector,
    Preview,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restored_widths_leave_room_for_document_and_reject_nonfinite_values() {
        let mut layout = LayoutState {
            sidebar_width: f32::INFINITY,
            ..Default::default()
        };
        layout.clamp(800.);
        assert_eq!(layout.sidebar_width, 240.);
        layout.sidebar_width = 10000.;
        layout.clamp(800.);
        assert_eq!(layout.sidebar_width, 240.);
        layout.preview_height = f32::NAN;
        layout.clamp(800.);
        assert_eq!(layout.preview_height, 160.);
        layout.preview_height = 10.;
        layout.clamp(800.);
        assert_eq!(layout.preview_height, 120.);
    }

    #[test]
    fn panel_visibility_flags_are_independent_and_widths_survive() {
        let mut layout = LayoutState {
            sidebar_visible: false,
            ..LayoutState::default()
        };
        assert!(!layout.sidebar_visible);
        assert!(!layout.preview_visible);
        assert_eq!(layout.sidebar_width, 240.);
        layout.preview_visible = true;
        layout.sidebar_visible = true;
        assert!(layout.preview_visible);
        assert!(layout.sidebar_visible);
        assert_eq!(layout.sidebar_width, 240.);
    }
}
