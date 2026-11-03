//! Copyedit presentation: category colours, tracked decoration layers and the
//! debounced background analysis tickets.
//!
//! Analysis never runs on the render path. A text change clears the layers and
//! issues a new generation; the editor waits for the debounce, classifies the
//! visible snapshot on the blocking service lane, and applies the result only
//! if that generation is still current. Each category owns one tracked
//! [`RangeDecorationCollection`], so a new result replaces a layer with one
//! `set` call instead of accumulating stale ranges.

use std::ops::Range;

use astrolith_core::preferences::{CopyeditCategory, CopyeditColors, CopyeditPreferences};
use astrolith_document::copyedit::{CopyeditSpan, analyze};
use gpui_kit::component::input::{
    EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle,
};
use gpui_kit::{App, Entity, Hsla};

use crate::theme::color_from_hex;

/// Wait this long after the last text change before classifying.
pub(crate) const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// Bytes on either side of the caret that stay undecorated.
const ACTIVE_WINDOW: usize = 2;

/// Fill opacity; the persisted colour supplies the hue, not the intensity.
const FILL_OPACITY: f32 = 0.28;

/// Resolved Copyedit toggles and the semantic colour for each category.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CopyeditConfig {
    preferences: CopyeditPreferences,
    colors: [Hsla; CopyeditCategory::ALL.len()],
}

impl Default for CopyeditConfig {
    fn default() -> Self {
        Self::from_preferences(&CopyeditPreferences::default())
    }
}

impl CopyeditConfig {
    /// Resolve persisted strings into concrete colours once per preference change.
    pub(crate) fn from_preferences(preferences: &CopyeditPreferences) -> Self {
        let defaults = CopyeditColors::default();
        let colors = std::array::from_fn(|index| {
            let category = CopyeditCategory::ALL[index];
            color_from_hex(preferences.colors.color(category))
                .or_else(|| color_from_hex(defaults.color(category)))
                .unwrap_or_else(Hsla::transparent_black)
        });
        Self {
            preferences: preferences.clone(),
            colors,
        }
    }

    /// Whether one category's layer is active.
    pub(crate) fn enabled(&self, category: CopyeditCategory) -> bool {
        self.preferences.enabled(category)
    }

    /// Whether any category would decorate.
    pub(crate) fn any_enabled(&self) -> bool {
        CopyeditCategory::ALL
            .iter()
            .any(|category| self.enabled(*category))
    }

    /// The semantic colour persisted for one category.
    pub(crate) fn color(&self, category: CopyeditCategory) -> Hsla {
        self.colors[index(category)]
    }
}

/// Generation of one background classification request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CopyeditTicket {
    generation: u64,
}

/// Monotonic request generations; only the newest may apply its result.
#[derive(Clone, Debug)]
pub(crate) struct CopyeditSchedule {
    current: u64,
    next: u64,
}

impl Default for CopyeditSchedule {
    fn default() -> Self {
        Self {
            current: 0,
            next: 1,
        }
    }
}

impl CopyeditSchedule {
    /// Start a new generation and return its ticket.
    pub(crate) fn issue(&mut self) -> CopyeditTicket {
        let generation = self.next;
        self.next += 1;
        self.current = generation;
        CopyeditTicket { generation }
    }

    /// Drop every issued ticket, e.g. while the text is being edited or when
    /// no category is enabled.
    pub(crate) fn cancel(&mut self) {
        self.current = self.next;
        self.next += 1;
    }

    /// Whether a ticket still owns the newest request.
    pub(crate) fn is_current(&self, ticket: CopyeditTicket) -> bool {
        self.current != 0 && ticket.generation == self.current
    }
}

/// One tracked decoration layer per category plus the last applied analysis.
pub(crate) struct CopyeditPresentation {
    config: CopyeditConfig,
    schedule: CopyeditSchedule,
    analysis: Vec<CopyeditSpan>,
    analysis_ticket: Option<CopyeditTicket>,
    layers: [Option<RangeDecorationCollection>; CopyeditCategory::ALL.len()],
    /// What the layers currently show: the analysis ticket, the config
    /// generation and the suppressed span indices. A caret move inside the
    /// same token therefore does not rebuild anything.
    applied: Option<(Option<CopyeditTicket>, u64, Vec<usize>)>,
    config_revision: u64,
}

impl Default for CopyeditPresentation {
    fn default() -> Self {
        Self {
            config: CopyeditConfig::default(),
            schedule: CopyeditSchedule::default(),
            analysis: Vec::new(),
            analysis_ticket: None,
            layers: std::array::from_fn(|_| None),
            applied: None,
            config_revision: 0,
        }
    }
}

impl CopyeditPresentation {
    /// Install a new configuration and repaint from the retained analysis.
    ///
    /// Returns `true` when the configuration changed, so the session can
    /// request a fresh analysis if none exists yet.
    pub(crate) fn set_config(
        &mut self,
        editor: &Entity<EditorState>,
        config: CopyeditConfig,
        cursor: usize,
        cx: &mut App,
    ) -> bool {
        if self.config == config {
            return false;
        }
        self.config = config;
        self.config_revision += 1;
        self.applied = None;
        self.reapply(editor, cursor, cx);
        true
    }

    /// The resolved configuration, for command labels and availability.
    pub(crate) fn config(&self) -> &CopyeditConfig {
        &self.config
    }

    /// Whether a result for the current text is retained.
    pub(crate) fn has_analysis(&self) -> bool {
        self.analysis_ticket.is_some()
    }

    /// Clear the layers and invalidate every in-flight result.
    pub(crate) fn invalidate(&mut self, cx: &mut App) {
        self.schedule.cancel();
        self.analysis.clear();
        self.analysis_ticket = None;
        self.applied = None;
        for layer in &mut self.layers {
            if let Some(layer) = layer.take() {
                layer.clear(cx);
            }
        }
    }

    /// Whether a ticket still owns the newest request.
    pub(crate) fn is_current(&self, ticket: CopyeditTicket) -> bool {
        self.schedule.is_current(ticket)
    }

    /// Issue a generation for an analysis about to be scheduled.
    pub(crate) fn issue(&mut self) -> CopyeditTicket {
        self.schedule.issue()
    }

    /// Apply one background result if it is still the newest request.
    pub(crate) fn apply(
        &mut self,
        editor: &Entity<EditorState>,
        ticket: CopyeditTicket,
        analysis: Vec<CopyeditSpan>,
        cursor: usize,
        cx: &mut App,
    ) -> bool {
        if !self.schedule.is_current(ticket) {
            return false;
        }
        self.analysis = analysis;
        self.analysis_ticket = Some(ticket);
        self.applied = None;
        self.reapply(editor, cursor, cx);
        true
    }

    /// Follow a caret move without reclassifying the text.
    pub(crate) fn refresh_cursor(
        &mut self,
        editor: &Entity<EditorState>,
        cursor: usize,
        cx: &mut App,
    ) {
        if self.analysis_ticket.is_none() || !self.config.any_enabled() {
            return;
        }
        self.reapply(editor, cursor, cx);
    }

    fn reapply(&mut self, editor: &Entity<EditorState>, cursor: usize, cx: &mut App) {
        let suppressed = self.suppressed_indices(cursor);
        let key = (self.analysis_ticket, self.config_revision, suppressed);
        if self.applied.as_ref() == Some(&key) {
            return;
        }
        let ranges = decorated_ranges(&self.analysis, cursor, &self.config);
        for (index, category) in CopyeditCategory::ALL.iter().enumerate() {
            let decorations = if self.config.enabled(*category) {
                ranges[index]
                    .iter()
                    .cloned()
                    .map(|range| {
                        RangeDecoration::new(range)
                            .with_style(RangeDecorationStyle::Fill)
                            .with_color(self.config.color(*category).opacity(FILL_OPACITY))
                    })
                    .collect()
            } else {
                Vec::new()
            };
            self.layers[index] = Some(match self.layers[index].take() {
                Some(layer) => {
                    layer.set(decorations, cx);
                    layer
                }
                None => editor.update(cx, |editor, cx| {
                    editor.create_range_decorations_collection(decorations, cx)
                }),
            });
        }
        self.applied = Some(key);
    }

    fn suppressed_indices(&self, cursor: usize) -> Vec<usize> {
        let start = cursor.saturating_sub(ACTIVE_WINDOW);
        let end = cursor.saturating_add(ACTIVE_WINDOW);
        let mut index = self
            .analysis
            .partition_point(|span| span.range.end <= start);
        let mut suppressed = Vec::new();
        while let Some(span) = self.analysis.get(index) {
            if span.range.start > end {
                break;
            }
            if self.config.enabled(span.category) && intersects_active_window(&span.range, cursor) {
                suppressed.push(index);
            }
            index += 1;
        }
        suppressed
    }

    /// The ranges currently tracked by one category layer.
    #[cfg(test)]
    pub(crate) fn ranges(&self, category: CopyeditCategory, cx: &App) -> Vec<Range<usize>> {
        self.layers[index(category)]
            .as_ref()
            .map(|layer| layer.get_ranges(cx))
            .unwrap_or_default()
    }
}

/// Build per-category ranges, leaving the active token and its two-byte
/// neighbourhood undecorated.
fn decorated_ranges(
    analysis: &[CopyeditSpan],
    cursor: usize,
    config: &CopyeditConfig,
) -> [Vec<Range<usize>>; CopyeditCategory::ALL.len()] {
    let mut ranges: [Vec<Range<usize>>; CopyeditCategory::ALL.len()] =
        std::array::from_fn(|_| Vec::new());
    for span in analysis {
        if !config.enabled(span.category) || intersects_active_window(&span.range, cursor) {
            continue;
        }
        ranges[index(span.category)].push(span.range.clone());
    }
    ranges
}

/// Inclusive byte window: the caret's own bytes and two on either side.
fn intersects_active_window(range: &Range<usize>, cursor: usize) -> bool {
    let start = cursor.saturating_sub(ACTIVE_WINDOW);
    let end = cursor.saturating_add(ACTIVE_WINDOW);
    range.start <= end && range.end > start
}

fn index(category: CopyeditCategory) -> usize {
    match category {
        CopyeditCategory::Noun => 0,
        CopyeditCategory::Verb => 1,
        CopyeditCategory::Adjective => 2,
        CopyeditCategory::Adverb => 3,
        CopyeditCategory::Conjunction => 4,
    }
}

/// Classify one visible snapshot. Runs on the service lane.
pub(crate) fn classify(visible: String) -> Vec<CopyeditSpan> {
    analyze(&visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::input::EditorState;
    use gpui_kit::{AppContext, TestAppContext, WindowOptions, base::Root};

    fn span(range: Range<usize>, category: CopyeditCategory) -> CopyeditSpan {
        CopyeditSpan { range, category }
    }

    fn enabled(categories: &[CopyeditCategory]) -> CopyeditConfig {
        let mut preferences = CopyeditPreferences::default();
        for category in CopyeditCategory::ALL {
            preferences.set_enabled(category, categories.contains(&category));
        }
        CopyeditConfig::from_preferences(&preferences)
    }

    #[test]
    fn persisted_colors_become_semantic_category_colors() {
        let mut preferences = CopyeditPreferences::default();
        preferences.colors.noun = "#112233".into();
        preferences.colors.verb = "not-a-color".into();
        let config = CopyeditConfig::from_preferences(&preferences);
        assert_eq!(
            config.color(CopyeditCategory::Noun),
            color_from_hex("#112233").unwrap()
        );
        assert_eq!(
            config.color(CopyeditCategory::Verb),
            color_from_hex(&CopyeditColors::default().verb).unwrap(),
            "an invalid persisted value falls back to the category default"
        );
    }

    #[test]
    fn active_window_suppresses_the_token_and_two_bytes_around_the_caret() {
        let spans = [
            span(0..4, CopyeditCategory::Noun),
            span(10..14, CopyeditCategory::Verb),
            span(20..24, CopyeditCategory::Noun),
        ];
        let config = enabled(&[CopyeditCategory::Noun, CopyeditCategory::Verb]);
        let ranges = |cursor: usize| decorated_ranges(&spans, cursor, &config);
        assert_eq!(
            ranges(0)[0],
            vec![20..24],
            "the token under the caret is dropped"
        );
        assert_eq!(ranges(0)[1], vec![10..14]);
        assert_eq!(ranges(2)[0], vec![20..24]);
        assert_eq!(ranges(2)[1], vec![10..14]);
        assert_eq!(
            ranges(4)[0],
            vec![20..24],
            "just after the token is still its vicinity"
        );
        assert_eq!(ranges(4)[1], vec![10..14]);
        assert_eq!(
            ranges(8)[0],
            vec![0..4, 20..24],
            "the +2 boundary reaches the next token"
        );
        assert_eq!(ranges(8)[1], Vec::<Range<usize>>::new());
        assert_eq!(
            ranges(7)[0],
            vec![0..4, 20..24],
            "one byte further leaves the token decorated"
        );
        assert_eq!(ranges(7)[1], vec![10..14]);
        assert!(ranges(7)[2..].iter().all(Vec::is_empty));
        assert_eq!(
            ranges(25)[0],
            vec![0..4],
            "the -1 byte after a token reaches it"
        );
        assert_eq!(ranges(25)[1], vec![10..14]);
        assert_eq!(
            ranges(26)[0],
            vec![0..4, 20..24],
            "two bytes after a token leaves it decorated"
        );
        assert_eq!(ranges(26)[1], vec![10..14]);
    }

    #[test]
    fn category_ranges_never_overlap_and_unknown_tokens_stay_out() {
        let spans = [
            span(0..4, CopyeditCategory::Noun),
            span(5..9, CopyeditCategory::Verb),
            span(10..15, CopyeditCategory::Adjective),
        ];
        let config = enabled(CopyeditCategory::ALL.as_slice());
        let ranges = decorated_ranges(&spans, 100, &config);
        let mut flat: Vec<Range<usize>> = ranges.into_iter().flatten().collect();
        flat.sort_by_key(|range| range.start);
        assert!(flat.windows(2).all(|pair| pair[0].end <= pair[1].start));
        assert!(decorated_ranges(&[], 0, &config).iter().all(Vec::is_empty));
    }

    #[test]
    fn schedule_drops_superseded_tickets() {
        let mut schedule = CopyeditSchedule::default();
        let first = schedule.issue();
        let second = schedule.issue();
        assert!(!schedule.is_current(first));
        assert!(schedule.is_current(second));
        schedule.cancel();
        assert!(!schedule.is_current(second));
    }

    #[gpui_kit::test]
    fn stale_results_are_dropped_and_new_analysis_replaces_a_layer_atomically(
        cx: &mut TestAppContext,
    ) {
        struct Host;
        impl gpui_kit::Render for Host {
            fn render(
                &mut self,
                _: &mut gpui_kit::Window,
                _: &mut gpui_kit::Context<Self>,
            ) -> impl gpui_kit::IntoElement {
                gpui_kit::div()
            }
        }
        let mut editor_slot = None;
        let (handle, _host) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                editor_slot =
                    Some(cx.new(|cx| {
                        EditorState::new(window, cx).default_value("noun verb adjective")
                    }));
                cx.new(|_| Host)
            })
            .unwrap()
        });
        let editor = editor_slot.expect("the editor is built with the window");
        let handle = handle.downcast::<Root>().unwrap();
        let config = enabled(&[CopyeditCategory::Noun, CopyeditCategory::Verb]);
        cx.update_window(handle.into(), |_, _, cx| {
            let mut presentation = CopyeditPresentation::default();
            presentation.set_config(&editor, config, 100, cx);

            let first = presentation.issue();
            let analysis = vec![
                span(0..4, CopyeditCategory::Noun),
                span(5..9, CopyeditCategory::Verb),
            ];
            assert!(presentation.apply(&editor, first, analysis, 100, cx));
            assert_eq!(presentation.ranges(CopyeditCategory::Noun, cx), vec![0..4]);
            assert_eq!(presentation.ranges(CopyeditCategory::Verb, cx), vec![5..9]);

            // A superseded ticket cannot repaint the layers.
            let second = presentation.issue();
            let stale = vec![span(0..4, CopyeditCategory::Noun)];
            assert!(!presentation.apply(&editor, first, stale, 100, cx));
            assert_eq!(presentation.ranges(CopyeditCategory::Noun, cx), vec![0..4]);
            assert_eq!(presentation.ranges(CopyeditCategory::Verb, cx), vec![5..9]);

            // The current result replaces both layers with one set call each.
            let fresh = vec![span(10..19, CopyeditCategory::Noun)];
            assert!(presentation.apply(&editor, second, fresh, 100, cx));
            assert_eq!(
                presentation.ranges(CopyeditCategory::Noun, cx),
                vec![10..19]
            );
            assert!(presentation.ranges(CopyeditCategory::Verb, cx).is_empty());
        })
        .unwrap();
    }
}
