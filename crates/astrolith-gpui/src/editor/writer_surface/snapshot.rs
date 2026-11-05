//! Byte-coordinate mapping while a rich snapshot is being parsed off-thread.

use gpui_kit::component::input::Rope;
use std::ops::Range;

/// One conservative replacement between the displayed and latest snapshots.
/// Source ranges stay in UTF-8 bytes, including composition text.
pub(super) struct PendingEdit {
    old: Range<usize>,
    new: Range<usize>,
}
impl PendingEdit {
    pub(super) fn between(before: &Rope, after: &Rope) -> Option<Self> {
        let mut prefix = common_bytes(before.chunks(), after.chunks(), false, usize::MAX);
        if prefix == before.len() && prefix == after.len() {
            return None;
        }
        while !before.is_char_boundary(prefix) || !after.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = common_bytes(
            before.chunks_at(before.len()).0.reversed(),
            after.chunks_at(after.len()).0.reversed(),
            true,
            (before.len() - prefix).min(after.len() - prefix),
        );
        while !before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        Some(Self {
            old: prefix..before.len() - suffix,
            new: prefix..after.len() - suffix,
        })
    }
    pub(super) fn to_current(&self, offset: usize) -> Option<usize> {
        if offset <= self.old.start {
            Some(offset)
        } else if offset >= self.old.end {
            Some(self.new.end + offset - self.old.end)
        } else {
            None
        }
    }
    pub(super) fn to_parsed(&self, offset: usize) -> usize {
        if offset <= self.new.start {
            offset
        } else if offset >= self.new.end {
            self.old.end + offset - self.new.end
        } else {
            self.old.start
        }
    }
    pub(super) fn contains_current(&self, offset: usize) -> bool {
        self.new.contains(&offset)
    }
}

fn common_bytes<'a, 'b>(
    mut left: impl Iterator<Item = &'a str>,
    mut right: impl Iterator<Item = &'b str>,
    reversed: bool,
    limit: usize,
) -> usize {
    let (mut a, mut b): (&[u8], &[u8]) = (&[], &[]);
    let mut count = 0;
    while count < limit {
        if a.is_empty() {
            let Some(chunk) = left.next() else { break };
            a = chunk.as_bytes();
        }
        if b.is_empty() {
            let Some(chunk) = right.next() else { break };
            b = chunk.as_bytes();
        }
        let len = a.len().min(b.len()).min(limit - count);
        let (aa, bb) = if reversed {
            (&a[a.len() - len..], &b[b.len() - len..])
        } else {
            (&a[..len], &b[..len])
        };
        if aa != bb {
            return count
                + if reversed {
                    aa.iter()
                        .rev()
                        .zip(bb.iter().rev())
                        .take_while(|(a, b)| a == b)
                        .count()
                } else {
                    aa.iter().zip(bb).take_while(|(a, b)| a == b).count()
                };
        }
        count += len;
        if reversed {
            a = &a[..a.len() - len];
            b = &b[..b.len() - len];
        } else {
            a = &a[len..];
            b = &b[len..];
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::input::Rope;

    #[test]
    fn pending_edit_maps_unchanged_text_and_refuses_replaced_pointer_targets() {
        let before = Rope::from_str("前\r\nHello\n後");
        let mut after = before.clone();
        after.remove(5..10);
        after.insert(5, "日本🦀");
        let edit = PendingEdit::between(&before, &after).unwrap();
        assert_eq!(edit.old, 5..10);
        assert_eq!(edit.new, 5..15);
        assert_eq!(edit.to_current(3), Some(3));
        assert_eq!(edit.to_current(8), None);
        assert_eq!(edit.to_current(11), Some(16));
        assert_eq!(edit.to_parsed(9), 5);
        assert_eq!(edit.to_parsed(16), 11);
        assert!(PendingEdit::between(&before, &before.clone()).is_none());
    }

    #[test]
    fn pending_unicode_edit_crossing_rope_chunks_keeps_character_boundaries() {
        let text = "中文🦀 é\r\n".repeat(4_000);
        let before = Rope::from_str(&text);
        let mut after = before.clone();
        let at = text.len() / 2;
        let at = (0..=at)
            .rev()
            .find(|&ix| text.is_char_boundary(ix))
            .unwrap();
        after.insert(at, "日本語");
        let edit = PendingEdit::between(&before, &after).unwrap();
        assert_eq!(edit.old, at..at);
        assert_eq!(edit.new, at..at + "日本語".len());
        assert_eq!(edit.to_current(before.len()), Some(after.len()));
    }
}
