//! Development-only native comparison entry for the M2 layout core.
//!
//! `--spike=text-layout` opens one document in the application-owned layout
//! next to the Kit `TextView` for the same source. `Tab` swaps the renderer;
//! arrow keys, Home/End, Ctrl+Home/End and clicking drive the private layout;
//! `x` inserts one character at the caret and reconciles the layout.
//! Counters reach `astrolith::perf` only when `ASTROLITH_EDITOR_PERF` is set,
//! and the spike never writes project files.
//!
//! Environment:
//!
//! - `ASTROLITH_TEXT_LAYOUT_FIXTURE`: Markdown file to open. Without it the
//!   spike renders a deterministic long CJK/emoji paragraph.
//! - `ASTROLITH_TEXT_LAYOUT_MODE`: `private` (default) or `kit`.
//! - `ASTROLITH_TEXT_LAYOUT_STEPS`: JSON array of
//!   `{ wait_ms, scroll, caret, mode, size, insert, log, quit }` steps;
//!   `scroll` is a fraction of the scrollable range, `caret` is `start` or
//!   `end`, `size` resizes the window, and `insert` edits at the caret.
//! - `ASTROLITH_TEXT_LAYOUT_EXIT_MS`: quit after this many milliseconds.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::ImageMetrics;
use astrolith_document::WysiwygDocument;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::text::{TextView, TextViewState};
use gpui_kit::{
    App, Bounds, Context, Element, ElementId, Entity, FocusHandle, GlobalElementId, IntoElement,
    KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, Pixels, Point, Render, ScrollHandle,
    Window, WindowOptions, div, point, prelude::*, px, relative, size,
};
use serde::Deserialize;

use super::{Hit, LayoutStyle, Selection, TextLayout, Viewport};
use crate::editor::perf;
use crate::editor::writing_modes::ProseStyle;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Private,
    Kit,
}

impl Mode {
    fn from_env() -> Self {
        match std::env::var("ASTROLITH_TEXT_LAYOUT_MODE").as_deref() {
            Ok("kit") => Self::Kit,
            _ => Self::Private,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Kit => "kit",
        }
    }
}

#[derive(Clone, Deserialize)]
struct Step {
    #[serde(default)]
    wait_ms: u64,
    scroll: Option<f32>,
    caret: Option<String>,
    mode: Option<String>,
    /// Window size in logical pixels.
    size: Option<[f32; 2]>,
    #[serde(default)]
    insert: bool,
    #[serde(default)]
    log: bool,
    #[serde(default)]
    quit: bool,
}

/// Open the comparison window in a development build.
pub(crate) fn open(cx: &mut App) -> anyhow::Result<()> {
    let source = fixture_source();
    let prose = ProseStyle::new(crate::theme::prose_font(cx), 16., 2., 100.);
    let style = Arc::new(LayoutStyle::from_app(&prose, cx));
    let document = Arc::new(WysiwygDocument::parse(&source));
    let mode = Mode::from_env();
    gpui_kit::open_window(WindowOptions::default(), cx, move |window, cx| {
        window.set_window_title("Astrolith · Text layout spike");
        cx.new(|cx| {
            TextLayoutSpike::new(
                source.clone(),
                document.clone(),
                style.clone(),
                mode,
                window,
                cx,
            )
        })
    })?;
    Ok(())
}

fn fixture_source() -> String {
    if let Ok(path) = std::env::var("ASTROLITH_TEXT_LAYOUT_FIXTURE")
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        tracing::info!(path, bytes = text.len(), "opened the layout fixture");
        return text;
    }
    let mut text = String::new();
    let unit = "这是一段用于验证长段落可见区布局的中文文本，包含 **强调**、`行内代码`、[链接](https://example.com) 与 emoji 🦀👩\u{200d}💻。";
    while text.len() < 102_420 {
        text.push_str(unit);
        if text.len().is_multiple_of(3) {
            text.push(' ');
        }
    }
    text
}

fn steps_from_env() -> Vec<Step> {
    std::env::var("ASTROLITH_TEXT_LAYOUT_STEPS")
        .ok()
        .and_then(|raw| match serde_json::from_str(&raw) {
            Ok(steps) => Some(steps),
            Err(error) => {
                tracing::error!(%error, "invalid ASTROLITH_TEXT_LAYOUT_STEPS");
                None
            }
        })
        .unwrap_or_default()
}

struct TextLayoutSpike {
    source: String,
    document: Arc<WysiwygDocument>,
    style: Arc<LayoutStyle>,
    width: Option<Pixels>,
    layout: Rc<RefCell<TextLayout>>,
    content_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    scroll: ScrollHandle,
    caret: Hit,
    anchor: Option<Hit>,
    mode: Mode,
    kit: Option<Entity<TextViewState>>,
    focus: FocusHandle,
    viewport_height: f32,
    follow: bool,
    logged_pending: bool,
}

impl TextLayoutSpike {
    fn new(
        source: String,
        document: Arc<WysiwygDocument>,
        style: Arc<LayoutStyle>,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let layout = Rc::new(RefCell::new(TextLayout::new(
            document.clone(),
            style.clone(),
            px(600.),
            &ImageMetrics::default(),
        )));
        let this = Self {
            source,
            document,
            style,
            width: None,
            layout,
            content_bounds: Rc::new(RefCell::new(None)),
            scroll: ScrollHandle::new(),
            caret: Hit {
                block: 0,
                rendered: 0,
            },
            anchor: None,
            mode,
            kit: None,
            focus: cx.focus_handle(),
            viewport_height: 0.,
            follow: true,
            logged_pending: false,
        };
        let steps = steps_from_env();
        if !steps.is_empty() {
            cx.spawn_in(window, async move |this, cx| {
                for step in steps {
                    if step.wait_ms > 0 {
                        cx.background_executor()
                            .timer(Duration::from_millis(step.wait_ms))
                            .await;
                    }
                    if this
                        .update_in(cx, |this, window, cx| this.apply_step(step, window, cx))
                        .is_err()
                    {
                        return;
                    }
                }
            })
            .detach();
        }
        if let Ok(raw) = std::env::var("ASTROLITH_TEXT_LAYOUT_EXIT_MS")
            && let Ok(milliseconds) = raw.parse::<u64>()
        {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(milliseconds))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    tracing::info!(
                        target: "astrolith::perf",
                        event = "text_layout_exit",
                        rows = this.layout.borrow().measured_rows(),
                        "text layout spike exit"
                    );
                    cx.quit();
                });
            })
            .detach();
        }
        this
    }

    fn apply_step(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(fraction) = step.scroll {
            let height = self.viewport_height.max(1.);
            let max = self.layout.borrow().max_scroll(height);
            let target = -fraction.clamp(0., 1.) * max;
            self.scroll.set_offset(point(px(0.), px(target)));
            self.follow = false;
            let entity = cx.entity().entity_id();
            window.on_next_frame(move |_, cx| cx.notify(entity));
        }
        if let Some(caret) = step.caret.as_deref() {
            self.follow = true;
            match caret {
                "end" => self.jump_to_end(),
                "start" => {
                    self.caret = Hit {
                        block: 0,
                        rendered: 0,
                    };
                }
                other => tracing::warn!(other, "unknown spike caret target"),
            }
        }
        if let Some(mode) = step.mode.as_deref() {
            self.mode = if mode == "kit" {
                Mode::Kit
            } else {
                Mode::Private
            };
        }
        if let Some([width, height]) = step.size {
            window.resize(size(px(width.max(200.)), px(height.max(200.))));
        }
        if step.insert {
            self.insert_at_caret(cx);
        }
        if step.log {
            let layout = self.layout.borrow();
            let stats = layout.stats();
            let ime = layout.bounds_for_range(self.caret, self.caret);
            tracing::info!(
                target: "astrolith::perf",
                event = "text_layout_step",
                mode = self.mode.label(),
                block = self.caret.block,
                rendered = self.caret.rendered,
                rows = layout.measured_rows(),
                total_height = layout.total_height(),
                pending = stats.pending,
                pending_bytes = layout.pending_bytes(self.caret.block),
                ime_x = ime.map(|bounds| f32::from(bounds.origin.x)),
                ime_y = ime.map(|bounds| f32::from(bounds.origin.y)),
                ime_width = ime.map(|bounds| f32::from(bounds.size.width)),
                ime_height = ime.map(|bounds| f32::from(bounds.size.height)),
                "text layout spike step"
            );
        }
        let entity = cx.entity().entity_id();
        window.on_next_frame(move |_, cx| cx.notify(entity));
        cx.notify();
        if step.quit {
            cx.quit();
        }
    }

    fn jump_to_end(&mut self) {
        let Some(block) = self.document.blocks.len().checked_sub(1) else {
            return;
        };
        self.caret = Hit {
            block,
            rendered: self.document.blocks[block].text.text.len(),
        };
        self.anchor = None;
    }

    fn move_horizontal(&mut self, forward: bool) {
        let Some(block) = self.document.blocks.get(self.caret.block) else {
            return;
        };
        let text = &block.text.text;
        let offset = self.caret.rendered.min(text.len());
        if forward {
            if offset < text.len() {
                self.caret.rendered = offset + super::next_grapheme_len(text, offset);
            } else if self.caret.block + 1 < self.document.blocks.len() {
                self.caret.block += 1;
                self.caret.rendered = 0;
            }
        } else if offset > 0 {
            self.caret.rendered = super::previous_grapheme_start(text, offset);
        } else if self.caret.block > 0 {
            self.caret.block -= 1;
            self.caret.rendered = self.document.blocks[self.caret.block].text.text.len();
        }
        self.anchor = None;
        self.follow = true;
    }

    fn move_vertical(&mut self, up: bool) {
        if let Some(target) = self.layout.borrow().vertical_target(self.caret, up) {
            self.caret = target;
            self.anchor = None;
            self.follow = true;
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        let shift = event.keystroke.modifiers.shift;
        let handled = match (key, command) {
            ("tab", _) => {
                self.mode = match self.mode {
                    Mode::Private => Mode::Kit,
                    Mode::Kit => Mode::Private,
                };
                true
            }
            ("home", false) => {
                self.caret.rendered = 0;
                self.anchor = if shift { self.anchor } else { None };
                self.follow = true;
                true
            }
            ("end", false) => {
                if let Some(block) = self.document.blocks.get(self.caret.block) {
                    self.caret.rendered = block.text.text.len();
                }
                self.anchor = if shift { self.anchor } else { None };
                self.follow = true;
                true
            }
            ("home", true) => {
                self.caret = Hit {
                    block: 0,
                    rendered: 0,
                };
                self.anchor = None;
                self.follow = true;
                true
            }
            ("end", true) => {
                self.jump_to_end();
                true
            }
            ("left", false) => {
                self.move_horizontal(false);
                true
            }
            ("right", false) => {
                self.move_horizontal(true);
                true
            }
            ("up", false) => {
                self.move_vertical(true);
                true
            }
            ("down", false) => {
                self.move_vertical(false);
                true
            }
            ("x", false) => {
                self.insert_at_caret(cx);
                true
            }
            _ => false,
        };
        if handled {
            cx.notify();
            cx.stop_propagation();
        }
    }

    fn insert_at_caret(&mut self, cx: &mut Context<Self>) {
        let Some(block) = self.document.blocks.get(self.caret.block) else {
            return;
        };
        let Some(offset) = block.text.source_offset(self.caret.rendered) else {
            return;
        };
        if offset > self.source.len() {
            return;
        }
        let inserted = "字";
        self.source.insert_str(offset, inserted);
        let document = Arc::new(WysiwygDocument::parse(&self.source));
        let outcome = self.layout.borrow_mut().update(document.clone());
        perf::text_layout_edit(outcome);
        if let Some((block, rendered)) = document.caret_location(offset + inserted.len()) {
            self.caret = Hit { block, rendered };
        }
        self.document = document;
        self.follow = true;
        self.logged_pending = false;
        cx.notify();
    }

    fn on_click(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(bounds) = *self.content_bounds.borrow() else {
            return;
        };
        let content = point(
            event.position.x - bounds.origin.x,
            event.position.y - bounds.origin.y,
        );
        if let Some(hit) = self.layout.borrow().hit_test(content) {
            self.anchor = event.modifiers.shift.then_some(self.caret);
            self.caret = hit;
            self.follow = false;
            cx.notify();
        }
    }

    fn kit_state(&mut self, cx: &mut Context<Self>) -> Entity<TextViewState> {
        if self.kit.is_none() {
            self.kit = Some(cx.new(|cx| TextViewState::markdown(self.source.as_str(), cx)));
        }
        self.kit
            .as_ref()
            .expect("the Kit state was just created")
            .clone()
    }

    fn ensure_caret_visible(&mut self, caret: Point<Pixels>) {
        let scroll = -f32::from(self.scroll.offset().y);
        let y = f32::from(caret.y);
        let top = scroll + 24.;
        let bottom = scroll + self.viewport_height - 48.;
        let mut target = scroll;
        if y < top {
            target = y - 24.;
        } else if y > bottom {
            target = y - self.viewport_height + 48.;
        }
        if (target - scroll).abs() > 0.5 {
            let max = f32::from(self.scroll.max_offset().y);
            tracing::info!(
                target: "astrolith::perf",
                event = "text_layout_follow",
                caret_y = y,
                scroll,
                target,
                max,
                viewport_height = self.viewport_height,
                "spike caret follow"
            );
            self.scroll
                .set_offset(point(self.scroll.offset().x, px(-target.clamp(0., max))));
        }
    }

    fn clamp_scroll(&mut self, total: f32) {
        let viewport = self.viewport_height.max(1.);
        let max = (total - viewport).max(0.);
        let offset = f32::from(self.scroll.offset().y);
        if offset < -max {
            self.scroll
                .set_offset(point(self.scroll.offset().x, px(-max)));
        }
    }
}

impl Render for TextLayoutSpike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let width = (viewport.width - px(80.)).max(px(200.));
        self.viewport_height = f32::from(viewport.height);
        if self.width != Some(width) {
            self.width = Some(width);
            self.layout = Rc::new(RefCell::new(TextLayout::new(
                self.document.clone(),
                self.style.clone(),
                width,
                &ImageMetrics::default(),
            )));
            self.content_bounds.borrow_mut().take();
            self.logged_pending = false;
        }
        if self.mode == Mode::Kit {
            let state = self.kit_state(cx);
            let style = self.style.clone();
            return div()
                .size_full()
                .bg(cx.theme().background)
                .p(px(40.))
                .track_focus(&self.focus)
                .on_key_down(
                    cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)),
                )
                .child(
                    div().size_full().child(
                        TextView::new(&state)
                            .scrollable(true)
                            .font_family(style.font.family.clone())
                            .text_size(style.font_size)
                            .line_height(relative(style.line_height))
                            .w_full(),
                    ),
                )
                .into_any_element();
        }

        let scroll = (-f32::from(self.scroll.offset().y)).max(0.);
        let prepare_started = Instant::now();
        let prepared = self.layout.borrow_mut().prepare(
            window,
            Viewport {
                scroll,
                height: self.viewport_height,
            },
            Some(self.caret),
            self.viewport_height * 0.5,
        );
        let prepare_elapsed = prepare_started.elapsed();
        let caret_point = {
            let layout = self.layout.borrow();
            let stats = layout.stats();
            perf::text_layout(
                Mode::Private.label(),
                stats,
                prepare_elapsed,
                layout.measured_rows(),
                layout.document().blocks.len(),
                layout.total_height(),
                layout.pending_bytes(self.caret.block),
                f32::from(self.width.unwrap_or(px(0.))),
                scroll,
            );
            if prepared.pending && !self.logged_pending {
                self.logged_pending = true;
                if stats.pending {
                    tracing::info!(
                        target: "astrolith::perf",
                        event = "text_layout_pending",
                        block = self.caret.block,
                        rendered = self.caret.rendered,
                        pending_bytes = layout.pending_bytes(self.caret.block),
                        "cold position awaits more layout budgets"
                    );
                }
            }
            layout.caret(self.caret).map(|caret| caret.origin)
        };
        let mut wake = prepared.pending;
        if self.follow
            && let Some(caret) = caret_point
        {
            self.ensure_caret_visible(caret);
            self.follow = false;
            wake = true;
        }
        if wake {
            window.request_animation_frame();
        }
        let total = self.layout.borrow().total_height();
        self.clamp_scroll(total);
        let selection = self.anchor.map(|anchor| Selection {
            anchor,
            focus: self.caret,
        });
        let canvas = LayoutCanvas {
            layout: self.layout.clone(),
            content_bounds: self.content_bounds.clone(),
            selection,
            caret: Some(self.caret),
        };
        div()
            .size_full()
            .bg(cx.theme().background)
            .p(px(40.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| this.on_click(event, cx)),
            )
            .child(
                div()
                    .id("text-layout-spike-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .scrollbar_width(px(0.))
                    .track_scroll(&self.scroll)
                    .child(canvas),
            )
            .into_any_element()
    }
}

struct LayoutCanvas {
    layout: Rc<RefCell<TextLayout>>,
    content_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    selection: Option<Selection>,
    caret: Option<Hit>,
}

impl IntoElement for LayoutCanvas {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LayoutCanvas {
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
        let height = self.layout.borrow().total_height().max(1.);
        let style = gpui_kit::Style {
            size: size(
                gpui_kit::Length::Definite(relative(1.0)),
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
        _: &mut Window,
        _: &mut App,
    ) {
        *self.content_bounds.borrow_mut() = Some(bounds);
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
        self.layout.borrow_mut().paint(
            window,
            cx,
            bounds.origin,
            self.selection,
            self.caret,
            &[],
            &[],
            None,
        );
    }
}
