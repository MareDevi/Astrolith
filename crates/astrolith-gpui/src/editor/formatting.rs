//! Writer command glue: link paste, external URL opening and cursor commands.
//!
//! Two behaviours need a hook on the editor element rather than a command:
//! pasting a URL over a selection wraps the selection as a link (one undo
//! step), and a modified click on an inline `http`/`https` URL opens it in the
//! browser. Both are installed by the document pane from [`DocumentHooks`].
//!
//! What the pinned facade does **not** permit, and what this module therefore
//! does not implement (see `docs/technical/editor-capabilities.md` §6):
//!
//! - a Ctrl-click cursor command — `index_for_mouse_position` is `pub(crate)`,
//!   so a pointer position cannot be resolved to a caret offset (B3);
//! - select-next-occurrence across *every* cursor and distinct per-range batch
//!   replacement — the selection set cannot be enumerated (B1) and
//!   `replace_text_in_ranges` is `pub(crate)` (B2).
//!
//! The commands that do ship are limited exactly as the capability contract
//! allows: select-next on the active selection, and line-end cursors derived
//! from the main selection, which is upstream's own definition.

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use astrolith_document::inline_links;
use gpui_kit::{
    App, ClipboardItem, ExternalPaths, MouseButton, MouseDownEvent, WeakEntity, Window,
};

use crate::editor::session::DocumentSession;

/// Everything a live session installs on the editor element it does not own:
/// paste/link hooks and the identity the selection toolbar acts on.
#[derive(Clone)]
pub(crate) struct EditorWiring {
    /// Element-level handlers.
    pub hooks: DocumentHooks,
    /// The session the toolbar commands dispatch to.
    pub session: WeakEntity<DocumentSession>,
    /// Canonical project root used to resolve document-relative previews.
    pub preview_root: Option<Arc<PathBuf>>,
}

impl EditorWiring {
    /// Build the wiring for one live session.
    pub(crate) fn new(session: WeakEntity<DocumentSession>) -> Self {
        Self {
            hooks: document_hooks(session.clone()),
            session,
            preview_root: None,
        }
    }

    /// Install the project root the Alt-hover preview resolves against.
    pub(crate) fn with_preview_root(mut self, root: PathBuf) -> Self {
        self.preview_root = Some(Arc::new(root));
        self
    }

    /// Install the editor file-drop handler.
    ///
    /// The handler owns asset copying and insertion; the element only forwards
    /// the platform's dropped paths.
    pub(crate) fn with_drop(mut self, drop: DropHook) -> Self {
        self.hooks.drop = Some(drop);
        self
    }
}

/// Paste interceptor: returns `true` when the payload was consumed.
type PasteHook = Rc<dyn Fn(&ClipboardItem, &mut Window, &mut App) -> bool>;
/// Bubble-phase mouse-down handler.
type MouseDownHook = Rc<dyn Fn(&MouseDownEvent, &mut Window, &mut App)>;

/// Platform file-drop handler: one or more external paths.
pub(crate) type DropHook = Rc<dyn Fn(&ExternalPaths, &mut Window, &mut App)>;

/// Editor-element handlers owned by one document pane.
#[derive(Clone)]
pub(crate) struct DocumentHooks {
    /// Intercept a paste before the editor's default text insertion.
    pub(crate) paste: PasteHook,
    /// Bubble-phase mouse down, for modified clicks on links.
    pub(crate) mouse_down: MouseDownHook,
    /// Copy dropped files through the asset service.
    pub(crate) drop: Option<DropHook>,
}

/// Build the editor hooks for one session.
pub(crate) fn document_hooks(session: WeakEntity<DocumentSession>) -> DocumentHooks {
    let paste_session = session.clone();
    let paste = Rc::new(
        move |item: &ClipboardItem, window: &mut Window, cx: &mut App| {
            let Some(text) = item.text() else {
                return false;
            };
            let Some(url) = external_url(&text) else {
                return false;
            };
            paste_session
                .update(cx, |session, cx| {
                    session.wrap_selection_as_link(&url, window, cx)
                })
                .unwrap_or(false)
        },
    );
    let mouse_down = Rc::new(
        move |event: &MouseDownEvent, _: &mut Window, cx: &mut App| {
            if !activates_link(event) {
                return;
            }
            let position = event.position;
            let opened = session
                .update(cx, |session, cx| session.open_url_at(position, cx))
                .unwrap_or(None);
            if let Some(url) = opened {
                cx.open_url(&url);
            }
        },
    );
    DocumentHooks {
        paste,
        mouse_down,
        drop: None,
    }
}

/// Whether a mouse-down activates the link under the pointer.
///
/// Alt is the modifier the pinned editor leaves free for link activation (Ctrl
/// belongs to the blocked cursor command), and only the primary button may
/// navigate: a modified right or middle click still belongs to the editor.
fn activates_link(event: &MouseDownEvent) -> bool {
    event.button == MouseButton::Left && event.modifiers.alt
}

/// A URL that may be opened externally: absolute `http` or `https`, with a
/// host and without embedded credentials.
///
/// Anything else — relative links, `javascript:`, `file:`, `data:`, text with
/// whitespace — is refused, so a modified click can never navigate the app.
pub(crate) fn external_url(candidate: &str) -> Option<String> {
    let trimmed = candidate.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    if url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    if !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.as_str().to_owned())
}

/// Ranges of the `http`/`https` URLs a modified click may activate in `text`.
///
/// Inline link destinations are taken from the document crate's own link
/// scan; autolinks (`<https://…>`) and bare URLs are found by a delimiter
/// scan. The ranges are disjoint and in document order, so the pane can invert
/// `range_to_bounds` over them without overlapping hit areas.
pub(crate) fn link_destinations(text: &str) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = inline_links(text)
        .into_iter()
        .filter(|link| external_url(&text[link.destination.clone()]).is_some())
        .map(|link| link.destination)
        .collect();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index + 7 <= bytes.len() {
        let rest = &bytes[index..];
        let prefix = if rest.starts_with(b"https://") {
            Some("https://".len())
        } else if rest.starts_with(b"http://") {
            Some("http://".len())
        } else {
            None
        };
        let Some(prefix) = prefix else {
            index += 1;
            continue;
        };
        let mut end = index + prefix;
        while end < bytes.len() {
            let byte = bytes[end];
            let delimiter = byte.is_ascii_whitespace()
                || matches!(
                    byte,
                    b'<' | b'>' | b'(' | b')' | b'[' | b']' | b'"' | b'\'' | b'`' | b'|'
                );
            if delimiter {
                break;
            }
            end += 1;
        }
        let mut trimmed = end;
        while trimmed > index
            && matches!(
                bytes[trimmed - 1],
                b'.' | b',' | b';' | b':' | b'!' | b'?' | b'*' | b'_'
            )
        {
            trimmed -= 1;
        }
        let range = index..trimmed;
        if trimmed > index
            && external_url(&text[range.clone()]).is_some()
            && !ranges
                .iter()
                .any(|existing| existing.start < range.end && range.start < existing.end)
        {
            ranges.push(range);
        }
        index = end.max(index + 1);
    }
    ranges.sort_by_key(|range| range.start);
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse_down(button: MouseButton, alt: bool) -> MouseDownEvent {
        MouseDownEvent {
            button,
            position: gpui_kit::point(gpui_kit::px(4.), gpui_kit::px(4.)),
            modifiers: gpui_kit::Modifiers {
                alt,
                ..Default::default()
            },
            click_count: 1,
            first_mouse: false,
        }
    }

    #[test]
    fn only_alt_left_click_activates_a_link() {
        assert!(activates_link(&mouse_down(MouseButton::Left, true)));
        assert!(!activates_link(&mouse_down(MouseButton::Left, false)));
        assert!(!activates_link(&mouse_down(MouseButton::Right, true)));
        assert!(!activates_link(&mouse_down(MouseButton::Middle, true)));
    }

    #[test]
    fn only_absolute_http_and_https_urls_are_external() {
        assert_eq!(
            external_url(" https://example.dev/a?b=1\n").as_deref(),
            Some("https://example.dev/a?b=1")
        );
        assert!(external_url("http://example.dev").is_some());
        for refused in [
            "",
            "example.dev",
            "/relative/path",
            "javascript:alert(1)",
            "file:///etc/passwd",
            "data:text/html,hello",
            "https://exa mple.dev",
            "https://user:secret@example.dev",
            "mailto:writer@example.dev",
        ] {
            assert_eq!(external_url(refused), None, "{refused:?} must be refused");
        }
    }

    #[test]
    fn link_destinations_cover_links_autolinks_and_bare_urls() {
        let text = "see [docs](https://a.dev/x) and https://b.dev/y, plus <https://c.dev>.\n";
        let ranges = link_destinations(text);
        let found: Vec<&str> = ranges.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(
            found,
            ["https://a.dev/x", "https://b.dev/y", "https://c.dev"]
        );
        for pair in ranges.windows(2) {
            assert!(pair[0].end <= pair[1].start);
        }
    }

    #[test]
    fn link_destinations_ignore_non_external_targets() {
        let text = "[local](/posts/one) [mail](mailto:a@b.dev) [ok](https://x.dev)";
        let ranges = link_destinations(text);
        assert_eq!(
            ranges
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["https://x.dev"]
        );
    }

    #[test]
    fn link_destinations_are_character_safe() {
        let text = "— https://é.example.dev/ü, ok";
        let ranges = link_destinations(text);
        assert_eq!(ranges.len(), 1);
        assert!(text.is_char_boundary(ranges[0].start) && text.is_char_boundary(ranges[0].end));
        assert_eq!(&text[ranges[0].clone()], "https://é.example.dev/ü");
    }
}
