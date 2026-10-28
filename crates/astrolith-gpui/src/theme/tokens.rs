use gpui_kit::Hsla;

/// Semantic roles owned by Astrolith; Kit supplies interaction/status roles.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ThemeTokens {
    pub background: Hsla,
    pub foreground: Hsla,
    pub surface: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
}
