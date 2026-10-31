use super::ThemeTokens;
use gpui_kit::rgb;

pub(super) fn tokens() -> ThemeTokens {
    ThemeTokens {
        background: rgb(0xfafafa).into(),
        foreground: rgb(0x383a42).into(),
        surface: rgb(0xf0f0f0).into(),
        elevated: rgb(0xe5e5e6).into(),
        muted: rgb(0xa0a1a7).into(),
        border: rgb(0xdbdbdc).into(),
        border_variant: rgb(0xe5e5e6).into(),
        accent: rgb(0x4078f2).into(),
        accent_foreground: rgb(0xffffff).into(),
        selection: rgb(0xd7e4ff).into(),
        hover: rgb(0xe5e5e6).into(),
        active: rgb(0xdcdcdc).into(),
        status_bar: rgb(0xf0f0f0).into(),
        tab_bar: rgb(0xf0f0f0).into(),
        tab_active: rgb(0xfafafa).into(),
        tab_active_foreground: rgb(0x202124).into(),
        tab_foreground: rgb(0x696c77).into(),
        popover: rgb(0xffffff).into(),
        popover_foreground: rgb(0x383a42).into(),
        input: rgb(0xffffff).into(),
        caret: rgb(0x526fff).into(),
        scrollbar: rgb(0xfafafa).into(),
        scrollbar_thumb: rgb(0xc3c7cf).into(),
        danger: rgb(0xe45649).into(),
        warning: rgb(0xc18401).into(),
        success: rgb(0x50a14f).into(),
        info: rgb(0x0184bc).into(),
        link: rgb(0x4078f2).into(),
    }
}
