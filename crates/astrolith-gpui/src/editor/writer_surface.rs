//! WYSIWYG writing surface.
//!
//! The retained [`EditorState`] remains the input model: it owns the cursor,
//! selection, IME composition and undo integration. This view renders the same
//! writer projection as rendered Markdown blocks, paints the caret and
//! selection itself, and maps pointer positions back into the editor's byte
//! offsets. Markdown syntax (heading hashes, emphasis markers, list bullets,
//! link destinations and fences) is not painted; the source stays byte-exact.
//!
//! Block rendering and leaf geometry come from the pinned GPUI Kit
//! `editor_host` seam: the host builds `BlockNode`s and receives per-leaf
//! geometry for hit-testing and caret placement.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use astrolith_document::{BlockKind, WysiwygDocument, cursor_sentence, other_sentences};
use gpui_kit::base::text::editor_host::{
    BlockNode, CodeBlock, FlowPosition, InlineNode, LeafGeometry, LinkMark, Paragraph,
    RenderContext, Span, TextMark, collect_leaf_geometry,
};
use gpui_kit::component::input::Editor;
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Element, ElementId, Focusable, GlobalElementId, Hsla,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render,
    ScrollHandle, SharedString, Window, div, fill, point, prelude::*, px, relative, size,
};

use crate::editor::session::DocumentSession;
use crate::editor::writing_modes::ProseStyle;

/// One frame's laid-out leaves and viewport.
#[derive(Default)]
struct FrameGeometry {
    leaves: HashMap<u64, LeafGeometry>,
    viewport: Bounds<Pixels>,
}

/// The writing surface for one retained document session.
pub(crate) struct WriterSurface {
    session: gpui_kit::WeakEntity<DocumentSession>,
    document: Arc<WysiwygDocument>,
    source: String,
    style: ProseStyle,
    geometry: Rc<RefCell<FrameGeometry>>,
    scroll: ScrollHandle,
    /// Source offset where the current drag began.
    drag_anchor: Option<usize>,
    /// Last rendered selection rectangle, for the floating toolbar.
    selection_bounds: Option<Bounds<Pixels>>,
    /// Last rendered text viewport, for the floating toolbar.
    viewport: Option<Bounds<Pixels>>,
}

impl WriterSurface {
    pub(crate) fn new(
        session: gpui_kit::WeakEntity<DocumentSession>,
        style: ProseStyle,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            session,
            document: Arc::new(WysiwygDocument::default()),
            source: String::new(),
            style,
            geometry: Rc::new(RefCell::new(FrameGeometry::default())),
            scroll: ScrollHandle::new(),
            drag_anchor: None,
            selection_bounds: None,
            viewport: None,
        }
    }

    /// Install the configured document typography.
    pub(crate) fn set_style(&mut self, style: ProseStyle, cx: &mut Context<Self>) {
        if self.style == style {
            return;
        }
        self.style = style;
        cx.notify();
    }

    /// Re-parse when the visible text changed.
    fn sync_document(&mut self, source: &str) {
        if self.source == source {
            return;
        }
        self.source.clear();
        self.source.push_str(source);
        self.document = Arc::new(WysiwygDocument::parse(source));
    }

    /// Source offset under a window position.
    pub(crate) fn source_at(&self, position: Point<Pixels>) -> Option<usize> {
        let (block, rendered) = self.location_at(position)?;
        self.document.source_at(block, rendered)
    }

    /// Link destination under a window position.
    pub(crate) fn url_at(&self, position: Point<Pixels>) -> Option<String> {
        let (block, rendered) = self.location_at(position)?;
        self.document.link_at(block, rendered).map(str::to_owned)
    }

    /// Image destination under a window position.
    pub(crate) fn image_at(&self, position: Point<Pixels>) -> Option<String> {
        let (block, rendered) = self.location_at(position)?;
        self.document.image_at(block, rendered).map(str::to_owned)
    }

    /// The rendered selection rectangle and viewport from the last frame.
    pub(crate) fn selection_geometry(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        Some((self.selection_bounds?, self.viewport?))
    }

    /// Move the editor selection along the rendered layout.
    ///
    /// Left/right skip Markdown syntax that is not painted; up/down follow the
    /// rendered line boxes rather than source lines, so headings and wrapped
    /// paragraphs behave like the text the reader sees.
    pub(crate) fn navigate(
        &mut self,
        key: &str,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.session.upgrade() else {
            return false;
        };
        let (focus, selection) = {
            let session = session.read(cx);
            let editor = session.editor.read(cx);
            (editor.cursor(), editor.selected_range())
        };
        let document = self.document.clone();
        let Some((block, rendered)) = document.caret_location(focus) else {
            return false;
        };
        let anchor = if shift {
            if focus == selection.start {
                selection.end
            } else {
                selection.start
            }
        } else {
            focus
        };
        let destination = match key {
            "left" => previous_rendered(&document, block, rendered),
            "right" => following_rendered(&document, block, rendered),
            "home" => Some((block, 0)),
            "end" => document
                .blocks
                .get(block)
                .map(|block| (block, block.text.text.len()))
                .map(|(_, len)| (block, len)),
            "up" | "down" => self.vertical_target(block, rendered, key == "up"),
            _ => return false,
        };
        let Some((next_block, next_rendered)) = destination else {
            return false;
        };
        let Some(source) = document.source_at(next_block, next_rendered) else {
            return false;
        };
        let anchor = if shift { anchor } else { source };
        session.update(cx, |session, cx| {
            session.set_writer_selection(anchor, source, window, cx);
        });
        cx.notify();
        true
    }

    fn vertical_target(&self, block: usize, rendered: usize, up: bool) -> Option<(usize, usize)> {
        let geometry = self.geometry.borrow();
        let leaf = geometry.leaves.get(&block_key(block))?;
        let caret = leaf.position_for_index(rendered);
        let target_y = if up {
            caret.y - leaf.line_height()
        } else {
            caret.y + leaf.line_height()
        };
        let best = geometry
            .leaves
            .iter()
            .map(|(key, leaf)| {
                let index = leaf.index_for_position(point(caret.x, target_y));
                let at = leaf.position_for_index(index);
                let dy = (f32::from(at.y) - f32::from(target_y)).abs();
                let dx = (f32::from(at.x) - f32::from(caret.x)).abs();
                (*key, index, at, dy, dx)
            })
            .filter(|(_, _, at, _, _)| at.y != caret.y)
            .min_by(|left, right| {
                left.3
                    .partial_cmp(&right.3)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        left.4
                            .partial_cmp(&right.4)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            })?;
        Some(((best.0 as usize).saturating_sub(1), best.1))
    }

    fn location_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let geometry = self.geometry.borrow();
        let (key, leaf) = geometry.leaves.iter().min_by(|(_, left), (_, right)| {
            distance(left.bounds(), position)
                .partial_cmp(&distance(right.bounds(), position))
                .unwrap_or(std::cmp::Ordering::Equal)
        })?;
        let rendered = leaf.index_for_position(position);
        Some(((*key as usize).saturating_sub(1), rendered))
    }

    /// Map a pointer position to a source offset and update the selection.
    fn select(
        &mut self,
        position: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(offset) = self.source_at(position) else {
            return;
        };
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let anchor = if extend {
            self.drag_anchor.or_else(|| {
                Some(session.read_with(cx, |session, cx| {
                    let editor = session.editor.read(cx);
                    let selection = editor.selected_range();
                    let cursor = editor.cursor();
                    if cursor == selection.start {
                        selection.end
                    } else {
                        selection.start
                    }
                }))
            })
        } else {
            None
        }
        .unwrap_or(offset);
        session.update(cx, |session, cx| {
            session.set_writer_selection(anchor, offset, window, cx);
        });
        if !extend {
            self.drag_anchor = Some(offset);
        }
        cx.notify();
    }

    /// Keep the caret in view; typewriter mode centres it.
    fn adjust_scroll(&mut self, document: &WysiwygDocument, cursor: usize, typewriter: bool) {
        let geometry = self.geometry.borrow();
        let Some((block, rendered)) = document.caret_location(cursor) else {
            return;
        };
        let Some(leaf) = geometry.leaves.get(&block_key(block)) else {
            return;
        };
        let caret = leaf.position_for_index(rendered);
        let height = leaf.caret_height();
        let viewport = geometry.viewport;
        if viewport.size.height <= px(0.) {
            return;
        }
        let offset = self.scroll.offset();
        // Child content coordinates: window position minus the viewport origin
        // minus the current scroll translation.
        let content_y = f32::from(caret.y) - f32::from(viewport.origin.y) - f32::from(offset.y);
        let viewport_height = f32::from(viewport.size.height);
        let mut target = f32::from(offset.y);
        if typewriter {
            let centre = (viewport_height - f32::from(height)) / 2.;
            target = -(content_y - centre);
        } else {
            let top = 20.;
            let bottom = viewport_height - 28.;
            if content_y < top {
                target += top - content_y;
            } else if content_y + f32::from(height) > bottom {
                target -= content_y + f32::from(height) - bottom;
            }
        }
        let max = f32::from(self.scroll.max_offset().y);
        let target = px(clamp_scroll_offset(target, max));
        if target != offset.y {
            self.scroll.set_offset(point(offset.x, target));
        }
    }

    fn update_selection_geometry(
        &mut self,
        document: &WysiwygDocument,
        selection: &Range<usize>,
        geometry: &HashMap<u64, LeafGeometry>,
    ) {
        if selection.is_empty() {
            self.selection_bounds = None;
            return;
        }
        let mut bounds: Option<Bounds<Pixels>> = None;
        for (index, block) in document.blocks.iter().enumerate() {
            if block.source.end < selection.start || block.source.start > selection.end {
                continue;
            }
            let start = if selection.start <= block.content.start {
                0
            } else {
                block.text.nearest_rendered_offset(selection.start)
            };
            let end = if selection.end >= block.content.end {
                block.text.text.len()
            } else {
                block.text.nearest_rendered_offset(selection.end)
            };
            let Some(leaf) = geometry.get(&block_key(index)) else {
                continue;
            };
            let start_point = leaf.position_for_index(start);
            let end_point = leaf.position_for_index(end);
            let leaf_bounds = Bounds::new(
                point(
                    start_point.x.min(end_point.x),
                    start_point.y.min(end_point.y),
                ),
                size(
                    (start_point.x - end_point.x).abs().max(px(2.)),
                    (end_point.y - start_point.y).abs() + leaf.line_height(),
                ),
            );
            bounds = Some(match bounds {
                Some(existing) => existing.union(&leaf_bounds),
                None => leaf_bounds,
            });
        }
        self.selection_bounds = bounds;
    }
}

fn block_key(index: usize) -> u64 {
    index as u64 + 1
}

/// Clamp a scroll translation to GPUI's convention: 0 at the top, negative
/// while scrolling down, bounded by the positive `max` distance.
fn clamp_scroll_offset(target: f32, max: f32) -> f32 {
    target.clamp(-max.max(0.), 0.)
}

/// The previous rendered character, crossing block boundaries.
fn previous_rendered(
    document: &WysiwygDocument,
    block: usize,
    rendered: usize,
) -> Option<(usize, usize)> {
    if rendered > 0 {
        let text = &document.blocks.get(block)?.text.text;
        let previous = text[..rendered]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        return Some((block, previous));
    }
    for index in (0..block).rev() {
        let text = &document.blocks.get(index)?.text.text;
        if !text.is_empty() {
            return Some((index, text.len()));
        }
    }
    None
}

/// The next rendered character, crossing block boundaries.
fn following_rendered(
    document: &WysiwygDocument,
    block: usize,
    rendered: usize,
) -> Option<(usize, usize)> {
    let text = &document.blocks.get(block)?.text.text;
    if rendered < text.len() {
        let next = text[rendered..]
            .chars()
            .next()
            .map_or(rendered, |character| rendered + character.len_utf8());
        return Some((block, next));
    }
    for index in block + 1..document.blocks.len() {
        let text = &document.blocks[index].text.text;
        if !text.is_empty() {
            return Some((index, 0));
        }
    }
    None
}

fn distance(bounds: Bounds<Pixels>, position: Point<Pixels>) -> f32 {
    if bounds.contains(&position) {
        return 0.;
    }
    let dx = if position.x < bounds.origin.x {
        bounds.origin.x - position.x
    } else if position.x > bounds.right() {
        position.x - bounds.right()
    } else {
        px(0.)
    };
    let dy = if position.y < bounds.origin.y {
        bounds.origin.y - position.y
    } else if position.y > bounds.bottom() {
        position.y - bounds.bottom()
    } else {
        px(0.)
    };
    let dx = f32::from(dx);
    let dy = f32::from(dy);
    dx * dx + dy * dy
}

impl Render for WriterSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(session) = self.session.upgrade() else {
            return div().size_full();
        };
        let (visible, cursor, selection, focused, modes, placeholders) = {
            let session = session.read(cx);
            let editor = session.editor.read(cx);
            (
                session.projection.visible.clone(),
                editor.cursor().min(session.projection.visible.len()),
                editor.selected_range(),
                editor.focus_handle(cx).is_focused(window),
                session.presentation.modes,
                session.placeholder_ranges().to_vec(),
            )
        };
        let copyedit = session.read(cx).copyedit.render_ranges(cursor);
        self.sync_document(&visible);
        let document = self.document.clone();
        self.adjust_scroll(&document, cursor, focused && modes.typewriter);
        let style = self.style.clone();
        let mut decorations: HashMap<u64, Vec<(Range<usize>, Hsla)>> = HashMap::new();
        let selection_color = cx.theme().selection.opacity(0.35);
        let placeholder_color = cx.theme().primary.opacity(0.18);

        let focus_ranges = if modes.focus {
            cursor_sentence(&visible, cursor)
                .map(|sentence| other_sentences(&visible, sentence.start))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let mut nodes = Vec::with_capacity(document.blocks.len());
        for (index, block) in document.blocks.iter().enumerate() {
            if block.text.text.is_empty() && !matches!(block.kind, BlockKind::Rule) {
                // Empty trailing lines still need a leaf so a caret can be
                // placed on them.
            }
            let key = block_key(index);
            let paragraph = paragraph_for(block, key);
            let node = match &block.kind {
                BlockKind::Heading(level) => BlockNode::Heading {
                    level: *level,
                    children: paragraph,
                    span: None,
                },
                BlockKind::Paragraph => BlockNode::Paragraph(paragraph),
                BlockKind::ListItem { .. } => BlockNode::Paragraph(paragraph),
                BlockKind::Quote => BlockNode::Paragraph(paragraph),
                BlockKind::Code { language } => BlockNode::CodeBlock(CodeBlock::new(
                    block.text.text.clone().into(),
                    (!language.is_empty()).then(|| SharedString::from(language.clone())),
                    None::<Span>,
                )),
                BlockKind::Rule => BlockNode::HorizontalRule { span: None },
            };
            nodes.push(node);

            if !selection.is_empty() {
                add_source_decoration(
                    &mut decorations,
                    index,
                    block,
                    selection.clone(),
                    selection_color,
                    false,
                );
            }
            for (range, color) in &copyedit {
                add_source_decoration(&mut decorations, index, block, range.clone(), *color, false);
            }
            for range in &placeholders {
                add_source_decoration(
                    &mut decorations,
                    index,
                    block,
                    range.clone(),
                    placeholder_color,
                    true,
                );
            }
        }

        let refs: Vec<&BlockNode> = nodes.iter().collect();
        let render_context = RenderContext::from_app(cx).with_body_font_size(px(style.font_size));
        let mut blocks = Vec::with_capacity(nodes.len());
        for (index, block) in document.blocks.iter().enumerate() {
            let element = nodes[index].render(
                index,
                FlowPosition::at(&refs, index, index + 1 == refs.len()),
                &render_context,
                window,
                cx,
            );
            let dimmed = modes.focus
                && !focus_ranges
                    .iter()
                    .any(|range| range.start < block.source.end && range.end > block.source.start);
            let element = block_wrapper(block, element, dimmed, cx);
            blocks.push(element);
        }

        let caret = (focused && selection.is_empty()).then_some(cursor);
        let caret_color = cx.theme().caret;
        let geometry_for_frame = self.geometry.clone();
        let flow = RichFlow {
            child: Some(
                div()
                    .w_full()
                    .font_family(style.font_family.clone())
                    .text_size(px(style.font_size))
                    .line_height(relative(style.line_height))
                    .children(blocks)
                    .into_any_element(),
            ),
            decorations,
            geometry: geometry_for_frame,
            document: document.clone(),
            caret,
            caret_color,
        };
        let content = div()
            .id("writer-wysiwyg-content")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(flow)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.drag_anchor = None;
                    let extend = event.modifiers.shift;
                    this.select(event.position, extend, window, cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                this.select(event.position, true, window, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &gpui_kit::MouseUpEvent, _, cx| {
                    this.drag_anchor = None;
                    cx.notify();
                }),
            );

        self.viewport = Some(self.geometry.borrow().viewport);
        let leaves = self.geometry.borrow().leaves.clone();
        self.update_selection_geometry(&document, &selection, &leaves);

        div()
            .size_full()
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .size_full()
                    .opacity(0.)
                    .child(Editor::new(&session.read(cx).editor)),
            )
            .child(content)
    }
}

fn paragraph_for(block: &astrolith_document::RichBlock, key: u64) -> Paragraph {
    let mut paragraph = Paragraph::default();
    if block.text.text.is_empty() {
        paragraph.push(InlineNode::new(""));
    } else {
        paragraph.push(InlineNode::new(block.text.text.clone()).marks(mark_runs(&block.text)));
    }
    paragraph.with_leaf_key(key)
}

fn mark_runs(text: &astrolith_document::RichText) -> Vec<(Range<usize>, TextMark)> {
    let mut cuts = vec![0, text.text.len()];
    for mark in &text.marks {
        cuts.push(mark.range.start.min(text.text.len()));
        cuts.push(mark.range.end.min(text.text.len()));
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    for pair in cuts.windows(2) {
        let start = pair[0];
        let end = pair[1];
        if start >= end {
            continue;
        }
        let mut style = TextMark::default();
        for mark in &text.marks {
            if mark.range.start <= start && mark.range.end >= end {
                match &mark.kind {
                    astrolith_document::InlineMarkKind::Bold => style.bold = true,
                    astrolith_document::InlineMarkKind::Italic => style.italic = true,
                    astrolith_document::InlineMarkKind::Strikethrough => style.strikethrough = true,
                    astrolith_document::InlineMarkKind::Code => style.code = true,
                    astrolith_document::InlineMarkKind::Link(url) => {
                        style.link = Some(LinkMark {
                            url: url.clone().into(),
                            identifier: None,
                            title: None,
                        });
                    }
                    astrolith_document::InlineMarkKind::Image(_) => style.italic = true,
                }
            }
        }
        runs.push((start..end, style));
    }
    runs
}

fn block_wrapper(
    block: &astrolith_document::RichBlock,
    element: AnyElement,
    dimmed: bool,
    cx: &mut App,
) -> AnyElement {
    let mut wrapper = div().w_full();
    match &block.kind {
        BlockKind::ListItem { ordered, number } => {
            let marker = if *ordered {
                format!("{number}.")
            } else {
                "•".to_owned()
            };
            return div()
                .w_full()
                .pl(px(block.depth as f32 * 20.))
                .h_flex()
                .items_start()
                .gap_2()
                .when(dimmed, |this| this.opacity(0.35))
                .child(
                    div()
                        .min_w(px(18.))
                        .text_color(cx.theme().muted_foreground)
                        .child(marker),
                )
                .child(div().flex_1().min_w_0().child(element))
                .into_any_element();
        }
        BlockKind::Quote => {
            wrapper = wrapper
                .pl(px(12.))
                .border_l_2()
                .border_color(cx.theme().border)
                .text_color(cx.theme().muted_foreground);
        }
        BlockKind::Code { .. } => {
            wrapper = wrapper.my(px(6.));
        }
        BlockKind::Rule => {
            wrapper = wrapper.py(px(6.));
        }
        BlockKind::Heading(_) | BlockKind::Paragraph => {}
    }
    wrapper
        .when(dimmed, |this| this.opacity(0.35))
        .child(element)
        .into_any_element()
}

/// Clip a source range to one block and record it under the leaf key.
fn add_source_decoration(
    decorations: &mut HashMap<u64, Vec<(Range<usize>, Hsla)>>,
    index: usize,
    block: &astrolith_document::RichBlock,
    source: Range<usize>,
    color: Hsla,
    anchor_empty: bool,
) {
    if source.end < block.source.start || source.start > block.source.end {
        return;
    }
    let start = if source.start <= block.content.start {
        0
    } else {
        block.text.nearest_rendered_offset(source.start)
    };
    let end = if source.end >= block.content.end {
        block.text.text.len()
    } else {
        block.text.nearest_rendered_offset(source.end)
    };
    if start < end {
        decorations
            .entry(block_key(index))
            .or_default()
            .push((start..end, color));
    } else if anchor_empty && start < block.text.text.len() {
        decorations
            .entry(block_key(index))
            .or_default()
            .push((start..(start + 1), color));
    }
}

/// Renders blocks, collects leaf geometry and paints the caret.
struct RichFlow {
    child: Option<AnyElement>,
    decorations: HashMap<u64, Vec<(Range<usize>, Hsla)>>,
    geometry: Rc<RefCell<FrameGeometry>>,
    document: Arc<WysiwygDocument>,
    caret: Option<usize>,
    caret_color: Hsla,
}

struct Prepaint {
    leaves: HashMap<u64, LeafGeometry>,
}

impl IntoElement for RichFlow {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RichFlow {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Prepaint;

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
    ) -> (LayoutId, AnyElement) {
        let mut child = self.child.take().expect("flow child is laid out once");
        let layout = child.request_layout(window, cx);
        (layout, child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let decorations = std::mem::take(&mut self.decorations);
        let (_, leaves) = collect_leaf_geometry(decorations, || child.prepaint(window, cx));
        *self.geometry.borrow_mut() = FrameGeometry {
            leaves: leaves.clone(),
            viewport: bounds,
        };
        Prepaint { leaves }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
        let Some(source) = self.caret else {
            return;
        };
        let Some((block, rendered)) = self.document.caret_location(source) else {
            return;
        };
        let Some(leaf) = prepaint.leaves.get(&block_key(block)) else {
            return;
        };
        let origin = leaf.position_for_index(rendered);
        let bounds = Bounds::new(origin, size(px(1.5), leaf.caret_height().max(px(2.))));
        window.paint_quad(fill(bounds, self.caret_color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_distance_prefers_the_containing_block() {
        let bounds = Bounds::new(point(px(10.), px(10.)), size(px(100.), px(20.)));
        assert_eq!(distance(bounds, point(px(20.), px(20.))), 0.);
        assert!(
            distance(bounds, point(px(0.), px(0.))) < distance(bounds, point(px(0.), px(100.)))
        );
    }

    #[test]
    fn scroll_offsets_stay_between_the_top_and_the_positive_max_distance() {
        assert_eq!(clamp_scroll_offset(120., 3219.), 0.);
        assert_eq!(clamp_scroll_offset(-500., 3219.), -500.);
        assert_eq!(clamp_scroll_offset(-9000., 3219.), -3219.);
        assert_eq!(clamp_scroll_offset(12., 0.), 0.);
    }

    #[test]
    fn rendered_marks_become_text_mark_runs() {
        let mut text = astrolith_document::RichText {
            text: "abc".to_owned(),
            ..Default::default()
        };
        text.marks.push(astrolith_document::InlineMark {
            range: 1..3,
            kind: astrolith_document::InlineMarkKind::Bold,
        });
        let runs = mark_runs(&text);
        assert_eq!(runs.len(), 2);
        assert!(!runs[0].1.bold);
        assert!(runs[1].1.bold);
    }
}
