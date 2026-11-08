//! Application appearance policy and the pinned Kit adapter.
mod metrics;
mod palettes;
mod tokens;
mod typography;

pub(crate) use metrics::METRICS;
pub(crate) use typography::{
    PROSE_INSET, PROSE_TOP, configured_prose_font, default_prose_page_width_percent, display_font,
    font_family_available, installed_font_names, preferred_ui_font, prose_font, system_ui_font,
};

use astrolith_core::preferences::{ColorScheme, GlobalPreferencesV2};
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

/// Return the named palette selected in global preferences.
pub fn palette(_mode: ThemeMode, scheme: ColorScheme) -> ThemeTokens {
    palettes::named(scheme)
}

/// Apply colors through Kit's synchronized Component/Base theme update path.
///
/// Heading colours belong to the resolved theme, so the published prose
/// heading is refreshed in the same pass that refreshes every window.
pub fn apply(settings: &GlobalPreferencesV2, appearance: WindowAppearance, cx: &mut App) {
    let mode = settings.theme.resolve(appearance);
    let fallback_font = system_ui_font(cx);
    let requested_font = settings.ui_font_family.trim();
    let ui_font = if requested_font.is_empty() {
        preferred_ui_font(cx)
    } else if !typography::font_family_available(cx, requested_font) {
        tracing::warn!(
            family = requested_font,
            "interface font is not installed; using the system UI font"
        );
        fallback_font
    } else {
        requested_font.to_owned().into()
    };
    let ui_font_size = gpui_kit::px(settings.ui_font_size.clamp(8., 28.));
    let system_mono_font = typography::system_mono_font(cx);
    let requested_mono = settings.build_font_family.trim();
    let mono_font = if requested_mono.is_empty() {
        system_mono_font.clone()
    } else if typography::font_family_available(cx, requested_mono) {
        requested_mono.to_owned().into()
    } else {
        tracing::warn!(
            family = requested_mono,
            "Build font is not installed; using the system monospace font"
        );
        system_mono_font
    };
    let kit_mode = match mode {
        ThemeMode::Dark => gpui_kit::component::ThemeMode::Dark,
        ThemeMode::Light | ThemeMode::System => gpui_kit::component::ThemeMode::Light,
    };
    Theme::change(kit_mode, None, cx);
    let tokens = palette(mode, settings.color_scheme);
    Theme::update(cx, |theme| {
        let colors = &mut theme.colors;
        colors.background = tokens.background;
        colors.foreground = tokens.foreground;
        colors.border = tokens.border;
        colors.window_border = tokens.border_variant;
        colors.selection = tokens.selection;
        colors.caret = tokens.caret;
        colors.ring = tokens.focus;
        colors.input = match mode {
            ThemeMode::Dark => tokens.control,
            ThemeMode::Light | ThemeMode::System => tokens.border,
        };

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
        colors.list_active_border = tokens.focus;
        colors.list_head = tokens.surface;
        colors.list_even = tokens.background;

        colors.muted = tokens.hover;
        colors.muted_foreground = tokens.muted;
        colors.accent = tokens.hover;
        colors.accent_foreground = tokens.foreground;
        colors.popover = tokens.popover;
        colors.popover_foreground = tokens.popover_foreground;

        colors.primary = tokens.primary;
        colors.primary_foreground = tokens.primary_foreground;
        colors.primary_hover = tokens.primary.opacity(0.8);
        colors.primary_active = tokens.primary.opacity(0.7);
        colors.button = tokens.control;
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

        colors.secondary = tokens.control;
        colors.secondary_hover = tokens.foreground.opacity(0.05).blend(tokens.control);
        colors.secondary_active = tokens.active;
        colors.secondary_foreground = tokens.foreground;
        colors.link_hover = tokens.primary.opacity(0.8);
        colors.link_active = tokens.primary.opacity(0.7);
        colors.overlay = tokens.overlay;
        colors.progress_bar = tokens.primary;
        colors.switch = tokens.primary;
        colors.switch_thumb = tokens.background;
        colors.skeleton = tokens.hover;
        colors.slider_bar = tokens.border;
        colors.slider_thumb = tokens.primary;
        colors.accordion = tokens.background;
        colors.drag_border = tokens.focus;
        colors.drop_target = tokens.selection;
        colors.description_list_label = tokens.surface;
        colors.description_list_label_foreground = tokens.muted;
        colors.tab_bar_segmented = tokens.control;

        colors.table = tokens.background;
        colors.table_head = tokens.surface;
        colors.table_head_foreground = tokens.muted;
        colors.table_hover = tokens.hover;
        colors.table_active = tokens.selection;
        colors.table_active_border = tokens.focus;
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

        // The mock's two stored radii: controls use `--radius` (10 px), while
        // popovers and dialogs use the larger surface tier (14 px).
        theme.font_family = ui_font;
        theme.font_size = ui_font_size;
        theme.mono_font_family = mono_font;
        theme.mono_font_size = gpui_kit::px(settings.build_font_size.clamp(1., 30.));
        let panel_radius = gpui_kit::px(settings.panel_radius.clamp(0., 24.));
        theme.radius = panel_radius;
        theme.radius_lg = panel_radius;
        theme.shadow = true;
    });
    let configured = match mode {
        ThemeMode::Dark => settings.heading_colors.dark.as_deref(),
        ThemeMode::Light | ThemeMode::System => settings.heading_colors.light.as_deref(),
    };
    let heading = configured
        .and_then(color_from_hex)
        .unwrap_or_else(|| cx.theme().foreground);
    cx.set_global(ProseHeadingColor(heading));
}

#[cfg(test)]
mod tests;
