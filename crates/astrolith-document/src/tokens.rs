//! One token snapshot per document revision.
//!
//! The footer word and character counts and the Copyedit classifier both read
//! the same snapshot, so the two never disagree about which bytes are a word.
//! A token is a maximal run of letters, digits, apostrophes or underscores;
//! everything else — Markdown punctuation, whitespace, symbols — separates
//! tokens. Characters are counted without whitespace, which is what writers
//! mean by "characters".

use std::ops::Range;

/// Word tokens and character count for one text snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TokenSnapshot {
    /// Token ranges in UTF-8 byte offsets, in document order.
    pub tokens: Vec<Range<usize>>,
    /// Characters that are not whitespace.
    pub characters: usize,
    /// Lines in the text, at least one for empty text.
    pub lines: usize,
}

impl TokenSnapshot {
    /// Number of word tokens.
    pub fn words(&self) -> usize {
        self.tokens.len()
    }

    /// The counts alone, without the token ranges, so a render path can read
    /// them without copying the snapshot.
    pub fn summary(&self) -> TokenSummary {
        TokenSummary {
            words: self.words(),
            characters: self.characters,
            lines: self.lines,
        }
    }
}

/// The counts of one [`TokenSnapshot`], cheap to copy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenSummary {
    /// Number of word tokens.
    pub words: usize,
    /// Characters that are not whitespace.
    pub characters: usize,
    /// Lines in the text, at least one for empty text.
    pub lines: usize,
}

/// Tokenize `text` once and count words and characters from that snapshot.
pub fn token_snapshot(text: &str) -> TokenSnapshot {
    let tokens = crate::copyedit::word_ranges(text);
    let mut characters = 0;
    let mut lines = 1;
    for character in text.chars() {
        if !character.is_whitespace() {
            characters += 1;
        }
        if character == '\n' {
            lines += 1;
        }
    }
    TokenSnapshot {
        tokens,
        characters,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_words_and_characters_from_one_snapshot() {
        let text = "Two words, and a [link](https://x.dev).\n";
        let snapshot = token_snapshot(text);
        assert_eq!(snapshot.words(), 8);
        assert_eq!(
            snapshot
                .tokens
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["Two", "words", "and", "a", "link", "https", "x", "dev"]
        );
        let visible: String = text.chars().filter(|c| !c.is_whitespace()).collect();
        assert_eq!(snapshot.characters, visible.chars().count());
        assert_eq!(
            snapshot.lines, 2,
            "the trailing newline opens one more line"
        );
    }

    #[test]
    fn counts_crlf_lines_without_splitting_them() {
        let snapshot = token_snapshot("one\r\ntwo\r\n");
        assert_eq!(snapshot.lines, 3);
        assert_eq!(snapshot.words(), 2);
    }

    #[test]
    fn counts_unicode_words_without_splitting_characters() {
        let text = "Été 中文 déjà-vu";
        let snapshot = token_snapshot(text);
        assert_eq!(snapshot.words(), 4);
        assert_eq!(
            snapshot
                .tokens
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["Été", "中文", "déjà", "vu"]
        );
        for range in &snapshot.tokens {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn empty_text_has_no_tokens() {
        let snapshot = token_snapshot("");
        assert_eq!(snapshot.words(), 0);
        assert_eq!(snapshot.characters, 0);
        assert_eq!(snapshot.lines, 1);
    }
}
