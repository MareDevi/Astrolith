//! Rendered document preview: the active entry's body as a reader sees it.
//!
//! The pane parses the reader's projection, lays it out with the shared
//! viewport text layout, and keeps its own selection,
//! focus and scroll anchor. It never mounts a Kit `TextView`, never splits the
//! source into pages and never copies the body on an unchanged frame.

use astrolith_document::{BlockKind, DocumentKind, WysiwygDocument};
use gpui_kit::base::AutoScroll;
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::{
    App, Bounds, Context, Corners, Element, ElementId, FocusHandle, GlobalElementId,
    ImageCacheItem, ImageSource, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, Pixels, Point, Render, Resource, ScrollHandle, SharedString, Window, div,
    point, prelude::*, px, size,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::editor::image_preview;
use crate::editor::text_layout::{
    Hit, ImageMetrics, LayoutImagePainter, LayoutStyle, Selection, TextLayout, Viewport,
};
use crate::editor::writing_modes::ProseStyle;

/// Why the reading pane has no body to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewEmpty {
    /// No document is active.
    NoDocument,
    /// The active document has no body yet.
    NothingYet,
    /// The active document is not Markdown or MDX.
    Unsupported,
}

impl PreviewEmpty {
    /// The explanation under the empty-state title.
    fn message(self) -> SharedString {
        match self {
            Self::NoDocument => tr!("editor.preview.no_document").into(),
            Self::NothingYet => tr!("editor.preview.nothing_yet").into(),
            Self::Unsupported => tr!("editor.preview.unsupported").into(),
        }
    }
}

pub(crate) struct RenderedDocument {
    focus: FocusHandle,
    /// Accepted semantic document, shared with the writer when possible.
    document: Arc<WysiwygDocument>,
    layout: Rc<RefCell<Option<TextLayout>>>,
    layout_style: Option<Arc<LayoutStyle>>,
    laid_out_width: Option<f32>,
    geometry: Rc<RefCell<FrameGeometry>>,
    scroll: ScrollHandle,
    selection: Option<Selection>,
    drag_anchor: Option<Hit>,
    auto_scroll: AutoScroll,
    /// Source offset and in-block pixel offset to restore after an update.
    pending_anchor: Option<(usize, f32)>,
    parse_task: Option<gpui_kit::Task<()>>,
    generation: u64,
    select_all: bool,
    text: SharedString,
    mdx: bool,
    unsupported: bool,
    empty_copy: PreviewEmpty,
    style: Option<ProseStyle>,
    /// Project root and document path used to resolve relative images.
    root: Option<PathBuf>,
    document_path: Option<PathBuf>,
    images: Rc<HashMap<String, ImageCacheItem>>,
    image_sizes: ImageMetrics,
    /// Number of document installs, so a test can prove an unchanged frame
    /// does not copy or re-parse the body.
    #[cfg(test)]
    syncs: usize,
}

#[derive(Clone, Copy)]
struct FrameGeometry {
    origin: Point<Pixels>,
    viewport: Bounds<Pixels>,
    width: f32,
    scroll_y: f32,
}

impl Default for FrameGeometry {
    fn default() -> Self {
        Self {
            origin: point(px(0.), px(0.)),
            viewport: Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.))),
            width: 0.,
            scroll_y: 0.,
        }
    }
}

impl RenderedDocument {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            document: Arc::new(WysiwygDocument::default()),
            layout: Rc::new(RefCell::new(None)),
            layout_style: None,
            laid_out_width: None,
            geometry: Rc::new(RefCell::new(FrameGeometry::default())),
            scroll: ScrollHandle::new(),
            selection: None,
            drag_anchor: None,
            auto_scroll: AutoScroll::default(),
            pending_anchor: None,
            parse_task: None,
            generation: 0,
            select_all: false,
            text: SharedString::default(),
            mdx: false,
            unsupported: false,
            empty_copy: PreviewEmpty::NoDocument,
            style: None,
            root: None,
            document_path: None,
            images: Rc::new(HashMap::new()),
            image_sizes: ImageMetrics::new(),
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
    ///
    /// `shared` is the writer's accepted parse of the same visible text; using
    /// it keeps Writing and Preview from parsing one revision twice.
    pub(crate) fn set_document(
        &mut self,
        root: Option<&Path>,
        document: Option<&Path>,
        body: Option<(DocumentKind, &str)>,
        cx: &mut Context<Self>,
    ) {
        let (unsupported, mdx, text, empty_copy) = match body {
            None => (false, false, "", PreviewEmpty::NoDocument),
            Some((DocumentKind::Markdown, text)) => (false, false, text, PreviewEmpty::NothingYet),
            Some((DocumentKind::Mdx, text)) => (false, true, text, PreviewEmpty::NothingYet),
            Some((DocumentKind::Plain, _)) => (true, false, "", PreviewEmpty::Unsupported),
        };
        let root_changed = self.root.as_deref() != root;
        let document_changed = self.document_path.as_deref() != document;
        if self.text.as_ref() == text
            && self.mdx == mdx
            && self.unsupported == unsupported
            && self.empty_copy == empty_copy
            && !root_changed
            && !document_changed
        {
            return;
        }
        if let Some(anchor) = self.scroll_anchor() {
            self.pending_anchor = Some(anchor);
        }
        self.text = text.into();
        self.mdx = mdx;
        self.unsupported = unsupported;
        self.empty_copy = empty_copy;
        self.root = root.map(Path::to_path_buf);
        self.document_path = document.map(Path::to_path_buf);
        #[cfg(test)]
        {
            self.syncs += 1;
        }
        self.select_all = false;
        self.selection = None;
        self.drag_anchor = None;
        self.auto_scroll.stop();
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let source = self.text.clone();
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let work = source.clone();
            let parsed = cx
                .background_spawn(async move { WysiwygDocument::parse(&work) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.install(generation, Arc::new(parsed), cx);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn install(&mut self, generation: u64, document: Arc<WysiwygDocument>, cx: &mut Context<Self>) {
        if self.generation != generation {
            return;
        }
        self.document = document;
        self.images = Rc::new(HashMap::new());
        self.image_sizes.clear();
        let urls: Vec<String> = self
            .document
            .blocks
            .iter()
            .filter_map(|block| match &block.kind {
                BlockKind::Image { url } => Some(url.clone()),
                _ => None,
            })
            .collect();
        for url in urls {
            let resource =
                preview_resource(self.root.as_deref(), self.document_path.as_deref(), &url);
            if let Some(map) = Rc::get_mut(&mut self.images) {
                map.entry(url)
                    .or_insert_with(|| ImageCacheItem::new(&resource, cx));
            }
        }
        self.layout.borrow_mut().take();
        self.laid_out_width = None;
    }

    /// Reading anchor: source byte of the block at the viewport top plus the
    /// pixel offset inside it.
    fn scroll_anchor(&self) -> Option<(usize, f32)> {
        let offset = (-f32::from(self.scroll.offset().y)).max(0.);
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        let block = layout.block_at_y(offset)?;
        let source = layout.document().blocks.get(block)?.source.start;
        Some((source, offset - layout.block_top(block)))
    }

    fn restore_anchor(&mut self, viewport_height: f32) {
        let Some((source, delta)) = self.pending_anchor.take() else {
            return;
        };
        let Some(block) = self.document.block_index(source) else {
            return;
        };
        let (top, total) = {
            let layout = self.layout.borrow();
            let Some(layout) = layout.as_ref() else {
                return;
            };
            (layout.block_top(block), layout.total_height())
        };
        let max = (total - viewport_height).max(0.);
        let target = (top + delta).clamp(0., max);
        self.scroll.set_offset(point(px(0.), px(-target)));
    }

    fn sync_layout(&mut self, width: f32, cx: &App) {
        let style = Arc::new(LayoutStyle::from_app(&self.prose_style(cx), cx).with_images(true));
        let width = width.max(1.);
        let mut slot = self.layout.borrow_mut();
        let rebuild = match slot.as_ref() {
            None => true,
            Some(layout) => {
                !layout.style().metrics_eq(&style)
                    || self
                        .laid_out_width
                        .is_some_and(|previous| (previous - width).abs() > 0.5)
            }
        };
        if rebuild {
            *slot = Some(TextLayout::new(
                self.document.clone(),
                style.clone(),
                px(width),
                &self.image_sizes,
            ));
            self.laid_out_width = Some(width);
            self.layout_style = Some(style);
            return;
        }
        if let Some(layout) = slot.as_mut()
            && *layout.style() != *style
        {
            layout.set_style(style.clone());
        }
        self.layout_style = Some(style);
        self.laid_out_width = Some(width);
    }

    /// Observe loaded picture sizes and update the reserved heights.
    fn refresh_images(&mut self, window: &Window) -> bool {
        let mut changed = false;
        let mut sizes = ImageMetrics::new();
        for (url, item) in self.images.iter() {
            match item.use_image(window) {
                Some(Ok(image)) => {
                    let size = image.size(0);
                    sizes.insert(url.clone(), (size.width.0 as f32, size.height.0 as f32));
                }
                Some(Err(_)) => {
                    // Keep a compact alt-text strip instead of a huge empty
                    // placeholder for a resource that cannot load.
                    sizes.insert(url.clone(), (0., 0.));
                }
                None => {}
            }
        }
        if sizes != self.image_sizes {
            self.image_sizes = sizes;
            if let Some(layout) = self.layout.borrow_mut().as_mut() {
                changed = layout.set_image_metrics(&self.image_sizes);
            }
        }
        changed
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

    fn ordered_selection(&self) -> Option<(Hit, Hit)> {
        self.selection.map(|selection| {
            if selection.anchor <= selection.focus {
                (selection.anchor, selection.focus)
            } else {
                (selection.focus, selection.anchor)
            }
        })
    }

    fn set_hit(&mut self, hit: Hit, extend: bool, cx: &mut Context<Self>) {
        let anchor = if extend {
            self.selection.map_or(hit, |selection| selection.anchor)
        } else {
            hit
        };
        self.selection = Some(Selection { anchor, focus: hit });
        self.select_all = false;
        cx.notify();
    }

    fn hit_at(&self, position: Point<Pixels>) -> Option<Hit> {
        let geometry = *self.geometry.borrow();
        let layout = self.layout.borrow();
        let layout = layout.as_ref()?;
        layout.hit_test(point(
            position.x - geometry.origin.x,
            position.y - geometry.origin.y
                + px((-f32::from(self.scroll.offset().y)).max(0.) - geometry.scroll_y),
        ))
    }

    fn update_drag_autoscroll(
        &mut self,
        position: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = self.geometry.borrow().viewport;
        self.auto_scroll.last_drag_position = Some(position);
        let delta = AutoScroll::compute_delta(position.y, viewport).map(|delta| -delta);
        let window = window.window_handle();
        let preview = cx.weak_entity();
        self.auto_scroll.set(delta, cx, move |delta, state, cx| {
            let offset = state.scroll.offset();
            let max_y = f32::from(state.scroll.max_offset().y).max(0.);
            let y = (f32::from(offset.y) + f32::from(delta)).clamp(-max_y, 0.);
            if px(y) == offset.y {
                state.auto_scroll.stop();
                return;
            }
            state.scroll.set_offset(point(offset.x, px(y)));
            let window = window;
            let preview = preview.clone();
            cx.defer(move |cx| {
                _ = window.update(cx, |_, _window, cx| {
                    _ = preview.update(cx, |preview, cx| {
                        if preview.drag_anchor.is_some()
                            && preview.auto_scroll.is_active()
                            && let (Some(anchor), Some(position)) =
                                (preview.drag_anchor, preview.auto_scroll.last_drag_position)
                            && let Some(focus) = preview.hit_at(position)
                        {
                            preview.selection = Some(Selection { anchor, focus });
                            preview.select_all = false;
                            cx.notify();
                        }
                    });
                });
            });
        });
    }

    fn focus_at(&self, position: Point<Pixels>) -> Option<Hit> {
        self.hit_at(position)
    }

    fn navigate(
        &mut self,
        key: &str,
        command: bool,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let focus = match self.selection {
            Some(selection) => selection.focus,
            None => Hit {
                block: 0,
                rendered: 0,
            },
        };
        let destination = match key {
            "home" if command => Some(Hit {
                block: 0,
                rendered: 0,
            }),
            "end" if command => self.document.blocks.len().checked_sub(1).map(|block| Hit {
                block,
                rendered: self.document.blocks[block].text.text.len(),
            }),
            "home" => Some(Hit {
                block: focus.block,
                rendered: 0,
            }),
            "end" => self.document.blocks.get(focus.block).map(|block| Hit {
                block: focus.block,
                rendered: block.text.text.len(),
            }),
            "left" => previous_hit(&self.document, focus),
            "right" => following_hit(&self.document, focus),
            "up" | "down" => {
                let layout = self.layout.borrow();
                layout
                    .as_ref()
                    .and_then(|layout| layout.vertical_target(focus, key == "up"))
            }
            "pageup" | "pagedown" => {
                let viewport = f32::from(self.geometry.borrow().viewport.size.height);
                let layout_slot = self.layout.borrow();
                let Some(layout) = layout_slot.as_ref() else {
                    return false;
                };
                let Some(caret) = layout.caret(focus) else {
                    return false;
                };
                let step = (viewport * 0.9).max(1.);
                let y = f32::from(caret.origin.y) + if key == "pageup" { -step } else { step };
                layout.hit_test(point(caret.origin.x, px(y)))
            }
            _ => None,
        };
        let Some(destination) = destination else {
            return false;
        };
        let anchor = if shift {
            self.selection.map_or(focus, |selection| selection.anchor)
        } else {
            destination
        };
        self.selection = Some(Selection {
            anchor,
            focus: destination,
        });
        self.select_all = false;
        if key == "pagedown" || key == "pageup" {
            self.reveal(destination, window);
        }
        cx.notify();
        true
    }

    fn reveal(&mut self, hit: Hit, window: &mut Window) {
        let layout = self.layout.borrow();
        let Some(layout) = layout.as_ref() else {
            return;
        };
        let Some(caret) = layout.caret(hit) else {
            return;
        };
        let viewport_height = f32::from(window.viewport_size().height);
        let offset = (-f32::from(self.scroll.offset().y)).max(0.);
        let top = f32::from(caret.origin.y);
        let bottom = top + f32::from(caret.height);
        let max = (layout.total_height() - viewport_height).max(0.);
        let mut target = offset;
        if top < offset + 16. {
            target = (top - 16.).max(0.);
        } else if bottom > offset + viewport_height - 16. {
            target = (bottom - viewport_height + 16.).max(0.);
        }
        let target = target.min(max);
        if (target - offset).abs() > 0.5 {
            self.scroll.set_offset(point(px(0.), px(-target)));
        }
    }

    /// Displayed text covered by the current selection.
    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.ordered_selection()?;
        if start == end {
            return None;
        }
        Some(display_text(&self.document, start, end))
    }

    /// Move the viewport to an absolute translation, for the native harness.
    pub(crate) fn set_scroll_offset(&mut self, y: f32, cx: &mut Context<Self>) {
        let max =
            (-f32::from(self.scroll.offset().y)).max(0.) + f32::from(self.scroll.max_offset().y);
        let target = y.clamp(0., max.max(0.));
        self.scroll.set_offset(point(px(0.), px(-target)));
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
    pub(crate) fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RenderedDocument {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.unsupported || self.text.is_empty() {
            return div()
                .size_full()
                .child(crate::ui::empty_state(
                    "preview-empty",
                    gpui_kit::assets::IconName::Eye,
                    tr!("editor.preview.empty_title"),
                    self.empty_copy.message(),
                    None,
                    cx,
                ))
                .into_any_element();
        }
        let style = self.prose_style(cx);
        let viewport_height = f32::from(window.viewport_size().height);
        let content_width = self.geometry.borrow().width;
        let width = if content_width > 0. {
            content_width
        } else {
            (f32::from(window.viewport_size().width) * 0.7).max(240.)
        };
        self.sync_layout(width, cx);
        let images_changed = self.refresh_images(window);
        self.restore_anchor(viewport_height);
        let scroll = (-f32::from(self.scroll.offset().y)).max(0.);
        let overscan = viewport_height * 0.5;
        let prepare_started = std::time::Instant::now();
        let (prepared, stats, rows, total_height) = {
            let mut slot = self.layout.borrow_mut();
            let layout = slot
                .as_mut()
                .expect("the layout is created before it is read");
            let prepared = layout.prepare(
                window,
                Viewport {
                    scroll,
                    height: viewport_height,
                },
                self.selection.map(|selection| selection.focus),
                overscan,
            );
            (
                prepared,
                layout.stats(),
                layout.measured_rows(),
                layout.total_height(),
            )
        };
        crate::editor::perf::text_layout(
            "preview",
            stats,
            prepare_started.elapsed(),
            rows,
            self.document.blocks.len(),
            total_height,
            0,
            width,
            scroll,
        );
        if prepared.pending || images_changed {
            window.request_animation_frame();
        }
        let canvas = PreviewCanvas {
            layout: self.layout.clone(),
            geometry: self.geometry.clone(),
            images: self.images.clone(),
            scroll_y: scroll,
            selection: self.selection,
        };
        let content = div()
            .id("rendered-preview-content")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div().w_full().h_flex().justify_center().child(
                    div()
                        .w(gpui_kit::relative(style.page_width_percent / 100.))
                        .min_w(px(320.))
                        .max_w(px(1200.))
                        .px(px(crate::theme::PROSE_INSET))
                        .py(px(crate::theme::PROSE_TOP))
                        .child(canvas),
                ),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.auto_scroll.stop();
                    window.focus(&this.focus, cx);
                    if let Some(hit) = this.focus_at(event.position) {
                        this.drag_anchor = Some(hit);
                        this.set_hit(hit, event.modifiers.shift, cx);
                    } else {
                        this.drag_anchor = None;
                        this.selection = None;
                        this.select_all = false;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let Some(anchor) = this.drag_anchor else {
                    return;
                };
                if let Some(hit) = this.hit_at(event.position) {
                    this.selection = Some(Selection { anchor, focus: hit });
                    this.select_all = false;
                    cx.notify();
                }
                this.update_drag_autoscroll(event.position, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &gpui_kit::MouseUpEvent, _, cx| {
                    this.auto_scroll.stop();
                    let clicked = this.drag_anchor.take();
                    if let Some(hit) = this.hit_at(event.position) {
                        if let Some(url) = this.document.link_at(hit.block, hit.rendered) {
                            let url = url.to_owned();
                            if url.starts_with("http://")
                                || url.starts_with("https://")
                                || url.starts_with("mailto:")
                            {
                                cx.open_url(&url);
                            }
                        } else if clicked.is_some_and(|anchor| anchor != hit) {
                            this.set_hit(hit, true, cx);
                        }
                    }
                    cx.notify();
                }),
            );
        let root = div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("TextView")
            .capture_action(cx.listener(
                |this, _: &gpui_kit::component::input::SelectAll, _, cx| {
                    this.select_all = true;
                    if let (Some(first), Some(last)) =
                        (this.document.blocks.first(), this.document.blocks.last())
                    {
                        let _ = first;
                        let block = this.document.blocks.len() - 1;
                        this.selection = Some(Selection {
                            anchor: Hit {
                                block: 0,
                                rendered: 0,
                            },
                            focus: Hit {
                                block,
                                rendered: last.text.text.len(),
                            },
                        });
                    }
                    cx.stop_propagation();
                    cx.notify();
                },
            ))
            .capture_action(
                cx.listener(|this, _: &gpui_kit::component::input::Copy, _, cx| {
                    let text = if this.select_all {
                        Some(this.text.to_string())
                    } else {
                        this.selected_text()
                    };
                    if let Some(text) = text {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                        cx.stop_propagation();
                    }
                }),
            )
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                if this.select_all {
                    this.select_all = false;
                    this.selection = None;
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let command =
                    event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
                let shift = event.keystroke.modifiers.shift;
                if key == "escape" {
                    this.selection = None;
                    this.select_all = false;
                    cx.notify();
                    return;
                }
                if matches!(
                    key,
                    "left" | "right" | "up" | "down" | "home" | "end" | "pageup" | "pagedown"
                ) {
                    this.navigate(key, command, shift, window, cx);
                }
            }))
            .overflow_hidden()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(content);
        root.into_any_element()
    }
}

/// The viewport-height canvas that paints the prepared reading rows.
struct PreviewCanvas {
    layout: Rc<RefCell<Option<TextLayout>>>,
    geometry: Rc<RefCell<FrameGeometry>>,
    images: Rc<HashMap<String, ImageCacheItem>>,
    scroll_y: f32,
    selection: Option<Selection>,
}

impl IntoElement for PreviewCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PreviewCanvas {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let height = self
            .layout
            .borrow()
            .as_ref()
            .map_or(1., |layout| layout.total_height().max(1.));
        let style = gpui_kit::Style {
            size: size(
                gpui_kit::Length::Definite(gpui_kit::relative(1.0)),
                gpui_kit::Length::Definite(px(height).into()),
            ),
            ..Default::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let mut geometry = self.geometry.borrow_mut();
        if (geometry.width - f32::from(bounds.size.width)).abs() > 0.5 {
            // Render used the previous frame's measurement. Reconcile on the
            // next frame even when short content has no pending layout work.
            window.request_animation_frame();
        }
        geometry.origin = bounds.origin;
        geometry.width = f32::from(bounds.size.width);
        geometry.viewport = bounds.intersect(&window.content_mask().bounds);
        geometry.scroll_y = self.scroll_y;
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let painter = PreviewImagePainter {
            images: self.images.as_ref(),
        };
        if let Some(layout) = self.layout.borrow_mut().as_mut() {
            layout.paint(
                window,
                cx,
                bounds.origin,
                self.selection,
                None,
                &[],
                None,
                Some(&painter),
            );
        }
    }
}

struct PreviewImagePainter<'a> {
    images: &'a HashMap<String, ImageCacheItem>,
}

impl LayoutImagePainter for PreviewImagePainter<'_> {
    fn paint(&self, url: &str, bounds: Bounds<Pixels>, window: &mut Window) -> bool {
        let Some(item) = self.images.get(url) else {
            return false;
        };
        let Some(Ok(image)) = item.use_image(window) else {
            return false;
        };
        window
            .paint_image(bounds, bounds, Corners::default(), image, 0, false)
            .is_ok()
    }
}

fn previous_hit(document: &WysiwygDocument, hit: Hit) -> Option<Hit> {
    if hit.rendered > 0 {
        let text = &document.blocks.get(hit.block)?.text.text;
        let previous = text[..hit.rendered]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        return Some(Hit {
            block: hit.block,
            rendered: previous,
        });
    }
    for block in (0..hit.block).rev() {
        let text = &document.blocks.get(block)?.text.text;
        if !text.is_empty() {
            return Some(Hit {
                block,
                rendered: text.len(),
            });
        }
    }
    Some(Hit {
        block: 0,
        rendered: 0,
    })
}

fn following_hit(document: &WysiwygDocument, hit: Hit) -> Option<Hit> {
    let text = &document.blocks.get(hit.block)?.text.text;
    if hit.rendered < text.len() {
        let next = text[hit.rendered..]
            .chars()
            .next()
            .map_or(hit.rendered, |character| {
                hit.rendered + character.len_utf8()
            });
        return Some(Hit {
            block: hit.block,
            rendered: next,
        });
    }
    for block in hit.block + 1..document.blocks.len() {
        let text = &document.blocks[block].text.text;
        if !text.is_empty() {
            return Some(Hit { block, rendered: 0 });
        }
    }
    Some(Hit {
        block: hit.block,
        rendered: text.len(),
    })
}

/// Displayed text between two rendered positions, including table cells.
fn display_text(document: &WysiwygDocument, start: Hit, end: Hit) -> String {
    let mut output = String::new();
    let first = start.block.min(document.blocks.len().saturating_sub(1));
    let last = end.block.min(document.blocks.len().saturating_sub(1));
    for index in first..=last {
        let Some(block) = document.blocks.get(index) else {
            break;
        };
        let from = if index == start.block {
            start.rendered
        } else {
            0
        };
        let to = if index == end.block {
            end.rendered
        } else {
            block.text.text.len()
        };
        let from = from.min(block.text.text.len());
        let to = to.min(block.text.text.len());
        if from < to {
            output.push_str(&block.text.text[from..to]);
        }
        if index != last {
            output.push('\n');
        }
    }
    output
}

/// Resolve one Markdown image destination to the resource a reader loads.
fn preview_resource(root: Option<&Path>, document: Option<&Path>, raw: &str) -> Resource {
    if let (Some(root), Some(document)) = (root, document)
        && let Some(path) = preview_image_path(root, document, raw)
    {
        return Resource::Path(path.into());
    }
    match ImageSource::from(SharedString::from(raw.to_owned())) {
        ImageSource::Resource(resource) => resource,
        _ => Resource::Uri(gpui_kit::SharedUri::from(raw.to_owned())),
    }
}

/// Resolve one Markdown image destination to a file inside the project.
///
/// Astro serves a leading `/` from `public/`; everything else is relative to
/// the document. Resolution is lexical and refuses paths that leave the
/// project.
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
    async fn preview_installs_once_and_keeps_selection_and_copy(cx: &mut gpui_kit::TestAppContext) {
        cx.executor().allow_parking();
        let source = "A paragraph with **bold** and `code`.\n\nSecond paragraph.\n";
        let (handle, preview) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                cx.new(RenderedDocument::new)
            })
            .unwrap()
        });
        preview.update(cx, |preview, cx| {
            preview.set_document(None, None, Some((DocumentKind::Markdown, source)), cx);
        });
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
            let installed = preview.read(cx).sync_count();
            window.render_frame(cx);
            assert_eq!(
                preview.read(cx).sync_count(),
                installed,
                "an unchanged frame never reinstalls the body"
            );
            let focus = preview.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
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
                Some(source.to_owned()),
                "Select All still copies the source body"
            );
        })
        .unwrap();
        // Dragging across blocks copies displayed text, not the source.
        preview.update(cx, |preview, _| {
            preview.select_all = false;
            preview.selection = Some(Selection {
                anchor: Hit {
                    block: 0,
                    rendered: 0,
                },
                focus: Hit {
                    block: 1,
                    rendered: "Second".len(),
                },
            });
            assert_eq!(
                preview.selected_text().as_deref(),
                Some("A paragraph with bold and code.\nSecond")
            );
        });
    }

    #[gpui_kit::test]
    async fn dragging_to_the_viewport_edge_scrolls_and_extends_selection(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let source = (0..2_000)
            .map(|index| format!("Paragraph {index} with several words for selection.\n\n"))
            .collect::<String>();
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
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        let (start, edge, before) = preview.read_with(cx, |preview, _| {
            let geometry = *preview.geometry.borrow();
            let layout = preview.layout.borrow();
            let caret = layout
                .as_ref()
                .unwrap()
                .caret(Hit {
                    block: 0,
                    rendered: 0,
                })
                .unwrap();
            (
                point(
                    geometry.origin.x + caret.origin.x + px(1.),
                    geometry.origin.y + caret.origin.y + caret.height * 0.5,
                ),
                point(
                    geometry.origin.x + caret.origin.x + px(140.),
                    geometry.viewport.bottom() - px(2.),
                ),
                preview.scroll.offset(),
            )
        });

        let mut visual = gpui_kit::VisualTestContext::from_window(handle, cx);
        visual.simulate_mouse_down(start, MouseButton::Left, Default::default());
        visual.simulate_mouse_move(edge, MouseButton::Left, Default::default());
        visual
            .executor()
            .advance_clock(std::time::Duration::from_millis(64));
        visual.run_until_parked();
        visual.simulate_mouse_up(edge, MouseButton::Left, Default::default());

        let (after, selection) = preview.read_with(cx, |preview, _| {
            (preview.scroll.offset(), preview.selection.unwrap())
        });
        assert!(after.y < before.y, "dragging at the edge scrolls down");
        assert!(
            selection.focus > selection.anchor,
            "selection extends toward newly revealed text"
        );

        cx.executor()
            .advance_clock(std::time::Duration::from_millis(64));
        cx.run_until_parked();
        assert_eq!(
            preview.read_with(cx, |preview, _| preview.scroll.offset()),
            after,
            "auto-scroll stops after mouse-up"
        );
    }

    #[gpui_kit::test]
    async fn long_preview_shapes_only_the_viewport_and_survives_resize(
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
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        preview.read_with(cx, |preview, _| {
            let layout = preview.layout.borrow();
            let layout = layout.as_ref().expect("layout exists");
            assert!(
                layout.measured_rows() < preview.document.blocks.len(),
                "the first frame must not wrap the whole document"
            );
            assert!(layout.total_height() > 0.);
        });
        cx.simulate_window_resize(
            handle,
            gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(700.)),
        );
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        preview.read_with(cx, |preview, _| {
            assert!(preview.laid_out_width.is_some());
        });
    }

    #[gpui_kit::test]
    fn short_preview_reconciles_width_on_the_next_frame(cx: &mut gpui_kit::TestAppContext) {
        let (handle, preview) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                cx.new(RenderedDocument::new)
            })
            .unwrap()
        });
        preview.update(cx, |preview, cx| {
            preview.document = Arc::new(WysiwygDocument::parse("Short paragraph."));
            preview.text = "Short paragraph.".into();
            cx.notify();
        });
        for width in [900., 1500., 900.] {
            cx.simulate_window_resize(handle, size(px(width), px(700.)));
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.update_window(handle, |_, window, cx| {
                assert!(
                    window.simulate_next_frame(cx) > 0,
                    "measured width changes must schedule reconciliation"
                );
            })
            .unwrap();
            cx.run_until_parked();
            cx.update_window(handle, |_, window, cx| window.render_frame(cx))
                .unwrap();
            preview.read_with(cx, |preview, _| {
                assert!(
                    (preview.laid_out_width.unwrap() - preview.geometry.borrow().width).abs() <= 0.5,
                    "stable short content must adopt its measured column width without input or scrolling"
                );
            });
        }
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
