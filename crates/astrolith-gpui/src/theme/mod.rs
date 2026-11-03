//! Application appearance policy and the pinned Kit adapter.
mod palettes;
mod tokens;
mod typography;

pub(crate) use typography::{PROSE_INSET, PROSE_MEASURE, PROSE_TOP, prose_font};

use astrolith_core::preferences::{DarkColorScheme, HeadingColors, LightColorScheme};
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
pub fn palette(mode: ThemeMode, light: LightColorScheme, dark: DarkColorScheme) -> ThemeTokens {
    match mode {
        ThemeMode::Dark => palettes::dark(dark),
        ThemeMode::Light | ThemeMode::System => palettes::light(light),
    }
}

/// Apply colors through Kit's synchronized Component/Base theme update path.
///
/// Heading colours belong to the resolved theme, so the published prose
/// heading is refreshed in the same pass that refreshes every window.
pub fn apply(
    preference: ThemeMode,
    appearance: WindowAppearance,
    light: LightColorScheme,
    dark: DarkColorScheme,
    headings: &HeadingColors,
    cx: &mut App,
) {
    let mode = preference.resolve(appearance);
    let kit_mode = match mode {
        ThemeMode::Dark => gpui_kit::component::ThemeMode::Dark,
        ThemeMode::Light | ThemeMode::System => gpui_kit::component::ThemeMode::Light,
    };
    Theme::change(kit_mode, None, cx);
    let tokens = palette(mode, light, dark);
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
        colors.scrollbar_thumb_hover = tokens.muted;
        colors.group_box = tokens.surface;
        colors.group_box_foreground = tokens.foreground;
        colors.link = tokens.link;
        colors.danger = tokens.danger;
        colors.danger_hover = tokens.danger;
        colors.danger_active = tokens.danger;
        colors.danger_foreground = tokens.background;
        colors.warning = tokens.warning;
        colors.warning_hover = tokens.warning;
        colors.warning_active = tokens.warning;
        colors.warning_foreground = tokens.background;
        colors.success = tokens.success;
        colors.success_hover = tokens.success;
        colors.success_active = tokens.success;
        colors.success_foreground = tokens.background;
        colors.info = tokens.info;
        colors.info_hover = tokens.info;
        colors.info_active = tokens.info;
        colors.info_foreground = tokens.background;

        colors.secondary = tokens.elevated;
        colors.secondary_hover = tokens.hover;
        colors.secondary_active = tokens.active;
        colors.secondary_foreground = tokens.foreground;
        colors.link_hover = tokens.accent;
        colors.link_active = tokens.accent;
        colors.overlay = Hsla::black().opacity(0.28);
        colors.progress_bar = tokens.accent;
        colors.switch = tokens.accent;
        colors.switch_thumb = tokens.background;
        colors.skeleton = tokens.hover;
        colors.slider_bar = tokens.border;
        colors.slider_thumb = tokens.accent;
        colors.accordion = tokens.background;
        colors.drag_border = tokens.accent;
        colors.drop_target = tokens.selection;
        colors.description_list_label = tokens.surface;
        colors.description_list_label_foreground = tokens.muted;
        colors.tab_bar_segmented = tokens.surface;

        colors.table = tokens.background;
        colors.table_head = tokens.surface;
        colors.table_head_foreground = tokens.muted;
        colors.table_hover = tokens.hover;
        colors.table_active = tokens.selection;
        colors.table_active_border = tokens.accent;
        colors.table_even = tokens.background;
        colors.table_row_border = tokens.border_variant;
        colors.table_foot = tokens.surface;
        colors.table_foot_foreground = tokens.muted;

        // The document is a page, not a code buffer: the editor shares the
        // page colour, hides the active-line band and keeps the gutter quiet.
        let highlight = std::sync::Arc::make_mut(&mut theme.highlight_theme);
        highlight.style.editor_background = Some(tokens.background);
        highlight.style.editor_active_line = Some(tokens.background);
        highlight.style.editor_gutter_background = Some(tokens.background);

        // A calm desktop scale: controls stay compact, while paper, panels and
        // overlays share one restrained, lightly rounded silhouette.
        theme.font_size = gpui_kit::px(14.);
        theme.mono_font_size = gpui_kit::px(13.);
        theme.radius = gpui_kit::px(6.);
        theme.radius_lg = gpui_kit::px(10.);
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
