use super::*;
use gpui_kit::{
    Hsla, Rgba, SharedString, TestAppContext, WindowAppearance, component::ActiveTheme, px, rgb,
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
        let mut settings = GlobalPreferences {
            theme: ThemeMode::Light,
            ui_font_family: custom.clone(),
            ui_font_size: 17.,
            ..GlobalPreferences::default()
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
        ColorScheme::AstrolithLight,
        ColorScheme::AstrolithDark,
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
            let settings = GlobalPreferences {
                theme: mode,
                ..GlobalPreferences::default()
            };
            apply(&settings, WindowAppearance::Dark, cx);
            let scheme = match mode {
                ThemeMode::Dark => settings.dark_color_scheme,
                ThemeMode::Light | ThemeMode::System => settings.light_color_scheme,
            };
            let tokens = palette(mode, scheme);
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
            assert_eq!(
                cx.theme().radius_lg,
                gpui_kit::px(settings.panel_radius + 4.)
            );
            assert_eq!(cx.theme().tab_bar_segmented, tokens.tab_bar_segmented);
            assert_eq!(cx.theme().danger_hover, tokens.danger_hover);
            assert_eq!(cx.theme().danger_foreground, tokens.danger_foreground);
            assert_ne!(tokens.background, tokens.foreground);
            assert!(tokens.background.a > 0.99 && tokens.foreground.a > 0.99);
        }
    });
}

#[gpui_kit::test]
fn radii_stay_concentric_and_collapse_with_the_panel_radius(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        for (panel_radius, inner, outer) in [(10., 10., 14.), (30., 20., 24.), (0., 0., 0.)] {
            let settings = GlobalPreferences {
                panel_radius,
                ..GlobalPreferences::default()
            };
            apply(&settings, WindowAppearance::Dark, cx);
            assert_eq!(cx.theme().radius, px(inner), "radius for {panel_radius}");
            assert_eq!(
                cx.theme().radius_lg,
                px(outer),
                "radius_lg for {panel_radius}"
            );
            if inner > 0. {
                assert!(
                    cx.theme().radius_lg > cx.theme().radius,
                    "the surface tier rounds more than the controls inside it"
                );
                assert_eq!(radius_sm(cx), px(inner - 2.));
            } else {
                assert_eq!(radius_sm(cx), px(0.), "square corners stay square");
            }
        }
    });
}

#[test]
fn chrome_planes_and_segmented_tracks_stay_distinct() {
    use astrolith_core::preferences::ColorScheme;

    for scheme in [
        ColorScheme::AstrolithLight,
        ColorScheme::AstrolithDark,
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
            assert_eq!(tokens.tab_bar, tokens.surface, "{scheme:?} {mode:?} strip");
            assert_eq!(
                tokens.tab_active, tokens.background,
                "{scheme:?} {mode:?} active tab"
            );
            assert_eq!(
                tokens.segmented_active, tokens.tab_active,
                "{scheme:?} {mode:?} selected segment rises onto the canvas"
            );
            assert_ne!(
                tokens.tab_bar_segmented, tokens.surface,
                "{scheme:?} {mode:?} track is a well inside the chrome plane"
            );
            assert_ne!(
                tokens.elevated, tokens.tab_bar_segmented,
                "{scheme:?} {mode:?} the raised pill clears the track"
            );
        }
    }
}

#[test]
fn semantic_status_roles_keep_readable_foregrounds_and_feedback() {
    use astrolith_core::preferences::ColorScheme;

    for scheme in [
        ColorScheme::AstrolithLight,
        ColorScheme::AstrolithDark,
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
            for (role, fill, foreground) in [
                ("danger", tokens.danger, tokens.danger_foreground),
                ("warning", tokens.warning, tokens.warning_foreground),
                ("success", tokens.success, tokens.success_foreground),
                ("info", tokens.info, tokens.info_foreground),
            ] {
                assert!(
                    contrast(foreground, fill) >= 4.5,
                    "{scheme:?} {mode:?} {role} text"
                );
            }
            assert_ne!(tokens.danger_hover, tokens.danger);
            assert_ne!(tokens.danger_active, tokens.danger);
            assert_ne!(tokens.warning_hover, tokens.warning);
            assert_ne!(tokens.success_hover, tokens.success);
            assert_ne!(tokens.info_hover, tokens.info);
        }
    }
}

#[test]
fn terminal_ansi_palettes_stay_readable_on_each_canvas() {
    use astrolith_core::preferences::ColorScheme;

    for (dark, canvas) in [
        (
            true,
            palette(ThemeMode::Dark, ColorScheme::AstrolithDark).background,
        ),
        (
            false,
            palette(ThemeMode::Light, ColorScheme::AstrolithLight).background,
        ),
    ] {
        let ansi = terminal_ansi_palette(dark);
        // Slots 0 and 15 are the terminal's own background/fill, so only the
        // four ordinary hues have to read as text on the canvas.
        for value in &ansi[1..=4] {
            let color: Hsla = rgb(*value).into();
            assert!(contrast(color, canvas) >= 4.5, "{dark:?} ansi hues");
        }
    }
}
