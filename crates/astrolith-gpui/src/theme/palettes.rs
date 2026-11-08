//! The two neutral palettes and the OKLCH boundary.
//!
//! Values come from the web mock's `index.css` and are the only raw colour
//! literals in the application. CSS speaks OKLCH; GPUI components speak HSLA,
//! so conversion happens here, once, at the theme boundary. Alpha survives the
//! conversion and is composited by the renderer exactly like CSS.

use gpui_kit::{Hsla, Rgba};

use super::ThemeTokens;

use astrolith_core::preferences::ColorScheme;

pub(super) fn named(scheme: ColorScheme) -> ThemeTokens {
    let (colors, dark) = match scheme {
        ColorScheme::CatppuccinLatte => (
            [
                "eff1f5", "4c4f69", "e6e9ef", "ccd0da", "8839ef", "d20f39", "df8e1d", "40a02b",
                "1e66f5",
            ],
            false,
        ),
        ColorScheme::CatppuccinFrappe => (
            [
                "303446", "c6d0f5", "292c3c", "414559", "ca9ee6", "e78284", "e5c890", "a6d189",
                "8caaee",
            ],
            true,
        ),
        ColorScheme::CatppuccinMacchiato => (
            [
                "24273a", "cad3f5", "1e2030", "363a4f", "c6a0f6", "ed8796", "eed49f", "a6da95",
                "8aadf4",
            ],
            true,
        ),
        ColorScheme::CatppuccinMocha => (
            [
                "1e1e2e", "cdd6f4", "181825", "313244", "cba6f7", "f38ba8", "f9e2af", "a6e3a1",
                "89b4fa",
            ],
            true,
        ),
        ColorScheme::Nord => (
            [
                "2e3440", "eceff4", "3b4252", "434c5e", "88c0d0", "bf616a", "ebcb8b", "a3be8c",
                "81a1c1",
            ],
            true,
        ),
        ColorScheme::NordLight => (
            [
                "eceff4", "2e3440", "e5e9f0", "d8dee9", "5e81ac", "bf616a", "d08770", "a3be8c",
                "5e81ac",
            ],
            false,
        ),
        ColorScheme::TokyoNight => (
            [
                "1a1b26", "c0caf5", "16161e", "292e42", "7aa2f7", "f7768e", "e0af68", "9ece6a",
                "7dcfff",
            ],
            true,
        ),
        ColorScheme::TokyoNightStorm => (
            [
                "24283b", "c0caf5", "1f2335", "3b4261", "7aa2f7", "f7768e", "e0af68", "9ece6a",
                "7dcfff",
            ],
            true,
        ),
        ColorScheme::TokyoNightMoon => (
            [
                "222436", "c8d3f5", "1e2030", "3b3f5c", "82aaff", "ff757f", "ffc777", "c3e88d",
                "86e1fc",
            ],
            true,
        ),
        ColorScheme::TokyoDay => (
            [
                "e1e2e7", "3760bf", "d0d5e3", "c4c8da", "2e7de9", "f52a65", "8c6c3e", "587539",
                "007197",
            ],
            false,
        ),
        ColorScheme::GruvboxLight => (
            [
                "fbf1c7", "3c3836", "f2e5bc", "ebdbb2", "076678", "9d0006", "b57614", "79740e",
                "076678",
            ],
            false,
        ),
        ColorScheme::GruvboxDark => (
            [
                "282828", "ebdbb2", "3c3836", "504945", "83a598", "fb4934", "fabd2f", "b8bb26",
                "83a598",
            ],
            true,
        ),
    };
    let [
        background,
        foreground,
        surface,
        border,
        primary,
        danger,
        warning,
        success,
        info,
    ] = colors.map(hex);
    let control = border.opacity(0.45);
    let muted = foreground.opacity(if dark { 0.72 } else { 0.68 });
    let selection = primary.opacity(if dark { 0.25 } else { 0.16 });
    let hover = foreground.opacity(if dark { 0.06 } else { 0.045 });
    let active = foreground.opacity(if dark { 0.1 } else { 0.08 });
    ThemeTokens {
        background,
        foreground,
        surface,
        elevated: surface,
        control,
        muted,
        border,
        border_variant: border.opacity(if dark { 0.7 } else { 0.8 }),
        primary,
        primary_foreground: if luminance(primary) > 0.18 {
            hex("000000")
        } else {
            hex("ffffff")
        },
        focus: primary,
        selection,
        hover,
        active,
        status_bar: surface,
        tab_bar: background,
        tab_active: surface,
        tab_active_foreground: foreground,
        tab_foreground: muted,
        popover: surface,
        popover_foreground: foreground,
        caret: foreground,
        scrollbar: background,
        scrollbar_thumb: border,
        overlay: hex("000000").opacity(if dark { 0.55 } else { 0.32 }),
        danger,
        warning,
        success,
        info,
        link: primary,
    }
}

fn hex(value: &str) -> Hsla {
    let value = u32::from_str_radix(value, 16).unwrap_or_default();
    Rgba {
        r: ((value >> 16) & 255) as f32 / 255.,
        g: ((value >> 8) & 255) as f32 / 255.,
        b: (value & 255) as f32 / 255.,
        a: 1.,
    }
    .into()
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
