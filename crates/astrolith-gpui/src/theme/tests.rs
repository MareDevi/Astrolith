use super::*;
use gpui_kit::{TestAppContext, WindowAppearance, component::ActiveTheme};

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
fn theme_adapter_projects_complete_semantic_roles_in_both_modes(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        for mode in [ThemeMode::Dark, ThemeMode::Light] {
            apply(mode, WindowAppearance::Dark, cx);
            let tokens = palette(mode);
            assert_eq!(cx.theme().background, tokens.background);
            assert_eq!(cx.theme().foreground, tokens.foreground);
            assert_eq!(cx.theme().sidebar, tokens.surface);
            assert_eq!(cx.theme().muted_foreground, tokens.muted);
            assert_eq!(cx.theme().border, tokens.border);
            assert_eq!(cx.theme().ring, tokens.accent);
            assert_eq!(cx.theme().tokens.background.color, tokens.background);
            assert_eq!(cx.theme().tokens.sidebar.color, tokens.surface);
            assert_ne!(tokens.background, tokens.foreground);
            assert!(tokens.background.a > 0.99 && tokens.foreground.a > 0.99);
        }
    });
}
