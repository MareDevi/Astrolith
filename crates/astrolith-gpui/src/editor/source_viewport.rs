//! Keep the native source caret visible when a pane shrinks.
use gpui_kit::component::input::EditorState;
use gpui_kit::{
    AnyElement, Entity, Focusable, IntoElement, Pixels, Size, canvas, div, prelude::*, px,
};
use std::{cell::Cell, rc::Rc};

pub(crate) fn source_viewport(editor: Entity<EditorState>, child: AnyElement) -> AnyElement {
    div()
        .relative()
        .size_full()
        .child(child)
        .child(
            canvas(
                move |_, window, cx| {
                    let state = editor.read(cx);
                    if !state.focus_handle(cx).is_focused(window) {
                        return None;
                    }
                    let geometry = state.prepaint_caret_geometry()?;
                    let bounds = state.input_bounds();
                    let mut caret = geometry.cursor_bounds();
                    caret.origin.y += geometry.scroll_offset().y;
                    let offset = state.scroll_offset();
                    let mut next = offset;
                    if caret.right() > bounds.right() {
                        next.x -= caret.right() - bounds.right() + px(2.);
                    } else if caret.left() < bounds.left() {
                        next.x += bounds.left() - caret.left() + px(2.);
                    }
                    if caret.bottom() > bounds.bottom() {
                        next.y -= caret.bottom() - bounds.bottom() + px(2.);
                    } else if caret.top() < bounds.top() {
                        next.y += bounds.top() - caret.top() + px(2.);
                    }
                    Some((editor.clone(), bounds.size, offset, next))
                },
                |_, correction, window, cx| {
                    let Some((editor, size, before, after)) = correction else {
                        return;
                    };
                    // Element state survives rebuilding this wrapper in its keyed parent.
                    let previous = window.use_keyed_state(
                        ("source-viewport", editor.entity_id()),
                        cx,
                        |_, _| Rc::new(Cell::new(None::<(Size<Pixels>, bool)>)),
                    );
                    let shrank = previous
                        .read(cx)
                        .replace(Some((size, before == after)))
                        .is_some_and(|(old, visible)| {
                            visible && (size.width < old.width || size.height < old.height)
                        });
                    if shrank && before != after {
                        cx.defer(move |cx| {
                            editor.update(cx, |state, cx| state.set_scroll_offset(after, cx))
                        });
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .into_any_element()
}
