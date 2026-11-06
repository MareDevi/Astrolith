//! Native application shell and its application-owned preferences.
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

/// Open the development-only text layout comparison window.
///
/// It renders one fixture in the application-owned layout core and can swap to
/// the Kit `TextView` for the same source. Release builds do not compile it.
#[cfg(debug_assertions)]
pub fn open_text_layout_spike(cx: &mut gpui_kit::App) -> anyhow::Result<()> {
    editor::text_layout::spike::open(cx)
}
