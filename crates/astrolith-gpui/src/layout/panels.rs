use gpui_kit::component::{
    ActiveTheme,
    dock::{BasePanel, Panel, PanelEvent},
    input::{Editor, EditorState},
};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
    SharedString, TestSupportExt, Window, div, prelude::*,
};

pub(crate) struct WorkspacePanel {
    id: &'static str,
    title: SharedString,
    content: SharedString,
    focus: FocusHandle,
    pub editor: Option<Entity<EditorState>>,
    readonly: bool,
    view: Option<gpui_kit::AnyView>,
}

impl WorkspacePanel {
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

    pub fn show_editor(
        &mut self,
        title: String,
        editor: Entity<EditorState>,
        cx: &mut Context<Self>,
    ) {
        self.title = title.into();
        self.editor = Some(editor);
        self.readonly = false;
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    pub fn clear_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
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
            view: None,
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
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }
}
impl Render for WorkspacePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id(self.id)
            .test_support()
            .track_focus(&self.focus)
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .focus_visible(|style| style.border_1().border_color(cx.theme().ring))
            .when_some(self.editor.as_ref(), |view, editor| {
                view.child(
                    Editor::new(editor)
                        .readonly(self.readonly)
                        .bordered(false)
                        .aria_label(if self.readonly {
                            "Document source, readonly"
                        } else {
                            "Document source, editable"
                        })
                        .size_full(),
                )
            })
            .when_some(self.view.as_ref(), |view, content| {
                view.child(content.clone())
            })
            .when(self.editor.is_none() && self.view.is_none(), |view| {
                view.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.content.clone()),
                )
            })
    }
}
