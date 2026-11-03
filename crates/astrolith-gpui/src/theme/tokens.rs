use gpui_kit::Hsla;

/// Semantic roles owned by Astrolith; Kit supplies interaction/status roles.
///
/// Every selectable colour scheme fills these roles: a flat editor surface,
/// slightly recessed panel chrome, hairline borders and one accent. Components
/// read roles, never raw palette values.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ThemeTokens {
    /// Editor and primary content surface.
    pub background: Hsla,
    /// Primary text.
    pub foreground: Hsla,
    /// Panel, dock and toolbar surface.
    pub surface: Hsla,
    /// Raised element surface (hovered rows, popovers, inputs).
    pub elevated: Hsla,
    /// Secondary text and inactive icons.
    pub muted: Hsla,
    /// Visible structural boundary.
    pub border: Hsla,
    /// Quiet boundary between adjacent surfaces.
    pub border_variant: Hsla,
    /// Focus, selection emphasis and the primary action.
    pub accent: Hsla,
    /// Text/icon color on top of `accent`.
    pub accent_foreground: Hsla,
    /// Selected row or tab fill.
    pub selection: Hsla,
    /// Hover fill for rows, tabs and quiet buttons.
    pub hover: Hsla,
    /// Pressed/active fill.
    pub active: Hsla,
    /// Bottom status bar surface.
    pub status_bar: Hsla,
    /// Dock tab strip surface.
    pub tab_bar: Hsla,
    /// Active tab surface; equals the editor surface in Zed.
    pub tab_active: Hsla,
    pub tab_active_foreground: Hsla,
    pub tab_foreground: Hsla,
    /// Dialog, menu and popover surface.
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    /// Text input surface.
    pub input: Hsla,
    /// Caret and focus ring.
    pub caret: Hsla,
    /// Scrollbar track and thumb.
    pub scrollbar: Hsla,
    pub scrollbar_thumb: Hsla,
    // Status roles.
    pub danger: Hsla,
    pub warning: Hsla,
    pub success: Hsla,
    pub info: Hsla,
    pub link: Hsla,
}
