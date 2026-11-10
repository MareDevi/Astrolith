//! One source history and retained editing models per canonical source.
use std::{ops::Range, path::PathBuf, time::Instant};

use astrolith_document::{
    DocumentBuffer, DocumentIdentity, DocumentKind, DocumentRevision, FormatCommand, FormatOutcome,
    ProjectionSegment, TextEdit, TokenSummary, WriterProjection, apply_visible_edit,
    format_document, map_visible_offset, project_writer, token_summary,
};
use astrolith_lsp::{ClientHandle, LanguageId};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::input::{
    EditorState, InputEvent, Position, RangeDecoration, RangeDecorationCollection,
    RangeDecorationStyle, Rope, RopeExt,
};
use gpui_kit::{
    App, AppContext, Context, Entity, EntityInputHandler, EventEmitter, Focusable, Pixels, Point,
    SharedString, Subscription, Task, Window, point, px,
};

use crate::editor::component_builder::snippets::PreparedInsertion;
use crate::editor::lsp::ProviderSupport;

use super::text_change::TextChange;
use crate::editor::copyedit::{CopyeditConfig, CopyeditPresentation, DEBOUNCE};
use crate::editor::formatting::external_url;
use crate::editor::writer_surface::WriterSurface;
use crate::editor::{
    source_surface::SourceSurface,
    writing_modes::{ProseStyle, WritingModes},
};
use crate::runtime;

pub(crate) struct DocumentSession {
    /// Active input model. Complete source and writer projection retain their
    /// own display state so changing modes does not reinstall the document.
    pub editor: Entity<EditorState>,
    writer_editor: Entity<EditorState>,
    source_editor: Option<Entity<EditorState>>,
    writer_revision: DocumentRevision,
    source_revision: Option<DocumentRevision>,
    source_subscriptions: Vec<Subscription>,
    /// A selection or mode change cannot create a text edit. Avoid scanning an
    /// unchanged rope at save, command and mode boundaries.
    absorbed_text: Option<(gpui_kit::EntityId, DocumentRevision, Rope)>,
    /// Previous absorbed input snapshot, used to find only the changed bytes.
    absorbed_writer_text: Rope,
    /// Rendered writing surface; the editor remains its input model.
    pub(crate) writer: Entity<WriterSurface>,
    /// Authoritative complete source: frontmatter and MDX imports included.
    pub buffer: DocumentBuffer,
    /// The projection that produced the editor's text and its byte mapping.
    pub projection: WriterProjection,
    /// Set while this session writes its own projection into the editor, so the
    /// resulting `Change` is not treated as a user edit.
    origin: bool,
    /// Set when this session applies one of its own commands (formatting, link
    /// paste) or rewrites the projection, so the `Change` it produces is not
    /// counted as typing by the chrome. The change subscription stores the
    /// outcome in [`Self::programmatic_change`].
    writing_self: bool,
    /// Whether the last `Change` this session delivered came from its own write
    /// rather than from the user's keyboard.
    programmatic_change: bool,
    /// Complete-source transactions, one ordered stack for body and frontmatter.
    history: SourceHistory,
    /// Selection captured before the next source transaction, in source bytes.
    last_selection: Range<usize>,
    /// The input bridge owns live preedit; source snapshots keep its baseline.
    writer_preedit: Option<(Rope, Range<usize>)>,
    /// Build selects the source model; Writing selects the projected model.
    source_view: bool,
    /// Syntax language the complete-source view highlights.
    language: SharedString,
    /// The active selection and the end of the occurrence `select_next_occurrence`
    /// added last, so a repeated press continues instead of finding it again.
    next_occurrence: Option<(Range<usize>, usize)>,
    /// Set while the editor text is known to diverge from the projection because
    /// an edit was refused. Every absorb is blocked until a rebuild puts the
    /// editor back on the authoritative source.
    refused: bool,
    /// Why the last visible edit was refused instead of guessed.
    pub notice: Option<SharedString>,
    pub externally_changed: bool,
    pub dirty: bool,
    pub disk_fingerprint: u64,
    kind: DocumentKind,
    /// Focus and typewriter preferences for the visible writing surface.
    pub writing_modes: WritingModes,
    /// Copyedit category layers and the background analysis generations.
    pub copyedit: CopyeditPresentation,
    /// In-flight debounce/classification task; dropping it cancels the result.
    copyedit_task: Option<gpui_kit::Task<()>>,
    /// The word and character counts of the current visible text.
    pub counts: TokenSummary,
    /// Set when [`Self::projection`] changed, so counts only update after
    /// actual text changes.
    presentation_dirty: bool,
    /// Snippet placeholder ranges in visible bytes, in insertion order.
    placeholders: Vec<Range<usize>>,
    /// The placeholder Tab moves from next.
    placeholder_index: usize,
    /// Caret offset from each tracked decoration anchor's start.
    ///
    /// Editor decorations refuse empty ranges, so every zero-width placeholder
    /// is tracked by the character immediately before it (or after it at the
    /// start of the text); this offset maps the anchor back to the caret.
    placeholder_offsets: Vec<usize>,
    /// Tracked decoration layer covering the placeholder anchors.
    placeholder_collection: Option<RangeDecorationCollection>,
    /// Visible text length when the placeholder ranges were last resolved.
    ///
    /// An edit that grows the text without moving an anchor happened at that
    /// anchor's edge, so the caret offset advances by the inserted length.
    placeholder_text_len: usize,
    /// This document's attachment to the project's shared server, while Build
    /// shows it. The process belongs to the project, never to a session.
    lsp: Option<LspAttachment>,
    /// In-flight debounced full-text document sync.
    lsp_change: Option<Task<()>>,
    /// Last diagnostics version accepted for this document, so an older
    /// `publishDiagnostics` cannot overwrite a newer set.
    lsp_diagnostic_version: Option<i32>,
    /// What this document's server is doing, surfaced in the status line.
    lsp_status: LspStatus,
    /// Set when a failure was already reported, so the exit that follows a
    /// handshake error does not overwrite its cause.
    lsp_failure_seen: bool,
    /// Whether the current notice came from the language server, so recovery
    /// can replace exactly that notice without inspecting translated text.
    lsp_notice_pending: bool,
    /// Visible complete-source surface, when the internal M6 switch selects it.
    source_surface: Option<Entity<SourceSurface>>,
    _change: Subscription,
    _cursor: Subscription,
    _blur: Subscription,
    _placeholder_keys: Subscription,
    _history_keys: Subscription,
    _writer_keys: Subscription,
}
pub(crate) struct SessionChanged;
impl EventEmitter<SessionChanged> for DocumentSession {}

/// What this document's language server is doing, for the status line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LspStatus {
    /// No server is expected right now (Writer view, stopped, or not installed).
    Inactive,
    /// Discovery or the initialize handshake is in flight.
    Starting,
    /// The server completed its handshake and providers are installed.
    Ready,
    /// The server could not start or failed mid-session.
    Failed(String),
    /// The server process ended on its own.
    Exited(Option<i32>),
}

/// One session's binding to a project-wide language server.
///
/// The server process, its event loop and its one-retry policy belong to
/// [`crate::project_discovery::ProjectWorkspace`]; a session only names the
/// document it has open and borrows the handle its providers call.
pub(crate) struct LspAttachment {
    pub handle: ClientHandle,
    pub uri: lsp_types::Uri,
    pub language: LanguageId,
}

/// A language server asked the editor to reveal a location in another file.
#[derive(Clone, Debug)]
pub(crate) struct DefinitionTarget {
    /// Canonical file path from the server's document URI.
    pub path: PathBuf,
    /// UTF-16 selection start within that file, when the server supplied one.
    pub selection: Option<lsp_types::Position>,
}

impl EventEmitter<DefinitionTarget> for DocumentSession {}

/// Emitted when undo/redo changed the complete source outside an editor change.
pub(crate) struct HistoryChanged;
impl EventEmitter<HistoryChanged> for DocumentSession {}

/// The editor lost focus while its document was dirty.
///
/// The writer saves on blur (ledger row 44) through the coordinator's immediate
/// request, not through the idle deadline.
pub(crate) struct SaveOnBlur;
impl EventEmitter<SaveOnBlur> for DocumentSession {}

/// The initial text state one session build adopts.
struct BuildState {
    original: String,
    current: String,
    revision: DocumentRevision,
    disk_fingerprint: u64,
    externally_changed: bool,
}

impl DocumentSession {
    /// Replace the notice with one that does not come from the language server.
    fn set_notice(&mut self, notice: impl Into<SharedString>) {
        self.notice = Some(notice.into());
        self.lsp_notice_pending = false;
    }

    /// Replace the notice with a language-server message, so recovery can
    /// replace exactly this notice later.
    fn set_lsp_notice(&mut self, notice: impl Into<SharedString>) {
        self.notice = Some(notice.into());
        self.lsp_notice_pending = true;
    }

    pub fn new(
        identity: DocumentIdentity,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let disk_fingerprint = fingerprint(&source);
        Self::build(
            identity,
            BuildState {
                original: source.clone(),
                current: source,
                revision: DocumentRevision::INITIAL,
                disk_fingerprint,
                externally_changed: false,
            },
            window,
            cx,
        )
    }

    /// Build a session from a recovery record that retained its revision.
    ///
    /// `original` is the record's baseline (or the current disk for a legacy
    /// record), `disk` is the content read now, and `current` is the recovered
    /// unsaved text. When the disk no longer matches the recorded baseline the
    /// document loads as an external conflict: the first save must report
    /// [`astrolith_document::SaveOutcome::Conflict`] instead of overwriting it.
    pub fn from_recovered_at(
        identity: DocumentIdentity,
        original: String,
        current: String,
        disk: String,
        revision: DocumentRevision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let externally_changed = original != disk;
        let disk_fingerprint = fingerprint(&disk);
        Self::build(
            identity,
            BuildState {
                original,
                current,
                revision,
                disk_fingerprint,
                externally_changed,
            },
            window,
            cx,
        )
    }

    /// Bytes this session retains for its document representations.
    ///
    /// Count logical baseline/current rope sizes conservatively even when they
    /// share chunks. The writer projection and both retained input models also
    /// count toward load admission; this is text-size accounting, not a complete
    /// measurement of allocator or rendered-geometry overhead.
    pub(crate) fn retained_bytes(&self, cx: &App) -> usize {
        self.buffer.original_len()
            + self.buffer.len()
            + self.projection.visible.len()
            + self.writer_editor.read(cx).text().len()
            + self
                .source_editor
                .as_ref()
                .map_or(0, |editor| editor.read(cx).text().len())
    }

    fn build(
        identity: DocumentIdentity,
        state: BuildState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let BuildState {
            original,
            current,
            revision,
            disk_fingerprint,
            externally_changed,
        } = state;
        let kind = kind_for(identity.canonical_source().as_str());
        let language = editor_language(identity.canonical_source().as_str());
        let (projection, notice) = project_or_complete(&current, kind);
        let buffer =
            DocumentBuffer::from_unsaved_at_revision(identity, original, current, revision);
        let dirty = buffer.is_dirty();
        let counts = token_summary(&projection.visible);
        // A writing surface is not a code buffer: no line numbers, indent
        // guides, folding or automatic bracket insertion.
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .line_number(false)
                .indent_guides(false)
                .folding(false)
                .auto_close(false)
                .smart_indent(false)
                .default_value(projection.visible.clone())
        });
        let writer = {
            let weak = cx.weak_entity();
            let style = ProseStyle::new(
                crate::theme::prose_font(cx),
                16.,
                2.,
                crate::theme::default_prose_page_width_percent(),
            );
            cx.new(|cx| WriterSurface::new(weak, style, cx))
        };
        writer.update(cx, |writer, cx| {
            writer.observe_text(editor.read(cx).text().clone(), cx)
        });
        let change = cx.subscribe_in(
            &editor,
            window,
            |this, _editor, event: &InputEvent, window, cx| {
                if _editor.entity_id() == this.editor.entity_id() {
                    this.on_editor_change(event, window, cx);
                }
            },
        );
        // Cursor moves notify the editor without changing text; focus dimming and
        // typewriter centring follow the caret, so they refresh here and nowhere
        // near the render path.
        let cursor = cx.observe_in(&editor, window, |this, _editor, window, cx| {
            if _editor.entity_id() == this.editor.entity_id() {
                this.last_selection = this.current_source_selection(cx);
                this.refresh_cursor(cx);
                let editor = _editor.read(cx);
                let state = (
                    editor.focus_handle(cx).is_focused(window),
                    editor.cursor(),
                    editor.selected_range(),
                );
                this.writer
                    .update(cx, |writer, cx| writer.refresh_caret(state, cx));
            }
        });
        // Losing focus is the writer's save trigger: the workspace turns it into
        // an immediate guarded save when the buffer is dirty.
        let blur = cx.on_blur(
            &editor.read(cx).focus_handle(cx),
            window,
            |this, window, cx| {
                this.cancel_writer_preedit(window, cx);
                if this.buffer.is_dirty() {
                    cx.emit(SaveOnBlur);
                }
            },
        );
        // Tab, Shift+Tab and Escape navigate insertion placeholders before the
        // editor consumes those keys. Interceptors run before key bindings, so
        // an active placeholder set owns them; an empty set passes through.
        let placeholder_weak = cx.weak_entity();
        let placeholder_keys = cx.intercept_keystrokes({
            let weak = placeholder_weak;
            move |event, window, cx| {
                if !weak
                    .read_with(cx, |session, cx| {
                        session.editor.read(cx).focus_handle(cx).is_focused(window)
                    })
                    .unwrap_or(false)
                {
                    return;
                }
                let step = match event.keystroke.key.as_str() {
                    "tab" if event.keystroke.modifiers.shift => -1,
                    "tab" => 1,
                    "escape"
                        if !event.keystroke.modifiers.shift
                            && !event.keystroke.modifiers.control
                            && !event.keystroke.modifiers.alt
                            && !event.keystroke.modifiers.platform =>
                    {
                        0
                    }
                    _ => return,
                };
                let handled = weak
                    .update(cx, |session, cx| session.handle_placeholder_key(step, cx))
                    .unwrap_or(false);
                if handled {
                    cx.stop_propagation();
                }
            }
        });
        let history_weak = cx.weak_entity();
        let history_keys = cx.intercept_keystrokes(move |event, window, cx| {
            if !history_weak
                .read_with(cx, |session, cx| {
                    session.editor.read(cx).focus_handle(cx).is_focused(window)
                })
                .unwrap_or(false)
            {
                return;
            }
            let modifiers = event.keystroke.modifiers;
            if !modifiers.control && !modifiers.platform {
                return;
            }
            let key = event.keystroke.key.as_str();
            match key {
                "z" if modifiers.shift => {
                    let _ = history_weak.update(cx, |session, cx| session.redo_source(window, cx));
                }
                "z" => {
                    let _ = history_weak.update(cx, |session, cx| session.undo_source(window, cx));
                }
                "y" => {
                    let _ = history_weak.update(cx, |session, cx| session.redo_source(window, cx));
                }
                _ => return,
            }
            // The complete-source stack owns undo; the editor's internal stack
            // must never receive the same key.
            cx.stop_propagation();
        });
        let writer_weak = cx.weak_entity();
        let writer_keys = cx.intercept_keystrokes(move |event, window, cx| {
            if !writer_weak
                .read_with(cx, |session, cx| {
                    session.editor.read(cx).focus_handle(cx).is_focused(window)
                })
                .unwrap_or(false)
            {
                return;
            }
            let modifiers = event.keystroke.modifiers;
            if modifiers.control || modifiers.alt || modifiers.platform {
                return;
            }
            let key = event.keystroke.key.as_str();
            if !matches!(key, "left" | "right" | "up" | "down" | "home" | "end") {
                return;
            }
            let writer = writer_weak
                .read_with(cx, |session, _| {
                    (!session.source_view).then(|| session.writer.clone())
                })
                .ok()
                .flatten();
            let handled = writer.is_some_and(|writer| {
                writer.update(cx, |writer, cx| {
                    writer.navigate(key, modifiers.shift, window, cx)
                })
            });
            if handled {
                cx.stop_propagation();
            }
        });
        let absorbed_writer_text = editor.read(cx).text().clone();
        Self {
            writer_editor: editor.clone(),
            source_editor: None,
            source_surface: None,
            writer_revision: revision,
            source_revision: None,
            source_subscriptions: Vec::new(),
            absorbed_text: None,
            absorbed_writer_text,
            editor,
            writer,
            buffer,
            projection,
            origin: false,
            writing_self: false,
            programmatic_change: false,
            history: SourceHistory::default(),
            last_selection: 0..0,
            writer_preedit: None,
            source_view: false,
            language,
            next_occurrence: None,
            refused: false,
            notice,
            externally_changed,
            dirty,
            disk_fingerprint,
            kind,
            writing_modes: WritingModes::default(),
            copyedit: CopyeditPresentation::default(),
            copyedit_task: None,
            counts,
            presentation_dirty: false,
            placeholders: Vec::new(),
            placeholder_index: 0,
            placeholder_offsets: Vec::new(),
            placeholder_collection: None,
            placeholder_text_len: 0,
            lsp: None,
            lsp_change: None,
            lsp_diagnostic_version: None,
            lsp_status: LspStatus::Inactive,
            lsp_failure_seen: false,
            lsp_notice_pending: false,
            _change: change,
            _cursor: cursor,
            _blur: blur,
            _placeholder_keys: placeholder_keys,
            _history_keys: history_keys,
            _writer_keys: writer_keys,
        }
    }

    fn on_editor_change(
        &mut self,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let started = crate::editor::perf::start();
        if self.origin {
            if !self.source_view {
                self.absorbed_writer_text = self.editor.read(cx).text().clone();
            }
            // This session's own projection rewrite; the buffer is already
            // authoritative for exactly this text.
            self.origin = false;
            self.writing_self = false;
            self.programmatic_change = true;
            return;
        }
        // Compare retained input snapshots and map the changed byte range
        // back before anything reads the authoritative buffer.
        self.programmatic_change = std::mem::take(&mut self.writing_self);
        let composition = if self.writer_preedit.is_some() && !self.source_view {
            if self.editor.update(cx, |editor, cx| {
                editor.marked_text_range(window, cx).is_some()
            }) {
                self.refresh_writer(cx);
                return;
            }
            self.writer_preedit.take()
        } else {
            None
        };
        if let Some((_, selection)) = &composition {
            self.last_selection = self.projection_range_to_source(selection.clone());
        }
        let intent = if self.programmatic_change || composition.is_some() {
            HistoryIntent::Atomic
        } else {
            HistoryIntent::Typing
        };
        self.absorb_with_intent(cx, intent);
        if self.refused {
            // Fail closed: the refused text never reached the source, so put
            // the editor back on the authoritative projection (and clear the
            // latch) instead of letting the next Change overwrite it.
            self.rebuild_projection(window, cx);
        }
        self.sync_placeholders(cx);
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        if self.source_view {
            self.sync_lsp_change(cx);
        }
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        if let Some(started) = started {
            crate::editor::perf::span("source_transaction", "change_handler", started.elapsed());
        }
    }

    fn watch_source_editor(
        &mut self,
        editor: &Entity<EditorState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.source_subscriptions.push(cx.subscribe_in(
            editor,
            window,
            |this, editor, event: &InputEvent, window, cx| {
                if editor.entity_id() == this.editor.entity_id() {
                    this.on_editor_change(event, window, cx);
                }
            },
        ));
        self.source_subscriptions
            .push(cx.observe_in(editor, window, |this, editor, _, cx| {
                if editor.entity_id() == this.editor.entity_id() {
                    this.last_selection = this.current_source_selection(cx);
                    this.refresh_cursor(cx);
                }
            }));
        self.source_subscriptions.push(cx.on_blur(
            &editor.read(cx).focus_handle(cx),
            window,
            |this, _, cx| {
                if this.buffer.is_dirty() {
                    cx.emit(SaveOnBlur);
                }
            },
        ));
    }

    /// Whether the editor currently shows complete source rather than prose.
    pub(crate) fn is_source_view(&self) -> bool {
        self.source_view
    }

    /// The visible complete-source surface, when the internal switch is on.
    pub(crate) fn source_surface(&self) -> Option<&Entity<SourceSurface>> {
        self.source_surface.as_ref()
    }

    /// The source syntax this session was built for.
    pub(crate) fn kind(&self) -> DocumentKind {
        self.kind
    }

    /// Number of lines in the visible text, at least one.
    pub(crate) fn line_count(&self, _: &App) -> usize {
        self.counts.lines
    }

    /// One-based line and column of the primary cursor in the visible text.
    pub(crate) fn cursor_line_column(&self, cx: &App) -> (u32, u32) {
        let visible = self.editor.read(cx).text().to_string();
        let offset = self.editor.read(cx).cursor().min(visible.len());
        let prefix = &visible[..offset];
        let line = prefix.matches('\n').count() as u32 + 1;
        let column = prefix
            .rsplit('\n')
            .next()
            .map(|line| line.chars().count() as u32 + 1)
            .unwrap_or(1);
        (line, column)
    }

    /// Apply one formatting command as one undo transaction.
    ///
    /// The replacement is mapped through the projection before the editor is
    /// touched, so a command that cannot be placed inside visible bytes is
    /// refused and the source is kept.
    pub(crate) fn apply_format(
        &mut self,
        command: FormatCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), SharedString> {
        if self.source_view {
            let message = tr!("editor.session.formatting_unavailable");
            self.set_notice(message.clone());
            cx.notify();
            return Err(message.into());
        }
        self.absorb(cx);
        let selection = self.editor.read(cx).selected_range();
        let formatted = match format_document(
            self.buffer.current_text(),
            &self.projection,
            selection,
            command,
        ) {
            Ok(formatted) => formatted,
            Err(error) => {
                let message = tr!("editor.session.formatting_refused", error = error);
                self.set_notice(message.clone());
                cx.emit(SessionChanged);
                cx.notify();
                return Err(message.into());
            }
        };
        let unchanged = formatted.outcome.is_unchanged(&self.projection.visible);
        self.apply_outcome(&formatted.outcome, unchanged, window, cx);
        self.absorb(cx);
        debug_assert_eq!(
            self.buffer.current_text(),
            formatted.source,
            "the mapped source must be exactly the edit the editor applied"
        );
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
    }

    /// Wrap the active selection in a Markdown link to `url` as one undo step.
    ///
    /// Used by the paste hook when a URL is pasted over a selection. Returns
    /// `false` for an empty selection or a destination that is not an external
    /// `http`/`https` URL, so the paste falls through to ordinary text.
    pub(crate) fn wrap_selection_as_link(
        &mut self,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.absorb(cx);
        let Some(url) = external_url(url) else {
            return false;
        };
        let visible = &self.projection.visible;
        let selection = self.editor.read(cx).selected_range();
        if selection.is_empty() {
            return false;
        }
        let Some(selected) = visible.get(selection.clone()) else {
            return false;
        };
        let cursor = selection.start + selected.len() + url.len() + 3;
        let outcome = FormatOutcome {
            replacement: format!("[{selected}]({url})"),
            range: selection,
            selection: cursor..cursor,
        };
        if self.mapped_source(&outcome).is_err() {
            cx.emit(SessionChanged);
            cx.notify();
            return false;
        }
        self.apply_outcome(&outcome, false, window, cx);
        self.absorb(cx);
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        true
    }

    /// Validate one outcome against the projection and return the mapped source.
    fn mapped_source(&mut self, outcome: &FormatOutcome) -> Result<String, SharedString> {
        let result = apply_visible_edit(
            self.buffer.current_text(),
            &self.projection,
            TextEdit::replace(outcome.range.clone(), outcome.replacement.clone()),
        );
        match result {
            Ok(source) => Ok(source),
            Err(error) => {
                let message = tr!("editor.session.writer_edit_refused", error = error);
                self.set_notice(message.clone());
                Err(message.into())
            }
        }
    }

    /// Apply one replacement to the editor so history records a single step.
    fn apply_outcome(
        &mut self,
        outcome: &FormatOutcome,
        unchanged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The `Change` this replace delivers is the session's own command, not
        // typing, so the change subscription reports it as programmatic.
        self.writing_self = !unchanged;
        self.editor.update(cx, |editor, cx| {
            if !unchanged {
                editor.set_selected_range(outcome.range.clone(), cx);
                editor.replace(outcome.replacement.clone(), window, cx);
            }
            editor.set_selected_range(outcome.selection.clone(), cx);
        });
    }

    /// Select the next occurrence of the active selection, if there is one.
    ///
    /// Only the active selection is read: the pinned facade cannot enumerate
    /// the cursor set (see `docs/technical/editor-capabilities.md` §6 B1), so
    /// repeating this over every existing cursor is unavailable by design.
    /// Because `add_selection` keeps the original selection active, the search
    /// continues from the end of the last range this command added; the record
    /// is dropped as soon as the active selection changes.
    ///
    /// Returns the range that was added.
    pub(crate) fn select_next_occurrence(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let selection = self.editor.read(cx).selected_range();
        if selection.is_empty() {
            return None;
        }
        let from = match &self.next_occurrence {
            Some((active, end)) if *active == selection => *end,
            _ => selection.end,
        };
        let range = {
            let visible = self.projection.visible.as_str();
            let needle = visible.get(selection.clone())?;
            visible
                .get(from..)?
                .find(needle)
                .map(|offset| from + offset..from + offset + needle.len())
        };
        let range = range?;
        self.editor
            .update(cx, |editor, cx| editor.add_selection(range.clone(), cx));
        let active = self.editor.read(cx).selected_range();
        self.next_occurrence = Some((active, range.end));
        Some(range)
    }

    /// Replace the cursor set with cursors at the line ends of the main selection.
    ///
    /// Upstream defines this from the main selection only, which is exactly
    /// what the public selection accessors expose: the cursor set covers every
    /// line the selection spans, up to and including the line holding its end.
    /// Returns the cursor count.
    pub(crate) fn cursors_at_line_ends(&mut self, cx: &mut Context<Self>) -> usize {
        let selection = self.editor.read(cx).selected_range();
        let mut line_ends = Vec::new();
        {
            let visible = self.projection.visible.as_str();
            let end = selection.end.min(visible.len());
            let Some(head) = visible.get(..selection.start.min(visible.len())) else {
                return 0;
            };
            let mut start = head.rfind('\n').map_or(0, |index| index + 1);
            loop {
                // A trailing newline opens an empty last line; it has no end of
                // its own to place a cursor on.
                if start >= visible.len() {
                    break;
                }
                let line_end = visible[start..]
                    .find('\n')
                    .map_or(visible.len(), |index| start + index);
                let content_end =
                    if line_end > start && visible.as_bytes().get(line_end - 1) == Some(&b'\r') {
                        line_end - 1
                    } else {
                        line_end
                    };
                line_ends.push(content_end);
                // The selection's own last line is the last cursor, exactly as
                // upstream scopes this command to the main selection.
                if line_end >= end || line_end >= visible.len() {
                    break;
                }
                start = line_end + 1;
            }
        }
        let Some(first) = line_ends.first().copied() else {
            return 0;
        };
        let rest: Vec<usize> = line_ends[1..].to_vec();
        let count = line_ends.len();
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(first..first, cx);
            for offset in rest {
                editor.add_selection(offset..offset, cx);
            }
        });
        count
    }

    /// Move the caret to a 1-based line number, reporting whether it exists.
    pub(crate) fn go_to_line(
        &mut self,
        line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if line == 0 {
            return false;
        }
        let lines = self.counts.lines;
        if line > lines {
            return false;
        }
        let target = Position::new((line - 1) as u32, 0);
        self.editor.update(cx, |editor, cx| {
            editor.set_cursor_position(target, window, cx)
        });
        true
    }

    /// The external URL under `position`, if the modified click hit one.
    ///
    /// The hit test inverts `range_to_bounds` over the document's own URL
    /// ranges: the facade exposes no pointer-to-caret mapping (§6 B3), but a
    /// range's bounds are public, which is the supported mechanism.
    pub(crate) fn open_url_at(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self.source_view
            && let Some(url) = self.writer.read(cx).url_at(position)
        {
            // The rendered surface collapses any modified-click cursor the
            // editor may have added; only the requested link opens.
            self.editor.update(cx, |editor, cx| {
                let active = editor.selected_range();
                editor.set_selected_range(active, cx);
            });
            return external_url(&url);
        }
        let url = {
            let ranges = crate::editor::formatting::link_destinations(&self.projection.visible);
            let editor = self.editor.read(cx);
            let hit = ranges.into_iter().find(|range| {
                Self::range_contains_point(editor, &self.projection.visible, range, position)
            })?;
            external_url(&self.projection.visible[hit])?
        };
        // The modified click also added a cursor. The facade cannot remove a
        // cursor (§6 B1), so the set is collapsed onto the active one: the
        // clicked caret is discarded rather than left as a second caret.
        self.editor.update(cx, |editor, cx| {
            let active = editor.selected_range();
            editor.set_selected_range(active, cx);
        });
        Some(url)
    }

    /// The image destination under `position`, if the pointer is over one.
    ///
    /// Uses the same inverse `range_to_bounds` hit test as modified link
    /// clicks; the facade exposes no pointer-to-caret mapping.
    pub(crate) fn image_destination_at(&self, position: Point<Pixels>, cx: &App) -> Option<String> {
        if !self.source_view
            && let Some(destination) = self.writer.read(cx).image_at(position)
        {
            return Some(destination);
        }
        let links = crate::editor::image_preview::image_links(&self.projection.visible);
        let editor = self.editor.read(cx);
        links
            .into_iter()
            .find(|(range, _)| {
                Self::range_contains_point(editor, &self.projection.visible, range, position)
            })
            .map(|(_, destination)| destination)
    }

    /// Whether `position` falls on any laid-out character of `range`.
    ///
    /// The pinned editor computes one rectangle from the range's start and end
    /// positions. That rectangle is valid only while the range stays on one soft
    /// line; a wrapped link yields an inverted, unusable rectangle. Hit testing
    /// each character inside the range keeps modified clicks and image hover
    /// working when a long URL wraps.
    fn range_contains_point(
        editor: &EditorState,
        visible: &str,
        range: &Range<usize>,
        position: Point<Pixels>,
    ) -> bool {
        let Some(bounds) = editor.range_to_bounds(range) else {
            return false;
        };
        if bounds.contains(&position) {
            return true;
        }
        let Some(text) = visible.get(range.clone()) else {
            return false;
        };
        text.char_indices().any(|(offset, character)| {
            let start = range.start + offset;
            let end = start + character.len_utf8();
            editor
                .range_to_bounds(&(start..end))
                .is_some_and(|bounds| bounds.contains(&position))
        })
    }

    /// Project-relative source this session edits.
    pub(crate) fn canonical_source(&self) -> &str {
        self.buffer.identity().canonical_source().as_str()
    }

    /// Insert one prepared component tag after applying its import plan.
    ///
    /// The plan is revalidated against the live complete source, the tag is
    /// mapped through the projection, and only then is anything written. The
    /// import patch edits complete source without touching undo history, and the
    /// tag insertion is one editor transaction, so undo removes the component
    /// and intentionally leaves the harmless import in place.
    pub(crate) fn insert_component(
        &mut self,
        prepared: PreparedInsertion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.absorb(cx);
        prepared
            .plan
            .revalidate(self.buffer.current_text())
            .map_err(|error| tr!("editor.session.import_replan", error = error))?;
        let selection = self.editor.read(cx).selected_range();
        let caret = prepared
            .tag
            .placeholders
            .first()
            .map_or(selection.start + prepared.tag.text.len(), |placeholder| {
                selection.start + placeholder.range.start
            });
        // The tag must be placeable in the visible text before the import patch
        // touches anything, so a refused insertion leaves the source untouched.
        apply_visible_edit(
            self.buffer.current_text(),
            &self.projection,
            TextEdit::replace(selection.clone(), prepared.tag.text.clone()),
        )
        .map_err(|error| tr!("editor.session.tag_refused", error = error))?;
        if prepared.changes_imports(self.buffer.current_text()) {
            // Import edits live in the hidden prelude; a plan that would change
            // visible text is refused rather than shifting the selection.
            let visible = project_or_complete(prepared.plan.source(), self.kind)
                .0
                .visible;
            if visible != self.projection.visible {
                return Err(tr!("editor.session.import_body_changed"));
            }
            self.buffer
                .apply_text(prepared.plan.source().to_owned())
                .map_err(|error| error.to_string())?;
            self.rebuild_projection(window, cx);
        }
        let outcome = FormatOutcome {
            replacement: prepared.tag.text.clone(),
            range: selection.clone(),
            selection: caret..caret,
        };
        self.apply_outcome(&outcome, false, window, cx);
        self.absorb(cx);
        let ranges = prepared
            .tag
            .placeholders
            .iter()
            .map(|placeholder| {
                selection.start + placeholder.range.start..selection.start + placeholder.range.end
            })
            .collect();
        self.install_placeholders(ranges, cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
    }

    /// The current placeholder ranges, for navigation and rendering.
    pub(crate) fn placeholder_ranges(&self) -> &[Range<usize>] {
        &self.placeholders
    }

    /// Handle Tab, Shift+Tab and Escape while placeholders are active.
    ///
    /// Returns whether the key was consumed. Escape always ends navigation; an
    /// empty set passes the key through to its normal editor meaning.
    fn handle_placeholder_key(&mut self, step: i32, cx: &mut Context<Self>) -> bool {
        if self.placeholders.is_empty() {
            return false;
        }
        if step == 0 {
            self.clear_placeholders(cx);
            return true;
        }
        self.sync_placeholders(cx);
        if self.placeholders.is_empty() {
            return false;
        }
        let count = self.placeholders.len();
        self.placeholder_index = if step > 0 {
            (self.placeholder_index + 1) % count
        } else {
            (self.placeholder_index + count - 1) % count
        };
        let range = self.placeholders[self.placeholder_index].clone();
        self.editor
            .update(cx, |editor, cx| editor.set_selected_range(range, cx));
        cx.notify();
        true
    }

    /// Read back the tracked ranges after an edit.
    ///
    /// Decorations are not undo history: when the inserted tag is undone the
    /// tracked ranges disappear, and a count mismatch ends navigation instead of
    /// jumping to a guessed offset. Typing at a placeholder leaves its anchor
    /// in place, so the caret offset advances by the inserted length; otherwise
    /// Shift+Tab would return before the text just typed.
    fn sync_placeholders(&mut self, cx: &mut Context<Self>) {
        let Some(collection) = self.placeholder_collection.clone() else {
            return;
        };
        let anchors = collection.get_ranges(cx);
        if anchors.len() == self.placeholders.len()
            && anchors.len() == self.placeholder_offsets.len()
        {
            let (text_len, caret) = {
                let editor = self.editor.read(cx);
                (editor.text().len(), editor.selected_range().start)
            };
            let grew = text_len.saturating_sub(self.placeholder_text_len);
            let mut positions = Vec::with_capacity(anchors.len());
            for (anchor, offset) in anchors.iter().zip(&mut self.placeholder_offsets) {
                let position = anchor.start + *offset;
                // The anchor did not move and the caret sits exactly after the
                // inserted text: this placeholder was typed into.
                if grew > 0 && position + grew == caret {
                    *offset += grew;
                }
                positions.push(anchor.start + *offset);
            }
            self.placeholder_text_len = text_len;
            self.placeholders = positions
                .into_iter()
                .map(|position| position..position)
                .collect();
            if self.placeholder_index >= self.placeholders.len() {
                self.placeholder_index = 0;
            }
        } else {
            collection.dispose(cx);
            self.placeholder_collection = None;
            self.placeholders.clear();
            self.placeholder_offsets.clear();
            self.placeholder_index = 0;
            self.placeholder_text_len = 0;
        }
    }

    /// Decorate the placeholder ranges and put the caret in the first one.
    fn install_placeholders(&mut self, ranges: Vec<Range<usize>>, cx: &mut Context<Self>) {
        self.clear_placeholders(cx);
        let Some(first) = ranges.first().cloned() else {
            return;
        };
        let text = self.editor.read(cx).text();
        let text_len = text.len();
        let color = cx.theme().primary;
        // Decorations reject empty ranges, so anchor each zero-width placeholder
        // to the character next to it and remember the caret offset.
        let mut decorations = Vec::with_capacity(ranges.len());
        let mut offsets = Vec::with_capacity(ranges.len());
        for range in &ranges {
            let position = range.start;
            let (anchor, offset) = if position > 0 {
                let mut start = position - 1;
                while start > 0 && !text.is_char_boundary(start) {
                    start -= 1;
                }
                (start..position, position - start)
            } else {
                let mut end = 1;
                while end < text.len() && !text.is_char_boundary(end) {
                    end += 1;
                }
                (0..end.min(text.len()), 0)
            };
            if anchor.is_empty() {
                continue;
            }
            offsets.push(offset);
            decorations.push(
                RangeDecoration::new(anchor)
                    .with_style(RangeDecorationStyle::Fill)
                    .with_color(color),
            );
        }
        if decorations.is_empty() {
            return;
        }
        let collection = self.editor.update(cx, |editor, cx| {
            editor.create_range_decorations_collection(decorations, cx)
        });
        self.placeholder_collection = Some(collection);
        self.placeholder_offsets = offsets;
        self.placeholders = ranges;
        self.placeholder_index = 0;
        self.placeholder_text_len = text_len;
        self.editor
            .update(cx, |editor, cx| editor.set_selected_range(first, cx));
    }

    fn clear_placeholders(&mut self, cx: &mut Context<Self>) {
        if let Some(collection) = self.placeholder_collection.take() {
            collection.dispose(cx);
        }
        self.placeholders.clear();
        self.placeholder_offsets.clear();
        self.placeholder_index = 0;
        self.placeholder_text_len = 0;
    }

    /// Insert literal text at the selection as one undo step.
    ///
    /// Drops use this for the complete image/link Markdown, so a multi-file
    /// drop stays one undo transaction.
    pub(crate) fn insert_snippet(
        &mut self,
        snippet: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.absorb(cx);
        let selection = self.editor.read(cx).selected_range();
        let caret = selection.start + snippet.len();
        let outcome = FormatOutcome {
            replacement: snippet,
            range: selection,
            selection: caret..caret,
        };
        if self.mapped_source(&outcome).is_err() {
            cx.emit(SessionChanged);
            cx.notify();
            return false;
        }
        self.apply_outcome(&outcome, false, window, cx);
        self.absorb(cx);
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        true
    }

    /// Install the document typography used by the rendered writing surface.
    pub(crate) fn set_prose_style(&mut self, style: ProseStyle, cx: &mut Context<Self>) {
        self.writer
            .update(cx, |writer, cx| writer.set_style(style, cx));
    }

    pub(crate) fn set_writer_image_root(&self, root: std::path::PathBuf, cx: &mut Context<Self>) {
        let document = std::path::Path::new(self.canonical_source());
        self.writer.update(cx, |writer, cx| {
            writer.set_image_location(root, document, cx)
        });
    }

    pub(super) fn retain_writer_preedit(&mut self, before: Rope, selection: Range<usize>) {
        self.writer_preedit.get_or_insert((before, selection));
    }

    pub(super) fn writer_preedit_active(&self) -> bool {
        self.writer_preedit.is_some()
    }

    /// Restore the original text and selection without recording a source edit.
    pub(super) fn cancel_writer_preedit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((before, selection)) = self.writer_preedit.take() else {
            return false;
        };
        self.writer_editor.update(cx, |editor, cx| {
            if let Some(change) = TextChange::between(editor.text(), &before) {
                let current = editor.text();
                let range = current.offset_to_offset_utf16(change.old.start)
                    ..current.offset_to_offset_utf16(change.old.end);
                let replacement = before.slice(change.new).to_string();
                editor.replace_text_in_range(Some(range), &replacement, window, cx);
            }
            editor.unmark_text(window, cx);
            editor.set_selected_range(selection.clone(), cx);
        });
        self.last_selection = self.projection_range_to_source(selection);
        true
    }

    /// Stop or resume the caret blink for this document's visible surface.
    pub(crate) fn set_writer_visible(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !visible {
            self.cancel_writer_preedit(window, cx);
        }
        self.writer
            .update(cx, |writer, cx| writer.set_visible(visible, cx));
    }

    /// Scroll the active surface (source editor or writing page) for the
    /// native perf driver. Negative values scroll down/right, matching GPUI's
    /// scroll convention.
    pub(crate) fn perf_scroll(&mut self, x: f32, y: f32, cx: &mut Context<Self>) {
        if self.source_view {
            if let Some(surface) = self.source_surface.clone() {
                surface.update(cx, |surface, cx| {
                    surface.set_scroll_offset(x, y, cx);
                });
                return;
            }
            self.editor.update(cx, |editor, cx| {
                editor.set_scroll_offset(point(px(-x), px(-y)), cx)
            });
        } else {
            self.writer
                .update(cx, |writer, cx| writer.set_scroll_offset(y, cx));
        }
    }

    /// Place the active editor's caret at the end of its text, for the native
    /// harness's end-of-document input steps.
    pub(crate) fn perf_cursor_to_end(&mut self, cx: &mut Context<Self>) {
        let end = self.editor.read(cx).text().len();
        self.editor
            .update(cx, |editor, cx| editor.set_selected_range(end..end, cx));
    }

    /// Select the same visible range in the editor model from the rendered
    /// writing surface and give that model the keyboard.
    pub(crate) fn set_writer_selection(
        &mut self,
        anchor: usize,
        focus: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let limit = self.projection.visible.len();
        let anchor = anchor.min(limit);
        let focus = focus.min(limit);
        let range = anchor.min(focus)..anchor.max(focus);
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(range.clone(), cx);
            editor.focus(window, cx);
        });
        self.last_selection = self.projection_range_to_source(range);
        cx.notify();
    }

    /// The rendered selection rectangle and viewport, when the writer surface
    /// has laid them out.
    pub(crate) fn writer_selection_geometry(
        &self,
        cx: &App,
    ) -> Option<(
        gpui_kit::Bounds<gpui_kit::Pixels>,
        gpui_kit::Bounds<gpui_kit::Pixels>,
    )> {
        self.writer
            .read(cx)
            .selection_geometry(&self.editor.read(cx).selected_range())
    }

    /// Install the writing modes for this session.
    pub(crate) fn set_writing_modes(
        &mut self,
        modes: WritingModes,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.writing_modes.typewriter != modes.typewriter {
            self.writer
                .update(cx, |writer, _| writer.request_caret_follow());
        }
        self.writing_modes = modes;
        self.refresh_writer(cx);
        cx.emit(SessionChanged);
        cx.notify();
    }

    /// Install Copyedit preferences for this session.
    ///
    /// A colour-only change repaints the retained analysis; a newly enabled
    /// category without one starts a debounced background analysis.
    pub(crate) fn set_copyedit_config(&mut self, config: CopyeditConfig, cx: &mut Context<Self>) {
        let cursor = self
            .editor
            .read(cx)
            .cursor()
            .min(self.projection.visible.len());
        if !self.copyedit.set_config(&self.editor, config, cursor, cx) {
            return;
        }
        self.copyedit_task = None;
        if self.copyedit.config().any_enabled() && !self.copyedit.has_analysis() {
            self.schedule_copyedit(cx);
        }
        cx.notify();
    }

    /// Clear layers immediately on an edit and classify after the debounce.
    fn refresh_copyedit(&mut self, cx: &mut Context<Self>) {
        self.copyedit_task = None;
        self.copyedit.invalidate(cx);
        if self.copyedit.config().any_enabled() {
            self.schedule_copyedit(cx);
        }
    }

    /// Debounce, classify off the render path and apply only the newest result.
    fn schedule_copyedit(&mut self, cx: &mut Context<Self>) {
        let ticket = self.copyedit.issue();
        self.copyedit_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            // Read the snapshot only after the debounce and only if this
            // generation is still current, so typing never clones the document.
            let snapshot = this.update(cx, |this, _| {
                this.copyedit
                    .is_current(ticket)
                    .then(|| this.projection.visible.clone())
            });
            let Ok(Some(visible)) = snapshot else {
                return;
            };
            let analysis =
                runtime::blocking(cx, move || crate::editor::copyedit::classify(visible)).await;
            let Ok(analysis) = analysis else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                let cursor = this
                    .editor
                    .read(cx)
                    .cursor()
                    .min(this.projection.visible.len());
                if this
                    .copyedit
                    .apply(&this.editor, ticket, analysis, cursor, cx)
                {
                    this.writer.update(cx, |_, cx| cx.notify());
                }
            });
        }));
    }

    /// Rebuild prose decorations, the focus layer, counts and the typewriter
    /// offset after a text change.
    fn refresh_writer(&mut self, cx: &mut Context<Self>) {
        if self.presentation_dirty {
            self.counts = token_summary(&self.projection.visible);
            self.presentation_dirty = false;
        }
        if self.source_view {
            self.copyedit.invalidate(cx);
            return;
        }
        // Typography, focus dimming and typewriter scrolling belong to the
        // visible rich surface. The input model needs no second prose layer.
        let text = self.writer_editor.read(cx).text().clone();
        self.writer.update(cx, |writer, cx| {
            writer.observe_text(text, cx);
            cx.notify();
        });
    }

    /// Refresh only what follows the caret.
    fn refresh_cursor(&mut self, cx: &mut Context<Self>) {
        if self.source_view {
            self.copyedit.invalidate(cx);
            return;
        }
        let cursor = self
            .editor
            .read(cx)
            .cursor()
            .min(self.projection.visible.len());
        self.copyedit.refresh_cursor(&self.editor, cursor, cx);
        let text = self.writer_editor.read(cx).text().clone();
        self.writer.update(cx, |writer, cx| {
            writer.observe_text(text, cx);
            cx.notify();
        });
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// Whether the last delivered `Change` came from one of this session's own
    /// commands instead of the user's keyboard.
    ///
    /// The chrome's typed-character policy reads this so formatting and link
    /// pastes never count as typing.
    pub(crate) fn programmatic_change(&self) -> bool {
        self.programmatic_change
    }

    /// Materialize the live editor text into the complete source.
    ///
    /// Called at every save, recovery snapshot and frontmatter patch boundary;
    /// the editor event path already absorbed the last delivered `Change`.
    /// Undo one complete-source transaction, body or hidden frontmatter.
    pub(crate) fn undo_source(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.cancel_writer_preedit(window, cx) {
            return true;
        }
        self.absorb(cx);
        let Some(entry) = self.history.undo_entry() else {
            return false;
        };
        let current = self.buffer.text_snapshot();
        if !entry.matches_after(&current) {
            // A source rewrite outside the transaction stack invalidates it.
            self.history.clear();
            return false;
        }
        let restored = entry.undo(&current);
        if self.buffer.apply_snapshot(restored).is_err() {
            return false;
        }
        self.after_history_step(entry.selection_before.clone(), window, cx);
        true
    }

    /// Redo one complete-source transaction.
    pub(crate) fn redo_source(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        self.absorb(cx);
        let Some(entry) = self.history.redo_entry() else {
            return false;
        };
        let current = self.buffer.text_snapshot();
        if !entry.matches_before(&current) {
            self.history.clear();
            return false;
        }
        let restored = entry.redo(&current);
        if self.buffer.apply_snapshot(restored).is_err() {
            return false;
        }
        self.after_history_step(entry.selection_after.clone(), window, cx);
        true
    }

    fn after_history_step(
        &mut self,
        selection: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_projection(window, cx);
        let view_selection = if self.source_view {
            clamp_range(selection.clone(), self.buffer.len())
        } else {
            self.source_range_to_projection(&selection)
        };
        let visible_len = self.editor.read(cx).text().len();
        let start = view_selection.start.min(visible_len);
        let end = view_selection.end.min(visible_len);
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(start..end, cx);
        });
        self.last_selection = selection;
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        self.sync_placeholders(cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(HistoryChanged);
        cx.emit(SessionChanged);
        cx.notify();
    }

    pub fn sync_buffer(&mut self, cx: &App) {
        self.absorb(cx);
        self.dirty = self.buffer.is_dirty();
    }

    /// Record that `revision` was persisted.
    ///
    /// The disk now holds our own bytes, so the external-change fingerprint
    /// follows them and the document is no longer reported as changed on disk.
    pub fn confirm_saved(&mut self, revision: DocumentRevision, persisted: &str) -> bool {
        self.disk_fingerprint = fingerprint(persisted);
        self.externally_changed = false;
        let confirmed = self.buffer.confirm_saved(revision, persisted);
        self.dirty = self.buffer.is_dirty();
        confirmed
    }

    /// Move the baseline to the bytes an earlier save persisted, keeping newer
    /// edits dirty so their follow-up save does not conflict with that write.
    ///
    /// Newer edits are not an external change: the disk holds our own write.
    pub fn rebase_persisted(&mut self, saved_revision: DocumentRevision, persisted: &str) -> bool {
        self.disk_fingerprint = fingerprint(persisted);
        self.externally_changed = false;
        let rebased = self.buffer.rebase_persisted(saved_revision, persisted);
        self.dirty = self.buffer.is_dirty();
        rebased
    }

    /// Discard this session's in-memory edits and adopt `source` from disk.
    ///
    /// The caller must have obtained `source` from an explicit user reload
    /// action: the current buffer text is lost, the document becomes clean and
    /// the external-change flag resets to the newly read bytes.
    ///
    /// # Errors
    ///
    /// Returns the buffer error as text when the revision cannot advance.
    pub fn force_reload(
        &mut self,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.buffer
            .force_reload(source.clone())
            .map_err(|error| error.to_string())?;
        self.disk_fingerprint = fingerprint(&source);
        self.externally_changed = false;
        self.dirty = false;
        self.rebuild_projection(window, cx);
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        self.sync_placeholders(cx);
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
    }

    /// Re-target this session at a renamed source and re-project it.
    ///
    /// A rename can change the projection kind (`.mdx` ↔ `.md`), so the retained
    /// projection must not outlive the path it was built from.
    pub fn retarget(
        &mut self,
        identity: DocumentIdentity,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.kind = kind_for(identity.canonical_source().as_str());
        self.buffer.retarget(identity);
        self.rebuild_projection(window, cx);
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
    }

    /// Apply one complete-source patch and rebuild the projection around it.
    ///
    /// The live editor text is absorbed first, so a concurrent unsaved body edit
    /// is rebased onto the patch rather than being overwritten by a stale form
    /// snapshot. The patch runs against the authoritative complete source, and
    /// the mapped caret is restored after the visible text is rebuilt.
    pub fn patch_complete(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        patch: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<(), String> {
        self.absorb(cx);
        let before = self.buffer.current_text().to_owned();
        let selection_before = self.current_source_selection(cx);
        let patched = patch(&before)?;
        self.buffer
            .apply_text(patched)
            .map_err(|error| error.to_string())?;
        self.rebuild_projection(window, cx);
        let after = self.buffer.current_text().to_owned();
        let selection_after = self.current_source_selection(cx);
        self.history.record(
            &before,
            &after,
            selection_before,
            selection_after.clone(),
            HistoryIntent::Atomic,
        );
        self.last_selection = selection_after;
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
    }

    /// Flush a pending visible edit as an atomic transaction boundary.
    fn absorb(&mut self, cx: &App) {
        self.absorb_with_intent(cx, HistoryIntent::Atomic);
    }

    /// Map a retained input snapshot change into the authoritative source.
    fn absorb_with_intent(&mut self, cx: &App, intent: HistoryIntent) {
        if !self.source_view && self.writer_preedit.is_some() {
            return;
        }
        let text_key = (
            self.editor.entity_id(),
            self.buffer.revision(),
            self.editor.read(cx).text().clone(),
        );
        if self.absorbed_text.as_ref() == Some(&text_key) {
            return;
        }
        self.absorbed_text = Some(text_key);
        if self.source_view {
            self.absorb_source(cx, intent);
            return;
        }
        if self.refused {
            // The editor text is known to be newer-and-rejected; nothing may be
            // mapped from it until a rebuild resynchronises it.
            return;
        }
        let visible = self.editor.read(cx).text().clone();
        // Projection replacement (reloads, recovery and synthetic layouts)
        // can precede delivery of its own input event. Such an event is a sync,
        // not an edit to the authoritative source.
        if visible == self.projection.visible.as_str() {
            self.absorbed_writer_text = visible;
            return;
        }
        let previous = if self.absorbed_writer_text == self.projection.visible.as_str() {
            self.absorbed_writer_text.clone()
        } else {
            Rope::from_str(&self.projection.visible)
        };
        let Some(change) = TextChange::between(&previous, &visible) else {
            return;
        };
        let replacement = visible.slice(change.new.clone()).to_string();
        let selection_before = self.last_selection.clone();
        let selection_after =
            self.projection_range_to_source(self.editor.read(cx).selected_range());
        let before = self.buffer.text_snapshot();
        let mut projected = None;
        let mut updated_counts = None;
        if let Ok(source_range) = self.projection.map_visible_range(&change.old) {
            let removed = before.slice(source_range.clone()).to_string();
            if let Err(error) = self.buffer.splice_text(source_range.clone(), &replacement) {
                self.refuse(&error.to_string());
                return;
            }
            self.history.record_splice(
                source_range,
                removed,
                replacement.clone(),
                selection_before.clone(),
                selection_after.clone(),
                intent,
            );
            // Keep the projection allocation; only move the tail of this local
            // edit. Parser/string interoperability must not copy the whole body
            // merely to renew two source-mapping segments.
            if !self.presentation_dirty {
                updated_counts = Some(self.counts.edited(
                    &self.projection.visible,
                    change.old.clone(),
                    &replacement,
                ));
            }
            let mut text = std::mem::take(&mut self.projection.visible);
            text.replace_range(change.old.clone(), &replacement);
            projected = Some(text);
        }
        let text = match projected {
            Some(text) => text,
            None => {
                // Empty or synthetic projections still use the validated full
                // mapping boundary; ordinary typing never enters this path.
                let edit = TextEdit::replace(change.old, replacement);
                match apply_visible_edit(&before.to_string(), &self.projection, edit) {
                    Ok(source) => {
                        if let Err(error) = self.buffer.apply_text(source) {
                            self.refuse(&error.to_string());
                            return;
                        }
                    }
                    Err(error) => {
                        self.refuse(&error.to_string());
                        return;
                    }
                }
                self.history.record(
                    &before.to_string(),
                    self.buffer.current_text(),
                    selection_before,
                    selection_after.clone(),
                    intent,
                );
                visible.to_string()
            }
        };
        match self.projection.renew_visible(text) {
            Ok(projection) => {
                self.projection = projection;
                self.absorbed_writer_text = visible;
                if let Some(counts) = updated_counts {
                    self.counts = counts;
                    self.presentation_dirty = false;
                } else {
                    self.presentation_dirty = true;
                }
            }
            Err(error) => self.refuse(&error.to_string()),
        }
        self.last_selection = selection_after;
    }

    fn absorb_source(&mut self, cx: &App, intent: HistoryIntent) {
        if self.refused {
            return;
        }
        let source = self.editor.read(cx).text().to_string();
        if source == self.buffer.current_text() {
            return;
        }
        let before = self.buffer.current_text().to_owned();
        let selection_before = self.last_selection.clone();
        let selection_after = self.editor.read(cx).selected_range();
        if let Err(error) = self.buffer.apply_text(source) {
            self.refuse(&error.to_string());
            return;
        }
        let (projection, notice) = project_or_complete(self.buffer.current_text(), self.kind);
        self.projection = projection;
        if self.notice.is_none() {
            self.notice = notice;
            self.lsp_notice_pending = false;
        }
        self.counts = token_summary(&self.projection.visible);
        self.presentation_dirty = false;
        self.history.record(
            &before,
            self.buffer.current_text(),
            selection_before,
            selection_after.clone(),
            intent,
        );
        self.last_selection = selection_after;
    }

    /// Show the complete source (Build) or the writer projection (Writing/Preview).
    pub(crate) fn set_source_view(
        &mut self,
        source: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.source_view == source {
            return;
        }
        self.cancel_writer_preedit(window, cx);
        self.absorb(cx);
        let source_selection = self.current_source_selection(cx);
        let revision = self.buffer.revision();
        if self.source_view {
            self.source_revision = Some(revision);
        } else {
            self.writer_revision = revision;
        }
        self.source_view = source;
        if source {
            self.writer
                .update(cx, |writer, cx| writer.refresh_caret((false, 0, 0..0), cx));
        }
        let selection = if source {
            source_selection.clone()
        } else {
            self.source_range_to_projection(&source_selection)
        };
        self.editor = if source {
            match self.source_editor.clone() {
                Some(editor) => editor,
                None => {
                    let text = self.buffer.current_text().to_owned();
                    let language = self.language.clone();
                    let editor = cx.new(|cx| {
                        EditorState::new(window, cx)
                            .language(language)
                            .line_number(true)
                            .indent_guides(true)
                            .folding(true)
                            .searchable(true)
                            .auto_close(true)
                            .smart_indent(true)
                            .soft_wrap(false)
                            .default_value(text)
                    });
                    self.watch_source_editor(&editor, window, cx);
                    self.source_editor = Some(editor.clone());
                    self.source_revision = Some(revision);
                    editor
                }
            }
        } else {
            self.writer_editor.clone()
        };
        if source && SourceSurface::enabled() && self.source_surface.is_none() {
            let editor = self.editor.clone();
            let language = self.language.clone();
            let weak = cx.weak_entity();
            self.source_surface =
                Some(cx.new(|cx| SourceSurface::new(weak, editor, language, window, cx)));
        }
        let stale = if source {
            self.source_revision != Some(revision)
        } else {
            self.writer_revision != revision
        };
        if stale {
            let target = if source {
                self.buffer.current_text()
            } else {
                &self.projection.visible
            };
            // Synchronize only the changed range. Cached syntax, scroll and
            // unchanged text survive mode switches and edits in the other view.
            let old = self.editor.read(cx).text().to_string();
            if let Some((range, _, replacement)) = source_diff(&old, target) {
                self.origin = true;
                self.editor.update(cx, |editor, cx| {
                    editor.set_selected_range(range, cx);
                    editor.replace(replacement, window, cx);
                });
            }
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(selection.clone(), cx)
        });
        if source {
            self.source_revision = Some(revision);
        } else {
            self.writer_revision = revision;
        }
        if !source {
            self.absorbed_writer_text = self.editor.read(cx).text().clone();
        }
        self.last_selection = source_selection;
        self.refresh_copyedit(cx);
        self.refresh_writer(cx);
        // The workspace owns the project's language servers: it attaches this
        // document when Build shows it and detaches it on the way out.
        // The mode owner notifies the shell; emitting SessionChanged here would
        // run project subscriptions reentrantly from inside a project update.
        cx.notify();
    }

    /// Attach this document to the project's shared server for its language.
    ///
    /// The workspace owns the process and its event loop; a session keeps only
    /// the document URI its providers and diagnostics are scoped to.
    pub(crate) fn attach_lsp(
        &mut self,
        handle: ClientHandle,
        uri: lsp_types::Uri,
        language: LanguageId,
        support: ProviderSupport,
        ready: bool,
        cx: &mut Context<Self>,
    ) {
        let same_document = self.lsp.as_ref().is_some_and(|attachment| {
            attachment.uri.as_str() == uri.as_str() && attachment.language == language
        });
        if !same_document {
            let weak = cx.weak_entity();
            let own_uri = uri.clone();
            self.editor.update(cx, |editor, _| {
                editor.lsp_mut().show_document =
                    Some(std::rc::Rc::new(move |params, _window, cx| {
                        if params.uri.as_str() == own_uri.as_str() {
                            return false;
                        }
                        let Ok(url) = url::Url::parse(params.uri.as_str()) else {
                            return false;
                        };
                        let Ok(path) = url.to_file_path() else {
                            return false;
                        };
                        let selection = params.selection.map(|range| range.start);
                        weak.update(cx, |_, cx| {
                            cx.emit(DefinitionTarget { path, selection });
                        })
                        .is_ok()
                    }));
            });
            self.lsp_diagnostic_version = None;
            self.lsp_failure_seen = false;
        }
        // A replacement runtime always supplies a fresh handle, even when the
        // document URI itself is unchanged.
        self.lsp = Some(LspAttachment {
            handle,
            uri,
            language,
        });
        self.lsp_status = if ready {
            LspStatus::Ready
        } else {
            LspStatus::Starting
        };
        self.install_lsp_providers(support, cx);
        cx.notify();
    }

    /// Record that the shared server completed its handshake.
    pub(crate) fn lsp_ready(&mut self, support: ProviderSupport, cx: &mut Context<Self>) {
        if self.lsp.is_none() {
            return;
        }
        self.lsp_status = LspStatus::Ready;
        if self.take_lsp_notice() {
            // Replace the failure the status line copied with recovery.
            self.set_lsp_notice(tr!("editor.session.lsp.ready"));
            cx.emit(SessionChanged);
        }
        self.install_lsp_providers(support, cx);
        cx.notify();
    }

    /// Surface work the shared server is doing for this document.
    pub(crate) fn note_lsp_status(&mut self, status: LspStatus, cx: &mut Context<Self>) {
        if self.lsp.is_none() || self.lsp_status == status {
            return;
        }
        self.lsp_status = status;
        cx.notify();
    }

    /// Report one server failure for this document, once per attachment.
    ///
    /// Returns whether the failure was new; the exit that follows a handshake
    /// error is the same cause and never overwrites it.
    pub(crate) fn report_lsp_failure(
        &mut self,
        status: LspStatus,
        retry: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.lsp_failure_seen {
            return false;
        }
        let message = match &status {
            LspStatus::Failed(error) if retry => {
                tr!("editor.session.lsp.failed_retry", error = error)
            }
            LspStatus::Failed(error) => tr!("editor.session.lsp.failed_no_retry", error = error),
            LspStatus::Exited(Some(code)) if retry => {
                tr!("editor.session.lsp.exited_retry", code = code)
            }
            LspStatus::Exited(Some(code)) => {
                tr!("editor.session.lsp.exited_no_retry", code = code)
            }
            LspStatus::Exited(None) if retry => {
                tr!("editor.session.lsp.exited_unexpectedly_retry")
            }
            LspStatus::Exited(None) => tr!("editor.session.lsp.exited_unexpectedly_no_retry"),
            _ => return false,
        };
        self.lsp_failure_seen = true;
        self.lsp_status = status;
        self.set_lsp_notice(message);
        cx.emit(SessionChanged);
        cx.notify();
        true
    }

    /// Take an LSP notice, leaving notices that belong to other features.
    fn take_lsp_notice(&mut self) -> bool {
        if !self.lsp_notice_pending || self.notice.is_none() {
            return false;
        }
        self.notice = None;
        self.lsp_notice_pending = false;
        true
    }

    /// Install only the providers the server advertised in its handshake.
    fn install_lsp_providers(&mut self, support: ProviderSupport, cx: &mut Context<Self>) {
        let Some(attachment) = self.lsp.as_ref() else {
            return;
        };
        crate::editor::lsp::install(
            &self.editor,
            attachment.handle.clone(),
            attachment.uri.clone(),
            support,
            cx,
        );
        cx.notify();
    }

    /// Drop this document's attachment without ending the project's server.
    ///
    /// The workspace sends `didClose` for the returned URI; the process stays
    /// up, so a tab switch never pays another discovery or `node_modules` scan.
    pub(crate) fn detach_lsp(&mut self, cx: &mut Context<Self>) -> Option<LspAttachment> {
        self.lsp_change = None;
        self.lsp_diagnostic_version = None;
        self.lsp_failure_seen = false;
        self.lsp_status = LspStatus::Inactive;
        let attachment = self.lsp.take()?;
        crate::editor::lsp::uninstall(&self.editor, cx);
        cx.notify();
        Some(attachment)
    }

    /// This document open on the shared server, while Build shows it.
    pub(crate) fn lsp_attachment(&self) -> Option<&LspAttachment> {
        self.lsp.as_ref()
    }

    /// What the shared server is doing for this document.
    ///
    /// The registry reads it to avoid restating a state the session already
    /// shows; the status line renders the same value.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn lsp_status(&self) -> &LspStatus {
        &self.lsp_status
    }

    /// Apply one publish the workspace routed here by its document URI.
    pub(crate) fn apply_lsp_diagnostics(
        &mut self,
        uri: lsp_types::Uri,
        version: Option<i32>,
        diagnostics: Vec<lsp_types::Diagnostic>,
        cx: &mut Context<Self>,
    ) {
        let Some(attachment) = self.lsp.as_ref() else {
            return;
        };
        if !crate::editor::lsp::accepts_diagnostics(
            &attachment.uri,
            self.lsp_diagnostic_version,
            &uri,
            version,
        ) {
            tracing::debug!(
                document = %uri.as_str(),
                ?version,
                "dropping diagnostics for another document or an older version"
            );
            return;
        }
        self.lsp_diagnostic_version = version;
        crate::editor::lsp::apply_diagnostics(&self.editor, &diagnostics, cx);
    }

    /// Focus this document at a language-server selection.
    pub(crate) fn reveal_source_position(
        &mut self,
        selection: Option<lsp_types::Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(selection) = selection else {
            return;
        };
        if !self.source_view {
            self.set_source_view(true, window, cx);
        }
        // Provider responses are converted to the editor's character-based
        // coordinates at the protocol boundary, so this is already a character
        // position, not a UTF-16 one.
        let text = self.buffer.current_text();
        let offset = astrolith_lsp::char_position_to_offset(text, selection).min(text.len());
        self.editor.update(cx, |editor, cx| {
            editor.set_selected_range(offset..offset, cx);
        });
        self.last_selection = offset..offset;
        cx.notify();
    }

    /// Push the latest complete source to the server, coalescing keystrokes.
    fn sync_lsp_change(&mut self, cx: &mut Context<Self>) {
        let Some(attachment) = self.lsp.as_ref() else {
            return;
        };
        let handle = attachment.handle.clone();
        let uri = attachment.uri.clone();
        let text = self.buffer.current_text().to_owned();
        let executor = cx.background_executor().clone();
        self.lsp_change = Some(cx.background_spawn(async move {
            executor.timer(std::time::Duration::from_millis(150)).await;
            handle.change(uri, text);
        }));
    }

    /// Current editor selection expressed in complete-source bytes.
    fn current_source_selection(&self, cx: &App) -> Range<usize> {
        let selection = self.editor.read(cx).selected_range();
        if self.source_view {
            clamp_range(selection, self.buffer.len())
        } else {
            self.projection_range_to_source(selection)
        }
    }

    fn projection_range_to_source(&self, range: Range<usize>) -> Range<usize> {
        self.projection_offset_to_source(range.start)..self.projection_offset_to_source(range.end)
    }

    fn projection_offset_to_source(&self, offset: usize) -> usize {
        let mut last_source = 0;
        for segment in &self.projection.segments {
            match segment {
                ProjectionSegment::Hidden { source } => last_source = source.end,
                ProjectionSegment::Visible { source, visible } => {
                    if offset < visible.end {
                        return source.start + offset.saturating_sub(visible.start);
                    }
                    last_source = source.end;
                }
            }
        }
        last_source
    }

    fn source_range_to_projection(&self, range: &Range<usize>) -> Range<usize> {
        self.source_offset_to_projection(range.start)..self.source_offset_to_projection(range.end)
    }

    fn source_offset_to_projection(&self, offset: usize) -> usize {
        for segment in &self.projection.segments {
            match segment {
                ProjectionSegment::Hidden { source } => {
                    if offset < source.end {
                        return self
                            .projection
                            .segments
                            .iter()
                            .find_map(|segment| match segment {
                                ProjectionSegment::Visible { visible, .. } => Some(visible.start),
                                ProjectionSegment::Hidden { .. } => None,
                            })
                            .unwrap_or(0);
                    }
                }
                ProjectionSegment::Visible { source, visible } => {
                    if offset < source.end {
                        return visible.start + offset.saturating_sub(source.start);
                    }
                }
            }
        }
        self.projection.visible.len()
    }

    fn refuse(&mut self, reason: &str) {
        // Fail closed: the complete source is authoritative and untouched. Rebuild
        // the projection from it so the next diff is still measured against the
        // real source rather than the rejected text, and latch the refusal so no
        // later absorb maps the divergent editor text over the kept bytes.
        self.refused = true;
        self.projection = project_or_complete(self.buffer.current_text(), self.kind).0;
        self.presentation_dirty = true;
        self.set_notice(tr!("editor.session.edit_refused_reloaded", reason = reason));
    }

    /// Re-project the complete source, keeping the active caret on its text.
    ///
    /// A rebuild moves hidden bytes around the visible text, so the caret is
    /// mapped through the visible diff rather than through source offsets.
    fn rebuild_projection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_writer_preedit(window, cx);
        let previous_visible = self.projection.visible.clone();
        let caret = self.editor.read(cx).cursor().min(previous_visible.len());
        let source_caret = if self.source_view {
            caret.min(self.buffer.len())
        } else {
            self.projection_offset_to_source(caret)
        };
        let (projection, notice) = project_or_complete(self.buffer.current_text(), self.kind);
        self.projection = projection;
        self.presentation_dirty = true;
        if let Some(notice) = notice {
            self.set_notice(notice);
        }
        if self.source_view {
            let source = self.buffer.current_text().to_owned();
            let matches = {
                let current = self.editor.read(cx).text();
                current.len() == source.len() && current.chars().eq(source.chars())
            };
            if !matches {
                self.origin = false;
                self.writing_self = false;
                self.editor.update(cx, |editor, cx| {
                    editor.set_value(source.clone(), window, cx);
                });
            }
            let caret = source_caret.min(source.len());
            self.editor
                .update(cx, |editor, cx| editor.set_selected_range(caret..caret, cx));
            self.refused = false;
            self.sync_lsp_change(cx);
            return;
        }
        let current = self.editor.read(cx).text();
        if current.len() != self.projection.visible.len()
            || !current.chars().eq(self.projection.visible.chars())
        {
            let caret = map_visible_offset(&previous_visible, &self.projection.visible, caret);
            let visible = self.projection.visible.clone();
            // History is owned by the complete-source transactions below; the
            // editor's own stack must not record this programmatic rewrite.
            self.origin = false;
            self.writing_self = false;
            self.editor.update(cx, |editor, cx| {
                editor.set_value(visible, window, cx);
                editor.set_selected_range(caret..caret, cx);
            });
        }
        self.absorbed_writer_text = self.editor.read(cx).text().clone();
        // The editor text now matches the projection again, so edits are safe to
        // absorb.
        self.refused = false;
    }
}

/// Whether a transaction may merge with adjacent typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryIntent {
    Typing,
    Atomic,
}

/// One complete-source edit: replacing `range` with `removed` changes `before`
/// into `after`.
#[derive(Clone, Debug)]
struct SourceTransaction {
    range: Range<usize>,
    removed: String,
    inserted: String,
    selection_before: Range<usize>,
    selection_after: Range<usize>,
    intent: HistoryIntent,
    at: Instant,
}

impl SourceTransaction {
    fn undo(&self, current: &Rope) -> Rope {
        let mut text = current.clone();
        text.remove(self.range.start..self.range.start + self.inserted.len());
        text.insert(self.range.start, &self.removed);
        text
    }
    fn redo(&self, current: &Rope) -> Rope {
        let mut text = current.clone();
        text.remove(self.range.start..self.range.start + self.removed.len());
        text.insert(self.range.start, &self.inserted);
        text
    }
    fn matches_after(&self, current: &Rope) -> bool {
        current
            .try_slice(self.range.start..self.range.start + self.inserted.len())
            .is_ok_and(|slice| slice == self.inserted.as_str())
    }
    fn matches_before(&self, current: &Rope) -> bool {
        current
            .try_slice(self.range.start..self.range.start + self.removed.len())
            .is_ok_and(|slice| slice == self.removed.as_str())
    }
}

/// Ordered complete-source history shared by body and frontmatter edits.
#[derive(Default)]
struct SourceHistory {
    entries: Vec<SourceTransaction>,
    cursor: usize,
}

impl SourceHistory {
    fn clear(&mut self) {
        self.entries.clear();
        self.cursor = 0;
    }

    fn record(
        &mut self,
        before: &str,
        after: &str,
        selection_before: Range<usize>,
        selection_after: Range<usize>,
        intent: HistoryIntent,
    ) {
        if let Some((range, removed, inserted)) = source_diff(before, after) {
            self.record_splice(
                range,
                removed,
                inserted,
                selection_before,
                selection_after,
                intent,
            );
        }
    }

    fn record_splice(
        &mut self,
        range: Range<usize>,
        removed: String,
        inserted: String,
        selection_before: Range<usize>,
        selection_after: Range<usize>,
        intent: HistoryIntent,
    ) {
        self.entries.truncate(self.cursor);
        let now = Instant::now();
        if let Some(last) = self.entries.last_mut()
            && intent == HistoryIntent::Typing
            && last.intent == HistoryIntent::Typing
            && now.duration_since(last.at).as_millis() <= 900
            && last.removed.is_empty()
            && removed.is_empty()
            && range.start == last.range.start + last.inserted.len()
        {
            last.inserted.push_str(&inserted);
            last.selection_after = selection_after;
            last.at = now;
        } else {
            self.entries.push(SourceTransaction {
                range,
                removed,
                inserted,
                selection_before,
                selection_after,
                intent,
                at: now,
            });
            if self.entries.len() > 500 {
                self.entries.remove(0);
            }
        }
        self.cursor = self.entries.len();
    }

    fn undo_entry(&mut self) -> Option<SourceTransaction> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        self.entries.get(self.cursor).cloned()
    }

    fn redo_entry(&mut self) -> Option<SourceTransaction> {
        let entry = self.entries.get(self.cursor).cloned()?;
        self.cursor += 1;
        Some(entry)
    }
}

/// Return the single contiguous replacement that turns `before` into `after`.
fn source_diff(before: &str, after: &str) -> Option<(Range<usize>, String, String)> {
    let before_bytes = before.as_bytes();
    let after_bytes = after.as_bytes();
    let mut prefix = before_bytes
        .iter()
        .zip(after_bytes)
        .take_while(|(left, right)| left == right)
        .count();
    while prefix > 0 && !before.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let mut suffix = 0;
    while suffix < before_bytes.len() - prefix
        && suffix < after_bytes.len() - prefix
        && before_bytes[before_bytes.len() - 1 - suffix]
            == after_bytes[after_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix))
    {
        suffix -= 1;
    }
    let before_end = before.len() - suffix;
    let after_end = after.len() - suffix;
    if prefix == before_end && prefix == after_end {
        return None;
    }
    Some((
        prefix..before_end,
        before[prefix..before_end].to_owned(),
        after[prefix..after_end].to_owned(),
    ))
}

/// Whether an editor rope holds exactly `text`.
///
/// The editor stores a rope, so an unchanged document can be compared without
/// materialising it into a `String`; only a real edit is copied out.
fn clamp_range(range: Range<usize>, length: usize) -> Range<usize> {
    let start = range.start.min(length);
    let end = range.end.min(length);
    start..end.max(start)
}

/// Which projection applies to a canonical source path.
fn kind_for(path: &str) -> DocumentKind {
    std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(DocumentKind::from_extension)
        .unwrap_or(DocumentKind::Plain)
}

/// Syntax language for the complete-source view, by file extension.
///
/// Unknown extensions use `text`, which the pinned highlighter installs
/// without a grammar, so unsupported files stay readable instead of failing.
fn editor_language(path: &str) -> SharedString {
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "md" | "mdx" => "markdown",
        "astro" => "astro",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" | "jsx" => "tsx",
        "js" | "mjs" | "cjs" => "javascript",
        "json" | "jsonc" => "json",
        "css" | "scss" | "less" => "css",
        "html" | "htm" => "html",
        "svelte" => "svelte",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        _ => "text",
    }
    .into()
}

/// Project `source`, showing everything (and saying why) when the metadata
/// cannot be delimited.
fn project_or_complete(
    source: &str,
    kind: DocumentKind,
) -> (WriterProjection, Option<SharedString>) {
    match project_writer(source, kind) {
        Ok(projection) => (projection, None),
        // A frontmatter opener without a closing delimiter cannot be delimited,
        // so the writer shows everything instead of guessing.
        Err(error) => (
            WriterProjection::complete(source),
            Some(tr!("editor.session.metadata_visible", error = error).into()),
        ),
    }
}

/// A process-local content signature: external-change status and the
/// presentation layers compare it instead of re-diffing the text.
pub(crate) fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolith_core::{ProjectId, SourcePath};

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

    fn mounted_session(
        cx: &mut gpui_kit::TestAppContext,
        source_path: &str,
        source: &str,
    ) -> (gpui_kit::AnyWindowHandle, Entity<DocumentSession>) {
        let mut session = None;
        let (window, _host) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                let identity = DocumentIdentity::new(
                    ProjectId::new("projection").expect("a project id"),
                    1,
                    SourcePath::new(source_path).expect("a source path"),
                );
                session = Some(
                    cx.new(|cx| DocumentSession::new(identity, source.to_owned(), window, cx)),
                );
                cx.new(|_| Host)
            })
            .expect("a test window")
        });
        (
            window,
            session.expect("the session is built with the window"),
        )
    }

    #[gpui_kit::test]
    fn writing_splice_reuses_visible_storage_and_keeps_hidden_source_undoable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let body = "中文🦀 ordinary words.\r\n\r\n".repeat(2_000);
        let source =
            format!("---\r\ntitle: Exact\r\n---\r\nimport Card from './Card.astro';\r\n\r\n{body}");
        let (handle, session) = mounted_session(cx, "entry.mdx", &source);
        let (pointer, snapshot, at) = session.update(cx, |session, _| {
            session.projection.visible.reserve(1_024);
            let at = session.projection.visible.find("ordinary").unwrap();
            (
                session.projection.visible.as_ptr() as usize,
                session.buffer.text_snapshot(),
                at,
            )
        });
        cx.update_window(handle, |_, window, cx| {
            let editor = session.read(cx).editor.clone();
            editor.update(cx, |editor, cx| {
                editor.set_selected_range(at..at + "ordinary".len(), cx);
                editor.replace("日本語", window, cx);
            });
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(
                session.projection.visible.as_ptr() as usize,
                pointer,
                "a local edit must not replace the whole visible allocation"
            );
            assert_eq!(
                session.buffer.current_text(),
                source.replacen("ordinary", "日本語", 1)
            );
            assert_eq!(snapshot, source);
            assert_eq!(session.counts, token_summary(&session.projection.visible));
        });
        cx.update_window(handle, |_, window, cx| {
            session.update(cx, |session, cx| assert!(session.undo_source(window, cx)));
        })
        .unwrap();
        session.read_with(cx, |session, _| {
            assert_eq!(session.buffer.current_text(), source)
        });
    }

    /// A handle for a shared server the test never talks to.
    ///
    /// Providers only borrow the handle; the protocol loop is not needed to
    /// prove that a session keeps, reports and drops its attachment.
    fn shared_handle() -> (ClientHandle, lsp_types::Uri) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        let client = runtime.block_on(async {
            let (client_stream, peer) = tokio::io::duplex(1024);
            let client = astrolith_lsp::Client::connect(
                client_stream,
                "file:///project".parse().expect("a root URI"),
            )
            .await
            .expect("an in-memory client");
            drop(peer);
            client
        });
        let handle = client.handle();
        drop(client);
        (
            handle,
            "file:///project/src/app.ts"
                .parse()
                .expect("a document URI"),
        )
    }

    #[gpui_kit::test]
    fn a_failure_is_visible_and_reported_once(cx: &mut gpui_kit::TestAppContext) {
        let (window, session) = mounted_session(cx, "src/app.ts", "const x = 1;\n");
        let (handle, uri) = shared_handle();
        cx.update_window(window, |_, _, cx| {
            session.update(cx, |session, cx| {
                session.attach_lsp(
                    handle,
                    uri,
                    LanguageId::TypeScript,
                    ProviderSupport::default(),
                    false,
                    cx,
                );

                assert!(session.report_lsp_failure(LspStatus::Failed("boom".into()), true, cx));
                assert_eq!(session.lsp_status, LspStatus::Failed("boom".into()));
                assert_eq!(
                    session.notice.as_deref(),
                    Some("Language server failed: boom. Retrying once.")
                );

                assert!(
                    !session.report_lsp_failure(LspStatus::Exited(Some(3)), true, cx),
                    "the exit that follows the failure is not a second cause"
                );
                assert_eq!(
                    &session.lsp_status,
                    &LspStatus::Failed("boom".into()),
                    "the first failure keeps the diagnosis and the retry"
                );
                assert_eq!(
                    session.notice.as_deref(),
                    Some("Language server failed: boom. Retrying once.")
                );
            });
        })
        .expect("the window is open");
    }

    #[gpui_kit::test]
    fn a_ready_server_becomes_visible_and_clears_the_failure_notice(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (window, session) = mounted_session(cx, "src/app.ts", "const x = 1;\n");
        let (handle, uri) = shared_handle();
        cx.update_window(window, |_, _, cx| {
            session.update(cx, |session, cx| {
                session.notice = Some(SharedString::from(
                    "Language server failed: boom. Retrying once.",
                ));
                session.lsp_notice_pending = true;
                session.attach_lsp(
                    handle,
                    uri,
                    LanguageId::TypeScript,
                    ProviderSupport::default(),
                    false,
                    cx,
                );
                assert_eq!(session.lsp_status, LspStatus::Starting);

                session.lsp_ready(ProviderSupport::default(), cx);
                assert_eq!(session.lsp_status, LspStatus::Ready);
                assert_eq!(
                    session.notice.as_deref(),
                    Some("Language server ready."),
                    "the retry notice must be replaced by the recovery"
                );
            });
        })
        .expect("the window is open");
    }

    #[gpui_kit::test]
    fn detaching_clears_the_status_and_keeps_no_attachment(cx: &mut gpui_kit::TestAppContext) {
        let (window, session) = mounted_session(cx, "src/app.ts", "const x = 1;\n");
        let (handle, uri) = shared_handle();
        cx.update_window(window, |_, _, cx| {
            session.update(cx, |session, cx| {
                session.attach_lsp(
                    handle,
                    uri.clone(),
                    LanguageId::TypeScript,
                    ProviderSupport::default(),
                    false,
                    cx,
                );
                assert!(session.report_lsp_failure(LspStatus::Failed("boom".into()), false, cx));

                let attachment = session.detach_lsp(cx).expect("the document was attached");
                assert_eq!(attachment.uri.as_str(), uri.as_str());
                assert_eq!(attachment.language, LanguageId::TypeScript);
                assert_eq!(session.lsp_status, LspStatus::Inactive);
                assert!(session.lsp_attachment().is_none());
                assert!(
                    session.detach_lsp(cx).is_none(),
                    "detaching twice is a no-op"
                );
                assert!(session.notice.is_some(), "the notice outlives the detach");
                assert!(
                    session.report_lsp_failure(LspStatus::Failed("again".into()), false, cx),
                    "detaching clears the failure latch for the next attachment"
                );
            });
        })
        .expect("the window is open");
    }
}
