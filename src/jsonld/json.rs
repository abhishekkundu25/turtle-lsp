//! JSON with spans: the tree the JSON-LD stages read. Every value, object
//! key and bracket keeps its byte span, so each triple and each diagnostic
//! can point at the text that produced it.
//!
//! The parser is fault-tolerant: it reports `json-syntax` errors, recovers
//! at commas, colons and brackets, and still returns the values it could
//! read. A value it could not read is [`Value::Invalid`], so a key with a
//! broken value keeps its span.
//!
//! Recovery, in short:
//! - a missing `,` is reported at the token that should follow it, and that
//!   token starts the next member or item;
//! - a trailing `,` is reported at the comma; a missing value at the token
//!   found instead, with an empty [`Value::Invalid`] in its place;
//! - an unclosed `{` or `[` is reported at the bracket itself; a closing
//!   bracket of the wrong kind closes the inner container when an enclosing
//!   one matches it, and is skipped otherwise;
//! - an unclosed string ends at the end of its line;
//! - anything after the top-level value is one error.
//!
//! Nesting is limited to [`MAX_DEPTH`] (deeper values are skipped without
//! recursion), and a document stops being checked after [`MAX_ERRORS`]
//! errors. Spans are byte offsets, as everywhere in the crate.

use crate::syntax::{Severity, Span, SyntaxError};

/// A parsed JSON document.
#[derive(Clone, Debug, Default)]
pub struct JsonDoc {
    /// The top-level value; `None` for an empty (or whitespace-only) text.
    pub root: Option<Json>,
    /// `json-syntax` errors, in text order.
    pub errors: Vec<SyntaxError>,
}

/// A JSON value and where it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Json {
    pub value: Value,
    /// The whole value: a string with its quotes, an object or array from
    /// its opening bracket to its closing one (or to its last member when
    /// unclosed).
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number as written (`1`, `-0.5`, `1e3`): JSON-LD tells integers
    /// from doubles by the value, and the lexical form keeps every digit.
    Number(Box<str>),
    /// The string with its escapes decoded.
    String(Box<str>),
    Array(Array),
    Object(Object),
    /// What could not be read as a value (a missing value after `:`, a stray
    /// word); its span covers the skipped text and may be empty.
    Invalid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Array {
    pub items: Vec<Json>,
    /// The `[`.
    pub open: Span,
    /// The `]`; `None` when the array is unclosed.
    pub close: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// Members in text order, duplicates included.
    pub members: Vec<Member>,
    /// The `{`.
    pub open: Span,
    /// The `}`; `None` when the object is unclosed.
    pub close: Option<Span>,
}

/// A `"key": value` pair.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// The key with its escapes decoded.
    pub key: Box<str>,
    /// The key with its quotes.
    pub key_span: Span,
    pub value: Json,
}

impl Object {
    /// The member for `key`; with duplicate keys, the last one (JSON-LD
    /// processors read objects as maps).
    pub fn get(&self, key: &str) -> Option<&Member> {
        self.members.iter().rev().find(|m| &*m.key == key)
    }
}

impl Json {
    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Object> {
        match &self.value {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Array> {
        match &self.value {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// How deep objects and arrays may nest. A deeper value is reported and
/// skipped, so no input can overflow the stack.
pub const MAX_DEPTH: u32 = 256;

/// After this many errors the rest of the document is not checked.
pub const MAX_ERRORS: usize = 100;

/// What a [`Token`] is, for highlighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    /// A string, key or value, with its quotes.
    String,
    Number,
    /// `true`, `false` or `null`.
    Literal,
    /// A `//` or `/* */` comment (not JSON, but skipped as space).
    Comment,
    /// A stray word or a malformed number.
    Invalid,
}

/// A lexical token of a JSON text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// The tokens of a JSON text, in order, without whitespace. Never fails:
/// anything that is not JSON is a [`TokenKind::Invalid`] token.
pub fn tokens(text: &str) -> Vec<Token> {
    let mut p = Parser::new(text, true);
    let mut out = Vec::new();
    loop {
        let t = p.lex();
        let kind = match t.kind {
            K::Eof => return out,
            K::LBrace => TokenKind::LBrace,
            K::RBrace => TokenKind::RBrace,
            K::LBracket => TokenKind::LBracket,
            K::RBracket => TokenKind::RBracket,
            K::Colon => TokenKind::Colon,
            K::Comma => TokenKind::Comma,
            K::Str { .. } => TokenKind::String,
            K::Num { valid: true } => TokenKind::Number,
            K::Num { valid: false } | K::Word => TokenKind::Invalid,
            K::True | K::False | K::Null => TokenKind::Literal,
            K::Comment => TokenKind::Comment,
        };
        out.push(Token {
            kind,
            span: t.span(),
        });
    }
}

/// Parse JSON text (RFC 8259) into a tree of spans, with `json-syntax`
/// errors for whatever is not JSON. A leading byte order mark is skipped.
pub fn parse(text: &str) -> JsonDoc {
    let mut p = Parser::new(text, false);
    let t = p.peek();
    let root = match t.kind {
        K::Eof => None,
        K::RBrace | K::RBracket | K::Comma | K::Colon => {
            let span = Span::new(t.start, trimmed_end(text).max(t.end));
            p.error(
                span,
                format!("Expected a JSON value, found {}", p.describe(t)),
            );
            Some(Json {
                value: Value::Invalid,
                span,
            })
        }
        _ => {
            let root = p.value();
            let t = p.peek();
            if t.kind != K::Eof {
                let span = Span::new(t.start, trimmed_end(text).max(t.end));
                p.error(span, "Unexpected text after the top-level value");
            }
            Some(root)
        }
    };
    let mut errors = p.errors;
    errors.sort_by_key(|e| e.span.start);
    JsonDoc { root, errors }
}

/// The end of `text` without trailing JSON whitespace.
fn trimmed_end(text: &str) -> u32 {
    let b = text.as_bytes();
    let mut end = b.len();
    while end > 0 && is_space(b[end - 1]) {
        end -= 1;
    }
    end as u32
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

const fn byte_set(bytes: &[u8]) -> [bool; 256] {
    let mut set = [false; 256];
    let mut i = 0;
    while i < bytes.len() {
        set[bytes[i] as usize] = true;
        i += 1;
    }
    set
}

/// Bytes that end a bare word (a literal, a number or a stray word).
static WORD_END: [bool; 256] = byte_set(b" \t\n\r{}[],:\"");

/// Bytes the string scanner must look at: the closing quote, an escape, and
/// control characters (a raw newline ends an unclosed string).
static STRING_STOP: [bool; 256] = {
    let mut set = byte_set(b"\"\\");
    let mut c = 0;
    while c < 0x20 {
        set[c] = true;
        c += 1;
    }
    set
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum K {
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,
    /// `escaped`: has a backslash (decode it); `closed`: ends with its
    /// quote. A single-quoted string (an error) is a string too.
    Str {
        escaped: bool,
        closed: bool,
    },
    Num {
        valid: bool,
    },
    True,
    False,
    Null,
    Word,
    Comment,
    Eof,
}

#[derive(Clone, Copy, Debug)]
struct Lexed {
    kind: K,
    start: u32,
    end: u32,
}

impl Lexed {
    fn span(self) -> Span {
        Span::new(self.start, self.end)
    }
}

fn is_number(w: &[u8]) -> bool {
    let digits = |mut i: usize| {
        while w.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        i
    };
    let mut i = usize::from(w.first() == Some(&b'-'));
    match w.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => i = digits(i + 1),
        _ => return false,
    }
    if w.get(i) == Some(&b'.') {
        let from = i + 1;
        i = digits(from);
        if i == from {
            return false;
        }
    }
    if matches!(w.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(w.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let from = i;
        i = digits(from);
        if i == from {
            return false;
        }
    }
    i == w.len()
}

fn hex4(b: &[u8], at: usize, end: usize) -> Option<u32> {
    if at + 4 > end {
        return None;
    }
    let mut v = 0;
    for &c in &b[at..at + 4] {
        v = v * 16 + (c as char).to_digit(16)?;
    }
    Some(v)
}

struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
    pos: usize,
    peeked: Option<Lexed>,
    errors: Vec<SyntaxError>,
    /// Lexing for [`tokens`]: no errors.
    quiet: bool,
    /// [`MAX_ERRORS`] reached: the lexer reports the end of the text.
    stopped: bool,
    depth: u32,
    /// Enclosing arrays and objects, so a closing bracket of the wrong kind
    /// can close the inner container when an outer one matches it.
    open_arrays: u32,
    open_objects: u32,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, quiet: bool) -> Self {
        Parser {
            text,
            b: text.as_bytes(),
            pos: if text.starts_with('\u{feff}') { 3 } else { 0 },
            peeked: None,
            errors: Vec::new(),
            quiet,
            stopped: false,
            depth: 0,
            open_arrays: 0,
            open_objects: 0,
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        if self.quiet || self.stopped {
            return;
        }
        let message = if self.errors.len() >= MAX_ERRORS {
            self.stopped = true;
            format!("More than {MAX_ERRORS} syntax errors; the rest of the file is not checked")
        } else {
            message.into()
        };
        self.errors.push(SyntaxError {
            span,
            message,
            severity: Severity::Error,
            code: "json-syntax",
        });
    }

    fn eof(&self) -> Lexed {
        let n = self.b.len() as u32;
        Lexed {
            kind: K::Eof,
            start: n,
            end: n,
        }
    }

    // ---- lexing ----

    fn peek(&mut self) -> Lexed {
        if let Some(t) = self.peeked {
            return t;
        }
        let t = loop {
            if self.stopped {
                break self.eof();
            }
            let t = self.lex();
            if t.kind != K::Comment {
                break t;
            }
        };
        self.peeked = Some(t);
        t
    }

    fn bump(&mut self) {
        self.peeked = None;
    }

    fn slice(&self, t: Lexed) -> &'a str {
        &self.text[t.start as usize..t.end as usize]
    }

    fn lex(&mut self) -> Lexed {
        let b = self.b;
        let mut i = self.pos;
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        let start = i;
        if i >= b.len() {
            self.pos = i;
            return self.eof();
        }
        let kind = match b[i] {
            b'{' => K::LBrace,
            b'}' => K::RBrace,
            b'[' => K::LBracket,
            b']' => K::RBracket,
            b':' => K::Colon,
            b',' => K::Comma,
            b'"' => return self.string(start),
            b'/' if matches!(b.get(i + 1), Some(b'/' | b'*')) => return self.comment(start),
            b'\'' => match self.single_quoted(start) {
                Some(t) => return t,
                None => K::Word,
            },
            _ => K::Word,
        };
        i = if kind == K::Word {
            let mut j = i + 1;
            while j < b.len() && !WORD_END[b[j] as usize] {
                j += 1;
            }
            j
        } else {
            i + 1
        };
        self.pos = i;
        let t = Lexed {
            kind,
            start: start as u32,
            end: i as u32,
        };
        if kind != K::Word {
            return t;
        }
        let kind = match &b[start..i] {
            b"true" => K::True,
            b"false" => K::False,
            b"null" => K::Null,
            w if matches!(w[0], b'-' | b'+' | b'.' | b'0'..=b'9') => {
                let valid = is_number(w);
                if !valid {
                    self.error(
                        t.span(),
                        format!("Invalid number `{}`", short(self.slice(t))),
                    );
                }
                K::Num { valid }
            }
            _ => K::Word,
        };
        Lexed { kind, ..t }
    }

    /// A string from its opening quote. Unclosed, it ends at the end of its
    /// line.
    fn string(&mut self, start: usize) -> Lexed {
        let b = self.b;
        let mut i = start + 1;
        let mut escaped = false;
        loop {
            while i < b.len() && !STRING_STOP[b[i] as usize] {
                i += 1;
            }
            match b.get(i) {
                Some(b'"') => {
                    i += 1;
                    break;
                }
                Some(b'\\') => {
                    escaped = true;
                    i += 1;
                    if i < b.len() && !matches!(b[i], b'\n' | b'\r') {
                        i += 1;
                    }
                }
                Some(b'\n' | b'\r') | None => {
                    self.pos = i;
                    self.error(
                        Span::new(start as u32, i as u32),
                        "Unclosed string: expected '\"' before the end of the line",
                    );
                    return Lexed {
                        kind: K::Str {
                            escaped,
                            closed: false,
                        },
                        start: start as u32,
                        end: i as u32,
                    };
                }
                Some(_) => {
                    let at = Span::new(i as u32, i as u32 + 1);
                    self.error(
                        at,
                        "Control character in a string: write it as an escape (\\t, \\u0001)",
                    );
                    i += 1;
                }
            }
        }
        self.pos = i;
        Lexed {
            kind: K::Str {
                escaped,
                closed: true,
            },
            start: start as u32,
            end: i as u32,
        }
    }

    /// `'text'` on one line: an error, read as a string. `None` when there
    /// is no closing quote on the line (a stray word).
    fn single_quoted(&mut self, start: usize) -> Option<Lexed> {
        let b = self.b;
        let mut i = start + 1;
        let mut escaped = false;
        loop {
            match b.get(i)? {
                b'\'' => break,
                b'\n' | b'\r' => return None,
                b'\\' => {
                    if matches!(b.get(i + 1), None | Some(b'\n' | b'\r')) {
                        return None;
                    }
                    escaped = true;
                    i += 2;
                }
                _ => i += 1,
            }
        }
        i += 1;
        self.pos = i;
        let t = Lexed {
            kind: K::Str {
                escaped,
                closed: true,
            },
            start: start as u32,
            end: i as u32,
        };
        self.error(t.span(), "Strings use double quotes");
        Some(t)
    }

    fn comment(&mut self, start: usize) -> Lexed {
        let b = self.b;
        let end = if b[start + 1] == b'/' {
            b[start..]
                .iter()
                .position(|&c| matches!(c, b'\n' | b'\r'))
                .map_or(b.len(), |n| start + n)
        } else {
            b[start + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or(b.len(), |n| start + 2 + n + 2)
        };
        self.pos = end;
        let t = Lexed {
            kind: K::Comment,
            start: start as u32,
            end: end as u32,
        };
        self.error(t.span(), "JSON has no comments");
        t
    }

    /// The string `t` with its escapes decoded; invalid escapes are errors
    /// at the escape.
    fn decode(&mut self, t: Lexed) -> Box<str> {
        let K::Str { escaped, closed } = t.kind else {
            return self.slice(t).into();
        };
        let from = t.start as usize + 1;
        let to = t.end as usize - usize::from(closed);
        let (text, b) = (self.text, self.b);
        if !escaped {
            return text[from..to].into();
        }
        let mut out = String::with_capacity(to - from);
        let mut i = from;
        while let Some(n) = b[i..to].iter().position(|&c| c == b'\\') {
            let at = i + n;
            out.push_str(&text[i..at]);
            let Some(&c) = b[..to].get(at + 1) else {
                self.error(Span::new(at as u32, at as u32 + 1), "Incomplete escape");
                i = to;
                break;
            };
            i = at + 2;
            let simple = match c {
                b'"' => Some('"'),
                b'\\' => Some('\\'),
                b'/' => Some('/'),
                b'b' => Some('\u{8}'),
                b'f' => Some('\u{c}'),
                b'n' => Some('\n'),
                b'r' => Some('\r'),
                b't' => Some('\t'),
                _ => None,
            };
            if let Some(ch) = simple {
                out.push(ch);
            } else if c == b'u' {
                i = self.unicode_escape(at, to, &mut out);
            } else {
                let ch = text[at + 1..].chars().next().unwrap_or('\u{fffd}');
                i = at + 1 + ch.len_utf8();
                let span = Span::new(at as u32, i as u32);
                self.error(span, format!("Invalid escape `\\{ch}`"));
                out.push(ch);
            }
        }
        out.push_str(&text[i..to]);
        out.into()
    }

    /// The `\uXXXX` escape at `at` (a surrogate pair takes two), pushed to
    /// `out`; returns where the text continues.
    fn unicode_escape(&mut self, at: usize, to: usize, out: &mut String) -> usize {
        let b = self.b;
        let digits = at + 2;
        let Some(hi) = hex4(b, digits, to) else {
            let n = b[digits..to]
                .iter()
                .take(4)
                .take_while(|c| c.is_ascii_hexdigit())
                .count();
            let span = Span::new(at as u32, (digits + n) as u32);
            self.error(span, "Invalid `\\u` escape: expected 4 hex digits");
            out.push('\u{fffd}');
            return digits + n;
        };
        let next = digits + 4;
        let ch = match hi {
            0xD800..=0xDBFF => {
                let lo = (b.get(next..next + 2) == Some(b"\\u"))
                    .then(|| hex4(b, next + 2, to))
                    .flatten()
                    .filter(|lo| (0xDC00..=0xDFFF).contains(lo));
                if let Some(lo) = lo {
                    let c = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                    out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
                    return next + 6;
                }
                None
            }
            0xDC00..=0xDFFF => None,
            _ => char::from_u32(hi),
        };
        match ch {
            Some(c) => out.push(c),
            None => {
                let span = Span::new(at as u32, next as u32);
                self.error(span, "Unpaired surrogate in a `\\u` escape");
                out.push('\u{fffd}');
            }
        }
        next
    }

    fn describe(&self, t: Lexed) -> String {
        match t.kind {
            K::LBrace => "'{'".into(),
            K::RBrace => "'}'".into(),
            K::LBracket => "'['".into(),
            K::RBracket => "']'".into(),
            K::Colon => "':'".into(),
            K::Comma => "','".into(),
            K::Str { .. } => "a string".into(),
            K::Num { .. } => "a number".into(),
            K::True | K::False | K::Null | K::Word | K::Comment => {
                format!("`{}`", short(self.slice(t)))
            }
            K::Eof => "the end of the file".into(),
        }
    }

    // ---- values ----

    fn value(&mut self) -> Json {
        let t = self.peek();
        let value = match t.kind {
            K::LBrace => return self.object(t),
            K::LBracket => return self.array(t),
            K::Str { .. } => Value::String(self.decode(t)),
            K::Num { valid: true } => Value::Number(self.slice(t).into()),
            K::Num { valid: false } => Value::Invalid,
            K::True => Value::Bool(true),
            K::False => Value::Bool(false),
            K::Null => Value::Null,
            K::Word => {
                self.error(
                    t.span(),
                    format!("Expected a value, found {}", self.describe(t)),
                );
                Value::Invalid
            }
            K::RBrace | K::RBracket | K::Colon | K::Comma | K::Comment | K::Eof => {
                self.error(
                    t.span(),
                    format!("Expected a value, found {}", self.describe(t)),
                );
                return Json {
                    value: Value::Invalid,
                    span: Span::new(t.start, t.start),
                };
            }
        };
        self.bump();
        Json {
            value,
            span: t.span(),
        }
    }

    /// The value after a key: `: value`. A missing colon is reported, and a
    /// value after it is still read.
    fn member_value(&mut self, key: Span) -> Json {
        let t = self.peek();
        if t.kind == K::Colon {
            self.bump();
            return self.value();
        }
        let at = if t.kind == K::Eof {
            Span::new(key.end, key.end)
        } else {
            t.span()
        };
        self.error(
            at,
            format!("Expected ':' after the key, found {}", self.describe(t)),
        );
        match t.kind {
            K::RBrace | K::RBracket | K::Colon | K::Comma | K::Comment | K::Eof => Json {
                value: Value::Invalid,
                span: Span::new(key.end, key.end),
            },
            _ => self.value(),
        }
    }

    /// A value nested past [`MAX_DEPTH`], skipped by counting brackets.
    fn too_deep(&mut self, open: Lexed) -> Json {
        self.error(
            open.span(),
            format!("Nested more than {MAX_DEPTH} levels deep"),
        );
        let mut depth = 0u32;
        let mut end = open.end;
        loop {
            let t = self.peek();
            match t.kind {
                K::LBrace | K::LBracket => depth += 1,
                K::RBrace | K::RBracket => depth = depth.saturating_sub(1),
                K::Eof => break,
                _ => {}
            }
            self.bump();
            end = t.end;
            if depth == 0 {
                break;
            }
        }
        Json {
            value: Value::Invalid,
            span: Span::new(open.start, end),
        }
    }

    fn object(&mut self, open: Lexed) -> Json {
        if self.depth >= MAX_DEPTH {
            return self.too_deep(open);
        }
        self.bump();
        self.depth += 1;
        self.open_objects += 1;
        let open = open.span();
        let mut members = Vec::new();
        let mut close = None;
        let mut end = open.end;
        let mut comma: Option<Span> = None;
        loop {
            let t = self.peek();
            let key = match t.kind {
                K::RBrace => {
                    if let Some(c) = comma {
                        self.error(c, "Trailing comma before '}'");
                    }
                    self.bump();
                    close = Some(t.span());
                    end = t.end;
                    break;
                }
                K::Eof => {
                    self.error(open, "Unclosed object: expected '}'");
                    break;
                }
                K::RBracket if self.open_arrays > 0 => {
                    self.error(open, "Unclosed object: expected '}' before ']'");
                    break;
                }
                K::RBracket => {
                    self.error(t.span(), "Unexpected ']'");
                    self.bump();
                    continue;
                }
                K::Comma => {
                    self.error(t.span(), "Unexpected ','");
                    self.bump();
                    comma = Some(t.span());
                    continue;
                }
                K::Str { .. } => {
                    self.bump();
                    Some((self.decode(t), t.span()))
                }
                // An unquoted key (`a`, `1`, `true`): read as the key.
                K::Word | K::Num { .. } | K::True | K::False | K::Null => {
                    self.error(t.span(), "Object keys must be strings in double quotes");
                    self.bump();
                    Some((self.slice(t).into(), t.span()))
                }
                K::Colon => {
                    self.error(t.span(), "Expected a key before ':'");
                    self.bump();
                    end = self.value().span.end.max(t.end);
                    None
                }
                // `{` or `[`: read and dropped, with a value after it.
                _ => {
                    let msg = format!(
                        "Expected a key in double quotes, found {}",
                        self.describe(t)
                    );
                    self.error(t.span(), msg);
                    end = self.value().span.end;
                    if self.peek().kind == K::Colon {
                        self.bump();
                        end = self.value().span.end;
                    }
                    None
                }
            };
            comma = None;
            if let Some((key, key_span)) = key {
                let value = self.member_value(key_span);
                end = value.span.end.max(key_span.end);
                members.push(Member {
                    key,
                    key_span,
                    value,
                });
            }
            // What follows a member: a comma, the end, or a recovery.
            loop {
                let t = self.peek();
                match t.kind {
                    K::Comma => {
                        self.bump();
                        comma = Some(t.span());
                        break;
                    }
                    K::RBrace | K::RBracket | K::Eof => break,
                    K::Str { .. } | K::Word => {
                        // On the value before the missing ',', as Turtle marks
                        // the token before a missing '.'.
                        let prev = members.last().map(|m| m.value.span);
                        self.missing_separator(prev, t.span(), "Expected ',' between members");
                        break;
                    }
                    K::Colon => {
                        self.error(t.span(), "Unexpected ':'");
                        self.bump();
                    }
                    _ => {
                        let msg = format!("Expected ',' or '}}', found {}", self.describe(t));
                        self.error(t.span(), msg);
                        end = self.value().span.end;
                    }
                }
            }
        }
        self.depth -= 1;
        self.open_objects -= 1;
        Json {
            value: Value::Object(Object {
                members,
                open,
                close,
            }),
            span: Span::new(open.start, end),
        }
    }

    /// A missing ',' is reported on the value before it, as Turtle marks the
    /// token before a missing '.'; at `next` when that value has no text. Not
    /// at all when the value already has an error there (an unclosed string
    /// swallows the comma).
    fn missing_separator(&mut self, prev: Option<Span>, next: Span, message: &str) {
        let at = prev.filter(|s| s.end > s.start).unwrap_or(next);
        if self.errors.iter().rev().any(|e| e.span == at) {
            return;
        }
        self.error(at, message);
    }

    fn array(&mut self, open: Lexed) -> Json {
        if self.depth >= MAX_DEPTH {
            return self.too_deep(open);
        }
        self.bump();
        self.depth += 1;
        self.open_arrays += 1;
        let open = open.span();
        let mut items = Vec::new();
        let mut close = None;
        let mut end = open.end;
        let mut comma: Option<Span> = None;
        loop {
            let t = self.peek();
            match t.kind {
                K::RBracket => {
                    if let Some(c) = comma {
                        self.error(c, "Trailing comma before ']'");
                    }
                    self.bump();
                    close = Some(t.span());
                    end = t.end;
                    break;
                }
                K::Eof => {
                    self.error(open, "Unclosed array: expected ']'");
                    break;
                }
                K::RBrace if self.open_objects > 0 => {
                    self.error(open, "Unclosed array: expected ']' before '}'");
                    break;
                }
                K::RBrace => {
                    self.error(t.span(), "Unexpected '}'");
                    self.bump();
                    continue;
                }
                K::Comma => {
                    self.error(t.span(), "Expected a value before ','");
                    self.bump();
                    comma = Some(t.span());
                    continue;
                }
                K::Colon => {
                    self.error(t.span(), "Unexpected ':'");
                    self.bump();
                    continue;
                }
                _ => {
                    let item = self.value();
                    end = item.span.end;
                    items.push(item);
                }
            }
            comma = None;
            loop {
                let t = self.peek();
                match t.kind {
                    K::Comma => {
                        self.bump();
                        comma = Some(t.span());
                        break;
                    }
                    K::RBracket | K::RBrace | K::Eof => break,
                    K::Colon => {
                        self.error(t.span(), "Unexpected ':'");
                        self.bump();
                    }
                    _ => {
                        let prev = items.last().map(|i| i.span);
                        self.missing_separator(prev, t.span(), "Expected ',' between items");
                        break;
                    }
                }
            }
        }
        self.depth -= 1;
        self.open_arrays -= 1;
        Json {
            value: Value::Array(Array { items, open, close }),
            span: Span::new(open.start, end),
        }
    }
}

/// `s`, cut to 40 characters for a message.
fn short(s: &str) -> String {
    match s.char_indices().nth(40) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_duplicate_key_wins() {
        let member = |key: &str, at: u32, v: bool| Member {
            key: key.into(),
            key_span: Span::new(at, at + 3),
            value: Json {
                value: Value::Bool(v),
                span: Span::new(at + 5, at + 9),
            },
        };
        let o = Object {
            members: vec![
                member("a", 1, false),
                member("b", 12, true),
                member("a", 23, true),
            ],
            open: Span::new(0, 1),
            close: Some(Span::new(33, 34)),
        };
        assert_eq!(o.get("a").map(|m| m.key_span.start), Some(23));
        assert!(o.get("c").is_none());
    }

    /// The errors as `(start, end, message)`.
    fn errors(text: &str) -> Vec<(u32, u32, String)> {
        parse(text)
            .errors
            .into_iter()
            .map(|e| (e.span.start, e.span.end, e.message))
            .collect()
    }

    /// The span of the `n`th (from 0) occurrence of `needle` in `text`.
    fn at(text: &str, needle: &str, n: usize) -> (u32, u32) {
        let start = text.match_indices(needle).nth(n).expect(needle).0 as u32;
        (start, start + needle.len() as u32)
    }

    fn one_error(text: &str, needle: &str, n: usize, message: &str) {
        let (s, e) = at(text, needle, n);
        assert_eq!(errors(text), vec![(s, e, message.to_string())], "{text}");
    }

    fn root(text: &str) -> Json {
        parse(text).root.expect("a root value")
    }

    fn span(s: (u32, u32)) -> Span {
        Span::new(s.0, s.1)
    }

    #[test]
    fn a_valid_document_keeps_every_span() {
        let text = r#"{
  "@context": {"ex": "http://example.org/"},
  "@id": "ex:a",
  "ex:n": [1, -0.5e+10, 1E3, 0, true, false, null],
  "ex:n": {}
}"#;
        let doc = parse(text);
        assert!(doc.errors.is_empty(), "{:?}", doc.errors);
        let root = doc.root.unwrap();
        assert_eq!(root.span, Span::new(0, text.len() as u32));
        let o = root.as_object().unwrap();
        assert_eq!(o.open, Span::new(0, 1));
        assert_eq!(o.close, Some(span(at(text, "}", 2))));
        let keys: Vec<&str> = o.members.iter().map(|m| &*m.key).collect();
        assert_eq!(
            keys,
            ["@context", "@id", "ex:n", "ex:n"],
            "duplicates kept in order"
        );
        let id = &o.members[1];
        assert_eq!(id.key_span, span(at(text, "\"@id\"", 0)));
        assert_eq!(id.value.span, span(at(text, "\"ex:a\"", 0)));
        assert_eq!(id.value.as_str(), Some("ex:a"));
        let ctx = o.members[0].value.as_object().unwrap();
        assert_eq!(ctx.open, span(at(text, "{", 1)));
        assert_eq!(ctx.close, Some(span(at(text, "}", 0))));
        assert_eq!(
            ctx.get("ex").unwrap().value.as_str(),
            Some("http://example.org/")
        );
        let list = o.members[2].value.as_array().unwrap();
        assert_eq!(list.open, span(at(text, "[", 0)));
        assert_eq!(list.close, Some(span(at(text, "]", 0))));
        let values: Vec<Value> = list.items.iter().map(|j| j.value.clone()).collect();
        assert_eq!(
            values,
            [
                Value::Number("1".into()),
                Value::Number("-0.5e+10".into()),
                Value::Number("1E3".into()),
                Value::Number("0".into()),
                Value::Bool(true),
                Value::Bool(false),
                Value::Null,
            ]
        );
        assert_eq!(list.items[1].span, span(at(text, "-0.5e+10", 0)));
        assert_eq!(list.items[6].span, span(at(text, "null", 0)));
        assert_eq!(o.get("ex:n").unwrap().value.span, span(at(text, "{}", 0)));
    }

    #[test]
    fn strings_decode_escapes_and_unicode() {
        let text = r#"["caf\u00e9 é", "\ud83d\ude00 😀", "\"\\\/\b\f\n\r\t", "", "日本"]"#;
        let doc = parse(text);
        assert!(doc.errors.is_empty(), "{:?}", doc.errors);
        let root = doc.root.unwrap();
        let items: Vec<&str> = root
            .as_array()
            .unwrap()
            .items
            .iter()
            .map(|j| j.as_str().unwrap())
            .collect();
        assert_eq!(
            items,
            ["café é", "😀 😀", "\"\\/\u{8}\u{c}\n\r\t", "", "日本"]
        );
        // Spans are bytes, past multi-byte characters.
        let last = &root.as_array().unwrap().items[4];
        assert_eq!(last.span, span(at(text, "\"日本\"", 0)));
    }

    #[test]
    fn empty_text_has_no_root_and_no_error() {
        for text in ["", "  \n\t ", "\u{feff}"] {
            let doc = parse(text);
            assert!(doc.root.is_none() && doc.errors.is_empty(), "{text:?}");
        }
        let doc = parse("\u{feff}{}");
        assert!(doc.errors.is_empty());
        assert_eq!(doc.root.unwrap().span, Span::new(3, 5));
    }

    #[test]
    fn scalars_at_the_top_level() {
        assert_eq!(root("\"a\"").value, Value::String("a".into()));
        assert_eq!(root(" 42 ").span, Span::new(1, 3));
        assert!(errors(" 42 ").is_empty());
    }

    #[test]
    fn invalid_numbers_are_errors_and_invalid_values() {
        for n in ["01", "1.", ".5", "+1", "-", "1e", "1.5e+", "0x10", "12abc"] {
            let text = format!("[{n}, 2]");
            one_error(&text, n, 0, &format!("Invalid number `{n}`"));
            let r = root(&text);
            let items = &r.as_array().unwrap().items;
            assert_eq!(items[0].value, Value::Invalid);
            assert_eq!(
                items[1].value,
                Value::Number("2".into()),
                "the rest is read"
            );
        }
    }

    #[test]
    fn invalid_escapes_are_errors_at_the_escape() {
        let text = r#"["a\xb", "c"]"#;
        one_error(text, r"\x", 0, "Invalid escape `\\x`");
        assert_eq!(
            root(text).as_array().unwrap().items[0].as_str(),
            Some("axb")
        );

        let text = r#"["\u12G4"]"#;
        one_error(
            text,
            r"\u12",
            0,
            "Invalid `\\u` escape: expected 4 hex digits",
        );
        assert_eq!(
            root(text).as_array().unwrap().items[0].as_str(),
            Some("\u{fffd}G4")
        );

        let text = r#"["\u00"]"#;
        one_error(
            text,
            r"\u00",
            0,
            "Invalid `\\u` escape: expected 4 hex digits",
        );

        let text = r#"["a\ud83dz"]"#;
        one_error(text, r"\ud83d", 0, "Unpaired surrogate in a `\\u` escape");
        assert_eq!(
            root(text).as_array().unwrap().items[0].as_str(),
            Some("a\u{fffd}z")
        );

        let text = r#"["\ude00", "\ud83d\u0041"]"#;
        let (lo, hi) = (at(text, r"\ude00", 0), at(text, r"\ud83d", 0));
        let msg = "Unpaired surrogate in a `\\u` escape".to_string();
        assert_eq!(
            errors(text),
            vec![(lo.0, lo.1, msg.clone()), (hi.0, hi.1, msg)]
        );
        assert_eq!(
            root(text).as_array().unwrap().items[1].as_str(),
            Some("\u{fffd}A")
        );

        let text = "[\"\\é\"]";
        one_error(text, "\\é", 0, "Invalid escape `\\é`");
    }

    #[test]
    fn control_characters_in_strings() {
        let text = "[\"a\tb\"]";
        one_error(
            text,
            "\t",
            0,
            "Control character in a string: write it as an escape (\\t, \\u0001)",
        );
        assert_eq!(
            root(text).as_array().unwrap().items[0].as_str(),
            Some("a\tb")
        );
    }

    #[test]
    fn a_missing_comma_between_members() {
        let text = r#"{"a": 1 "b": 2, "c": [3 4]}"#;
        let (one, three) = (at(text, "1", 0), at(text, "3", 0));
        assert_eq!(
            errors(text),
            vec![
                (one.0, one.1, "Expected ',' between members".into()),
                (three.0, three.1, "Expected ',' between items".into()),
            ]
        );
        let r = root(text);
        let o = r.as_object().unwrap();
        assert_eq!(o.members.len(), 3, "every member is read");
        assert_eq!(o.get("c").unwrap().value.as_array().unwrap().items.len(), 2);
        assert!(o.close.is_some());
    }

    #[test]
    fn a_trailing_comma() {
        one_error(r#"{"a": 1,}"#, ",", 0, "Trailing comma before '}'");
        one_error("[1, 2, ]", ",", 1, "Trailing comma before ']'");
        assert_eq!(root("[1, 2, ]").as_array().unwrap().items.len(), 2);
    }

    #[test]
    fn extra_commas() {
        one_error("[1,, 2]", ",", 1, "Expected a value before ','");
        one_error("[, 2]", ",", 0, "Expected a value before ','");
        one_error(r#"{, "a": 1}"#, ",", 0, "Unexpected ','");
        assert_eq!(root("[1,, 2]").as_array().unwrap().items.len(), 2);
    }

    #[test]
    fn a_missing_value() {
        let text = r#"{"a": , "b": 1}"#;
        one_error(text, ",", 0, "Expected a value, found ','");
        let r = root(text);
        let o = r.as_object().unwrap();
        let a = &o.get("a").unwrap().value;
        let comma = at(text, ",", 0).0;
        assert_eq!(
            (a.value.clone(), a.span),
            (Value::Invalid, Span::new(comma, comma))
        );
        assert_eq!(o.get("b").unwrap().value.value, Value::Number("1".into()));

        one_error(r#"{"a": }"#, "}", 0, "Expected a value, found '}'");
        let text = r#"{"a":"#;
        let end = text.len() as u32;
        assert_eq!(
            errors(text),
            vec![
                (0, 1, "Unclosed object: expected '}'".into()),
                (
                    end,
                    end,
                    "Expected a value, found the end of the file".into()
                ),
            ]
        );
    }

    #[test]
    fn a_missing_colon() {
        let text = r#"{"a" 1, "b": 2}"#;
        one_error(text, "1", 0, "Expected ':' after the key, found a number");
        let r = root(text);
        let o = r.as_object().unwrap();
        assert_eq!(o.get("a").unwrap().value.value, Value::Number("1".into()));
        assert_eq!(o.members.len(), 2);

        let text = r#"{"a", "b": 2}"#;
        one_error(text, ",", 0, "Expected ':' after the key, found ','");
        let r = root(text);
        let a = &r.as_object().unwrap().members[0];
        assert_eq!(a.value.value, Value::Invalid);
        assert_eq!(a.value.span, Span::new(a.key_span.end, a.key_span.end));
    }

    #[test]
    fn an_unclosed_string_ends_at_its_line() {
        let text = "{\n  \"a\": \"oops,\n  \"b\": 2\n}";
        let s = at(text, "\"oops,", 0);
        assert_eq!(
            errors(text),
            vec![(
                s.0,
                s.1,
                "Unclosed string: expected '\"' before the end of the line".into()
            )]
        );
        let r = root(text);
        let o = r.as_object().unwrap();
        assert_eq!(o.get("a").unwrap().value.as_str(), Some("oops,"));
        assert_eq!(o.get("b").unwrap().value.value, Value::Number("2".into()));
        assert!(o.close.is_some());

        assert_eq!(
            errors("[\"abc"),
            vec![
                (0, 1, "Unclosed array: expected ']'".into()),
                (
                    1,
                    5,
                    "Unclosed string: expected '\"' before the end of the line".into()
                ),
            ]
        );
    }

    #[test]
    fn unclosed_objects_and_arrays_are_reported_at_their_bracket() {
        let text = r#"{"a": [1, 2"#;
        let (o, a) = (at(text, "{", 0), at(text, "[", 0));
        assert_eq!(
            errors(text),
            vec![
                (o.0, o.1, "Unclosed object: expected '}'".into()),
                (a.0, a.1, "Unclosed array: expected ']'".into()),
            ]
        );
        let r = root(text);
        assert_eq!(
            r.span,
            Span::new(0, text.len() as u32),
            "up to its last member"
        );
        let obj = r.as_object().unwrap();
        assert_eq!(obj.close, None);
        let list = obj.get("a").unwrap().value.as_array().unwrap();
        assert_eq!((list.items.len(), list.close), (2, None));

        // A closing bracket of an enclosing container closes the inner one.
        let text = r#"[{"a": 1]"#;
        one_error(text, "{", 0, "Unclosed object: expected '}' before ']'");
        let r = root(text);
        let list = r.as_array().unwrap();
        assert_eq!(list.close, Some(span(at(text, "]", 0))));
        assert_eq!(list.items[0].span, Span::new(1, at(text, "1", 0).1));
        let text = r#"{"a": [1}"#;
        one_error(text, "[", 0, "Unclosed array: expected ']' before '}'");
        assert!(root(text).as_object().unwrap().close.is_some());
    }

    #[test]
    fn stray_tokens_are_skipped() {
        let text = r#"{"a": 1]}"#;
        one_error(text, "]", 0, "Unexpected ']'");
        assert!(root(text).as_object().unwrap().close.is_some());
        let text = "[1}]";
        one_error(text, "}", 0, "Unexpected '}'");
        assert_eq!(root(text).as_array().unwrap().items.len(), 1);
        one_error(r#"{"a": 1:, "b": 2}"#, ":", 1, "Unexpected ':'");
        one_error(r#"{"a": 1 [2]}"#, "[", 0, "Expected ',' or '}', found '['");
        one_error(
            r#"{"a": 1, 2: 3}"#,
            "2",
            0,
            "Object keys must be strings in double quotes",
        );
        one_error(
            r#"{"a": 1, [2]: 3}"#,
            "[",
            0,
            "Expected a key in double quotes, found '['",
        );
        let text = r#"{"a": tru, b: 1}"#;
        let (v, k) = (at(text, "tru", 0), at(text, "b", 0));
        assert_eq!(
            errors(text),
            vec![
                (v.0, v.1, "Expected a value, found `tru`".into()),
                (
                    k.0,
                    k.1,
                    "Object keys must be strings in double quotes".into()
                ),
            ]
        );
        let r = root(text);
        let o = r.as_object().unwrap();
        assert_eq!(o.get("a").unwrap().value.span, span(v));
        assert_eq!(o.get("b").unwrap().value.value, Value::Number("1".into()));
    }

    #[test]
    fn text_after_the_root_is_one_error() {
        let text = "{} xyz {\"a\": }\n\n";
        one_error(
            text,
            "xyz {\"a\": }",
            0,
            "Unexpected text after the top-level value",
        );
        one_error("[]]", "]", 1, "Unexpected text after the top-level value");
        let text = "} {}";
        one_error(text, "} {}", 0, "Expected a JSON value, found '}'");
        assert_eq!(root(text).value, Value::Invalid);
    }

    #[test]
    fn comments_and_single_quotes_are_errors_but_read() {
        let text = "{\n  // note\n  'a': /* x */ 1\n}";
        let (c1, q, c2) = (
            at(text, "// note", 0),
            at(text, "'a'", 0),
            at(text, "/* x */", 0),
        );
        assert_eq!(
            errors(text),
            vec![
                (c1.0, c1.1, "JSON has no comments".into()),
                (q.0, q.1, "Strings use double quotes".into()),
                (c2.0, c2.1, "JSON has no comments".into()),
            ]
        );
        let r = root(text);
        assert_eq!(
            r.as_object().unwrap().get("a").unwrap().value.value,
            Value::Number("1".into())
        );
    }

    #[test]
    fn later_errors_are_still_reported() {
        let text = "{\n  \"a\": 1,,\n  \"b\": [1 2],\n  \"c\": {\"d\" 4},\n  \"e\": 5,\n}";
        let messages: Vec<String> = errors(text).into_iter().map(|e| e.2).collect();
        assert_eq!(
            messages,
            [
                "Unexpected ','",
                "Expected ',' between items",
                "Expected ':' after the key, found a number",
                "Trailing comma before '}'",
            ]
        );
        let r = root(text);
        let keys: Vec<&str> = r
            .as_object()
            .unwrap()
            .members
            .iter()
            .map(|m| &*m.key)
            .collect();
        assert_eq!(keys, ["a", "b", "c", "e"]);
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_stack_overflow() {
        let ok = format!(
            "{}{}",
            "[".repeat(MAX_DEPTH as usize),
            "]".repeat(MAX_DEPTH as usize)
        );
        assert!(errors(&ok).is_empty());

        let n = MAX_DEPTH as usize + 1;
        let deep = format!("{}{}", "[".repeat(n), "]".repeat(n));
        let d = MAX_DEPTH;
        assert_eq!(
            errors(&deep),
            vec![(
                d,
                d + 1,
                format!("Nested more than {MAX_DEPTH} levels deep")
            )]
        );
        let r = root(&deep);
        assert_eq!(r.span, Span::new(0, deep.len() as u32));

        // Unclosed and far deeper: one error per unclosed level up to the
        // limit, capped, and no recursion past the limit.
        let text = "{\"a\":".repeat(200_000);
        let errs = parse(&text).errors;
        assert_eq!(errs.len(), MAX_ERRORS + 1);
    }

    #[test]
    fn errors_are_capped() {
        let text = format!("[{}1]", ",".repeat(500));
        let errs = parse(&text).errors;
        assert_eq!(errs.len(), MAX_ERRORS + 1);
        assert!(
            errs[MAX_ERRORS]
                .message
                .starts_with("More than 100 syntax errors")
        );
    }

    #[test]
    fn tokens_for_highlighting() {
        let text = "{\"a\": [1, true, x]} // c";
        let kinds: Vec<(TokenKind, &str)> = tokens(text)
            .into_iter()
            .map(|t| (t.kind, &text[t.span.start as usize..t.span.end as usize]))
            .collect();
        use TokenKind::*;
        assert_eq!(
            kinds,
            [
                (LBrace, "{"),
                (String, "\"a\""),
                (Colon, ":"),
                (LBracket, "["),
                (Number, "1"),
                (Comma, ","),
                (Literal, "true"),
                (Comma, ","),
                (Invalid, "x"),
                (RBracket, "]"),
                (RBrace, "}"),
                (Comment, "// c"),
            ]
        );
    }

    /// The benchmark for the parser, run with `cargo test --release jsonld`:
    /// a 30,000-line JSON-LD file against a 30,000-line Turtle file with the
    /// same data. Prints both times; fails when JSON is over twice as slow.
    #[cfg(not(debug_assertions))]
    #[test]
    fn parses_30000_lines_about_as_fast_as_turtle() {
        use std::time::{Duration, Instant};
        let mut json = String::from(
            "{\n  \"@context\": {\"ex\": \"http://example.org/\"},\n  \"@graph\": [\n",
        );
        let mut ttl = String::from("@prefix ex: <http://example.org/> .\n");
        let items = 30_000 / 7;
        for i in 0..items {
            let sep = if i + 1 < items { "," } else { "" };
            json.push_str(&format!(
                "    {{\n      \"@id\": \"ex:item{i}\",\n      \"@type\": \"ex:Thing\",\n      \"ex:name\": \"Item {i} \\u00e9 caf\u{e9}\",\n      \"ex:value\": {i}.5,\n      \"ex:tags\": [\"a\", \"b\"]\n    }}{sep}\n"
            ));
            ttl.push_str(&format!(
                "ex:item{i}\n  a ex:Thing ;\n  ex:name \"Item {i} \\u00e9 caf\u{e9}\" ;\n  ex:value {i}.5 ;\n  ex:tags \"a\" ,\n    \"b\" .\n\n"
            ));
        }
        json.push_str("  ]\n}\n");
        let best = |f: &dyn Fn()| {
            (0..7)
                .map(|_| {
                    let t = Instant::now();
                    f();
                    t.elapsed()
                })
                .min()
                .unwrap_or(Duration::ZERO)
        };
        let doc = parse(&json);
        assert!(doc.errors.is_empty(), "{:?}", &doc.errors[..1]);
        let prefixes = crate::syntax::PrefixMap::default();
        let p = crate::syntax::parse(&ttl, &prefixes, crate::syntax::Syntax::Turtle);
        assert!(p.errors.is_empty());
        let j = best(&|| drop(parse(&json)));
        let t = best(&|| {
            drop(crate::syntax::parse(
                &ttl,
                &prefixes,
                crate::syntax::Syntax::Turtle,
            ))
        });
        eprintln!(
            "json::parse {} lines ({} KB): {j:?}; Turtle {} lines ({} KB): {t:?}",
            json.lines().count(),
            json.len() / 1024,
            ttl.lines().count(),
            ttl.len() / 1024,
        );
        assert!(
            j <= t * 2 + Duration::from_millis(2),
            "JSON {j:?} vs Turtle {t:?}"
        );
    }
}
