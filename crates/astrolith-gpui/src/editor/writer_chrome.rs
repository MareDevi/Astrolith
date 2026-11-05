//! Writer chrome auto-hide policy and typed-character accounting.

use std::path::PathBuf;

/// Typed-character accounting for the chrome auto-hide policy.
///
/// The baseline is keyed by the active document and measured in characters, so
/// switching documents, growing the text with a formatting command or a link
/// paste, and inserting a multi-byte character cannot satisfy the policy.
#[derive(Clone, Debug, Default)]
pub(crate) struct TypedCharacters {
    last: Option<(PathBuf, usize)>,
}

impl TypedCharacters {
    /// Characters the user typed into `path` since the previous observation.
    ///
    /// `programmatic` marks text this session wrote itself: the baseline moves
    /// without counting anything, so formatting and pasting stay invisible.
    pub(crate) fn observe(&mut self, path: PathBuf, visible: &str, programmatic: bool) -> usize {
        let characters = visible.chars().count();
        let typed = match (&self.last, programmatic) {
            (Some((last_path, last_characters)), false)
                if last_path == &path && characters > *last_characters =>
            {
                characters - last_characters
            }
            _ => 0,
        };
        self.last = Some((path, characters));
        typed
    }

    /// Drop the baseline, so the next observation starts a fresh count.
    ///
    /// Called when no document is active: a document reopened after that may
    /// have been reloaded from disk, so its previous length means nothing.
    pub(crate) fn forget(&mut self) {
        self.last = None;
    }
}

/// Chrome auto-hide after continuous typing with both sidebars closed.
///
/// The writer hides its title bar once four characters have been typed while
/// neither sidebar is open, and a hover at the window's top edge restores it.
/// The state is explicit so the policy is testable without a window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChromeVisibility {
    typed: u32,
    hidden: bool,
}

/// Typed characters that hide the chrome.
pub(crate) const AUTO_HIDE_AFTER: u32 = 4;

impl ChromeVisibility {
    /// Record `characters` typed since the last change.
    ///
    /// Returns whether the chrome should be hidden now. Nothing hides while a
    /// sidebar is open, and non-typing edits (zero or negative growth) never
    /// accumulate.
    pub(crate) fn type_characters(
        &mut self,
        characters: usize,
        both_sidebars_closed: bool,
    ) -> bool {
        if !both_sidebars_closed {
            self.typed = 0;
            self.hidden = false;
            return false;
        }
        self.typed = self
            .typed
            .saturating_add(characters.min(u32::MAX as usize) as u32);
        if self.typed >= AUTO_HIDE_AFTER {
            self.hidden = true;
        }
        self.hidden
    }

    /// Restore the chrome; returns whether it was hidden.
    pub(crate) fn restore(&mut self) -> bool {
        self.typed = 0;
        std::mem::replace(&mut self.hidden, false)
    }

    /// Track sidebar visibility; opening either sidebar always restores.
    pub(crate) fn set_sidebars(&mut self, both_sidebars_closed: bool) {
        if !both_sidebars_closed {
            self.typed = 0;
            self.hidden = false;
        }
    }

    /// Whether the title bar is currently hidden.
    pub(crate) fn is_hidden(&self) -> bool {
        self.hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_hides_only_after_four_typed_characters_with_sidebars_closed() {
        let mut chrome = ChromeVisibility::default();
        assert!(!chrome.type_characters(3, true));
        assert!(!chrome.is_hidden());
        assert!(chrome.type_characters(1, true));
        assert!(chrome.is_hidden());

        let mut chrome = ChromeVisibility::default();
        for _ in 0..8 {
            assert!(!chrome.type_characters(1, false));
        }
        assert!(!chrome.is_hidden());

        // Opening a sidebar while hidden restores the chrome.
        let mut chrome = ChromeVisibility::default();
        assert!(chrome.type_characters(AUTO_HIDE_AFTER as usize, true));
        chrome.set_sidebars(false);
        assert!(!chrome.is_hidden());
        // The counter restarts, so one more character cannot hide it again.
        assert!(!chrome.type_characters(1, false));
    }

    #[test]
    fn hovering_the_top_edge_restores_the_chrome_once() {
        let mut chrome = ChromeVisibility::default();
        chrome.type_characters(AUTO_HIDE_AFTER as usize, true);
        assert!(chrome.restore());
        assert!(!chrome.is_hidden());
        assert!(!chrome.restore());
        assert!(!chrome.type_characters(3, true));
    }

    #[test]
    fn non_typing_edits_do_not_hide_the_chrome() {
        let mut chrome = ChromeVisibility::default();
        for _ in 0..8 {
            assert!(!chrome.type_characters(0, true));
        }
        assert!(!chrome.is_hidden());
    }

    #[test]
    fn typed_characters_skip_commands_and_document_switches() {
        let mut typed = TypedCharacters::default();
        assert_eq!(typed.observe("a.md".into(), "one", false), 0);
        assert_eq!(typed.observe("a.md".into(), "one two", false), 4);
        // A formatting command grows the text without typing: the baseline
        // moves and the policy stays where it was.
        let mut chrome = ChromeVisibility::default();
        assert!(!chrome.type_characters(typed.observe("a.md".into(), "**one** two", true), true));
        assert!(
            !chrome.type_characters(typed.observe("a.md".into(), "**one** two  ", true), true),
            "programmatic growth never hides the chrome"
        );
        // The next real keystroke is one character, not the whole rewrite.
        assert!(
            !chrome.type_characters(typed.observe("a.md".into(), "**one** two  x", false), true)
        );

        // Closing the document drops the baseline, so reopening a reloaded file
        // does not count its whole new length as typing.
        let mut typed = TypedCharacters::default();
        assert_eq!(typed.observe("a.md".into(), "short", false), 0);
        typed.forget();
        assert_eq!(typed.observe("a.md".into(), &"long ".repeat(20), false), 0);

        // Switching documents starts from a fresh baseline.
        let mut typed = TypedCharacters::default();
        assert_eq!(typed.observe("a.md".into(), "short", false), 0);
        assert_eq!(
            typed.observe("b.md".into(), &"long document ".repeat(20), false),
            0
        );
        assert_eq!(
            typed.observe(
                "b.md".into(),
                &format!("{}x", "long document ".repeat(20)),
                false
            ),
            1
        );
    }

    #[test]
    fn typed_characters_count_cjk_as_one_character_each() {
        let mut typed = TypedCharacters::default();
        let mut chrome = ChromeVisibility::default();
        let mut text = String::new();
        assert_eq!(typed.observe("a.md".into(), &text, false), 0);
        // Three bytes per character: byte counting would hide on the second
        // keystroke and is not what "characters" means.
        let mut hidden = false;
        for character in ['日', '本', '語', '文'] {
            text.push(character);
            hidden = chrome.type_characters(typed.observe("a.md".into(), &text, false), true);
            assert!(!hidden || text.chars().count() == 4);
        }
        assert!(hidden);
    }
}
