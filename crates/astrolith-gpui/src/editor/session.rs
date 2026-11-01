//! One retained editor/history for each canonical source, independent of selection.
use std::ops::Range;

use astrolith_document::{
    DocumentBuffer, DocumentIdentity, DocumentKind, DocumentRevision, FormatCommand, FormatOutcome,
    TextEdit, TokenSnapshot, WriterProjection, apply_visible_edit, diff_visible, format_document,
    map_visible_offset, project_writer, token_snapshot,
};
use gpui_kit::component::input::{EditorState, InputEvent, Position};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, Pixels, Point, SharedString, Subscription,
    Window,
};

use crate::editor::formatting::external_url;
use crate::editor::writing_modes::{SessionPresentation, WritingModes};

pub(crate) struct DocumentSession {
    /// The editor displays the writer projection, never the complete source.
    pub editor: Entity<EditorState>,
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
    /// Prose, focus and typewriter presentation for this session.
    pub presentation: SessionPresentation,
    /// The word and character counts of the current visible text.
    pub counts: TokenSnapshot,
    /// [`fingerprint`] of [`Self::projection`]'s visible text, computed once per
    /// text change so presentation refreshes never re-hash the document.
    text_key: u64,
    _change: Subscription,
    _cursor: Subscription,
}
pub(crate) struct SessionChanged;
impl EventEmitter<SessionChanged> for DocumentSession {}
impl DocumentSession {
    pub fn new(
        identity: DocumentIdentity,
        source: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(identity, source.clone(), source, window, cx)
    }

    /// Build a session whose baseline is the on-disk `original` but whose visible
    /// text is recovered unsaved `current` content (so it loads dirty).
    pub fn from_recovered(
        identity: DocumentIdentity,
        original: String,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(identity, original, current, window, cx)
    }

    fn build(
        identity: DocumentIdentity,
        original: String,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let disk_fingerprint = fingerprint(&original);
        let kind = kind_for(identity.canonical_source().as_str());
        let (projection, notice) = project_or_complete(&current, kind);
        let buffer = DocumentBuffer::from_unsaved(identity, original, current);
        let dirty = buffer.is_dirty();
        let counts = token_snapshot(&projection.visible);
        let text_key = fingerprint(&projection.visible);
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .default_value(projection.visible.clone())
        });
        let change = cx.subscribe_in(
            &editor,
            window,
            |this, _editor, event: &InputEvent, window, cx| {
                if !matches!(event, InputEvent::Change) {
                    return;
                }
                if this.origin {
                    // This session's own projection rewrite; the buffer is already
                    // authoritative for exactly this text.
                    this.origin = false;
                    this.writing_self = false;
                    this.programmatic_change = true;
                    return;
                }
                // The editor reports that *some* text changed, so the whole visible
                // text is diffed at this boundary and mapped back before anything
                // reads the buffer.
                this.programmatic_change = std::mem::take(&mut this.writing_self);
                this.absorb(cx);
                if this.refused {
                    // Fail closed: the refused text never reached the source, so put
                    // the editor back on the authoritative projection (and clear the
                    // latch) instead of letting the next Change overwrite it.
                    this.rebuild_projection(window, cx);
                }
                this.refresh_writer(cx);
                this.dirty = this.buffer.is_dirty();
                cx.emit(SessionChanged);
                cx.notify();
            },
        );
        // Cursor moves notify the editor without changing text; focus dimming and
        // typewriter centring follow the caret, so they refresh here and nowhere
        // near the render path.
        let cursor = cx.observe_in(&editor, window, |this, _editor, window, cx| {
            let _ = window;
            this.refresh_cursor(cx);
        });
        Self {
            editor,
            buffer,
            projection,
            origin: false,
            writing_self: false,
            programmatic_change: false,
            next_occurrence: None,
            refused: false,
            notice,
            externally_changed: false,
            dirty,
            disk_fingerprint,
            kind,
            presentation: SessionPresentation::new(WritingModes::default()),
            counts,
            text_key,
            _change: change,
            _cursor: cursor,
        }
    }

    /// The source syntax this session was built for.
    pub(crate) fn kind(&self) -> DocumentKind {
        self.kind
    }

    /// Number of lines in the visible text, at least one.
    pub(crate) fn line_count(&self, _: &App) -> usize {
        self.counts.lines
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
                let message =
                    format!("The formatting command was refused and the source was kept: {error}");
                self.notice = Some(SharedString::from(message.clone()));
                cx.emit(SessionChanged);
                cx.notify();
                return Err(SharedString::from(message));
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
                let message =
                    format!("The writer edit was refused and the source was kept: {error}");
                self.notice = Some(SharedString::from(message.clone()));
                Err(SharedString::from(message))
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
        let url = {
            let ranges = crate::editor::formatting::link_destinations(&self.projection.visible);
            let editor = self.editor.read(cx);
            let hit = ranges.into_iter().find(|range| {
                editor
                    .range_to_bounds(range)
                    .is_some_and(|bounds| bounds.contains(&position))
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

    /// Install the writing modes for this session.
    pub(crate) fn set_writing_modes(
        &mut self,
        modes: WritingModes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.presentation.set_modes(&self.editor, modes, window, cx);
        self.presentation.invalidate();
        self.refresh_writer(cx);
        cx.emit(SessionChanged);
        cx.notify();
    }

    /// Rebuild prose decorations, the focus layer, counts and the typewriter
    /// offset after a text change.
    fn refresh_writer(&mut self, cx: &mut Context<Self>) {
        self.counts = token_snapshot(&self.projection.visible);
        self.text_key = fingerprint(&self.projection.visible);
        self.presentation
            .refresh_prose(&self.editor, &self.projection.visible, self.text_key, cx);
        self.presentation
            .refresh_focus(&self.editor, &self.projection.visible, cx);
        self.presentation.centre_cursor(&self.editor, cx);
    }

    /// Refresh only what follows the caret.
    fn refresh_cursor(&mut self, cx: &mut Context<Self>) {
        // The prose key includes the text and the pane width, so a caret move
        // after either changed rebuilds the layer instead of leaving it stale.
        self.presentation
            .refresh_prose(&self.editor, &self.projection.visible, self.text_key, cx);
        self.presentation
            .refresh_focus(&self.editor, &self.projection.visible, cx);
        self.presentation.centre_cursor(&self.editor, cx);
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
    pub fn sync_buffer(&mut self, cx: &App) {
        self.absorb(cx);
        self.dirty = self.buffer.is_dirty();
    }

    /// Record that `revision` was persisted.
    pub fn confirm_saved(&mut self, revision: DocumentRevision, persisted: &str) -> bool {
        self.buffer.confirm_saved(revision, persisted)
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
        let patched = patch(self.buffer.current_text())?;
        self.buffer
            .apply_text(patched)
            .map_err(|error| error.to_string())?;
        self.rebuild_projection(window, cx);
        self.refresh_writer(cx);
        self.dirty = self.buffer.is_dirty();
        cx.emit(SessionChanged);
        cx.notify();
        Ok(())
    }

    /// Map the whole visible text back into the complete source and absorb it.
    fn absorb(&mut self, cx: &App) {
        if self.refused {
            // The editor text is known to be newer-and-rejected; nothing may be
            // mapped from it until a rebuild resynchronises it.
            return;
        }
        let visible = self.editor.read(cx).text().to_string();
        if visible == self.projection.visible {
            return;
        }
        let edit = diff_visible(&self.projection.visible, &visible);
        match apply_visible_edit(self.buffer.current_text(), &self.projection, edit) {
            Ok(source) => {
                if let Err(error) = self.buffer.apply_text(source) {
                    self.refuse(&error.to_string());
                    return;
                }
                match self.projection.renew_visible(visible) {
                    Ok(projection) => self.projection = projection,
                    Err(error) => self.refuse(&error.to_string()),
                }
            }
            // Fail closed: the complete source is untouched and the rejected
            // text is never guessed into it.
            Err(error) => self.refuse(&error.to_string()),
        }
    }

    fn refuse(&mut self, reason: &str) {
        // Fail closed: the complete source is authoritative and untouched. Rebuild
        // the projection from it so the next diff is still measured against the
        // real source rather than the rejected text, and latch the refusal so no
        // later absorb maps the divergent editor text over the kept bytes.
        self.refused = true;
        self.projection = project_or_complete(self.buffer.current_text(), self.kind).0;
        self.notice = Some(SharedString::from(format!(
            "A writer edit was refused and the source was kept: {reason}. The editor was reloaded from the document."
        )));
    }

    /// Re-project the complete source, keeping the active caret on its text.
    ///
    /// A rebuild moves hidden bytes around the visible text, so the caret is
    /// mapped through the visible diff rather than through source offsets.
    fn rebuild_projection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let previous_visible = self.projection.visible.clone();
        let caret = self.editor.read(cx).cursor().min(previous_visible.len());
        let (projection, notice) = project_or_complete(self.buffer.current_text(), self.kind);
        self.projection = projection;
        if let Some(notice) = notice {
            self.notice = Some(notice);
        }
        let current = self.editor.read(cx).text();
        if current.len() != self.projection.visible.len()
            || !current.chars().eq(self.projection.visible.chars())
        {
            let caret = map_visible_offset(&previous_visible, &self.projection.visible, caret);
            let visible = self.projection.visible.clone();
            self.origin = true;
            self.editor.update(cx, |editor, cx| {
                editor.replace_all(visible, window, cx);
                editor.set_selected_range(caret..caret, cx);
            });
        }
        // The editor text now matches the projection again, so edits are safe to
        // absorb.
        self.refused = false;
    }
}

/// Which projection applies to a canonical source path.
fn kind_for(path: &str) -> DocumentKind {
    std::path::Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(DocumentKind::from_extension)
        .unwrap_or(DocumentKind::Plain)
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
            Some(SharedString::from(format!(
                "Writer metadata stays visible: {error}"
            ))),
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
