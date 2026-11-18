//! Lossless Java lexical tokens. Trivia and invalid input retain source ranges.
use super::SyntaxCancellation;
use std::ops::Range;
mod identifier;
pub(super) use identifier::{identifier_ignorable, identifier_part, identifier_start};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JavaTokenKind {
    Whitespace,
    Comment,
    Identifier,
    Keyword,
    Number,
    String,
    Character,
    Symbol,
    Error,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JavaToken {
    pub kind: JavaTokenKind,
    pub bytes: Range<usize>,
}
impl JavaToken {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.bytes.clone()]
    }
    pub fn is_trivia(&self) -> bool {
        matches!(
            self.kind,
            JavaTokenKind::Whitespace | JavaTokenKind::Comment
        )
    }
}

fn whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\u{c}' | '\n' | '\r')
}
pub(crate) fn keyword(s: &str) -> bool {
    matches!(
        s,
        "abstract"
            | "assert"
            | "boolean"
            | "break"
            | "byte"
            | "case"
            | "catch"
            | "char"
            | "class"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extends"
            | "final"
            | "finally"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "implements"
            | "import"
            | "instanceof"
            | "int"
            | "interface"
            | "long"
            | "native"
            | "new"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "short"
            | "static"
            | "strictfp"
            | "super"
            | "switch"
            | "synchronized"
            | "this"
            | "throw"
            | "throws"
            | "transient"
            | "try"
            | "void"
            | "volatile"
            | "while"
            | "true"
            | "false"
            | "null"
            | "_"
    )
}

/// A resumable scanner: even one huge comment/string yields every 4096 scalars.
#[derive(Clone, Default)]
pub struct Lexer {
    pub tokens: Vec<JavaToken>,
    at: usize,
    start: usize,
    mode: Mode,
}
#[derive(Clone, Default)]
enum Mode {
    #[default]
    Start,
    Space,
    Ident(KeywordCandidate),
    Number,
    LineComment,
    BlockComment,
    Quoted {
        quote: char,
        block: bool,
        escaped: bool,
    },
}

// Java keywords are ASCII and at most 12 bytes ("synchronized"). Accumulate
// their semantic spelling while scanning, so a huge identifier never needs an
// unbudgeted normalization pass just to decide its token kind.
#[derive(Clone, Default)]
struct KeywordCandidate {
    bytes: [u8; 12],
    len: usize,
}
impl KeywordCandidate {
    fn push(&mut self, c: char) {
        if identifier_ignorable(c) || self.len > self.bytes.len() {
            return;
        }
        if !c.is_ascii() || self.len == self.bytes.len() {
            self.len = self.bytes.len() + 1;
            return;
        }
        self.bytes[self.len] = c as u8;
        self.len += 1;
    }
    fn kind(&self) -> JavaTokenKind {
        if self.len <= self.bytes.len()
            && keyword(std::str::from_utf8(&self.bytes[..self.len]).unwrap())
        {
            JavaTokenKind::Keyword
        } else {
            JavaTokenKind::Identifier
        }
    }
}
impl Lexer {
    pub fn restart(tokens: Vec<JavaToken>, at: usize) -> Self {
        Self {
            tokens,
            at,
            start: at,
            mode: Mode::Start,
        }
    }
    fn emit(&mut self, kind: JavaTokenKind) {
        self.tokens.push(JavaToken {
            kind,
            bytes: self.start..self.at,
        });
        self.start = self.at;
        self.mode = Mode::Start;
    }
    pub fn advance(&mut self, source: &str, budget: usize, cancel: &SyntaxCancellation) -> bool {
        let mut work = 0usize;
        loop {
            if cancel.is_cancelled() || work >= budget {
                return false;
            }
            let rest = &source[self.at..];
            let Some(c) = rest.chars().next() else {
                match &self.mode {
                    Mode::Start => return true,
                    Mode::Space => self.emit(JavaTokenKind::Whitespace),
                    Mode::Ident(candidate) => self.emit(candidate.kind()),
                    Mode::Number => self.emit(JavaTokenKind::Number),
                    Mode::LineComment => self.emit(JavaTokenKind::Comment),
                    _ => self.emit(JavaTokenKind::Error),
                }
                return true;
            };
            work += 1;
            match &mut self.mode {
                Mode::Start => {
                    self.start = self.at;
                    if whitespace(c) {
                        self.mode = Mode::Space;
                    } else if identifier_start(c) {
                        let mut candidate = KeywordCandidate::default();
                        candidate.push(c);
                        self.mode = Mode::Ident(candidate);
                    } else if c.is_ascii_digit()
                        || c == '.' && rest[1..].starts_with(|ch: char| ch.is_ascii_digit())
                    {
                        self.mode = Mode::Number;
                    } else if rest.starts_with("//") {
                        self.at += 2;
                        self.mode = Mode::LineComment;
                        continue;
                    } else if rest.starts_with("/*") {
                        self.at += 2;
                        self.mode = Mode::BlockComment;
                        continue;
                    } else if c == '"' || c == '\'' {
                        let block = rest.starts_with("\"\"\"");
                        self.mode = Mode::Quoted {
                            quote: c,
                            block,
                            escaped: false,
                        };
                        self.at += if block { 3 } else { 1 };
                        continue;
                    } else {
                        // Separate angle brackets also handle nested generic types.
                        let op = [
                            "...", "::", "->", "==", "!=", "&&", "||", "++", "--", "+=", "-=",
                            "*=", "/=", "%=", "&=", "|=", "^=",
                        ]
                        .into_iter()
                        .find(|op| rest.starts_with(op));
                        self.at += op.map_or(c.len_utf8(), str::len);
                        self.emit(if "{}()[];,.@?:~!+-*/%&|^=<>".contains(c) {
                            JavaTokenKind::Symbol
                        } else {
                            JavaTokenKind::Error
                        });
                        continue;
                    }
                }
                Mode::Space if !whitespace(c) => {
                    self.emit(JavaTokenKind::Whitespace);
                    continue;
                }
                Mode::Ident(candidate) if !identifier_part(c) => {
                    let kind = candidate.kind();
                    self.emit(kind);
                    continue;
                }
                Mode::Ident(candidate) => candidate.push(c),
                Mode::Number => {
                    let previous = source[..self.at].chars().next_back().unwrap_or(' ');
                    let hex = source[self.start..].starts_with("0x")
                        || source[self.start..].starts_with("0X");
                    if !(c.is_ascii_alphanumeric()
                        || c == '_'
                        || c == '.'
                        || matches!(c, '+' | '-')
                            && (matches!(previous, 'p' | 'P')
                                || !hex && matches!(previous, 'e' | 'E')))
                    {
                        self.emit(JavaTokenKind::Number);
                        continue;
                    }
                }
                Mode::LineComment if matches!(c, '\n' | '\r') => {
                    self.emit(JavaTokenKind::Comment);
                    continue;
                }
                Mode::BlockComment if rest.starts_with("*/") => {
                    self.at += 2;
                    self.emit(JavaTokenKind::Comment);
                    continue;
                }
                Mode::Quoted {
                    quote,
                    block,
                    escaped,
                } => {
                    if !*escaped
                        && ((*block && rest.starts_with("\"\"\"")) || (!*block && c == *quote))
                    {
                        let kind = if *quote == '\'' {
                            JavaTokenKind::Character
                        } else {
                            JavaTokenKind::String
                        };
                        self.at += if *block { 3 } else { 1 };
                        self.emit(kind);
                        continue;
                    }
                    if !*block && matches!(c, '\n' | '\r') {
                        self.emit(JavaTokenKind::Error);
                        continue;
                    }
                    *escaped = c == '\\' && !*escaped;
                }
                _ => {}
            }
            self.at += c.len_utf8();
        }
    }
}
