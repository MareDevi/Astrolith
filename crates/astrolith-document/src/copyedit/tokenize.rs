//! Word token boundaries and Markdown/MDX exclusion ranges.
//!
//! Classification runs on the writer's *visible* projection, so hidden
//! frontmatter and MDX imports never reach this module. Everything visible is
//! still Markdown or MDX, so code, link destinations and tags must be masked
//! explicitly. The scan is deliberately mechanical: it has no parser state
//! beyond fences, backtick runs, bracket depth and quote state, and therefore
//! produces the same ranges for the same bytes on every platform.

use std::ops::Range;

/// Every maximal run of letters, digits, apostrophes or underscores.
///
/// The footer counts and the classifier share this definition, so a word is
/// either counted and eligible for classification or neither.
pub(crate) fn word_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start: Option<usize> = None;
    for (offset, character) in text.char_indices() {
        if is_word_character(character) {
            start.get_or_insert(offset);
        } else if let Some(begin) = start.take() {
            ranges.push(begin..offset);
        }
    }
    if let Some(begin) = start {
        ranges.push(begin..text.len());
    }
    ranges
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '\'' | '_')
}

/// Ranges whose bytes must never be classified: fenced and inline code,
/// link destinations, HTML/MDX tags and comments, MDX expressions and
/// reference definitions.
pub(crate) fn excluded_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = fenced_code(text);
    let mut cursor = 0;
    for fence in ranges.clone() {
        if cursor < fence.start {
            scan_inline(text, cursor, fence.start, &mut ranges);
        }
        cursor = fence.end;
    }
    if cursor < text.len() {
        scan_inline(text, cursor, text.len(), &mut ranges);
    }
    reference_definitions(text, &mut ranges);
    merge(ranges)
}

/// Whether any excluded range intersects `range`.
pub(crate) fn is_excluded(range: &Range<usize>, excluded: &[Range<usize>]) -> bool {
    let index = excluded.partition_point(|excluded| excluded.end <= range.start);
    excluded
        .get(index)
        .is_some_and(|excluded| excluded.start < range.end)
}

/// Opening and closing fence lines plus everything between them.
///
/// A fence opens on a line with at most three leading spaces and a run of at
/// least three backticks or tildes, and closes on a line at most three spaces in
/// whose run of the same character is at least as long with nothing but
/// whitespace after it. A more indented fence line is code content. An unclosed
/// fence runs to the end of the text.
fn fenced_code(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open: Option<(u8, usize, usize)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content_end = line.trim_end_matches(['\r', '\n']).len();
        let bytes = line.as_bytes();
        let indent = bytes.iter().take_while(|byte| **byte == b' ').count();
        let rest = &line[indent..content_end];
        match open {
            Some((character, length, start)) => {
                if indent <= 3 && closes_fence(rest, character, length) {
                    ranges.push(start..offset + line.len());
                    open = None;
                }
            }
            None => {
                if indent <= 3
                    && let Some((character, length)) = opens_fence(rest)
                {
                    open = Some((character, length, offset));
                }
            }
        }
        offset += line.len();
    }
    if let Some((_, _, start)) = open {
        ranges.push(start..text.len());
    }
    ranges
}

fn opens_fence(rest: &str) -> Option<(u8, usize)> {
    let character = *rest.as_bytes().first()?;
    if !matches!(character, b'`' | b'~') {
        return None;
    }
    let length = rest
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == character)
        .count();
    (length >= 3).then_some((character, length))
}

fn closes_fence(rest: &str, character: u8, minimum: usize) -> bool {
    let length = rest
        .as_bytes()
        .iter()
        .take_while(|byte| **byte == character)
        .count();
    length >= minimum && rest[length..].trim().is_empty()
}

/// Scan `text[from..to]` for inline code, tags and link destinations.
fn scan_inline(text: &str, from: usize, to: usize, ranges: &mut Vec<Range<usize>>) {
    let bytes = text.as_bytes();
    let mut index = from;
    while index < to {
        match bytes[index] {
            b'`' if !escaped(bytes, index) => {
                let length = run(bytes, index, b'`');
                if let Some(close) = closing_backticks(bytes, index + length, to, length) {
                    ranges.push(index..close + length);
                    index = close + length;
                } else {
                    index += length;
                }
            }
            b'<' if !escaped(bytes, index) => {
                if let Some(end) = tag_end(bytes, index, to) {
                    ranges.push(index..end);
                    index = end;
                } else {
                    index += utf8_len(bytes[index]);
                }
            }
            b'{' if !escaped(bytes, index) => {
                if let Some(end) = expression_end(bytes, index, to) {
                    ranges.push(index..end);
                    index = end;
                } else {
                    index += 1;
                }
            }
            b'[' if !escaped(bytes, index) => {
                if let Some(close) = closing_bracket(bytes, index, to) {
                    match bytes.get(close + 1) {
                        Some(b'(') => {
                            if let Some(end) = destination_end(bytes, close + 1, to) {
                                ranges.push(close + 1..end);
                            }
                        }
                        Some(b'[') => {
                            if let Some(reference) = closing_bracket(bytes, close + 1, to) {
                                ranges.push(close + 1..reference + 1);
                            }
                        }
                        _ => {}
                    }
                }
                index += 1;
            }
            byte => index += utf8_len(byte),
        }
    }
}

/// A backslash escapes the Markdown punctuation that follows it.
fn escaped(bytes: &[u8], index: usize) -> bool {
    let mut backslashes = 0;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        backslashes += 1;
        cursor -= 1;
    }
    backslashes % 2 == 1
}

fn run(bytes: &[u8], start: usize, character: u8) -> usize {
    bytes[start..]
        .iter()
        .take_while(|byte| **byte == character)
        .count()
}

/// The start of the next run of exactly `length` backticks, or `None`.
fn closing_backticks(bytes: &[u8], mut index: usize, to: usize, length: usize) -> Option<usize> {
    while index < to {
        if bytes[index] == b'`' {
            let candidate = run(bytes, index, b'`');
            if candidate == length {
                return Some(index);
            }
            index += candidate;
        } else {
            index += utf8_len(bytes[index]);
        }
    }
    None
}

/// The index of the `]` closing the label opened at `open`.
fn closing_bracket(bytes: &[u8], open: usize, to: usize) -> Option<usize> {
    let mut depth = 0;
    let mut index = open;
    while index < to {
        match bytes[index] {
            b'[' if !escaped(bytes, index) => {
                depth += 1;
                index += 1;
            }
            b']' if !escaped(bytes, index) => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index - 1);
                }
            }
            byte => index += utf8_len(byte),
        }
    }
    None
}

/// The byte just past the destination opened at `open` (`(`).
fn destination_end(bytes: &[u8], open: usize, to: usize) -> Option<usize> {
    let mut index = open + 1;
    while index < to && matches!(bytes[index], b' ' | b'\t' | b'\r' | b'\n') {
        index += 1;
    }
    if bytes.get(index) == Some(&b'<') {
        let mut angle = index + 1;
        while angle < to && bytes[angle] != b'>' {
            angle += utf8_len(bytes[angle]);
        }
        if angle >= to {
            return None;
        }
        let mut close = angle + 1;
        while close < to && matches!(bytes[close], b' ' | b'\t' | b'\r' | b'\n') {
            close += 1;
        }
        return (bytes.get(close) == Some(&b')')).then_some(close + 1);
    }
    let mut depth = 1;
    let mut quote = false;
    while index < to {
        match bytes[index] {
            b'\\' => index += 1 + utf8_len(*bytes.get(index + 1).unwrap_or(&b' ')),
            b'"' => {
                quote = !quote;
                index += 1;
            }
            b'(' if !quote => {
                depth += 1;
                index += 1;
            }
            b')' if !quote => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            byte => index += utf8_len(byte),
        }
        if index > to {
            return None;
        }
    }
    None
}

/// The byte just past an HTML comment or HTML/MDX tag starting at `open`.
fn tag_end(bytes: &[u8], open: usize, to: usize) -> Option<usize> {
    let rest = &bytes[open..to];
    if rest.starts_with(b"<!--") {
        return find(bytes, open + 4, to, b"-->").map(|position| position + 3);
    }
    let mut index = open + 1;
    match bytes.get(index) {
        Some(b'/') | Some(b'!') | Some(b'?') => index += 1,
        Some(byte) if byte.is_ascii_alphabetic() => {}
        _ => return None,
    }
    let mut quote = None;
    let mut braces = 0usize;
    while index < to {
        let byte = bytes[index];
        match quote {
            Some(delimiter) => {
                if byte == delimiter {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'{' => braces += 1,
                b'}' => braces = braces.saturating_sub(1),
                b'>' if braces == 0 => return Some(index + 1),
                _ => {}
            },
        }
        index += utf8_len(byte);
    }
    None
}

/// The byte just past the MDX comment or balanced `{expression}` opened at
/// `open`, or `None` when the braces never balance.
///
/// Braces outside tags are MDX JavaScript, so an identifier inside them is not
/// prose. Quoted strings are skipped so a `}` inside one does not close early.
fn expression_end(bytes: &[u8], open: usize, to: usize) -> Option<usize> {
    if bytes[open..to].starts_with(b"{/*") {
        return find(bytes, open + 3, to, b"*/}").map(|position| position + 3);
    }
    let mut depth = 0usize;
    let mut quote = None;
    let mut index = open;
    while index < to {
        let byte = bytes[index];
        match quote {
            Some(delimiter) => {
                if byte == delimiter {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'{' => depth += 1,
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(index + 1);
                    }
                }
                b'\\' => index += 1,
                _ => {}
            },
        }
        index += 1;
    }
    None
}

fn find(bytes: &[u8], from: usize, to: usize, needle: &[u8]) -> Option<usize> {
    let mut index = from;
    while index + needle.len() <= to {
        if &bytes[index..index + needle.len()] == needle {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Exclude the destination of `[id]: /path "title"` definitions.
///
/// Footnote definitions (`[^1]: text`) match the same shape, so their prose is
/// excluded too; footnotes are not visible prose in the writer projection.
fn reference_definitions(text: &str, ranges: &mut Vec<Range<usize>>) {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        let indent = content.len() - content.trim_start_matches(' ').len();
        if indent <= 3 {
            let rest = &content[indent..];
            if rest.starts_with('[')
                && let Some(close) = rest.find("]:")
            {
                let destination = rest[close + 2..].trim_start();
                if !destination.is_empty() {
                    let start = offset + indent + close + 2;
                    ranges.push(start..offset + content.len());
                }
            }
        }
        offset += line.len();
    }
}

fn utf8_len(byte: u8) -> usize {
    match byte {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// Sort and coalesce overlapping or adjacent ranges.
fn merge(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn excluded_words(text: &str) -> Vec<&str> {
        let excluded = excluded_ranges(text);
        word_ranges(text)
            .into_iter()
            .filter(|range| is_excluded(range, &excluded))
            .map(|range| &text[range])
            .collect()
    }

    #[test]
    fn word_ranges_keep_apostrophes_and_unicode_boundaries() {
        let text = "don't déjà-vu 中文";
        let ranges = word_ranges(text);
        assert_eq!(
            ranges
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["don't", "déjà", "vu", "中文"]
        );
        for range in ranges {
            assert!(text.is_char_boundary(range.start));
            assert!(text.is_char_boundary(range.end));
        }
    }

    #[test]
    fn fenced_and_inline_code_are_excluded_with_crlf() {
        let text = "Before\r\n```rust\r\nlet hidden = 1;\r\n```\r\nAfter `inline code` tail\r\n";
        assert_eq!(
            excluded_words(text),
            ["rust", "let", "hidden", "1", "inline", "code"]
        );
    }

    #[test]
    fn inline_code_and_unclosed_fences_are_bounded() {
        let text = "Keep `open and ` tail\n```\ncode\n";
        assert_eq!(excluded_words(text), ["open", "and", "code"]);
    }

    #[test]
    fn indented_fence_lines_do_not_close_an_open_fence() {
        let text = "Before\n```\ncode one\n    ```\nmore code\n```\nAfter\n";
        assert_eq!(excluded_words(text), ["code", "one", "more", "code"]);
    }

    #[test]
    fn link_destinations_and_reference_definitions_are_excluded() {
        let text =
            "[Read](https://example.dev/word) [ref][target] now\n\n[refid]: /guide \"Title\"\n";
        assert_eq!(
            excluded_words(text),
            [
                "https", "example", "dev", "word", "target", "guide", "Title"
            ]
        );
    }

    #[test]
    fn html_and_mdx_tags_are_excluded_but_their_content_is_not() {
        let text = "<Card kind=\"verb\" value={1 > 0}>Visible</Card>\n";
        assert_eq!(
            excluded_words(text),
            ["Card", "kind", "verb", "value", "1", "0", "Card"]
        );
    }

    #[test]
    fn mdx_comments_and_expressions_are_excluded() {
        let text =
            "Keep {/* hidden note */} and {data.author} and {items.map((item) => item)} tail\n";
        assert_eq!(
            excluded_words(text),
            [
                "hidden", "note", "data", "author", "items", "map", "item", "item"
            ]
        );
    }

    #[test]
    fn html_comments_are_excluded() {
        let text = "Before <!-- hidden comment --> after\n";
        assert_eq!(excluded_words(text), ["hidden", "comment"]);
    }

    #[test]
    fn escaped_punctuation_is_not_markup() {
        let text = r"\[label\](dest) \<notatag>";
        assert!(excluded_words(text).is_empty());
    }

    #[test]
    fn merged_exclusions_are_sorted_and_disjoint() {
        let text = "`a[b]` <x y> [z](q)\n\n[id]: /p\n";
        let ranges = excluded_ranges(text);
        assert!(ranges.windows(2).all(|pair| pair[0].end < pair[1].start));
    }
}
