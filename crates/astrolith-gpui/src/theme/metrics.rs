//! Application-owned scales for spacing, type, radii and elevation.
//!
//! The mock is a CSS layout at a 16 px root; these are native logical pixels,
//! not a web rem. They are design tokens, so controls and layouts consume the
//! named scale instead of scattering one-off numbers.
//!
//! Kit 0.8.1 stores only two radii (`radius`, `radius_lg`) and no spacing
//! scale, so the remaining tiers live here and are applied to Astrolith-owned
//! elements. Kit components inherit the two radii through [`ThemeMetrics::radius`]
//! and [`ThemeMetrics::surface_radius`].

/// Semantic spacing steps in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SpaceScale {
    /// 2 px: hairline separation inside one control.
    pub xxs: u32,
    /// 4 px: icon/text gap in compact chrome.
    pub xs: u32,
    /// 6 px: control-local complement.
    pub sm: u32,
    /// 8 px: toolbar and list rhythm.
    pub md: u32,
    /// 12 px: panel padding.
    pub lg: u32,
    /// 16 px: section inset.
    pub xl: u32,
    /// 20 px: control-local complement.
    pub xxl: u32,
    /// 24 px: group gap.
    pub section: u32,
    /// 32 px: panel inset / prose inset.
    pub gutter: u32,
    /// 48 px: writing-space step.
    pub writing: u32,
    /// 64 px: prose top.
    pub page: u32,
    /// 96 px: wide-window prose top.
    pub canvas: u32,
}

/// Type sizes and line heights in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TypeScale {
    /// 12 px metadata, labels and captions; never used for required prose.
    pub caption: f32,
    /// 24 px second-level heading.
    pub heading: f32,
    /// 36 px document title at the narrow breakpoint.
    pub title: f32,
    /// 48 px document title on wide windows.
    pub display: f32,
    /// 13 px monospace base for code and terminal.
    pub mono: f32,
}

/// One shadow layer: CSS box-shadow geometry with the mock's ink alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ShadowLayer {
    /// Vertical offset.
    pub offset_y: f32,
    /// Gaussian blur radius.
    pub blur: f32,
    /// Spread radius.
    pub spread: f32,
    /// Ink alpha, applied over black.
    pub alpha: f32,
}

/// Elevation tiers. Only floating surfaces use a shadow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ElevationScale {
    /// Menu, popover and selection toolbar: the mock's `shadow-xl`.
    pub overlay: [ShadowLayer; 2],
}

/// The complete design-token snapshot projected at startup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ThemeMetrics {
    pub space: SpaceScale,
    pub type_scale: TypeScale,
    pub elevation: ElevationScale,
}

/// The single source for spacing, type, radius and elevation.
pub(crate) const METRICS: ThemeMetrics = ThemeMetrics {
    space: SpaceScale {
        xxs: 2,
        xs: 4,
        sm: 6,
        md: 8,
        lg: 12,
        xl: 16,
        xxl: 20,
        section: 24,
        gutter: 32,
        writing: 48,
        page: 64,
        canvas: 96,
    },
    type_scale: TypeScale {
        caption: 12.,
        heading: 24.,
        title: 36.,
        display: 48.,
        mono: 13.,
    },
    elevation: ElevationScale {
        overlay: [
            ShadowLayer {
                offset_y: 10.,
                blur: 15.,
                spread: -3.,
                alpha: 0.1,
            },
            ShadowLayer {
                offset_y: 4.,
                blur: 6.,
                spread: -4.,
                alpha: 0.1,
            },
        ],
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_match_the_mock_snapshot() {
        assert_eq!(
            [
                METRICS.space.xxs,
                METRICS.space.xs,
                METRICS.space.sm,
                METRICS.space.md,
                METRICS.space.lg,
                METRICS.space.xl,
                METRICS.space.xxl,
                METRICS.space.section,
                METRICS.space.gutter,
                METRICS.space.writing,
                METRICS.space.page,
                METRICS.space.canvas,
            ],
            [2, 4, 6, 8, 12, 16, 20, 24, 32, 48, 64, 96]
        );
        assert_eq!(METRICS.type_scale.caption, 12.);
        assert_eq!(METRICS.type_scale.title, 36.);
        assert_eq!(METRICS.type_scale.display, 48.);
        assert_eq!(METRICS.elevation.overlay[0].blur, 15.);
    }
}
