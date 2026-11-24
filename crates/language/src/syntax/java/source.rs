use std::{ops::Range, sync::Arc};

use super::{
    SyntaxCancellation,
    lexer::{JavaTokenKind, Lexer, identifier_ignorable},
    translation,
};

/// The parser reads translated Java; all token output uses the original byte offsets.
pub(super) struct Source {
    pub text: Arc<str>,
    boundaries: Option<Vec<usize>>,
}

impl Source {
    pub fn new(raw: String, cancel: &SyntaxCancellation) -> Option<Self> {
        let translated = if raw.contains("\\u") {
            Some(translation::translate(&raw, cancel)?)
        } else {
            None
        };
        let text = translated
            .as_ref()
            .map_or(raw.as_str(), |source| &source.text);
        let previous = translated
            .as_ref()
            .map(|source| source.boundaries.as_slice());
        if let Some((text, boundaries)) = identifier_source(text, previous, cancel)? {
            return Some(Self {
                text: text.into(),
                boundaries: Some(boundaries),
            });
        }
        Some(match translated {
            Some(translated) => Self {
                text: translated.text.into(),
                boundaries: Some(translated.boundaries),
            },
            None => Self {
                text: raw.into(),
                boundaries: None,
            },
        })
    }

    pub fn mapped(&self) -> bool {
        self.boundaries.is_some()
    }

    pub fn raw_byte(&self, byte: usize) -> usize {
        self.boundaries.as_ref().map_or(byte, |map| map[byte])
    }

    pub fn raw_bytes(&self, bytes: Range<usize>) -> Range<usize> {
        self.raw_byte(bytes.start)..self.raw_byte(bytes.end)
    }
}

/// Materialize identifier identities only after lexical boundaries are known.
/// Comments, literals and invalid standalone characters keep their spelling.
fn identifier_source(
    text: &str,
    previous: Option<&[usize]>,
    cancel: &SyntaxCancellation,
) -> Option<Option<(String, Vec<usize>)>> {
    if !text.chars().any(identifier_ignorable) {
        return Some(None);
    }
    let mut lexer = Lexer::default();
    if !lexer.advance(text, usize::MAX, cancel) {
        return None;
    }
    let mut removed = Vec::new();
    for token in lexer.tokens {
        if cancel.is_cancelled() {
            return None;
        }
        if matches!(
            token.kind,
            JavaTokenKind::Identifier | JavaTokenKind::Keyword
        ) {
            for (offset, character) in token.text(text).char_indices() {
                if identifier_ignorable(character) {
                    let start = token.bytes.start + offset;
                    removed.push(start..start + character.len_utf8());
                }
            }
        }
    }
    if removed.is_empty() {
        return Some(None);
    }
    let raw_byte = |byte: usize| previous.map_or(byte, |map| map[byte]);
    let mut normalized = String::with_capacity(text.len());
    let mut boundaries = vec![raw_byte(0)];
    let mut next = removed.into_iter().peekable();
    for (start, character) in text.char_indices() {
        if cancel.is_cancelled() {
            return None;
        }
        let end = start + character.len_utf8();
        if next.peek().is_some_and(|range| range.start == start) {
            // A trailing ignorable belongs to the preceding identifier scalar,
            // so the public token/identifier range includes its raw spelling.
            *boundaries.last_mut().unwrap() = raw_byte(end);
            next.next();
        } else {
            normalized.push(character);
            boundaries.extend((start + 1..=end).map(raw_byte));
        }
    }
    Some(Some((normalized, boundaries)))
}
