use super::ThemeTokens;
use gpui_kit::rgb;

pub(super) fn tokens() -> ThemeTokens {
    ThemeTokens {
        background: rgb(0xfafafa).into(),
        foreground: rgb(0x383a42).into(),
        surface: rgb(0xf0f0f0).into(),
        muted: rgb(0x626772).into(),
        border: rgb(0xc3c7cf).into(),
        accent: rgb(0x1f65c5).into(),
    }
}
