//! The two neutral palettes and the OKLCH boundary.
//!
//! Values come from the web mock's `index.css` and are the only raw colour
//! literals in the application. CSS speaks OKLCH; GPUI components speak HSLA,
//! so conversion happens here, once, at the theme boundary. Alpha survives the
//! conversion and is composited by the renderer exactly like CSS.

use gpui_kit::{Hsla, Rgba};

use super::ThemeTokens;

/// Convert a CSS `oklch(L C H / alpha)` colour to sRGB-backed HSLA.
///
/// `lightness` is CSS's 0…1 perceptual lightness (not the 0…100 percentage),
/// `chroma` is its 0…0.4-ish chroma, `hue` is degrees, and `alpha` is 0…1.
/// Out-of-gamut channels are clipped to sRGB, matching browser behaviour.
pub(super) fn oklch(lightness: f32, chroma: f32, hue: f32, alpha: f32) -> Hsla {
    let hue = hue.to_radians();
    let a = chroma * hue.cos();
    let b = chroma * hue.sin();

    let l_ = lightness + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = lightness - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = lightness - 0.089_484_18 * a - 1.291_485_5 * b;

    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;

    let red = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s;
    let green = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s;
    let blue = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;

    Rgba {
        r: encode_srgb(red),
        g: encode_srgb(green),
        b: encode_srgb(blue),
        a: alpha.clamp(0., 1.),
    }
    .into()
}

fn encode_srgb(linear: f32) -> f32 {
    let linear = linear.clamp(0., 1.);
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    }
}

/// One scheme's semantic relationships before they become concrete roles.
struct Base {
    background: Hsla,
    foreground: Hsla,
    surface: Hsla,
    elevated: Hsla,
    control: Hsla,
    muted: Hsla,
    border: Hsla,
    border_variant: Hsla,
    primary: Hsla,
    primary_foreground: Hsla,
    focus: Hsla,
    selection: Hsla,
    hover: Hsla,
    active: Hsla,
    tab_bar: Hsla,
    tab_active: Hsla,
    danger: Hsla,
    warning: Hsla,
    success: Hsla,
    info: Hsla,
    overlay: Hsla,
}

impl Base {
    fn tokens(self) -> ThemeTokens {
        let popover = self.elevated;
        ThemeTokens {
            background: self.background,
            foreground: self.foreground,
            surface: self.surface,
            elevated: self.elevated,
            control: self.control,
            muted: self.muted,
            border: self.border,
            border_variant: self.border_variant,
            primary: self.primary,
            primary_foreground: self.primary_foreground,
            focus: self.focus,
            selection: self.selection,
            hover: self.hover,
            active: self.active,
            status_bar: self.surface,
            tab_bar: self.tab_bar,
            tab_active: self.tab_active,
            tab_active_foreground: self.foreground,
            tab_foreground: self.muted,
            popover,
            popover_foreground: self.foreground,
            caret: self.foreground,
            scrollbar: self.background,
            scrollbar_thumb: self.border,
            overlay: self.overlay,
            danger: self.danger,
            warning: self.warning,
            success: self.success,
            info: self.info,
            link: self.primary,
        }
    }
}

/// Neutral light: white canvas, near-black text, one quiet fill ladder.
pub(super) fn light() -> ThemeTokens {
    Base {
        background: oklch(1., 0., 0., 1.),
        foreground: oklch(0.145, 0., 0., 1.),
        surface: oklch(0.985, 0., 0., 1.),
        elevated: oklch(1., 0., 0., 1.),
        control: oklch(0.961, 0., 0., 1.),
        muted: oklch(0.556, 0., 0., 1.),
        border: oklch(0.922, 0., 0., 1.),
        border_variant: oklch(0.96, 0., 0., 1.),
        primary: oklch(0.205, 0., 0., 1.),
        primary_foreground: oklch(0.985, 0., 0., 1.),
        focus: oklch(0.65, 0., 0., 1.),
        selection: oklch(0.94, 0., 0., 1.),
        hover: oklch(0.97, 0., 0., 1.),
        active: oklch(0.93, 0., 0., 1.),
        tab_bar: oklch(1., 0., 0., 1.),
        tab_active: oklch(0.985, 0., 0., 1.),
        danger: oklch(0.577, 0.245, 27.325, 1.),
        warning: oklch(0.7, 0.15, 75., 1.),
        success: oklch(0.6, 0.13, 150., 1.),
        info: oklch(0.55, 0.12, 250., 1.),
        overlay: oklch(0., 0., 0., 0.32),
    }
    .tokens()
}

/// Neutral dark: near-black canvas, near-white text, translucent hairlines.
pub(super) fn dark() -> ThemeTokens {
    Base {
        background: oklch(0.145, 0., 0., 1.),
        foreground: oklch(0.985, 0., 0., 1.),
        surface: oklch(0.205, 0., 0., 1.),
        elevated: oklch(0.205, 0., 0., 1.),
        control: oklch(1., 0., 0., 0.15),
        muted: oklch(0.708, 0., 0., 1.),
        border: oklch(1., 0., 0., 0.1),
        border_variant: oklch(1., 0., 0., 0.06),
        primary: oklch(0.922, 0., 0., 1.),
        primary_foreground: oklch(0.205, 0., 0., 1.),
        focus: oklch(0.556, 0., 0., 1.),
        selection: oklch(0.269, 0., 0., 1.),
        hover: oklch(1., 0., 0., 0.06),
        active: oklch(1., 0., 0., 0.1),
        tab_bar: oklch(0.145, 0., 0., 1.),
        tab_active: oklch(0.205, 0., 0., 1.),
        danger: oklch(0.704, 0.191, 22.216, 1.),
        warning: oklch(0.79, 0.13, 80., 1.),
        success: oklch(0.76, 0.13, 150., 1.),
        info: oklch(0.72, 0.1, 250., 1.),
        overlay: oklch(0., 0., 0., 0.55),
    }
    .tokens()
}
