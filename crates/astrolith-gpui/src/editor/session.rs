//! One retained editor/history for each canonical source, independent of selection.
use astrolith_document::{DocumentBuffer, DocumentIdentity};
use gpui_kit::component::input::{EditorState, InputEvent};
use gpui_kit::{App, AppContext, Context, Entity, EventEmitter, Subscription, Window};

pub(crate) struct DocumentSession {
    pub editor: Entity<EditorState>,
    pub buffer: DocumentBuffer,
    pub externally_changed: bool,
    pub dirty: bool,
    pub disk_fingerprint: u64,
    /// Editor changes are materialized into the retained text buffer lazily:
    /// typing stays O(1), while save/recovery read the latest Rope once.
    pending: bool,
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
        let buffer = DocumentBuffer::from_unsaved(identity, original, current.clone());
        let dirty = buffer.is_dirty();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .default_value(current)
        });
        let change = cx.subscribe(&editor, |this, _editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                // Materializing a whole document per keystroke makes typing
                // linear in file size. Record the dirty state (length first)
                // and sync the buffer only when a save or recovery needs it.
                this.pending = true;
                this.dirty = this.is_dirty(cx);
                cx.emit(SessionChanged);
                cx.notify();
            }
        });
        Self {
            editor,
            buffer,
            externally_changed: false,
            dirty,
            disk_fingerprint,
            pending: false,
            _change: change,
        }
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        if !self.pending {
            return self.buffer.is_dirty();
        }
        // The event queue may not have delivered the last Change at close time.
        // Compare the live Rope against the baseline without allocating; an
        // equal length is the only case that can still be clean.
        let text = self.editor.read(cx).text();
        if text.len() != self.buffer.original_text().len() {
            return true;
        }
        !text.chars().eq(self.buffer.original_text().chars())
    }

    /// Materialize the live editor text into the retained buffer.
    ///
    /// Called by save and recovery only; the editor event path never copies a
    /// whole document.
    pub fn sync_buffer(&mut self, cx: &App) {
        if !self.pending {
            return;
        }
        let text = self.editor.read(cx).text().to_string();
        if let Err(error) = self.buffer.apply_text(text) {
            eprintln!("{error}");
        }
        self.dirty = self.buffer.is_dirty();
        self.pending = false;
    }
}

/// A process-local content signature used only to display external-change status.
pub(crate) fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
