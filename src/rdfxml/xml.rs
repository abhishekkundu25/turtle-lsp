//! A fault-tolerant XML scanner that keeps the span of every name,
//! attribute value, text and entity reference, with namespace scopes.
//!
//! [`Scanner::next`] gives one [`Event`] at a time: a start tag with its
//! attributes, an end tag, character data or a processing instruction.
//! Comments are kept as spans ([`Scanner::finish`]); the XML declaration
//! and the DOCTYPE are read inside. Every name comes resolved against the
//! namespace declarations in scope (`xmlns`, `xmlns:p`), and every text
//! and attribute value comes with its references expanded: the five
//! predefined entities, character references and the internal entities of
//! the DOCTYPE, in either quote style.
//!
//! Errors are `xml-syntax`, with the span of what is wrong. The scanner
//! reads on after each one:
//! - a `<` that starts no tag is text;
//! - a broken start or end tag ends at its `>`, or before the next `<`;
//!   an attribute value with a `<` in it ends there, and so does its tag;
//! - an end tag that names an open element further out closes the ones
//!   inside it ([`End::implied`]); one that names no open element is
//!   skipped;
//! - at the end of the text, every open element is closed the same way;
//! - text outside the root element is dropped.
//!
//! What well-formed XML forbids and `oxrdfxml` (through quick-xml) reads
//! anyway is reported and read the way it reads it: a second root element,
//! a duplicate attribute. Line ends and attribute values are not
//! normalized, as quick-xml does not normalize them.
//!
//! Limits: elements nest at most [`MAX_DEPTH`] deep (deeper ones are
//! skipped, with one error); the DOCTYPE declares at most [`MAX_ENTITIES`]
//! entities whose expanded values total at most [`MAX_ENTITY_BYTES`];
//! expanding entity references stops after [`EXPANSION_PER_BYTE`] bytes
//! per byte of text (plus [`MAX_ENTITY_BYTES`]); and at most [`MAX_ERRORS`]
//! errors are reported. Every step consumes at least one byte, so a scan
//! ends. Nesting is what bounds triple terms too: each level of one takes
//! two elements, so they nest at most [`MAX_DEPTH`] / 2 deep.

use crate::syntax::{Severity, Span, SyntaxError};
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::rc::Rc;

/// Elements nest at most this deep; a deeper element is skipped with its
/// content.
pub(crate) const MAX_DEPTH: usize = 256;
/// Entity declarations read from a DOCTYPE.
pub(crate) const MAX_ENTITIES: usize = 256;
/// The expanded values of all declared entities, together.
pub(crate) const MAX_ENTITY_BYTES: usize = 64 * 1024;
/// Entity references expand to at most this many bytes per byte of text.
pub(crate) const EXPANSION_PER_BYTE: usize = 16;
/// Errors reported per document, scanner and grammar together.
pub(crate) const MAX_ERRORS: usize = 100;

pub(crate) const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NS: &str = "http://www.w3.org/2000/xmlns/";

/// The namespace a name is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Ns {
    Bound(Rc<str>),
    /// No prefix and no default namespace (always the case for an
    /// attribute without a prefix).
    Unbound,
    /// A prefix with no declaration in scope.
    Unknown,
}

/// An element or attribute name as written, resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Name<'a> {
    pub(crate) raw: &'a str,
    pub(crate) span: Span,
    /// The byte length of the prefix of `p:local`; `None` without a colon.
    pub(crate) prefix_len: Option<u32>,
    pub(crate) ns: Ns,
}

impl<'a> Name<'a> {
    pub(crate) fn prefix(&self) -> Option<&'a str> {
        self.prefix_len.map(|n| &self.raw[..n as usize])
    }

    pub(crate) fn local(&self) -> &'a str {
        self.prefix_len
            .map_or(self.raw, |n| &self.raw[n as usize + 1..])
    }

    pub(crate) fn ns(&self) -> Option<&str> {
        match &self.ns {
            Ns::Bound(ns) => Some(ns),
            _ => None,
        }
    }

    /// Namespace and local name: `ns == n && local == l`.
    pub(crate) fn is(&self, n: &str, l: &str) -> bool {
        self.ns() == Some(n) && self.local() == l
    }
}

/// Which references a text or attribute value used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Refs {
    None,
    /// Predefined entities (`&amp;`) and character references only.
    Chars,
    /// At least one entity the DOCTYPE declares.
    Entity,
}

/// An attribute value, its references expanded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Value<'a> {
    /// Inside the quotes.
    pub(crate) span: Span,
    pub(crate) text: Cow<'a, str>,
    pub(crate) refs: Refs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Attr<'a> {
    pub(crate) name: Name<'a>,
    pub(crate) value: Value<'a>,
    /// From the name to the closing quote.
    pub(crate) span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Tag<'a> {
    pub(crate) name: Name<'a>,
    pub(crate) attrs: Vec<Attr<'a>>,
    /// From `<` to `>` (or to where a broken tag stops).
    pub(crate) span: Span,
    /// `<a/>`: an [`Event::End`] follows at once.
    pub(crate) empty: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct End {
    /// The end tag; for an empty element, its tag; for an implied end, an
    /// empty span where the element stops.
    pub(crate) span: Span,
    /// Closed by an error (an end tag further out, or the end of the text),
    /// not by its own end tag.
    pub(crate) implied: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event<'a> {
    Start(Tag<'a>),
    End(End),
    /// Character data between tags, or a CDATA section, references
    /// expanded. `span` is the raw text (a CDATA section's content).
    Text {
        span: Span,
        text: Cow<'a, str>,
    },
    /// A processing instruction inside an element, from `<?` to `?>`.
    Pi(Span),
    Eof,
}

struct Open<'a> {
    raw: &'a str,
    name_span: Span,
    /// `ns.len()` before this element's declarations.
    ns_len: usize,
}

pub(crate) struct Scanner<'a> {
    text: &'a str,
    b: &'a [u8],
    pos: usize,
    stack: Vec<Open<'a>>,
    /// The prefixes declared by the open elements, in order (`""` for the
    /// default namespace), and the namespaces each prefix is bound to,
    /// innermost last: lookups and undoing take constant time.
    ns: Vec<&'a str>,
    scopes: FxHashMap<&'a str, Vec<Rc<str>>>,
    xml_ns: Rc<str>,
    xmlns_ns: Rc<str>,
    pending: VecDeque<Event<'a>>,
    entities: FxHashMap<&'a str, Box<str>>,
    entity_bytes: usize,
    expanded: usize,
    budget: usize,
    over_budget: bool,
    /// Inside an element deeper than [`MAX_DEPTH`]: its nesting count.
    skip: usize,
    roots: usize,
    done: bool,
    errors: Vec<SyntaxError>,
    dropped: bool,
    comments: Vec<Span>,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

/// Bytes that end a name.
fn ends_name(c: u8) -> bool {
    is_space(c) || matches!(c, b'>' | b'/' | b'<' | b'=' | b'"' | b'\'' | b'?')
}

pub(crate) fn is_name_start_char(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

pub(crate) fn is_name_char(c: char) -> bool {
    is_name_start_char(c)
        || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// An XML `Name`.
pub(crate) fn is_name(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(is_name_start_char) && c.all(is_name_char)
}

/// An XML `NCName`: a name without a colon.
pub(crate) fn is_nc_name(s: &str) -> bool {
    is_name(s) && !s.contains(':')
}

/// A character XML allows in a document.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

pub(crate) fn is_whitespace(s: &str) -> bool {
    s.bytes().all(is_space)
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(text: &'a str) -> Scanner<'a> {
        let pos = if text.starts_with('\u{FEFF}') { 3 } else { 0 };
        Scanner {
            text,
            b: text.as_bytes(),
            pos,
            stack: Vec::new(),
            ns: Vec::new(),
            scopes: FxHashMap::default(),
            xml_ns: Rc::from(XML_NS),
            xmlns_ns: Rc::from(XMLNS_NS),
            pending: VecDeque::new(),
            entities: FxHashMap::default(),
            entity_bytes: 0,
            expanded: 0,
            budget: text.len().saturating_mul(EXPANSION_PER_BYTE) + MAX_ENTITY_BYTES,
            over_budget: false,
            skip: 0,
            roots: 0,
            done: false,
            errors: Vec::new(),
            dropped: false,
            comments: Vec::new(),
        }
    }

    /// The errors and the comment spans, once the scan is over.
    pub(crate) fn finish(self) -> (Vec<SyntaxError>, Vec<Span>) {
        (self.errors, self.comments)
    }

    /// Report an error, unless [`MAX_ERRORS`] have been.
    pub(crate) fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(SyntaxError::new(
                span,
                message,
                Severity::Error,
                "xml-syntax",
            ));
        } else {
            self.dropped = true;
        }
    }

    fn span(start: usize, end: usize) -> Span {
        Span::new(start as u32, end as u32)
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn skip_space(&mut self) -> bool {
        let start = self.pos;
        while self.peek().is_some_and(is_space) {
            self.pos += 1;
        }
        self.pos > start
    }

    /// Step over one character (at least one byte).
    fn bump_char(&mut self) {
        let n = self.text[self.pos..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
        self.pos += n;
    }

    fn find(&self, from: usize, needle: &[u8]) -> Option<usize> {
        let hay = &self.b[from.min(self.b.len())..];
        if needle.len() == 1 {
            return hay.iter().position(|&c| c == needle[0]).map(|i| from + i);
        }
        hay.windows(needle.len())
            .position(|w| w == needle)
            .map(|i| from + i)
    }

    fn read_name(&mut self) -> (&'a str, Span) {
        let start = self.pos;
        while self.peek().is_some_and(|c| !ends_name(c)) {
            self.pos += 1;
        }
        (&self.text[start..self.pos], Self::span(start, self.pos))
    }

    /// The next event. After [`Event::Eof`], always [`Event::Eof`].
    pub(crate) fn next(&mut self) -> Event<'a> {
        loop {
            if let Some(e) = self.pending.pop_front() {
                return e;
            }
            if self.done {
                return Event::Eof;
            }
            if self.pos >= self.b.len() {
                self.close_all();
                self.done = true;
                continue;
            }
            if let Some(e) = self.step() {
                return e;
            }
        }
    }

    /// At the end of the text: every open element is unclosed.
    fn close_all(&mut self) {
        let end = Self::span(self.b.len(), self.b.len());
        self.skip = 0;
        while let Some(open) = self.stack.pop() {
            self.error(open.name_span, format!("<{}> is not closed", open.raw));
            self.unscope(open.ns_len);
            self.pending.push_back(Event::End(End {
                span: end,
                implied: true,
            }));
        }
    }

    /// Read one construct; `Some` when it gives an event.
    fn step(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        if self.b[start] != b'<' {
            let end = self.find(start + 1, b"<").unwrap_or(self.b.len());
            self.pos = end;
            return self.text_event(start, end);
        }
        match self.b.get(start + 1) {
            Some(b'/') => self.end_tag(),
            Some(b'!') => self.bang(),
            Some(b'?') => self.pi(),
            _ if self.text[start + 1..]
                .chars()
                .next()
                .is_some_and(is_name_start_char) =>
            {
                self.start_tag()
            }
            _ => {
                self.error(Self::span(start, start + 1), "`<` starts no tag");
                let end = self.find(start + 1, b"<").unwrap_or(self.b.len());
                self.pos = end;
                self.text_event(start, end)
            }
        }
    }

    fn text_event(&mut self, start: usize, end: usize) -> Option<Event<'a>> {
        if self.skip > 0 {
            return None;
        }
        let raw = &self.text[start..end];
        if self.stack.is_empty() {
            if !is_whitespace(raw) {
                let lead = raw.len() - raw.trim_start().len();
                self.error(
                    Self::span(start + lead, end),
                    "text outside the root element",
                );
            }
            return None;
        }
        let (text, _) = self.decode(start, end);
        Some(Event::Text {
            span: Self::span(start, end),
            text,
        })
    }

    fn bang(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        let rest = &self.b[start..];
        if rest.starts_with(b"<!--") {
            let end = match self.find(start + 4, b"-->") {
                Some(e) => e + 3,
                None => {
                    self.error(Self::span(start, start + 4), "the comment is not closed");
                    self.b.len()
                }
            };
            self.comments.push(Self::span(start, end));
            self.pos = end;
            None
        } else if rest.starts_with(b"<![CDATA[") {
            let from = start + 9;
            let (to, end) = match self.find(from, b"]]>") {
                Some(e) => (e, e + 3),
                None => {
                    self.error(Self::span(start, from), "the CDATA section is not closed");
                    (self.b.len(), self.b.len())
                }
            };
            self.pos = end;
            if self.skip > 0 {
                return None;
            }
            let raw = &self.text[from..to];
            if self.stack.is_empty() {
                if !is_whitespace(raw) {
                    self.error(Self::span(start, end), "text outside the root element");
                }
                return None;
            }
            Some(Event::Text {
                span: Self::span(from, to),
                text: Cow::Borrowed(raw),
            })
        } else if rest.starts_with(b"<!DOCTYPE") {
            self.doctype();
            None
        } else {
            self.error(Self::span(start, start + 2), "unknown markup after `<!`");
            self.pos = start + 2;
            self.skip_decl();
            None
        }
    }

    /// Skip to the end of a declaration: past its `>` outside quotes, or
    /// to the next `<`.
    fn skip_decl(&mut self) {
        let mut quote = None;
        while let Some(c) = self.peek() {
            match (quote, c) {
                (Some(q), _) if q == c => quote = None,
                (Some(_), _) => {}
                (None, b'"' | b'\'') => quote = Some(c),
                (None, b'>') => {
                    self.pos += 1;
                    return;
                }
                (None, b'<') => return,
                _ => {}
            }
            self.pos += 1;
        }
    }

    fn pi(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        let end = match self.find(start + 2, b"?>") {
            Some(e) => e + 2,
            None => {
                self.error(
                    Self::span(start, start + 2),
                    "the processing instruction is not closed",
                );
                self.b.len()
            }
        };
        self.pos = end;
        let body = &self.text[start + 2..end.max(start + 2)];
        let target_len = body.bytes().position(ends_name).unwrap_or(body.len());
        let target = &body[..target_len];
        if target == "xml" {
            self.declaration(start, body);
            return None;
        }
        if target.eq_ignore_ascii_case("xml") || !is_name(target) {
            self.error(
                Self::span(start + 2, start + 2 + target_len),
                "not a processing instruction target",
            );
        }
        (self.skip == 0 && !self.stack.is_empty()).then(|| Event::Pi(Self::span(start, end)))
    }

    /// `<?xml version="1.0" encoding="..."?>`: only UTF-8 is read.
    fn declaration(&mut self, start: usize, body: &str) {
        if start != 0 && start != 3 {
            self.error(
                Self::span(start, start + 5),
                "the XML declaration is not at the start of the document",
            );
        }
        if let Some(i) = body.find("encoding") {
            let rest = body[i + 8..].trim_start();
            let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
            let q = rest.chars().next();
            if let Some(q @ ('"' | '\'')) = q {
                let value = rest[1..].split(q).next().unwrap_or("");
                if !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "utf-8"
                        | "utf8"
                        | "unicode-1-1-utf-8"
                        | "unicode11utf8"
                        | "unicode20utf8"
                        | "x-unicode20utf8"
                ) {
                    let at = start + 2 + i;
                    self.error(
                        Self::span(at, at + 8),
                        "only UTF-8 is read: the text is read as UTF-8",
                    );
                }
            }
        }
    }

    /// `<!DOCTYPE name ExternalID? [ internal subset ]? >`; reads the
    /// internal entity declarations.
    fn doctype(&mut self) {
        let start = self.pos;
        self.pos += 9;
        if !self.stack.is_empty() || self.roots > 0 {
            self.error(
                Self::span(start, start + 9),
                "a DOCTYPE after the root element starts",
            );
        }
        self.skip_space();
        let (name, name_span) = self.read_name();
        if name.is_empty() {
            self.error(Self::span(start, start + 9), "the DOCTYPE has no name");
        }
        let _ = name_span;
        self.skip_space();
        if self.b[self.pos..].starts_with(b"SYSTEM") || self.b[self.pos..].starts_with(b"PUBLIC") {
            let public = self.b[self.pos..].starts_with(b"PUBLIC");
            self.pos += 6;
            for _ in 0..if public { 2 } else { 1 } {
                self.skip_space();
                self.quoted();
            }
            self.skip_space();
        }
        if self.peek() == Some(b'[') {
            self.pos += 1;
            self.internal_subset();
        }
        self.skip_space();
        match self.peek() {
            Some(b'>') => self.pos += 1,
            _ => {
                self.error(
                    Self::span(start, start + 9),
                    "the DOCTYPE does not end with `>`",
                );
                self.skip_decl();
            }
        }
    }

    /// A quoted literal: its content's range, or `None` (reported) when the
    /// next byte is no quote or the quote is not closed.
    fn quoted(&mut self) -> Option<(usize, usize)> {
        let q = self.peek().filter(|c| matches!(c, b'"' | b'\''))?;
        let from = self.pos + 1;
        match self.find(from, &[q]) {
            Some(to) => {
                self.pos = to + 1;
                Some((from, to))
            }
            None => {
                self.error(Self::span(self.pos, from), "the quote is not closed");
                self.pos = self.b.len();
                None
            }
        }
    }

    fn internal_subset(&mut self) {
        loop {
            self.skip_space();
            let at = self.pos;
            let rest = &self.b[at..];
            if rest.is_empty() {
                self.error(Self::span(at, at), "the DOCTYPE is not closed");
                return;
            } else if rest[0] == b']' {
                self.pos += 1;
                return;
            } else if rest.starts_with(b"<!ENTITY") {
                self.entity_decl();
            } else if rest.starts_with(b"<!--") {
                match self.find(at + 4, b"-->") {
                    Some(e) => {
                        self.comments.push(Self::span(at, e + 3));
                        self.pos = e + 3;
                    }
                    None => {
                        self.error(Self::span(at, at + 4), "the comment is not closed");
                        self.pos = self.b.len();
                    }
                }
            } else if rest.starts_with(b"<?") {
                match self.find(at + 2, b"?>") {
                    Some(e) => self.pos = e + 2,
                    None => {
                        self.error(
                            Self::span(at, at + 2),
                            "the processing instruction is not closed",
                        );
                        self.pos = self.b.len();
                    }
                }
            } else if rest.starts_with(b"<!") {
                // ELEMENT, ATTLIST, NOTATION: not read.
                self.pos += 2;
                self.skip_decl();
            } else if rest[0] == b'%' {
                // A parameter entity reference: not read.
                let end = self.b.len().min(at + 256);
                let semi = self.b[at + 1..end]
                    .iter()
                    .position(|&c| c == b';' || is_space(c) || c == b'<')
                    .map(|p| at + 1 + p)
                    .filter(|&p| self.b[p] == b';');
                match semi {
                    Some(e) => {
                        self.pos = e + 1;
                    }
                    _ => {
                        self.error(Self::span(at, at + 1), "a `%` that starts no reference");
                        self.pos += 1;
                    }
                }
            } else {
                self.bump_char();
                self.error(Self::span(at, self.pos), "unexpected text in the DOCTYPE");
                // Up to the next declaration or the end of the subset.
                while self.peek().is_some_and(|c| !matches!(c, b'<' | b']')) {
                    self.pos += 1;
                }
            }
        }
    }

    /// `<!ENTITY name "value">` (or `'value'`). Parameter entities are
    /// skipped, external entities reported.
    fn entity_decl(&mut self) {
        let start = self.pos;
        self.pos += 8;
        let spaced = self.skip_space();
        let parameter = self.peek() == Some(b'%');
        if parameter {
            self.pos += 1;
            self.skip_space();
        }
        let (name, name_span) = self.read_name();
        if !spaced || !is_nc_name(name) {
            self.error(
                if name.is_empty() {
                    Self::span(start, start + 8)
                } else {
                    name_span
                },
                "the entity declaration has no valid name",
            );
        }
        self.skip_space();
        let value = match self.peek() {
            Some(b'"' | b'\'') => self.quoted(),
            _ => {
                if !parameter {
                    self.error(
                        Self::span(start, start + 8),
                        "external entities are not read",
                    );
                }
                self.skip_decl();
                return;
            }
        };
        self.skip_space();
        if self.peek() == Some(b'>') {
            self.pos += 1;
        } else {
            self.error(
                Self::span(start, start + 8),
                "the entity declaration does not end with `>`",
            );
            self.skip_decl();
        }
        let Some((from, to)) = value else { return };
        if parameter || name.is_empty() {
            return;
        }
        if self.entities.contains_key(name) {
            // The first declaration binds (XML 1.0, 4.2).
            return;
        }
        if self.entities.len() >= MAX_ENTITIES {
            self.error(
                name_span,
                format!("more than {MAX_ENTITIES} entities are declared: this one is not read"),
            );
            return;
        }
        let (value, _) = self.decode(from, to);
        if value.contains('%') && self.text[from..to].contains('%') {
            // A parameter entity reference in a value: not allowed in the
            // internal subset; kept as written.
            self.error(
                Self::span(from, to),
                "a parameter entity reference in an entity value",
            );
        }
        if self.entity_bytes + value.len() > MAX_ENTITY_BYTES {
            self.error(
                name_span,
                format!(
                    "the declared entities are over {} KiB: this one is not read",
                    MAX_ENTITY_BYTES / 1024
                ),
            );
            return;
        }
        self.entity_bytes += value.len();
        self.entities.insert(name, value.into());
    }

    /// The text of `start..end` with its references expanded. Errors are
    /// reported at each bad reference, which is kept as written.
    pub(crate) fn decode(&mut self, start: usize, end: usize) -> (Cow<'a, str>, Refs) {
        let raw = &self.text[start..end];
        if !raw.contains('&') {
            return (Cow::Borrowed(raw), Refs::None);
        }
        let mut out = String::with_capacity(raw.len());
        let mut refs = Refs::None;
        let mut i = start;
        while i < end {
            let Some(amp) = self.b[i..end]
                .iter()
                .position(|&c| c == b'&')
                .map(|p| i + p)
            else {
                out.push_str(&self.text[i..end]);
                break;
            };
            out.push_str(&self.text[i..amp]);
            // The reference ends at `;`, before any space, `&` or `<`.
            let semi = self.b[amp + 1..end]
                .iter()
                .take(256)
                .position(|&c| c == b';' || is_space(c) || c == b'&' || c == b'<')
                .map(|p| amp + 1 + p)
                .filter(|&p| self.b[p] == b';');
            let Some(semi) = semi else {
                self.error(
                    Self::span(amp, amp + 1),
                    "`&` starts no reference (write `&amp;`)",
                );
                out.push('&');
                i = amp + 1;
                continue;
            };
            let name = &self.text[amp + 1..semi];
            let span = Self::span(amp, semi + 1);
            i = semi + 1;
            if let Some(num) = name.strip_prefix('#') {
                let c = match num.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse::<u32>().ok(),
                }
                .filter(|_| !num.starts_with(['+', '-']) && !num.starts_with("x+"))
                .and_then(char::from_u32)
                .filter(|&c| is_xml_char(c));
                match c {
                    Some(c) => {
                        out.push(c);
                        refs = refs.max(Refs::Chars);
                    }
                    None => {
                        self.error(span, "not a character reference to an XML character");
                        out.push_str(&self.text[amp..semi + 1]);
                    }
                }
                continue;
            }
            let predefined = match name {
                "lt" => Some('<'),
                "gt" => Some('>'),
                "amp" => Some('&'),
                "apos" => Some('\''),
                "quot" => Some('"'),
                _ => None,
            };
            if let Some(c) = predefined {
                out.push(c);
                refs = refs.max(Refs::Chars);
                continue;
            }
            let Some(value) = self.entities.get(name) else {
                self.error(span, format!("the entity &{name}; is not declared"));
                out.push_str(&self.text[amp..semi + 1]);
                continue;
            };
            if self.over_budget || self.expanded + value.len() > self.budget {
                if !self.over_budget {
                    self.over_budget = true;
                    self.error(
                        span,
                        "entity references expand to too much text: the rest are not expanded",
                    );
                }
                out.push_str(&self.text[amp..semi + 1]);
                continue;
            }
            self.expanded += value.len();
            out.push_str(value);
            refs = Refs::Entity;
        }
        (Cow::Owned(out), refs)
    }

    fn start_tag(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        self.pos += 1;
        let (raw, name_span) = self.read_name();
        if !is_name(raw) {
            self.error(name_span, "not an XML name");
        }
        let mut attrs: Vec<(&'a str, Span, Value<'a>, Span)> = Vec::new();
        let mut seen: rustc_hash::FxHashSet<&'a str> = Default::default();
        let mut empty = false;
        loop {
            let spaced = self.skip_space();
            let at = self.pos;
            match self.peek() {
                None => {
                    self.error(Self::span(start, at), "the tag is not closed");
                    break;
                }
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(b'/') if self.b.get(at + 1) == Some(&b'>') => {
                    self.pos += 2;
                    empty = true;
                    break;
                }
                Some(b'<') => {
                    self.error(Self::span(start, at), "the tag is not closed");
                    break;
                }
                Some(_) => {}
            }
            let (aname, aspan) = self.read_name();
            if aname.is_empty() {
                self.bump_char();
                self.error(Self::span(at, self.pos), "unexpected character in a tag");
                continue;
            }
            if !spaced {
                self.error(aspan, "no space before the attribute");
            }
            if !is_name(aname) {
                self.error(aspan, "not an XML name");
            }
            self.skip_space();
            if self.peek() != Some(b'=') {
                self.error(aspan, "the attribute has no value");
                continue;
            }
            self.pos += 1;
            self.skip_space();
            let Some(q) = self.peek().filter(|c| matches!(c, b'"' | b'\'')) else {
                self.error(aspan, "the attribute value is not quoted");
                // Skip the unquoted word.
                while self
                    .peek()
                    .is_some_and(|c| !is_space(c) && c != b'>' && c != b'<')
                {
                    self.pos += 1;
                }
                continue;
            };
            let from = self.pos + 1;
            let stop = self.b[from..]
                .iter()
                .position(|&c| c == q || c == b'<')
                .map(|p| from + p);
            let (to, closed) = match stop {
                Some(p) if self.b[p] == q => (p, true),
                Some(p) => {
                    self.error(
                        Self::span(p, p + 1),
                        "`<` in an attribute value (write `&lt;`): the value and its tag end here",
                    );
                    (p, false)
                }
                None => {
                    self.error(
                        Self::span(from - 1, from),
                        "the attribute value is not closed",
                    );
                    (self.b.len(), false)
                }
            };
            self.pos = if closed { to + 1 } else { to };
            let (text, refs) = self.decode(from, to);
            let value = Value {
                span: Self::span(from, to),
                text,
                refs,
            };
            // A set once a tag has many attributes, so a long tag stays linear.
            let twice = if attrs.len() < 16 {
                attrs.iter().any(|a| a.0 == aname)
            } else {
                if seen.is_empty() {
                    seen.extend(attrs.iter().map(|a| a.0));
                }
                !seen.insert(aname)
            };
            if twice {
                self.error(aspan, format!("the attribute {aname} is written twice"));
            }
            attrs.push((
                aname,
                aspan,
                value,
                Self::span(aspan.start as usize, self.pos),
            ));
            if !closed {
                break;
            }
        }
        let span = Self::span(start, self.pos);
        if self.skip > 0 {
            if !empty {
                self.skip += 1;
            }
            return None;
        }
        if self.stack.len() >= MAX_DEPTH {
            self.error(
                name_span,
                format!("elements nest deeper than {MAX_DEPTH}: this one is not read"),
            );
            if !empty {
                self.skip = 1;
            }
            return None;
        }
        if self.stack.is_empty() {
            self.roots += 1;
            if self.roots == 2 {
                self.error(name_span, "a second root element");
            }
        }
        // This element's declarations apply to itself and its attributes.
        let ns_len = self.ns.len();
        for (aname, _, value, _) in &attrs {
            let prefix = if *aname == "xmlns" {
                ""
            } else if let Some(p) = aname.strip_prefix("xmlns:") {
                p
            } else {
                continue;
            };
            self.ns.push(prefix);
            self.scopes
                .entry(prefix)
                .or_default()
                .push(Rc::from(&*value.text));
        }
        let name = self.resolve(raw, name_span, false);
        let attrs = attrs
            .into_iter()
            .map(|(aname, aspan, value, span)| Attr {
                name: self.resolve(aname, aspan, true),
                value,
                span,
            })
            .collect();
        let tag = Tag {
            name,
            attrs,
            span,
            empty,
        };
        if empty {
            self.unscope(ns_len);
            self.pending.push_back(Event::End(End {
                span,
                implied: false,
            }));
        } else {
            self.stack.push(Open {
                raw,
                name_span,
                ns_len,
            });
        }
        Some(Event::Start(tag))
    }

    /// The namespace `prefix` is bound to; an empty one unbinds it.
    fn bound(&self, prefix: &str) -> Option<Rc<str>> {
        let uri = self.scopes.get(prefix)?.last()?;
        (!uri.is_empty()).then(|| uri.clone())
    }

    /// Undo the declarations after the first `len`.
    fn unscope(&mut self, len: usize) {
        while self.ns.len() > len {
            let prefix = self.ns.pop().expect("longer than len");
            if let Some(uris) = self.scopes.get_mut(prefix) {
                uris.pop();
            }
        }
    }

    fn resolve(&self, raw: &'a str, span: Span, attr: bool) -> Name<'a> {
        let colon = raw.find(':').filter(|&i| i > 0 && i + 1 < raw.len());
        let ns = match colon.map(|i| &raw[..i]) {
            Some("xml") => Ns::Bound(self.xml_ns.clone()),
            Some("xmlns") => Ns::Bound(self.xmlns_ns.clone()),
            Some(p) => match self.bound(p) {
                Some(uri) => Ns::Bound(uri),
                None => Ns::Unknown,
            },
            None if attr => Ns::Unbound,
            None => match self.bound("") {
                Some(uri) => Ns::Bound(uri),
                None => Ns::Unbound,
            },
        };
        Name {
            raw,
            span,
            prefix_len: colon.map(|i| i as u32),
            ns,
        }
    }

    fn end_tag(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        self.pos += 2;
        let (raw, name_span) = self.read_name();
        self.skip_space();
        if self.peek() == Some(b'>') {
            self.pos += 1;
        } else {
            self.error(
                Self::span(start, self.pos),
                "the end tag does not end with `>`",
            );
            while self.peek().is_some_and(|c| c != b'>' && c != b'<') {
                self.pos += 1;
            }
            if self.peek() == Some(b'>') {
                self.pos += 1;
            }
        }
        let span = Self::span(start, self.pos);
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        let Some(k) = self.stack.iter().rposition(|o| o.raw == raw) else {
            self.error(
                if raw.is_empty() { span } else { name_span },
                format!("</{raw}> closes no open element"),
            );
            return None;
        };
        // The elements inside the one it closes are unclosed.
        let here = Self::span(start, start);
        while self.stack.len() > k + 1 {
            let open = self.stack.pop().expect("deeper than k");
            self.error(
                open.name_span,
                format!("<{}> is not closed before </{raw}>", open.raw),
            );
            self.unscope(open.ns_len);
            self.pending.push_back(Event::End(End {
                span: here,
                implied: true,
            }));
        }
        let open = self.stack.pop().expect("k is open");
        self.unscope(open.ns_len);
        self.pending.push_back(Event::End(End {
            span,
            implied: false,
        }));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(text: &str) -> (Vec<String>, Vec<SyntaxError>) {
        let mut s = Scanner::new(text);
        let mut out = Vec::new();
        loop {
            match s.next() {
                Event::Eof => break,
                Event::Start(t) => {
                    let attrs: Vec<String> = t
                        .attrs
                        .iter()
                        .map(|a| format!(" {}={:?}", a.name.raw, a.value.text))
                        .collect();
                    out.push(format!(
                        "<{}{}{}>",
                        t.name.raw,
                        attrs.concat(),
                        if t.empty { "/" } else { "" }
                    ));
                }
                Event::End(e) => out.push(if e.implied { "</?>" } else { "</>" }.into()),
                Event::Text { text, .. } => out.push(format!("{text:?}")),
                Event::Pi(span) => out.push(text[span.start as usize..span.end as usize].into()),
            }
        }
        (out, s.finish().0)
    }

    fn messages(errors: &[SyntaxError]) -> Vec<&str> {
        errors.iter().map(|e| e.message()).collect()
    }

    #[test]
    fn elements_attributes_and_text() {
        let (ev, errors) =
            events("<?xml version=\"1.0\"?>\n<a x='1' y=\"2\"><b/>t<![CDATA[<c>]]><?p d?></a>\n");
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            ev,
            [
                "<a x=\"1\" y=\"2\">",
                "<b/>",
                "</>",
                "\"t\"",
                "\"<c>\"",
                "<?p d?>",
                "</>"
            ]
        );
    }

    #[test]
    fn references_expand_and_keep_their_spans() {
        let text = "<!DOCTYPE r [\n <!ENTITY ex 'http://example.org/'>\n <!ENTITY two \"&ex;two&#37;\">\n]>\n<r a=\"&ex;x\" b='&lt;&#x41;&two;'>&amp;&ex;</r>";
        let mut s = Scanner::new(text);
        let Event::Start(t) = s.next() else { panic!() };
        assert_eq!(t.attrs[0].value.text, "http://example.org/x");
        assert_eq!(t.attrs[0].value.refs, Refs::Entity);
        let v = &t.attrs[0].value;
        assert_eq!(&text[v.span.start as usize..v.span.end as usize], "&ex;x");
        assert_eq!(t.attrs[1].value.text, "<Ahttp://example.org/two%");
        let Event::Text { text: body, .. } = s.next() else {
            panic!()
        };
        assert_eq!(body, "&http://example.org/");
        assert_eq!(
            s.next(),
            Event::End(End {
                span: Span::new(text.len() as u32 - 4, text.len() as u32),
                implied: false
            })
        );
        assert_eq!(s.next(), Event::Eof);
        assert!(s.finish().0.is_empty());
    }

    #[test]
    fn namespaces_resolve_in_scope() {
        let text = "<r xmlns='http://example.org/d#' xmlns:p='http://example.org/p#'><p:a p:x='1' y='2'><b xmlns:p='http://example.org/q#' p:z=''/></p:a><q:c/></r>";
        let mut s = Scanner::new(text);
        let Event::Start(r) = s.next() else { panic!() };
        assert_eq!(r.name.ns(), Some("http://example.org/d#"));
        let Event::Start(a) = s.next() else { panic!() };
        assert!(a.name.is("http://example.org/p#", "a"));
        assert_eq!(a.name.prefix(), Some("p"));
        assert!(a.attrs[0].name.is("http://example.org/p#", "x"));
        assert_eq!(a.attrs[1].name.ns, Ns::Unbound);
        let Event::Start(b) = s.next() else { panic!() };
        assert!(b.name.is("http://example.org/d#", "b"));
        assert!(b.attrs[1].name.is("http://example.org/q#", "z"));
        assert!(matches!(s.next(), Event::End(_)));
        assert!(matches!(s.next(), Event::End(_)));
        let Event::Start(c) = s.next() else { panic!() };
        assert_eq!(c.name.ns, Ns::Unknown);
    }

    #[test]
    fn errors_are_reported_and_reading_goes_on() {
        let (ev, errors) = events("<r><a x=1 y='2'>&nope; & <</a><b></c></r>tail");
        assert_eq!(
            ev,
            [
                "<r>",
                "<a y=\"2\">",
                "\"&nope; & \"",
                "\"<\"",
                "</>",
                "<b>",
                "</?>",
                "</>"
            ]
        );
        assert_eq!(
            messages(&errors),
            [
                "the attribute value is not quoted",
                "the entity &nope; is not declared",
                "`&` starts no reference (write `&amp;`)",
                "`<` starts no tag",
                "</c> closes no open element",
                "<b> is not closed before </r>",
                "text outside the root element",
            ]
        );
        let (ev, errors) = events("<r><a><b>x</r>");
        assert_eq!(ev, ["<r>", "<a>", "<b>", "\"x\"", "</?>", "</?>", "</>"]);
        assert_eq!(errors.len(), 2);
        let (ev, errors) = events("<r><a x='1");
        assert_eq!(ev, ["<r>", "<a x=\"1\">", "</?>", "</?>"]);
        assert_eq!(
            messages(&errors),
            [
                "the attribute value is not closed",
                "<a> is not closed",
                "<r> is not closed"
            ]
        );
        let (ev, _) = events("<r a='x<b/>");
        assert_eq!(ev, ["<r a=\"x\">", "<b/>", "</>", "</?>"]);
    }

    /// An error at unexpected text in the DOCTYPE spans its whole first
    /// character (found by the chaos test: a byte order mark after an
    /// entity declaration).
    #[test]
    fn an_error_at_a_wide_character_spans_all_of_it() {
        for text in [
            "<!DOCTYPE r [<!ENTITY a 'x'>\u{feff}\n]><r/>",
            "<!DOCTYPE r [é😀]><r/>",
        ] {
            let (_, errors) = events(text);
            assert!(!errors.is_empty(), "{text}");
            for e in &errors {
                let (start, end) = (e.span.start as usize, e.span.end as usize);
                assert!(
                    text.is_char_boundary(start) && text.is_char_boundary(end),
                    "{text}: {e:?}"
                );
            }
        }
    }

    #[test]
    fn limits_hold() {
        let deep = "<a>".repeat(MAX_DEPTH + 10) + "x" + &"</a>".repeat(MAX_DEPTH + 10) + "<b/>";
        let (ev, errors) = events(&deep);
        assert_eq!(ev.iter().filter(|e| *e == "<a>").count(), MAX_DEPTH);
        assert_eq!(ev.last().map(String::as_str), Some("</>"));
        assert!(ev.contains(&"<b/>".to_string()));
        assert_eq!(errors.len(), 2, "{errors:?}"); // too deep, a second root
        let mut decl = String::from("<!DOCTYPE r [");
        decl += "<!ENTITY a0 \"0123456789012345678901234567890123456789012345678901234567890123\">";
        for i in 1..12 {
            decl += &format!("<!ENTITY a{i} \"&a{0};&a{0};\">", i - 1);
        }
        decl += "]><r>";
        decl += &"&a11;".repeat(1000);
        decl += "</r>";
        let (_, errors) = events(&decl);
        let m = messages(&errors);
        assert!(m.iter().any(|m| m.contains("KiB")), "{m:?}");
        let mut s = Scanner::new(&decl);
        let mut total = 0;
        loop {
            match s.next() {
                Event::Eof => break,
                Event::Text { text, .. } => total += text.len(),
                _ => {}
            }
        }
        assert!(total <= decl.len() * EXPANSION_PER_BYTE + MAX_ENTITY_BYTES + decl.len());
    }

    #[test]
    fn garbage_ends() {
        let mut seed = 7u64;
        for _ in 0..2000 {
            let len = (seed % 40) as usize;
            let mut t = String::new();
            for _ in 0..len {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let alphabet = b"<>/!?-[]&;#'\"= ax:CDATA%\n";
                t.push(alphabet[(seed % alphabet.len() as u64) as usize] as char);
            }
            let (ev, _) = events(&t);
            assert!(ev.len() <= t.len() * 2 + 2, "{t:?}");
        }
    }
}
