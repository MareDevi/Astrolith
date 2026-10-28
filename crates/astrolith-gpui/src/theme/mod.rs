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
        theme.colors.background = tokens.background;
        theme.colors.foreground = tokens.foreground;
        theme.colors.sidebar = tokens.surface;
        theme.colors.sidebar_foreground = tokens.foreground;
        theme.colors.muted_foreground = tokens.muted;
        theme.colors.border = tokens.border;
        theme.colors.ring = tokens.accent;
        theme.colors.tab_bar = tokens.surface;
        theme.colors.tab = tokens.surface;
        theme.colors.tab_active = tokens.background;
        theme.colors.tab_active_foreground = tokens.foreground;
        theme.colors.title_bar = tokens.surface;
        theme.radius = gpui_kit::px(4.);
    });
}

#[cfg(test)]
mod tests;
