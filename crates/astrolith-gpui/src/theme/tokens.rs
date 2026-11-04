use gpui_kit::Hsla;

/// Semantic roles owned by Astrolith; Kit supplies interaction/status roles.
///
/// The application uses one neutral light and one neutral dark palette. Roles
/// are named for what they mean, not for the mock's CSS custom properties:
/// notably `muted` is secondary *text*, while [`Self::control`] and
/// [`Self::surface`] carry the quiet *fills* the mock calls `muted` and
/// `sidebar`. Components read roles, never raw palette values.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ThemeTokens {
    /// Editor and primary content surface.
    pub background: Hsla,
    /// Primary text.
    pub foreground: Hsla,
    /// Navigation, dock and grouped-panel surface.
    pub surface: Hsla,
    /// Card and raised-element surface.
    pub elevated: Hsla,
    /// Quiet control fill: input wells, secondary buttons, muted chips.
    pub control: Hsla,
    /// Secondary text and inactive icons.
    pub muted: Hsla,
    /// Visible structural boundary.
    pub border: Hsla,
    /// Quiet boundary between adjacent surfaces.
    pub border_variant: Hsla,
    /// Primary action fill; neutral, never a brand colour.
    pub primary: Hsla,
    /// Text and icon colour on top of [`Self::primary`].
    pub primary_foreground: Hsla,
    /// Keyboard focus ring and focused-control border.
    pub focus: Hsla,
    /// Selected row, tab or segmented-item fill.
    pub selection: Hsla,
    /// Hover fill for rows, tabs and quiet buttons.
    pub hover: Hsla,
    /// Pressed/active fill.
    pub active: Hsla,
    /// Bottom status bar surface.
    pub status_bar: Hsla,
    /// Dock tab strip surface.
    pub tab_bar: Hsla,
    /// Active tab surface; equals the editor surface.
    pub tab_active: Hsla,
    pub tab_active_foreground: Hsla,
    pub tab_foreground: Hsla,
    /// Dialog, menu and popover surface.
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    /// Caret colour.
    pub caret: Hsla,
    /// Scrollbar track and thumb.
    pub scrollbar: Hsla,
    pub scrollbar_thumb: Hsla,
    /// Modal backdrop over the window canvas.
    pub overlay: Hsla,
    // Status roles: only for real state, never for navigation.
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub info: Hsla,
    /// Inline links; follows the neutral primary action.
    pub link: Hsla,
}
