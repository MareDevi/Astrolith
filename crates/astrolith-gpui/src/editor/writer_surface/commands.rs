//! Editing commands for the visible rich surface, using published input APIs.

use super::WriterSurface;
use crate::editor::input_boundaries::{grapheme_boundary, word_boundary};
use gpui_kit::component::input::{self, RopeExt};
use gpui_kit::{ClipboardItem, Context, Div, EntityInputHandler, Stateful, Window, prelude::*};

#[derive(Clone, Copy)]
enum Boundary {
    Grapheme,
    Word,
    Line,
    Document,
}

impl WriterSurface {
    pub(super) fn bind_commands(
        &self,
        root: Stateful<Div>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        macro_rules! movement {
            ($root:expr, $action:ty, $back:expr, $unit:ident, $extend:expr, $delete:expr) => {
                $root.on_action(cx.listener(|this, _: &$action, window, cx| {
                    this.move_or_delete($back, Boundary::$unit, $extend, $delete, window, cx);
                }))
            };
        }
        let root = root.key_context("Input");
        let root = movement!(root, input::Backspace, true, Grapheme, false, true);
        let root = movement!(root, input::Delete, false, Grapheme, false, true);
        let root = movement!(
            root,
            input::DeleteToPreviousWordStart,
            true,
            Word,
            false,
            true
        );
        let root = movement!(root, input::DeleteToNextWordEnd, false, Word, false, true);
        let root = movement!(
            root,
            input::DeleteToBeginningOfLine,
            true,
            Line,
            false,
            true
        );
        let root = movement!(root, input::DeleteToEndOfLine, false, Line, false, true);
        let root = movement!(root, input::MoveLeft, true, Grapheme, false, false);
        let root = movement!(root, input::MoveRight, false, Grapheme, false, false);
        let root = movement!(
            root,
            gpui_kit::base::actions::SelectLeft,
            true,
            Grapheme,
            true,
            false
        );
        let root = movement!(
            root,
            gpui_kit::base::actions::SelectRight,
            false,
            Grapheme,
            true,
            false
        );
        let root = movement!(root, input::MoveToPreviousWord, true, Word, false, false);
        let root = movement!(root, input::MoveToNextWord, false, Word, false, false);
        let root = movement!(
            root,
            input::SelectToPreviousWordStart,
            true,
            Word,
            true,
            false
        );
        let root = movement!(root, input::SelectToNextWordEnd, false, Word, true, false);
        let root = movement!(root, input::MoveToStartOfLine, true, Line, false, false);
        let root = movement!(root, input::MoveToEndOfLine, false, Line, false, false);
        let root = movement!(root, input::SelectToStartOfLine, true, Line, true, false);
        let root = movement!(root, input::SelectToEndOfLine, false, Line, true, false);
        let root = movement!(root, input::MoveToStart, true, Document, false, false);
        let root = movement!(root, input::MoveToEnd, false, Document, false, false);
        let root = movement!(root, input::SelectToStart, true, Document, true, false);
        let root = movement!(root, input::SelectToEnd, false, Document, true, false);
        macro_rules! navigation {
            ($root:expr, $action:ty, $key:expr, $shift:expr, $page:expr) => {
                $root.on_action(cx.listener(|this, _: &$action, window, cx| {
                    let steps = if $page {
                        (this.effective_viewport_height(window)
                            / (this.style.font_size * this.style.line_height))
                            .max(1.) as usize
                    } else {
                        1
                    };
                    for _ in 0..steps {
                        if !this.navigate($key, $shift, window, cx) {
                            break;
                        }
                    }
                }))
            };
        }
        let root = navigation!(root, input::MoveUp, "up", false, false);
        let root = navigation!(root, input::MoveDown, "down", false, false);
        let root = navigation!(root, input::MoveHome, "home", false, false);
        let root = navigation!(root, input::MoveEnd, "end", false, false);
        let root = navigation!(root, gpui_kit::base::actions::SelectUp, "up", true, false);
        let root = navigation!(
            root,
            gpui_kit::base::actions::SelectDown,
            "down",
            true,
            false
        );
        let root = navigation!(root, input::MovePageUp, "up", false, true);
        let root = navigation!(root, input::MovePageDown, "down", false, true);
        root.on_action(
            cx.listener(|this, _: &input::Indent, window, cx| this.indent(false, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &input::IndentInline, window, cx| this.indent(false, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &input::Outdent, window, cx| this.indent(true, window, cx)),
        )
        .on_action(
            cx.listener(|this, _: &input::OutdentInline, window, cx| this.indent(true, window, cx)),
        )
        .on_action(cx.listener(|this, _: &input::Enter, window, cx| this.insert("\n", window, cx)))
        .on_action(cx.listener(|this, _: &input::SelectAll, window, cx| {
            if let Some(editor) = this.input_model(cx) {
                editor.update(cx, |editor, cx| editor.select_all(window, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &input::Copy, _, cx| this.copy(cx)))
        .on_action(cx.listener(|this, _: &input::Cut, window, cx| {
            this.copy(cx);
            if let Some(editor) = this.input_model(cx)
                && !editor.read(cx).selected_range().is_empty()
            {
                this.insert("", window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &input::Paste, window, cx| {
            if let Some(item) = cx.read_from_clipboard() {
                this.paste(item, window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &input::Undo, window, cx| {
            if let Some(session) = this.session.upgrade() {
                session.update(cx, |session, cx| {
                    session.undo_source(window, cx);
                });
            }
        }))
        .on_action(cx.listener(|this, _: &input::Redo, window, cx| {
            if let Some(session) = this.session.upgrade() {
                session.update(cx, |session, cx| {
                    session.redo_source(window, cx);
                });
            }
        }))
        .on_action(cx.listener(|this, _: &input::Escape, window, cx| this.unmark_text(window, cx)))
    }

    fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.input_model(cx)
            && editor.read(cx).is_editable()
        {
            editor.update(cx, |editor, cx| editor.replace(text.to_owned(), window, cx));
        }
    }

    fn indent(&mut self, outdent: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editor) = self.input_model(cx) else {
            return;
        };
        let model = editor.read(cx);
        if !model.is_editable() {
            return;
        }
        let selection = model.selected_range();
        if !outdent && selection.is_empty() {
            self.insert("  ", window, cx);
            return;
        }
        let first = model.text().offset_to_point(selection.start).row;
        let last = model
            .text()
            .offset_to_point(
                selection
                    .end
                    .saturating_sub(usize::from(!selection.is_empty())),
            )
            .row;
        let start = model.text().line_start_offset(first);
        let end = model.text().line_end_offset(last);
        let old = model.text().slice(start..end).to_string();
        let replacement = old
            .split_inclusive('\n')
            .map(|line| {
                if outdent {
                    line.strip_prefix("  ")
                        .or_else(|| line.strip_prefix([' ', '\t']))
                        .unwrap_or(line)
                        .to_owned()
                } else {
                    format!("  {line}")
                }
            })
            .collect::<String>();
        editor.update(cx, |editor, cx| {
            editor.set_selected_range(start..end, cx);
            editor.replace(replacement, window, cx);
        });
    }

    fn copy(&self, cx: &mut Context<Self>) {
        if let Some(editor) = self.input_model(cx) {
            let editor = editor.read(cx);
            if !editor.selected_range().is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    editor.selected_text().to_string(),
                ));
            }
        }
    }

    fn move_or_delete(
        &mut self,
        backward: bool,
        unit: Boundary,
        extend: bool,
        delete: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.input_model(cx) else {
            return;
        };
        let model = editor.read(cx);
        if delete && !model.is_editable() {
            return;
        }
        let selection = model.selected_range();
        let cursor = model.cursor();
        let target = match unit {
            Boundary::Grapheme if !extend && !delete && !selection.is_empty() => {
                if backward {
                    selection.start
                } else {
                    selection.end
                }
            }
            Boundary::Grapheme => grapheme_boundary(model.text(), cursor, backward),
            Boundary::Word => word_boundary(model.text(), cursor, backward),
            Boundary::Line => {
                let row = model.text().offset_to_point(cursor).row;
                if backward {
                    model.text().line_start_offset(row)
                } else {
                    model.text().line_start_offset(row) + model.text().slice_line(row).len()
                        - usize::from(
                            model.text().slice_line(row).chars().reversed().next() == Some('\r'),
                        )
                }
            }
            Boundary::Document => {
                if backward {
                    0
                } else {
                    model.text().len()
                }
            }
        };
        if delete {
            let range = if !selection.is_empty() {
                selection
            } else {
                cursor.min(target)..cursor.max(target)
            };
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(range, cx);
                editor.replace("", window, cx);
            });
        } else if let Some(session) = self.session.upgrade() {
            let anchor = if extend {
                if cursor == selection.start {
                    selection.end
                } else {
                    selection.start
                }
            } else {
                target
            };
            session.update(cx, |session, cx| {
                session.set_writer_selection(anchor, target, window, cx)
            });
        }
    }

    pub(crate) fn refresh_caret(
        &mut self,
        state: (bool, usize, std::ops::Range<usize>),
        cx: &mut Context<Self>,
    ) {
        if self.caret_state.as_ref() == Some(&state) {
            return;
        }
        self.caret_state = Some(state);
        // The blink timer belongs to the caret layer. This notification only
        // rebuilds the text layer when focus or selection actually changed.
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::input::Rope;
    use unicode_segmentation::UnicodeSegmentation;

    #[test]
    fn deletion_boundaries_match_extended_graphemes_across_rope_chunks() {
        let text = "a e\u{301} 👩🏽‍💻 🇨🇳\r\n中文 ".repeat(200);
        let rope = Rope::from_str(&text);
        let boundaries: Vec<_> = text
            .grapheme_indices(true)
            .map(|(at, _)| at)
            .chain(std::iter::once(text.len()))
            .collect();
        for pair in boundaries.windows(2) {
            assert_eq!(grapheme_boundary(&rope, pair[0], false), pair[1]);
            assert_eq!(grapheme_boundary(&rope, pair[1], true), pair[0]);
        }
        assert_eq!(grapheme_boundary(&rope, 0, true), 0);
        assert_eq!(grapheme_boundary(&rope, text.len(), false), text.len());
    }
}
