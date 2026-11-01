//! Deterministic sentence detection for focus mode.
//!
//! Focus mode dims every sentence except the one holding the caret, so the
//! boundaries must be stable across frames and cheap enough to recompute on a
//! cursor move. The rules are deliberately mechanical rather than
//! language-aware:
//!
//! - `.`, `!`, `?` and `…` end a sentence when the next character is
//!   whitespace or the text ends, after skipping closing quotes and brackets
//!   (`."`, `?)`, …). A `.` between digits (`3.14`) and a `.` followed directly
//!   by a word character (`e.g.x`, `file.md`) do not end a sentence.
//! - `。`, `！` and `？` always end a sentence; they need no following space.
//! - A blank line ends a sentence even without a terminator, so paragraphs are
//!   never merged.
//!
//! The returned ranges tile the input exactly: they start at 0, do not
//! overlap, and the last one ends at `text.len()`. Whitespace after a
//! terminator belongs to the sentence it follows.

use std::ops::Range;

/// Every sentence of `text`, in order, tiling the whole input.
pub fn sentences(text: &str) -> Vec<Range<usize>> {
    let characters: Vec<(usize, char)> = text.char_indices().collect();
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut index = 0;
    while index < characters.len() {
        let (_, character) = characters[index];
        if character == '\n'
            && let Some(next) = blank_line_break(&characters, index)
        {
            let end = offset_at(&characters, next, text.len());
            ranges.push(start..end);
            start = end;
            index = next;
            continue;
        }
        if is_terminator(character)
            && ends_sentence(&characters, index)
            && let Some(end) = sentence_end(&characters, index, text.len())
        {
            ranges.push(start..end);
            start = end;
            index = characters
                .iter()
                .position(|(offset, _)| *offset >= end)
                .unwrap_or(characters.len());
            continue;
        }
        index += 1;
    }
    if start < text.len() {
        ranges.push(start..text.len());
    }
    ranges
}

/// The sentence containing `cursor`, or `None` for empty text.
///
/// A cursor past the end belongs to the last sentence.
pub fn cursor_sentence(text: &str, cursor: usize) -> Option<Range<usize>> {
    let ranges = sentences(text);
    let cursor = cursor.min(text.len());
    ranges
        .iter()
        .find(|range| range.start <= cursor && cursor < range.end)
        .or_else(|| ranges.last())
        .cloned()
}

/// Every sentence except the one holding `cursor`.
///
/// These are the ranges focus mode dims. Neighbouring sentences stay separate
/// entries, so the cursor sentence leaves one clean undimmed span.
pub fn other_sentences(text: &str, cursor: usize) -> Vec<Range<usize>> {
    let Some(cursor_sentence) = cursor_sentence(text, cursor) else {
        return Vec::new();
    };
    sentences(text)
        .into_iter()
        .filter(|range| *range != cursor_sentence)
        .collect()
}

fn is_terminator(character: char) -> bool {
    matches!(character, '.' | '!' | '?' | '…' | '。' | '！' | '？')
}

fn is_cjk_terminator(character: char) -> bool {
    matches!(character, '。' | '！' | '？')
}

fn is_closer(character: char) -> bool {
    matches!(
        character,
        '"' | '\'' | ')' | ']' | '}' | '»' | '”' | '’' | '」' | '』' | '】' | '〉' | '》'
    )
}

/// Index of the character after a blank line starting at a line break.
fn blank_line_break(characters: &[(usize, char)], index: usize) -> Option<usize> {
    let mut next = index + 1;
    while next < characters.len() && matches!(characters[next].1, ' ' | '\t' | '\r') {
        next += 1;
    }
    (next < characters.len() && characters[next].1 == '\n').then_some(next + 1)
}

/// Byte offset of the character at `index`, or `end` past the last one.
fn offset_at(characters: &[(usize, char)], index: usize, end: usize) -> usize {
    characters.get(index).map_or(end, |(offset, _)| *offset)
}

/// Whether the terminator at `index` ends a sentence.
fn ends_sentence(characters: &[(usize, char)], index: usize) -> bool {
    let (_, terminator) = characters[index];
    let mut next = index + 1;
    while next < characters.len() && is_closer(characters[next].1) {
        next += 1;
    }
    let following = characters.get(next).map(|(_, character)| *character);
    match following {
        None => true,
        Some(character) => {
            if is_cjk_terminator(terminator) {
                return true;
            }
            if !character.is_whitespace() {
                return false;
            }
            // `3.14` never ends a sentence, even before whitespace.
            !(terminator == '.'
                && index > 0
                && characters[index - 1].1.is_ascii_digit()
                && characters
                    .get(next)
                    .is_some_and(|(_, character)| character.is_ascii_digit()))
        }
    }
}

/// End offset of the sentence whose terminator is at `index`.
fn sentence_end(characters: &[(usize, char)], index: usize, end: usize) -> Option<usize> {
    let mut next = index + 1;
    if !is_cjk_terminator(characters[index].1) {
        while next < characters.len() && is_closer(characters[next].1) {
            next += 1;
        }
    }
    while next < characters.len() && characters[next].1.is_whitespace() {
        next += 1;
    }
    Some(offset_at(characters, next, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(text: &str) -> Vec<&str> {
        sentences(text)
            .into_iter()
            .map(|range| &text[range])
            .collect()
    }

    /// The ranges must tile the input: no gaps, no overlaps, full coverage.
    fn assert_tiles(text: &str) {
        let ranges = sentences(text);
        let mut cursor = 0;
        for range in &ranges {
            assert_eq!(range.start, cursor, "{text:?}");
            assert!(range.end > range.start, "{text:?}");
            cursor = range.end;
        }
        assert_eq!(cursor, text.len(), "{text:?}");
    }

    #[test]
    fn splits_on_ascii_terminators_and_keeps_trailing_space() {
        let text = "One. Two! Three?";
        assert_eq!(pieces(text), ["One. ", "Two! ", "Three?"]);
        assert_tiles(text);
    }

    #[test]
    fn splits_after_closing_quotes_and_brackets() {
        let text = "He said \"stop.\" Then (quietly?) left.";
        assert_eq!(
            pieces(text),
            ["He said \"stop.\" ", "Then (quietly?) ", "left."]
        );
    }

    #[test]
    fn does_not_split_decimals_or_file_names() {
        let text = "Pi is 3.14 and file.md stays whole.";
        assert_eq!(pieces(text), [text]);
    }

    #[test]
    fn blank_lines_end_sentences_without_a_terminator() {
        let text = "first line\n\nsecond line";
        assert_eq!(pieces(text), ["first line\n\n", "second line"]);
        assert_tiles(text);
    }

    #[test]
    fn keeps_crlf_line_endings_intact() {
        let text = "First.\r\n\r\nSecond.\r\n";
        assert_eq!(pieces(text), ["First.\r\n\r\n", "Second.\r\n"]);
        assert_tiles(text);
        for range in sentences(text) {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn splits_cjk_terminators_without_whitespace() {
        let text = "日本語。次の文！三つ？";
        assert_eq!(pieces(text), ["日本語。", "次の文！", "三つ？"]);
        assert_tiles(text);
    }

    #[test]
    fn unicode_text_keeps_character_boundaries() {
        let text = "Été. Ça va? Très bien… 中文。";
        assert_tiles(text);
        let ranges = sentences(text);
        assert_eq!(ranges.len(), 4);
        for range in ranges {
            assert!(text.is_char_boundary(range.start) && text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn text_without_a_terminator_is_one_sentence() {
        assert_eq!(pieces("no terminator here"), ["no terminator here"]);
        assert_eq!(pieces(""), Vec::<&str>::new());
    }

    #[test]
    fn cursor_selects_the_sentence_it_sits_in() {
        let text = "One. Two!\n\nThree?";
        assert_eq!(cursor_sentence(text, 0), Some(0..5));
        assert_eq!(cursor_sentence(text, 4), Some(0..5));
        // Trailing whitespace belongs to the sentence before it.
        assert_eq!(cursor_sentence(text, 5), Some(5..11));
        assert_eq!(cursor_sentence(text, 10), Some(5..11));
        assert_eq!(cursor_sentence(text, 11), Some(11..17));
        // Past the end belongs to the last sentence.
        assert_eq!(cursor_sentence(text, text.len() + 4), Some(11..17));
        assert_eq!(cursor_sentence("", 0), None);
    }

    #[test]
    fn other_sentences_dim_everything_but_the_cursor_sentence() {
        let text = "One. Two! Three?";
        let dimmed = other_sentences(text, 6);
        assert_eq!(
            dimmed
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["One. ", "Three?"]
        );
        for range in &dimmed {
            assert!(!(range.start <= 6 && 6 < range.end));
        }
    }
}
