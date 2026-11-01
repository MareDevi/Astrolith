use crate::editor::formatting::EditorWiring;
use crate::editor::selection_toolbar;
use crate::editor::writing_modes::{ProseTypography, prose_typography};
use gpui_kit::assets::IconName;
use gpui_kit::component::{
    ActiveTheme, Icon, Sizable, StyledExt,
    dock::{BasePanel, Panel, PanelEvent},
    input::{Editor, EditorState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    MouseButton, Render, SharedString, TestSupportExt, Window, div, prelude::*, px,
};

pub(crate) struct WorkspacePanel {
    id: &'static str,
    title: SharedString,
    content: SharedString,
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
    /// Base prose size from preferences and the document pane width.
    prose: Option<(f32, f32)>,
}

impl WorkspacePanel {
    /// Focus the selection toolbar from the registry, so it is reachable by
    /// keyboard even though the editor traps Tab for indentation.
    pub fn focus_selection_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toolbar_focus.focus(window, cx);
        cx.notify();
    }

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
                    .language("markdown")
                    .default_value(source.source)
            }));
        }
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
        self.title = title.into();
        self.editor = Some(editor);
        self.readonly = false;
        self.wiring = wiring;
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Resolve responsive prose typography from the base size and pane width.
    pub fn set_prose_width(
        &mut self,
        base_font_size: f32,
        pane_width: f32,
        cx: &mut Context<Self>,
    ) {
        let next = (base_font_size, pane_width);
        if self.prose == Some(next) {
            return;
        }
        self.prose = Some(next);
        cx.notify();
    }

    pub fn clear_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        self.wiring = None;
        self.title = "Document".into();
        self.content = "Select a collection entry to edit its source.".into();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    pub fn set_content(&mut self, content: String, cx: &mut Context<Self>) {
        self.content = content.into();
        cx.notify();
    }

    pub fn new(
        id: &'static str,
        title: &'static str,
        content: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let toolbar_focus = cx.focus_handle().tab_stop(true);
        let editor = (id == "document-panel").then(|| {
            cx.new(|cx| {
                EditorState::new(window, cx)
                    .language("markdown")
                    .default_value(content)
            })
        });
        Self {
            id,
            title: title.into(),
            content: content.into(),
            focus: cx.focus_handle().tab_stop(true),
            editor,
            readonly: true,
            header: None,
            view: None,
            wiring: None,
            toolbar_focus,
            prose: None,
        }
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
        let icon = match self.id {
            "navigator-panel" => IconName::FolderTree,
            "inspector-panel" => IconName::SlidersHorizontal,
            "preview-panel" => IconName::Globe,
            _ => IconName::FileText,
        };
        div()
            .h_flex()
            .items_center()
            .gap_1p5()
            .child(
                Icon::new(icon)
                    .small()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(self.title.clone())
    }
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }
    fn title_bar(&self, _: &App) -> bool {
        // The document panel draws its own Zed-style tab strip and breadcrumb.
        self.id != "document-panel"
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
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .focus_visible(|style| style.border_1().border_color(cx.theme().ring))
            .when_some(self.header.as_ref(), |view, header| {
                view.child(header.clone())
            })
            .when_some(self.editor.as_ref(), |view, editor| {
                let editor = editor.clone();
                let typography = self
                    .prose
                    .map(|(base, width)| prose_typography(width, base));
                let wiring = self.wiring.clone();
                let overlay = wiring.as_ref().and_then(|wiring| {
                    // Focus on the document, the toolbar or one of its buttons
                    // keeps the popover mounted, so Tab can walk its controls.
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
                let mut element = Editor::new(&editor)
                    .readonly(self.readonly)
                    .bordered(false)
                    .aria_label(if self.readonly {
                        "Document source, readonly"
                    } else {
                        "Document source, editable"
                    })
                    .size_full();
                if let Some(ProseTypography {
                    font_size,
                    line_height,
                }) = typography
                {
                    element = element
                        .text_size(px(font_size))
                        .line_height(gpui_kit::relative(line_height));
                }
                if let Some(wiring) = wiring.as_ref() {
                    let paste = wiring.hooks.paste.clone();
                    element = element.on_paste(move |item, window, cx| (paste)(item, window, cx));
                }
                let mut body = div().flex_1().min_h_0().child(element);
                if let Some(hooks) = wiring.as_ref().map(|wiring| wiring.hooks.clone()) {
                    let mouse_down = hooks.mouse_down.clone();
                    body = body.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                        (mouse_down)(event, window, cx)
                    });
                }
                view.child(body.when_some(overlay, |body, overlay| body.child(overlay)))
            })
            .when_some(self.view.as_ref(), |view, content| {
                view.child(div().flex_1().min_h_0().child(content.clone()))
            })
            .when(self.editor.is_none() && self.view.is_none(), |view| {
                view.child(
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.content.clone()),
                )
            })
    }
}
