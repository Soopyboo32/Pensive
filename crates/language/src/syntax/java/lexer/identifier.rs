//! Java 25 / Unicode 16.0 identifiers, with source spelling kept separately.
#[path = "identifier_tables.rs"]
mod identifier_tables;

fn contains(ranges: &[(u32, u32)], point: u32) -> bool {
    let next = ranges.partition_point(|&(start, _)| start <= point);
    next > 0 && point <= ranges[next - 1].1
}

pub fn identifier_start(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphabetic() || matches!(c, '_' | '$')
    } else {
        contains(identifier_tables::START, c as u32)
    }
}

pub fn identifier_part(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '$') || identifier_ignorable(c)
    } else {
        contains(identifier_tables::PART, c as u32)
    }
}

pub fn identifier_ignorable(c: char) -> bool {
    if c.is_ascii() {
        matches!(c, '\u{0}'..='\u{8}' | '\u{e}'..='\u{1b}' | '\u{7f}')
    } else {
        contains(identifier_tables::IGNORABLE, c as u32)
    }
}
