use std::sync::Arc;
use std::time::Instant;

use crate::editor::formatting::EditorWiring;
use crate::editor::image_preview::{self, ImagePreviewState, PreviewSource};
use crate::editor::selection_toolbar;
use crate::editor::writing_modes::ProseStyle;
use crate::runtime;
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, StyledExt,
    dock::{BasePanel, Panel, PanelEvent},
    input::{Editor, EditorState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable,
    IntoElement, ModifiersChangedEvent, MouseButton, MouseMoveEvent, ObjectFit, Render,
    SharedString, Task, TestSupportExt, Window, div, img, prelude::*, px,
};

pub(crate) struct WorkspacePanel {
    id: &'static str,
    title: SharedString,
    content: SharedString,
    /// Translation key of the placeholder title, or `None` when a document
    /// file name owns the title.
    title_key: Option<&'static str>,
    /// Translation key of the empty-state body, or `None` for live copy.
    content_key: Option<&'static str>,
    focus: FocusHandle,
    pub editor: Option<Entity<EditorState>>,
    readonly: bool,
    header: Option<gpui_kit::AnyView>,
    view: Option<gpui_kit::AnyView>,
    /// Live-session wiring: paste/link hooks and the selection toolbar target.
    wiring: Option<EditorWiring>,
    /// Focus handle of the selection toolbar, so focusing one of its buttons
    /// keeps the overlay mounted and reachable.
    pub(crate) toolbar_focus: FocusHandle,
    /// Configured document face, size, line height and measure.
    prose: Option<ProseStyle>,
    /// The panel paints a source-code editor instead of the writer page.
    code: bool,
    /// Whether this panel is navigation chrome and uses the sidebar surface.
    surface: bool,
    /// Alt-hover preview and its in-flight request.
    preview: ImagePreviewState,
    preview_source: Option<PreviewSource>,
    preview_started: Option<Instant>,
    preview_task: Option<Task<()>>,
}

/// Whether a hover over `source` needs a new load.
///
/// A failed load leaves the source recorded with no image, so repeating the
/// same hover must not re-dispatch; moving to another source or dismissing
/// resets it.
fn needs_preview_request(current: Option<&PreviewSource>, source: &PreviewSource) -> bool {
    current != Some(source)
}

impl WorkspacePanel {
    /// Focus the selection toolbar from the registry, so it is reachable by
    /// keyboard even though the editor traps Tab for indentation.
    pub fn focus_selection_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toolbar_focus.focus(window, cx);
        cx.notify();
    }

    #[cfg(test)]
    pub fn is_editable(&self) -> bool {
        self.editor.is_some() && !self.readonly
    }

    pub fn show_source(
        &mut self,
        source: crate::file::DocumentSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.readonly = true;
        self.wiring = None;
        self.title_key = None;
        self.title = source
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
            .into();
        {
            // A readonly file is a new snapshot, not an edit of the previous
            // document. Initial content avoids the full replacement edit path.
            self.editor = Some(cx.new(|cx| {
                EditorState::new(window, cx)
                    .line_number(false)
                    .indent_guides(false)
                    .folding(false)
                    .default_value(source.source)
            }));
        }
        if let Some(editor) = self.editor.as_ref() {
            crate::editor::hide_scrollbar(editor, cx);
        }
        self.apply_editor_chrome(cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    pub fn set_view(&mut self, view: gpui_kit::AnyView, cx: &mut Context<Self>) {
        self.view = Some(view);
        cx.notify();
    }

    /// Install chrome drawn above the panel body (for example document tabs).
    pub fn set_header(&mut self, header: gpui_kit::AnyView, cx: &mut Context<Self>) {
        self.header = Some(header);
        cx.notify();
    }

    pub fn show_editor(
        &mut self,
        title: String,
        editor: Entity<EditorState>,
        wiring: Option<EditorWiring>,
        cx: &mut Context<Self>,
    ) {
        self.title_key = None;
        self.title = title.into();
        self.editor = Some(editor);
        self.readonly = false;
        self.wiring = wiring;
        if let Some(wiring) = &self.wiring
            && let (Some(root), Some(session)) = (&wiring.preview_root, wiring.session.upgrade())
        {
            session.update(cx, |session, cx| {
                session.set_writer_image_root(root.as_ref().clone(), cx)
            });
        }
        if let (Some(style), Some(session)) = (
            self.prose.clone(),
            self.wiring
                .as_ref()
                .and_then(|wiring| wiring.session.upgrade()),
        ) {
            session.update(cx, |session, cx| session.set_prose_style(style, cx));
        }
        self.apply_editor_chrome(cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Install the configured document typography.
    ///
    /// This only changes how the existing editor is painted; the editor's text,
    /// caret, selection and history are untouched.
    pub fn set_prose_style(&mut self, style: ProseStyle, cx: &mut Context<Self>) {
        if self.prose.as_ref() == Some(&style) {
            return;
        }
        self.prose = Some(style.clone());
        if let Some(session) = self
            .wiring
            .as_ref()
            .and_then(|wiring| wiring.session.upgrade())
        {
            session.update(cx, |session, cx| session.set_prose_style(style, cx));
        }
        cx.notify();
    }

    /// Paint this panel as an IDE source editor instead of the writer page.
    ///
    /// The editor entity is shared with the writer, so this switches only what
    /// the panel contributes: the full-width code surface, the visible
    /// scrollbar, and the absence of writer chrome.
    pub fn set_code(&mut self, code: bool, cx: &mut Context<Self>) {
        if self.code == code {
            return;
        }
        self.code = code;
        self.apply_editor_chrome(cx);
        cx.notify();
    }

    /// Match the editor's scrollbar policy to the panel presentation.
    fn apply_editor_chrome(&self, cx: &mut App) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        if self.code {
            editor.update(cx, |editor, _| editor.set_scrollbar_renderer(None));
        } else {
            crate::editor::hide_scrollbar(editor, cx);
        }
    }

    pub fn clear_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.wiring = None;
        self.title_key = Some("shell.panel.document");
        self.content_key = Some("shell.empty.document");
        self.title = tr!("shell.panel.document").into();
        self.content = tr!("shell.empty.document").into();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    pub fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content_key = None;
        if self.content.as_ref() == content {
            return;
        }
        self.content = content.into();
        cx.notify();
    }

    /// Show a localized empty-state body from its translation key.
    pub fn set_content_key(&mut self, key: &'static str, cx: &mut Context<Self>) {
        self.content_key = Some(key);
        let content = tr!(key);
        if self.content.as_ref() == content.as_str() {
            return;
        }
        self.content = content.into();
        cx.notify();
    }

    /// Mark the title and empty-state body as placeholder copy that follows
    /// the active locale.
    ///
    /// Opening a document clears the title key, so a language change rewrites
    /// placeholder copy only and never a live document's file name.
    pub(crate) fn with_placeholders(
        mut self,
        title: Option<&'static str>,
        content: Option<&'static str>,
    ) -> Self {
        if let Some(key) = title {
            self.title_key = Some(key);
            self.title = tr!(key).into();
        }
        if let Some(key) = content {
            self.content_key = Some(key);
            self.content = tr!(key).into();
        }
        self
    }

    /// Rewrite localized placeholder copy in the active locale.
    pub fn retranslate(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        if let Some(key) = self.title_key {
            let title = tr!(key);
            if self.title.as_ref() != title.as_str() {
                self.title = title.into();
                changed = true;
            }
        }
        if let Some(key) = self.content_key {
            let content = tr!(key);
            if self.content.as_ref() != content.as_str() {
                self.content = content.into();
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
    }

    pub fn new(
        id: &'static str,
        title: impl Into<SharedString>,
        content: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let toolbar_focus = cx.focus_handle().tab_stop(true);
        Self {
            id,
            title: title.into(),
            content: content.into(),
            title_key: None,
            content_key: None,
            focus: cx.focus_handle().tab_stop(true),
            editor: None,
            readonly: true,
            header: None,
            view: None,
            wiring: None,
            toolbar_focus,
            prose: None,
            code: false,
            surface: false,
            preview: ImagePreviewState::default(),
            preview_source: None,
            preview_started: None,
            preview_task: None,
        }
    }

    /// Paint this panel with the navigation surface instead of the canvas.
    pub(crate) fn with_surface(mut self) -> Self {
        self.surface = true;
        self
    }

    /// Hide the preview and cancel its request.
    fn dismiss_preview(&mut self, cx: &mut Context<Self>) {
        let visible = self.preview.is_visible() || self.preview_source.is_some();
        self.preview.dismiss();
        self.preview_source = None;
        self.preview_started = None;
        self.preview_task = None;
        if visible {
            cx.notify();
        }
    }

    /// Resolve the image under the pointer and start a bounded load.
    fn hover_image(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !event.modifiers.alt {
            self.dismiss_preview(cx);
            return;
        }
        let Some(wiring) = self.wiring.as_ref() else {
            return;
        };
        let Some(root) = wiring.preview_root.clone() else {
            return;
        };
        let Some(session) = wiring.session.upgrade() else {
            return;
        };
        let destination = {
            let session = session.read(cx);
            session.image_destination_at(event.position, cx)
        };
        let document = {
            let session = session.read(cx);
            root.join(session.canonical_source())
        };
        let source = destination
            .as_deref()
            .and_then(|destination| image_preview::resolve_preview(&root, &document, destination));
        let Some(source) = source else {
            self.dismiss_preview(cx);
            return;
        };
        if !needs_preview_request(self.preview_source.as_ref(), &source) {
            return;
        }
        let generation = self.preview.begin();
        self.preview_source = Some(source.clone());
        self.preview_started = Some(Instant::now());
        let task = match source {
            PreviewSource::Local(path) => {
                let load = runtime::blocking(cx, move || image_preview::load_local(&root, &path));
                cx.spawn_in(window, async move |this, cx| {
                    let result = load.await.ok().and_then(Result::ok);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.finish_preview(generation, result, window, cx)
                    });
                })
            }
            PreviewSource::Remote(url) => {
                let load = runtime::blocking(cx, move || image_preview::load_remote(&url));
                cx.spawn_in(window, async move |this, cx| {
                    let result = load.await.ok().and_then(Result::ok);
                    let _ = this.update_in(cx, |this, window, cx| {
                        this.finish_preview(generation, result, window, cx)
                    });
                })
            }
        };
        self.preview_task = Some(task);
        cx.notify();
    }

    fn finish_preview(
        &mut self,
        generation: u64,
        result: Option<Arc<gpui_kit::RenderImage>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview.generation() != generation {
            return;
        }
        self.preview_task = None;
        let started = self.preview_started.take().unwrap_or_else(Instant::now);
        let accepted = self
            .preview
            .accept(generation, started, Instant::now(), result);
        if !accepted {
            self.preview_source = None;
        }
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for WorkspacePanel {}
impl Focusable for WorkspacePanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor
            .as_ref()
            .map(|editor| editor.read(cx).focus_handle(cx))
            .unwrap_or_else(|| self.focus.clone())
    }
}
impl BasePanel for WorkspacePanel {
    fn panel_name(&self) -> &'static str {
        self.id
    }
    fn closable(&self, _: &App) -> bool {
        false
    }
    fn zoomable(&self, _: &App) -> bool {
        false
    }
}
impl Panel for WorkspacePanel {
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(self.title.clone())
    }
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }
    fn title_bar(&self, _: &App) -> bool {
        // Every panel carries its own editorial header; the dock must not add
        // a second, IDE-like tab strip above it.
        false
    }
}
impl Render for WorkspacePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.id)
            .test_support()
            .track_focus(&self.focus)
            .v_flex()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(if self.surface {
                cx.theme().sidebar
            } else {
                cx.theme().background
            })
            .text_color(cx.theme().foreground)
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                if !event.modifiers.alt {
                    this.dismiss_preview(cx);
                }
            }))
            .when_some(
                self.header.as_ref().filter(|_| !self.code),
                |view, header| view.child(header.clone()),
            )
            .when_some(self.editor.as_ref(), |view, editor| {
                let editor = editor.clone();
                let style = self.prose.clone();
                let code = self.code;
                let page_width_percent = style
                    .as_ref()
                    .map(|style| style.page_width_percent)
                    .unwrap_or(crate::theme::default_prose_page_width_percent());
                let wiring = self.wiring.clone();
                let overlay = wiring.as_ref().and_then(|wiring| {
                    // Focus on the document, the toolbar or one of its buttons
                    // keeps the popover mounted, so Tab can walk its controls.
                    if code {
                        return None;
                    }
                    let focused = editor.read(cx).focus_handle(cx).is_focused(window)
                        || self.toolbar_focus.contains_focused(window, cx);
                    selection_toolbar::overlay(
                        &editor,
                        wiring.session.clone(),
                        &self.toolbar_focus,
                        focused,
                        cx,
                    )
                });
                let preview_image = self.preview.image();
                let mut element = Editor::new(&editor)
                    .readonly(self.readonly)
                    .bordered(false)
                    .aria_label(if code {
                        tr!("shell.editor.source_file_editable")
                    } else if self.readonly {
                        tr!("shell.editor.document_readonly")
                    } else {
                        tr!("shell.editor.document_editable")
                    })
                    .size_full();
                if code {
                    // The editor supplies its own mono face, gutter and code
                    // metrics; the panel only gives it the full pane.
                } else {
                    element = element.font_family(
                        style
                            .as_ref()
                            .map(|style| style.font_family.clone())
                            .unwrap_or_else(|| crate::theme::prose_font(cx)),
                    );
                    if let Some(style) = style.as_ref() {
                        element = element
                            .text_size(px(style.font_size))
                            .line_height(gpui_kit::relative(style.line_height));
                    }
                }
                if let Some(wiring) = wiring.as_ref() {
                    let paste = wiring.hooks.paste.clone();
                    element = element.on_paste(move |item, window, cx| (paste)(item, window, cx));
                }
                let writer = (!code)
                    .then(|| wiring.as_ref().and_then(|wiring| wiring.session.upgrade()))
                    .flatten()
                    .map(|session| session.read(cx).writer.clone());
                let content = match writer {
                    Some(surface) => surface.into_any_element(),
                    None if code => {
                        let surface = wiring
                            .as_ref()
                            .and_then(|wiring| wiring.session.upgrade())
                            .and_then(|session| session.read(cx).source_surface().cloned());
                        match surface {
                            Some(surface) => surface.into_any_element(),
                            None => crate::editor::source_viewport(
                                editor.clone(),
                                crate::editor::perf::build_frame_element(
                                    element.into_any_element(),
                                ),
                            ),
                        }
                    }
                    None => element.into_any_element(),
                };
                let mut body = if code {
                    div().flex_1().min_h_0().relative().child(content)
                } else {
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .h_flex()
                        .justify_center()
                        .child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative(page_width_percent / 100.))
                                .min_w(px(320.))
                                .max_w(px(1200.))
                                .px(px(crate::theme::PROSE_INSET))
                                .pt(px(crate::theme::PROSE_TOP))
                                .child(content),
                        )
                };
                if let Some(hooks) = wiring.as_ref().map(|wiring| wiring.hooks.clone()) {
                    let mouse_down = hooks.mouse_down.clone();
                    body = body.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        (mouse_down)(event, window, cx)
                    });
                    if !code {
                        body = body.on_mouse_move(cx.listener(
                            |this, event: &MouseMoveEvent, window, cx| {
                                this.hover_image(event, window, cx)
                            },
                        ));
                        body = body
                            .on_mouse_exit(cx.listener(|this, _, _, cx| this.dismiss_preview(cx)));
                    }
                    if let Some(drop) = hooks.drop.clone() {
                        body = body.on_drop(move |paths: &ExternalPaths, window, cx| {
                            (drop)(paths, window, cx)
                        });
                    }
                }
                if let Some(image) = preview_image.filter(|_| !code) {
                    body = body.child(
                        div()
                            .absolute()
                            .bottom(px(12.))
                            .right(px(12.))
                            .p_1()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().window_border)
                            .bg(cx.theme().background)
                            .child(
                                img(image)
                                    .max_w(px(360.))
                                    .max_h(px(240.))
                                    .object_fit(ObjectFit::Contain),
                            ),
                    );
                }
                view.child(body.when_some(overlay, |body, overlay| body.child(overlay)))
            })
            .when_some(self.view.as_ref(), |view, content| {
                view.child(div().flex_1().min_h_0().child(content.clone()))
            })
            .when(self.editor.is_none() && self.view.is_none(), |view| {
                view.child(crate::ui::empty_state(
                    "document-empty",
                    IconName::NotebookPen,
                    tr!("shell.panel.no_file_open"),
                    self.content.clone(),
                    None,
                    cx,
                ))
            })
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    use gpui_kit::test::TestWindowExt;

    #[gpui_kit::test]
    async fn typography_changes_leave_the_document_state_untouched(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (handle, panel, editor) = cx.update(|cx| {
            gpui_kit::init(cx);
            let mut panel = None;
            let mut editor = None;
            let (handle, _) =
                gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                    let state =
                        cx.new(|cx| EditorState::new(window, cx).default_value("alpha\nbeta\n"));
                    state.update(cx, |state, cx| state.set_selected_range(2..6, cx));
                    let view = cx.new(|cx| {
                        WorkspacePanel::new("document-panel", "Document", "", window, cx)
                    });
                    view.update(cx, |view, cx| {
                        view.show_editor("doc.md".into(), state.clone(), None, cx)
                    });
                    panel = Some(view.clone());
                    editor = Some(state);
                    view
                })
                .unwrap();
            (handle, panel.unwrap(), editor.unwrap())
        });
        let style = ProseStyle::new("Inter".into(), 22., 2.6, 900.);
        cx.update_window(handle, |_, window, cx| {
            panel.update(cx, |panel, cx| panel.set_prose_style(style.clone(), cx));
            window.render_frame(cx);
            assert_eq!(editor.read(cx).value(), "alpha\nbeta\n");
            assert_eq!(editor.read(cx).selected_range(), 2..6);
            assert_eq!(editor.read(cx).cursor(), 6);
        })
        .unwrap();
    }

    #[test]
    fn a_failed_preview_is_not_reloaded_for_the_same_source() {
        let mut preview = ImagePreviewState::default();
        let source = PreviewSource::Local(std::path::PathBuf::from("/project/src/assets/x.png"));
        let generation = preview.begin();
        let started = Instant::now();
        // A failure is accepted with no image and keeps the source recorded.
        assert!(preview.accept(generation, started, Instant::now(), None));
        assert!(!preview.is_visible());
        assert!(!needs_preview_request(Some(&source), &source));
        assert!(needs_preview_request(
            Some(&source),
            &PreviewSource::Local("/project/src/assets/y.png".into())
        ));
        assert!(needs_preview_request(None, &source));
    }
}
