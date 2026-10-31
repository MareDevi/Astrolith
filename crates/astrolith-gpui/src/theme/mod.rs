//! Application appearance policy and the pinned Kit adapter.
mod dark;
mod light;
mod tokens;

use gpui_kit::{App, WindowAppearance, component::Theme};
pub use tokens::ThemeTokens;

/// User appearance preference; System remains distinct from its resolved mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemeMode {
    /// Resolve all native appearance variants without changing the preference.
    pub fn resolve(self, appearance: WindowAppearance) -> Self {
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
pub fn apply(preference: ThemeMode, appearance: WindowAppearance, cx: &mut App) {
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
}

#[cfg(test)]
mod tests;
