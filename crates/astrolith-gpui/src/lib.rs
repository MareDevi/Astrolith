//! Native application shell and its application-owned preferences.
mod commands;
mod editor;
mod file;
pub mod frontmatter;
mod layout;
mod preferences;
mod preview;
mod project_discovery;
mod runtime;
mod settings;
mod shell;
pub mod theme;
pub use shell::open;
