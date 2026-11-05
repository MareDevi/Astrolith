//! Position conversions between LSP (UTF-16 code units) and this editor.
//!
//! The GPUI editor's `Position::character` counts Unicode scalar values while
//! LSP counts UTF-16 code units. Servers send UTF-16 positions; every position
//! crossing into the UI must be converted, or ranges drift after an emoji or
//! another non-BMP character.

use lsp_types::Position;

/// Convert an LSP UTF-16 position into this editor's character-based position.
pub fn utf16_position_to_char(text: &str, position: Position) -> Position {
    let line = line_slice(text, position.line);
    let character = utf16_column_to_char(line, position.character);
    Position::new(position.line, character)
}

/// Convert this editor's character-based position into an LSP UTF-16 position.
pub fn char_position_to_utf16(text: &str, position: Position) -> Position {
    let line = line_slice(text, position.line);
    let character = char_column_to_utf16(line, position.character);
    Position::new(position.line, character)
}

/// Byte offset of an LSP UTF-16 position, clamped to the document.
pub fn position_to_offset(text: &str, position: Position) -> usize {
    let start = line_start(text, position.line);
    let line = line_slice(text, position.line);
    start + byte_offset_for_utf16_column(line, position.character)
}

/// Byte offset of a character-based position, clamped to the document.
pub fn char_position_to_offset(text: &str, position: Position) -> usize {
    let start = line_start(text, position.line);
    let line = line_slice(text, position.line);
    let column = line
        .chars()
        .take(position.character as usize)
        .map(char::len_utf8)
        .sum::<usize>();
    start + column.min(line.len())
}

/// LSP UTF-16 position of a byte offset, clamped to the document.
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let offset = floor_char_boundary(text, offset.min(text.len()));
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let column = text[line_start..offset]
        .chars()
        .map(char::len_utf16)
        .sum::<usize>() as u32;
    Position::new(line, column)
}

fn line_start(text: &str, line: u32) -> usize {
    if line == 0 {
        return 0;
    }
    let mut remaining = line;
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            remaining -= 1;
            if remaining == 0 {
                return index + 1;
            }
        }
    }
    text.len()
}

fn line_slice(text: &str, line: u32) -> &str {
    let start = line_start(text, line);
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |index| start + index);
    text[start..end].trim_end_matches('\r')
}

fn utf16_column_to_char(line: &str, utf16_column: u32) -> u32 {
    let mut seen = 0usize;
    for (index, character) in line.chars().enumerate() {
        if seen >= utf16_column as usize {
            return index as u32;
        }
        seen += character.len_utf16();
    }
    line.chars().count() as u32
}

fn char_column_to_utf16(line: &str, char_column: u32) -> u32 {
    line.chars()
        .take(char_column as usize)
        .map(char::len_utf16)
        .sum::<usize>() as u32
}

fn byte_offset_for_utf16_column(line: &str, utf16_column: u32) -> usize {
    let mut seen = 0usize;
    for (index, character) in line.char_indices() {
        if seen >= utf16_column as usize {
            return index;
        }
        seen += character.len_utf16();
    }
    line.len()
}

fn floor_char_boundary(text: &str, offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    let mut offset = offset;
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "let a = 1;\nlet 🎉x = \"中\";\nend\n";

    #[test]
    fn round_trips_ascii_positions() {
        let position = Position::new(0, 4);
        assert_eq!(char_position_to_utf16(TEXT, position), position);
        assert_eq!(utf16_position_to_char(TEXT, position), position);
        assert_eq!(position_to_offset(TEXT, position), 4);
        assert_eq!(offset_to_position(TEXT, 4), position);
    }

    #[test]
    fn converts_non_bmp_and_wide_characters() {
        // Line 1: `let 🎉x = "中";` — 🎉 is one scalar (2 UTF-16 units).
        assert_eq!(
            utf16_position_to_char(TEXT, Position::new(1, 4)),
            Position::new(1, 4)
        );
        assert_eq!(
            utf16_position_to_char(TEXT, Position::new(1, 6)),
            Position::new(1, 5)
        );
        assert_eq!(
            char_position_to_utf16(TEXT, Position::new(1, 5)),
            Position::new(1, 6)
        );
        assert_eq!(
            char_position_to_utf16(TEXT, Position::new(1, 4)),
            Position::new(1, 4)
        );

        let line_two = TEXT.lines().nth(1).unwrap();
        let emoji_byte = line_two.find('🎉').unwrap();
        let expected = line_two[..emoji_byte]
            .chars()
            .map(char::len_utf16)
            .sum::<usize>() as u32;
        assert_eq!(
            offset_to_position(TEXT, TEXT.find("🎉").unwrap()),
            Position::new(1, expected)
        );
        assert_eq!(
            position_to_offset(TEXT, Position::new(1, expected)),
            TEXT.find("🎉").unwrap()
        );
    }

    #[test]
    fn clamps_positions_past_the_end() {
        assert_eq!(position_to_offset(TEXT, Position::new(99, 99)), TEXT.len());
        assert_eq!(
            offset_to_position(TEXT, usize::MAX),
            offset_to_position(TEXT, TEXT.len())
        );
    }

    #[test]
    fn tolerates_a_mid_character_offset() {
        let offset = TEXT.find('🎉').unwrap() + 1;
        assert!(!TEXT.is_char_boundary(offset));
        let position = offset_to_position(TEXT, offset);
        assert_eq!(position_to_offset(TEXT, position), TEXT.find('🎉').unwrap());
    }
}

/// Rewrite every LSP position inside a JSON response into this editor's
/// character-based coordinates.
///
/// Servers answer in UTF-16; the editor converts a `Position::character` as a
/// character count. Converting the response once, at the protocol boundary,
/// keeps every provider free of ad-hoc position math.
pub fn convert_positions(value: &mut serde_json::Value, text: &str) {
    match value {
        serde_json::Value::Object(map) => {
            let converted = match (map.get("start").cloned(), map.get("end").cloned()) {
                (Some(start), Some(end)) => {
                    match (
                        serde_json::from_value::<Position>(start),
                        serde_json::from_value::<Position>(end),
                    ) {
                        (Ok(start), Ok(end)) => {
                            let start = utf16_position_to_char(text, start);
                            let end = utf16_position_to_char(text, end);
                            Some((
                                serde_json::to_value(start).ok(),
                                serde_json::to_value(end).ok(),
                            ))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((Some(start), Some(end))) = converted {
                map.insert("start".into(), start);
                map.insert("end".into(), end);
            }
            for value in map.values_mut() {
                convert_positions(value, text);
            }
        }
        serde_json::Value::Array(items) => {
            for value in items {
                convert_positions(value, text);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    #[test]
    fn converts_ranges_in_nested_responses() {
        let mut value = serde_json::json!({
            "range": {
                "start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 6}
            },
            "items": [{
                "textEdit": {
                    "range": {
                        "start": {"line": 0, "character": 6},
                        "end": {"line": 0, "character": 7}
                    },
                    "newText": "x"
                }
            }]
        });
        convert_positions(&mut value, "let 🎉x = 1");
        assert_eq!(value["range"]["start"]["character"], 4);
        assert_eq!(value["range"]["end"]["character"], 5);
        assert_eq!(
            value["items"][0]["textEdit"]["range"]["start"]["character"],
            5
        );
        assert_eq!(
            value["items"][0]["textEdit"]["range"]["end"]["character"],
            6
        );
    }

    #[test]
    fn leaves_non_position_objects_alone() {
        let mut value = serde_json::json!({"start": "text", "end": null});
        convert_positions(&mut value, "abc");
        assert_eq!(value, serde_json::json!({"start": "text", "end": null}));
    }
}
