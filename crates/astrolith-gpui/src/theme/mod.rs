//! Application appearance policy and the pinned Kit adapter.
mod dark;
mod light;
mod tokens;

use astrolith_core::preferences::HeadingColors;
use gpui_kit::{
    App, Global, Hsla, WindowAppearance,
    component::{ActiveTheme, Theme},
    rgb, rgba,
};
pub use tokens::ThemeTokens;

/// Heading colour for the resolved theme, published for prose rendering.
///
/// The preference stores `#rrggbb` strings; this global carries the colour that
/// actually applies after parsing and theme resolution, so the editor never
/// re-reads the preference file and an invalid value falls back once here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProseHeadingColor(pub Hsla);

impl Global for ProseHeadingColor {}

/// The active heading colour, falling back to the theme foreground.
pub(crate) fn prose_heading_color(cx: &App) -> Hsla {
    cx.try_global::<ProseHeadingColor>()
        .map(|color| color.0)
        .unwrap_or_else(|| cx.theme().foreground)
}

/// Parse a persisted `#rrggbb` or `#rrggbbaa` colour into a theme colour.
///
/// Preference files store colours as strings so the core model needs no GPUI
/// dependency. Validation happens at this presentation boundary; an invalid
/// value is rejected and the caller keeps its own fallback.
pub(crate) fn color_from_hex(value: &str) -> Option<Hsla> {
    let digits = value.trim().strip_prefix('#')?;
    let channel = u32::from_str_radix(digits, 16).ok()?;
    match digits.len() {
        6 => Some(rgb(channel).into()),
        8 => Some(rgba(channel).into()),
        _ => None,
    }
}

/// User appearance preference; System remains distinct from its resolved mode.
pub use astrolith_core::preferences::ThemePreference as ThemeMode;

/// Resolve a preference against the native appearance.
pub(crate) trait ResolveTheme {
    fn resolve(self, appearance: WindowAppearance) -> Self;
}

impl ResolveTheme for ThemeMode {
    fn resolve(self, appearance: WindowAppearance) -> Self {
        match self {
            Self::System => match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            },
            mode => mode,
        }
    }
}

/// Return a complete application palette. Callers resolve System first.
pub fn palette(mode: ThemeMode) -> ThemeTokens {
    match mode {
        ThemeMode::Dark => dark::tokens(),
        ThemeMode::Light | ThemeMode::System => light::tokens(),
    }
}

/// Apply colors through Kit's synchronized Component/Base theme update path.
///
/// Heading colours belong to the resolved theme, so the published prose
/// heading is refreshed in the same pass that refreshes every window.
pub fn apply(
    preference: ThemeMode,
    appearance: WindowAppearance,
    headings: &HeadingColors,
    cx: &mut App,
) {
    let mode = preference.resolve(appearance);
    let kit_mode = match mode {
        ThemeMode::Dark => gpui_kit::component::ThemeMode::Dark,
        ThemeMode::Light | ThemeMode::System => gpui_kit::component::ThemeMode::Light,
    };
    Theme::change(kit_mode, None, cx);
    let tokens = palette(mode);
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        colors.background = tokens.background;
        colors.foreground = tokens.foreground;
        colors.border = tokens.border;
        colors.window_border = tokens.border_variant;
        colors.selection = tokens.selection;
        colors.caret = tokens.caret;
        colors.ring = tokens.accent;
        colors.input = tokens.input;

        colors.sidebar = tokens.surface;
        colors.sidebar_foreground = tokens.foreground;
        colors.sidebar_border = tokens.border_variant;
        colors.sidebar_accent = tokens.hover;
        colors.sidebar_accent_foreground = tokens.foreground;

        colors.title_bar = tokens.surface;
        colors.title_bar_border = tokens.border_variant;
        colors.status_bar = tokens.status_bar;
        colors.status_bar_border = tokens.border_variant;

        colors.tab_bar = tokens.tab_bar;
        colors.tab = tokens.tab_bar;
        colors.tab_active = tokens.tab_active;
        colors.tab_active_foreground = tokens.tab_active_foreground;
        colors.tab_foreground = tokens.tab_foreground;

        colors.list = tokens.background;
        colors.list_hover = tokens.hover;
        colors.list_active = tokens.selection;
        colors.list_active_border = tokens.accent;
        colors.list_head = tokens.surface;
        colors.list_even = tokens.background;

        colors.muted = tokens.hover;
        colors.muted_foreground = tokens.muted;
        colors.accent = tokens.accent;
        colors.accent_foreground = tokens.accent_foreground;
        colors.popover = tokens.popover;
        colors.popover_foreground = tokens.popover_foreground;

        colors.primary = tokens.accent;
        colors.primary_foreground = tokens.accent_foreground;
        colors.primary_hover = tokens.accent;
        colors.primary_active = tokens.accent;
        colors.button = tokens.elevated;
        colors.button_hover = tokens.hover;
        colors.button_active = tokens.active;
        colors.button_foreground = tokens.foreground;

        colors.scrollbar = tokens.scrollbar;
        colors.scrollbar_thumb = tokens.scrollbar_thumb;
        colors.scrollbar_thumb_hover = tokens.border;
        colors.group_box = tokens.surface;
        colors.group_box_foreground = tokens.foreground;
        colors.link = tokens.link;
        colors.danger = tokens.danger;
        colors.warning = tokens.warning;
        colors.success = tokens.success;
        colors.info = tokens.info;

        // Zed's chrome is compact: a 14 px UI scale, 13 px code, 6 px radii.
        theme.font_size = gpui_kit::px(14.);
        theme.mono_font_size = gpui_kit::px(13.);
        theme.radius = gpui_kit::px(6.);
        theme.radius_lg = gpui_kit::px(8.);
        theme.shadow = true;
    });
    let configured = match mode {
        ThemeMode::Dark => headings.dark.as_deref(),
        ThemeMode::Light | ThemeMode::System => headings.light.as_deref(),
    };
    let heading = configured
        .and_then(color_from_hex)
        .unwrap_or_else(|| cx.theme().foreground);
    cx.set_global(ProseHeadingColor(heading));
}

#[cfg(test)]
mod tests;
