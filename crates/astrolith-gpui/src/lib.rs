//! Native application shell and its application-owned preferences.
rust_i18n::i18n!("locales", fallback = "en");

/// Register application translations with GPUI Kit before component startup.
pub fn extend_i18n() {
    use gpui_kit::component as gpui_component;
    rust_i18n::extend!(gpui_component);
}

mod commands;
mod desktop_actions;
mod drag_drop;
mod editor;
mod file;
mod file_browser;
pub mod frontmatter;
mod help;
mod http;
mod layout;
mod preferences;
mod preview;
mod project_discovery;
mod recent_projects;
mod runtime;
mod settings;
mod shell;
mod single_instance;
mod terminal;
pub mod theme;
mod ui;
mod update;
mod url_scheme;
pub use shell::open;
