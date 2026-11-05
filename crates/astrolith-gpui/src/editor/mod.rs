pub(crate) mod assets;
pub(crate) mod component_builder;
pub(crate) mod content_finder;
pub(crate) mod copyedit;
pub(crate) mod formatting;
pub(crate) mod header;
pub(crate) mod image_preview;
pub(crate) mod lsp;
pub(crate) mod rendered;
pub(crate) mod save_coordinator;
pub(crate) mod save_failure;
pub(crate) mod selection_toolbar;
pub(crate) mod session;
mod text_change;
pub(crate) mod writer_chrome;
pub(crate) mod writer_surface;
pub(crate) mod writing_modes;

use std::rc::Rc;

use gpui_kit::component::input::EditorState;
use gpui_kit::{App, Entity, IntoElement as _, div};

/// Drop the editor's own scrollbar.
///
/// The writing page scrolls with the wheel and the keyboard; the vertical
/// track Base paints on the document reads as IDE chrome and competes with the
/// prose. Painting an empty element in its place keeps every other editor
/// behaviour, including the scroll range, intact.
pub(crate) fn hide_scrollbar(editor: &Entity<EditorState>, cx: &mut App) {
    editor.update(cx, |state, _| {
        state.set_scrollbar_renderer(Some(Rc::new(|_, _, _| div().into_any_element())));
    });
}
