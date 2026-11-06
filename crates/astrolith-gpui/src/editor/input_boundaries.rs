//! Byte boundaries shared by the visible editing surfaces.
//!
//! Movement and deletion operate on the retained editor model, so these helpers
//! read its rope directly and never derive boundaries from a rendered string.

use gpui_kit::component::input::Rope;
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

/// Next (`backward == false`) or previous grapheme boundary.
pub(crate) fn grapheme_boundary(text: &Rope, at: usize, backward: bool) -> usize {
    if (backward && at == 0) || (!backward && at == text.len()) {
        return at;
    }
    let mut cursor = GraphemeCursor::new(at, text.len(), true);
    let (mut chunk, mut start) = text.chunk(if backward { at - 1 } else { at });
    loop {
        let result = if backward {
            cursor.prev_boundary(chunk, start)
        } else {
            cursor.next_boundary(chunk, start)
        };
        match result {
            Ok(boundary) => return boundary.unwrap_or(at),
            Err(GraphemeIncomplete::PreContext(end)) => {
                let (context, context_start) = text.chunk(end - 1);
                cursor.provide_context(&context[..end - context_start], context_start);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                (chunk, start) = text.chunk(start - 1);
            }
            Err(GraphemeIncomplete::NextChunk) => {
                (chunk, start) = text.chunk(start + chunk.len());
            }
            Err(GraphemeIncomplete::InvalidOffset) => {
                unreachable!("cursor and chunks belong to the same rope")
            }
        }
    }
}

/// Next (`backward == false`) or previous word boundary.
pub(crate) fn word_boundary(text: &Rope, at: usize, backward: bool) -> usize {
    let mut offset = at;
    let mut in_word = false;
    let characters: Box<dyn Iterator<Item = char> + '_> = if backward {
        Box::new(text.chars_at(at).reversed())
    } else {
        Box::new(text.chars_at(at))
    };
    for character in characters {
        let word = character.is_alphanumeric() || character == '_';
        if in_word && !word {
            break;
        }
        in_word |= word;
        if backward {
            offset -= character.len_utf8();
        } else {
            offset += character.len_utf8();
        }
    }
    offset
}
