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
        let change = cx.subscribe(&editor, |this, editor, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                // Changes are captured on editor events, never during render.
                // Keeping the retained editor preserves selection, history and IME state.
                let text = editor.read(cx).text().to_string();
                if let Err(error) = this.buffer.apply_text(text) {
                    eprintln!("{error}");
                }
                this.dirty = this.buffer.is_dirty();
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
            _change: change,
        }
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        // The event queue may not have delivered the last Change at close time.
        // Read the real editor before permitting a destructive transition.
        self.editor.read(cx).text() != self.buffer.original_text()
    }
}

/// A process-local content signature used only to display external-change status.
pub(crate) fn fingerprint(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}
