//! One retained editor/history for each canonical source, independent of selection.
use astrolith_document::{
    DocumentBuffer, DocumentIdentity, DocumentKind, DocumentRevision, WriterProjection,
    apply_visible_edit, diff_visible, map_visible_offset, project_writer,
};
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, SharedString, Subscription, Window,
};

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
    _change: Subscription,
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
                    return;
                }
                // The editor reports that *some* text changed, so the whole visible
                // text is diffed at this boundary and mapped back before anything
                // reads the buffer.
                this.absorb(cx);
                if this.refused {
                    // Fail closed: the refused text never reached the source, so put
                    // the editor back on the authoritative projection (and clear the
                    // latch) instead of letting the next Change overwrite it.
                    this.rebuild_projection(window, cx);
                }
                this.dirty = this.buffer.is_dirty();
                cx.emit(SessionChanged);
                cx.notify();
            },
        );
        Self {
            editor,
            buffer,
            projection,
            origin: false,
            refused: false,
            notice,
            externally_changed: false,
            dirty,
            disk_fingerprint,
            kind,
            _change: change,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
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

/// A process-local content signature used only to display external-change status.
pub(crate) fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
