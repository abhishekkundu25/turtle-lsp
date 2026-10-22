//! Resilient recursive-descent parser for Turtle 1.2, TriG 1.2, N-Triples 1.2
//! and N-Quads 1.2.
//!
//! On a syntax error the current statement is abandoned and parsing resumes at
//! the next `.`, or at the next line that starts at column 0 with something that
//! can begin a statement. Triples and references found before the error are kept,
//! so completion and navigation keep working while a statement is half typed.
//!
//! TriG: inside a graph block, recovery also stops at the `}` that closes it,
//! and a line that opens another block closes an unclosed one (reported as a
//! missing `}`), so one broken block does not swallow the rest of the file.
//!
//! N-Triples is parsed as Turtle; afterwards every construct N-Triples does not
//! have (directives, prefixed names, `a`, `;` and `,`, `[ ]`, collections,
//! unquoted literals, relative IRIs, reified triples and annotations, more or
//! less than one triple per line) is reported as an error.
//!
//! N-Quads is parsed as N-Triples, plus an optional graph label (an IRI or a
//! blank node) before each line's `.`, which puts the line's triple in that
//! graph. A literal graph label or a fifth term is an error; prefixed names
//! and relative IRIs are reported as in N-Triples.
//!
//! A document reports at most [`MAX_ERRORS`] syntax errors and warnings: the
//! ones that start first in the text, in the order they were found. One more
//! error, empty and at the end of the last one shown, says that there are
//! more. Parsing goes on past the cap, so the triples of every statement
//! that parses are still there.

use super::lexer::{Kind, Token, tokenize};
use super::*;
use std::borrow::Cow;
use std::sync::OnceLock;

type R<T> = Result<T, ()>;

/// Deepest nesting of `[ ]`, `( )`, `<< >>`, `<<( )>>` and `{| |}` the parser follows.
/// Real data rarely goes past a dozen; thousands only come from generated or
/// hostile input, and would overflow a worker thread's stack.
pub const MAX_NESTING: u32 = 256;

/// How many syntax errors a document reports, as JSON's
/// ([`crate::jsonld::json::MAX_ERRORS`]).
pub const MAX_ERRORS: usize = 100;

/// How many errors the parser holds before it drops all but the first
/// [`MAX_ERRORS`]: a multiple of it, so that dropping is rare and a broken
/// document never holds an error per token.
const ERRORS_HELD: usize = 4 * MAX_ERRORS;

const TT_HOLDS_NO_REIFIED_TRIPLE: &str =
    "A triple term <<( )>> cannot contain a reified triple << >>: write it as a triple term";

/// IRIs the parser produces itself, interned once.
struct Vocab {
    rdf_type: Iri,
    first: Iri,
    rest: Iri,
    nil: Iri,
    reifies: Iri,
    lang_string: Iri,
    dir_lang_string: Iri,
    string: Iri,
    integer: Iri,
    decimal: Iri,
    double: Iri,
    boolean: Iri,
}

fn vocab() -> &'static Vocab {
    static V: OnceLock<Vocab> = OnceLock::new();
    V.get_or_init(|| {
        let rdf = |l: &str| Iri::intern(&format!("{RDF}{l}"));
        let xsd = |l: &str| Iri::intern(&format!("{XSD}{l}"));
        Vocab {
            rdf_type: Iri::intern(RDF_TYPE),
            first: rdf("first"),
            rest: rdf("rest"),
            nil: rdf("nil"),
            reifies: rdf("reifies"),
            lang_string: rdf("langString"),
            dir_lang_string: rdf("dirLangString"),
            string: xsd("string"),
            integer: xsd("integer"),
            decimal: xsd("decimal"),
            double: xsd("double"),
            boolean: xsd("boolean"),
        }
    })
}

pub fn parse(src: &str, external: &PrefixMap, syntax: Syntax) -> Parsed {
    parse_with_base(src, external, syntax, None)
}

/// [`parse`], with relative IRIs resolved against `base` (the document's own
/// URL) until an `@base` replaces it. An invalid `base` is ignored.
///
/// For the Turtle family only. It never fails, but it does not read JSON-LD,
/// which has its own frontend: given [`Syntax::JsonLd`] it returns no triples
/// and one `syntax` error at offset 0.
pub fn parse_with_base(
    src: &str,
    external: &PrefixMap,
    syntax: Syntax,
    base: Option<&str>,
) -> Parsed {
    run(src, external, syntax, base, None).0
}

impl Parsed {
    /// The graph of `src` and no more: [`parse_with_base`] without the
    /// tokens, references, prefix uses and comment spans the editor features
    /// read, for a check of what a text says (the formatter's). The second
    /// value is how many comments the text has. Room for `triples` triples,
    /// the number the text is expected to have, is made at the start.
    pub fn graph_only(
        src: &str,
        external: &PrefixMap,
        syntax: Syntax,
        base: Option<&str>,
        triples: usize,
    ) -> (Parsed, usize) {
        run(src, external, syntax, base, Some(triples))
    }
}

impl Syntax {
    /// The tokens [`parse_with_base`] reads `src` as, comments included:
    /// what it keeps as [`Parsed::tokens`]. None for JSON-LD, which has its
    /// own frontend.
    pub fn tokens(self, src: &str) -> Vec<Token> {
        if self == Syntax::JsonLd {
            return Vec::new();
        }
        let mut all = tokenize(src);
        if self != Syntax::TriG {
            // `GRAPH` is only a keyword in TriG; elsewhere it is a stray word.
            for t in all.iter_mut().filter(|t| t.kind == Kind::Graph) {
                *t = t.into_unexpected_word();
            }
        }
        all
    }
}

/// The parse of `src`, and how many comments it has. With `graph_only` (the
/// number of triples expected), the tokens, references, prefix uses and
/// comment spans are left out.
fn run(
    src: &str,
    external: &PrefixMap,
    syntax: Syntax,
    base: Option<&str>,
    graph_only: Option<usize>,
) -> (Parsed, usize) {
    if syntax == Syntax::JsonLd {
        let refused = Parsed {
            syntax,
            document_base: base.map(str::to_string),
            errors: vec![SyntaxError::new(
                Span::new(0, 0),
                "JSON-LD documents are parsed by jsonld::parse",
                Severity::Error,
                "syntax",
            )],
            ..Parsed::default()
        };
        return (refused, 0);
    }
    let out = Parsed {
        syntax,
        document_base: base.map(str::to_string),
        triples: Vec::with_capacity(graph_only.unwrap_or(0)),
        ..Parsed::default()
    };
    let full = graph_only.is_none();
    let mut p = P {
        src,
        toks: syntax.tokens(src),
        full,
        pos: 0,
        prev: None,
        prefixes: external
            .iter()
            .map(|(k, v)| (k.clone(), Iri::intern(v)))
            .collect(),
        base: base.and_then(|b| oxiri::Iri::parse(b.to_string()).ok()),
        out,
        bnodes: 0,
        bnode_labels: FxHashMap::default(),
        quoted: 0,
        depth: 0,
        subject: None,
        statement: 0,
        trig: syntax == Syntax::TriG,
        quads: syntax == Syntax::NQuads,
        graph: None,
        label: None,
        in_graph: false,
        graph_indent: None,
        iri_cache: FxHashMap::default(),
        pname_cache: FxHashMap::default(),
        buf: String::new(),
        v: vocab(),
        reported: 0,
    };
    let mut comments = 0;
    for i in 0..p.toks.len() {
        let t = p.toks[i];
        if t.kind == Kind::Comment {
            comments += 1;
            if full {
                p.out.comments.push(Span::new(t.start, t.end));
            }
        }
        if let Some(msg) = t.error() {
            p.error(Span::new(t.start, t.end), msg);
        }
    }
    p.pos = p.skip_comments(0);
    p.document();
    if syntax.is_line_based() {
        p.ntriples_restrictions();
    }
    p.cap_errors();
    if full {
        p.out.tokens = p.toks;
    }
    (p.out, comments)
}

/// A term, and whether it is a prefixed name with an undeclared prefix or a
/// triple term holding one ([`Triple::unresolved`]). Kept per occurrence, not
/// per IRI: an undeclared `urn:x` stands in as the IRI `urn:x`, which
/// `<urn:x>` also is.
type Flagged = (Node, bool);

struct P<'a> {
    src: &'a str,
    /// Every token, comments too: they become [`Parsed::tokens`]. The parser
    /// steps over the comments.
    toks: Vec<Token>,
    /// Keep the tokens, references, prefix uses and comment spans
    /// ([`Parsed::graph_only`] does not).
    full: bool,
    /// The current token, never a comment (`toks.len()` at the end).
    pos: usize,
    /// The token before `pos` that is not a comment.
    prev: Option<usize>,
    /// Prefix name -> namespace, as declared so far.
    prefixes: FxHashMap<String, Iri>,
    base: Option<oxiri::Iri<String>>,
    out: Parsed,
    bnodes: u32,
    /// `_:label` -> its number in this document.
    bnode_labels: FxHashMap<&'a str, u32>,
    quoted: u32,
    /// Open `[`, `(`, `<<` and `<<(` around the current position.
    depth: u32,
    subject: Option<(Iri, Span)>,
    /// Index of the statement being parsed.
    statement: u32,
    trig: bool,
    /// N-Quads: a statement may end with a graph label.
    quads: bool,
    /// Name of the graph block being parsed (`None`: the default graph).
    graph: Option<GraphName>,
    /// The graph label of the N-Quads line being parsed, and its span.
    label: Option<(GraphName, Span)>,
    /// Inside `{ ... }`: a `}` ends the statement and stops recovery.
    in_graph: bool,
    /// Indentation of the first statement in the block that starts a line.
    graph_indent: Option<u32>,
    /// `<iri>` and `@lang` token text -> interned value. Cleared when the base changes.
    iri_cache: FxHashMap<&'a str, Iri>,
    /// `prefix:local` token text -> (prefix name, IRI or the text if unresolved, resolved).
    /// Cleared when a prefix is declared. Spares the global interner's locks.
    pname_cache: FxHashMap<&'a str, (Iri, Iri, bool)>,
    /// Scratch space for building IRIs from prefixed names.
    buf: String,
    v: &'static Vocab,
    /// Errors and warnings reported so far, the dropped ones too.
    reported: usize,
}

fn is_starter(k: Kind) -> bool {
    matches!(
        k,
        Kind::Iri
            | Kind::PName
            | Kind::BNode
            | Kind::LBracket
            | Kind::LParen
            | Kind::LTriple
            | Kind::PrefixDirective
            | Kind::BaseDirective
            | Kind::VersionDirective
            | Kind::SparqlPrefix
            | Kind::SparqlBase
            | Kind::SparqlVersion
    )
}

/// A token that starts a term (a fifth one, on an N-Quads line).
fn is_term(k: Kind) -> bool {
    matches!(
        k,
        Kind::Iri
            | Kind::PName
            | Kind::BNode
            | Kind::String
            | Kind::Number
            | Kind::Boolean
            | Kind::LBracket
            | Kind::LParen
            | Kind::LTriple
            | Kind::LTripleTerm
    )
}

/// The tokens N-Triples has no use for, and what to say about them.
fn ntriples_forbids(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::PrefixDirective
        | Kind::BaseDirective
        | Kind::VersionDirective
        | Kind::SparqlPrefix
        | Kind::SparqlBase
        | Kind::SparqlVersion => "N-Triples does not allow directives",
        Kind::PName => "N-Triples needs full IRIs like <http://example.org/x>, not prefixed names",
        Kind::A => {
            "N-Triples does not allow 'a': write <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"
        }
        Kind::Semicolon | Kind::Comma => {
            "N-Triples does not allow ';' or ',': write every triple in full"
        }
        Kind::LBracket => "N-Triples does not allow '[ ]': use a blank node label like _:b1",
        Kind::LParen => "N-Triples does not allow collections '( )'",
        Kind::Number | Kind::Boolean => {
            "N-Triples literals must be quoted, e.g. \"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        }
        Kind::LTriple | Kind::Tilde | Kind::LAnnotation => {
            "N-Triples does not allow reified triples or annotations: use a triple term <<( s p o )>>"
        }
        _ => return None,
    })
}

fn describe(k: Kind) -> &'static str {
    match k {
        Kind::Iri => "an IRI",
        Kind::PName => "a prefixed name",
        Kind::BNode => "a blank node",
        Kind::String => "a string",
        Kind::LangTag => "a language tag",
        Kind::Number => "a number",
        Kind::Boolean => "a boolean",
        Kind::A => "'a'",
        Kind::Dot => "'.'",
        Kind::Semicolon => "';'",
        Kind::Comma => "','",
        Kind::LBracket => "'['",
        Kind::RBracket => "']'",
        Kind::LParen => "'('",
        Kind::RParen => "')'",
        Kind::Caret2 => "'^^'",
        Kind::LTriple => "'<<'",
        Kind::RTriple => "'>>'",
        Kind::LTripleTerm => "'<<('",
        Kind::RTripleTerm => "')>>'",
        Kind::LAnnotation => "'{|'",
        Kind::RAnnotation => "'|}'",
        Kind::LBrace => "'{'",
        Kind::RBrace => "'}'",
        Kind::Graph => "'GRAPH'",
        Kind::Tilde => "'~'",
        Kind::PrefixDirective | Kind::SparqlPrefix => "a PREFIX directive",
        Kind::BaseDirective | Kind::SparqlBase => "a BASE directive",
        Kind::VersionDirective | Kind::SparqlVersion => "a VERSION directive",
        Kind::Comment | Kind::Error => "invalid text",
    }
}

impl<'a> P<'a> {
    // ---- token helpers ----

    fn peek(&self) -> Option<Token> {
        self.toks.get(self.pos).copied()
    }
    fn peek_kind(&self) -> Option<Kind> {
        self.toks.get(self.pos).map(|t| t.kind)
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos];
        self.advance();
        t
    }
    fn eat(&mut self, k: Kind) -> bool {
        if self.peek_kind() == Some(k) {
            self.advance();
            true
        } else {
            false
        }
    }
    /// Step to the next token that is not a comment.
    fn advance(&mut self) {
        self.prev = Some(self.pos);
        self.pos = self.skip_comments(self.pos + 1);
    }
    /// The first token at or after `i` that is not a comment.
    fn skip_comments(&self, mut i: usize) -> usize {
        while self.toks.get(i).is_some_and(|t| t.kind == Kind::Comment) {
            i += 1;
        }
        i
    }
    /// The kind of the `n`th token after the current one, comments not
    /// counted (0 is the current one).
    fn kind_ahead(&self, n: usize) -> Option<Kind> {
        let mut i = self.pos;
        for _ in 0..n {
            i = self.skip_comments(i + 1);
        }
        self.toks.get(i).map(|t| t.kind)
    }
    fn text(&self, t: Token) -> &'a str {
        t.text(self.src)
    }
    /// Where a new statement can start, for recovery: the current token is at
    /// column 0, or inside a graph block, first on its line and no deeper
    /// than the block's statements.
    fn at_line_start(&self) -> bool {
        match self.line_indent() {
            Some(0) => true,
            Some(c) => self.in_graph && self.graph_indent.is_some_and(|indent| c <= indent),
            None => false,
        }
    }

    /// The column of the current token if only spaces and tabs precede it on
    /// its line. Looks only at the gap since the previous token (no token
    /// ends a line but a string, and a comment reaches the line's end), so
    /// the cost is the size of that gap, not the length of the line.
    fn line_indent(&self) -> Option<u32> {
        let t = self.toks.get(self.pos)?;
        let from = match self.prev {
            Some(i) => self.toks[i].end as usize,
            None => 0,
        };
        let gap = &self.src.as_bytes()[from..t.start as usize];
        let (row, offset) = match gap.iter().rposition(|c| matches!(c, b'\n' | b'\r')) {
            Some(i) => (&gap[i + 1..], gap.len() - i - 1),
            // No line break since the previous token: the same line, unless
            // this is the first token and nothing but blanks precedes it.
            None if from == 0 => (gap, gap.len()),
            None => return None,
        };
        row.iter()
            .all(|c| matches!(c, b' ' | b'\t'))
            .then_some(offset as u32)
    }
    fn prev_span(&self) -> Span {
        match self.prev.map(|i| self.toks[i]) {
            Some(t) => Span::new(t.start, t.end),
            None => Span::new(0, 0),
        }
    }

    fn add_ref(&mut self, r: Ref) {
        if self.full {
            self.out.refs.push(r);
        }
    }
    fn add_prefix_use(&mut self, u: PrefixUse) {
        if self.full {
            self.out.prefix_uses.push(u);
        }
    }

    fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        self.report(SyntaxError::new(span, message, Severity::Error, "syntax"));
    }
    fn warning(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        self.report(SyntaxError::new(span, message, Severity::Warning, "syntax"));
    }
    fn report(&mut self, e: SyntaxError) {
        self.reported += 1;
        self.out.errors.push(e);
        if self.out.errors.len() >= ERRORS_HELD {
            self.keep_first_errors();
        }
    }

    /// Keep the [`MAX_ERRORS`] errors that start first in the text (the
    /// first reported, on a tie), in the order they were reported. A later
    /// error that starts before them all can still take a place.
    fn keep_first_errors(&mut self) {
        let errors = &mut self.out.errors;
        if errors.len() <= MAX_ERRORS {
            return;
        }
        let mut order: Vec<usize> = (0..errors.len()).collect();
        order.select_nth_unstable_by_key(MAX_ERRORS, |&i| (errors[i].span.start, i));
        let mut keep = vec![false; errors.len()];
        for &i in &order[..MAX_ERRORS] {
            keep[i] = true;
        }
        let mut i = 0;
        errors.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }

    /// Past [`MAX_ERRORS`], the first ones and one error that says so, at
    /// the end of the last one shown.
    fn cap_errors(&mut self) {
        if self.reported <= MAX_ERRORS {
            return;
        }
        self.keep_first_errors();
        let at = self
            .out
            .errors
            .iter()
            .map(|e| e.span.end)
            .max()
            .unwrap_or(0);
        self.out.errors.push(SyntaxError::new(
            Span::new(at, at),
            Cow::Owned(format!(
                "More than {MAX_ERRORS} syntax errors; the rest are not shown"
            )),
            Severity::Error,
            "syntax",
        ));
    }

    /// Report "Expected X" at the current token (or after the last token at EOF).
    fn expected<T>(&mut self, what: &str) -> R<T> {
        match self.peek() {
            // The lexer already reported malformed tokens.
            Some(t) if t.kind == Kind::Error => {}
            Some(t) => {
                let msg = format!("Expected {what}, found {}", describe(t.kind));
                self.error(Span::new(t.start, t.end), msg)
            }
            None => {
                let s = self.prev_span();
                self.error(s, format!("Expected {what} at end of file"))
            }
        }
        Err(())
    }

    fn fresh_bnode(&mut self) -> Node {
        self.bnodes += 1;
        Node::Blank(BlankId(self.bnodes))
    }

    /// A `_:label` token: the same label is the same node within a document.
    fn labelled_bnode(&mut self) -> Node {
        let t = self.bump();
        let label = self.text(t);
        let id = match self.bnode_labels.get(label) {
            Some(&id) => id,
            None => {
                self.bnodes += 1;
                self.bnode_labels.insert(label, self.bnodes);
                self.bnodes
            }
        };
        Node::Blank(BlankId(id))
    }

    /// Add a triple; `unresolved` says one of its terms is, or holds, an
    /// undeclared-prefix name.
    fn emit(&mut self, s: &Node, p: Iri, o: Node, p_span: Span, o_span: Span, unresolved: bool) {
        self.out.triples.push(Triple {
            s: s.clone(),
            p,
            o,
            p_span,
            o_span,
            statement: self.statement,
            graph: self.graph,
            unresolved,
        });
    }

    /// The span from token `from` to the last consumed token.
    fn span_from(&self, from: u32) -> Span {
        Span::new(from, self.prev_span().end.max(from))
    }

    fn start(&self) -> u32 {
        self.peek()
            .map_or_else(|| self.prev_span().end, |t| t.start)
    }

    // ---- document ----

    fn document(&mut self) {
        while let Some(t) = self.peek() {
            let start = self.pos;
            let ok = match t.kind {
                Kind::PrefixDirective | Kind::SparqlPrefix => self.prefix_directive(),
                Kind::BaseDirective | Kind::SparqlBase => self.base_directive(),
                Kind::VersionDirective | Kind::SparqlVersion => self.version_directive(),
                _ if self.graph_block_ahead() => self.graph_block(),
                _ => self.statement(),
            };
            if ok.is_err() {
                self.recover(start);
            }
        }
    }

    fn is_starter(&self, k: Kind) -> bool {
        is_starter(k) || (self.trig && matches!(k, Kind::LBrace | Kind::Graph))
    }

    fn recover(&mut self, from: usize) {
        if self.pos == from {
            self.advance();
        }
        while let Some(t) = self.peek() {
            if t.kind == Kind::Dot {
                self.advance();
                return;
            }
            if self.in_graph && t.kind == Kind::RBrace {
                return;
            }
            if self.is_starter(t.kind) && self.at_line_start() {
                return;
            }
            self.advance();
        }
    }

    // ---- TriG graph blocks ----

    /// Length in tokens of a graph name at the current token (IRI, prefixed
    /// name, `_:b`, `[]`).
    fn label_len(&self) -> Option<usize> {
        match self.kind_ahead(0)? {
            Kind::Iri | Kind::PName | Kind::BNode => Some(1),
            Kind::LBracket if self.kind_ahead(1) == Some(Kind::RBracket) => Some(2),
            _ => None,
        }
    }

    /// `{`, `GRAPH`, or a graph name followed by `{` (TriG only).
    fn graph_block_ahead(&self) -> bool {
        if !self.trig {
            return false;
        }
        match self.peek_kind() {
            Some(Kind::LBrace | Kind::Graph) => true,
            _ => self
                .label_len()
                .is_some_and(|n| self.kind_ahead(n) == Some(Kind::LBrace)),
        }
    }

    fn graph_label(&mut self) -> R<(GraphName, Span)> {
        const WHAT: &str = "a graph name (an IRI or a blank node)";
        let Some(t) = self.peek() else {
            return self.expected(WHAT);
        };
        match self.label_len() {
            Some(_) if matches!(t.kind, Kind::Iri | Kind::PName) => {
                let iri = self.iri(Role::Graph)?;
                Ok((GraphName::Iri(iri), Span::new(t.start, t.end)))
            }
            Some(n) => {
                let node = if n == 1 {
                    self.labelled_bnode()
                } else {
                    self.advance();
                    self.advance();
                    self.fresh_bnode()
                };
                let Node::Blank(b) = node else { unreachable!() };
                Ok((GraphName::Blank(b), self.span_from(t.start)))
            }
            None => self.expected(WHAT),
        }
    }

    /// `{ ... }`, `name { ... }` or `GRAPH name { ... }`. A graph block does
    /// not end with `.`.
    fn graph_block(&mut self) -> R<()> {
        let first = self.peek().unwrap();
        let mut name = None;
        if first.kind == Kind::Graph {
            self.bump();
            if self.peek_kind() == Some(Kind::LBrace) {
                let s = Span::new(first.start, first.end);
                self.error(s, "Expected a graph name after GRAPH");
            } else {
                name = Some(self.graph_label()?);
            }
        } else if first.kind != Kind::LBrace {
            name = Some(self.graph_label()?);
        }
        let Some(open) = self.peek().filter(|t| t.kind == Kind::LBrace) else {
            return self.expected("'{'");
        };
        self.bump();
        let first_statement = self.out.statements.len() as u32;
        let index = self.out.graphs.len();
        self.out.graphs.push(GraphBlock {
            span: Span::new(first.start, open.end),
            name: name.map(|(n, _)| n),
            name_span: name.map(|(_, s)| s),
            statements: first_statement..first_statement,
        });
        self.graph = name.map(|(n, _)| n);
        self.in_graph = true;
        self.graph_indent = None;
        self.graph_body(Span::new(open.start, open.end));
        self.in_graph = false;
        self.graph = None;
        let end = self.prev_span().end;
        let statements = self.out.statements.len() as u32;
        let g = &mut self.out.graphs[index];
        g.span.end = g.span.end.max(end);
        g.statements.end = statements;
        Ok(())
    }

    /// The statements of a block up to and including its `}`.
    fn graph_body(&mut self, open: Span) {
        loop {
            let Some(t) = self.peek() else {
                self.error(open, "Unclosed '{': expected '}' to close the graph block");
                return;
            };
            let span = Span::new(t.start, t.end);
            match t.kind {
                Kind::RBrace => {
                    self.bump();
                    return;
                }
                Kind::Dot => {
                    self.bump();
                    self.error(span, "Unexpected '.'");
                }
                // Another block: this one is missing its `}`.
                _ if self.graph_block_ahead() => {
                    let s = self.prev_span();
                    self.error(s, "Expected '}' to close the graph block");
                    return;
                }
                Kind::PrefixDirective
                | Kind::SparqlPrefix
                | Kind::BaseDirective
                | Kind::SparqlBase
                | Kind::VersionDirective
                | Kind::SparqlVersion => {
                    let start = self.pos;
                    let ok = match t.kind {
                        Kind::PrefixDirective | Kind::SparqlPrefix => self.prefix_directive(),
                        Kind::BaseDirective | Kind::SparqlBase => self.base_directive(),
                        _ => self.version_directive(),
                    };
                    let span = self.span_from(t.start);
                    self.error(span, "Directives are not allowed inside a graph block");
                    if ok.is_err() {
                        self.recover(start);
                    }
                }
                _ => {
                    if self.graph_indent.is_none() {
                        self.graph_indent = self.line_indent();
                    }
                    let start = self.pos;
                    if self.statement().is_err() {
                        self.recover(start);
                    }
                }
            }
        }
    }

    // ---- N-Triples and N-Quads ----

    /// What N-Triples forbids (N-Quads too: the messages name the syntax).
    fn ntriples_restrictions(&mut self) {
        let mut errors: Vec<(Span, &'static str)> = Vec::new();
        for t in &self.toks {
            let msg = match t.kind {
                Kind::String => {
                    let text = self.text(*t);
                    if text.starts_with('\'') || text.starts_with("\"\"\"") {
                        Some("N-Triples strings use double quotes on one line: \"...\"")
                    } else {
                        None
                    }
                }
                Kind::Iri if t.error().is_none() => {
                    let text = self.text(*t);
                    let raw = &text[1..text.len() - 1];
                    let raw = if raw.contains('\\') {
                        Cow::Owned(unescape_iri(raw))
                    } else {
                        Cow::Borrowed(raw)
                    };
                    (!has_scheme(&raw))
                        .then_some("N-Triples needs absolute IRIs (no relative IRIs, no @base)")
                }
                // N-Triples 1.2 allows `VERSION "1.2"`, in upper case.
                Kind::SparqlVersion if self.text(*t) == "VERSION" => None,
                k => ntriples_forbids(k),
            };
            if let Some(m) = msg {
                errors.push((Span::new(t.start, t.end), m));
            }
        }
        // One triple per line, each on a single line. Statements are in text
        // order, so lines are counted in one pass.
        let bytes = self.src.as_bytes();
        let (mut at, mut line) = (0usize, 0usize);
        let mut line_of = |off: u32| {
            let off = off as usize;
            let newlines = |r: &[u8]| r.iter().filter(|b| **b == b'\n').count();
            if off >= at {
                line += newlines(&bytes[at..off]);
            } else {
                line -= newlines(&bytes[off..at]);
            }
            at = off;
            line
        };
        let mut prev_end_line = None;
        for st in &self.out.statements {
            let (l0, l1) = (line_of(st.span.start), line_of(st.span.end));
            if prev_end_line == Some(l0) {
                errors.push((st.span, "N-Triples allows one triple per line"));
            } else if l1 > l0 {
                errors.push((st.span, "An N-Triples triple must be on one line"));
            }
            prev_end_line = Some(l1);
        }
        for (span, m) in errors {
            let m = if self.quads {
                Cow::Owned(
                    m.replace("one triple per line", "one quad per line")
                        .replace("N-Triples triple", "N-Quads quad")
                        .replace("N-Triples", "N-Quads"),
                )
            } else {
                Cow::Borrowed(m)
            };
            self.error(span, m);
        }
    }

    /// N-Quads: the optional graph label before the `.`, which puts the
    /// triples of the statement (from index `from` on) in that graph.
    fn quad_label(&mut self, from: usize) -> R<()> {
        let Some(t) = self.peek() else {
            return Ok(());
        };
        let span = Span::new(t.start, t.end);
        let name = match t.kind {
            // A prefixed name or a relative IRI is reported with the other
            // N-Triples restrictions.
            Kind::Iri | Kind::PName => GraphName::Iri(self.iri(Role::Graph)?),
            Kind::BNode => {
                let Node::Blank(b) = self.labelled_bnode() else {
                    unreachable!()
                };
                GraphName::Blank(b)
            }
            Kind::String | Kind::Number | Kind::Boolean => {
                self.error(
                    span,
                    "An N-Quads graph label is an IRI or a blank node, not a literal",
                );
                return Err(());
            }
            _ => return Ok(()),
        };
        for t in &mut self.out.triples[from..] {
            t.graph = Some(name);
        }
        self.label = Some((name, span));
        match self.peek() {
            Some(t) if is_term(t.kind) && !(self.is_starter(t.kind) && self.at_line_start()) => {
                self.error(
                    Span::new(t.start, t.end),
                    "An N-Quads line has at most four terms: subject, predicate, object and graph",
                );
                Err(())
            }
            _ => Ok(()),
        }
    }

    /// `.` after a directive or statement. A missing dot right before a line that
    /// starts a new statement is reported but does not cost the next statement.
    fn end_statement(&mut self, sparql: bool) -> R<()> {
        if sparql {
            if self.peek_kind() == Some(Kind::Dot) {
                let t = self.bump();
                self.error(
                    Span::new(t.start, t.end),
                    "SPARQL-style PREFIX/BASE must not end with '.'",
                );
            }
            return Ok(());
        }
        match self.peek() {
            Some(t) if t.kind == Kind::Dot => {
                self.advance();
                Ok(())
            }
            None => {
                let s = self.prev_span();
                self.error(s, "Expected '.' to end the statement");
                Ok(())
            }
            // In a graph block, the closing `}` ends the last statement.
            Some(t) if self.in_graph && t.kind == Kind::RBrace => Ok(()),
            Some(t) if self.is_starter(t.kind) && self.at_line_start() => {
                let s = self.prev_span();
                self.error(s, "Expected '.' to end the statement");
                Ok(())
            }
            Some(_) => self.expected("'.', ';' or ','"),
        }
    }

    // ---- directives ----

    fn prefix_directive(&mut self) -> R<()> {
        let kw = self.bump();
        let sparql = kw.kind == Kind::SparqlPrefix;
        let name_tok = match self.peek() {
            Some(t)
                if t.kind == Kind::PName
                    && self.text(t).ends_with(':')
                    && self.text(t).find(':') == Some(t.end as usize - t.start as usize - 1) =>
            {
                self.bump()
            }
            _ => return self.expected("a prefix name like `ex:`"),
        };
        let iri_tok = match self.peek() {
            Some(t) if t.kind == Kind::Iri => self.bump(),
            _ => return self.expected("a namespace IRI like `<http://example.org/>`"),
        };
        let name = self.text(name_tok).trim_end_matches(':').to_string();
        let iri = Iri::intern(&self.resolve_iri(iri_tok));
        self.end_statement(sparql)?;
        self.prefixes.insert(name.clone(), iri);
        self.pname_cache.clear();
        self.out.prefixes.push(PrefixDecl {
            name,
            iri,
            span: Span::new(kw.start, self.prev_span().end),
            name_span: Span::new(name_tok.start, name_tok.end - 1),
            iri_span: Span::new(iri_tok.start, iri_tok.end),
        });
        Ok(())
    }

    fn base_directive(&mut self) -> R<()> {
        let kw = self.bump();
        let iri_tok = match self.peek() {
            Some(t) if t.kind == Kind::Iri => self.bump(),
            _ => return self.expected("a base IRI"),
        };
        let iri = self.resolve_iri(iri_tok).into_owned();
        self.iri_cache.clear();
        match oxiri::Iri::parse(iri.clone()) {
            Ok(b) => self.base = Some(b),
            Err(e) => self.error(
                Span::new(iri_tok.start, iri_tok.end),
                format!("Invalid base IRI: {e}"),
            ),
        }
        self.out.base = Some(iri);
        self.end_statement(kw.kind == Kind::SparqlBase)
    }

    fn version_directive(&mut self) -> R<()> {
        let kw = self.bump();
        match self.peek() {
            Some(t) if t.kind == Kind::String => {
                self.bump();
                let text = self.text(t);
                if text.starts_with("\"\"\"") || text.starts_with("'''") {
                    self.error(
                        Span::new(t.start, t.end),
                        "A version is a single-line string like \"1.2\"",
                    );
                }
            }
            _ => return self.expected("a version string like \"1.2\""),
        }
        self.end_statement(kw.kind == Kind::SparqlVersion)
    }

    // ---- statements ----

    fn statement(&mut self) -> R<()> {
        let start = self.peek().map_or(0, |t| t.start);
        self.subject = None;
        self.label = None;
        self.statement = self.out.statements.len() as u32;
        let r = self.triples();
        let subject = self.subject.take();
        let end = self.prev_span().end.max(start);
        self.out.statements.push(Statement {
            span: Span::new(start, end),
            subject,
            graph: self.label.take(),
        });
        r
    }

    fn triples(&mut self) -> R<()> {
        let from = self.out.triples.len();
        match self.peek_kind() {
            Some(Kind::LBracket) if self.kind_ahead(1) != Some(Kind::RBracket) => {
                let s = self.blank_node_property_list()?;
                if !self.at_statement_end() {
                    self.predicate_object_list(&s, false)?;
                }
            }
            Some(Kind::LTriple) => {
                let (s, bad) = self.reified_triple()?;
                if !self.at_statement_end() {
                    self.predicate_object_list(&s, bad)?;
                }
            }
            _ => {
                let (s, bad) = self.subject()?;
                self.predicate_object_list(&s, bad)?;
            }
        }
        if self.quads {
            self.quad_label(from)?;
        }
        self.end_statement(false)
    }

    fn at_statement_end(&self) -> bool {
        match self.peek_kind() {
            Some(Kind::Dot) => true,
            Some(Kind::RBrace) => self.in_graph,
            _ => false,
        }
    }

    fn subject(&mut self) -> R<Flagged> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => {
                let t = self.peek().unwrap();
                let role = if self.quoted > 0 {
                    Role::Quoted
                } else {
                    Role::Subject
                };
                let (iri, bad) = self.flagged_iri(role)?;
                if role == Role::Subject && self.subject.is_none() {
                    self.subject = Some((iri, Span::new(t.start, t.end)));
                }
                Ok((Node::Iri(iri), bad))
            }
            Some(Kind::BNode) => Ok((self.labelled_bnode(), false)),
            Some(Kind::LBracket) if self.kind_ahead(1) == Some(Kind::RBracket) => {
                self.advance();
                self.advance();
                Ok((self.fresh_bnode(), false))
            }
            Some(Kind::LParen) => Ok((self.collection()?, false)),
            Some(Kind::LTriple) => self.reified_triple(),
            _ => self.expected("a subject"),
        }
    }

    /// `s_bad`: the subject is an undeclared-prefix name.
    fn predicate_object_list(&mut self, s: &Node, s_bad: bool) -> R<()> {
        loop {
            let (p, p_span, p_bad) = self.verb()?;
            self.object_list(s, p, p_span, s_bad || p_bad)?;
            if self.peek_kind() != Some(Kind::Semicolon) {
                return Ok(());
            }
            while self.eat(Kind::Semicolon) {}
            if !matches!(self.peek_kind(), Some(Kind::Iri | Kind::PName | Kind::A)) {
                return Ok(());
            }
        }
    }

    /// The predicate, its span and whether it is an undeclared-prefix name.
    fn verb(&mut self) -> R<(Iri, Span, bool)> {
        let role = if self.quoted > 0 {
            Role::Quoted
        } else {
            Role::Predicate
        };
        match self.peek() {
            Some(t) if t.kind == Kind::A => {
                self.bump();
                let span = Span::new(t.start, t.end);
                self.add_ref(Ref {
                    span,
                    iri: self.v.rdf_type,
                    role,
                    form: Form::A,
                });
                Ok((self.v.rdf_type, span, false))
            }
            Some(t) if matches!(t.kind, Kind::Iri | Kind::PName) => {
                let (iri, bad) = self.flagged_iri(role)?;
                Ok((iri, Span::new(t.start, t.end), bad))
            }
            _ => self.expected("a predicate"),
        }
    }

    /// `sp_bad`: the subject or the predicate is an undeclared-prefix name.
    fn object_list(&mut self, s: &Node, p: Iri, p_span: Span, sp_bad: bool) -> R<()> {
        loop {
            let start = self.start();
            let (o, o_bad) = self.object()?;
            let o_span = self.span_from(start);
            let bad = sp_bad || o_bad;
            self.emit(s, p, o.clone(), p_span, o_span, bad);
            self.annotation(s, p, p_span, &o, o_span, bad)?;
            if !self.eat(Kind::Comma) {
                return Ok(());
            }
        }
    }

    fn object(&mut self) -> R<Flagged> {
        let role = if self.quoted > 0 {
            Role::Quoted
        } else {
            Role::Object
        };
        let node = match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => {
                let (iri, bad) = self.flagged_iri(role)?;
                return Ok((Node::Iri(iri), bad));
            }
            Some(Kind::LTripleTerm) => return self.triple_term(),
            Some(Kind::LTriple) => return self.reified_triple(),
            _ => self.plain_object()?,
        };
        Ok((node, false))
    }

    /// An object that is no IRI and holds none that counts for
    /// [`Triple::unresolved`]: a blank node, a collection or a literal.
    fn plain_object(&mut self) -> R<Node> {
        match self.peek_kind() {
            Some(Kind::BNode) => Ok(self.labelled_bnode()),
            Some(Kind::LBracket) => {
                if self.kind_ahead(1) == Some(Kind::RBracket) {
                    self.advance();
                    self.advance();
                    Ok(self.fresh_bnode())
                } else {
                    self.blank_node_property_list()
                }
            }
            Some(Kind::LParen) => self.collection(),
            Some(Kind::String) => self.rdf_literal(),
            Some(Kind::Number) => {
                let t = self.bump();
                let text = self.text(t);
                let datatype = if text.contains(['e', 'E']) {
                    self.v.double
                } else if text.contains('.') {
                    self.v.decimal
                } else {
                    self.v.integer
                };
                Ok(Node::Literal {
                    value: text.into(),
                    datatype,
                    lang: None,
                    dir: None,
                })
            }
            Some(Kind::Boolean) => {
                let t = self.bump();
                Ok(Node::Literal {
                    value: self.text(t).into(),
                    datatype: self.v.boolean,
                    lang: None,
                    dir: None,
                })
            }
            _ => self.expected("an object"),
        }
    }

    fn rdf_literal(&mut self) -> R<Node> {
        let t = self.bump();
        let value: Box<str> = string_value(self.text(t)).into();
        match self.peek_kind() {
            Some(Kind::LangTag) => {
                let lt = self.bump();
                let tag = &self.text(lt)[1..];
                let (lang, dir_text) = match tag.split_once("--") {
                    Some((l, d)) => (l, Some(d)),
                    None => (tag, None),
                };
                if let Err(e) = oxilangtag::LanguageTag::parse(lang) {
                    self.error(
                        Span::new(lt.start, lt.end),
                        format!("Invalid language tag '{lang}': {e}"),
                    );
                }
                // An unknown direction is reported and dropped: the literal is
                // then a plain language-tagged string.
                let dir = dir_text.and_then(Direction::parse);
                if let Some(d) = dir_text
                    && dir.is_none()
                {
                    self.error(
                        Span::new(lt.start, lt.end),
                        format!("Base direction must be 'ltr' or 'rtl', found '{d}'"),
                    );
                }
                let datatype = if dir.is_some() {
                    self.v.dir_lang_string
                } else {
                    self.v.lang_string
                };
                // Keyed by the token (`@en--rtl`), the cache holds the bare tag.
                let lang = match self.iri_cache.get(self.text(lt)) {
                    Some(&l) => l,
                    None => {
                        let l = Iri::intern(&lang.to_ascii_lowercase());
                        self.iri_cache.insert(self.text(lt), l);
                        l
                    }
                };
                Ok(Node::Literal {
                    value,
                    datatype,
                    lang: Some(lang),
                    dir,
                })
            }
            Some(Kind::Caret2) => {
                self.bump();
                if !matches!(self.peek_kind(), Some(Kind::Iri | Kind::PName)) {
                    return self.expected("a datatype IRI after '^^'");
                }
                let datatype = self.iri(if self.quoted > 0 {
                    Role::Quoted
                } else {
                    Role::Datatype
                })?;
                if datatype == self.v.lang_string || datatype == self.v.dir_lang_string {
                    self.error(
                        self.prev_span(),
                        "A language-tagged string is written \"text\"@lang, not with ^^",
                    );
                }
                Ok(Node::Literal {
                    value,
                    datatype,
                    lang: None,
                    dir: None,
                })
            }
            _ => Ok(Node::Literal {
                value,
                datatype: self.v.string,
                lang: None,
                dir: None,
            }),
        }
    }

    /// Run `f` one nesting level deeper. Past [`MAX_NESTING`] the statement
    /// is abandoned with an error: unbounded recursion would overflow the
    /// stack, which no panic guard can catch.
    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> R<T> {
        if self.depth >= MAX_NESTING {
            let span = self
                .peek()
                .map_or(self.prev_span(), |t| Span::new(t.start, t.end));
            self.error(span, format!("Nested more than {MAX_NESTING} levels deep"));
            return Err(());
        }
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    fn blank_node_property_list(&mut self) -> R<Node> {
        self.nested(Self::blank_node_property_list_inner)
    }

    fn blank_node_property_list_inner(&mut self) -> R<Node> {
        let open = self.bump();
        let b = self.fresh_bnode();
        self.predicate_object_list(&b, false)?;
        if !self.eat(Kind::RBracket) {
            if self.peek().is_none() {
                self.error(Span::new(open.start, open.end), "Unclosed '['");
                return Err(());
            }
            return self.expected("']'");
        }
        Ok(b)
    }

    fn collection(&mut self) -> R<Node> {
        self.nested(Self::collection_inner)
    }

    fn collection_inner(&mut self) -> R<Node> {
        let open = self.bump();
        let mut items = Vec::new();
        loop {
            match self.peek_kind() {
                Some(Kind::RParen) => {
                    self.advance();
                    break;
                }
                None => {
                    self.error(Span::new(open.start, open.end), "Unclosed '('");
                    return Err(());
                }
                _ => {
                    let start = self.start();
                    let (item, bad) = self.object()?;
                    items.push((item, bad, self.span_from(start)));
                }
            }
        }
        let nil = Node::Iri(self.v.nil);
        if items.is_empty() {
            return Ok(nil);
        }
        let head = self.fresh_bnode();
        let mut cur = head.clone();
        let n = items.len();
        let (first, rest) = (self.v.first, self.v.rest);
        for (i, (item, bad, span)) in items.into_iter().enumerate() {
            self.emit(&cur, first, item, span, span, bad);
            let next = if i + 1 == n {
                nil.clone()
            } else {
                self.fresh_bnode()
            };
            self.emit(&cur, rest, next.clone(), span, span, false);
            cur = next;
        }
        Ok(head)
    }

    /// `<<( s p o )>>`: unresolved when one of its terms is.
    fn triple_term(&mut self) -> R<Flagged> {
        self.nested(Self::triple_term_inner)
    }

    fn triple_term_inner(&mut self) -> R<Flagged> {
        self.bump();
        self.quoted += 1;
        let r = (|| {
            let (s, s_bad) = self.tt_subject()?;
            let (p, _, p_bad) = self.verb()?;
            let (o, o_bad) = self.tt_object()?;
            if !self.eat(Kind::RTripleTerm) {
                return self.expected("')>>'");
            }
            Ok((Node::Triple(Box::new((s, p, o))), s_bad || p_bad || o_bad))
        })();
        self.quoted -= 1;
        r
    }

    /// `ttSubject ::= iri | BlankNode`: a triple term holds no reified triple.
    fn tt_subject(&mut self) -> R<Flagged> {
        let (start, reified) = (self.start(), self.peek_kind() == Some(Kind::LTriple));
        let s = self.rt_subject()?;
        if reified {
            self.error(self.span_from(start), TT_HOLDS_NO_REIFIED_TRIPLE);
        }
        Ok(s)
    }

    /// `ttObject ::= iri | BlankNode | literal | tripleTerm`.
    fn tt_object(&mut self) -> R<Flagged> {
        let (start, reified) = (self.start(), self.peek_kind() == Some(Kind::LTriple));
        let o = self.rt_object()?;
        if reified {
            self.error(self.span_from(start), TT_HOLDS_NO_REIFIED_TRIPLE);
        }
        Ok(o)
    }

    /// `rtSubject ::= iri | BlankNode | reifiedTriple`.
    fn rt_subject(&mut self) -> R<Flagged> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => {
                let (iri, bad) = self.flagged_iri(Role::Quoted)?;
                Ok((Node::Iri(iri), bad))
            }
            Some(Kind::BNode) => Ok((self.labelled_bnode(), false)),
            Some(Kind::LBracket) if self.kind_ahead(1) == Some(Kind::RBracket) => {
                self.advance();
                self.advance();
                Ok((self.fresh_bnode(), false))
            }
            Some(Kind::LTriple) => self.reified_triple(),
            _ => self.expected("a subject"),
        }
    }

    /// The object of a triple term or reified triple: no collections and no
    /// `[ ... ]` with content. Those are reported and parsed anyway.
    fn rt_object(&mut self) -> R<Flagged> {
        let nested = match self.peek_kind() {
            Some(Kind::LParen) => true,
            Some(Kind::LBracket) => self.kind_ahead(1) != Some(Kind::RBracket),
            _ => false,
        };
        let start = self.start();
        let o = self.object()?;
        if nested {
            self.error(
                self.span_from(start),
                "A triple cannot contain a collection '( )' or a '[ ... ]' with properties",
            );
        }
        Ok(o)
    }

    /// `<< s p o ~ r >>`: returns the reifier, flagged when it is an
    /// undeclared-prefix name.
    fn reified_triple(&mut self) -> R<Flagged> {
        self.nested(Self::reified_triple_inner)
    }

    fn reified_triple_inner(&mut self) -> R<Flagged> {
        let open = self.bump();
        self.quoted += 1;
        let r = (|| {
            let (s, s_bad) = self.rt_subject()?;
            let (p, _, p_bad) = self.verb()?;
            let (o, o_bad) = self.rt_object()?;
            let reifier = if self.eat(Kind::Tilde) {
                self.reifier()?
            } else {
                (self.fresh_bnode(), false)
            };
            if !self.eat(Kind::RTriple) {
                return self.expected("'>>'");
            }
            let triple = Node::Triple(Box::new((s, p, o)));
            Ok((reifier, triple, s_bad || p_bad || o_bad))
        })();
        self.quoted -= 1;
        let ((reifier, r_bad), triple, t_bad) = r?;
        let span = self.span_from(open.start);
        self.emit(&reifier, self.v.reifies, triple, span, span, r_bad || t_bad);
        Ok((reifier, r_bad))
    }

    fn reifier(&mut self) -> R<Flagged> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => {
                let (iri, bad) = self.flagged_iri(Role::Object)?;
                Ok((Node::Iri(iri), bad))
            }
            Some(Kind::BNode) => Ok((self.labelled_bnode(), false)),
            _ => Ok((self.fresh_bnode(), false)),
        }
    }

    /// `~ reifier` and `{| ... |}` after an object. The `rdf:reifies` triples
    /// point at the annotated predicate and object; `spo_bad` says one of
    /// those three is an undeclared-prefix name.
    fn annotation(
        &mut self,
        s: &Node,
        p: Iri,
        p_span: Span,
        o: &Node,
        o_span: Span,
        spo_bad: bool,
    ) -> R<()> {
        let mut last: Option<Flagged> = None;
        let reifies = self.v.reifies;
        loop {
            match self.peek_kind() {
                Some(Kind::Tilde) => {
                    self.bump();
                    let (r, r_bad) = self.reifier()?;
                    self.emit(
                        &r,
                        reifies,
                        Node::Triple(Box::new((s.clone(), p, o.clone()))),
                        p_span,
                        o_span,
                        r_bad || spo_bad,
                    );
                    last = Some((r, r_bad));
                }
                Some(Kind::LAnnotation) => {
                    self.bump();
                    let (r, r_bad) = match last.take() {
                        Some(r) => r,
                        None => {
                            let r = self.fresh_bnode();
                            self.emit(
                                &r,
                                reifies,
                                Node::Triple(Box::new((s.clone(), p, o.clone()))),
                                p_span,
                                o_span,
                                spo_bad,
                            );
                            (r, false)
                        }
                    };
                    self.nested(|this| this.predicate_object_list(&r, r_bad))?;
                    if !self.eat(Kind::RAnnotation) {
                        return self.expected("'|}'");
                    }
                }
                _ => return Ok(()),
            }
        }
    }

    // ---- IRIs ----

    /// Consume an IRIREF or prefixed name, record a [`Ref`], return the IRI.
    /// An undeclared prefix is not a parse failure: it is recorded in
    /// `prefix_uses` and reported by the diagnostics layer.
    fn iri(&mut self, role: Role) -> R<Iri> {
        self.flagged_iri(role).map(|(iri, _)| iri)
    }

    /// [`P::iri`], and whether it is a prefixed name with an undeclared
    /// prefix, which stands in as an IRI of its own text
    /// ([`Triple::unresolved`]).
    fn flagged_iri(&mut self, role: Role) -> R<(Iri, bool)> {
        let t = self.bump();
        let span = Span::new(t.start, t.end);
        if t.kind == Kind::Iri {
            let text = self.text(t);
            let iri = match self.iri_cache.get(text) {
                Some(&iri) => iri,
                None => {
                    let warnings = self.reported;
                    let iri = Iri::intern(&self.resolve_iri(t));
                    // Invalid IRIs are not cached, so each occurrence is reported.
                    if self.reported == warnings {
                        self.iri_cache.insert(text, iri);
                    }
                    iri
                }
            };
            self.add_ref(Ref {
                span,
                iri,
                role,
                form: Form::Iri,
            });
            return Ok((iri, false));
        }
        let text = self.text(t);
        let colon = text.find(':').unwrap_or(text.len());
        let (name, iri, resolved) = match self.pname_cache.get(text) {
            Some(&hit) => hit,
            None => {
                let (name, local) = (&text[..colon], &text[colon + 1..]);
                let entry = match self.prefixes.get(name) {
                    Some(ns) => {
                        self.buf.clear();
                        self.buf.push_str(ns);
                        unescape_local(local, &mut self.buf);
                        (Iri::intern(name), Iri::intern(&self.buf), true)
                    }
                    None => (Iri::intern(name), Iri::intern(text), false),
                };
                self.pname_cache.insert(text, entry);
                entry
            }
        };
        self.add_prefix_use(PrefixUse {
            name,
            span,
            resolved,
        });
        if resolved {
            self.add_ref(Ref {
                span,
                iri,
                role,
                form: Form::PName {
                    prefix_len: colon as u32,
                },
            });
        }
        Ok((iri, !resolved))
    }

    fn resolve_iri(&mut self, t: Token) -> Cow<'a, str> {
        let text = self.text(t);
        let raw = text
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or(text);
        let raw: Cow<'a, str> = if raw.contains('\\') {
            Cow::Owned(unescape_iri(raw))
        } else {
            Cow::Borrowed(raw)
        };
        match &self.base {
            // An absolute IRI without dot segments resolves to itself: check it
            // and skip building the resolved copy.
            Some(_) if has_scheme(&raw) && !raw.contains("/.") && !raw.contains(":.") => {
                if let Err(e) = oxiri::Iri::parse(&*raw) {
                    self.warning(Span::new(t.start, t.end), format!("Invalid IRI: {e}"));
                }
                raw
            }
            Some(base) => match oxiri::IriRef::parse(&*raw).and_then(|r| base.resolve(&r)) {
                Ok(iri) => Cow::Owned(iri.into_inner()),
                Err(e) => {
                    self.warning(Span::new(t.start, t.end), format!("Invalid IRI: {e}"));
                    raw
                }
            },
            None => {
                if has_scheme(&raw)
                    && let Err(e) = oxiri::Iri::parse(&*raw)
                {
                    self.warning(Span::new(t.start, t.end), format!("Invalid IRI: {e}"));
                }
                raw
            }
        }
    }
}

/// Whether `s` starts with a URI scheme and `:`: an absolute IRI, not a
/// relative reference.
pub fn has_scheme(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && s.find(':').is_some_and(|i| {
            s[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        })
}

/// Append `local` to `out` with backslash escapes removed.
fn unescape_local(local: &str, out: &mut String) {
    if !local.contains('\\') {
        out.push_str(local);
        return;
    }
    let mut chars = local.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
}

fn unescape_iri(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(i) = rest.find('\\') {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 1..];
        let n = match tail.as_bytes().first() {
            Some(b'u') => 4,
            Some(b'U') => 8,
            _ => 0,
        };
        match tail
            .get(1..1 + n)
            .and_then(|h| u32::from_str_radix(h, 16).ok())
            .and_then(char::from_u32)
        {
            Some(c) if n > 0 => {
                out.push(c);
                rest = &tail[1 + n..];
            }
            _ => {
                out.push('\\');
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Lexical value of a string token (quotes stripped, escapes decoded).
pub fn string_value(tok: &str) -> String {
    let q = &tok[..1];
    let long = tok.len() >= 6 && tok.starts_with(&q.repeat(3));
    let inner = if long {
        let body = &tok[3..];
        // Closing delimiter is the last three quotes (extra quotes belong to the content).
        body.strip_suffix(&q.repeat(3)).unwrap_or(body)
    } else {
        let body = &tok[1..];
        body.strip_suffix(q).unwrap_or(body)
    };
    if !inner.contains('\\') {
        return inner.to_string();
    }
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{8}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some(u @ ('u' | 'U')) => {
                let n = if u == 'u' { 4 } else { 8 };
                let hex: String = chars.by_ref().take(n).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(src: &str) -> Parsed {
        parse(src, &PrefixMap::default(), Syntax::Turtle)
    }

    fn trig(src: &str) -> Parsed {
        parse(src, &PrefixMap::default(), Syntax::TriG)
    }

    fn nt(src: &str) -> Parsed {
        parse(src, &PrefixMap::default(), Syntax::NTriples)
    }

    fn nq(src: &str) -> Parsed {
        parse(src, &PrefixMap::default(), Syntax::NQuads)
    }
    fn messages(r: &Parsed) -> Vec<&str> {
        r.errors.iter().map(|e| e.message()).collect()
    }

    #[test]
    fn fixed_messages_are_borrowed() {
        let r = p("@prefix ex: <http://e/> .\nex:a ex:p \"open\nex:b ex:p ex:o");
        let unterminated = r
            .errors
            .iter()
            .find(|e| e.message() == "Unterminated string")
            .expect("the lexer's error");
        assert!(matches!(unterminated.message, Cow::Borrowed(_)));
        let missing_dot = r
            .errors
            .iter()
            .find(|e| e.message() == "Expected '.' to end the statement")
            .expect("the parser's error");
        assert!(matches!(missing_dot.message, Cow::Borrowed(_)));
    }

    /// `n` lines, each a statement that parses and one that does not.
    fn broken_lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("<http://e/s{i}> <http://e/p> <http://e/o{i}> . <http://e/t{i}> <http://e/p> .\n"))
            .collect()
    }

    #[test]
    fn errors_are_capped_and_parsing_goes_on() {
        let src = broken_lines(1000);
        let r = p(&src);
        assert_eq!(r.errors.len(), MAX_ERRORS + 1);
        let line = |e: &SyntaxError| src[..e.span.start as usize].matches('\n').count();
        for (i, e) in r.errors[..MAX_ERRORS].iter().enumerate() {
            assert_eq!(line(e), i, "{e:?}");
            assert_eq!(e.message(), "Expected an object, found '.'");
        }
        let cap = &r.errors[MAX_ERRORS];
        assert_eq!(
            cap.message(),
            "More than 100 syntax errors; the rest are not shown"
        );
        assert_eq!((cap.severity, cap.code), (Severity::Error, "syntax"));
        let last_shown = r.errors[MAX_ERRORS - 1].span.end;
        assert_eq!(cap.span, Span::new(last_shown, last_shown));
        // Every valid statement still gives its triple, the last line's too.
        assert_eq!(r.triples.len(), 1000);
        assert_eq!(r.statements.len(), 2000);
        assert!(
            r.triples
                .iter()
                .any(|t| t.o == Node::Iri(Iri::intern("http://e/o999")))
        );
    }

    #[test]
    fn up_to_the_cap_every_error_is_shown() {
        let r = p(&broken_lines(MAX_ERRORS));
        assert_eq!(r.errors.len(), MAX_ERRORS);
        assert!(r.errors.iter().all(|e| e.message().starts_with("Expected")));
        let r = p(&broken_lines(MAX_ERRORS + 1));
        assert_eq!(r.errors.len(), MAX_ERRORS + 1);
        assert!(r.errors[MAX_ERRORS].message().starts_with("More than 100"));
    }

    #[test]
    fn the_cap_keeps_the_errors_that_come_first_in_the_text() {
        // The lexer's errors are found before the parser's; here they come
        // last in the text, so the parser's are the ones shown.
        let mut src = broken_lines(300);
        for i in 0..300 {
            src.push_str(&format!("<http://e/u{i}> <http://e/p> \"open\n"));
        }
        let r = p(&src);
        assert_eq!(r.errors.len(), MAX_ERRORS + 1);
        let starts: Vec<u32> = r.errors[..MAX_ERRORS]
            .iter()
            .map(|e| e.span.start)
            .collect();
        assert!(starts.is_sorted(), "{starts:?}");
        assert!(
            r.errors[..MAX_ERRORS]
                .iter()
                .all(|e| e.message() == "Expected an object, found '.'")
        );
        assert_eq!(r.triples.len(), 600);
    }

    #[test]
    fn triples_and_refs() {
        let r = p(
            "@prefix ex: <http://e/> .\nex:a a ex:B ;\n  ex:p \"x\"@en, 1, [ ex:q ex:r ], (ex:l1 ex:l2) .\n",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.prefixes[0].name, "ex");
        assert_eq!(r.statements.len(), 1);
        assert_eq!(r.statements[0].subject.as_ref().unwrap().0, "http://e/a");
        assert!(
            r.refs
                .iter()
                .any(|x| x.iri == "http://e/q" && x.role == Role::Predicate)
        );
        // a, B, p x4 objects, q r, l1 l2 + list triples
        assert_eq!(r.triples.len(), 1 + 4 + 1 + 4); // a, 4 objects of ex:p, [ ex:q ], 2-item list
    }

    #[test]
    fn recovers_after_errors() {
        let r = p(
            "@prefix ex: <http://e/> .\nex:a ex:p ex:o .\nex:b ex:p \"open .\nex:c ex:p ex:o\nex:d ex:p ex:o .\nex:e ex:p nope:x .\nex:f ex:p ex:o .\n",
        );
        let subjects: Vec<_> = r
            .statements
            .iter()
            .filter_map(|s| s.subject.map(|x| x.0.as_str()))
            .collect();
        for s in [
            "http://e/a",
            "http://e/c",
            "http://e/d",
            "http://e/e",
            "http://e/f",
        ] {
            assert!(subjects.contains(&s), "{s} missing from {subjects:?}");
        }
        assert!(
            r.errors
                .iter()
                .any(|e| e.message().contains("Unterminated string"))
        );
        assert!(
            r.errors
                .iter()
                .any(|e| e.message().contains("Expected '.'"))
        );
        assert!(
            r.prefix_uses
                .iter()
                .any(|u| u.name == "nope" && !u.resolved)
        );
    }

    #[test]
    fn external_prefixes() {
        let mut ext = PrefixMap::default();
        ext.insert("ex".into(), "http://e/".into());
        let r = parse("ex:a ex:p ex:o .", &ext, Syntax::Turtle);
        assert!(r.errors.is_empty());
        assert_eq!(r.triples[0].p, "http://e/p");
    }

    #[test]
    fn object_on_next_line_is_fine() {
        let r = p("@prefix ex: <http://e/> .\nex:s ex:p\n    ex:o .\n");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    #[test]
    fn rdf12_syntax() {
        let r = p(
            "@prefix ex: <http://e/> .\nex:a ex:p ex:o ~ ex:r {| ex:src ex:s |} .\n<< ex:a ex:p ex:o >> ex:q ex:z .\nex:x ex:y <<( ex:a ex:p ex:o )>> .\n\"x\"@en--ltr .\n",
        );
        assert!(
            r.errors
                .iter()
                .filter(|e| e.severity == Severity::Error)
                .count()
                >= 1
        ); // last line has no subject
        assert!(r.triples.iter().any(|t| t.p.ends_with("reifies")));
    }

    #[test]
    fn triples_carry_predicate_and_object_spans() {
        let src = "@prefix ex: <http://e/> .\nex:a ex:p \"x\"@en ;\n  ex:q [ ex:r ( ex:l1 ex:l2 ) ] .\nex:b ex:p ex:o ~ ex:r .\n";
        let r = p(src);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let at = |sp: Span| &src[sp.start as usize..sp.end as usize];
        let find = |p: &str| r.triples.iter().find(|t| t.p == p).unwrap();
        let q = find("http://e/q");
        assert_eq!(at(q.p_span), "ex:q");
        assert_eq!(at(q.o_span), "[ ex:r ( ex:l1 ex:l2 ) ]");
        assert_eq!(at(find("http://e/r").o_span), "( ex:l1 ex:l2 )");
        let lit = find("http://e/p");
        assert_eq!(at(lit.o_span), "\"x\"@en");
        assert_eq!(lit.statement, 0);
        let firsts: Vec<_> = r
            .triples
            .iter()
            .filter(|t| t.p.ends_with("#first"))
            .map(|t| at(t.o_span))
            .collect();
        assert_eq!(firsts, ["ex:l1", "ex:l2"]);
        let reifies = r.triples.iter().find(|t| t.p.ends_with("reifies")).unwrap();
        assert_eq!((at(reifies.p_span), at(reifies.o_span)), ("ex:p", "ex:o"));
        assert_eq!(reifies.statement, 1);
        let st = &r.statements[reifies.statement as usize];
        assert_eq!(st.subject.unwrap().0, "http://e/b");
    }

    #[test]
    fn blank_node_labels_are_unique_per_document() {
        let r = p("@prefix ex: <http://e/> .\n_:g1 ex:p [ ex:q _:g1 ] .\n_:b1 ex:p _:g2 .\n");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let bnode = |n: &Node| match n {
            Node::Blank(b) => *b,
            _ => panic!("not a blank node"),
        };
        let [inner, outer, last] = &r.triples[..] else {
            panic!("{:?}", r.triples)
        };
        // `_:g1` is the same node both times, and not the generated `[ ]` node.
        assert_eq!(bnode(&outer.s), bnode(&inner.o));
        assert_ne!(bnode(&outer.s), bnode(&outer.o));
        assert_eq!(bnode(&outer.o), bnode(&inner.s));
        let all = [&outer.s, &outer.o, &last.s, &last.o].map(bnode);
        let distinct: std::collections::BTreeSet<_> = all.iter().collect();
        assert_eq!(distinct.len(), 4);
    }

    #[test]
    fn trig_graph_blocks() {
        let src = "@prefix ex: <http://e/> .\nex:d ex:p ex:o .\nex:g1 { ex:a ex:p ex:o . ex:b ex:p ex:o }\nGRAPH ex:g2 {\n  ex:c ex:p [ ex:q 1 ] .\n}\n{ ex:e ex:p ex:o }\n_:g { ex:f ex:p ex:o . }\n[] { ex:h ex:p ex:o }\n";
        let r = trig(src);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let graph_of = |s: &str| {
            let t = r
                .triples
                .iter()
                .find(|t| t.s == Node::Iri(Iri::intern(s)))
                .unwrap();
            t.graph
        };
        assert_eq!(graph_of("http://e/d"), None);
        assert_eq!(
            graph_of("http://e/a"),
            Some(GraphName::Iri(Iri::intern("http://e/g1")))
        );
        assert_eq!(
            graph_of("http://e/b"),
            Some(GraphName::Iri(Iri::intern("http://e/g1")))
        );
        assert_eq!(
            graph_of("http://e/c"),
            Some(GraphName::Iri(Iri::intern("http://e/g2")))
        );
        assert_eq!(graph_of("http://e/e"), None);
        assert!(matches!(graph_of("http://e/f"), Some(GraphName::Blank(_))));
        assert!(matches!(graph_of("http://e/h"), Some(GraphName::Blank(_))));
        // The [ ex:q 1 ] triple is in ex:g2 too.
        let q = r.triples.iter().find(|t| t.p == "http://e/q").unwrap();
        assert_eq!(q.graph, Some(GraphName::Iri(Iri::intern("http://e/g2"))));
        assert_eq!(r.graphs.len(), 5);
        let at = |sp: Span| &src[sp.start as usize..sp.end as usize];
        assert_eq!(
            at(r.graphs[0].span),
            "ex:g1 { ex:a ex:p ex:o . ex:b ex:p ex:o }"
        );
        assert_eq!(at(r.graphs[1].name_span.unwrap()), "ex:g2");
        assert!(at(r.graphs[1].span).starts_with("GRAPH") && at(r.graphs[1].span).ends_with('}'));
        assert_eq!(r.graphs[2].name, None);
        // Statements inside blocks have their subjects; names are Graph refs.
        let subjects: Vec<_> = r.graphs[0]
            .statements
            .clone()
            .map(|i| r.statements[i as usize].subject.unwrap().0.as_str())
            .collect();
        assert_eq!(subjects, ["http://e/a", "http://e/b"]);
        assert!(
            r.refs
                .iter()
                .any(|x| x.iri == "http://e/g1" && x.role == Role::Graph)
        );
        assert!(
            r.refs
                .iter()
                .any(|x| x.iri == "http://e/c" && x.role == Role::Subject)
        );
    }

    #[test]
    fn trig_recovers_inside_and_between_blocks() {
        // Broken triple inside a block, unclosed block followed by another block,
        // directive inside a block, nested block, unclosed block at the end.
        let src = "@prefix ex: <http://e/> .\nex:g1 {\n  ex:a ex:p \"open .\n  ex:b ex:p ex:o .\n}\nex:g2 {\n  ex:c ex:p ex:o .\nex:g3 {\n  @prefix x: <http://x/> .\n  ex:d ex:p x:o\n}\nex:after ex:p ex:o .\nex:g4 { ex:e ex:p ex:o .\n";
        let r = trig(src);
        let msgs = messages(&r);
        assert!(
            msgs.iter().any(|m| m.contains("Unterminated string")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("Expected '}'")), "{msgs:?}");
        assert!(
            msgs.iter()
                .any(|m| m.contains("Directives are not allowed")),
            "{msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("Unclosed '{'")), "{msgs:?}");
        let graph_of = |s: &str| {
            r.triples
                .iter()
                .find(|t| t.s == Node::Iri(Iri::intern(s)))
                .unwrap_or_else(|| panic!("{s} lost: {msgs:?}"))
                .graph
        };
        let g = |n: &str| Some(GraphName::Iri(Iri::intern(n)));
        assert_eq!(graph_of("http://e/b"), g("http://e/g1"));
        assert_eq!(graph_of("http://e/c"), g("http://e/g2"));
        assert_eq!(graph_of("http://e/d"), g("http://e/g3"));
        assert_eq!(graph_of("http://e/after"), None);
        assert_eq!(graph_of("http://e/e"), g("http://e/g4"));
        // A stray '}' and a missing graph name are reported, parsing goes on.
        let r = trig("@prefix ex: <http://e/> .\n}\nGRAPH { ex:a ex:p ex:o }\nex:b ex:p ex:o .\n");
        let msgs = messages(&r);
        assert!(msgs.iter().any(|m| m.contains("found '}'")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("graph name")), "{msgs:?}");
        assert_eq!(r.triples.len(), 2);
        // Cut short anywhere: never a panic.
        for src in [
            "GRAPH",
            "GRAPH ex:g",
            "ex:g {",
            "{ ex:a",
            "_:b",
            "[] {",
            "{ [ ex:p",
        ] {
            trig(src);
        }
    }

    #[test]
    fn braces_and_graph_are_errors_in_turtle() {
        let r = p(
            "@prefix ex: <http://e/> .\nex:g { ex:a ex:p ex:o }\nGRAPH ex:g { }\nex:b ex:p ex:o .\n",
        );
        assert!(!r.errors.is_empty());
        assert!(r.graphs.is_empty());
        assert!(r.triples.iter().all(|t| t.graph.is_none()));
        assert!(
            r.statements
                .iter()
                .any(|s| s.subject.is_some_and(|x| x.0 == "http://e/b"))
        );
    }

    #[test]
    fn ntriples_accepts_ntriples() {
        let r = nt(concat!(
            "<http://e/a> <http://e/p> <http://e/o> .\n",
            "# comment\n",
            "_:b1 <http://e/p> \"x\\n\"@en-GB . # trailing\n",
            "<http://e/a> <http://e/p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
            "<http://e/a> <http://e/p> <<( <http://e/s> <http://e/p> \"o\"@en--ltr )>> .\n",
            "\n",
        ));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.triples.len(), 4);
    }

    #[test]
    fn nquads_puts_each_line_in_its_graph() {
        let r = nq(concat!(
            "<http://e/a> <http://e/p> <http://e/o> <http://e/g> .\n",
            "<http://e/a> <http://e/p> \"x\" .\n",
            "_:s <http://e/p> <<( <http://e/s> <http://e/p> _:o )>> _:g . # c\n",
        ));
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let g = |i: usize| r.triples[i].graph;
        assert_eq!(g(0), Some(GraphName::Iri(Iri::intern("http://e/g"))));
        assert_eq!(g(1), None);
        assert!(matches!(g(2), Some(GraphName::Blank(_))));
        assert_eq!(
            r.statements[0].graph.map(|(_, s)| s),
            Some(Span::new(39, 51))
        );
        assert!(r.graphs.is_empty());
    }

    #[test]
    fn nquads_reports_bad_graph_labels_per_line() {
        let r = nq(concat!(
            "<http://e/a> <http://e/p> <http://e/o> \"g\" .\n",
            "<http://e/a> <http://e/p> <http://e/o> <http://e/g> <http://e/h> .\n",
            "<http://e/a> <http://e/p> <http://e/o> ex:g .\n",
            "<http://e/a> <http://e/p> <http://e/o> <g> .\n",
            "<http://e/b> <http://e/p> <http://e/o> <http://e/g> .\n",
        ));
        let msgs = messages(&r);
        assert!(msgs[0].contains("not a literal"), "{msgs:?}");
        assert!(
            msgs.iter().any(|m| m.contains("at most four terms")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.starts_with("N-Quads needs full IRIs")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.starts_with("N-Quads needs absolute IRIs")),
            "{msgs:?}"
        );
        assert!(msgs.iter().all(|m| !m.contains("N-Triples")), "{msgs:?}");
        let last = r.triples.last().unwrap();
        assert_eq!(last.s, Node::Iri(Iri::intern("http://e/b")));
        // A quad is an error in N-Triples.
        assert!(
            !nt("<http://e/a> <http://e/p> <http://e/o> <http://e/g> .\n")
                .errors
                .is_empty()
        );
    }

    #[test]
    fn ntriples_reports_turtle_only_constructs() {
        let cases = [
            ("@prefix ex: <http://e/> .\n", "directives"),
            ("PREFIX ex: <http://e/>\n", "directives"),
            ("@base <http://e/> .\n", "directives"),
            ("<http://e/a> <http://e/p> ex:o .\n", "prefixed names"),
            ("<http://e/a> a <http://e/C> .\n", "'a'"),
            (
                "<http://e/a> <http://e/p> <http://e/o> ; <http://e/q> <http://e/o> .\n",
                "';' or ','",
            ),
            (
                "<http://e/a> <http://e/p> <http://e/o> , <http://e/o2> .\n",
                "';' or ','",
            ),
            (
                "<http://e/a> <http://e/p> [ <http://e/q> <http://e/o> ] .\n",
                "'[ ]'",
            ),
            ("<http://e/a> <http://e/p> [] .\n", "'[ ]'"),
            (
                "<http://e/a> <http://e/p> ( <http://e/o> ) .\n",
                "collections",
            ),
            ("<http://e/a> <http://e/p> 1 .\n", "must be quoted"),
            ("<http://e/a> <http://e/p> true .\n", "must be quoted"),
            ("<http://e/a> <http://e/p> 'x' .\n", "double quotes"),
            (
                "<http://e/a> <http://e/p> \"\"\"x\"\"\" .\n",
                "double quotes",
            ),
            ("<a> <http://e/p> <http://e/o> .\n", "absolute IRIs"),
            (
                "<< <http://e/a> <http://e/p> <http://e/o> >> <http://e/q> <http://e/r> .\n",
                "reified",
            ),
            (
                "<http://e/a> <http://e/p> <http://e/o> {| <http://e/q> <http://e/r> |} .\n",
                "annotations",
            ),
            (
                "<http://e/a> <http://e/p> <http://e/o> . <http://e/b> <http://e/p> <http://e/o> .\n",
                "one triple per line",
            ),
            (
                "<http://e/a> <http://e/p>\n  <http://e/o> .\n",
                "on one line",
            ),
        ];
        for (src, want) in cases {
            let r = nt(src);
            let msgs = messages(&r);
            assert!(msgs.iter().any(|m| m.contains(want)), "{src:?}: {msgs:?}");
            // The same text is fine as Turtle (apart from the prefixed name).
            if want != "prefixed names" {
                assert!(p(src).errors.is_empty(), "{src:?}: {:?}", p(src).errors);
            }
        }
    }

    #[test]
    fn syntax_detection() {
        assert_eq!(Syntax::detect(Some("trig"), "file:///a.ttl"), Syntax::TriG);
        assert_eq!(
            Syntax::detect(Some("n-triples"), "file:///a"),
            Syntax::NTriples
        );
        assert_eq!(
            Syntax::detect(Some("ntriples"), "file:///a"),
            Syntax::NTriples
        );
        assert_eq!(
            Syntax::detect(Some("plaintext"), "file:///a.trig"),
            Syntax::TriG
        );
        assert_eq!(Syntax::detect(None, "file:///x/a.nt"), Syntax::NTriples);
        assert_eq!(Syntax::detect(None, "file:///x/a.nq"), Syntax::NQuads);
        assert_eq!(Syntax::detect(Some("n-quads"), "file:///a"), Syntax::NQuads);
        assert_eq!(
            Syntax::detect(Some("turtle"), "file:///a.nq"),
            Syntax::NQuads
        );
        assert!(Syntax::NQuads.is_line_based() && !Syntax::NQuads.has_prefixes());
        assert_eq!(
            Syntax::detect(None, "file:///x.trig/a.turtle"),
            Syntax::Turtle
        );
        assert_eq!(Syntax::detect(None, "untitled:Untitled-1"), Syntax::Turtle);
        // An editor that calls every RDF file "turtle".
        assert_eq!(
            Syntax::detect(Some("turtle"), "file:///a.trig"),
            Syntax::TriG
        );
        assert_eq!(
            Syntax::detect(Some("turtle"), "file:///a.nt"),
            Syntax::NTriples
        );
        assert_eq!(
            Syntax::detect(Some("turtle"), "file:///a.ttl"),
            Syntax::Turtle
        );
        assert_eq!(Syntax::from_extension("C:\\x\\a.trig"), Some(Syntax::TriG));
        assert_eq!(Syntax::from_extension("/x/notes.txt"), None);
    }

    #[test]
    fn jsonld_is_detected_by_extension_and_language_id_but_json_is_not() {
        assert_eq!(Syntax::from_extension("/x/a.jsonld"), Some(Syntax::JsonLd));
        assert_eq!(Syntax::from_language_id("jsonld"), Some(Syntax::JsonLd));
        assert_eq!(Syntax::from_extension("/x/a.json"), None);
        assert_eq!(Syntax::from_language_id("json"), None);
        // Editors that call `.jsonld` files `json` (VS Code) or `turtle`.
        for id in [None, Some("json"), Some("turtle"), Some("jsonld")] {
            assert_eq!(
                Syntax::detect(id, "file:///a.jsonld"),
                Syntax::JsonLd,
                "{id:?}"
            );
        }
        assert_eq!(
            Syntax::detect(Some("jsonld"), "untitled:Untitled-1"),
            Syntax::JsonLd
        );
        assert_eq!(
            Syntax::detect(Some("json"), "file:///a.json"),
            Syntax::Turtle
        );
        assert!(!Syntax::JsonLd.is_line_based() && !Syntax::JsonLd.has_prefixes());
    }

    /// Parse on a 2 MiB stack, the smallest any host thread gets: a missing
    /// depth guard aborts the process here instead of failing an assertion.
    fn parse_small_stack(src: String) -> Vec<String> {
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || messages(&p(&src)).into_iter().map(str::to_owned).collect())
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn deep_nesting_is_one_error_not_an_abort() {
        let n = 100_000;
        let cases = [
            ("{|", format!("<a> <p> <o> {}", "{| <p> <o> ".repeat(n))),
            ("[", format!("<a> <p> {}", "[ <p> ".repeat(n))),
            ("(", format!("<a> <p> {}", "( ".repeat(n))),
            ("<<", format!("{} <p> <o> .", "<< ".repeat(n))),
            ("<<(", format!("<a> <p> {}", "<<( <a> <p> ".repeat(n))),
        ];
        for (open, src) in cases {
            let errors = parse_small_stack(src);
            let deep: Vec<_> = errors
                .iter()
                .filter(|m| m.contains("Nested more than 256 levels deep"))
                .collect();
            assert_eq!(deep.len(), 1, "{open}: {errors:?}");
        }
    }

    #[test]
    fn annotation_nesting_up_to_the_limit_parses() {
        let n = MAX_NESTING as usize;
        let src = format!(
            "<a> <p> <o> {}{}.",
            "{| <p> <o> ".repeat(n),
            "|} ".repeat(n)
        );
        let errors = parse_small_stack(src);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn backslash_newline_in_a_short_string_keeps_the_next_statement() {
        for nl in ["\n", "\r", "\r\n"] {
            let src = format!("<a> <p> \"x\\{nl}<b> <p> <o> .{nl}<c> <p> <o> .{nl}");
            let r = p(&src);
            assert!(
                r.errors
                    .iter()
                    .any(|e| e.message() == "Unterminated string"),
                "{nl:?}: {:?}",
                r.errors
            );
            let subjects: Vec<_> = r
                .statements
                .iter()
                .filter_map(|s| s.subject.map(|x| x.0.as_str()))
                .collect();
            assert!(subjects.contains(&"b"), "{nl:?}: {subjects:?}");
            assert!(subjects.contains(&"c"), "{nl:?}: {subjects:?}");
        }
    }

    #[test]
    fn cr_only_file_keeps_its_triples_after_a_comment() {
        let src = "# header\r<a> <p> <o> .\r# mid\r<b> <p> <o> .\r";
        let r = p(src);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.triples.len(), 2);
        assert_eq!(r.comments.len(), 2);
        // A statement missing its dot is still recovered at a CR line start.
        let r = p("<a> <p> <o>\r<b> <p> <o> .\r");
        assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
    }

    #[test]
    fn string_values() {
        assert_eq!(string_value("\"a\\nb\""), "a\nb");
        assert_eq!(string_value("'''x''y'''"), "x''y");
        assert_eq!(string_value("\"\\u00e9\""), "é");
    }

    #[test]
    fn a_base_direction_is_kept_apart_from_the_language_tag() {
        let r = p("<http://e/a> <http://e/p> \"x\"@AR--rtl, \"y\"@en--ltr, \"z\"@en, \"w\" .");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let lits: Vec<_> = r
            .triples
            .iter()
            .map(|t| match &t.o {
                Node::Literal {
                    datatype,
                    lang,
                    dir,
                    ..
                } => (
                    datatype.rsplit('#').next().unwrap().to_string(),
                    lang.map(|l| l.to_string()),
                    dir.map(|d| d.to_string()),
                ),
                other => panic!("{other:?}"),
            })
            .collect();
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            lits,
            vec![
                ("dirLangString".to_string(), some("ar"), some("rtl")),
                ("dirLangString".to_string(), some("en"), some("ltr")),
                ("langString".to_string(), some("en"), None),
                ("string".to_string(), None, None),
            ]
        );
        // An unknown direction is reported and the literal is a plain tagged string.
        let r = p("<http://e/a> <http://e/p> \"x\"@en--up .");
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
        assert!(matches!(
            &r.triples[0].o,
            Node::Literal {
                lang: Some(_),
                dir: None,
                ..
            }
        ));
    }

    /// The same text at a line start `n` tokens in: the cost of recovering must
    /// not grow with the length of the line.
    #[test]
    fn recovery_inside_a_long_trig_line_is_linear() {
        let tokens = 200_000;
        let body = "<http://e/a> ".repeat(tokens);
        for src in [
            // An error, then 200k tokens that start statements but are mid-line.
            format!(
                "<http://e/g> {{ <http://e/s> <http://e/p> ) {body} }}\n<http://e/z> <http://e/p> <http://e/o> ."
            ),
            format!("<http://e/s> <http://e/p> ) {body}\n<http://e/z> <http://e/p> <http://e/o> ."),
        ] {
            let started = std::time::Instant::now();
            let r = trig(&src);
            let took = started.elapsed();
            let limit = if cfg!(debug_assertions) { 5.0 } else { 0.5 };
            assert!(took.as_secs_f64() < limit, "{took:?} for {tokens} tokens");
            assert!(!r.errors.is_empty());
            // Recovery still stops at the next line start.
            assert!(
                r.triples
                    .iter()
                    .any(|t| matches!(t.s, Node::Iri(i) if &*i == "http://e/z")),
                "{:?}",
                r.errors
            );
        }
    }

    #[test]
    fn line_starts_are_found_from_the_gap_before_the_token() {
        // Indented statements in a block, a multi-line string and a comment
        // between tokens.
        let r = trig("<g> {\n  <a> <p> <o>\n  <b> <p> <o> .\n}\n");
        assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
        let r = p("<a> <p> \"\"\"x\n<b> <p> <o> .\n\"\"\" .\n<c> <p> <o> .");
        assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let r = p("<a> <p> <o> # c\n<b> <p> <o> .");
        assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
    }

    #[test]
    fn a_name_with_an_undeclared_prefix_marks_its_triples() {
        let r = p(
            "@prefix ex: <http://e/> .\nex:a ex:p nope:o , ex:o2 .\nnope:s ex:p ex:o .\nex:a ex:q [ nope:r 1 ] .\nex:a ex:p <<( ex:x ex:y nope:z )>> .\nex:b ex:p ex:o .",
        );
        let flags: Vec<_> = r.triples.iter().map(|t| t.unresolved).collect();
        // ex:a p nope:o | ex:a p ex:o2 | nope:s p o | _:b nope:r 1 | ex:a q _:b | ex:a p <<( )>> | ex:b p o
        assert_eq!(flags, [true, false, true, true, false, true, false]);
        assert_eq!(
            r.prefix_uses.iter().filter(|u| !u.resolved).count(),
            4,
            "{:?}",
            r.prefix_uses
        );
        // A name the document declares is resolved, however late.
        let r = p("ex:a ex:p ex:o .\n@prefix ex: <http://e/> .\nex:a ex:p ex:o .");
        let flags: Vec<_> = r.triples.iter().map(|t| t.unresolved).collect();
        assert_eq!(flags, [true, false]);
    }

    #[test]
    fn a_triple_term_holds_no_reified_triple() {
        let ok = |src: &str| {
            let r = p(&format!("@prefix ex: <http://e/> .\n{src}"));
            assert!(r.errors.is_empty(), "{src}: {:?}", r.errors);
        };
        let bad = |src: &str| {
            let r = p(&format!("@prefix ex: <http://e/> .\n{src}"));
            assert!(
                r.errors.iter().any(|e| e
                    .message()
                    .contains("triple term <<( )>> cannot contain a reified")),
                "{src}: {:?}",
                r.errors
            );
        };
        // rtSubject and rtObject take reified triples and triple terms.
        ok("<< << ex:a ex:p ex:o >> ex:q << ex:b ex:p ex:o >> >> ex:r ex:s .");
        ok("<< ex:a ex:p <<( ex:b ex:p ex:o )>> >> ex:r ex:s .");
        ok("ex:a ex:p <<( ex:b ex:p <<( ex:c ex:p ex:o )>> )>> .");
        // ttSubject and ttObject do not.
        bad("ex:a ex:p <<( << ex:b ex:p ex:o >> ex:q ex:o )>> .");
        bad("ex:a ex:p <<( ex:b ex:q << ex:b ex:p ex:o >> )>> .");
        bad("ex:a ex:p <<( ex:b ex:q <<( ex:c ex:p << ex:d ex:p ex:o >> )>> )>> .");
        // Inside a reified triple, a triple term still holds none.
        bad("<< ex:a ex:p <<( ex:b ex:q << ex:c ex:p ex:o >> )>> >> ex:r ex:s .");
    }

    #[test]
    fn absolute_iris_resolve_to_themselves_but_lose_dot_segments() {
        let r = parse_with_base(
            "<http://e/a/../b> <http://e/./p> <http://e/o#x> , <urn:a:b> , <http://e> , <http://e/a/b/> .",
            &PrefixMap::default(),
            Syntax::Turtle,
            Some("file:///w/d.ttl"),
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        let iris: Vec<_> = r.refs.iter().map(|r| r.iri.to_string()).collect();
        assert_eq!(
            iris,
            [
                "http://e/b",
                "http://e/p",
                "http://e/o#x",
                "urn:a:b",
                "http://e",
                "http://e/a/b/"
            ]
        );
        // An invalid absolute IRI is still reported once.
        let r = parse_with_base(
            "<http://e/%zz> <http://e/p> <http://e/o> .",
            &PrefixMap::default(),
            Syntax::Turtle,
            Some("file:///w/d.ttl"),
        );
        assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    }

    #[test]
    fn a_graph_only_parse_has_the_same_graph_and_nothing_else() {
        let cases = [
            (
                Syntax::Turtle,
                "@prefix ex: <http://e/> . # c1\nex:a ex:p [ ex:q ( 1 2 ) ] ; a ex:C . # c2\n<< ex:a ex:p ex:b >> ex:r nope:x .\n",
            ),
            (
                Syntax::TriG,
                "@prefix ex: <http://e/> .\nGRAPH ex:g { ex:a ex:p ex:b } # c\n{ ex:c ex:p ex:d . }\nex:broken ex:p .\n",
            ),
            (
                Syntax::NTriples,
                "<http://e/a> <http://e/p> \"x\" . # c\n<a> <http://e/p> <http://e/b> .\n",
            ),
            (
                Syntax::NQuads,
                "<http://e/a> <http://e/p> <http://e/b> <http://e/g> .\n",
            ),
            (Syntax::JsonLd, "{}"),
        ];
        let ext: PrefixMap = [("ext".to_string(), "http://ext/".to_string())]
            .into_iter()
            .collect();
        for (syntax, text) in cases {
            let full = parse_with_base(text, &ext, syntax, Some("file:///w/a"));
            let (graph, comments) = Parsed::graph_only(text, &ext, syntax, Some("file:///w/a"), 3);
            let quads = |p: &Parsed| -> Vec<String> {
                p.triples
                    .iter()
                    .map(|t| {
                        format!(
                            "{:?} {:?} {:?} {:?} {}",
                            t.s, t.p, t.o, t.graph, t.unresolved
                        )
                    })
                    .collect()
            };
            let errors = |p: &Parsed| -> Vec<String> {
                p.errors
                    .iter()
                    .map(|e| format!("{:?} {}", e.span, e.message()))
                    .collect()
            };
            assert_eq!(quads(&graph), quads(&full), "{syntax:?}");
            assert_eq!(errors(&graph), errors(&full), "{syntax:?}");
            let names = |p: &Parsed| -> Vec<_> { p.graphs.iter().map(|g| g.name).collect() };
            assert_eq!(names(&graph), names(&full), "{syntax:?}");
            assert_eq!(comments, full.comments.len(), "{syntax:?}");
            assert!(
                graph.tokens.is_empty()
                    && graph.refs.is_empty()
                    && graph.prefix_uses.is_empty()
                    && graph.comments.is_empty(),
                "{syntax:?}"
            );
            let keys = |ts: &[Token]| -> Vec<_> {
                ts.iter()
                    .map(|t| (t.kind, t.start, t.end, t.error()))
                    .collect()
            };
            assert_eq!(keys(&syntax.tokens(text)), keys(&full.tokens), "{syntax:?}");
        }
    }
}

#[cfg(test)]
mod review_tests {
    use crate::syntax::{PrefixMap, Syntax, parse};

    fn flags(text: &str) -> Vec<bool> {
        parse(text, &PrefixMap::default(), Syntax::Turtle)
            .triples
            .iter()
            .map(|t| t.unresolved)
            .collect()
    }

    /// `urn:isbn:1` with no `urn:` prefix declared is a mistake; `<urn:isbn:1>`
    /// is an absolute IRI with the same text. Only the first is unresolved,
    /// in either order, in every position.
    #[test]
    fn an_absolute_iri_spelled_like_an_undeclared_name_is_resolved() {
        let p = "<http://e.org/p>";
        assert_eq!(
            flags(&format!("<urn:isbn:1> {p} 1 .\nurn:isbn:1 {p} 2 .\n")),
            [false, true]
        );
        assert_eq!(
            flags(&format!("urn:isbn:1 {p} 2 .\n<urn:isbn:1> {p} 1 .\n")),
            [true, false]
        );
        assert_eq!(
            flags(
                "<http://e.org/s> urn:x:p <urn:x:o> .\n<http://e.org/s> <urn:x:p> urn:x:o .\n<http://e.org/s> <urn:x:p> <urn:x:o> .\n"
            ),
            [true, true, false]
        );
        // Inside a triple term, and as a reifier.
        assert_eq!(
            flags(&format!(
                "<http://e.org/s> {p} <<( <urn:x:a> {p} <urn:x:b> )>> .\n<http://e.org/s> {p} <<( urn:x:a {p} <urn:x:b> )>> .\n"
            )),
            [false, true]
        );
        assert_eq!(
            flags(&format!(
                "<http://e.org/s> {p} <urn:x:o> ~ <urn:x:r> .\n<http://e.org/s> {p} <urn:x:o> ~ urn:x:r .\n"
            )),
            [false, false, false, true]
        );
        // The graph keeps the absolute IRI's triple.
        let parsed = parse(
            &format!("urn:isbn:1 {p} 2 .\n<urn:isbn:1> {p} 1 .\n"),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        assert_eq!(crate::rdf::Graph::new(&[&parsed.triples]).len(), 1);
    }
}
