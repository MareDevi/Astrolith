use super::*;
use gpui_kit::{
    Hsla, Rgba, SharedString, TestAppContext, WindowAppearance, component::ActiveTheme, px,
};

#[test]
fn persisted_hex_colors_round_trip_through_the_theme_boundary() {
    let opaque = color_from_hex("#4ec9b0").expect("six-digit colour");
    assert_eq!(opaque.a, 1.0);
    let translucent = color_from_hex(" #4ec9b080 ").expect("eight-digit colour");
    assert!(translucent.a > 0.4 && translucent.a < 0.6);
    assert!(color_from_hex("#abc").is_none());
    assert!(color_from_hex("4ec9b0").is_none());
    assert!(color_from_hex("#gggggg").is_none());
}

#[test]
fn system_mode_resolves_each_platform_appearance_without_overriding_explicit_mode() {
    for (appearance, expected) in [
        (WindowAppearance::Light, ThemeMode::Light),
        (WindowAppearance::VibrantLight, ThemeMode::Light),
        (WindowAppearance::Dark, ThemeMode::Dark),
        (WindowAppearance::VibrantDark, ThemeMode::Dark),
    ] {
        assert_eq!(ThemeMode::System.resolve(appearance), expected);
        assert_eq!(ThemeMode::Dark.resolve(appearance), ThemeMode::Dark);
        assert_eq!(ThemeMode::Light.resolve(appearance), ThemeMode::Light);
    }
}

#[gpui_kit::test]
fn explicit_interface_typography_overrides_the_theme_face_and_scale(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let system = system_ui_font(cx);
        let custom = cx
            .text_system()
            .all_font_names()
            .into_iter()
            .find(|family| family.as_str() != system.as_ref())
            .unwrap_or_else(|| "Example Sans".to_owned());
        let mut settings = GlobalPreferencesV2 {
            theme: ThemeMode::Light,
            ui_font_family: custom.clone(),
            ui_font_size: 17.,
            ..GlobalPreferencesV2::default()
        };
        assert_eq!(
            configured_prose_font(cx, ""),
            prose_font(cx),
            "an empty document family uses the mock's sans face"
        );
        assert_eq!(
            configured_prose_font(cx, &custom),
            SharedString::from(custom.clone())
        );
        apply(&settings, WindowAppearance::Light, cx);
        assert_eq!(cx.theme().font_family, SharedString::from(custom.clone()));
        assert_eq!(cx.theme().font_size, px(17.));
        assert_eq!(
            display_font(cx),
            SharedString::from(custom),
            "an explicit interface family also sets editorial headings"
        );

        settings.ui_font_family.clear();
        settings.ui_font_size = 14.;
        apply(&settings, WindowAppearance::Light, cx);
        assert_eq!(cx.theme().font_size, px(14.));
        assert_eq!(
            display_font(cx),
            prose_font(cx),
            "the system UI face restores the literary display pair"
        );
    });
}

fn luminance(color: Hsla) -> f32 {
    let color: Rgba = color.into();
    let linear = |channel: f32| {
        if channel <= 0.040_45 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(first: Hsla, second: Hsla) -> f32 {
    let (lighter, darker) = {
        let (a, b) = (luminance(first), luminance(second));
        if a > b { (a, b) } else { (b, a) }
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[test]
fn catppuccin_mocha_roles_keep_text_contrast_and_distinct_selection_states() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let tokens = palette(
            mode,
            astrolith_core::preferences::ColorScheme::CatppuccinMocha,
        );
        assert!(
            contrast(tokens.foreground, tokens.background) >= 4.5,
            "{mode:?} primary text meets the normal-text target"
        );
        assert!(
            contrast(tokens.muted, tokens.background) >= 4.5,
            "{mode:?} secondary text meets the normal-text target"
        );
        assert!(
            contrast(tokens.primary_foreground, tokens.primary) >= 4.5,
            "{mode:?} primary action text meets the normal-text target"
        );
        assert!(
            contrast(tokens.focus, tokens.background) >= 3.,
            "{mode:?} the neutral focus ring is visible against the canvas"
        );
        assert_ne!(tokens.hover, tokens.selection);
    }

    let dark = palette(
        ThemeMode::Dark,
        astrolith_core::preferences::ColorScheme::CatppuccinMocha,
    );
    assert_ne!(dark.surface, dark.background);
    assert_ne!(dark.hover, dark.selection);
}

#[test]
fn popular_schemes_keep_text_and_action_contrast_in_both_modes() {
    use astrolith_core::preferences::ColorScheme;

    for scheme in [
        ColorScheme::CatppuccinLatte,
        ColorScheme::CatppuccinFrappe,
        ColorScheme::CatppuccinMacchiato,
        ColorScheme::CatppuccinMocha,
        ColorScheme::Nord,
        ColorScheme::NordLight,
        ColorScheme::TokyoNight,
        ColorScheme::TokyoNightStorm,
        ColorScheme::TokyoNightMoon,
        ColorScheme::TokyoDay,
        ColorScheme::GruvboxLight,
        ColorScheme::GruvboxDark,
    ] {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let tokens = palette(mode, scheme);
            assert!(
                contrast(tokens.foreground, tokens.background) >= 4.5,
                "{scheme:?} {mode:?} text"
            );
            assert!(
                contrast(tokens.primary_foreground, tokens.primary) >= 4.5,
                "{scheme:?} {mode:?} action"
            );
            assert!(
                contrast(tokens.focus, tokens.background) >= 3.0,
                "{scheme:?} {mode:?} focus"
            );
        }
    }
}

#[gpui_kit::test]
fn theme_adapter_projects_complete_semantic_roles_in_both_modes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            let settings = GlobalPreferencesV2 {
                theme: mode,
                ..GlobalPreferencesV2::default()
            };
            apply(&settings, WindowAppearance::Dark, cx);
            let tokens = palette(mode, settings.color_scheme);
            assert_eq!(cx.theme().background, tokens.background);
            assert_eq!(cx.theme().foreground, tokens.foreground);
            assert_eq!(cx.theme().sidebar, tokens.surface);
            assert_eq!(cx.theme().muted_foreground, tokens.muted);
            assert_eq!(cx.theme().border, tokens.border);
            assert_eq!(cx.theme().ring, tokens.focus);
            assert_eq!(cx.theme().primary, tokens.primary);
            assert_eq!(cx.theme().popover, tokens.popover);
            assert_eq!(cx.theme().tokens.background.color, tokens.background);
            assert_eq!(cx.theme().tokens.sidebar.color, tokens.surface);
            assert_eq!(cx.theme().status_bar, tokens.status_bar);
            assert_eq!(cx.theme().status_bar_border, tokens.border_variant);
            assert_eq!(cx.theme().tab_active, tokens.tab_active);
            assert_eq!(cx.theme().tab_foreground, tokens.tab_foreground);
            assert_eq!(cx.theme().list_hover, tokens.hover);
            assert_eq!(cx.theme().list_active, tokens.selection);
            assert_eq!(cx.theme().list_active_border, tokens.focus);
            assert_eq!(cx.theme().window_border, tokens.border_variant);
            assert_eq!(cx.theme().caret, tokens.caret);
            assert_eq!(cx.theme().font_size, gpui_kit::px(settings.ui_font_size));
            assert_eq!(cx.theme().radius, gpui_kit::px(settings.panel_radius));
            assert_eq!(cx.theme().radius_lg, gpui_kit::px(settings.panel_radius));
            assert_ne!(tokens.background, tokens.foreground);
            assert!(tokens.background.a > 0.99 && tokens.foreground.a > 0.99);
        }
    });
}
