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
//! Every syntax error and warning is kept, in the order found: the lexer's
//! first, then the parser's, then the N-Triples restrictions. Each costs a
//! span and, for a fixed message, a borrowed string. The editor shows the
//! first `MAX_SYNTAX_ERRORS` of a document;
//! `turtle-lsp check` reports them all.

use super::lexer::{Kind, Token, tokenize};
use super::*;
use std::borrow::Cow;
use std::sync::OnceLock;

type R<T> = Result<T, ()>;

#[cfg(test)]
thread_local! {
    /// Token steps plus bytes read by line-start checks: a parse's cost without a clock.
    static WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn work(n: usize) {
    #[cfg(test)]
    WORK.with(|w| w.set(w.get() + n as u64));
    #[cfg(not(test))]
    let _ = n;
}

/// Deepest nesting of `[ ]`, `( )`, `<< >>`, `<<( )>>` and `{| |}` the parser follows.
/// Real data rarely goes past a dozen; thousands only come from generated or
/// hostile input, and would overflow a worker thread's stack.
pub(crate) const MAX_NESTING: u32 = 256;

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
/// RDF/XML, OWL Functional syntax or OWL/XML, which have their own
/// frontends: given [`Syntax::JsonLd`], [`Syntax::RdfXml`],
/// [`Syntax::OwlFunctional`] or [`Syntax::OwlXml`] it returns no triples
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
    pub(crate) fn graph_only(
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
    /// what it keeps as [`Parsed::tokens`]. None for JSON-LD, RDF/XML, OWL
    /// Functional syntax and OWL/XML, which have their own frontends.
    pub(crate) fn tokens(self, src: &str) -> Vec<Token> {
        match self {
            Syntax::JsonLd | Syntax::RdfXml | Syntax::OwlFunctional | Syntax::OwlXml => {
                return Vec::new();
            }
            Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads => {}
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
    let frontend = match syntax {
        Syntax::JsonLd => Some("JSON-LD documents are parsed by jsonld::parse"),
        Syntax::RdfXml => Some("RDF/XML documents are parsed by rdfxml::parse"),
        Syntax::OwlFunctional => Some("OWL Functional documents are parsed by ofn::parse"),
        Syntax::OwlXml => Some("OWL/XML documents are parsed by owx::parse"),
        Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads => None,
    };
    if let Some(message) = frontend {
        let refused = Parsed {
            syntax,
            document_base: base.map(str::to_string),
            errors: vec![SyntaxError::new(
                Span::new(0, 0),
                message,
                Severity::Error,
                "syntax",
            )],
            ..Parsed::default()
        };
        return (refused, 0);
    }
    let toks = syntax.tokens(src);
    let full = graph_only.is_none();
    let mut p = P::new(
        src,
        &toks,
        syntax,
        Start::document(external, base),
        full,
        None,
    );
    p.out.document_base = base.map(str::to_string);
    p.out.triples.reserve(graph_only.unwrap_or(0));
    let mut comments = 0;
    for t in &toks {
        if t.kind == Kind::Comment {
            comments += 1;
            if full {
                p.out.comments.push(Span::new(t.start, t.end));
            }
        }
        if let Some(e) = lexer_error(t) {
            p.out.errors.push(e);
        }
    }
    p.document();
    if syntax.is_line_based() {
        p.ntriples_restrictions();
    }
    let mut out = p.out;
    if full {
        out.tokens = toks;
    }
    (out, comments)
}

/// The state of the parser at the top of its statement loop, where a parse
/// can start: a full parse at the first token in the initial state, a splice
/// ([`super::splice`]) at the first token of an old statement, in the state
/// the full parse had there.
#[derive(Clone)]
pub(crate) struct Start {
    /// Prefix name -> namespace in effect: the external ones, then those
    /// declared before.
    pub(crate) prefixes: FxHashMap<String, Iri>,
    /// The base in effect.
    pub(crate) base: Option<oxiri::Iri<String>>,
    /// The index the first statement parsed gets.
    pub(crate) statement: u32,
    /// Inside a TriG graph block: the block's name, and the indentation of
    /// its first statement that starts a line, if one has been parsed.
    pub(crate) block: Option<(Option<GraphName>, Option<u32>)>,
    /// The token to start at, and the token before it that is not a comment.
    pub(crate) at: usize,
    pub(crate) prev: Option<usize>,
}

impl Start {
    /// The start of a document: the first token, the external prefixes and
    /// the document's base.
    pub(crate) fn document(external: &PrefixMap, base: Option<&str>) -> Start {
        Start {
            prefixes: external
                .iter()
                .map(|(k, v)| (k.clone(), Iri::intern(v)))
                .collect(),
            base: base.and_then(|b| oxiri::Iri::parse(b.to_string()).ok()),
            statement: 0,
            block: None,
            at: 0,
            prev: None,
        }
    }
}

/// The parser at the top of its statement loop, as a window parse's `until`
/// sees it: the current token (never a comment) and the token before it that
/// is not a comment.
#[derive(Clone, Copy)]
pub(crate) struct Top {
    pub(crate) pos: usize,
    pub(crate) prev: Option<usize>,
}

/// Whether a window parse goes on past the [`Top`] it is at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Halt {
    Go,
    Stop,
}

/// How a window parse ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    /// `until` stopped it at this token.
    Stopped(usize),
    /// At the end of the tokens, outside a graph block.
    End,
    /// At a directive or a graph block, or out of the block it started in,
    /// at this token: the state a splice keeps would change.
    Left(usize),
}

/// The statements of a window parse ([`window`]).
pub(crate) struct Window {
    /// The statements, triples, references, prefix uses and parser errors
    /// (not the lexer's, nor the N-Triples restrictions).
    pub(crate) out: Parsed,
    pub(crate) ended: Ended,
    /// Blank nodes allocated, numbered from 1.
    pub(crate) bnodes: u32,
    /// The graph block's indentation at the end ([`Start::block`]).
    pub(crate) indent: Option<u32>,
    /// The deepest nesting reached.
    pub(crate) deepest: u32,
}

/// Parse the statements of `toks` from `start` until `until` says stop at the
/// top of the statement loop, the tokens end, or a directive or graph block
/// comes (that `until` does not stop at). The same parser as a full parse, from a given state; blank nodes
/// are numbered from 1, and no `_:label` is known.
pub(crate) fn window(
    src: &str,
    toks: &[Token],
    syntax: Syntax,
    start: Start,
    until: &mut dyn FnMut(Top) -> Halt,
) -> Window {
    let in_block = start.block.is_some();
    let mut p = P::new(src, toks, syntax, start, true, Some(until));
    if in_block {
        p.graph_body(Span::default());
    } else {
        p.document();
    }
    let ended = p.ended.unwrap_or(if in_block {
        Ended::Left(p.pos)
    } else {
        Ended::End
    });
    Window {
        ended,
        bnodes: p.bnodes,
        indent: p.graph_indent,
        deepest: p.deepest,
        out: p.out,
    }
}

/// The error for a token the lexer found malformed.
pub(crate) fn lexer_error(t: &Token) -> Option<SyntaxError> {
    let msg = t.error()?;
    Some(SyntaxError::new(
        Span::new(t.start, t.end),
        msg,
        Severity::Error,
        "syntax",
    ))
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
    toks: &'a [Token],
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
    /// Index of the first statement this parse pushes ([`Start::statement`]).
    first_statement: u32,
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
    /// A window parse's stopping rule ([`window`]); `None` in a full parse.
    until: Option<&'a mut dyn FnMut(Top) -> Halt>,
    /// How a window parse ended, once it has.
    ended: Option<Ended>,
    /// The deepest `depth` reached.
    deepest: u32,
}

/// The column of a token starting at `start` if only spaces and tabs precede
/// it on its line; `from` is the end of the token before it that is not a
/// comment.
pub(crate) fn line_indent(src: &str, from: Option<u32>, start: u32) -> Option<u32> {
    let from = from.unwrap_or(0) as usize;
    let gap = &src.as_bytes()[from..start as usize];
    work(gap.len());
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

const NT_DIRECTIVES: &str = "N-Triples does not allow directives";
const NT_PNAME: &str = "N-Triples needs full IRIs like <http://example.org/x>, not prefixed names";
const NT_A: &str =
    "N-Triples does not allow 'a': write <http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const NT_ABBREVIATION: &str = "N-Triples does not allow ';' or ',': write every triple in full";
const NT_BRACKETS: &str = "N-Triples does not allow '[ ]': use a blank node label like _:b1";
const NT_COLLECTION: &str = "N-Triples does not allow collections '( )'";
const NT_UNQUOTED: &str =
    "N-Triples literals must be quoted, e.g. \"1\"^^<http://www.w3.org/2001/XMLSchema#integer>";
const NT_REIFIED: &str =
    "N-Triples does not allow reified triples or annotations: use a triple term <<( s p o )>>";
const NT_STRING: &str = "N-Triples strings use double quotes on one line: \"...\"";
const NT_RELATIVE: &str = "N-Triples needs absolute IRIs (no relative IRIs, no @base)";
const NT_ONE_PER_LINE: &str = "N-Triples allows one triple per line";
const NT_ONE_LINE: &str = "An N-Triples triple must be on one line";

/// What the N-Triples restrictions say about tokens, then about statements:
/// the third group of [`Parsed::errors`], in that order.
const TOKEN_RESTRICTIONS: [&str; 10] = [
    NT_DIRECTIVES,
    NT_PNAME,
    NT_A,
    NT_ABBREVIATION,
    NT_BRACKETS,
    NT_COLLECTION,
    NT_UNQUOTED,
    NT_REIFIED,
    NT_STRING,
    NT_RELATIVE,
];
const STATEMENT_RESTRICTIONS: [&str; 2] = [NT_ONE_PER_LINE, NT_ONE_LINE];

/// The tokens N-Triples has no use for, and what to say about them.
fn ntriples_forbids(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::PrefixDirective
        | Kind::BaseDirective
        | Kind::VersionDirective
        | Kind::SparqlPrefix
        | Kind::SparqlBase
        | Kind::SparqlVersion => NT_DIRECTIVES,
        Kind::PName => NT_PNAME,
        Kind::A => NT_A,
        Kind::Semicolon | Kind::Comma => NT_ABBREVIATION,
        Kind::LBracket => NT_BRACKETS,
        Kind::LParen => NT_COLLECTION,
        Kind::Number | Kind::Boolean => NT_UNQUOTED,
        Kind::LTriple | Kind::Tilde | Kind::LAnnotation => NT_REIFIED,
        _ => return None,
    })
}

/// A restriction's message, named for N-Quads when `quads`.
fn restriction_message(m: &'static str, quads: bool) -> Cow<'static, str> {
    if quads {
        Cow::Owned(
            m.replace("one triple per line", "one quad per line")
                .replace("N-Triples triple", "N-Quads quad")
                .replace("N-Triples", "N-Quads"),
        )
    } else {
        Cow::Borrowed(m)
    }
}

fn restriction(span: Span, m: &'static str, quads: bool) -> SyntaxError {
    SyntaxError::new(
        span,
        restriction_message(m, quads),
        Severity::Error,
        "syntax",
    )
}

/// What N-Triples (N-Quads when `quads`) forbids in token `t`, if anything.
pub(crate) fn token_restriction(src: &str, t: &Token, quads: bool) -> Option<SyntaxError> {
    let m = match t.kind {
        Kind::String => {
            let text = t.text(src);
            (text.starts_with('\'') || text.starts_with("\"\"\"")).then_some(NT_STRING)
        }
        Kind::Iri if t.error().is_none() => {
            let text = t.text(src);
            let raw = &text[1..text.len() - 1];
            let raw = if raw.contains('\\') {
                Cow::Owned(unescape_iri(raw))
            } else {
                Cow::Borrowed(raw)
            };
            (!has_scheme(&raw)).then_some(NT_RELATIVE)
        }
        // N-Triples 1.2 allows `VERSION "1.2"`, in upper case.
        Kind::SparqlVersion if t.text(src) == "VERSION" => None,
        k => ntriples_forbids(k),
    }?;
    Some(restriction(Span::new(t.start, t.end), m, quads))
}

/// What N-Triples forbids in a statement at `span`, after one that ends at
/// `prev_end`: one triple per line, each on a single line.
pub(crate) fn statement_restriction(
    src: &str,
    prev_end: Option<u32>,
    span: Span,
    quads: bool,
) -> Option<SyntaxError> {
    let newline = |a: u32, b: u32| {
        let (a, b) = (a.min(b) as usize, a.max(b) as usize);
        src.as_bytes()[a..b].contains(&b'\n')
    };
    let m = if prev_end.is_some_and(|end| !newline(end, span.start)) {
        NT_ONE_PER_LINE
    } else if newline(span.start, span.end) {
        NT_ONE_LINE
    } else {
        return None;
    };
    Some(restriction(span, m, quads))
}

/// Which part of the N-Triples restrictions an error is from, if it is one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Restriction {
    Token,
    Statement,
}

/// Whether `e` is an N-Triples (N-Quads when `quads`) restriction, and
/// which part. The parser and the lexer never say what a restriction says.
pub(crate) fn restriction_of(e: &SyntaxError, quads: bool) -> Option<Restriction> {
    /// Every restriction's message, for N-Triples and for N-Quads.
    type Texts = [Vec<(Restriction, Cow<'static, str>)>; 2];
    static TEXTS: OnceLock<Texts> = OnceLock::new();
    let texts = TEXTS.get_or_init(|| {
        [false, true].map(|quads| {
            let token = TOKEN_RESTRICTIONS.map(|m| (Restriction::Token, m));
            let statement = STATEMENT_RESTRICTIONS.map(|m| (Restriction::Statement, m));
            token
                .into_iter()
                .chain(statement)
                .map(|(r, m)| (r, restriction_message(m, quads)))
                .collect()
        })
    });
    texts[usize::from(quads)]
        .iter()
        .find(|(_, m)| *m == e.message())
        .map(|(r, _)| *r)
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
    fn new(
        src: &'a str,
        toks: &'a [Token],
        syntax: Syntax,
        start: Start,
        full: bool,
        until: Option<&'a mut dyn FnMut(Top) -> Halt>,
    ) -> P<'a> {
        let (graph, graph_indent) = start.block.unwrap_or_default();
        let mut p = P {
            src,
            toks,
            full,
            pos: start.at,
            prev: start.prev,
            prefixes: start.prefixes,
            base: start.base,
            out: Parsed {
                syntax,
                ..Parsed::default()
            },
            bnodes: 0,
            bnode_labels: FxHashMap::default(),
            quoted: 0,
            depth: 0,
            subject: None,
            statement: start.statement,
            first_statement: start.statement,
            trig: syntax == Syntax::TriG,
            quads: syntax == Syntax::NQuads,
            graph,
            label: None,
            in_graph: start.block.is_some(),
            graph_indent,
            iri_cache: FxHashMap::default(),
            pname_cache: FxHashMap::default(),
            buf: String::new(),
            v: vocab(),
            until,
            ended: None,
            deepest: 0,
        };
        p.pos = p.skip_comments(p.pos);
        p
    }

    /// The index the next statement pushed gets.
    fn next_statement(&self) -> u32 {
        self.first_statement + self.out.statements.len() as u32
    }

    /// At the top of the statement loop of a window parse: whether to stop
    /// here, for `until` or because a directive or a graph block comes.
    fn halted(&mut self) -> bool {
        let top = Top {
            pos: self.pos,
            prev: self.prev,
        };
        let Some(until) = self.until.as_mut() else {
            return false;
        };
        if until(top) == Halt::Stop {
            self.ended = Some(Ended::Stopped(self.pos));
            return true;
        }
        let structural = matches!(
            self.peek_kind(),
            Some(
                Kind::PrefixDirective
                    | Kind::SparqlPrefix
                    | Kind::BaseDirective
                    | Kind::SparqlBase
                    | Kind::VersionDirective
                    | Kind::SparqlVersion
            )
        ) || self.graph_block_ahead();
        if structural {
            self.ended = Some(Ended::Left(self.pos));
        }
        structural
    }

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
        work(1);
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
        let from = self.prev.map(|i| self.toks[i].end);
        line_indent(self.src, from, t.start)
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
        let e = SyntaxError::new(span, message, Severity::Error, "syntax");
        self.out.errors.push(e);
    }
    fn warning(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        let e = SyntaxError::new(span, message, Severity::Warning, "syntax");
        self.out.errors.push(e);
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
            if self.halted() {
                return;
            }
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
        let first_statement = self.next_statement();
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
        let statements = self.next_statement();
        let g = &mut self.out.graphs[index];
        g.span.end = g.span.end.max(end);
        g.statements.end = statements;
        Ok(())
    }

    /// The statements of a block up to and including its `}`.
    fn graph_body(&mut self, open: Span) {
        loop {
            let Some(t) = self.peek() else {
                if self.until.is_some() {
                    self.ended = Some(Ended::Left(self.pos));
                    return;
                }
                self.error(open, "Unclosed '{': expected '}' to close the graph block");
                return;
            };
            if self.halted() {
                return;
            }
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
        for t in self.toks {
            if let Some(e) = token_restriction(self.src, t, self.quads) {
                self.out.errors.push(e);
            }
        }
        let mut prev = None;
        for st in &self.out.statements {
            if let Some(e) = statement_restriction(self.src, prev, st.span, self.quads) {
                self.out.errors.push(e);
            }
            prev = Some(st.span.end);
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
        self.statement = self.next_statement();
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
        self.deepest = self.deepest.max(self.depth);
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
                    let warnings = self.out.errors.len();
                    let iri = Iri::intern(&self.resolve_iri(t));
                    // Invalid IRIs are not cached, so each occurrence is reported.
                    if self.out.errors.len() == warnings {
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
pub(crate) fn has_scheme(s: &str) -> bool {
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
pub(crate) fn string_value(tok: &str) -> String {
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
mod tests;

#[cfg(test)]
mod review_tests;
