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

impl TokenSummary {
    /// Update an exact summary after replacing a UTF-8 byte range in `text`.
    ///
    /// Scan only the replaced bytes, replacement and one character on each
    /// side. That context preserves word joins and splits at both edit seams.
    /// `self` must describe `text`; `range` must be valid for `str::replace_range`.
    pub fn edited(self, text: &str, range: Range<usize>, replacement: &str) -> Self {
        let left = text[..range.start]
            .chars()
            .next_back()
            .map_or(range.start, |character| range.start - character.len_utf8());
        let right = text[range.end..]
            .chars()
            .next()
            .map_or(range.end, |character| range.end + character.len_utf8());
        let before = token_summary(&text[left..right]);
        let after = summarize(
            text[left..range.start]
                .chars()
                .chain(replacement.chars())
                .chain(text[range.end..right].chars()),
        );
        Self {
            words: self.words - before.words + after.words,
            characters: self.characters - before.characters + after.characters,
            lines: self.lines - before.lines + after.lines,
        }
    }
}

/// Tokenize `text` once and count words and characters from that snapshot.
pub fn token_snapshot(text: &str) -> TokenSnapshot {
    let tokens = crate::copyedit::word_ranges(text);
    let summary = scan_counts(text);
    TokenSnapshot {
        tokens,
        characters: summary.characters,
        lines: summary.lines,
    }
}

/// Count words, characters and lines in one allocation-free scan.
///
/// This is the footer's per-edit path: it applies the same word rule as
/// [`token_snapshot`] but never materialises the token ranges, so the cost is
/// one pass over the bytes with no allocation.
pub fn token_summary(text: &str) -> TokenSummary {
    summarize(text.chars())
}

fn summarize(characters: impl Iterator<Item = char>) -> TokenSummary {
    let mut words = 0;
    let mut in_word = false;
    let mut counts = ScanCounts::default();
    for character in characters {
        let word = is_word_character(character);
        if word && !in_word {
            words += 1;
        }
        in_word = word;
        counts.push(character);
    }
    TokenSummary {
        words,
        characters: counts.characters,
        lines: counts.lines + 1,
    }
}

/// The same word rule the Copyedit classifier uses, kept in one place here.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '\'' | '_')
}

#[derive(Default)]
struct ScanCounts {
    characters: usize,
    lines: usize,
}

impl ScanCounts {
    fn push(&mut self, character: char) {
        if !character.is_whitespace() {
            self.characters += 1;
        }
        if character == '\n' {
            self.lines += 1;
        }
    }
}

fn scan_counts(text: &str) -> ScanCounts {
    let mut counts = ScanCounts::default();
    for character in text.chars() {
        counts.push(character);
    }
    counts.lines += 1;
    counts
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
        assert_eq!(token_summary(""), snapshot.summary());
    }

    #[test]
    fn summary_matches_the_range_snapshot_on_a_large_single_line() {
        // One 100 KB line: the footer's allocation-free path must agree with
        // the token ranges the classifier uses, including apostrophes and
        // underscores inside words.
        let text = "don't_stop 中文 🦀 **bold** `code` ".repeat(2_800);
        let snapshot = token_snapshot(&text);
        assert_eq!(token_summary(&text), snapshot.summary());
        assert!(snapshot.words() > 0);
    }

    #[test]
    fn summary_matches_the_range_snapshot_across_the_word_rule() {
        for text in [
            "Two words, and a [link](https://x.dev).\n",
            "one\r\ntwo\r\n",
            "Été 中文 déjà-vu",
            "a_b c'd e''f __ _'",
        ] {
            assert_eq!(
                token_summary(text),
                token_snapshot(text).summary(),
                "{text:?}"
            );
        }
    }
    #[test]
    fn edited_summary_matches_full_scan_at_every_unicode_boundary() {
        for text in [
            "",
            "alpha beta",
            "don't_stop",
            "中文🦀 café",
            "a\r\nb\n",
            "a--b _'c",
        ] {
            let boundaries: Vec<_> = (0..=text.len())
                .filter(|&offset| text.is_char_boundary(offset))
                .collect();
            for &start in &boundaries {
                for &end in boundaries.iter().filter(|&&end| end >= start) {
                    for replacement in ["", "x", "中文", "🦀", "' _", "\r\n"] {
                        let mut edited = text.to_owned();
                        edited.replace_range(start..end, replacement);
                        assert_eq!(
                            token_summary(text).edited(text, start..end, replacement),
                            token_summary(&edited),
                            "{text:?}, {start}..{end}, {replacement:?}",
                        );
                    }
                }
            }
        }
    }
}
