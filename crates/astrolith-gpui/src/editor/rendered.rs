//! Rendered document preview: the active entry's body as a reader sees it.
//!
//! The pane is read-only and independent of the editor: it parses the writer
//! projection (frontmatter and MDX imports already hidden) with the pinned
//! rich-text view, so a Preview mode switch never touches the retained
//! session, its selection or its history.

use astrolith_document::preview_pages::PreviewPages;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use astrolith_document::DocumentKind;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::{
    App, Context, Entity, FocusHandle, ImageSource, IntoElement, Render, Resource, SharedString,
    Window, div, prelude::*, px,
};

use crate::editor::image_preview;
use crate::editor::writing_modes::ProseStyle;

pub(crate) struct RenderedDocument {
    state: Entity<TextViewState>,
    focus: FocusHandle,
    pages: Arc<PreviewPages>,
    page_source: SharedString,
    page_states: HashMap<usize, Entity<TextViewState>>,
    page_subscriptions: HashMap<usize, gpui_kit::Subscription>,
    list: gpui_kit::ListState,
    parse_task: Option<gpui_kit::Task<()>>,
    generation: u64,
    select_all: bool,
    list_width: std::rc::Rc<std::cell::Cell<Option<gpui_kit::Pixels>>>,
    text: SharedString,
    mdx: bool,
    unsupported: bool,
    empty_copy: &'static str,
    style: Option<ProseStyle>,
    /// Project root and document path used to resolve relative images.
    root: Option<PathBuf>,
    document: Option<PathBuf>,
    /// Number of document installs, so a test can prove an unchanged frame
    /// does not copy or re-parse the body.
    #[cfg(test)]
    syncs: usize,
}

impl RenderedDocument {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            state: cx.new(|cx| TextViewState::markdown("", cx)),
            focus: cx.focus_handle(),
            pages: Arc::default(),
            page_source: SharedString::default(),
            page_states: HashMap::new(),
            page_subscriptions: HashMap::new(),
            list: gpui_kit::ListState::new(0, gpui_kit::ListAlignment::Top, px(300.)),
            parse_task: None,
            generation: 0,
            select_all: false,
            list_width: Default::default(),
            text: SharedString::default(),
            mdx: false,
            unsupported: false,
            empty_copy: "Open a Markdown or MDX document to preview it.",
            style: None,
            root: None,
            document: None,
            #[cfg(test)]
            syncs: 0,
        }
    }

    /// Install the reading typography. This never re-parses the document.
    pub(crate) fn set_prose_style(&mut self, style: ProseStyle, cx: &mut Context<Self>) {
        if self.style.as_ref() == Some(&style) {
            return;
        }
        self.style = Some(style);
        cx.notify();
    }

    /// Show the body of the active document, or explain why there is none.
    pub(crate) fn set_document(
        &mut self,
        root: Option<&Path>,
        document: Option<&Path>,
        body: Option<(DocumentKind, &str)>,
        cx: &mut Context<Self>,
    ) {
        let (unsupported, mdx, text, empty_copy) = match body {
            None => (
                false,
                false,
                "",
                "Open a Markdown or MDX document to preview it.",
            ),
            Some((DocumentKind::Markdown, text)) => (false, false, text, "Nothing to preview yet."),
            Some((DocumentKind::Mdx, text)) => (false, true, text, "Nothing to preview yet."),
            Some((DocumentKind::Plain, _)) => (
                true,
                false,
                "",
                "Preview is available for Markdown and MDX documents.",
            ),
        };
        let root_changed = self.root.as_deref() != root;
        let document_changed = self.document.as_deref() != document;
        if self.text.as_ref() == text
            && self.mdx == mdx
            && self.unsupported == unsupported
            && self.empty_copy == empty_copy
            && !root_changed
            && !document_changed
        {
            return;
        }
        self.text = text.into();
        self.mdx = mdx;
        self.unsupported = unsupported;
        self.empty_copy = empty_copy;
        self.root = root.map(Path::to_path_buf);
        self.document = document.map(Path::to_path_buf);
        #[cfg(test)]
        {
            self.syncs += 1;
        }
        self.select_all = false;
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let source = self.text.clone();
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(80))
                .await;
            let work = source.clone();
            let pages = cx
                .background_spawn(async move { PreviewPages::parse(&work, mdx, 8 * 1024) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pages = Arc::new(pages);
                this.page_source = source;
                this.page_states.clear();
                this.page_subscriptions.clear();
                this.list
                    .reset_with_uniform_height(this.pages.ranges.len(), px(4096.));
                if !this.pages.ranges.is_empty() {
                    let text = this.pages.page_source(&this.page_source, 0);
                    this.state.update(cx, |state, cx| state.set_text(&text, cx));
                    this.retain_page(0, this.state.clone(), cx);
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn retain_page(&mut self, index: usize, state: Entity<TextViewState>, cx: &mut Context<Self>) {
        self.page_subscriptions.insert(
            index,
            cx.observe(&state, move |this, _, cx| {
                this.list.remeasure_items(index..index + 1);
                cx.notify();
            }),
        );
        self.page_states.insert(index, state);
    }

    fn render_page(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let state = if let Some(state) = self.page_states.get(&index) {
            state.clone()
        } else {
            let text = self.pages.page_source(&self.page_source, index);
            let state = cx.new(|cx| TextViewState::markdown(&text, cx));
            if self.select_all {
                state.update(cx, |state, cx| state.select_all(cx));
            }
            self.retain_page(index, state.clone(), cx);
            state
        };
        // Retain a bounded neighbourhood of parsed page views while scrolling.
        self.page_states
            .retain(|&page, _| page.abs_diff(index) <= 16);
        self.page_subscriptions
            .retain(|&page, _| page.abs_diff(index) <= 16);
        let style = self.prose_style(cx);
        let root = self.root.clone();
        let document = self.document.clone();
        let pending = state.read(cx).rendered_text().is_empty();
        let mut view = TextView::new(&state)
            .selectable(true)
            .scrollable(false)
            .image_source(move |uri| match (&root, &document) {
                (Some(root), Some(document)) => {
                    preview_image_path(root, document, &uri.to_string())
                        .map(|path| ImageSource::Resource(Resource::Path(path.into())))
                        .unwrap_or_else(|| ImageSource::from(uri.clone()))
                }
                _ => ImageSource::from(uri.clone()),
            })
            .font_family(style.font_family.clone())
            .text_size(px(style.font_size))
            .line_height(gpui_kit::relative(style.line_height))
            .w_full();
        if self.mdx {
            view = view.markdown_mdx();
        }
        div()
            .w_full()
            .min_h(if pending {
                window.viewport_size().height
            } else {
                px(0.)
            })
            .child(view)
            .into_any_element()
    }

    fn prose_style(&self, cx: &App) -> ProseStyle {
        self.style.clone().unwrap_or_else(|| {
            ProseStyle::new(
                crate::theme::prose_font(cx),
                16.,
                2.,
                crate::theme::default_prose_page_width_percent(),
            )
        })
    }

    /// Number of document installs performed, for the per-frame cost test.
    #[cfg(test)]
    pub(crate) fn sync_count(&self) -> usize {
        self.syncs
    }

    /// The Markdown source the reading pane currently shows.
    #[cfg(test)]
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Give the reading pane the keyboard so the hidden editor never receives
    /// typing after a mode switch.
    pub(crate) fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RenderedDocument {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.unsupported || self.text.is_empty() {
            return div()
                .size_full()
                .child(crate::ui::empty_state(
                    "preview-empty",
                    gpui_kit::assets::IconName::Eye,
                    "Nothing to preview",
                    self.empty_copy,
                    None,
                    cx,
                ))
                .into_any_element();
        }
        let style = self.prose_style(cx);
        let list_state = self.list.clone();
        let list_width = self.list_width.clone();
        let owner = cx.weak_entity();
        let view = gpui_kit::list(
            self.list.clone(),
            cx.processor(|this, index, window, cx| this.render_page(index, window, cx)),
        )
        .size_full();
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("TextView")
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::SelectAll, _, cx| {
                    this.select_all = true;
                    for state in this.page_states.values() {
                        state.update(cx, |state, cx| state.select_all(cx));
                    }
                    cx.stop_propagation();
                },
            ))
            .capture_action(
                cx.listener(|this, _: &gpui_kit::component::input::Copy, _, cx| {
                    if this.select_all {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                            this.text.to_string(),
                        ));
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if this.select_all {
                    this.select_all = false;
                    for state in this.page_states.values() {
                        state.update(cx, |state, cx| state.clear_selection(cx));
                    }
                }
            }))
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div().size_full().h_flex().justify_center().child(
                    div()
                        .h_full()
                        .w(gpui_kit::relative(style.page_width_percent / 100.))
                        .min_w(px(320.))
                        .max_w(px(1200.))
                        .px(px(crate::theme::PROSE_INSET))
                        .py(px(crate::theme::PROSE_TOP))
                        .child(div().size_full().child(view).on_children_prepainted(
                            move |bounds, _, cx| {
                                let Some(bounds) = bounds.first() else {
                                    return;
                                };
                                if list_width.replace(Some(bounds.size.width))
                                    != Some(bounds.size.width)
                                {
                                    // Upstream clears height hints on width changes. Restore
                                    // estimates after that layout, using only public APIs.
                                    list_state.clone().with_uniform_item_height(px(4096.));
                                    let owner = owner.clone();
                                    cx.defer(move |cx| {
                                        let _ = owner.update(cx, |_, cx| cx.notify());
                                    });
                                }
                            },
                        )),
                ),
            )
            .into_any_element()
    }
}

/// Resolve one Markdown image destination to a file inside the project.
///
/// Astro serves a leading `/` from `public/`; everything else is relative to
/// the document. Resolution is lexical and refuses paths that leave the
/// project. The returned path is loaded by GPUI's asset cache off the UI
/// thread, so painting never performs file I/O.
fn preview_image_path(root: &Path, document: &Path, raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains("://") || raw.starts_with("//") || raw.contains(['\\', ':']) {
        return None;
    }
    let candidate = match raw.strip_prefix('/') {
        Some(rest) => root.join("public").join(rest),
        None => document.parent()?.join(raw),
    };
    let normalized = image_preview::normalize_lexical(&candidate)?;
    normalized.starts_with(root).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt;

    #[gpui_kit::test]
    fn source_long_line_keeps_all_unicode_and_edits_at_the_end_after_resize(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::input::{Editor, EditorState};
        use gpui_kit::{Focusable, IntoElement, Render, Styled};
        struct SourceHost(gpui_kit::Entity<EditorState>);
        impl Render for SourceHost {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                crate::editor::source_viewport(
                    self.0.clone(),
                    Editor::new(&self.0).size_full().into_any_element(),
                )
            }
        }
        let source = "中文🦀 é العربية **bold** `code` ".repeat(1_000);
        let (handle, host) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx)
                        .language("markdown")
                        .soft_wrap(false)
                        .default_value(source.clone())
                });
                cx.new(|_| SourceHost(editor))
            })
            .unwrap()
        });
        let editor = host.read_with(cx, |host, _| host.0.clone());
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(source.len()..source.len(), cx);
                window.focus(&editor.focus_handle(cx), cx);
            });
            window.render_frame(cx);
            window.render_frame(cx);
            window.input("终", cx);
        })
        .unwrap();
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(600.), gpui_kit::px(500.)),
        );
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            let editor = editor.read(cx);
            assert_eq!(editor.value().as_ref(), format!("{source}终"));
            assert_eq!(editor.cursor(), source.len() + "终".len());
            let geometry = editor
                .prepaint_caret_geometry()
                .expect("visible native input caret");
            let mut caret = geometry.cursor_bounds();
            caret.origin.y += geometry.scroll_offset().y;
            assert!(
                editor.input_bounds().intersects(&caret),
                "input {:?}, caret {:?}, scroll {:?}",
                editor.input_bounds(),
                caret,
                editor.scroll_offset()
            );
        })
        .unwrap();
    }

    #[gpui_kit::test]
    async fn preview_pages_leave_tail_unmeasured_and_keep_it_reachable_after_resize(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.executor().allow_parking();
        let source = "A paragraph with **bold** and `code`.\n\n".repeat(2_000);
        let (handle, preview) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                cx.new(RenderedDocument::new)
            })
            .unwrap()
        });
        preview.update(cx, |preview, cx| {
            preview.set_document(None, None, Some((DocumentKind::Markdown, &source)), cx);
        });
        let state = preview.read_with(cx, |preview, _| preview.state.clone());
        cx.condition(&state, |state, _| !state.rendered_text().is_empty())
            .await;
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.update_window(handle, |_, window, cx| {
            let focus = preview.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            window.render_frame(cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-a"
                } else {
                    "ctrl-a"
                },
                cx,
            );
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-c"
                } else {
                    "ctrl-c"
                },
                cx,
            );
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some(source.clone())
            );
        })
        .unwrap();
        let list = preview.read_with(cx, |preview, _| preview.list.clone());
        let last = list.item_count() - 1;
        assert!(
            last > 1,
            "long documents have multiple bounded source pages"
        );
        assert!(
            list.bounds_for_item(last).is_none(),
            "first layout must not measure the document tail"
        );
        assert!(
            list.max_offset_for_scrollbar().y > gpui_kit::px(0.),
            "estimated blocks keep a usable scroll range"
        );
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(700.)),
        );
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        assert!(
            list.bounds_for_item(last).is_none(),
            "resize must not measure the document tail"
        );
        assert!(list.max_offset_for_scrollbar().y > gpui_kit::px(0.));
        list.scroll_to_reveal_item(last);
        preview.update(cx, |_, cx| cx.notify());
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
        assert!(
            list.bounds_for_item(last).is_some(),
            "the last paragraph remains reachable"
        );
    }

    #[test]
    fn images_resolve_from_the_document_or_astro_public() {
        let root = Path::new("/project");
        let document = Path::new("/project/src/content/blog/post.md");
        assert_eq!(
            preview_image_path(root, document, "./cover.png"),
            Some(PathBuf::from("/project/src/content/blog/cover.png"))
        );
        assert_eq!(
            preview_image_path(root, document, "art/banner.svg"),
            Some(PathBuf::from("/project/src/content/blog/art/banner.svg"))
        );
        assert_eq!(
            preview_image_path(root, document, "/parity-banner.svg"),
            Some(PathBuf::from("/project/public/parity-banner.svg"))
        );
        assert_eq!(
            preview_image_path(root, document, "../../../../etc/passwd.png"),
            None,
            "a traversal outside the project never loads"
        );
        assert_eq!(
            preview_image_path(root, document, "https://x.dev/a.png"),
            None
        );
    }
}
