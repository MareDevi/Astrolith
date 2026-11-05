//! Language-server client for the Build mode code editor.
//!
//! This crate is GUI-free service code: it discovers project-local language
//! servers, speaks LSP over their stdio pipes, and exposes a small command /
//! event API. The GPUI layer owns presentation and never blocks on a server.
//!
//! Supported servers cover the usual Astro toolchain: `astro-ls`,
//! `typescript-language-server`, the VS Code JSON/CSS/HTML servers,
//! `svelteserver` and `vue-language-server`. A language with no installed
//! server is reported as [`DiscoveryError::NotFound`]; the editor then stays a
//! plain syntax-highlighted buffer.

pub mod client;
pub mod discovery;
pub mod framing;
pub mod language;
pub mod position;

pub use client::{
    Client, ClientError, ClientHandle, DEFAULT_REQUEST_TIMEOUT, Event, SpawnSpec,
    code_action_params, completion_params, definition_params, hover_params, methods, range_offsets,
    uri_from_path,
};
pub use discovery::{DiscoveryError, ServerSpec, discover};
pub use language::{LanguageId, ServerDef};
pub use position::{
    char_position_to_offset, char_position_to_utf16, convert_positions, offset_to_position,
    position_to_offset, utf16_position_to_char,
};
