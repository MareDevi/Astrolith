use super::ThemeTokens;
use gpui_kit::rgb;

pub(super) fn tokens() -> ThemeTokens {
    ThemeTokens {
        background: rgb(0x282c34).into(),
        foreground: rgb(0xabb2bf).into(),
        surface: rgb(0x21252b).into(),
        muted: rgb(0x939baa).into(),
        border: rgb(0x464b57).into(),
        accent: rgb(0x61afef).into(),
    }
}
