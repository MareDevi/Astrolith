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

mod input;

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use astrolith_document::{BlockKind, WysiwygDocument, cursor_sentence};
use gpui_kit::base::text::editor_host::{
    BlockNode, CodeBlock, FlowPosition, InlineNode, LeafGeometry, LinkMark, Paragraph,
    RenderContext, Span, TextMark, collect_leaf_geometry,
};
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::{
    AnyElement, App, Bounds, Context, Element, ElementId, Focusable, GlobalElementId, Hsla,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render,
    ScrollHandle, SharedString, TestSupportExt, Window, div, fill, point, prelude::*, px, relative,
    size,
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
    /// Measured leaf height of every block the last frame rendered.
    ///
    /// `None` means the block has not been laid out since the document last
    /// changed; its height is estimated from the block kind, text length and
    /// the prose style instead.
    heights: Rc<RefCell<Vec<Option<f32>>>>,
    /// Prefix sum of the estimated heights, in content pixels.
    prefix: Vec<f32>,
    /// Buffer revision the parsed document was built from.
    document_revision: Option<u64>,
    last_cursor: Option<usize>,
    /// Content width from the last laid-out frame, for the line estimate.
    content_width: Option<f32>,
    /// Number of blocks the last frame laid out, for the windowing tests.
    #[cfg(test)]
    rendered_blocks: usize,
    /// Global block range the last frame laid out, for the windowing tests.
    #[cfg(test)]
    rendered_range: Range<usize>,
    /// The block range of the very first layout, for the windowing tests.
    #[cfg(test)]
    first_rendered_range: Option<Range<usize>>,
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
            heights: Rc::new(RefCell::new(Vec::new())),
            prefix: Vec::new(),
            document_revision: None,
            last_cursor: None,
            content_width: None,
            #[cfg(test)]
            rendered_blocks: 0,
            #[cfg(test)]
            rendered_range: 0..0,
            #[cfg(test)]
            first_rendered_range: None,
        }
    }

    /// Install the configured document typography.
    pub(crate) fn set_style(&mut self, style: ProseStyle, cx: &mut Context<Self>) {
        if self.style == style {
            return;
        }
        self.style = style;
        // Line metrics changed, so every measured height is stale.
        self.heights.borrow_mut().fill(None);
        cx.notify();
    }

    /// Re-parse when the buffer revision changed.
    fn sync_document(&mut self, revision: u64, source: &str) {
        if self.document_revision == Some(revision) {
            return;
        }
        self.document_revision = Some(revision);
        self.source.clear();
        self.source.push_str(source);
        self.document = Arc::new(WysiwygDocument::parse(source));
        // Keep the measured heights across an edit: clearing them would make
        // the first frame after every keystroke render the whole document
        // again. The vector only grows or shrinks; blocks that moved keep an
        // approximate height until they are laid out and remeasured.
        let blocks = self.document.blocks.len();
        let mut heights = self.heights.borrow_mut();
        heights.truncate(blocks);
        heights.resize(blocks, None);
        drop(heights);
        self.prefix.clear();
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
    pub(crate) fn selection_geometry(
        &self,
        selection: &Range<usize>,
    ) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let geometry = self.geometry.borrow();
        let bounds = Self::selection_bounds(&self.document, selection, &geometry.leaves)?;
        Some((bounds, geometry.viewport))
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
        let block = (*key as usize).saturating_sub(1);
        let rendered = leaf
            .index_for_position(position)
            .min(self.document.blocks.get(block)?.text.text.len());
        Some((block, rendered))
    }

    /// The viewport height the visible window is derived from.
    ///
    /// The flow's own layout bounds are the fallback, but a scrolled child can
    /// report its full content height; the window can never be taller than the
    /// window it paints into, so the two are intersected.
    fn effective_viewport_height(&self, window: &Window) -> f32 {
        let geometry = f32::from(self.geometry.borrow().viewport.size.height);
        let window_height = f32::from(window.viewport_size().height);
        if geometry > 0. {
            geometry.min(window_height)
        } else {
            window_height
        }
    }

    /// Prefix sum of every block's height, estimated or measured.
    ///
    /// Height zero is the content top; the last entry is the total content
    /// height the spacers and rendered blocks add up to.
    fn build_prefix(&mut self, document: &WysiwygDocument) {
        let width = self.content_width.unwrap_or(640.);
        let line_height = self.style.font_size * self.style.line_height;
        let gap = paragraph_gap_px(self.style.font_size);
        self.prefix.clear();
        self.prefix.reserve(document.blocks.len() + 1);
        self.prefix.push(0.);
        let mut y = 0.;
        for (index, block) in document.blocks.iter().enumerate() {
            y += match self.heights.borrow().get(index).copied().flatten() {
                Some(measured) => measured + gap,
                None => estimate_block_height(block, width, self.style.font_size, line_height, gap),
            };
            self.prefix.push(y);
        }
    }

    /// The block range worth laying out for the current scroll position.
    ///
    /// The first frame uses height estimates just like a resized frame. Only
    /// the visible run and its overscan are laid out. A changed caret includes
    /// its block and one neighbour on each side. A caret far outside the window (a programmatic jump) moves the
    /// scroll to its estimated position instead of rendering every block in
    /// between.
    fn visible_block_range(
        &mut self,
        document: &WysiwygDocument,
        cursor: usize,
        viewport_height: f32,
        follow_cursor: bool,
    ) -> Range<usize> {
        let total = document.blocks.len();
        if total == 0 {
            return 0..0;
        }
        self.build_prefix(document);
        let overscan = WINDOW_OVERSCAN_PX + 2. * self.style.font_size * self.style.line_height;
        let scroll = (-f32::from(self.scroll.offset().y)).max(0.);
        let top = (scroll - overscan).max(0.);
        let bottom = scroll + viewport_height + overscan;
        let start = self.prefix.partition_point(|y| *y <= top).saturating_sub(1);
        let mut end = self.prefix.partition_point(|y| *y < bottom).min(total);
        let mut start = start.min(total);
        end = end.max(start + 1).min(total);

        if !follow_cursor {
            return start..end;
        }
        let Some((caret, _)) = document.caret_location(cursor) else {
            return start..end;
        };
        if (start..end).contains(&caret) {
            start = start.min(caret.saturating_sub(1));
            end = end.max((caret + 2).min(total));
            return start..end;
        }
        let caret_y = self.prefix.get(caret).copied().unwrap_or(0.);
        let window_start = caret.saturating_sub(1);
        let limit = caret_y + viewport_height + overscan;
        let window_end = self
            .prefix
            .partition_point(|y| *y < limit)
            .max(window_start + 1)
            .min(total);
        let max = f32::from(self.scroll.max_offset().y);
        let target = clamp_scroll_offset(-(caret_y - viewport_height * 0.3), max);
        if px(target) != self.scroll.offset().y {
            self.scroll
                .set_offset(point(self.scroll.offset().x, px(target)));
        }
        window_start..window_end
    }

    /// Map a pointer position to a source offset and update the selection.
    fn select(
        &mut self,
        position: Point<Pixels>,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.source_at(position).unwrap_or(self.source.len());
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
    fn adjust_scroll(
        &mut self,
        document: &WysiwygDocument,
        cursor: usize,
        typewriter: bool,
        viewport_height: f32,
    ) {
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
        if viewport.size.height <= px(0.) || viewport_height <= 0. {
            return;
        }
        let offset = self.scroll.offset();
        // Both positions are from the same laid-out frame and include scrolling.
        let content_y = f32::from(caret.y) - f32::from(viewport.origin.y);
        let mut target = f32::from(offset.y);
        if typewriter {
            let centre = (viewport_height - f32::from(height)) / 2.;
            target += centre - content_y;
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

    fn selection_bounds(
        document: &WysiwygDocument,
        selection: &Range<usize>,
        geometry: &HashMap<u64, LeafGeometry>,
    ) -> Option<Bounds<Pixels>> {
        if selection.is_empty() {
            return None;
        }
        let mut bounds: Option<Bounds<Pixels>> = None;
        for (key, leaf) in geometry {
            let Some(block) = document.blocks.get((*key as usize).saturating_sub(1)) else {
                continue;
            };
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
        bounds
    }
}

fn block_key(index: usize) -> u64 {
    index as u64 + 1
}

/// Extra pixels rendered above and below the viewport.
const WINDOW_OVERSCAN_PX: f32 = 400.;

/// Gap the block renderer leaves below a paragraph, at the default 0.75 rem.
fn paragraph_gap_px(font_size: f32) -> f32 {
    font_size * 0.75
}

/// A deterministic height estimate for a block that has not been laid out.
///
/// The estimate only has to keep the scroll range and the rendered window
/// consistent; measured heights replace it as soon as the block is visible.
/// Line counts come from the byte length and an average glyph width, which
/// overestimates CJK text (safe: it renders a little more, never less).
fn estimate_block_height(
    block: &astrolith_document::RichBlock,
    width: f32,
    font_size: f32,
    line_height: f32,
    gap: f32,
) -> f32 {
    let characters_per_line = (width / (font_size * 0.55)).max(8.);
    let (scale, extra) = match block.kind {
        BlockKind::Heading(level) => (heading_scale(level), 0.),
        BlockKind::Rule => (1., 1. + 12.),
        BlockKind::Code { .. } => (1., 12.),
        BlockKind::ListItem { .. } | BlockKind::Quote | BlockKind::Paragraph => (1., 0.),
    };
    let lines = (block.text.text.len() as f32 / characters_per_line)
        .ceil()
        .max(1.);
    lines * line_height * scale + gap + extra
}

fn heading_scale(level: u8) -> f32 {
    match level {
        1 => 2.0,
        2 => 1.6,
        3 => 1.4,
        4 => 1.2,
        5 => 1.1,
        _ => 1.05,
    }
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
            return div().size_full().into_any_element();
        };
        // The visible text is copied only when the buffer revision changed;
        // unrelated frames never materialise the document.
        let revision = session.read(cx).editor.read(cx).text_revision();
        if self.document_revision != Some(revision) {
            // The editing snapshot includes IME preedit before source commits it.
            let visible = session.read(cx).editor.read(cx).text().to_string();
            self.sync_document(revision, &visible);
        }
        let (cursor, selection, focused, modes, placeholders) = {
            let session = session.read(cx);
            let editor = session.editor.read(cx);
            (
                editor.cursor().min(editor.text().len()),
                editor.selected_range(),
                editor.focus_handle(cx).is_focused(window),
                session.presentation.modes,
                session.placeholder_ranges().to_vec(),
            )
        };
        let copyedit = session.read(cx).copyedit.render_ranges(cursor);
        let document = self.document.clone();
        let viewport_height = self.effective_viewport_height(window);
        let content_width = f32::from(self.geometry.borrow().viewport.size.width);
        self.content_width = (content_width > 0.).then_some(content_width);
        let follow_cursor = self.last_cursor != Some(cursor);
        self.last_cursor = Some(cursor);
        if follow_cursor {
            self.adjust_scroll(
                &document,
                cursor,
                focused && modes.typewriter,
                viewport_height,
            );
        }
        let rendered = self.visible_block_range(&document, cursor, viewport_height, follow_cursor);
        #[cfg(test)]
        {
            self.rendered_blocks = rendered.len();
            self.rendered_range = rendered.clone();
            self.first_rendered_range
                .get_or_insert_with(|| rendered.clone());
        }
        let style = self.style.clone();
        let mut decorations: HashMap<u64, Vec<(Range<usize>, Hsla)>> = HashMap::new();
        let selection_color = cx.theme().selection.opacity(0.35);
        let placeholder_color = cx.theme().primary.opacity(0.18);
        // Focus mode dims every block outside the cursor sentence. The sentence
        // is only scanned when the mode is on; the rendered window is all that
        // needs the predicate.
        let focus_sentence = if modes.focus {
            cursor_sentence(&self.source, cursor)
        } else {
            None
        };

        let mut nodes = Vec::with_capacity(rendered.len());
        for index in rendered.clone() {
            let block = &document.blocks[index];
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
        let mut blocks: Vec<AnyElement> = Vec::with_capacity(rendered.len() + 2);
        // The omitted blocks keep their scroll range through spacers whose
        // heights are the same estimates the window was computed from.
        let total = document.blocks.len();
        let prefix = &self.prefix;
        if rendered.start > 0 && prefix.len() == total + 1 {
            blocks.push(
                div()
                    .w_full()
                    .h(px(prefix[rendered.start]))
                    .into_any_element(),
            );
        }
        for (local, index) in rendered.clone().enumerate() {
            let block = &document.blocks[index];
            let element = nodes[local].render(
                local,
                FlowPosition::at(&refs, local, local + 1 == refs.len()),
                &render_context,
                window,
                cx,
            );
            let dimmed = modes.focus
                && focus_sentence.as_ref().is_none_or(|sentence| {
                    !(sentence.start < block.source.end && sentence.end > block.source.start)
                });
            blocks.push(block_wrapper(block, element, dimmed, cx));
        }
        if rendered.end < total && prefix.len() == total + 1 {
            let omitted = prefix[total] - prefix[rendered.end];
            blocks.push(div().w_full().h(px(omitted.max(0.))).into_any_element());
        }

        let caret = (focused
            && selection.is_empty()
            && session
                .read(cx)
                .editor
                .read(cx)
                .is_cursor_visible(window, cx))
        .then_some(cursor);
        let caret_color = cx.theme().caret;
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
            geometry: self.geometry.clone(),
            heights: self.heights.clone(),
            document: document.clone(),
            caret,
            caret_color,
            input: cx.entity(),
        };
        let content = div()
            .id("writer-wysiwyg-content")
            .test_support()
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
                    // The writer chose its input model; ancestors must not steal focus.
                    window.prevent_default();
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

        let editor = session.read(cx).editor.clone();
        let root = div()
            .id(("input", editor.entity_id()))
            .size_full()
            .child(content);
        editor
            .update(cx, |editor, cx| editor.bind_input_actions(root, window, cx))
            .test_support()
            .into_any_element()
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
    let mut events = Vec::with_capacity(text.marks.len() * 2);
    for (index, mark) in text.marks.iter().enumerate() {
        let start = mark.range.start.min(text.text.len());
        let end = mark.range.end.min(text.text.len());
        if start < end {
            cuts.extend([start, end]);
            events.extend([(start, true, index), (end, false, index)]);
        }
    }
    events.sort_unstable();
    let mut event_index = 0;
    let mut active = BTreeSet::new();
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::new();
    for pair in cuts.windows(2) {
        let start = pair[0];
        let end = pair[1];
        if start >= end {
            continue;
        }
        while let Some(&(offset, entering, index)) = events.get(event_index) {
            if offset > start {
                break;
            }
            if entering {
                active.insert(index);
            } else {
                active.remove(&index);
            }
            event_index += 1;
        }
        let mut style = TextMark::default();
        for index in &active {
            let mark = &text.marks[*index];
            {
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
    /// Per-block measured heights, owned by the surface across frames.
    heights: Rc<RefCell<Vec<Option<f32>>>>,
    document: Arc<WysiwygDocument>,
    caret: Option<usize>,
    caret_color: Hsla,
    input: gpui_kit::Entity<WriterSurface>,
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
        // Leaf height is independent of the scroll position, so a frame that
        // laid a block out is enough to measure it for later windows.
        {
            let mut heights = self.heights.borrow_mut();
            for (key, leaf) in &leaves {
                if let Some(slot) = heights.get_mut((*key as usize).saturating_sub(1)) {
                    *slot = Some(f32::from(leaf.bounds().size.height));
                }
            }
        }
        *self.geometry.borrow_mut() = FrameGeometry {
            leaves: leaves.clone(),
            viewport: bounds.intersect(&window.content_mask().bounds),
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
        let input = self.input.clone();
        input.update(cx, |input, cx| {
            input.register_input(input.geometry.borrow().viewport, window, cx)
        });
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
    use astrolith_core::{ProjectId, SourcePath};
    use astrolith_document::DocumentIdentity;
    use gpui_kit::{
        Entity, TestAppContext, WindowHandle, WindowOptions, base::Root, test::TestWindowExt,
    };

    struct WriterHost {
        writer: Entity<WriterSurface>,
        focus: gpui_kit::FocusHandle,
    }

    impl Render for WriterHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .track_focus(&self.focus)
                .child(self.writer.clone())
        }
    }

    fn mount_writer(
        cx: &mut TestAppContext,
        source: &str,
    ) -> (WindowHandle<Root>, Entity<DocumentSession>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            let mut session = None;
            let (handle, _) = gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let identity = DocumentIdentity::new(
                    ProjectId::new("window").unwrap(),
                    1,
                    SourcePath::new("src/posts/entry.md").unwrap(),
                );
                let created =
                    cx.new(|cx| DocumentSession::new(identity, source.to_owned(), window, cx));
                let writer = created.read(cx).writer.clone();
                session = Some(created);
                cx.new(|cx| WriterHost {
                    writer,
                    focus: cx.focus_handle(),
                })
            })
            .unwrap();
            (
                handle.downcast::<Root>().unwrap(),
                session.expect("the writer session must exist"),
            )
        })
    }

    #[gpui_kit::test]
    fn writer_click_keeps_native_input_focus_inside_a_focusable_panel(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, "# Heading\n\nEditable paragraph.\n");
        let editor = session.read_with(cx, |session, _| session.editor.clone());
        let before = editor.read_with(cx, |editor, _| editor.value().len());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("writer-wysiwyg-content", cx);
            assert!(editor.read(cx).focus_handle(cx).is_focused(window));
            window.input("中", cx);
        })
        .unwrap();
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.value().len()),
            before + "中".len()
        );
        session.read_with(cx, |session, _| {
            assert!(session.buffer.current_text().contains('中'));
        });
    }

    #[gpui_kit::test]
    fn writer_native_composition_uses_visible_geometry_and_keeps_hidden_source(
        cx: &mut TestAppContext,
    ) {
        use gpui_kit::{ElementInputHandler, EntityInputHandler, test::TestInput};
        let source = "---\ntitle: Exact\n---\n# Hello\n";
        let (handle, session) = mount_writer(cx, source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            session.update(cx, |session, cx| {
                session.set_writer_selection(2, 2, window, cx)
            });
            window.render_frame(cx);
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let caret = writer.update(cx, |writer, cx| {
                writer.bounds_for_range(2..2, bounds, window, cx)
            });
            assert!(
                caret.is_some(),
                "IME candidate geometry comes from the visible heading"
            );
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.compose_text(None, "に🦀", Some(3..3), cx);
            assert_eq!(input.marked_text_range(cx), Some(2..5));
            drop(input);
            assert!(
                writer.read(cx).source.contains("に🦀"),
                "preedit is visible before commit"
            );
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.compose_text(None, "日本", Some(2..2), cx);
        })
        .unwrap();
        cx.update_window(handle.into(), |_, window, cx| {
            let bounds = writer.read(cx).geometry.borrow().viewport;
            let mut input =
                TestInput::new(ElementInputHandler::new(bounds, writer.clone()), window);
            input.commit_text("日本語", cx);
            assert_eq!(input.marked_text_range(cx), None);
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                "---\ntitle: Exact\n---\n# 日本語Hello\n"
            )
        });
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.undo_source(window, cx);
            });
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.buffer.current_text(),
                source,
                "one undo removes the whole composition"
            )
        });
    }

    #[gpui_kit::test]
    fn long_paragraph_keeps_offscreen_caret_geometry_after_resize(cx: &mut TestAppContext) {
        let source = "中文日本語🦀 **bold** `code` ".repeat(1_000);
        let (handle, session) = mount_writer(cx, &source);
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        let end = source.len();
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writer_selection(end, end, window, cx)
            });
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(writer.scroll.offset().y < px(-1_000.));
            let geometry = writer.geometry.borrow();
            let leaf = geometry.leaves.get(&block_key(0)).unwrap();
            assert!(leaf.bounds().size.height > px(1_000.));
            let rendered_end = writer.document.blocks[0].text.text.len();
            let caret = leaf.position_for_index(rendered_end);
            assert!(geometry.viewport.contains(&caret));
        });
        cx.simulate_window_resize(handle.into(), size(px(900.), px(700.)));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            let geometry = writer.geometry.borrow();
            let leaf = geometry.leaves.get(&block_key(0)).unwrap();
            assert!(
                leaf.position_for_index(writer.document.blocks[0].text.text.len())
                    .y
                    >= leaf.bounds().top()
            );
        });
    }

    #[gpui_kit::test]
    fn writer_scrolling_does_not_snap_back_to_an_unchanged_caret(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        writer.update(cx, |writer, cx| {
            writer.scroll.set_offset(point(px(0.), px(-20_000.)));
            cx.notify();
        });
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(writer.rendered_range.start > 0);
            assert!(writer.scroll.offset().y < px(-1_000.));
        });
    }

    fn window_source(paragraphs: usize) -> String {
        let mut source = String::new();
        for index in 0..paragraphs {
            source.push_str(&format!(
                "Paragraph {index} with a handful of ordinary words for the window test.\n\n"
            ));
        }
        source
    }

    #[gpui_kit::test]
    fn writer_windows_from_the_first_layout_and_keeps_the_caret_block(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        let (total, first) = writer.read_with(cx, |writer, _| {
            (
                writer.document.blocks.len(),
                writer
                    .first_rendered_range
                    .clone()
                    .expect("a frame has been laid out"),
            )
        });
        assert!(
            first.len() < total,
            "the first frame must also stay windowed"
        );
        assert!(first.contains(&0));
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(
                writer.rendered_blocks < total,
                "later frames render a window, not all {total} blocks"
            );
            assert!(
                writer.rendered_range.contains(&0),
                "the caret block stays rendered: {:?}",
                writer.rendered_range
            );
        });

        // An edit keeps the measured heights, so the first frame after a
        // keystroke stays windowed instead of rendering the whole document.
        cx.update_window(handle.into(), |_, window, cx| {
            let editor = session.read_with(cx, |session, _| session.editor.clone());
            editor.update(cx, |editor, cx| {
                let mut value = editor.value().to_string();
                value.push_str("A trailing sentence after the edit.\n\n");
                editor.set_value(value, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(
                writer.rendered_blocks < writer.document.blocks.len(),
                "an edit must not fall back to a full render"
            );
        });
    }

    #[gpui_kit::test]
    fn scrolling_keeps_the_caret_block_and_hit_testing_aligned(cx: &mut TestAppContext) {
        let (handle, session) = mount_writer(cx, &window_source(2_000));
        let writer = session.read_with(cx, |session, _| session.writer.clone());
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        let target = 1_000;
        let offset = writer.read_with(cx, |writer, _| writer.document.blocks[target].content.start);
        // Move the caret into the middle: no rendered geometry exists for that
        // block yet, so the window must follow the caret's estimated position.
        cx.update_window(handle.into(), |_, window, cx| {
            session.update(cx, |session, cx| {
                session.set_writer_selection(offset, offset, window, cx);
            });
            window.render_frame(cx);
        })
        .unwrap();
        writer.read_with(cx, |writer, _| {
            assert!(
                writer.rendered_range.contains(&target),
                "the caret block is rendered after the jump: {:?}",
                writer.rendered_range
            );
            assert!(
                writer.rendered_range.start > 0,
                "a mid-document caret must not fall back to the top"
            );
        });
        cx.update_window(handle.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();
        let (range, index, hit) = writer.read_with(cx, |writer, _| {
            let range = writer.rendered_range.clone();
            let index = range.start + range.len() / 2;
            let bounds = writer
                .geometry
                .borrow()
                .leaves
                .get(&block_key(index))
                .expect("a rendered block reports geometry")
                .bounds();
            let position = bounds.center();
            (range.clone(), index, writer.source_at(position))
        });
        assert!(range.contains(&target), "the caret stays in the window");
        let hit = hit.expect("hit testing maps a rendered leaf");
        let source = writer.read_with(cx, |writer, _| writer.document.blocks[index].source.clone());
        assert!(
            source.start <= hit && hit <= source.end,
            "hit {hit} must land in the rendered block's source range {source:?}"
        );
    }

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
