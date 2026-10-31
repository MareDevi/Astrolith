use super::ThemeTokens;
use gpui_kit::rgb;

pub(super) fn tokens() -> ThemeTokens {
    ThemeTokens {
        background: rgb(0x282c34).into(),
        foreground: rgb(0xabb2bf).into(),
        surface: rgb(0x21252b).into(),
        elevated: rgb(0x2c313a).into(),
        muted: rgb(0x5c6370).into(),
        border: rgb(0x3b4048).into(),
        border_variant: rgb(0x181a1f).into(),
        accent: rgb(0x61afef).into(),
        accent_foreground: rgb(0x1e2127).into(),
        selection: rgb(0x3e4451).into(),
        hover: rgb(0x2c313a).into(),
        active: rgb(0x3a3f4b).into(),
        status_bar: rgb(0x21252b).into(),
        tab_bar: rgb(0x21252b).into(),
        tab_active: rgb(0x282c34).into(),
        tab_active_foreground: rgb(0xd7dae0).into(),
        tab_foreground: rgb(0x8b929e).into(),
        popover: rgb(0x2c313a).into(),
        popover_foreground: rgb(0xd7dae0).into(),
        input: rgb(0x1b1f24).into(),
        caret: rgb(0x528bff).into(),
        scrollbar: rgb(0x282c34).into(),
        scrollbar_thumb: rgb(0x4b5263).into(),
        danger: rgb(0xe06c75).into(),
        warning: rgb(0xe5c07b).into(),
        success: rgb(0x98c379).into(),
        info: rgb(0x56b6c2).into(),
        link: rgb(0x61afef).into(),
    }
}
