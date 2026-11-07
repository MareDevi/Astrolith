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
    let mut characters = text.char_indices().peekable();
    let mut ranges = Vec::new();
    let mut start = 0;
    while let Some((_, character)) = characters.next() {
        if character != '\n' && !is_terminator(character) {
            continue;
        }
        let mut after = characters.clone();
        if character == '\n' {
            while after
                .peek()
                .is_some_and(|(_, c)| matches!(c, ' ' | '\t' | '\r'))
            {
                after.next();
            }
            if !after.peek().is_some_and(|(_, c)| *c == '\n') {
                continue;
            }
            after.next();
        } else {
            if !is_cjk_terminator(character) {
                while after.peek().is_some_and(|(_, c)| is_closer(*c)) {
                    after.next();
                }
                if after.peek().is_some_and(|(_, c)| !c.is_whitespace()) {
                    continue;
                }
            }
            while after.peek().is_some_and(|(_, c)| c.is_whitespace()) {
                after.next();
            }
        }
        let end = after.peek().map_or(text.len(), |(offset, _)| *offset);
        ranges.push(start..end);
        start = end;
        characters = after;
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
