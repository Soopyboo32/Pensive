//! JLS 3.3 translation happens before recognizing comments, literals or keywords.
//! Keep every translated UTF-8 boundary mapped back to the original input.
use super::SyntaxCancellation;
pub(crate) struct Translated {
    pub text: String,
    pub boundaries: Vec<usize>,
}
pub(crate) fn translate(raw: &str, cancel: &SyntaxCancellation) -> Option<Translated> {
    let bytes = raw.as_bytes();
    let mut at = 0;
    let mut escaped = false;
    let mut slashes = 0usize;
    let mut pieces = vec![];
    while at < bytes.len() {
        if cancel.is_cancelled() {
            return None;
        }
        let start = at;
        let c = raw[at..].chars().next().unwrap();
        let eligible = escaped || slashes.is_multiple_of(2);
        let mut marker = at + 1;
        if c == '\\' && eligible && bytes.get(marker) == Some(&b'u') {
            while bytes.get(marker) == Some(&b'u') {
                marker += 1;
            }
            let hex = bytes.get(marker..marker.saturating_add(4));
            if let Some(hex) = hex.filter(|s| s.iter().all(u8::is_ascii_hexdigit)) {
                let unit = hex
                    .iter()
                    .fold(0u32, |n, b| n * 16 + (*b as char).to_digit(16).unwrap());
                at = marker + 4;
                pieces.push((unit, start..at));
                escaped = true;
                slashes = if unit == b'\\' as u32 { slashes + 1 } else { 0 };
                continue;
            }
        }
        at += c.len_utf8();
        pieces.push((c as u32, start..at));
        escaped = false;
        slashes = if c == '\\' { slashes + 1 } else { 0 };
    }
    let mut text = String::new();
    let mut boundaries = vec![0];
    let mut i = 0;
    while i < pieces.len() {
        if cancel.is_cancelled() {
            return None;
        }
        let (mut unit, mut range) = pieces[i].clone();
        if (0xd800..=0xdbff).contains(&unit)
            && let Some((low, second)) = pieces.get(i + 1)
            && (0xdc00..=0xdfff).contains(low)
        {
            unit = 0x10000 + ((unit - 0xd800) << 10) + low - 0xdc00;
            range.end = second.end;
            i += 1;
        }
        // An isolated UTF-16 surrogate is retained as a replacement scalar. It
        // remains legal inside a UTF-16 literal, and is invalid as an identifier.
        let c = char::from_u32(unit).unwrap_or('\u{fffd}');
        text.push(c);
        boundaries.extend(std::iter::repeat_n(range.start, c.len_utf8() - 1));
        boundaries.push(range.end);
        i += 1;
    }
    Some(Translated { text, boundaries })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn eligible_backslashes_multiple_markers_and_surrogate_pairs() {
        for (raw, expected) in [
            (r"\u0061", "a"),
            (r"\uuuu0061", "a"),
            (r"\\u005a", r"\\u005a"),
            (r"\\\u005a", r"\\Z"),
            (r"\u005cu005a", r"\u005a"),
            (r"\u005c\u005a", r"\Z"),
            (r"\uD801\uDC00", "𐐀"),
        ] {
            let translated = translate(raw, &SyntaxCancellation::default()).unwrap();
            assert_eq!(translated.text, expected, "{raw}");
            assert_eq!(translated.boundaries.len(), translated.text.len() + 1);
            assert_eq!(translated.boundaries.last(), Some(&raw.len()));
        }
    }
}
