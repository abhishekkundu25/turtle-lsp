//! Document formatting. Works from the token stream, never from the graph: the
//! tokens are laid out again and their text is copied unchanged, which is what
//! keeps the graph and the comments intact. A document with syntax errors is
//! left alone.
//!
//! Layout:
//! - directives first, in their original order, with prefix IRIs aligned;
//! - one blank line between statements; blank lines in the directive block and
//!   between comments are kept (a run collapses to one);
//! - the subject and its first predicate on one line, every further predicate
//!   on its own line one level in, ` ;` between them and ` .` at the end;
//!   any other `;` (a second in a row, or one after the last predicate) is
//!   left out, and a comment on it goes with the token before or after it;
//! - an object list on one line when it fits the line width, else one object
//!   per line;
//! - `[ ]`, `( )` and `{| |}` on one line when short and flat (nothing nested),
//!   else broken with their contents one level in; `<< >>` and `<<( )>>` always
//!   on one line;
//! - an own-line comment stays above the token after it, a trailing comment
//!   stays at the end of the line its token lands on;
//! - TriG: a graph block's opener (`GRAPH g {`, `g {` or `{`) on its own line,
//!   its statements one level in under the same rules (the optional last `.`
//!   kept or left out as written), and `}` on its own line;
//! - N-Triples and N-Quads: one triple (or quad, its graph label last) per
//!   line with single spaces, blank lines kept as written (a run collapses to
//!   one) instead of one between statements.

use crate::analysis::{Analysis, Encoding};
use crate::layout::{Blank, Doc, Groups, Ix, Obj, Po, Printer, Semicolons, Style, Term};
use crate::syntax::lexer::{Kind, Token};
use crate::syntax::{Parsed, PrefixMap, Severity, Syntax, Triple};
use lsp_types::{FormattingOptions, Position, Range, TextEdit};
use rustc_hash::FxHashMap;
use std::borrow::Cow;

pub use crate::layout::Options;

/// The LSP adapter: the layout core does not depend on `lsp_types`.
impl Options {
    pub(crate) fn from_lsp(o: &FormattingOptions, line_width: usize) -> Self {
        Options {
            tab_size: o.tab_size.clamp(1, 16),
            insert_spaces: o.insert_spaces,
            line_width,
        }
    }
}

/// `textDocument/formatting`: one edit replacing the whole document, or none
/// when the document is already formatted or has syntax errors.
pub fn formatting(a: &Analysis, enc: Encoding, opts: &Options) -> Vec<TextEdit> {
    match format_analysis(a, opts) {
        Some(new_text) if new_text != *a.text => vec![TextEdit {
            range: Range::new(Position::new(0, 0), a.position(enc, a.text.len() as u32)),
            new_text,
        }],
        _ => Vec::new(),
    }
}

/// [`format`] of an analysis, laid out from [`Analysis::tokens`].
pub(crate) fn format_analysis(a: &Analysis, opts: &Options) -> Option<String> {
    format_with(&a.text, &a.parsed, &a.external, opts, || {
        Cow::Borrowed(a.tokens())
    })
}

/// The formatted text, or `None` when the document has syntax errors, or, as a
/// safety net, when the result would not parse to the same triples (the same
/// JSON value for JSON-LD).
/// `parsed` is the parse of `text` with `external` prefixes. The text is
/// tokenized again; [`format_analysis`] takes an analysis's tokens instead.
pub fn format(text: &str, parsed: &Parsed, external: &PrefixMap, opts: &Options) -> Option<String> {
    format_with(text, parsed, external, opts, || {
        Cow::Owned(parsed.syntax.tokens(text))
    })
}

/// [`format`], with the tokens of `text` from `tokens`, asked for only when
/// there is something to lay out.
fn format_with<'t>(
    text: &str,
    parsed: &Parsed,
    external: &PrefixMap,
    opts: &Options,
    tokens: impl FnOnce() -> Cow<'t, [Token]>,
) -> Option<String> {
    if parsed.syntax == Syntax::JsonLd {
        // Only JSON syntax errors stop it; context problems do not.
        if parsed.errors.iter().any(|e| e.code == "json-syntax") {
            return None;
        }
        return crate::jsonld::format::format(text, opts);
    }
    if parsed.errors.iter().any(|e| e.severity == Severity::Error) {
        return None;
    }
    let mut doc = Doc::new(text, &tokens());
    let items = Cst {
        t: &doc.toks,
        pos: 0,
        trig: parsed.syntax == Syntax::TriG,
        quads: parsed.syntax == Syntax::NQuads,
    }
    .document()?;
    // The `;` the printer leaves out (`Semicolons::Needed`): the second of a
    // run, and every one with no predicate after its run, as `Cst::pol` reads them.
    let t = &doc.toks;
    let omit: Vec<Ix> = (0..t.len())
        .filter(|&i| {
            t[i].kind == Kind::Semicolon
                && (t[i - 1].kind == Kind::Semicolon
                    || !t[i..]
                        .iter()
                        .find(|t| t.kind != Kind::Semicolon)
                        .is_some_and(|t| matches!(t.kind, Kind::Iri | Kind::PName | Kind::A)))
        })
        .map(|i| i as Ix)
        .collect();
    if !omit.is_empty() {
        doc.attach(&omit);
    }
    let out = Formatter::new(&doc, opts, parsed.syntax).document(&items);
    #[cfg(test)]
    let out = tests::injected(text, out);
    let comments = doc.comments.len();
    // The layout is written: its memory goes before the output is parsed.
    drop(items);
    drop(doc);
    if let Err(bug) = same_graph(&out, parsed, comments, external) {
        eprintln!("{bug}");
        return None;
    }
    Some(out)
}

/// The safety net: `out` parses without errors to the triples and graphs of
/// `parsed`, with `comments` comments. Otherwise the formatter has a bug,
/// and this says what differs.
fn same_graph(
    out: &str,
    parsed: &Parsed,
    comments: usize,
    external: &PrefixMap,
) -> Result<(), String> {
    // Same starting base as the original, or relative IRIs would not match.
    let (again, again_comments) = Parsed::graph_only(
        out,
        external,
        parsed.syntax,
        parsed.document_base.as_deref(),
        parsed.triples.len(),
    );
    let same = !again.errors.iter().any(|e| e.severity == Severity::Error)
        && again_comments == comments
        && same_triples(&again.triples, &parsed.triples)
        && again.graphs.len() == parsed.graphs.len()
        && again
            .graphs
            .iter()
            .zip(&parsed.graphs)
            .all(|(x, y)| x.name == y.name);
    if same {
        return Ok(());
    }
    Err(format!(
        "turtle-lsp: formatter bug: the formatted text does not match the original \
         ({} vs {} triples, {} vs {} comments); no edits applied",
        again.triples.len(),
        parsed.triples.len(),
        again_comments,
        comments
    ))
}

/// Token order is kept, so the quads (and generated blank node labels) come
/// out in the same order.
fn same_triples(a: &[Triple], b: &[Triple]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.s == y.s && x.p == y.p && x.o == y.o && x.graph == y.graph)
}

// ---- concrete syntax tree ----

enum Item {
    Directive {
        kw: Ix,
        name: Option<Ix>,
        value: Ix,
        dot: Option<Ix>,
    },
    Triples {
        subject: Term,
        pol: Vec<Po>,
        /// The graph label of an N-Quads line.
        graph: Option<Ix>,
        /// Only the last statement of a TriG graph block may leave it out.
        dot: Option<Ix>,
    },
    /// A TriG graph block: `GRAPH name { ... }`, `name { ... }` or `{ ... }`.
    Graph {
        kw: Option<Ix>,
        name: Option<Term>,
        open: Ix,
        /// `Item::Triples` only.
        body: Vec<Item>,
        close: Ix,
    },
}

impl Item {
    fn first(&self) -> Ix {
        match self {
            Item::Directive { kw, .. } => *kw,
            Item::Triples { subject, .. } => subject.first(),
            Item::Graph { kw, name, open, .. } => {
                kw.or(name.as_ref().map(Term::first)).unwrap_or(*open)
            }
        }
    }
}

/// The tree of a document's tokens. Its lists are held at their length (a
/// list of one object is the common case, and a growing list holds four):
/// the tree of a large document is the larger part of the formatter's memory.
struct Cst<'a> {
    t: &'a [Token],
    pos: usize,
    trig: bool,
    /// N-Quads: a statement may end with a graph label.
    quads: bool,
}

impl Cst<'_> {
    fn kind(&self, ahead: usize) -> Option<Kind> {
        self.t.get(self.pos + ahead).map(|t| t.kind)
    }
    fn bump(&mut self) -> Ix {
        self.pos += 1;
        (self.pos - 1) as Ix
    }
    fn eat(&mut self, k: Kind) -> Option<Ix> {
        (self.kind(0) == Some(k)).then(|| self.bump())
    }

    fn document(mut self) -> Option<Vec<Item>> {
        let mut items = Vec::new();
        while let Some(k) = self.kind(0) {
            items.push(match k {
                Kind::PrefixDirective
                | Kind::SparqlPrefix
                | Kind::BaseDirective
                | Kind::SparqlBase
                | Kind::VersionDirective
                | Kind::SparqlVersion => self.directive(k)?,
                _ if self.graph_ahead() => self.graph()?,
                _ => self.triples(false)?,
            });
        }
        items.shrink_to_fit();
        Some(items)
    }

    fn directive(&mut self, k: Kind) -> Option<Item> {
        let kw = self.bump();
        let (name, value) = match k {
            Kind::PrefixDirective | Kind::SparqlPrefix => {
                (Some(self.eat(Kind::PName)?), self.eat(Kind::Iri)?)
            }
            Kind::BaseDirective | Kind::SparqlBase => (None, self.eat(Kind::Iri)?),
            _ => (None, self.eat(Kind::String)?),
        };
        let at_form = matches!(
            k,
            Kind::PrefixDirective | Kind::BaseDirective | Kind::VersionDirective
        );
        let dot = if at_form {
            Some(self.eat(Kind::Dot)?)
        } else {
            None
        };
        Some(Item::Directive {
            kw,
            name,
            value,
            dot,
        })
    }

    /// Length in tokens of a graph name at `ahead`: an IRI, `_:b` or `[]`.
    fn label_len(&self, ahead: usize) -> Option<usize> {
        match self.kind(ahead)? {
            Kind::Iri | Kind::PName | Kind::BNode => Some(1),
            Kind::LBracket if self.kind(ahead + 1) == Some(Kind::RBracket) => Some(2),
            _ => None,
        }
    }

    fn graph_ahead(&self) -> bool {
        self.trig
            && match self.kind(0) {
                Some(Kind::LBrace | Kind::Graph) => true,
                _ => self
                    .label_len(0)
                    .is_some_and(|n| self.kind(n) == Some(Kind::LBrace)),
            }
    }

    fn graph(&mut self) -> Option<Item> {
        let kw = self.eat(Kind::Graph);
        let name = match self.label_len(0) {
            Some(1) => {
                let t = self.bump();
                Some(Term::Atom(t, t))
            }
            Some(_) => Some(self.empty_pair()),
            None => None,
        };
        let open = self.eat(Kind::LBrace)?;
        let mut body = Vec::new();
        let close = loop {
            if let Some(close) = self.eat(Kind::RBrace) {
                break close;
            }
            body.push(self.triples(true)?);
        };
        Some(Item::Graph {
            kw,
            name,
            open,
            body,
            close,
        })
    }

    /// A statement; `in_graph`: inside a TriG graph block, where the last one
    /// may end at the `}` without a `.`.
    fn triples(&mut self, in_graph: bool) -> Option<Item> {
        let at_end = |c: &Self| match c.kind(0) {
            Some(Kind::Dot) => true,
            Some(Kind::RBrace) => in_graph,
            _ => false,
        };
        let (subject, pol) = match self.kind(0)? {
            Kind::LBracket if self.kind(1) != Some(Kind::RBracket) => {
                let s = self.list(Kind::RBracket)?;
                let pol = if at_end(self) {
                    Vec::new()
                } else {
                    self.pol()?
                };
                (s, pol)
            }
            Kind::LTriple => {
                let s = self.reified()?;
                let pol = if at_end(self) {
                    Vec::new()
                } else {
                    self.pol()?
                };
                (s, pol)
            }
            _ => {
                let s = self.subject()?;
                (s, self.pol()?)
            }
        };
        let graph = if self.quads {
            self.eat(Kind::Iri).or_else(|| self.eat(Kind::BNode))
        } else {
            None
        };
        let dot = match self.eat(Kind::Dot) {
            Some(d) => Some(d),
            None if in_graph && self.kind(0) == Some(Kind::RBrace) => None,
            None => return None,
        };
        Some(Item::Triples {
            subject,
            pol,
            graph,
            dot,
        })
    }

    fn is_verb(&self) -> bool {
        matches!(self.kind(0), Some(Kind::Iri | Kind::PName | Kind::A))
    }

    fn pol(&mut self) -> Option<Vec<Po>> {
        let mut out = Vec::new();
        loop {
            if !self.is_verb() {
                return None;
            }
            let v = self.bump();
            let objs = self.objects()?;
            let mut semis = Vec::new();
            while let Some(s) = self.eat(Kind::Semicolon) {
                semis.push(s);
            }
            let more = !semis.is_empty() && self.is_verb();
            out.push(Po {
                verb: Term::Atom(v, v),
                objs,
                semis,
            });
            if !more {
                out.shrink_to_fit();
                return Some(out);
            }
        }
    }

    fn objects(&mut self) -> Option<Vec<Obj>> {
        let mut out = Vec::with_capacity(1);
        loop {
            let term = self.object()?;
            let annots = self.annotations()?;
            let comma = self.eat(Kind::Comma);
            out.push(Obj {
                term,
                annots,
                comma,
            });
            if comma.is_none() {
                out.shrink_to_fit();
                return Some(out);
            }
        }
    }

    fn annotations(&mut self) -> Option<Vec<Term>> {
        let mut out = Vec::new();
        loop {
            match self.kind(0) {
                Some(Kind::Tilde) => {
                    let t = self.bump();
                    out.push(Term::Atom(t, t));
                    if let Some(r) = self.reifier() {
                        out.push(r);
                    }
                }
                Some(Kind::LAnnotation) => out.push(self.list(Kind::RAnnotation)?),
                _ => return Some(out),
            }
        }
    }

    fn reifier(&mut self) -> Option<Term> {
        match self.kind(0)? {
            Kind::Iri | Kind::PName | Kind::BNode => {
                let t = self.bump();
                Some(Term::Atom(t, t))
            }
            _ => None,
        }
    }

    /// `[ pol ]` / `{| pol |}`; the opening token is current.
    fn list(&mut self, close: Kind) -> Option<Term> {
        let open = self.bump();
        let pol = self.pol()?;
        let close = self.eat(close)?;
        Some(Term::List { open, pol, close })
    }

    fn empty_pair(&mut self) -> Term {
        let a = self.bump();
        let b = self.bump();
        Term::Atom(a, b)
    }

    fn subject(&mut self) -> Option<Term> {
        match self.kind(0)? {
            Kind::Iri | Kind::PName | Kind::BNode => {
                let t = self.bump();
                Some(Term::Atom(t, t))
            }
            Kind::LBracket if self.kind(1) == Some(Kind::RBracket) => Some(self.empty_pair()),
            Kind::LParen => self.collection(),
            Kind::LTriple => self.reified(),
            _ => None,
        }
    }

    fn quoted_subject(&mut self) -> Option<Term> {
        match self.kind(0)? {
            Kind::LParen => None,
            _ => self.subject(),
        }
    }

    fn object(&mut self) -> Option<Term> {
        match self.kind(0)? {
            Kind::Iri | Kind::PName | Kind::BNode | Kind::Number | Kind::Boolean => {
                let t = self.bump();
                Some(Term::Atom(t, t))
            }
            Kind::LBracket if self.kind(1) == Some(Kind::RBracket) => Some(self.empty_pair()),
            Kind::LBracket => self.list(Kind::RBracket),
            Kind::LParen => self.collection(),
            Kind::String => {
                let a = self.bump();
                let b = match self.kind(0) {
                    Some(Kind::LangTag) => self.bump(),
                    Some(Kind::Caret2) => {
                        self.bump();
                        match self.kind(0)? {
                            Kind::Iri | Kind::PName => self.bump(),
                            _ => return None,
                        }
                    }
                    _ => a,
                };
                Some(Term::Atom(a, b))
            }
            Kind::LTripleTerm => {
                let open = self.bump();
                let s = self.quoted_subject()?;
                let p = self.verb()?;
                let o = self.object()?;
                let close = self.eat(Kind::RTripleTerm)?;
                Some(Term::Quoted {
                    open,
                    parts: vec![s, p, o],
                    close,
                })
            }
            Kind::LTriple => self.reified(),
            _ => None,
        }
    }

    fn verb(&mut self) -> Option<Term> {
        if !self.is_verb() {
            return None;
        }
        let t = self.bump();
        Some(Term::Atom(t, t))
    }

    fn collection(&mut self) -> Option<Term> {
        if self.kind(1) == Some(Kind::RParen) {
            return Some(self.empty_pair());
        }
        let open = self.bump();
        let mut items = Vec::new();
        loop {
            if let Some(close) = self.eat(Kind::RParen) {
                return Some(Term::Coll { open, items, close });
            }
            items.push(Obj {
                term: self.object()?,
                annots: Vec::new(),
                comma: None,
            });
        }
    }

    fn reified(&mut self) -> Option<Term> {
        let open = self.bump();
        let mut parts = vec![self.quoted_subject()?, self.verb()?, self.object()?];
        if let Some(t) = self.eat(Kind::Tilde) {
            parts.push(Term::Atom(t, t));
            if let Some(r) = self.reifier() {
                parts.push(r);
            }
        }
        let close = self.eat(Kind::RTriple)?;
        Some(Term::Quoted { open, parts, close })
    }
}

// ---- printing ----

/// The Turtle-family layout of the items, written through the shared printer.
struct Formatter<'a> {
    p: Printer<'a, Token>,
    d: &'a Doc<'a, Token>,
    /// N-Triples and N-Quads: no blank line forced between statements.
    line_based: bool,
}

impl<'a> Formatter<'a> {
    fn new(d: &'a Doc<'a, Token>, o: &Options, syntax: Syntax) -> Formatter<'a> {
        let style = Style {
            groups: Groups::FromTerm,
            semicolons: Semicolons::Needed,
        };
        Formatter {
            p: Printer::new(d, o, style),
            d,
            line_based: syntax.is_line_based(),
        }
    }

    fn document(mut self, items: &[Item]) -> String {
        let order = hoisted_order(self.d, items);
        let align = items
            .iter()
            .filter_map(|it| match it {
                Item::Directive {
                    kw, name: Some(n), ..
                } => Some(self.d.width(*kw) + 1 + self.d.width(*n) + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let mut prev: Option<&Item> = None;
        for &k in &order {
            let it = &items[k];
            let blank = match prev {
                None => Blank::No,
                Some(p) => {
                    let both_directives =
                        matches!(p, Item::Directive { .. }) && matches!(it, Item::Directive { .. });
                    if both_directives || self.line_based {
                        Blank::Author
                    } else {
                        Blank::Forced
                    }
                }
            };
            self.p.start(0, it.first(), blank);
            match it {
                Item::Directive {
                    kw,
                    name,
                    value,
                    dot,
                } => {
                    self.p.tok(*kw, 0);
                    self.p.space(1);
                    if let Some(n) = name {
                        self.p.tok(*n, 1);
                        self.p.pad_to(align, 1);
                    }
                    self.p.tok(*value, 1);
                    if let Some(d) = dot {
                        self.p.punct(*d, 0);
                    }
                }
                Item::Triples { .. } => self.statement(it, 0),
                Item::Graph {
                    kw,
                    name,
                    open,
                    body,
                    close,
                } => self.graph(*kw, name.as_ref(), *open, body, *close),
            }
            prev = Some(it);
        }
        self.p.finish()
    }

    /// A statement whose subject starts at `level` on the current line.
    fn statement(&mut self, it: &Item, level: usize) {
        let Item::Triples {
            subject,
            pol,
            graph,
            dot,
        } = it
        else {
            return;
        };
        self.p.term(subject, level, false);
        if !pol.is_empty() {
            self.p.space(level + 1);
            self.p.pol_from(pol, level + 1, false, true);
        }
        if let Some(g) = graph {
            self.p.space(level + 1);
            self.p.tok(*g, level + 1);
        }
        if let Some(d) = dot {
            self.p.punct(*d, level);
        }
    }

    /// A graph block starting on the current (top-level) line.
    fn graph(&mut self, kw: Option<Ix>, name: Option<&Term>, open: Ix, body: &[Item], close: Ix) {
        if let Some(k) = kw {
            self.p.tok(k, 0);
            self.p.space(1);
        }
        if let Some(n) = name {
            self.p.term(n, 0, true);
            self.p.space(1);
        }
        self.p.tok(open, 0);
        for (k, it) in body.iter().enumerate() {
            let blank = if k > 0 { Blank::Forced } else { Blank::No };
            self.p.start(1, it.first(), blank);
            self.statement(it, 1);
        }
        // Comments after the last statement stay inside the block.
        self.p.close_group(close, 0, body.is_empty());
    }
}

/// Item order: every directive in the leading block, then directives that come
/// later moved up behind it as long as that cannot change what any IRI means
/// (no `BASE` moves, and no prefix moves above a use of its name), then the
/// rest in original order.
fn hoisted_order(d: &Doc<Token>, items: &[Item]) -> Vec<usize> {
    let first_stmt = items
        .iter()
        .position(|it| !matches!(it, Item::Directive { .. }))
        .unwrap_or(items.len());
    let mut first_use: FxHashMap<&str, Ix> = FxHashMap::default();
    for (i, t) in d.toks.iter().enumerate() {
        if t.kind == Kind::PName {
            let text = t.text(d.src);
            let name = &text[..text.find(':').unwrap_or(text.len())];
            first_use.entry(name).or_insert(i as Ix);
        }
    }
    let mut head: Vec<usize> = (0..first_stmt).collect();
    let mut rest = Vec::new();
    let mut hoisting = true;
    for (k, it) in items.iter().enumerate().skip(first_stmt) {
        if let Item::Directive { kw, name, .. } = it
            && hoisting
        {
            let movable = match d.toks[*kw as usize].kind {
                Kind::BaseDirective | Kind::SparqlBase => false,
                Kind::VersionDirective | Kind::SparqlVersion => true,
                _ => name.is_some_and(|n| {
                    let pname = d.text(n).trim_end_matches(':');
                    // The name token itself is the first PName of a fresh name.
                    first_use.get(pname).is_none_or(|&u| u >= n)
                }),
            };
            if movable {
                head.push(k);
                continue;
            }
            hoisting = false;
        }
        rest.push(k);
    }
    head.extend(rest);
    head
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::parse;

    /// A document holding this has one triple too many in its formatted
    /// text, as a formatter bug would give it.
    const ADD_A_TRIPLE: &str = "# turtle-lsp-test: the formatter adds a triple";

    /// The formatter's output, with the bug [`ADD_A_TRIPLE`] asks for.
    pub(super) fn injected(text: &str, mut out: String) -> String {
        if text.contains(ADD_A_TRIPLE) {
            out.push_str("\n<urn:injected:s> <urn:injected:p> <urn:injected:o> .\n");
        }
        out
    }

    #[test]
    fn a_formatter_that_changes_the_graph_is_refused() {
        let opts = Options::default();
        let good = "@prefix ex: <http://e/> .\nex:a   ex:p ex:b . # c\n";
        let parsed = parse(good, &PrefixMap::default(), Syntax::Turtle);
        let out = format(good, &parsed, &PrefixMap::default(), &opts).expect("formats");
        assert!(same_graph(&out, &parsed, 1, &PrefixMap::default()).is_ok());

        let bad = format!("{good}{ADD_A_TRIPLE}\n");
        let parsed = parse(&bad, &PrefixMap::default(), Syntax::Turtle);
        assert_eq!(format(&bad, &parsed, &PrefixMap::default(), &opts), None);
        let out = injected(&bad, format!("{good}{ADD_A_TRIPLE}\n"));
        let bug = same_graph(&out, &parsed, 2, &PrefixMap::default()).unwrap_err();
        assert!(bug.contains("formatter bug"), "{bug}");
        assert!(bug.contains("(2 vs 1 triples, 2 vs 2 comments)"), "{bug}");
        // A comment lost is a bug too.
        let bug = same_graph(
            good,
            &parse(good, &PrefixMap::default(), Syntax::Turtle),
            2,
            &PrefixMap::default(),
        )
        .unwrap_err();
        assert!(bug.contains("1 vs 1 triples, 1 vs 2 comments"), "{bug}");
    }

    #[test]
    fn an_analysis_is_formatted_from_its_tokens_kept_or_not() {
        let text = "@prefix ex: <http://e/> .\nex:a ex:p ex:b ;  ex:q ( 1 2 ) . # c\n";
        let opts = Options::default();
        let parsed = parse(text, &PrefixMap::default(), Syntax::Turtle);
        let expected = format(text, &parsed, &PrefixMap::default(), &opts);
        assert!(expected.is_some());
        for keep in [true, false] {
            let a = Analysis::new("file:///w/a.ttl".into(), text, Default::default(), keep);
            assert_eq!(format_analysis(&a, &opts), expected, "keep tokens: {keep}");
        }
    }
}
