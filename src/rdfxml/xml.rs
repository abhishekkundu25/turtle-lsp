//! A fault-tolerant XML scanner that keeps the span of every name,
//! attribute value, text and entity reference, with namespace scopes.
//!
//! [`Scanner::next`] gives one [`Event`] at a time: a start tag with its
//! attributes, an end tag, character data or a processing instruction.
//! Comments are kept as spans ([`Scanner::finish`]); the XML declaration
//! is read inside. The prolog, with its DOCTYPE, is read first
//! ([`Prolog::read`]), and the scanner borrows the internal entities it
//! declares. Every name comes resolved against the namespace declarations
//! in scope (`xmlns`, `xmlns:p`), and every text and attribute value comes
//! with its references expanded: the five predefined entities, character
//! references and the internal entities of the DOCTYPE, in either quote
//! style.
//!
//! As XML 1.0 reads them: line ends are normalized to `\n` (2.11), and
//! attribute values are normalized (3.3.3: each white space character
//! becomes a space; a character reference stays the character it names).
//! An entity's replacement text has its character references expanded and
//! its entity references kept (4.5); they expand where it is used. Used in
//! content, an entity whose replacement text holds markup is read as
//! markup (4.4.2, "included"); in an attribute value, its text is data.
//! Everything read from an entity's replacement text has the span of the
//! reference in the document (the outermost one, when references nest).
//!
//! Errors are `xml-syntax`, with the span of what is wrong. The scanner
//! reads on after each one:
//! - a `<` that starts no tag is text;
//! - a broken start or end tag ends at its `>`, or before the next `<`;
//!   an attribute value with a `<` in it ends there, and so does its tag;
//! - an end tag that names an open element further out closes the ones
//!   inside it ([`End::implied`]); one that names no open element is
//!   skipped;
//! - at the end of the text, or of an entity's replacement text, every
//!   element opened there is closed the same way;
//! - text outside the root element is dropped.
//!
//! What well-formed XML forbids and `oxrdfxml` (through quick-xml) reads
//! anyway is reported and read the way it reads it: a second root element,
//! a duplicate attribute (also two names with one namespace and local name,
//! Namespaces in XML 6.3), an XML declaration that is not first (after a
//! byte order mark at most, 2.8), and an empty prefixed namespace in XML
//! 1.0 (Namespaces in XML 1.0, 3: an undeclaration, as XML 1.1 reads it).
//! A declaration Namespaces in XML reserves (3: `xml` to another
//! namespace, another prefix or the default to `xml`'s or `xmlns`'s, any
//! `xmlns:xmlns`) is reported and not applied.
//!
//! Limits: elements nest at most [`MAX_DEPTH`] deep (deeper ones are
//! skipped, with one error); the DOCTYPE declares at most [`MAX_ENTITIES`]
//! entities whose full expansions total at most [`MAX_ENTITY_BYTES`] (a
//! recursive entity is not expanded, nor is a reference met inside its own
//! expansion); expanding entity references stops after
//! [`EXPANSION_PER_BYTE`] steps per byte of text (plus
//! [`MAX_ENTITY_BYTES`]), each expansion costing its replacement text's
//! length plus one, at any depth, so an entity that expands to nothing
//! costs too; and at most [`MAX_ERRORS`] errors are reported.
//! Every step consumes at least one byte of the document or of a
//! replacement text, so a scan ends. Nesting is what bounds triple terms
//! too: each level of one takes two elements, so they nest at most
//! [`MAX_DEPTH`] / 2 deep.
//!
//! The scanner counts every fault ([`Scanner::faults`]): each error,
//! reported or past [`MAX_ERRORS`], each reference left unexpanded past
//! the expansion limit and each element skipped for its depth. The OWL/XML
//! reader (`owx`), which reads this scanner too, compares the count before
//! and after an axiom to know whether it read the axiom whole.

use crate::syntax::{Severity, Span, SyntaxError, shown};
use rustc_hash::FxHashMap;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::rc::Rc;

pub(crate) mod cursor;

/// Elements nest at most this deep; a deeper element is skipped with its
/// content.
pub(crate) const MAX_DEPTH: usize = 256;
/// Entity declarations read from a DOCTYPE.
pub(crate) const MAX_ENTITIES: usize = 256;
/// The expanded values of all declared entities, together.
pub(crate) const MAX_ENTITY_BYTES: usize = 64 * 1024;
/// Expanding entity references takes at most this many steps per byte of
/// text: one per byte of replacement text read, and one per reference.
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
    /// Read from an entity's replacement text: `span` is the reference's,
    /// not the name's.
    pub(crate) entity: bool,
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
    /// Predefined entities (`&amp;`), character references or normalized
    /// white space only.
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
    /// The reference to a declared entity the value starts with, in the
    /// document, and the bytes of `text` its expansion gives.
    pub(crate) lead: Option<(Span, u32)>,
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
    /// The name in the end tag; `None` for an empty element and an implied
    /// end.
    pub(crate) name: Option<Span>,
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
    /// A processing instruction inside an element, from `<?` to `?>`: its
    /// span and its text.
    Pi(Span, &'a str),
    Eof,
}

struct Open<'a> {
    raw: &'a str,
    name_span: Span,
    /// `ns.len()` before this element's declarations.
    ns_len: usize,
}

/// An internal entity the DOCTYPE declares.
pub(crate) struct Entity {
    /// Its place in the declarations: [`Scanner::open`] is indexed by it.
    index: usize,
    /// Its replacement text: character references expanded, entity
    /// references kept (XML 1.0, 4.5).
    text: Box<str>,
    /// Its expansion holds `<`: used in content, it is read as markup.
    markup: bool,
    /// Recursive, or over [`MAX_ENTITY_BYTES`] (reported at its
    /// declaration): a reference to it is kept as written.
    refused: bool,
}

type Entities = FxHashMap<Box<str>, Entity>;

/// A reference to an entity read as markup: `&`, `;`, its name, the entity.
type MarkupRef<'a> = (usize, usize, &'a str, &'a Entity);

/// What comes before the root element, read before the rest: where the
/// root starts, the errors and comments before it, and the internal
/// entities of the DOCTYPE, which the [`Scanner`] borrows.
#[derive(Default)]
pub(crate) struct Prolog {
    entities: Entities,
    /// Some entity holds markup.
    markup: bool,
    /// The XML declaration says `version="1.1"`.
    xml11: bool,
    pos: usize,
    errors: Vec<SyntaxError>,
    dropped: bool,
    /// [`Scanner::faults`] at the root element.
    faults: usize,
    comments: Vec<Span>,
}

impl Prolog {
    pub(crate) fn read(text: &str) -> Prolog {
        let none = Entities::default();
        let mut s = Scanner::start(text, &none, false);
        s.prolog = true;
        while s.pos < s.b.len() && !s.at_start_tag() {
            let event = s.step();
            debug_assert!(event.is_none(), "no event before the root element");
        }
        let (entities, markup) = s.entities();
        Prolog {
            entities,
            markup,
            xml11: s.xml11,
            pos: s.pos,
            errors: s.errors,
            dropped: s.dropped,
            faults: s.faults,
            comments: s.comments,
        }
    }
}

/// The input a scanner reads on after an entity's replacement text.
struct Frame<'a> {
    text: &'a str,
    pos: usize,
    /// The elements open when the entity started: it closes its own.
    depth: usize,
    name: &'a str,
    index: usize,
}

/// Where a text to expand is.
#[derive(Clone, Copy)]
enum Origin {
    /// In the current input, at this offset.
    Input(usize),
    /// In the replacement text of the entity referenced at this span.
    Ref(Span),
}

/// What a text is read as: content, or an attribute value (normalized).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Text,
    Attr,
}

pub(crate) struct Scanner<'a> {
    /// The current input: the document, or the replacement text of the
    /// entity being read ([`Scanner::frames`]).
    text: &'a str,
    b: &'a [u8],
    pos: usize,
    /// The inputs to read on in, innermost last, while in an entity.
    frames: Vec<Frame<'a>>,
    /// The span of the outermost entity reference, while in an entity.
    at: Span,
    stack: Vec<Open<'a>>,
    /// The prefixes declared by the open elements, in order (`""` for the
    /// default namespace), and the namespaces each prefix is bound to,
    /// innermost last: lookups and undoing take constant time.
    ns: Vec<&'a str>,
    scopes: FxHashMap<&'a str, Vec<Rc<str>>>,
    xml_ns: Rc<str>,
    xmlns_ns: Rc<str>,
    pending: VecDeque<Event<'a>>,
    entities: &'a Entities,
    /// Some entity holds markup: content is searched for references to it.
    markup: bool,
    /// Reading the prolog: the first DOCTYPE's entities are recorded.
    prolog: bool,
    /// The XML declaration says `version="1.1"`: a prefix may be
    /// undeclared (Namespaces in XML 1.1).
    xml11: bool,
    doctypes: usize,
    /// The entity declarations read, in order: name, its span, replacement
    /// text.
    declared: Vec<(&'a str, Span, Box<str>)>,
    /// The steps of expansion taken, and the most allowed: see
    /// [`EXPANSION_PER_BYTE`].
    expanded: usize,
    budget: usize,
    over_budget: bool,
    /// The entities being expanded, by [`Entity::index`]: a reference to
    /// one of them, met inside its own expansion, is not expanded.
    open: Vec<bool>,
    /// The [`Value::lead`] of the attribute value being read.
    lead: Option<(Span, u32)>,
    /// Inside an element deeper than [`MAX_DEPTH`]: its nesting count.
    skip: usize,
    roots: usize,
    done: bool,
    errors: Vec<SyntaxError>,
    dropped: bool,
    /// Every fault so far: see [`Scanner::faults`].
    faults: usize,
    comments: Vec<Span>,
}

/// The value of the pseudo-attribute `name` in an XML declaration's
/// `body`, as written between its quotes.
fn pseudo_attr<'b>(body: &'b str, name: &str) -> Option<&'b str> {
    let i = body.find(name)?;
    let rest = body[i + name.len()..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    let q = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    rest[1..].split(q).next()
}

pub(super) fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

/// Bytes that end a name.
pub(super) fn ends_name(c: u8) -> bool {
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

/// The end of the reference that starts at `b[amp]` (`&`): its `;`, before
/// any space, `&` or `<`, within 256 bytes.
fn ref_end(b: &[u8], amp: usize) -> Option<usize> {
    b[amp + 1..]
        .iter()
        .take(256)
        .position(|&c| c == b';' || is_space(c) || c == b'&' || c == b'<')
        .map(|p| amp + 1 + p)
        .filter(|&p| b[p] == b';')
}

/// The character `&#num;` names, if it is an XML character.
fn char_ref(num: &str) -> Option<char> {
    match num.strip_prefix('x') {
        Some(hex) => u32::from_str_radix(hex, 16).ok(),
        None => num.parse::<u32>().ok(),
    }
    .filter(|_| !num.starts_with(['+', '-']) && !num.starts_with("x+"))
    .and_then(char::from_u32)
    .filter(|&c| is_xml_char(c))
}

fn predefined(name: &str) -> Option<char> {
    match name {
        "lt" => Some('<'),
        "gt" => Some('>'),
        "amp" => Some('&'),
        "apos" => Some('\''),
        "quot" => Some('"'),
        _ => None,
    }
}

/// `text` with its line ends normalized (XML 1.0, 2.11).
fn lines(text: &str) -> Cow<'_, str> {
    if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

#[derive(Clone, Copy)]
enum Walk {
    New,
    Open,
    Done { size: usize, markup: bool },
    Recursive,
}

/// The full expansion of declared entity `i`: its size and whether it holds
/// markup; `None` when it refers to itself, directly or not.
fn walk(
    i: usize,
    declared: &[(&str, Span, Box<str>)],
    index: &FxHashMap<&str, usize>,
    seen: &mut [Walk],
) -> Option<(usize, bool)> {
    match seen[i] {
        Walk::Open | Walk::Recursive => return None,
        Walk::Done { size, markup } => return Some((size, markup)),
        Walk::New => {}
    }
    seen[i] = Walk::Open;
    let text = &*declared[i].2;
    let b = text.as_bytes();
    let (mut size, mut markup, mut recursive) = (0usize, text.contains('<'), false);
    let mut from = 0;
    while let Some(amp) = b[from..]
        .iter()
        .position(|&c| c == b'&' || c == b'<')
        .map(|p| from + p)
    {
        size = size.saturating_add(amp - from);
        from = amp + 1;
        if b[amp] == b'<' {
            // Read as markup, a comment, CDATA section or processing
            // instruction holds no reference, to its end or to the end of
            // the text, as the scanner reads it. As data (an attribute
            // value) it does: a reference met inside its own expansion is
            // caught there.
            let (open, close): (&[u8], &[u8]) = match &b[amp..] {
                r if r.starts_with(b"<!--") => (b"<!--", b"-->"),
                r if r.starts_with(b"<![CDATA[") => (b"<![CDATA[", b"]]>"),
                r if r.starts_with(b"<?") => (b"<?", b"?>"),
                _ => {
                    size = size.saturating_add(1);
                    continue;
                }
            };
            let body = amp + open.len();
            let end = b[body..]
                .windows(close.len())
                .position(|w| w == close)
                .map_or(b.len(), |p| body + p + close.len());
            size = size.saturating_add(end - amp);
            from = end;
            continue;
        }
        let Some(semi) = ref_end(b, amp) else {
            size = size.saturating_add(1);
            continue;
        };
        from = semi + 1;
        let name = &text[amp + 1..semi];
        match index.get(name) {
            Some(&j) if predefined(name).is_none() => match walk(j, declared, index, seen) {
                Some((s, m)) => {
                    size = size.saturating_add(s);
                    markup |= m;
                }
                None => recursive = true,
            },
            // Kept as written, or a character no longer than that.
            _ => size = size.saturating_add(semi + 1 - amp),
        }
    }
    size = size.saturating_add(b.len() - from);
    seen[i] = if recursive {
        Walk::Recursive
    } else {
        Walk::Done { size, markup }
    };
    (!recursive).then_some((size, markup))
}

impl<'a> Scanner<'a> {
    /// The scanner of the document after its prolog.
    pub(crate) fn new(text: &'a str, prolog: &'a Prolog) -> Scanner<'a> {
        let mut s = Scanner::start(text, &prolog.entities, prolog.markup);
        s.pos = prolog.pos.max(s.pos);
        s.xml11 = prolog.xml11;
        s.errors.clone_from(&prolog.errors);
        s.dropped = prolog.dropped;
        s.faults = prolog.faults;
        s.comments.clone_from(&prolog.comments);
        s
    }

    fn start(text: &'a str, entities: &'a Entities, markup: bool) -> Scanner<'a> {
        let pos = if text.starts_with('\u{FEFF}') { 3 } else { 0 };
        Scanner {
            text,
            b: text.as_bytes(),
            pos,
            frames: Vec::new(),
            at: Span::new(0, 0),
            stack: Vec::new(),
            ns: Vec::new(),
            scopes: FxHashMap::default(),
            xml_ns: Rc::from(XML_NS),
            xmlns_ns: Rc::from(XMLNS_NS),
            pending: VecDeque::new(),
            entities,
            markup,
            prolog: false,
            xml11: false,
            doctypes: 0,
            declared: Vec::new(),
            expanded: 0,
            budget: text.len().saturating_mul(EXPANSION_PER_BYTE) + MAX_ENTITY_BYTES,
            over_budget: false,
            open: vec![false; entities.len()],
            lead: None,
            skip: 0,
            roots: 0,
            done: false,
            errors: Vec::new(),
            dropped: false,
            faults: 0,
            comments: Vec::new(),
        }
    }

    /// The errors and the comment spans, once the scan is over.
    pub(crate) fn finish(self) -> (Vec<SyntaxError>, Vec<Span>) {
        (self.errors, self.comments)
    }

    /// How many faults the scan has met so far: one per error, reported or
    /// past [`MAX_ERRORS`], one per reference left unexpanded past the
    /// expansion limit, and one per element skipped for its depth. A reader
    /// that reads it between events knows whether what it read in between
    /// was read whole.
    pub(crate) fn faults(&self) -> usize {
        self.faults
    }

    /// Report an error, unless [`MAX_ERRORS`] have been.
    pub(crate) fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        self.faults += 1;
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

    /// The span of `start..end` in the current input: in an entity's
    /// replacement text, the span of the reference to it.
    fn span(&self, start: usize, end: usize) -> Span {
        if self.frames.is_empty() {
            Span::new(start as u32, end as u32)
        } else {
            self.at
        }
    }

    fn comment(&mut self, span: Span) {
        // A comment in a replacement text is none in the document.
        if self.frames.is_empty() {
            self.comments.push(span);
        }
    }

    fn at_start_tag(&self) -> bool {
        self.b[self.pos] == b'<'
            && self.text[self.pos + 1..]
                .chars()
                .next()
                .is_some_and(is_name_start_char)
    }

    /// The entities declared, with their expansions measured: a recursive
    /// one, and those past [`MAX_ENTITY_BYTES`] in all, are refused.
    fn entities(&mut self) -> (Entities, bool) {
        let declared = std::mem::take(&mut self.declared);
        let index: FxHashMap<&str, usize> =
            declared.iter().enumerate().map(|(i, d)| (d.0, i)).collect();
        let mut seen = vec![Walk::New; declared.len()];
        let measured: Vec<_> = (0..declared.len())
            .map(|i| walk(i, &declared, &index, &mut seen))
            .collect();
        let (mut total, mut any_markup) = (0usize, false);
        let mut out = Entities::default();
        for (index, ((name, span, text), measured)) in
            declared.into_iter().zip(measured).enumerate()
        {
            let (markup, refused) = match measured {
                None => {
                    self.error(
                        span,
                        format!(
                            "the entity &{}; refers to itself: it is not expanded",
                            shown(name)
                        ),
                    );
                    (false, true)
                }
                Some((size, _)) if total.saturating_add(size) > MAX_ENTITY_BYTES => {
                    self.error(
                        span,
                        format!(
                            "the declared entities are over {} KiB: this one is not read",
                            MAX_ENTITY_BYTES / 1024
                        ),
                    );
                    (false, true)
                }
                Some((size, markup)) => {
                    total += size;
                    (markup, false)
                }
            };
            any_markup |= markup;
            out.insert(
                name.into(),
                Entity {
                    index,
                    text,
                    markup,
                    refused,
                },
            );
        }
        (out, any_markup)
    }

    /// One expansion of `entity`, referenced at `here`: false when the
    /// budget is spent (reported once) or the entity is being expanded
    /// already (reported at each reference). Otherwise it is charged.
    fn may_expand(&mut self, name: &str, entity: &Entity, here: Span) -> bool {
        if self.open[entity.index] {
            self.error(
                here,
                format!(
                    "the entity &{}; refers to itself: it is not expanded",
                    shown(name)
                ),
            );
            return false;
        }
        let cost = entity.text.len() + 1;
        if self.over_budget || self.expanded + cost > self.budget {
            if self.over_budget {
                self.faults += 1;
            } else {
                self.over_budget = true;
                self.error(
                    here,
                    "entity expansion is over its limit: the rest of the references are not expanded",
                );
            }
            return false;
        }
        self.expanded += cost;
        true
    }

    /// Read the replacement text of `entity`, referenced at `amp..=semi`,
    /// as markup.
    fn enter(&mut self, name: &'a str, entity: &'a Entity, amp: usize, semi: usize) {
        if self.frames.is_empty() {
            self.at = self.span(amp, semi + 1);
        }
        self.open[entity.index] = true;
        self.frames.push(Frame {
            text: self.text,
            pos: semi + 1,
            depth: self.stack.len(),
            name,
            index: entity.index,
        });
        self.text = &entity.text;
        self.b = self.text.as_bytes();
        self.pos = 0;
    }

    /// At the end of a replacement text: the elements it opened are
    /// unclosed; read on after its reference.
    fn leave(&mut self, frame: Frame<'a>) {
        // The frame is popped: `self.pos` is in the replacement text still,
        // and the elements it leaves open end at the reference.
        let here = self.at;
        while self.stack.len() > frame.depth {
            let open = self.stack.pop().expect("deeper than the frame");
            self.error(
                open.name_span,
                format!(
                    "<{}> is not closed in the entity &{};",
                    shown(open.raw),
                    shown(frame.name)
                ),
            );
            self.unscope(open.ns_len);
            self.pending.push_back(Event::End(End {
                span: here,
                implied: true,
                name: None,
            }));
        }
        self.skip = 0;
        self.open[frame.index] = false;
        self.text = frame.text;
        self.b = frame.text.as_bytes();
        self.pos = frame.pos;
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
        (&self.text[start..self.pos], self.span(start, self.pos))
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
                if let Some(frame) = self.frames.pop() {
                    self.leave(frame);
                    continue;
                }
                self.close_all();
                if self.roots == 0 {
                    let end = self.span(self.b.len(), self.b.len());
                    self.error(end, "the document has no root element");
                }
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
        let end = self.span(self.b.len(), self.b.len());
        self.skip = 0;
        while let Some(open) = self.stack.pop() {
            self.error(
                open.name_span,
                format!("<{}> is not closed", shown(open.raw)),
            );
            self.unscope(open.ns_len);
            self.pending.push_back(Event::End(End {
                span: end,
                implied: true,
                name: None,
            }));
        }
    }

    /// Read one construct; `Some` when it gives an event.
    fn step(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        if self.b[start] != b'<' {
            let end = match self.text_run(start) {
                Err(end) => end,
                Ok((amp, ..)) if amp > start => amp,
                Ok((amp, semi, name, entity)) => {
                    if self.may_expand(name, entity, self.span(amp, semi + 1)) {
                        self.enter(name, entity, amp, semi);
                        return None;
                    }
                    // Read as text: the reference is kept as written.
                    semi + 1
                }
            };
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
                self.error(self.span(start, start + 1), "`<` starts no tag");
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
                    self.span(start + lead, end),
                    "text outside the root element",
                );
            }
            return None;
        }
        let (text, _) = self.decode(start, end, Mode::Text);
        Some(Event::Text {
            span: self.span(start, end),
            text,
        })
    }

    /// Where the content from `start` (not a `<`) ends: `Err` at the next
    /// `<` or the end of the input, or `Ok` at the first reference to an
    /// entity read as markup: `&`, `;`, its name and the entity. Only what
    /// is passed over is read, so a run of references reads in linear time.
    fn text_run(&self, start: usize) -> Result<MarkupRef<'a>, usize> {
        let b = self.b;
        if !self.markup || self.skip > 0 || self.stack.is_empty() || self.over_budget {
            return Err(self.find(start + 1, b"<").unwrap_or(b.len()));
        }
        let entities: &'a Entities = self.entities;
        let text: &'a str = self.text;
        let mut from = start;
        while let Some(at) = b[from..]
            .iter()
            .position(|&c| c == b'&' || c == b'<')
            .map(|p| from + p)
        {
            if b[at] == b'<' {
                return Err(at);
            }
            from = at + 1;
            let Some(semi) = ref_end(b, at) else {
                continue;
            };
            let name = &text[at + 1..semi];
            match entities.get(name) {
                // One being expanded stays in the text: `expand` reports it.
                Some(e)
                    if e.markup
                        && !e.refused
                        && !self.open[e.index]
                        && predefined(name).is_none() =>
                {
                    return Ok((at, semi, name, e));
                }
                _ => from = semi + 1,
            }
        }
        Err(b.len())
    }

    fn bang(&mut self) -> Option<Event<'a>> {
        let start = self.pos;
        let rest = &self.b[start..];
        if rest.starts_with(b"<!--") {
            let end = match self.find(start + 4, b"-->") {
                Some(e) => e + 3,
                None => {
                    self.error(self.span(start, start + 4), "the comment is not closed");
                    self.b.len()
                }
            };
            self.comment(self.span(start, end));
            self.pos = end;
            None
        } else if rest.starts_with(b"<![CDATA[") {
            let from = start + 9;
            let (to, end) = match self.find(from, b"]]>") {
                Some(e) => (e, e + 3),
                None => {
                    self.error(self.span(start, from), "the CDATA section is not closed");
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
                    self.error(self.span(start, end), "text outside the root element");
                }
                return None;
            }
            Some(Event::Text {
                span: self.span(from, to),
                text: if self.frames.is_empty() {
                    lines(raw)
                } else {
                    Cow::Borrowed(raw)
                },
            })
        } else if rest.starts_with(b"<!DOCTYPE") {
            self.doctype();
            None
        } else {
            self.error(self.span(start, start + 2), "unknown markup after `<!`");
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
                    self.span(start, start + 2),
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
                self.span(start + 2, start + 2 + target_len),
                "not a processing instruction target",
            );
        }
        (self.skip == 0 && !self.stack.is_empty())
            .then(|| Event::Pi(self.span(start, end), &self.text[start..end]))
    }

    /// `<?xml version="1.0" encoding="..."?>`, first in the document or
    /// right after its byte order mark (XML 1.0, 2.8): only UTF-8 is read.
    /// A document in ASCII may declare an encoding that extends ASCII: its
    /// bytes mean the same in UTF-8.
    fn declaration(&mut self, start: usize, body: &str) {
        let bom = if self.text.starts_with('\u{FEFF}') {
            3
        } else {
            0
        };
        let first = start == bom && self.frames.is_empty();
        if !first {
            self.error(
                self.span(start, start + 5),
                "the XML declaration is not at the start of the document",
            );
        } else if let Some(v) = pseudo_attr(body, "version") {
            self.xml11 = v == "1.1";
        }
        if let Some(i) = body.find("encoding")
            && let Some(value) = pseudo_attr(&body[i..], "encoding")
        {
            let value = value.to_ascii_lowercase();
            let utf8 = matches!(
                value.as_str(),
                "utf-8"
                    | "utf8"
                    | "unicode-1-1-utf-8"
                    | "unicode11utf8"
                    | "unicode20utf8"
                    | "x-unicode20utf8"
            );
            let extends_ascii = matches!(value.as_str(), "us-ascii" | "ascii" | "latin1")
                || value.starts_with("iso-8859-")
                || value.starts_with("windows-125");
            // Only the first declaration is checked against the text, so
            // many declarations cost no more than one.
            if !utf8 && !(extends_ascii && first && self.b.is_ascii()) {
                let at = start + 2 + i;
                self.error(
                    self.span(at, at + 8),
                    "only UTF-8 is read: the text is read as UTF-8",
                );
            }
        }
    }

    /// `<!DOCTYPE name ExternalID? [ internal subset ]? >`; the first
    /// one in the prolog records its internal entity declarations.
    fn doctype(&mut self) {
        let start = self.pos;
        self.pos += 9;
        self.doctypes += 1;
        if !self.prolog {
            self.error(
                self.span(start, start + 9),
                "a DOCTYPE after the root element starts: it is not read",
            );
        } else if self.doctypes > 1 {
            self.error(
                self.span(start, start + 9),
                "a second DOCTYPE: it is not read",
            );
        }
        self.skip_space();
        let (name, name_span) = self.read_name();
        if name.is_empty() {
            self.error(self.span(start, start + 9), "the DOCTYPE has no name");
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
                    self.span(start, start + 9),
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
                self.error(self.span(self.pos, from), "the quote is not closed");
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
                self.error(self.span(at, at), "the DOCTYPE is not closed");
                return;
            } else if rest[0] == b']' {
                self.pos += 1;
                return;
            } else if rest.starts_with(b"<!ENTITY") {
                self.entity_decl();
            } else if rest.starts_with(b"<!--") {
                match self.find(at + 4, b"-->") {
                    Some(e) => {
                        self.comment(self.span(at, e + 3));
                        self.pos = e + 3;
                    }
                    None => {
                        self.error(self.span(at, at + 4), "the comment is not closed");
                        self.pos = self.b.len();
                    }
                }
            } else if rest.starts_with(b"<?") {
                match self.find(at + 2, b"?>") {
                    Some(e) => self.pos = e + 2,
                    None => {
                        self.error(
                            self.span(at, at + 2),
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
                        self.error(self.span(at, at + 1), "a `%` that starts no reference");
                        self.pos += 1;
                    }
                }
            } else {
                self.bump_char();
                self.error(self.span(at, self.pos), "unexpected text in the DOCTYPE");
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
                    self.span(start, start + 8)
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
                        self.span(start, start + 8),
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
                self.span(start, start + 8),
                "the entity declaration does not end with `>`",
            );
            self.skip_decl();
        }
        let Some((from, to)) = value else { return };
        if parameter || name.is_empty() || !self.prolog || self.doctypes > 1 {
            return;
        }
        if self.declared.iter().any(|d| d.0 == name) {
            // The first declaration binds (XML 1.0, 4.2).
            return;
        }
        if self.declared.len() >= MAX_ENTITIES {
            self.error(
                name_span,
                format!("more than {MAX_ENTITIES} entities are declared: this one is not read"),
            );
            return;
        }
        let value = self.entity_value(from, to);
        self.declared.push((name, name_span, value));
    }

    /// The replacement text of the entity value `from..to` (XML 1.0, 4.5):
    /// line ends normalized, character references expanded, entity
    /// references kept as written (4.4.7, "bypassed").
    fn entity_value(&mut self, from: usize, to: usize) -> Box<str> {
        let raw = &self.text[from..to];
        if raw.contains('%') {
            // A parameter entity reference in a value: not allowed in the
            // internal subset; kept as written.
            self.error(
                self.span(from, to),
                "a parameter entity reference in an entity value",
            );
        }
        let mut out = String::with_capacity(raw.len());
        let mut i = from;
        while let Some(j) = self.b[i..to]
            .iter()
            .position(|&c| c == b'&' || c == b'\r')
            .map(|p| i + p)
        {
            out.push_str(&self.text[i..j]);
            if self.b[j] == b'\r' {
                out.push('\n');
                i = j + 1 + usize::from(self.b.get(j + 1) == Some(&b'\n') && j + 1 < to);
                continue;
            }
            let Some(semi) = ref_end(&self.b[..to], j) else {
                self.error(
                    self.span(j, j + 1),
                    "`&` starts no reference (write `&amp;`)",
                );
                out.push('&');
                i = j + 1;
                continue;
            };
            i = semi + 1;
            let name = &self.text[j + 1..semi];
            match name.strip_prefix('#').map(char_ref) {
                Some(Some(c)) => out.push(c),
                Some(None) => {
                    self.error(
                        self.span(j, semi + 1),
                        "not a character reference to an XML character",
                    );
                    out.push_str(&self.text[j..=semi]);
                }
                None => out.push_str(&self.text[j..=semi]),
            }
        }
        out.push_str(&self.text[i..to]);
        out.into()
    }

    /// The text of `start..end` with its references expanded, and, for an
    /// attribute value, normalized. Errors are reported at each bad
    /// reference, which is kept as written.
    fn decode(&mut self, start: usize, end: usize, mode: Mode) -> (Cow<'a, str>, Refs) {
        let raw: &'a str = &self.text[start..end];
        self.lead = None;
        // Line ends are normalized in the document; a replacement text
        // already is, and a `\r` in it comes from a character reference.
        let document = self.frames.is_empty();
        let plain = !raw.bytes().any(|c| {
            c == b'&'
                || (c == b'\r' && document)
                || (mode == Mode::Attr && matches!(c, b'\t' | b'\n' | b'\r'))
        });
        if plain {
            return (Cow::Borrowed(raw), Refs::None);
        }
        let mut out = String::with_capacity(raw.len());
        let mut refs = Refs::None;
        self.expand(raw, Origin::Input(start), mode, &mut out, &mut refs);
        (Cow::Owned(out), refs)
    }

    /// The span of `i..j` in a text at `origin`.
    fn place(&self, origin: Origin, i: usize, j: usize) -> Span {
        match origin {
            Origin::Input(at) => self.span(at + i, at + j),
            Origin::Ref(span) => span,
        }
    }

    /// Expand `raw`, at `origin`, into `out`.
    fn expand(
        &mut self,
        raw: &'a str,
        origin: Origin,
        mode: Mode,
        out: &mut String,
        refs: &mut Refs,
    ) {
        let b = raw.as_bytes();
        let input = matches!(origin, Origin::Input(_));
        let document = input && self.frames.is_empty();
        let entities: &'a Entities = self.entities;
        let mut i = 0;
        while let Some(j) = b[i..]
            .iter()
            .position(|&c| {
                c == b'&'
                    || (c == b'\r' && document)
                    || (mode == Mode::Attr && matches!(c, b'\t' | b'\n' | b'\r'))
            })
            .map(|p| i + p)
        {
            out.push_str(&raw[i..j]);
            i = j + 1;
            if b[j] != b'&' {
                // A line end (`\r\n` or `\r`) is one `\n`; in an attribute
                // value, white space is a space.
                if b[j] == b'\r' && document && b.get(i) == Some(&b'\n') {
                    i += 1;
                }
                if mode == Mode::Attr {
                    out.push(' ');
                    *refs = (*refs).max(Refs::Chars);
                } else {
                    out.push('\n');
                }
                continue;
            }
            let Some(semi) = ref_end(b, j) else {
                let at = self.place(origin, j, j + 1);
                self.error(at, "`&` starts no reference (write `&amp;`)");
                out.push('&');
                continue;
            };
            i = semi + 1;
            let name = &raw[j + 1..semi];
            let here = self.place(origin, j, semi + 1);
            if let Some(num) = name.strip_prefix('#') {
                match char_ref(num) {
                    Some(c) => {
                        out.push(c);
                        *refs = (*refs).max(Refs::Chars);
                    }
                    None => {
                        self.error(here, "not a character reference to an XML character");
                        out.push_str(&raw[j..=semi]);
                    }
                }
                continue;
            }
            if let Some(c) = predefined(name) {
                out.push(c);
                *refs = (*refs).max(Refs::Chars);
                continue;
            }
            let entity = match entities.get(name) {
                None => {
                    self.error(
                        here,
                        format!("the entity &{}; is not declared", shown(name)),
                    );
                    out.push_str(&raw[j..=semi]);
                    continue;
                }
                Some(e) if e.refused => {
                    self.error(
                        here,
                        format!(
                            "the entity &{}; is not expanded: see its declaration",
                            shown(name)
                        ),
                    );
                    out.push_str(&raw[j..=semi]);
                    continue;
                }
                Some(e) => e,
            };
            if !self.may_expand(name, entity, here) {
                out.push_str(&raw[j..=semi]);
                continue;
            }
            if input && mode == Mode::Attr && entity.markup {
                // XML 1.0, 3.1, "No < in Attribute Values".
                self.error(
                    here,
                    format!(
                        "the entity &{}; puts `<` in an attribute value",
                        shown(name)
                    ),
                );
            }
            self.open[entity.index] = true;
            let before = out.len();
            self.expand(&entity.text, Origin::Ref(here), mode, out, refs);
            self.open[entity.index] = false;
            if j == 0 && document && mode == Mode::Attr {
                self.lead = Some((here, (out.len() - before) as u32));
            }
            *refs = Refs::Entity;
        }
        out.push_str(&raw[i..]);
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
                    self.error(self.span(start, at), "the tag is not closed");
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
                    self.error(self.span(start, at), "the tag is not closed");
                    break;
                }
                Some(_) => {}
            }
            let (aname, aspan) = self.read_name();
            if aname.is_empty() {
                self.bump_char();
                self.error(self.span(at, self.pos), "unexpected character in a tag");
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
                        self.span(p, p + 1),
                        "`<` in an attribute value (write `&lt;`): the value and its tag end here",
                    );
                    (p, false)
                }
                None => {
                    self.error(
                        self.span(from - 1, from),
                        "the attribute value is not closed",
                    );
                    (self.b.len(), false)
                }
            };
            self.pos = if closed { to + 1 } else { to };
            let (text, refs) = self.decode(from, to, Mode::Attr);
            let value = Value {
                span: self.span(from, to),
                text,
                refs: if self.frames.is_empty() {
                    refs
                } else {
                    Refs::Entity
                },
                lead: self.lead.take(),
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
                self.error(
                    aspan,
                    format!("the attribute {} is written twice", shown(aname)),
                );
            }
            attrs.push((
                aname,
                aspan,
                value,
                self.span(aspan.start as usize, self.pos),
            ));
            if !closed {
                break;
            }
        }
        let span = self.span(start, self.pos);
        if self.skip > 0 {
            self.faults += 1;
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
        for (aname, aspan, value, _) in &attrs {
            let prefix = if *aname == "xmlns" {
                ""
            } else if let Some(p) = aname.strip_prefix("xmlns:") {
                p
            } else {
                continue;
            };
            if let Some((at, message)) = self.reserved(prefix, *aspan, value) {
                self.error(at, message);
                continue;
            }
            if !prefix.is_empty() && value.text.is_empty() && !self.xml11 {
                self.error(
                    *aspan,
                    "an empty namespace undeclares a prefix only in XML 1.1",
                );
            }
            self.ns.push(prefix);
            self.scopes
                .entry(prefix)
                .or_default()
                .push(Rc::from(&*value.text));
        }
        let name = self.resolve(raw, name_span, false);
        let attrs: Vec<Attr<'a>> = attrs
            .into_iter()
            .map(|(aname, aspan, value, span)| Attr {
                name: self.resolve(aname, aspan, true),
                value,
                span,
            })
            .collect();
        self.one_expanded_name(&attrs);
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
                name: None,
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

    /// Namespaces in XML, 6.3: no two attributes of a tag have one
    /// namespace and local name. Two written alike are reported already.
    fn one_expanded_name(&mut self, attrs: &[Attr<'a>]) {
        fn key<'t>(a: &'t Attr<'_>) -> Option<(&'t str, &'t str)> {
            a.name.prefix_len?;
            Some((a.name.ns()?, a.name.local()))
        }
        // A map once a tag has many attributes, so a long tag stays linear.
        let mut seen: FxHashMap<(&str, &str), &str> = FxHashMap::default();
        for (i, a) in attrs.iter().enumerate().skip(1) {
            let Some(k) = key(a) else { continue };
            let first = if attrs.len() <= 16 {
                attrs[..i]
                    .iter()
                    .find(|b| key(b) == Some(k))
                    .map(|b| b.name.raw)
            } else {
                if seen.is_empty() {
                    seen.extend(
                        attrs[..1]
                            .iter()
                            .filter_map(|b| Some((key(b)?, b.name.raw))),
                    );
                }
                match seen.get(&k) {
                    Some(&first) => Some(first),
                    None => {
                        seen.insert(k, a.name.raw);
                        None
                    }
                }
            };
            if let Some(first) = first.filter(|&f| f != a.name.raw) {
                self.error(
                    a.name.span,
                    format!(
                        "the attributes {} and {} have one namespace and local name",
                        shown(first),
                        shown(a.name.raw)
                    ),
                );
            }
        }
    }

    /// Namespaces in XML 1.0, 3: why the declaration of `prefix` (`""` for
    /// the default namespace) to `value` is not applied, and where. `xml`
    /// is bound to its namespace only, and no other prefix to it; `xmlns`
    /// is never declared, and its namespace is bound to nothing.
    fn reserved(
        &self,
        prefix: &str,
        name: Span,
        value: &Value<'_>,
    ) -> Option<(Span, &'static str)> {
        let uri: &str = &value.text;
        if prefix == "xmlns" {
            Some((name, "the prefix xmlns is never declared"))
        } else if prefix == "xml" {
            (uri != XML_NS).then_some((
                value.span,
                "the prefix xml is bound to http://www.w3.org/XML/1998/namespace only",
            ))
        } else if uri == XML_NS {
            Some((value.span, "only the prefix xml is bound to this namespace"))
        } else if uri == XMLNS_NS {
            Some((value.span, "no prefix is bound to the xmlns namespace"))
        } else {
            None
        }
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
            entity: !self.frames.is_empty(),
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
                self.span(start, self.pos),
                "the end tag does not end with `>`",
            );
            while self.peek().is_some_and(|c| c != b'>' && c != b'<') {
                self.pos += 1;
            }
            if self.peek() == Some(b'>') {
                self.pos += 1;
            }
        }
        let span = self.span(start, self.pos);
        if self.skip > 0 {
            self.skip -= 1;
            return None;
        }
        // In an entity, only an element it opened.
        let base = self.frames.last().map_or(0, |f| f.depth);
        let Some(k) = self.stack[base..]
            .iter()
            .rposition(|o| o.raw == raw)
            .map(|k| base + k)
        else {
            self.error(
                if raw.is_empty() { span } else { name_span },
                format!("</{}> closes no open element", shown(raw)),
            );
            return None;
        };
        // The elements inside the one it closes are unclosed.
        let here = self.span(start, start);
        while self.stack.len() > k + 1 {
            let open = self.stack.pop().expect("deeper than k");
            self.error(
                open.name_span,
                format!(
                    "<{}> is not closed before </{}>",
                    shown(open.raw),
                    shown(raw)
                ),
            );
            self.unscope(open.ns_len);
            self.pending.push_back(Event::End(End {
                span: here,
                implied: true,
                name: None,
            }));
        }
        let open = self.stack.pop().expect("k is open");
        self.unscope(open.ns_len);
        self.pending.push_back(Event::End(End {
            span,
            implied: false,
            name: Some(name_span),
        }));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn events(text: &str) -> (Vec<String>, Vec<SyntaxError>) {
        let prolog = Prolog::read(text);
        let mut s = Scanner::new(text, &prolog);
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
                Event::Pi(_, pi) => out.push(pi.into()),
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
        let prolog = Prolog::read(text);
        let mut s = Scanner::new(text, &prolog);
        let Event::Start(t) = s.next() else { panic!() };
        assert_eq!(t.attrs[0].value.text, "http://example.org/x");
        assert_eq!(t.attrs[0].value.refs, Refs::Entity);
        let v = &t.attrs[0].value;
        assert_eq!(&text[v.span.start as usize..v.span.end as usize], "&ex;x");
        // The entity the value starts with, and how much of the text it gives.
        let (at, len) = v.lead.unwrap();
        assert_eq!(&text[at.start as usize..at.end as usize], "&ex;");
        assert_eq!(len as usize, "http://example.org/".len());
        assert_eq!(t.attrs[1].value.text, "<Ahttp://example.org/two%");
        // A predefined entity first is no declared entity.
        assert_eq!(t.attrs[1].value.lead, None);
        let Event::Text { text: body, .. } = s.next() else {
            panic!()
        };
        assert_eq!(body, "&http://example.org/");
        assert_eq!(
            s.next(),
            Event::End(End {
                span: Span::new(text.len() as u32 - 4, text.len() as u32),
                implied: false,
                name: Some(Span::new(text.len() as u32 - 2, text.len() as u32 - 1)),
            })
        );
        assert_eq!(s.next(), Event::Eof);
        assert!(s.finish().0.is_empty());
    }

    #[test]
    fn namespaces_resolve_in_scope() {
        let text = "<r xmlns='http://example.org/d#' xmlns:p='http://example.org/p#'><p:a p:x='1' y='2'><b xmlns:p='http://example.org/q#' p:z=''/></p:a><q:c/></r>";
        let prolog = Prolog::read(text);
        let mut s = Scanner::new(text, &prolog);
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

    /// The errors of a scan: their message and the text at their span.
    fn errors_at(text: &str) -> Vec<(String, &str)> {
        events(text)
            .1
            .iter()
            .map(|e| {
                let s = e.span;
                (
                    e.message().to_string(),
                    &text[s.start as usize..s.end as usize],
                )
            })
            .collect()
    }

    /// Namespaces in XML 1.0, 3 (No Prefix Undeclaring): a prefixed
    /// declaration is not empty. XML 1.1 (Namespaces in XML 1.1) allows it,
    /// and an empty default namespace is allowed in both. Either way the
    /// prefix is undeclared.
    #[test]
    fn an_empty_prefixed_namespace_is_an_error_in_xml_1_0() {
        const BODY: &str =
            "<r xmlns:ex='http://example.org/'><a xmlns:ex=''><ex:b/></a><c xmlns=''/></r>";
        let undeclare = "an empty namespace undeclares a prefix only in XML 1.1";
        for decl in [
            "",
            "<?xml version=\"1.0\"?>",
            "<?xml version='1.0' encoding='UTF-8'?>",
        ] {
            let text = format!("{decl}{BODY}");
            assert_eq!(
                errors_at(&text),
                [(undeclare.to_string(), "xmlns:ex")],
                "{decl}"
            );
        }
        for decl in ["<?xml version=\"1.1\"?>", "\u{feff}<?xml version='1.1'?>"] {
            let text = format!("{decl}{BODY}");
            assert_eq!(errors_at(&text), [], "{decl}");
        }
        // Undeclared in both: the name has no namespace.
        let text = format!("<?xml version=\"1.1\"?>{BODY}");
        let prolog = Prolog::read(&text);
        let mut s = Scanner::new(&text, &prolog);
        let mut names = Vec::new();
        loop {
            match s.next() {
                Event::Eof => break,
                Event::Start(t) => names.push((t.name.raw, t.name.ns().map(str::to_string))),
                _ => {}
            }
        }
        assert_eq!(
            names,
            [("r", None), ("a", None), ("ex:b", None), ("c", None)]
        );
    }

    /// Namespaces in XML 1.0, 3 (Reserved Prefixes and Namespace Names):
    /// `xml` is bound to its namespace only, which no other prefix (nor
    /// the default) takes; `xmlns` is never declared, and its namespace is
    /// bound to nothing. Such a declaration is not applied.
    #[test]
    fn reserved_namespaces_are_bound_only_to_their_prefixes() {
        const XML: &str = "http://www.w3.org/XML/1998/namespace";
        const XMLNS: &str = "http://www.w3.org/2000/xmlns/";
        // Protégé declares `xml` to its own namespace: allowed.
        assert_eq!(
            errors_at(&format!("<r xmlns:xml='{XML}' xml:lang='en'/>")),
            []
        );
        for (decl, at) in [
            (format!("xmlns:x='{XML}'"), XML),
            (format!("xmlns='{XML}'"), XML),
            (
                "xmlns:xml='http://example.org/'".to_string(),
                "http://example.org/",
            ),
            (format!("xmlns:x='{XMLNS}'"), XMLNS),
            (format!("xmlns='{XMLNS}'"), XMLNS),
            (format!("xmlns:xmlns='{XMLNS}'"), "xmlns:xmlns"),
            (
                "xmlns:xmlns='http://example.org/'".to_string(),
                "xmlns:xmlns",
            ),
        ] {
            let text = format!("<r {decl}/>");
            let errors = errors_at(&text);
            assert_eq!(errors.len(), 1, "{decl}: {errors:?}");
            assert_eq!(errors[0].1, at, "{decl}");
        }
        // Not applied: `x:lang` is in no namespace, `xml:lang` stays XML's.
        let text = format!(
            "<r xmlns:x='{XML}' xmlns:xml='http://example.org/' x:lang='en' xml:lang='fr'/>"
        );
        let prolog = Prolog::read(&text);
        let mut s = Scanner::new(&text, &prolog);
        let Event::Start(t) = s.next() else {
            panic!("a start tag")
        };
        let ns: Vec<_> = t
            .attrs
            .iter()
            .map(|a| (a.name.raw, a.name.ns().map(str::to_string)))
            .collect();
        assert_eq!(
            ns[2..],
            [("x:lang", None), ("xml:lang", Some(XML.to_string()))]
        );
    }

    /// XML 1.0, 2.8: the XML declaration is the first thing in the
    /// document, after a byte order mark at most.
    #[test]
    fn the_xml_declaration_is_at_the_start() {
        for text in [
            "<?xml version=\"1.0\"?><r/>",
            "\u{feff}<?xml version=\"1.0\"?><r/>",
        ] {
            assert_eq!(errors_at(text), [], "{text:?}");
        }
        for text in [
            "\n\n\n<?xml version=\"1.0\"?><r/>",
            "   <?xml version=\"1.0\"?><r/>",
            " <?xml version=\"1.0\"?><r/>",
            "\u{feff} <?xml version=\"1.0\"?><r/>",
            "<!-- c --><?xml version=\"1.0\"?><r/>",
        ] {
            let errors = errors_at(text);
            assert_eq!(
                errors,
                [(
                    "the XML declaration is not at the start of the document".to_string(),
                    "<?xml"
                )],
                "{text:?}"
            );
        }
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
        let prolog = Prolog::read(&decl);
        let mut s = Scanner::new(&decl, &prolog);
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

    /// A run of references to a markup entity is read in time linear in
    /// its length, in the document and in a replacement text (each return
    /// from an entity searched the rest of the run for `<`: 160,000
    /// references took 8 s).
    #[test]
    fn timing_markup_entity_runs_scan_in_linear_time() {
        const N: usize = 160_000;
        let text = format!(
            "<!DOCTYPE r [<!ENTITY m \"<q>v</q>\">]><r>{}</r>",
            "&m;".repeat(N)
        );
        let inner = format!(
            "<!DOCTYPE r [<!ENTITY m \"<q>v</q>\"><!ENTITY run \"{}\">]><r>{}</r>",
            "&m;".repeat(4000),
            "&run;".repeat(4)
        );
        for (text, starts) in [(&text, N + 1), (&inner, 4 * 4000 + 1)] {
            let started = Instant::now();
            let (ev, errors) = events(text);
            let took = started.elapsed();
            eprintln!("{} bytes in {took:?}", text.len());
            assert!(errors.is_empty(), "{:?}", errors.first());
            assert_eq!(
                ev.iter().filter(|e| e.starts_with("<q")).count() + 1,
                starts
            );
            assert!(took < Duration::from_millis(500), "{took:?}");
        }
    }

    /// A reference inside a CDATA section, a comment or a processing
    /// instruction of a replacement text is no reference when the entity
    /// is read as markup: such an entity does not refer to itself. As data
    /// (in an attribute value) it does, and is reported there.
    #[test]
    fn markup_in_a_replacement_text_hides_its_references() {
        let text = "<!DOCTYPE r [<!ENTITY note \"<p><![CDATA[&note;]]></p>\"><!ENTITY c \"<!-- &c; --><p>x</p>\"><!ENTITY pi \"<?t &pi; ?><p>y</p>\">]><r>&note;&c;&pi;</r>";
        let (ev, errors) = events(text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            ev,
            [
                "<r>",
                "<p>",
                "\"&note;\"",
                "</>",
                "<p>",
                "\"x\"",
                "</>",
                "<?t &pi; ?>",
                "<p>",
                "\"y\"",
                "</>",
                "</>"
            ]
        );
        let text = "<!DOCTYPE r [<!ENTITY note \"<p><![CDATA[&note;]]></p>\">]><r a='&note;'/>";
        let (ev, errors) = events(text);
        assert_eq!(ev[0], "<r a=\"<p><![CDATA[&note;]]></p>\"/>");
        assert_eq!(
            messages(&errors),
            [
                "the entity &note; puts `<` in an attribute value",
                "the entity &note; refers to itself: it is not expanded"
            ]
        );
        // The walk reads a quoted `<!--` as a comment, the scanner ends the
        // declaration at the `>` before it: the reference met inside its
        // own expansion is reported once and kept.
        let text = "<!DOCTYPE r [<!ENTITY a \"<!X '<!--' >&a;-->\">]><r>&a;</r>";
        let (ev, errors) = events(text);
        assert_eq!(ev, ["<r>", "\"&a;-->\"", "</>"]);
        assert_eq!(
            messages(&errors),
            [
                "unknown markup after `<!`",
                "the entity &a; refers to itself: it is not expanded"
            ]
        );
    }

    /// Every error moves the fault count, past the cap of reported ones too.
    #[test]
    fn the_fault_count_moves_past_the_error_cap() {
        let text = format!("<r>{}</r>", "& ".repeat(150));
        let prolog = Prolog::read(&text);
        let mut s = Scanner::new(&text, &prolog);
        assert_eq!(s.faults(), 0);
        while s.next() != Event::Eof {}
        assert_eq!(s.faults(), 150);
        let (errors, _) = s.finish();
        assert_eq!(errors.len(), MAX_ERRORS);
        // An error in the prolog is a fault before the root element.
        let text = "<!DOCTYPE r [ x ]><r/>";
        let prolog = Prolog::read(text);
        let s = Scanner::new(text, &prolog);
        assert_eq!(s.faults(), 1);
    }

    /// Past the expansion limit, each reference left as written is a fault,
    /// counted while the event that holds it is read.
    #[test]
    fn the_fault_count_moves_for_each_reference_left_unexpanded() {
        let value = "v".repeat(60_000);
        let text = format!(
            "<!DOCTYPE r [<!ENTITY e \"{value}\">]><r>{}</r>",
            "<x>&e;</x>".repeat(40)
        );
        let prolog = Prolog::read(&text);
        let mut s = Scanner::new(&text, &prolog);
        let (mut expanded, mut kept) = (0, 0);
        loop {
            let before = s.faults();
            match s.next() {
                Event::Eof => break,
                Event::Text { text, .. } if text == "&e;" => {
                    kept += 1;
                    assert_eq!(s.faults(), before + 1);
                }
                Event::Text { text, .. } => {
                    assert_eq!(text, value);
                    expanded += 1;
                    assert_eq!(s.faults(), before);
                }
                _ => assert_eq!(s.faults(), before),
            }
        }
        assert!(expanded > 0 && kept > 0, "{expanded} {kept}");
        assert_eq!(expanded + kept, 40);
        let (errors, _) = s.finish();
        assert_eq!(
            messages(&errors),
            ["entity expansion is over its limit: the rest of the references are not expanded"]
        );
    }

    /// An element too deep is a fault, and so is each element inside it.
    #[test]
    fn the_fault_count_moves_for_a_skipped_element() {
        let text = format!(
            "{}<b><c/><c>t</c></b>{}",
            "<a>".repeat(MAX_DEPTH),
            "</a>".repeat(MAX_DEPTH)
        );
        let prolog = Prolog::read(&text);
        let mut s = Scanner::new(&text, &prolog);
        let mut starts = 0;
        loop {
            let before = s.faults();
            match s.next() {
                Event::Eof => break,
                Event::Start(_) => starts += 1,
                // The innermost `<a>` ends after the skipped elements.
                Event::End(_) if starts == MAX_DEPTH => {
                    assert_eq!(s.faults(), before + 3);
                    starts += 1;
                }
                _ => assert_eq!(s.faults(), before),
            }
        }
        assert_eq!(s.faults(), 3);
        assert_eq!(s.finish().0.len(), 1);
    }

    /// An error quotes at most the first characters of a name, so a long
    /// name quoted in many errors costs bytes linear in the text: the
    /// first of two attributes with one expanded name, and the end tag
    /// that closes many elements.
    #[test]
    fn an_error_quotes_a_long_name_cut() {
        let long = "p".repeat(100_000);
        let others: String = (0..120)
            .map(|i| format!(" xmlns:b{i}=\"http://example.org/\" b{i}:x=\"\""))
            .collect();
        let duplicates = format!("<r xmlns:{long}=\"http://example.org/\" {long}:x=\"\"{others}/>");
        let nested = format!("<{long}>{}</{long}>", "<a>".repeat(120));
        for text in [duplicates, nested] {
            let (_, errors) = events(&text);
            assert!(errors.len() >= 90, "{:?}", messages(&errors));
            let bytes: usize = errors.iter().map(|e| e.message().len()).sum();
            assert!(bytes <= text.len(), "{bytes} bytes of messages");
            assert!(messages(&errors).iter().any(|m| m.contains("ppp…")));
        }
    }

    /// A document in ASCII means the same in any encoding that extends
    /// ASCII, so declaring one is no error; other declarations are.
    #[test]
    fn an_ascii_document_may_declare_an_encoding_that_extends_ascii() {
        for enc in ["ISO-8859-1", "us-ascii", "windows-1252", "iso-8859-15"] {
            let text = format!("<?xml version=\"1.0\" encoding=\"{enc}\"?><r>x</r>");
            let (ev, errors) = events(&text);
            assert!(errors.is_empty(), "{enc}: {errors:?}");
            assert_eq!(ev, ["<r>", "\"x\"", "</>"]);
        }
        for text in [
            "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><r>é</r>",
            "<?xml version=\"1.0\" encoding=\"UTF-16\"?><r>x</r>",
            "<?xml version=\"1.0\" encoding=\"Shift_JIS\"?><r>x</r>",
        ] {
            let (_, errors) = events(text);
            assert_eq!(
                messages(&errors),
                ["only UTF-8 is read: the text is read as UTF-8"],
                "{text}"
            );
        }
    }
}
