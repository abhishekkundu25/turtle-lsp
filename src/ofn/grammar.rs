//! The productions of OWL 2 Functional-Style Syntax (section 13 of the
//! structural specification) over [`super::lexer`], into a [`Document`].
//!
//! Each construct is `Keyword(` its arguments `)`, and each keyword has one
//! signature ([`signature`]): a list of argument slots, each a category
//! (class expression, data range, individual, …) taken once, at most once,
//! or at least n times. The descent is bounded ([`MAX_NESTING`]); past it
//! the axiom is an error. After an error the axiom is abandoned and the
//! parse goes on after it ([`Parser::recover`]): at the `)` that closes it,
//! or at the next axiom keyword followed by `(` when it is not closed, so
//! one typo costs one error.
//!
//! Besides the grammar, what the specification states that a parser sees
//! without reasoning is checked:
//! - an abbreviated IRI's prefix is declared (or is one of `rdf:`, `rdfs:`,
//!   `xsd:` and `owl:`, Table 2), and its expansion is an IRI (section 3.7);
//! - a full IRI is an absolute IRI (section 2.4);
//! - a prefix name is declared at most once, or again with the same IRI
//!   (section 3.7);
//! - the arity of each construct: at least two operands of
//!   `ObjectIntersectionOf`, `ObjectUnionOf`, `DataIntersectionOf`,
//!   `DataUnionOf`, `EquivalentClasses`, `DisjointClasses`, `DisjointUnion`,
//!   `ObjectPropertyChain`, `EquivalentObjectProperties`,
//!   `DisjointObjectProperties`, `EquivalentDataProperties`,
//!   `DisjointDataProperties`, `SameIndividual`, `DifferentIndividuals`; at
//!   least one of `ObjectOneOf`, `DataOneOf`, the facets of
//!   `DatatypeRestriction`, and the data properties of `DataSomeValuesFrom`
//!   and `DataAllValuesFrom` (section 13);
//! - a cardinality is a non-negative integer (section 13);
//! - a `HasKey` names at least one property (section 9.5);
//! - `Import`s come before the ontology annotations, and those before the
//!   axioms (section 3.7);
//! - a language tag is a BCP 47 tag, and a string holds only XML
//!   characters and the escapes `\"` and `\\` (section 2.3).

use super::lexer::{Kind, Lexer, Token};
use super::signature::{Cat, Rep, is_axiom, keyword_in, signature};
use super::tree::{Document, Keyword, Node, NodeKind};
use super::{Errors, shown};
use crate::syntax::{Iri, PrefixDecl, Span, has_scheme};
use rustc_hash::FxHashMap;

/// How deep constructs nest before the axiom is an error.
pub(crate) const MAX_NESTING: u32 = 256;

/// The prefix names of Table 2, which every document has.
const STANDARD: [(&str, &str); 4] = [
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
];

/// Parse `text`.
pub(crate) fn parse(text: &str) -> Document {
    let mut p = Parser::new(text);
    p.document();
    let mut doc = p.doc;
    doc.comments = p.lexer.comments;
    doc.errors = p.errors.into_vec();
    doc
}

/// The current item is abandoned; the error that says why is reported.
struct Stop;

type R<T = ()> = Result<T, Stop>;

/// Whether a token of `kind` (not a keyword) starts one of `cat`.
fn token_in(kind: Kind, cat: Cat) -> bool {
    let iri = matches!(kind, Kind::FullIri | Kind::PName);
    match cat {
        Cat::Iri(_)
        | Cat::DataProperty
        | Cat::ClassExpression
        | Cat::ObjectProperty
        | Cat::SubObjectProperty
        | Cat::DataRange
        | Cat::DataPropertiesThenRange
        | Cat::Facets => iri,
        Cat::Individual | Cat::AnnotationSubject | Cat::IArg => iri || kind == Kind::NodeId,
        Cat::AnnotationValue => iri || matches!(kind, Kind::NodeId | Kind::String),
        Cat::Literal | Cat::DArg => kind == Kind::String,
        Cat::Integer => kind == Kind::Integer,
        Cat::ObjectKeys | Cat::DataKeys => kind == Kind::LParen,
        Cat::Annotation | Cat::Entity | Cat::Body | Cat::Head | Cat::Atom => false,
    }
}

/// A declared prefix: its IRI as written, whether it is one, and whether
/// the document declares it (a Table 2 name is bound before).
#[derive(Clone, Copy)]
struct Prefix<'a> {
    iri: &'a str,
    valid: bool,
    declared: bool,
    /// What its expansions are checked after: see [`context`].
    context: &'static str,
}

/// Where the items of the ontology are.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Imports,
    Annotations,
    Axioms,
}

struct Parser<'a> {
    text: &'a str,
    lexer: Lexer<'a>,
    errors: Errors,
    tok: Token,
    ahead: Option<Token>,
    /// The span of the token before `tok`.
    prev: Span,
    /// Whether the token before `tok` was malformed.
    prev_bad: bool,
    /// `(` read minus `)` read: where recovery stops.
    parens: u32,
    nesting: u32,
    doc: Document,
    prefixes: FxHashMap<&'a str, Prefix<'a>>,
    /// Why each IRI written so far is not one (`None`: it is), by its text.
    checked: FxHashMap<&'a str, Option<Box<str>>>,
    /// The IRI being checked for an expansion: its prefix's context and
    /// the local name ([`context`]).
    buf: String,
    /// The text after the ontology is reported once.
    trailing: bool,
    /// The item being read used a prefix whose declaration is in error:
    /// its IRIs are not known.
    tainted: bool,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Parser<'a> {
        let mut lexer = Lexer::new(text);
        let mut errors = Errors::default();
        let tok = lexer.next(&mut errors);
        let prefixes = STANDARD
            .iter()
            .map(|&(name, iri)| {
                let prefix = Prefix {
                    iri,
                    valid: true,
                    declared: false,
                    context: context(iri),
                };
                (name, prefix)
            })
            .collect();
        Parser {
            text,
            lexer,
            errors,
            tok,
            ahead: None,
            prev: Span::new(tok.start, tok.start),
            prev_bad: false,
            parens: 0,
            nesting: 0,
            doc: Document::default(),
            prefixes,
            checked: FxHashMap::default(),
            buf: String::new(),
            trailing: false,
            tainted: false,
        }
    }

    // ---- tokens ----

    fn text_of(&self, t: Token) -> &'a str {
        &self.text[t.start as usize..t.end as usize]
    }

    fn bump(&mut self) {
        match self.tok.kind {
            Kind::LParen => self.parens += 1,
            Kind::RParen => self.parens = self.parens.saturating_sub(1),
            _ => {}
        }
        self.prev = self.tok.span();
        self.prev_bad = self.tok.kind == Kind::Bad;
        self.tok = match self.ahead.take() {
            Some(t) => t,
            None => self.lexer.next(&mut self.errors),
        };
    }

    /// The token after `tok`.
    fn peek(&mut self) -> Token {
        match self.ahead {
            Some(t) => t,
            None => {
                let t = self.lexer.next(&mut self.errors);
                self.ahead = Some(t);
                t
            }
        }
    }

    /// The keyword `tok` is, if it is one.
    fn keyword(&self) -> Option<Keyword> {
        (self.tok.kind == Kind::Keyword)
            .then(|| Keyword::of(self.text_of(self.tok)))
            .flatten()
    }

    /// Whether `tok` starts an item a broken one cannot hold: an axiom,
    /// `Import`, `Prefix` or `Ontology` keyword followed by `(`.
    fn at_item(&mut self) -> bool {
        let item =
            |k| is_axiom(k) || matches!(k, Keyword::Import | Keyword::Prefix | Keyword::Ontology);
        self.keyword().is_some_and(item) && self.peek().kind == Kind::LParen
    }

    /// How `tok` is named in an error.
    fn found(&self) -> String {
        let t = self.tok;
        match t.kind {
            Kind::Eof => "the end of the document".into(),
            Kind::String => "a string".into(),
            Kind::LangTag => "a language tag".into(),
            _ => format!("'{}'", shown(self.text_of(t))),
        }
    }

    /// Report that `what` was expected at `tok`, and stop. Malformed text
    /// was reported by the lexer already, and so was an end of the document
    /// right after it (a string or IRI that is not closed); else the end is
    /// reported at the last token.
    fn expected<T>(&mut self, what: &str) -> R<T> {
        match self.tok.kind {
            Kind::Bad => {}
            Kind::Eof if self.prev_bad => {}
            Kind::Eof => {
                let message = format!("Expected {what}, found {}", self.found());
                self.errors.error(self.prev, message);
            }
            _ => {
                let message = format!("Expected {what}, found {}", self.found());
                self.errors.error(self.tok.span(), message);
            }
        }
        Err(Stop)
    }

    fn expect(&mut self, kind: Kind, what: &str) -> R {
        if self.tok.kind == kind {
            self.bump();
            Ok(())
        } else {
            self.expected(what)
        }
    }

    // ---- the document ----

    fn document(&mut self) {
        let mut reported = false;
        loop {
            match self.keyword() {
                Some(Keyword::Prefix) => self.prefix_declaration(),
                Some(Keyword::Ontology) => {
                    self.ontology();
                    break;
                }
                _ => {
                    if !reported {
                        reported = true;
                        let _ = self.expected::<()>("'Prefix(' or 'Ontology('");
                    }
                    if self.tok.kind == Kind::Eof || self.at_item() {
                        // Axioms without an ontology: read for their errors.
                        self.trailing = true;
                        self.items();
                        break;
                    }
                    self.bump();
                    self.recover(0);
                }
            }
        }
        if self.tok.kind != Kind::Eof {
            if !self.trailing {
                let message = format!(
                    "Expected the end of the document after the ontology, found {}",
                    self.found()
                );
                self.errors.error(self.tok.span(), message);
                self.trailing = true;
            }
            self.items();
        }
    }

    /// `Prefix( name: = <iri> )`.
    fn prefix_declaration(&mut self) {
        let start = self.tok.start;
        let level = self.parens;
        if self.prefix_body(start).is_err() {
            self.recover(level);
        }
    }

    fn prefix_body(&mut self, start: u32) -> R {
        self.bump();
        self.expect(Kind::LParen, "'(' after 'Prefix'")?;
        let name = self.tok;
        // SPARQL's PNAME_NS: one ':', at the end.
        let written = self.text_of(name);
        if name.kind != Kind::PName || written.find(':') != Some(written.len() - 1) {
            return self.expected("a prefix name such as 'ex:'");
        }
        self.bump();
        self.expect(Kind::Equals, "'='")?;
        let iri = self.tok;
        if iri.kind != Kind::FullIri {
            return self.expected("a full IRI such as <http://example.org/>");
        }
        self.bump();
        self.expect(Kind::RParen, "')' to close 'Prefix('")?;
        self.declare(name, iri, Span::new(start, self.prev.end));
        Ok(())
    }

    fn declare(&mut self, name: Token, iri: Token, span: Span) {
        let written = self.text_of(name);
        let prefix = &written[..written.len() - 1];
        let full = self.text_of(iri);
        let value = &full[1..full.len() - 1];
        let valid = self.check_iri(full, value, iri.span());
        // Only a valid binding is the document's: other files read these
        // as namespaces to offer, and an invalid IRI is no namespace.
        if valid {
            self.doc.prefixes.push(PrefixDecl {
                name: prefix.to_string(),
                iri: Iri::intern(value),
                span,
                name_span: name.span(),
                iri_span: iri.span(),
            });
        }
        let new = Prefix {
            iri: value,
            valid,
            declared: true,
            context: context(value),
        };
        match self.prefixes.get(prefix) {
            // Table 2's names are bound before; a declaration binds them
            // again, as OWL API writes them and as the W3C tests have it.
            Some(before) if !before.declared => {
                self.prefixes.insert(prefix, new);
            }
            Some(before) if before.iri != value => {
                let message = format!(
                    "The prefix '{written}' is declared twice: as <{}> before, and as <{value}> here",
                    shown(before.iri)
                );
                self.errors.error(name.span(), message);
            }
            Some(_) => {}
            None => {
                self.prefixes.insert(prefix, new);
            }
        }
    }

    /// `Ontology( [iri [version]] imports annotations axioms )`.
    fn ontology(&mut self) {
        let start = self.tok.start;
        self.bump();
        let opened = self.expect(Kind::LParen, "'(' after 'Ontology'").is_ok();
        if opened {
            let mut slots = 2;
            while slots > 0 && token_in(self.tok.kind, Cat::Iri(None)) {
                let node = self.header_iri();
                if slots == 2 {
                    self.doc.ontology_iri = node;
                } else {
                    self.doc.version_iri = node;
                }
                slots -= 1;
            }
            // An IRI not finished (`<…` to the line end) in a slot still
            // open is the header's IRI being typed: the header is broken,
            // not an anonymous ontology, or one with no version IRI.
            if slots > 0 && self.tok.kind == Kind::Bad && self.text_of(self.tok).starts_with('<') {
                self.doc.header_broken = true;
            }
        }
        let level = self.parens;
        self.items();
        if opened {
            if self.tok.kind == Kind::RParen && self.parens == level {
                self.bump();
            } else {
                let _ = self.expected::<()>("')' to close 'Ontology('");
            }
        }
        self.doc.ontology = Some(Span::new(start, self.prev.end));
    }

    /// The ontology IRI or the version IRI, `tok` being an IRI: its node, if
    /// it gives one. One that is not read whole marks the header broken.
    fn header_iri(&mut self) -> Option<usize> {
        let i = self.doc.nodes.len();
        let whole = self.whole(|p| p.iri());
        let pushed = self.doc.nodes.len() > i;
        if !whole || !pushed || self.doc.broken.last() == Some(&(i as u32)) {
            self.doc.header_broken = true;
        }
        pushed.then_some(i)
    }

    /// The items of the ontology, up to the `)` that closes it or the end:
    /// imports, ontology annotations and axioms; a stray `Prefix` too. After
    /// the ontology, or without one, the same are read for their errors.
    fn items(&mut self) {
        let level = self.parens;
        let mut phase = Phase::Imports;
        loop {
            let at = self.tok;
            match (at.kind, self.keyword()) {
                (Kind::Eof, _) => return,
                (Kind::RParen, _) if self.parens == level && level > 0 => return,
                (Kind::RParen, _) => {
                    // After the ontology: reported once already.
                    self.bump();
                }
                (Kind::Keyword, Some(Keyword::Prefix)) => {
                    self.errors
                        .error(at.span(), "A 'Prefix' declaration comes before 'Ontology('");
                    self.prefix_declaration();
                }
                (Kind::Keyword, Some(Keyword::Ontology)) => {
                    if !self.trailing {
                        let _ = self.expected::<()>("an axiom: a document holds one ontology");
                    }
                    self.bump();
                    if self.tok.kind == Kind::LParen {
                        self.bump();
                        self.recover(level);
                    }
                }
                (Kind::Keyword, Some(k @ (Keyword::Import | Keyword::Annotation))) => {
                    let here = if k == Keyword::Import {
                        Phase::Imports
                    } else {
                        Phase::Annotations
                    };
                    let misplaced = phase > here;
                    if misplaced {
                        let message = if k == Keyword::Import {
                            "An 'Import' comes before the ontology's annotations and axioms"
                        } else {
                            "An ontology annotation comes before the axioms"
                        };
                        self.errors.error(at.span(), message);
                    }
                    phase = phase.max(here);
                    let i = self.doc.nodes.len();
                    self.item(k);
                    // Its error is reported before it is read: it is an
                    // item with an error all the same.
                    if misplaced
                        && i < self.doc.nodes.len()
                        && self.doc.broken.last() != Some(&(i as u32))
                    {
                        self.doc.broken.push(i as u32);
                    }
                }
                (Kind::Keyword, Some(k)) if is_axiom(k) => {
                    phase = Phase::Axioms;
                    self.item(k);
                }
                _ => {
                    let _ = self.expected::<()>("an axiom, an annotation or ')'");
                    self.bump();
                    self.recover(level);
                }
            }
        }
    }

    /// One import, ontology annotation or axiom; after an error, the parse
    /// goes on after it.
    fn item(&mut self, k: Keyword) {
        let level = self.parens;
        if !self.whole(|p| p.construct(k)) {
            self.recover(level);
        }
    }

    /// Read one top-level node with `read`, and record it in
    /// [`Document::broken`] unless it was read whole: `read` stopped, or an
    /// error was reported inside the node (one past the limit too), or it
    /// used a prefix whose declaration is in error. An error after the node
    /// (the lexer reads a token ahead) does not count. Whether it was read
    /// to its end.
    fn whole(&mut self, read: impl FnOnce(&mut Self) -> R) -> bool {
        let i = self.doc.nodes.len();
        let outer = self.errors.restart();
        self.tainted = false;
        let r = read(self);
        let inside = self.errors.restart();
        self.errors.resume(inside.min(outer));
        let end = self.doc.nodes.get(i).map_or(0, |n| n.span.end);
        let bad = r.is_err() || inside < end || self.tainted;
        if bad && i < self.doc.nodes.len() {
            self.doc.broken.push(i as u32);
        }
        r.is_ok()
    }

    /// Skip what is left of an item that started at `level` parentheses:
    /// up to the `)` that closes it, the `)` that closes what holds it (not
    /// read), the next axiom keyword followed by `(` (its `)` is missing),
    /// or the end.
    fn recover(&mut self, level: u32) {
        loop {
            let kind = self.tok.kind;
            match kind {
                Kind::Eof => return,
                Kind::RParen if self.parens <= level => return,
                Kind::RParen => {
                    self.bump();
                    if self.parens == level {
                        return;
                    }
                }
                Kind::Keyword if self.at_item() => {
                    self.parens = level;
                    return;
                }
                _ => self.bump(),
            }
        }
    }

    // ---- constructs ----

    /// Open a node of `kind` at `start`; [`Parser::close`] ends it.
    fn open(&mut self, kind: NodeKind, start: u32) -> usize {
        self.doc.nodes.push(Node {
            kind,
            span: Span::new(start, start),
            next: 0,
        });
        self.doc.nodes.len() - 1
    }

    fn close(&mut self, i: usize) {
        let next = self.doc.nodes.len() as u32;
        let end = self.prev.end.max(self.doc.nodes[i].span.start);
        let node = &mut self.doc.nodes[i];
        node.span.end = end;
        node.next = next;
    }

    /// A leaf node at `tok`, which is read.
    fn leaf(&mut self, kind: NodeKind) {
        let span = self.tok.span();
        self.doc.nodes.push(Node {
            kind,
            span,
            next: self.doc.nodes.len() as u32 + 1,
        });
        self.bump();
    }

    /// `k( … )`, `tok` being `k`.
    fn construct(&mut self, k: Keyword) -> R {
        let keyword = self.tok.span();
        let i = self.open(NodeKind::Construct(k), keyword.start);
        self.bump();
        let r = if self.nesting >= MAX_NESTING {
            self.errors.error(
                keyword,
                format!("Nested more than {MAX_NESTING} levels deep"),
            );
            Err(Stop)
        } else {
            self.nesting += 1;
            let r = self.construct_body(k);
            self.nesting -= 1;
            r
        };
        self.close(i);
        r?;
        self.check(k, i);
        Ok(())
    }

    fn construct_body(&mut self, k: Keyword) -> R {
        if self.tok.kind != Kind::LParen {
            return self.expected(&format!("'(' after '{}'", k.as_str()));
        }
        self.bump();
        // The slot that may take more, named when `)` is not found.
        let mut open: Option<Cat> = None;
        for &(cat, rep) in signature(k) {
            match rep {
                Rep::One => {
                    self.arg(cat, k)?;
                    open = None;
                }
                Rep::Optional => {
                    if self.starts(cat) {
                        self.arg(cat, k)?;
                        open = None;
                    } else {
                        open = Some(cat);
                    }
                }
                Rep::AtLeast(_) => {
                    while self.starts(cat) {
                        self.arg(cat, k)?;
                    }
                    open = Some(cat);
                }
            }
        }
        if self.tok.kind != Kind::RParen {
            let what = match open {
                Some(cat) => format!("{} or ')'", cat.name()),
                None => format!("')' to close '{}('", k.as_str()),
            };
            return self.expected(&what);
        }
        self.bump();
        Ok(())
    }

    /// Whether `tok` starts one of `cat`.
    fn starts(&self, cat: Cat) -> bool {
        match self.keyword() {
            Some(k) => keyword_in(k, cat),
            None => token_in(self.tok.kind, cat),
        }
    }

    /// One argument of `cat` in a `k`.
    fn arg(&mut self, cat: Cat, k: Keyword) -> R {
        match cat {
            Cat::DataPropertiesThenRange => return self.properties_then_range(k),
            Cat::Facets => return self.facets(),
            Cat::ObjectKeys => return self.group(Cat::ObjectProperty),
            Cat::DataKeys => return self.group(Cat::DataProperty),
            _ => {}
        }
        if let Some(kw) = self.keyword()
            && keyword_in(kw, cat)
        {
            return self.construct(kw);
        }
        match cat {
            _ if !token_in(self.tok.kind, cat) => self.expected(cat.name()),
            Cat::Literal | Cat::DArg => self.literal(),
            Cat::Integer => {
                self.leaf(NodeKind::Integer);
                Ok(())
            }
            Cat::AnnotationValue if self.tok.kind == Kind::String => self.literal(),
            _ if self.tok.kind == Kind::NodeId => {
                self.leaf(NodeKind::NodeId);
                Ok(())
            }
            _ => self.iri(),
        }
    }

    /// An IRI, full or abbreviated, `tok` being one.
    fn iri(&mut self) -> R {
        let t = self.tok;
        let written = self.text_of(t);
        match t.kind {
            Kind::FullIri => {
                self.check_iri(written, &written[1..written.len() - 1], t.span());
                self.leaf(NodeKind::Iri { prefix_len: None });
            }
            Kind::PName => {
                let colon = written.find(':').unwrap_or(written.len());
                let (prefix, local) = (&written[..colon], &written[colon + 1..]);
                if local.is_empty() {
                    self.errors.error(
                        t.span(),
                        format!(
                            "Expected an IRI, found the prefix name '{written}': an abbreviated IRI has a local name after the ':'"
                        ),
                    );
                    return Err(Stop);
                }
                match self.prefixes.get(prefix).copied() {
                    None => self
                        .errors
                        .error(t.span(), format!("Undeclared prefix '{prefix}:'")),
                    Some(p) if p.valid => self.check_expansion(written, p.iri, local, t.span()),
                    // Its declaration is reported; the IRI is not known.
                    Some(_) => self.tainted = true,
                }
                self.leaf(NodeKind::Iri {
                    prefix_len: Some(colon as u32),
                });
            }
            _ => return self.expected(Cat::Iri(None).name()),
        }
        Ok(())
    }

    /// Whether `value` (written as `written`) is an absolute IRI; else
    /// report it at `span`. Each text is checked once.
    fn check_iri(&mut self, written: &'a str, value: &str, span: Span) -> bool {
        let why = self
            .checked
            .entry(written)
            .or_insert_with(|| why_not_iri(value))
            .clone();
        match why {
            None => true,
            Some(why) => {
                self.errors.error(span, why.into_string());
                false
            }
        }
    }

    /// Whether `written`, `prefix` (a valid IRI) followed by `local`, is
    /// an IRI; else report it at `span`. Only the local name is read, after
    /// the prefix's [`context`], so the time is that of `local`, however
    /// long the prefix.
    fn check_expansion(&mut self, written: &'a str, prefix: &str, local: &str, span: Span) {
        if let Some(why) = self.checked.get(written) {
            if let Some(why) = why.clone() {
                self.errors.error(span, why.into_string());
            }
            return;
        }
        let name = &written[..written.len() - local.len() - 1];
        let context = self.prefixes.get(name).map_or("a:", |p| p.context);
        self.buf.clear();
        self.buf.push_str(context);
        self.buf.push_str(local);
        let why = why_not_iri(&self.buf).map(|why| {
            // The expansion as `shown` cuts it, from the first characters
            // of the prefix only.
            let mut full: String = prefix.chars().take(61).collect();
            full.push_str(local);
            format!("'{written}' expands to <{}>: {why}", shown(&full)).into_boxed_str()
        });
        if let Some(why) = &why {
            self.errors.error(span, why.to_string());
        }
        self.checked.insert(written, why);
    }

    /// `"…"`, `"…"@tag` or `"…"^^datatype`, `tok` being the string.
    fn literal(&mut self) -> R {
        let string = self.tok.span();
        let i = self.open(
            NodeKind::Literal {
                string_end: string.end,
            },
            string.start,
        );
        self.bump();
        let r = match self.tok.kind {
            Kind::Carets => {
                self.bump();
                if token_in(self.tok.kind, Cat::Iri(None)) {
                    self.iri()
                } else {
                    self.expected("a datatype IRI after '^^'")
                }
            }
            Kind::LangTag => {
                self.bump();
                Ok(())
            }
            _ => Ok(()),
        };
        self.close(i);
        r
    }

    /// `( {property} )` of `HasKey`.
    fn group(&mut self, cat: Cat) -> R {
        if self.tok.kind != Kind::LParen {
            let what = if cat == Cat::ObjectProperty {
                Cat::ObjectKeys
            } else {
                Cat::DataKeys
            };
            return self.expected(what.name());
        }
        let i = self.open(NodeKind::Group, self.tok.start);
        self.bump();
        let r = (|| {
            while self.starts(cat) {
                self.arg(cat, Keyword::HasKey)?;
            }
            if self.tok.kind != Kind::RParen {
                return self.expected(&format!("{} or ')'", cat.name()));
            }
            self.bump();
            Ok(())
        })();
        self.close(i);
        r
    }

    /// `DataPropertyExpression { DataPropertyExpression } DataRange` of
    /// `DataSomeValuesFrom` and `DataAllValuesFrom`: an IRI can be either,
    /// so the last argument is the range and the others are properties.
    fn properties_then_range(&mut self, k: Keyword) -> R {
        let first = self.doc.nodes.len();
        while self.starts(Cat::DataRange) {
            self.arg(Cat::DataRange, k)?;
        }
        let args: Vec<usize> = {
            let mut at = first;
            std::iter::from_fn(|| {
                (at < self.doc.nodes.len()).then(|| {
                    let here = at;
                    at = self.doc.nodes[here].next as usize;
                    here
                })
            })
            .collect()
        };
        match args.split_last() {
            None => self.expected(Cat::DataProperty.name()),
            Some((&range, [])) => {
                let span = self.doc.nodes[range].span;
                self.errors.error(
                    span,
                    format!("'{}' needs a data property and a data range", k.as_str()),
                );
                Ok(())
            }
            Some((_, properties)) => {
                for &p in properties {
                    let node = self.doc.nodes[p];
                    if let NodeKind::Construct(c) = node.kind {
                        self.errors.error(
                            node.span,
                            format!(
                                "Expected a data property, found '{}': only the last argument of '{}' is a data range",
                                c.as_str(),
                                k.as_str()
                            ),
                        );
                    }
                }
                Ok(())
            }
        }
    }

    /// `constrainingFacet restrictionValue { … }` of `DatatypeRestriction`.
    fn facets(&mut self) -> R {
        if !token_in(self.tok.kind, Cat::Iri(None)) {
            return self.expected("a facet (an IRI such as xsd:minInclusive)");
        }
        while token_in(self.tok.kind, Cat::Iri(None)) {
            self.iri()?;
            if self.tok.kind != Kind::String {
                return self.expected("a literal (the facet's value)");
            }
            self.literal()?;
        }
        Ok(())
    }

    /// What the grammar's repetition cannot say: the least number of
    /// arguments, and a key's properties.
    fn check(&mut self, k: Keyword, i: usize) {
        let counted = signature(k)
            .iter()
            .any(|&(_, rep)| matches!(rep, Rep::AtLeast(n) if n > 0));
        if !counted && k != Keyword::HasKey {
            return;
        }
        let span = self.doc.nodes[i].span;
        let children: Vec<usize> = {
            let end = self.doc.nodes[i].next as usize;
            let mut at = i + 1;
            std::iter::from_fn(|| {
                (at < end).then(|| {
                    let here = at;
                    at = self.doc.nodes[here].next as usize;
                    here
                })
            })
            .collect()
        };
        let is_annotation = |n: &Node| n.kind == NodeKind::Construct(Keyword::Annotation);
        let operands = children
            .iter()
            .filter(|&&c| !is_annotation(&self.doc.nodes[c]))
            .count();
        for &(cat, rep) in signature(k) {
            let Rep::AtLeast(n) = rep else { continue };
            if n == 0 {
                continue;
            }
            // The other slots of these signatures take one argument each.
            let fixed = signature(k)
                .iter()
                .filter(|(c, r)| *c != Cat::Annotation && *r == Rep::One)
                .count();
            let count = operands.saturating_sub(fixed);
            if count < n as usize {
                let message = format!(
                    "'{}' needs at least {}, found {count}",
                    k.as_str(),
                    cat.counted(n)
                );
                self.errors.error(span, message);
            }
        }
        if k == Keyword::HasKey {
            let keys: usize = children
                .iter()
                .filter(|&&c| self.doc.nodes[c].kind == NodeKind::Group)
                .map(|&c| self.doc.nodes[c].next as usize - c - 1)
                .sum();
            if keys == 0 {
                self.errors.error(
                    span,
                    "'HasKey' needs at least one object or data property expression",
                );
            }
        }
    }
}

/// A short IRI that ends in the same part of an IRI as `ns`, a valid
/// IRI: `ns` followed by a local name is an IRI if and only if the context
/// followed by it is one, and with the same error. A local name (SPARQL's
/// `PN_LOCAL`) holds none of `/ ? # @ [ ]` and its `%` starts a whole
/// `%XX`, and a valid `ns` ends in no part of one, so the local name
/// extends the last part of `ns` and starts no other. Each part is checked
/// one character at a time, so the characters of `ns`, all valid, change
/// nothing, but for the authority's structure: its first `:` after any `@`
/// starts the port (checked before the host), and a `]` ends an IP
/// literal. The contexts keep those apart: host, port, after an IP
/// literal, path, query and fragment.
fn context(ns: &str) -> &'static str {
    let Ok(iri) = oxiri::Iri::parse(ns) else {
        return "a:";
    };
    if iri.fragment().is_some() {
        return "a:#";
    }
    if iri.query().is_some() {
        return "a:?";
    }
    match iri.authority() {
        Some(authority) if iri.path().is_empty() => {
            let host = authority
                .split_once('@')
                .map_or(authority, |(_, host)| host);
            match host.rsplit_once(']') {
                Some((_, "")) => "a://[::]",
                Some(_) => "a://h:",
                None if host.contains(':') => "a://h:",
                None => "a://h",
            }
        }
        _ => "a:",
    }
}

/// Why `value` is not an absolute IRI, if it is not.
fn why_not_iri(value: &str) -> Option<Box<str>> {
    match oxiri::Iri::parse(value) {
        Ok(_) => None,
        Err(_) if !has_scheme(value) => Some(
            format!(
                "<{}> is a relative IRI: an IRI in OWL 2 is absolute",
                shown(value)
            )
            .into_boxed_str(),
        ),
        Err(e) => Some(format!("Invalid IRI: {e}").into_boxed_str()),
    }
}
