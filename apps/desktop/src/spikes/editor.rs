//! Disposable M00.2 probe. No filesystem, persistence, or document architecture.
use gpui_kit::component::{
    ActiveTheme,
    button::Button,
    input::{Editor, EditorState},
};
use gpui_kit::{
    App, AppContext, Context, ElementInputHandler, Entity, InputHandler, IntoElement, KeyBinding,
    Render, SharedString, Subscription, Window, actions, div, prelude::*,
};

const REFRESH_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-alt-r"
} else {
    "ctrl-alt-r"
};

const SAMPLE: &str = "# Astrolith editor spike\n\n中文 · 日本語 · 🦀 · e\u{301}\n\n**Bold** and *emphasis*, `inline code`, [link](https://example.com).\n\n- First item\n- Second item\n\n```rust\nfn main() {}\n```\n";
actions!(editor_spike, [RefreshSample]);

pub(crate) struct EditorSpike {
    editor: Entity<EditorState>,
    status: SharedString,
    _subscription: Subscription,
}

impl EditorSpike {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .default_value(SAMPLE)
        });
        let subscription = cx.observe(&editor, |_, _, cx| cx.notify());
        Self {
            editor,
            status: "Edit the sample; refresh is undoable".into(),
            _subscription: subscription,
        }
    }

    pub(crate) fn bind_keys(cx: &mut App) {
        cx.bind_keys([KeyBinding::new(REFRESH_KEY, RefreshSample, None)]);
    }

    fn refresh_sample(&mut self, _: &RefreshSample, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_content(SAMPLE.into(), window, cx);
    }

    fn replace_content(&mut self, text: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let mut handler =
            ElementInputHandler::new(self.editor.read(cx).input_bounds(), self.editor.clone());
        if handler.marked_text_range(window, cx).is_some() {
            self.status = "Refresh blocked during composition; commit or cancel, then retry".into();
            cx.notify();
            return;
        }
        self.editor
            .update(cx, |editor, cx| editor.replace_all(text, window, cx));
        self.status = "Replaced content; undo restores the previous buffer".into();
        cx.notify();
    }
}

impl Render for EditorSpike {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.editor.read(cx);
        let detail = format!(
            "{} bytes · cursor {} · selection {:?} (UTF-8)",
            state.text().len(),
            state.cursor(),
            state.selected_range()
        );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(Self::refresh_sample))
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .child("Markdown editor · M00.2")
                    .child(
                        Button::new("refresh-sample")
                            .label("Refresh sample")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.refresh_sample(&RefreshSample, window, cx)
                            })),
                    )
                    .children(
                        [
                            (1024, "1 KiB"),
                            (100 * 1024, "100 KiB"),
                            (1024 * 1024, "1 MiB"),
                        ]
                        .map(|(bytes, label)| {
                            Button::new(("load", bytes))
                                .label(label)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.replace_content(workload(bytes, false).into(), window, cx)
                                }))
                        }),
                    )
                    .child(Button::new("load-long-line").label("Long line").on_click(cx.listener(|this, _, window, cx| {
                        this.replace_content(workload(100 * 1024, true).into(), window, cx)
                    }))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.editor).size_full()),
            )
            .child(
                div()
                    .p_2()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .flex()
                    .flex_col()
                    .child(detail)
                    .child(self.status.clone())
                    .child(format!("Refresh: {REFRESH_KEY} · standard selection, undo/redo and clipboard shortcuts")),
            )
    }
}

fn workload(min_bytes: usize, long_line: bool) -> String {
    let unit = if long_line {
        "中文日本語🦀 **bold** `code` ".to_owned()
    } else {
        format!(
            "{SAMPLE}{}\n\n",
            "中文・日本語・🦀 **bold** and `inline code`. ".repeat(16)
        )
    };
    unit.repeat(min_bytes.div_ceil(unit.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{
        Bounds, Point, TestAppContext, WindowBounds, WindowHandle, WindowOptions, base::Root, px,
        size, test::TestWindowExt,
    };

    fn mount(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<EditorSpike>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            EditorSpike::bind_keys(cx);
        });
        cx.update(|cx| {
            let (handle, view) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(1000.), px(700.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| EditorSpike::new(window, cx)),
            )
            .unwrap();
            (handle.downcast::<Root>().unwrap(), view)
        })
    }

    #[gpui_kit::test]
    fn refresh_during_preedit_preserves_marked_text_and_selection(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).editor.clone();
            window.click(("input", state.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("A🦀Z", cx);
            let mut handler = ElementInputHandler::new(
                window.find(("input", state.entity_id())).bounds(),
                state.clone(),
            );
            handler.replace_and_mark_text_in_range(Some(1..3), "日本", Some(2..2), window, cx);
            assert_eq!(handler.marked_text_range(window, cx), Some(1..3));
            window.press(REFRESH_KEY, cx);
            assert_eq!(
                state.read(cx).value(),
                "A日本Z",
                "refresh must not erase active preedit"
            );
            assert_eq!(handler.marked_text_range(window, cx), Some(1..3));
            assert_eq!(
                handler
                    .selected_text_range(false, window, cx)
                    .unwrap()
                    .range,
                3..3
            );
            handler.replace_text_in_range(None, "日本語", window, cx);
            window.render_frame(cx);
            assert_eq!(state.read(cx).value(), "A日本語Z");
            assert_eq!(handler.marked_text_range(window, cx), None);
        })
        .unwrap();
    }

    fn undo_key() -> &'static str {
        if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        }
    }
    fn redo_key() -> &'static str {
        if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-y"
        }
    }

    #[gpui_kit::test]
    fn unicode_selection_keyboard_replacement_and_history_round_trip(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).editor.clone();
            window.click(("input", state.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("A🦀中文", cx);
            window.press("shift-left", cx);
            window.press("shift-left", cx);
            assert_eq!(state.read(cx).selected_range(), 5..11);
            assert_eq!(state.read(cx).selected_value(), "中文");
            assert_eq!(state.read(cx).cursor(), 5);
            let mut handler = ElementInputHandler::new(
                window.find(("input", state.entity_id())).bounds(),
                state.clone(),
            );
            let selection = handler.selected_text_range(false, window, cx).unwrap();
            assert_eq!(selection.range, 3..5);
            assert!(selection.reversed);
            window.input("日", cx);
            assert_eq!(
                window.find(("input", state.entity_id())).value(),
                Some("A🦀日")
            );
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), "A🦀中文");
            assert_eq!(state.read(cx).selected_range(), 5..11);
            window.press(redo_key(), cx);
            assert_eq!(state.read(cx).value(), "A🦀日");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn refresh_is_an_undoable_programmatic_edit(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).editor.clone();
            window.click(("input", state.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("Unsaved 中文", cx);
            window.press(REFRESH_KEY, cx);
            assert_eq!(state.read(cx).value(), SAMPLE);
            assert_eq!(state.read(cx).selected_range(), 0..0);
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), "Unsaved 中文");
            window.press(redo_key(), cx);
            assert_eq!(state.read(cx).value(), SAMPLE);
            window.press(undo_key(), cx);
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), "U");
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), SAMPLE);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn clipboard_shortcuts_copy_cut_and_paste_unicode(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).editor.clone();
            window.click(("input", state.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("e\u{301}👩‍💻中文", cx);
            window.press("secondary-a", cx);
            window.press("secondary-c", cx);
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().as_deref(),
                Some("e\u{301}👩‍💻中文")
            );
            window.press("secondary-x", cx);
            assert_eq!(state.read(cx).value(), "");
            window.press("secondary-v", cx);
            assert_eq!(state.read(cx).value(), "e\u{301}👩‍💻中文");
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), "");
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), "e\u{301}👩‍💻中文");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn simulated_chinese_preedit_cancel_then_retry_refresh_keeps_history(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).editor.clone();
            window.click(("input", state.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("A🦀", cx);
            let mut handler = ElementInputHandler::new(
                window.find(("input", state.entity_id())).bounds(),
                state.clone(),
            );
            handler.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
            handler.replace_and_mark_text_in_range(None, "你", Some(1..1), window, cx);
            window.press(REFRESH_KEY, cx);
            assert_eq!(state.read(cx).value(), "A🦀你");
            assert_eq!(handler.marked_text_range(window, cx), Some(3..4));
            handler.replace_and_mark_text_in_range(None, "", None, window, cx);
            assert_eq!(handler.marked_text_range(window, cx), None);
            assert_eq!(state.read(cx).value(), "A🦀");
            window.press(REFRESH_KEY, cx);
            assert_eq!(state.read(cx).value(), SAMPLE);
            window.press(undo_key(), cx);
            assert_eq!(state.read(cx).value(), "A🦀");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn pointer_fixture_controls_preserve_preedit_until_cancel(cx: &mut TestAppContext) {
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            let state = view.read(cx).editor.clone();
            window.click(("input", state.entity_id()), cx);
            window.press("secondary-a", cx);
            window.input("A", cx);
            let mut handler = ElementInputHandler::new(
                window.find(("input", state.entity_id())).bounds(),
                state.clone(),
            );
            handler.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx);
            window.click(("load", 1024usize), cx);
            assert_eq!(state.read(cx).value(), "Ani");
            assert_eq!(handler.marked_text_range(window, cx), Some(1..3));
            handler.replace_and_mark_text_in_range(None, "", None, window, cx);
            window.click(("load", 1024usize), cx);
            assert_eq!(state.read(cx).value(), workload(1024, false));
            window.click("load-long-line", cx);
            assert_eq!(
                state.read(cx).text().len(),
                workload(100 * 1024, true).len()
            );
            window.click("refresh-sample", cx);
            assert_eq!(state.read(cx).value(), SAMPLE);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn markdown_grammar_produces_syntax_styles_for_the_actual_editor_buffer(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::component::highlighter::{
            HighlightTheme, LanguageRegistry, SyntaxHighlighter,
        };
        let (handle, view) = mount(cx);
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            let state = view.read(cx).editor.read(cx);
            assert_eq!(state.language_name(), "markdown");
            let registry = LanguageRegistry::singleton();
            assert!(registry.language("markdown").unwrap().has_grammar());
            assert!(registry.language("markdown_inline").unwrap().has_grammar());
            let mut highlighter = SyntaxHighlighter::new("markdown");
            assert!(highlighter.update(None, state.text(), None));
            assert!(highlighter.tree().is_some());
            let styles = highlighter.styles(&(0..SAMPLE.len()), &*HighlightTheme::default_dark());
            assert!(!styles.is_empty());
            assert!(
                styles
                    .iter()
                    .any(|(range, style)| range.start < SAMPLE.find('\n').unwrap()
                        && (style.color.is_some() || style.font_weight.is_some()))
            );
        })
        .unwrap();
    }

    // Diagnostic only: test-platform CPU event dispatch/frame generation is not
    // native input-to-present latency. Kept out of ordinary CI timing gates.
    #[gpui_kit::test]
    #[ignore = "bounded workload measurements; run mise run test:editor-perf"]
    fn large_document_workload(cx: &mut TestAppContext) {
        use std::time::Instant;
        let (handle, view) = mount(cx);
        for (bytes, long_line) in [
            (1024, false),
            (100 * 1024, false),
            (1024 * 1024, false),
            (100 * 1024, true),
        ] {
            let text = workload(bytes, long_line);
            let actual_bytes = text.len();
            let lines = text.lines().count();
            cx.update_window(handle.into(), |_, window, cx| {
                let state = view.read(cx).editor.clone();
                let start = Instant::now();
                state.update(cx, |state, cx| state.set_value(text, window, cx));
                let replace_ms = start.elapsed().as_secs_f64() * 1000.;
                let frame_start = Instant::now();
                window.render_frame(cx);
                let frame_ms = frame_start.elapsed().as_secs_f64() * 1000.;
                window.click(("input", state.entity_id()), cx);
                window.press(if cfg!(target_os = "macos") { "cmd-up" } else { "ctrl-home" }, cx);
                let mut samples = Vec::with_capacity(100);
                for _ in 0..100 {
                    let start = Instant::now();
                    window.input("中", cx);
                    samples.push(start.elapsed().as_secs_f64() * 1000.);
                }
                samples.sort_by(f64::total_cmp);
                println!("EDITOR_WORKLOAD bytes={actual_bytes} lines={lines} long_line={long_line} set_value_ms={replace_ms:.3} frame_ms={frame_ms:.3} headless_input_frame_samples=100 p50_ms={:.3} p95_ms={:.3} max_ms={:.3}", samples[49], samples[94], samples[99]);
                assert_eq!(state.read(cx).value().len(), actual_bytes + 300);
            }).unwrap();
        }
    }
}
