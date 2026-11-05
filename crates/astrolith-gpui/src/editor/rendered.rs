//! Rendered document preview: the active entry's body as a reader sees it.
//!
//! The pane is read-only and independent of the editor: it parses the writer
//! projection (frontmatter and MDX imports already hidden) with the pinned
//! rich-text view, so a Preview mode switch never touches the retained
//! session, its selection or its history.

use std::path::{Path, PathBuf};

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
        self.state.update(cx, |state, cx| state.set_text(text, cx));
        cx.notify();
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
    pub(crate) fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle().clone()
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
        let style = self.style.clone().unwrap_or_else(|| {
            ProseStyle::new(
                crate::theme::prose_font(cx),
                16.,
                2.,
                crate::theme::default_prose_page_width_percent(),
            )
        });
        let root = self.root.clone();
        let document = self.document.clone();
        let mut view = TextView::new(&self.state)
            .selectable(true)
            .scrollable(true)
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
            .size_full();
        if self.mdx {
            view = view.markdown_mdx();
        }
        div()
            .size_full()
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
                        .child(view),
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
