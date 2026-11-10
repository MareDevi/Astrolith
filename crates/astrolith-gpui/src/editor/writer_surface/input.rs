//! Native input uses the retained editing model and the visible rich geometry.
//! No invisible source editor is mounted or measured for Writing.

use std::ops::Range;

use gpui_kit::component::input::{EditorState, InputEvent, RopeExt};
use gpui_kit::{
    Bounds, ClipboardItem, Context, ElementInputHandler, Entity, EntityInputHandler, Focusable,
    Pixels, Point, UTF16Selection, Window, point, px, size,
};

use super::WriterSurface;

impl WriterSurface {
    pub(super) fn input_model(&self, cx: &Context<Self>) -> Option<Entity<EditorState>> {
        let session = self.session.upgrade()?;
        let session = session.read(cx);
        (!session.is_source_view()).then(|| session.editor.clone())
    }

    pub(super) fn register_input(
        &self,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.input_model(cx) else {
            return;
        };
        let focus = editor.read(cx).focus_handle(cx);
        window.handle_input(&focus, ElementInputHandler::new(bounds, cx.entity()), cx);
    }
}

impl EntityInputHandler for WriterSurface {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        self.input_model(cx)?.update(cx, |editor, cx| {
            editor.text_for_range(range, adjusted, window, cx)
        })
    }

    fn selected_text_range(
        &mut self,
        ignore_disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        self.input_model(cx)?.update(cx, |editor, cx| {
            editor.selected_text_range(ignore_disabled, window, cx)
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.input_model(cx)?
            .update(cx, |editor, cx| editor.marked_text_range(window, cx))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.input_model(cx) {
            let committing = self
                .session
                .upgrade()
                .is_some_and(|session| session.read(cx).writer_preedit_active());
            editor.update(cx, |editor, cx| {
                editor.unmark_text(window, cx);
                if committing {
                    // Kit unmarks without a Change; publish this commit to the
                    // session just as a replacement commit would.
                    cx.emit(InputEvent::Change);
                }
            });
        }
    }

    fn paste(&mut self, item: ClipboardItem, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.input_model(cx) {
            editor.update(cx, |editor, cx| editor.paste(item, window, cx));
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.input_model(cx) {
            editor.update(cx, |editor, cx| {
                editor.replace_text_in_range(range, text, window, cx)
            });
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(session), Some(editor)) = (self.session.upgrade(), self.input_model(cx)) else {
            return;
        };
        if text.is_empty()
            && session.update(cx, |session, cx| session.cancel_writer_preedit(window, cx))
        {
            return;
        }
        let initial = (!text.is_empty() && !session.read(cx).writer_preedit_active()).then(|| {
            let editor = editor.read(cx);
            (editor.text().clone(), editor.selected_range())
        });
        let marked = editor.update(cx, |editor, cx| {
            editor.replace_and_mark_text_in_range(range, text, selected, window, cx);
            initial.is_some() && editor.marked_text_range(window, cx).is_some()
        });
        if marked && let Some((before, selection)) = initial {
            session.update(cx, |session, _| {
                session.retain_writer_preedit(before, selection)
            });
        }
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let editor = self.input_model(cx)?;
        let source = editor.read(cx).text().offset_utf16_to_offset(range.start);
        let hit = self.hit_for_source(self.parsed_offset(source))?;
        let layout = self.layout.borrow();
        let caret = layout.as_ref()?.caret(hit)?;
        let geometry = *self.geometry.borrow();
        Some(Bounds::new(
            point(
                geometry.origin.x + caret.origin.x,
                geometry.origin.y + caret.origin.y,
            ),
            size(px(1.5), caret.height.max(px(2.))),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let source = self.source_at(position)?;
        let editor = self.input_model(cx)?;
        Some(editor.read(cx).text().offset_to_offset_utf16(source))
    }

    fn text_length_utf16(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Option<usize> {
        let editor = self.input_model(cx)?;
        let text = editor.read(cx).text();
        Some(text.offset_to_offset_utf16(text.len()))
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = self.input_model(cx) {
            editor.update(cx, |editor, cx| {
                let text = editor.text();
                let range = text.offset_utf16_to_offset(range.start)
                    ..text.offset_utf16_to_offset(range.end);
                editor.set_selected_range(range, cx);
            });
        }
    }
}
