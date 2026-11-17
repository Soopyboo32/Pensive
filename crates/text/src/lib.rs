//! Shared text values, with no I/O, GPUI or mutable document owner.
//!
//! Source ranges are half-open UTF-8 byte ranges. Offsets must be scalar boundaries;
//! neither conversion nor slicing silently rounds or normalizes text. CRLF is retained.
//! Snapshot and identity design draws on SoopyCode's editor-model; see NOTICE.

use std::{fmt, ops::Range, sync::Arc};

pub use ropey::Rope;

/// Opaque identity for one source during its owner's lifetime. Cloning retains identity;
/// constructing a new identity distinguishes reopened files and the two sides of a diff.
#[derive(Clone, Default)]
pub struct SourceId(Arc<()>);

impl PartialEq for SourceId {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SourceId {}

impl fmt::Debug for SourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SourceId")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

/// Revisions only have meaning together with source identity. They increase monotonically,
/// including undo/redo, and are never reused for a source. These stamps are process-local.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStamp {
    pub source: SourceId,
    pub revision: usize,
}

impl SourceStamp {
    /// The next revision of the same source. Exhaustion is an error, never wraparound.
    pub fn next(&self) -> Self {
        Self {
            source: self.source.clone(),
            revision: self
                .revision
                .checked_add(1)
                .expect("source revision exhausted"),
        }
    }
}

/// A cheap Rope clone with a stable source stamp. The owner must issue a new revision
/// whenever its contents change. Mutating the owner's rope cannot change this value.
#[derive(Clone, Debug)]
pub struct TextSnapshot {
    stamp: SourceStamp,
    rope: Rope,
}

impl TextSnapshot {
    pub fn new(stamp: SourceStamp, rope: &Rope) -> Self {
        Self {
            stamp,
            rope: rope.clone(),
        }
    }

    pub fn stamp(&self) -> &SourceStamp {
        &self.stamp
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }
}

/// A zero-based LF line and UTF-8 byte column, suitable for incremental parser edits.
/// CR is an ordinary byte; the position between CR and LF is valid and preserves both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BytePoint {
    pub line: usize,
    pub column: usize,
}

/// An invalid or reversed range, an out-of-bounds offset, or an offset inside a scalar
/// (UTF-8) or surrogate pair (UTF-16). Strict boundaries return this instead of rounding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidOffset;

impl fmt::Display for InvalidOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("offset is out of bounds or not on a character boundary")
    }
}

impl std::error::Error for InvalidOffset {}

/// Validates a half-open byte range, including order and both scalar boundaries.
pub fn validate_range(rope: &Rope, range: &Range<usize>) -> Result<(), InvalidOffset> {
    if range.start > range.end {
        return Err(InvalidOffset);
    }
    byte_to_char(rope, range.start)?;
    byte_to_char(rope, range.end)?;
    Ok(())
}

/// The scalar index at a UTF-8 boundary. Scalar indices are only used at the Rope boundary.
pub fn byte_to_char(rope: &Rope, byte: usize) -> Result<usize, InvalidOffset> {
    let char = rope.try_byte_to_char(byte).map_err(|_| InvalidOffset)?;
    (rope.char_to_byte(char) == byte)
        .then_some(char)
        .ok_or(InvalidOffset)
}

pub fn byte_to_utf16(rope: &Rope, byte: usize) -> Result<usize, InvalidOffset> {
    Ok(rope.char_to_utf16_cu(byte_to_char(rope, byte)?))
}

/// Rejects surrogate interiors and offsets beyond EOF. Platform input adapters may apply
/// their own documented clamping before entering this strict shared contract.
pub fn utf16_to_byte(rope: &Rope, utf16: usize) -> Result<usize, InvalidOffset> {
    let char = rope
        .try_utf16_cu_to_char(utf16)
        .map_err(|_| InvalidOffset)?;
    if rope.char_to_utf16_cu(char) != utf16 {
        return Err(InvalidOffset);
    }
    Ok(rope.char_to_byte(char))
}

pub fn byte_to_point(rope: &Rope, byte: usize) -> Result<BytePoint, InvalidOffset> {
    byte_to_char(rope, byte)?;
    let line = rope.byte_to_line(byte);
    Ok(BytePoint {
        line,
        column: byte - rope.line_to_byte(line),
    })
}

/// One replacement in a transaction. `old`/`old_points` refer to the text immediately
/// before this edit; `new`/`new_points` refer to the text immediately after it. Later edits
/// use that resulting text, not the original transaction's coordinate space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub old: Range<usize>,
    pub new: Range<usize>,
    pub old_points: Range<BytePoint>,
    pub new_points: Range<BytePoint>,
}

impl TextEdit {
    /// Describes a replacement without mutating or copying the source text.
    pub fn replacement(
        rope: &Rope,
        old: Range<usize>,
        inserted: &str,
    ) -> Result<Self, InvalidOffset> {
        validate_range(rope, &old)?;
        let start = byte_to_point(rope, old.start)?;
        let old_end = byte_to_point(rope, old.end)?;
        let lines = inserted.bytes().filter(|&byte| byte == b'\n').count();
        let new_end = BytePoint {
            line: start.line + lines,
            column: inserted
                .rfind('\n')
                .map_or(start.column + inserted.len(), |lf| inserted.len() - lf - 1),
        };
        Ok(Self {
            new: old.start..old.start + inserted.len(),
            old,
            old_points: start..old_end,
            new_points: start..new_end,
        })
    }
}

/// One atomic publication: one revision for typing/replacement, an entire undo/redo group,
/// or a disk reload. `edits` are nonempty and in application order (undo reverses history;
/// redo follows it). No intermediate revision is published. A consumer that missed `before`
/// must rebuild from a snapshot rather than applying this transaction to unrelated text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextChange {
    pub before: SourceStamp,
    pub after: SourceStamp,
    pub edits: Vec<TextEdit>,
}

impl TextChange {
    /// Earliest LF line invalidated by this ordered transaction.
    pub fn first_changed_line(&self) -> usize {
        self.edits
            .iter()
            .map(|edit| edit.old_points.start.line)
            .min()
            .expect("a change has edits")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(rope: &Rope) -> TextSnapshot {
        TextSnapshot::new(
            SourceStamp {
                source: SourceId::default(),
                revision: 0,
            },
            rope,
        )
    }

    #[test]
    fn snapshots_share_storage_but_never_follow_mutations() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<TextSnapshot>();
        let mut rope = Rope::from_str(&"é😀\r\n".repeat(2000));
        let before = snapshot(&rope);
        assert!(before.rope().is_instance(&rope));
        let copy = before.clone();
        rope.remove(0..2);
        rope.insert(0, "changed");
        assert_eq!(before.rope().to_string(), "é😀\r\n".repeat(2000));
        assert_eq!(copy.stamp(), before.stamp());
        assert!(copy.rope().is_instance(before.rope()));
    }

    #[test]
    fn equal_revisions_and_storage_do_not_identify_a_source() {
        let rope = Rope::from_str("same text");
        let first = snapshot(&rope);
        let second = snapshot(&rope);
        assert_eq!(first.stamp().revision, second.stamp().revision);
        assert_ne!(first.stamp(), second.stamp());
        let next = TextSnapshot::new(first.stamp().next(), &rope);
        assert_eq!(first.stamp().source, next.stamp().source);
        assert_ne!(first.stamp(), next.stamp());
    }

    #[test]
    fn conversions_are_exact_and_reject_scalar_and_surrogate_interiors() {
        let rope = Rope::from_str("aé😀\r\n中\n");
        for (byte, utf16) in [
            (0, 0),
            (1, 1),
            (3, 2),
            (7, 4),
            (8, 5),
            (9, 6),
            (12, 7),
            (13, 8),
        ] {
            assert_eq!(byte_to_utf16(&rope, byte), Ok(utf16));
            assert_eq!(utf16_to_byte(&rope, utf16), Ok(byte));
        }
        for byte in [2, 4, 5, 6, 10, 11, 14, usize::MAX] {
            assert_eq!(byte_to_utf16(&rope, byte), Err(InvalidOffset));
            assert_eq!(byte_to_point(&rope, byte), Err(InvalidOffset));
        }
        for utf16 in [3, 9, usize::MAX] {
            assert_eq!(utf16_to_byte(&rope, utf16), Err(InvalidOffset));
        }
        assert_eq!(
            validate_range(&rope, &Range { start: 7, end: 3 }),
            Err(InvalidOffset)
        );
        assert_eq!(validate_range(&rope, &(1..2)), Err(InvalidOffset));
        assert_eq!(validate_range(&rope, &(13..13)), Ok(()));
        assert_eq!(
            byte_to_point(&rope, 8),
            Ok(BytePoint { line: 0, column: 8 })
        );
        assert_eq!(
            byte_to_point(&rope, 9),
            Ok(BytePoint { line: 1, column: 0 })
        );
        assert_eq!(
            byte_to_point(&rope, 13),
            Ok(BytePoint { line: 2, column: 0 })
        );
    }

    #[test]
    fn replacement_reports_bytes_and_lf_points_without_normalizing_crlf() {
        let mut rope = Rope::from_str("é\r\n😀x\nlast");
        let inserted = "中\r\n🙂";
        let edit = TextEdit::replacement(&rope, 4..9, inserted).unwrap();
        assert_eq!(edit.old, 4..9);
        assert_eq!(edit.new, 4..13);
        assert_eq!(
            edit.old_points,
            BytePoint { line: 1, column: 0 }..BytePoint { line: 1, column: 5 }
        );
        assert_eq!(
            edit.new_points,
            BytePoint { line: 1, column: 0 }..BytePoint { line: 2, column: 4 }
        );
        rope.remove(rope.byte_to_char(4)..rope.byte_to_char(9));
        rope.insert(rope.byte_to_char(4), inserted);
        assert_eq!(
            byte_to_point(&rope, edit.new.end).unwrap(),
            edit.new_points.end
        );
        assert_eq!(rope.to_string(), "é\r\n中\r\n🙂\nlast");
        assert_eq!(TextEdit::replacement(&rope, 5..5, ""), Err(InvalidOffset));
    }

    #[test]
    fn empty_text_and_line_end_edits_have_valid_points() {
        let rope = Rope::new();
        assert_eq!(byte_to_utf16(&rope, 0), Ok(0));
        assert_eq!(utf16_to_byte(&rope, 0), Ok(0));
        let edit = TextEdit::replacement(&rope, 0..0, "\r\n").unwrap();
        assert_eq!(edit.new, 0..2);
        assert_eq!(edit.new_points.end, BytePoint { line: 1, column: 0 });
    }
}
