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
//!
//! What is an error and what is only a warning:
//! - **Errors** ([`JsonDoc::errors`], code `json-syntax`) are text that is
//!   not JSON. Consumers that need valid JSON (a remote context, an
//!   `rdf:JSON` literal) refuse a document with any.
//! - **Warnings** ([`JsonDoc::warnings`]) are valid JSON that is almost
//!   certainly not what its author meant, or that a reader may not take
//!   as written. They never make a document invalid:
//!   - `json-lone-surrogate`: a `\uD800`..`\uDFFF` escape with no partner.
//!     RFC 8259 allows it and other JSON-LD processors accept it, but a Rust
//!     string cannot hold it, so it is read as U+FFFD;
//!   - `json-duplicate-key`, at the *earlier* key of each pair: JSON-LD
//!     reads an object as a map, so only the last member of a key counts
//!     (see [`Object::get`]). Every earlier one is reported, so a key
//!     written three times has two warnings.
//!
//!   The number of warnings is capped at [`MAX_ERRORS`] too, without a
//!   message.
//! - A leading byte order mark is skipped by the parser and kept by the
//!   formatter ([`super::format`]).
//! - Past [`MAX_ERRORS`] errors, one more error says so. It is empty and
//!   sits at the end of the reported range (after the last error and the
//!   text read so far), so it sorts after every other error and points at
//!   where the checking stopped.

use crate::syntax::{Severity, Span, SyntaxError};
use std::borrow::Cow;

/// A parsed JSON document.
#[derive(Clone, Debug, Default)]
pub struct JsonDoc {
    /// The top-level value; `None` for an empty (or whitespace-only) text.
    pub root: Option<Json>,
    /// `json-syntax` errors, in text order. Text that is not JSON.
    pub errors: Vec<SyntaxError>,
    /// Warnings about valid JSON (`json-lone-surrogate`,
    /// `json-duplicate-key`), in text order; see the module doc.
    pub(crate) warnings: Vec<SyntaxError>,
}

/// A JSON value and where it is written.
#[derive(Clone, Debug, PartialEq)]
pub struct Json {
    pub value: Value,
    /// The whole value: a string with its quotes, an object or array from
    /// its opening bracket to its closing one (or to its last member when
    /// unclosed).
    pub(crate) span: Span,
    /// A string's text without its quotes (an unclosed string has only the
    /// opening one); any other value's span.
    pub(crate) inner: Span,
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
    pub(crate) open: Span,
    /// The `]`; `None` when the array is unclosed.
    pub(crate) close: Option<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// Members in text order, duplicates included.
    pub members: Vec<Member>,
    /// The `{`.
    pub(crate) open: Span,
    /// The `}`; `None` when the object is unclosed.
    pub(crate) close: Option<Span>,
}

/// A `"key": value` pair.
#[derive(Clone, Debug, PartialEq)]
pub struct Member {
    /// The key with its escapes decoded.
    pub key: Box<str>,
    /// The key with its quotes.
    pub(crate) key_span: Span,
    /// The key without its quotes, as [`Json::inner`].
    pub(crate) key_inner: Span,
    pub value: Json,
}

impl Object {
    /// The member for `key`; with duplicate keys, the last one (JSON-LD
    /// processors read objects as maps).
    pub(crate) fn get(&self, key: &str) -> Option<&Member> {
        self.members.iter().rev().find(|m| &*m.key == key)
    }

    /// The members as a map reads them, in text order: with duplicate keys,
    /// only the last one. Linear in the member count.
    pub(crate) fn entries(&self) -> Vec<&Member> {
        let last = last_per_key(&self.members);
        self.members
            .iter()
            .zip(last)
            .filter_map(|(m, last)| last.then_some(m))
            .collect()
    }
}

/// Per member of `members`, whether no later member has its key: the one a
/// map keeps. [`Object::entries`] keeps those, and the parser warns at the
/// others (`json-duplicate-key`). Linear in the member count.
fn last_per_key(members: &[Member]) -> Vec<bool> {
    if members.len() <= 16 {
        return members
            .iter()
            .enumerate()
            .map(|(i, m)| !members[i + 1..].iter().any(|later| later.key == m.key))
            .collect();
    }
    let mut last: rustc_hash::FxHashMap<&str, usize> = rustc_hash::FxHashMap::default();
    for (i, m) in members.iter().enumerate() {
        last.insert(&m.key, i);
    }
    members
        .iter()
        .enumerate()
        .map(|(i, m)| last[&*m.key] == i)
        .collect()
}

impl Json {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_object(&self) -> Option<&Object> {
        match &self.value {
            Value::Object(o) => Some(o),
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn as_array(&self) -> Option<&Array> {
        match &self.value {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

/// How deep objects and arrays may nest. A deeper value is reported and
/// skipped, so no input can overflow the stack.
pub(crate) const MAX_DEPTH: u32 = 256;

/// After this many errors the rest of the document is not checked (and no
/// more warnings are kept).
pub(crate) const MAX_ERRORS: usize = 100;

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
    lex(text).map(|l| l.token).collect()
}

/// A [`Token`] and whether it closed: a string with its quote, a block
/// comment with its `*/`. A line comment runs to the end of its line and
/// never closes; other tokens always do.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Lexeme {
    pub(crate) token: Token,
    pub(crate) closed: bool,
}

/// [`tokens`] one at a time, with whether each closed, for a scan that
/// stops early.
pub(crate) fn lex(text: &str) -> impl Iterator<Item = Lexeme> + '_ {
    let mut p = Parser::new(text, true);
    std::iter::from_fn(move || {
        let t = p.lex();
        let closed = match t.kind {
            K::Str { closed, .. } | K::Comment { closed } => closed,
            _ => true,
        };
        let kind = match t.kind {
            K::Eof => return None,
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
            K::Comment { .. } => TokenKind::Comment,
        };
        Some(Lexeme {
            token: Token {
                kind,
                span: t.span(),
            },
            closed,
        })
    })
}

/// Parse JSON text (RFC 8259) into a tree of spans, with `json-syntax`
/// errors for whatever is not JSON. A leading byte order mark is skipped.
pub fn parse(text: &str) -> JsonDoc {
    #[cfg(test)]
    parses::COUNT.with(|c| c.set(c.get() + 1));
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
                inner: span,
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
    let mut warnings = p.warnings;
    warnings.sort_by_key(|e| e.span.start);
    JsonDoc {
        root,
        errors,
        warnings,
    }
}

/// Whether `text` is JSON with no syntax errors: what an `rdf:JSON` literal must be.
pub fn valid(text: &str) -> bool {
    let doc = parse(text);
    doc.root.is_some() && doc.errors.is_empty()
}

/// How many times [`parse`] ran on this thread, for tests that check a
/// feature reads a text once.
#[cfg(test)]
pub(crate) mod parses {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    /// The parses `f` makes.
    pub(crate) fn during<R>(f: impl FnOnce() -> R) -> (R, usize) {
        let before = COUNT.with(Cell::get);
        let r = f();
        (r, COUNT.with(Cell::get) - before)
    }
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
    /// `closed`: a block comment that ends with its `*/` (a line comment
    /// runs to the end of its line and never closes).
    Comment {
        closed: bool,
    },
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

/// Where a string scanned by [`scan_string`] ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StringScan {
    /// After the closing quote; unclosed, the end of the line (or text).
    pub(crate) end: usize,
    pub(crate) closed: bool,
    /// Has a backslash.
    pub(crate) escaped: bool,
}

/// Scans the string whose opening `"` is at `open` in `b`, the way the
/// lexer does and the editor scans ([`string_end`]) must: a backslash takes
/// the next byte with it, except a line break, which still ends an unclosed
/// string. `control` is called with the offset of each raw control
/// character before the end of the string.
pub(crate) fn scan_string(b: &[u8], open: usize, mut control: impl FnMut(usize)) -> StringScan {
    let mut i = open + 1;
    let mut escaped = false;
    loop {
        while i < b.len() && !STRING_STOP[b[i] as usize] {
            i += 1;
        }
        match b.get(i) {
            Some(b'"') => {
                return StringScan {
                    end: i + 1,
                    closed: true,
                    escaped,
                };
            }
            Some(b'\\') => {
                escaped = true;
                i += 1;
                if i < b.len() && !matches!(b[i], b'\n' | b'\r') {
                    i += 1;
                }
            }
            Some(b'\n' | b'\r') | None => {
                return StringScan {
                    end: i,
                    closed: false,
                    escaped,
                };
            }
            Some(_) => {
                control(i);
                i += 1;
            }
        }
    }
}

/// For the closing quote of the string whose opening `"` is at `open`: its
/// offset, or (unclosed) the end of its line or of the text. The flag says
/// whether it closed. The same string [`tokens`] gives, for scans of text
/// that may be half typed.
pub(crate) fn string_end(b: &[u8], open: usize) -> (usize, bool) {
    let s = scan_string(b, open, |_| {});
    if s.closed {
        (s.end - 1, true)
    } else {
        (s.end, false)
    }
}

struct Parser<'a> {
    text: &'a str,
    b: &'a [u8],
    pos: usize,
    peeked: Option<Lexed>,
    errors: Vec<SyntaxError>,
    warnings: Vec<SyntaxError>,
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
            warnings: Vec::new(),
            quiet,
            stopped: false,
            depth: 0,
            open_arrays: 0,
            open_objects: 0,
        }
    }

    fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        if self.quiet || self.stopped {
            return;
        }
        let (span, message) = if self.errors.len() >= MAX_ERRORS {
            self.stopped = true;
            // At the end of the reported range: after every error so far and
            // after the text read, where the checking stops.
            let at = self
                .errors
                .iter()
                .map(|e| e.span.end)
                .chain([span.start, self.pos as u32])
                .max()
                .unwrap_or(span.start)
                .min(self.b.len() as u32);
            (
                Span::new(at, at),
                Cow::Owned(format!(
                    "More than {MAX_ERRORS} syntax errors; the rest of the file is not checked"
                )),
            )
        } else {
            (span, message.into())
        };
        self.errors.push(SyntaxError::new(
            span,
            message,
            Severity::Error,
            "json-syntax",
        ));
    }

    /// A warning: valid JSON that deserves a second look (see the module
    /// doc). Not counted with the errors; capped on its own.
    fn warn(&mut self, span: Span, code: &'static str, message: impl Into<Cow<'static, str>>) {
        if self.quiet || self.stopped || self.warnings.len() >= MAX_ERRORS {
            return;
        }
        self.warnings
            .push(SyntaxError::new(span, message, Severity::Warning, code));
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
            if !matches!(t.kind, K::Comment { .. }) {
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
        let mut controls: Vec<usize> = Vec::new();
        let s = scan_string(self.b, start, |i| controls.push(i));
        for i in controls {
            let at = Span::new(i as u32, i as u32 + 1);
            self.pos = s.end;
            self.error(
                at,
                "Control character in a string: write it as an escape (\\t, \\u0001)",
            );
        }
        self.pos = s.end;
        if !s.closed {
            self.error(
                Span::new(start as u32, s.end as u32),
                "Unclosed string: expected '\"' before the end of the line",
            );
        }
        Lexed {
            kind: K::Str {
                escaped: s.escaped,
                closed: s.closed,
            },
            start: start as u32,
            end: s.end as u32,
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
        let (end, closed) = if b[start + 1] == b'/' {
            let end = b[start..]
                .iter()
                .position(|&c| matches!(c, b'\n' | b'\r'))
                .map_or(b.len(), |n| start + n);
            (end, false)
        } else {
            b[start + 2..]
                .windows(2)
                .position(|w| w == b"*/")
                .map_or((b.len(), false), |n| (start + 2 + n + 2, true))
        };
        self.pos = end;
        let t = Lexed {
            kind: K::Comment { closed },
            start: start as u32,
            end: end as u32,
        };
        self.error(t.span(), "JSON has no comments");
        t
    }

    /// `t`'s text without its quotes: a string loses the opening quote and,
    /// when it closed, the closing one; anything else keeps its span.
    fn inner(t: Lexed) -> Span {
        match t.kind {
            K::Str { closed, .. } => Span::new(t.start + 1, t.end - u32::from(closed)),
            _ => t.span(),
        }
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
                self.warn(
                    span,
                    "json-lone-surrogate",
                    "Unpaired surrogate in a `\\u` escape; it is read as U+FFFD",
                );
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
            K::True | K::False | K::Null | K::Word | K::Comment { .. } => {
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
            K::RBrace | K::RBracket | K::Colon | K::Comma | K::Comment { .. } | K::Eof => {
                self.error(
                    t.span(),
                    format!("Expected a value, found {}", self.describe(t)),
                );
                let span = Span::new(t.start, t.start);
                return Json {
                    value: Value::Invalid,
                    span,
                    inner: span,
                };
            }
        };
        self.bump();
        Json {
            value,
            span: t.span(),
            inner: Self::inner(t),
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
            K::RBrace | K::RBracket | K::Colon | K::Comma | K::Comment { .. } | K::Eof => Json {
                value: Value::Invalid,
                span: Span::new(key.end, key.end),
                inner: Span::new(key.end, key.end),
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
        let span = Span::new(open.start, end);
        Json {
            value: Value::Invalid,
            span,
            inner: span,
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
                    Some((self.decode(t), t.span(), Self::inner(t)))
                }
                // An unquoted key (`a`, `1`, `true`): read as the key.
                K::Word | K::Num { .. } | K::True | K::False | K::Null => {
                    self.error(t.span(), "Object keys must be strings in double quotes");
                    self.bump();
                    Some((self.slice(t).into(), t.span(), t.span()))
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
            if let Some((key, key_span, key_inner)) = key {
                let value = self.member_value(key_span);
                end = value.span.end.max(key_span.end);
                members.push(Member {
                    key,
                    key_span,
                    key_inner,
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
        self.duplicate_keys(&members);
        Json {
            value: Value::Object(Object {
                members,
                open,
                close,
            }),
            span: Span::new(open.start, end),
            inner: Span::new(open.start, end),
        }
    }

    /// Warns at each key of `members` that a later member repeats.
    fn duplicate_keys(&mut self, members: &[Member]) {
        if self.quiet || self.stopped || members.len() < 2 {
            return;
        }
        for (m, last) in members.iter().zip(last_per_key(members)) {
            if !last {
                self.warn(
                    m.key_span,
                    "json-duplicate-key",
                    format!(
                        "Duplicate key \"{}\": only the last member with it counts",
                        short(&m.key)
                    ),
                );
            }
        }
    }

    /// A missing ',' is reported on the value before it, as Turtle marks the
    /// token before a missing '.'; at `next` when that value has no text. Not
    /// at all when the value already has an error there (an unclosed string
    /// swallows the comma).
    fn missing_separator(&mut self, prev: Option<Span>, next: Span, message: &'static str) {
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
            inner: Span::new(open.start, end),
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
mod tests;
