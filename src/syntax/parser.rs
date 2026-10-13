//! Resilient recursive-descent parser for Turtle 1.2, TriG 1.2 and N-Triples 1.2.
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

use super::lexer::{Kind, Token, tokenize};
use super::*;
use std::borrow::Cow;
use std::sync::OnceLock;

type R<T> = Result<T, ()>;

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
    let mut all = tokenize(src);
    if syntax != Syntax::TriG {
        // `GRAPH` is only a keyword in TriG; elsewhere it is a stray word.
        for t in all.iter_mut().filter(|t| t.kind == Kind::Graph) {
            t.kind = Kind::Error;
            t.error = Some(super::lexer::UNEXPECTED_WORD);
        }
    }
    let mut out = Parsed {
        syntax,
        ..Parsed::default()
    };
    for t in &all {
        if t.kind == Kind::Comment {
            out.comments.push(Span::new(t.start, t.end));
        }
        if let Some(msg) = t.error {
            out.errors.push(SyntaxError {
                span: Span::new(t.start, t.end),
                message: msg.to_string(),
                severity: Severity::Error,
            });
        }
    }
    let toks: Vec<Token> = all
        .iter()
        .copied()
        .filter(|t| t.kind != Kind::Comment)
        .collect();
    out.tokens = all;
    let mut p = P {
        src,
        toks,
        pos: 0,
        prefixes: external
            .iter()
            .map(|(k, v)| (k.clone(), Iri::intern(v)))
            .collect(),
        base: None,
        out,
        bnodes: 0,
        bnode_labels: FxHashMap::default(),
        quoted: 0,
        subject: None,
        statement: 0,
        trig: syntax == Syntax::TriG,
        graph: None,
        in_graph: false,
        graph_indent: None,
        iri_cache: FxHashMap::default(),
        pname_cache: FxHashMap::default(),
        buf: String::new(),
        v: vocab(),
    };
    p.document();
    if syntax == Syntax::NTriples {
        p.ntriples_restrictions();
    }
    p.out
}

struct P<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    /// Prefix name -> namespace, as declared so far.
    prefixes: FxHashMap<String, Iri>,
    base: Option<oxiri::Iri<String>>,
    out: Parsed,
    bnodes: u32,
    /// `_:label` -> its number in this document.
    bnode_labels: FxHashMap<&'a str, u32>,
    quoted: u32,
    subject: Option<(Iri, Span)>,
    /// Index of the statement being parsed.
    statement: u32,
    trig: bool,
    /// Name of the graph block being parsed (`None`: the default graph).
    graph: Option<GraphName>,
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
        self.pos += 1;
        t
    }
    fn eat(&mut self, k: Kind) -> bool {
        if self.peek_kind() == Some(k) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn text(&self, t: Token) -> &'a str {
        t.text(self.src)
    }
    /// Where a new statement can start, for recovery: column 0, or inside a
    /// graph block, first on its line and no deeper than the block's statements.
    fn at_line_start(&self, t: Token) -> bool {
        if t.start == 0 || self.src.as_bytes()[t.start as usize - 1] == b'\n' {
            return true;
        }
        match self.graph_indent {
            Some(indent) if self.in_graph => self.indent_of(t).is_some_and(|c| c <= indent),
            _ => false,
        }
    }

    /// The column of `t` if only spaces and tabs precede it on its line.
    fn indent_of(&self, t: Token) -> Option<u32> {
        let before = &self.src.as_bytes()[..t.start as usize];
        let line_start = before
            .iter()
            .rposition(|c| *c == b'\n')
            .map_or(0, |p| p + 1);
        before[line_start..]
            .iter()
            .all(|c| matches!(c, b' ' | b'\t'))
            .then_some((before.len() - line_start) as u32)
    }
    fn prev_span(&self) -> Span {
        match self.pos.checked_sub(1).and_then(|i| self.toks.get(i)) {
            Some(t) => Span::new(t.start, t.end),
            None => Span::new(0, 0),
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.out.errors.push(SyntaxError {
            span,
            message: message.into(),
            severity: Severity::Error,
        });
    }
    fn warning(&mut self, span: Span, message: impl Into<String>) {
        self.out.errors.push(SyntaxError {
            span,
            message: message.into(),
            severity: Severity::Warning,
        });
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

    fn emit(&mut self, s: &Node, p: Iri, o: Node, p_span: Span, o_span: Span) {
        self.out.triples.push(Triple {
            s: s.clone(),
            p,
            o,
            p_span,
            o_span,
            statement: self.statement,
            graph: self.graph,
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
            self.pos += 1;
        }
        while let Some(t) = self.peek() {
            if t.kind == Kind::Dot {
                self.pos += 1;
                return;
            }
            if self.in_graph && t.kind == Kind::RBrace {
                return;
            }
            if self.at_line_start(t) && self.is_starter(t.kind) {
                return;
            }
            self.pos += 1;
        }
    }

    // ---- TriG graph blocks ----

    fn kind_at(&self, i: usize) -> Option<Kind> {
        self.toks.get(i).map(|t| t.kind)
    }

    /// Length in tokens of a graph name at `i` (IRI, prefixed name, `_:b`, `[]`).
    fn label_len(&self, i: usize) -> Option<usize> {
        match self.kind_at(i)? {
            Kind::Iri | Kind::PName | Kind::BNode => Some(1),
            Kind::LBracket if self.kind_at(i + 1) == Some(Kind::RBracket) => Some(2),
            _ => None,
        }
    }

    /// `{`, `GRAPH`, or a graph name followed by `{` (TriG only).
    fn graph_block_ahead(&self) -> bool {
        if !self.trig {
            return false;
        }
        match self.kind_at(self.pos) {
            Some(Kind::LBrace | Kind::Graph) => true,
            _ => self
                .label_len(self.pos)
                .is_some_and(|n| self.kind_at(self.pos + n) == Some(Kind::LBrace)),
        }
    }

    fn graph_label(&mut self) -> R<(GraphName, Span)> {
        const WHAT: &str = "a graph name (an IRI or a blank node)";
        let Some(t) = self.peek() else {
            return self.expected(WHAT);
        };
        match self.label_len(self.pos) {
            Some(_) if matches!(t.kind, Kind::Iri | Kind::PName) => {
                let iri = self.iri(Role::Graph)?;
                Ok((GraphName::Iri(iri), Span::new(t.start, t.end)))
            }
            Some(n) => {
                let node = if n == 1 {
                    self.labelled_bnode()
                } else {
                    self.pos += 2;
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
                        self.graph_indent = self.indent_of(t);
                    }
                    let start = self.pos;
                    if self.statement().is_err() {
                        self.recover(start);
                    }
                }
            }
        }
    }

    // ---- N-Triples ----

    fn ntriples_restrictions(&mut self) {
        let mut errors = Vec::new();
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
                Kind::Iri if t.error.is_none() => {
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
            self.error(span, m);
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
                self.pos += 1;
                Ok(())
            }
            None => {
                let s = self.prev_span();
                self.error(s, "Expected '.' to end the statement");
                Ok(())
            }
            // In a graph block, the closing `}` ends the last statement.
            Some(t) if self.in_graph && t.kind == Kind::RBrace => Ok(()),
            Some(t) if self.at_line_start(t) && self.is_starter(t.kind) => {
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
            }
            _ => return self.expected("a version string like \"1.2\""),
        }
        self.end_statement(kw.kind == Kind::SparqlVersion)
    }

    // ---- statements ----

    fn statement(&mut self) -> R<()> {
        let start = self.peek().map_or(0, |t| t.start);
        self.subject = None;
        self.statement = self.out.statements.len() as u32;
        let r = self.triples();
        let subject = self.subject.take();
        let end = self.prev_span().end.max(start);
        self.out.statements.push(Statement {
            span: Span::new(start, end),
            subject,
        });
        r
    }

    fn triples(&mut self) -> R<()> {
        match self.peek_kind() {
            Some(Kind::LBracket)
                if self.toks.get(self.pos + 1).map(|t| t.kind) != Some(Kind::RBracket) =>
            {
                let s = self.blank_node_property_list()?;
                if !self.at_statement_end() {
                    self.predicate_object_list(&s)?;
                }
            }
            Some(Kind::LTriple) => {
                let s = self.reified_triple()?;
                if !self.at_statement_end() {
                    self.predicate_object_list(&s)?;
                }
            }
            _ => {
                let s = self.subject()?;
                self.predicate_object_list(&s)?;
            }
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

    fn subject(&mut self) -> R<Node> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => {
                let t = self.peek().unwrap();
                let role = if self.quoted > 0 {
                    Role::Quoted
                } else {
                    Role::Subject
                };
                let iri = self.iri(role)?;
                if role == Role::Subject && self.subject.is_none() {
                    self.subject = Some((iri, Span::new(t.start, t.end)));
                }
                Ok(Node::Iri(iri))
            }
            Some(Kind::BNode) => Ok(self.labelled_bnode()),
            Some(Kind::LBracket)
                if self.toks.get(self.pos + 1).map(|t| t.kind) == Some(Kind::RBracket) =>
            {
                self.pos += 2;
                Ok(self.fresh_bnode())
            }
            Some(Kind::LParen) => self.collection(),
            Some(Kind::LTriple) => self.reified_triple(),
            _ => self.expected("a subject"),
        }
    }

    fn predicate_object_list(&mut self, s: &Node) -> R<()> {
        loop {
            let (p, p_span) = self.verb()?;
            self.object_list(s, p, p_span)?;
            if self.peek_kind() != Some(Kind::Semicolon) {
                return Ok(());
            }
            while self.eat(Kind::Semicolon) {}
            if !matches!(self.peek_kind(), Some(Kind::Iri | Kind::PName | Kind::A)) {
                return Ok(());
            }
        }
    }

    /// The predicate and its span.
    fn verb(&mut self) -> R<(Iri, Span)> {
        let role = if self.quoted > 0 {
            Role::Quoted
        } else {
            Role::Predicate
        };
        match self.peek() {
            Some(t) if t.kind == Kind::A => {
                self.bump();
                let span = Span::new(t.start, t.end);
                self.out.refs.push(Ref {
                    span,
                    iri: self.v.rdf_type,
                    role,
                    form: Form::A,
                });
                Ok((self.v.rdf_type, span))
            }
            Some(t) if matches!(t.kind, Kind::Iri | Kind::PName) => {
                Ok((self.iri(role)?, Span::new(t.start, t.end)))
            }
            _ => self.expected("a predicate"),
        }
    }

    fn object_list(&mut self, s: &Node, p: Iri, p_span: Span) -> R<()> {
        loop {
            let start = self.start();
            let o = self.object()?;
            let o_span = self.span_from(start);
            self.emit(s, p, o.clone(), p_span, o_span);
            self.annotation(s, p, p_span, &o, o_span)?;
            if !self.eat(Kind::Comma) {
                return Ok(());
            }
        }
    }

    fn object(&mut self) -> R<Node> {
        let role = if self.quoted > 0 {
            Role::Quoted
        } else {
            Role::Object
        };
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => Ok(Node::Iri(self.iri(role)?)),
            Some(Kind::BNode) => Ok(self.labelled_bnode()),
            Some(Kind::LBracket) => {
                if self.toks.get(self.pos + 1).map(|t| t.kind) == Some(Kind::RBracket) {
                    self.pos += 2;
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
                })
            }
            Some(Kind::Boolean) => {
                let t = self.bump();
                Ok(Node::Literal {
                    value: self.text(t).into(),
                    datatype: self.v.boolean,
                    lang: None,
                })
            }
            Some(Kind::LTripleTerm) => self.triple_term(),
            Some(Kind::LTriple) => self.reified_triple(),
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
                let (lang, dir) = match tag.split_once("--") {
                    Some((l, d)) => (l, Some(d)),
                    None => (tag, None),
                };
                if let Err(e) = oxilangtag::LanguageTag::parse(lang) {
                    self.warning(
                        Span::new(lt.start, lt.end),
                        format!("Invalid language tag '{lang}': {e}"),
                    );
                }
                if let Some(d) = dir
                    && d != "ltr"
                    && d != "rtl"
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
                let lang = match self.iri_cache.get(self.text(lt)) {
                    Some(&l) => l,
                    None => {
                        let l = Iri::intern(&tag.to_ascii_lowercase());
                        self.iri_cache.insert(self.text(lt), l);
                        l
                    }
                };
                Ok(Node::Literal {
                    value,
                    datatype,
                    lang: Some(lang),
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
                Ok(Node::Literal {
                    value,
                    datatype,
                    lang: None,
                })
            }
            _ => Ok(Node::Literal {
                value,
                datatype: self.v.string,
                lang: None,
            }),
        }
    }

    fn blank_node_property_list(&mut self) -> R<Node> {
        let open = self.bump();
        let b = self.fresh_bnode();
        self.predicate_object_list(&b)?;
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
        let open = self.bump();
        let mut items = Vec::new();
        loop {
            match self.peek_kind() {
                Some(Kind::RParen) => {
                    self.pos += 1;
                    break;
                }
                None => {
                    self.error(Span::new(open.start, open.end), "Unclosed '('");
                    return Err(());
                }
                _ => {
                    let start = self.start();
                    let item = self.object()?;
                    items.push((item, self.span_from(start)));
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
        for (i, (item, span)) in items.into_iter().enumerate() {
            self.emit(&cur, first, item, span, span);
            let next = if i + 1 == n {
                nil.clone()
            } else {
                self.fresh_bnode()
            };
            self.emit(&cur, rest, next.clone(), span, span);
            cur = next;
        }
        Ok(head)
    }

    /// `<<( s p o )>>`
    fn triple_term(&mut self) -> R<Node> {
        self.bump();
        self.quoted += 1;
        let r = (|| {
            let s = self.quoted_subject()?;
            let (p, _) = self.verb()?;
            let o = self.object()?;
            if !self.eat(Kind::RTripleTerm) {
                return self.expected("')>>'");
            }
            Ok(Node::Triple(Box::new((s, p, o))))
        })();
        self.quoted -= 1;
        r
    }

    fn quoted_subject(&mut self) -> R<Node> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => Ok(Node::Iri(self.iri(Role::Quoted)?)),
            Some(Kind::BNode) => Ok(self.labelled_bnode()),
            Some(Kind::LBracket)
                if self.toks.get(self.pos + 1).map(|t| t.kind) == Some(Kind::RBracket) =>
            {
                self.pos += 2;
                Ok(self.fresh_bnode())
            }
            Some(Kind::LTriple) => self.reified_triple(),
            _ => self.expected("a subject"),
        }
    }

    /// `<< s p o ~ r >>`: returns the reifier.
    fn reified_triple(&mut self) -> R<Node> {
        let open = self.bump();
        self.quoted += 1;
        let r = (|| {
            let s = self.quoted_subject()?;
            let (p, _) = self.verb()?;
            let o = self.object()?;
            let reifier = if self.eat(Kind::Tilde) {
                self.reifier()?
            } else {
                self.fresh_bnode()
            };
            if !self.eat(Kind::RTriple) {
                return self.expected("'>>'");
            }
            Ok((reifier, Node::Triple(Box::new((s, p, o)))))
        })();
        self.quoted -= 1;
        let (reifier, triple) = r?;
        let span = self.span_from(open.start);
        self.emit(&reifier, self.v.reifies, triple, span, span);
        Ok(reifier)
    }

    fn reifier(&mut self) -> R<Node> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => Ok(Node::Iri(self.iri(Role::Object)?)),
            Some(Kind::BNode) => Ok(self.labelled_bnode()),
            _ => Ok(self.fresh_bnode()),
        }
    }

    /// `~ reifier` and `{| ... |}` after an object. The `rdf:reifies` triples
    /// point at the annotated predicate and object.
    fn annotation(&mut self, s: &Node, p: Iri, p_span: Span, o: &Node, o_span: Span) -> R<()> {
        let mut last: Option<Node> = None;
        let reifies = self.v.reifies;
        loop {
            match self.peek_kind() {
                Some(Kind::Tilde) => {
                    self.bump();
                    let r = self.reifier()?;
                    self.emit(
                        &r,
                        reifies,
                        Node::Triple(Box::new((s.clone(), p, o.clone()))),
                        p_span,
                        o_span,
                    );
                    last = Some(r);
                }
                Some(Kind::LAnnotation) => {
                    self.bump();
                    let r = match last.take() {
                        Some(r) => r,
                        None => {
                            let r = self.fresh_bnode();
                            self.emit(
                                &r,
                                reifies,
                                Node::Triple(Box::new((s.clone(), p, o.clone()))),
                                p_span,
                                o_span,
                            );
                            r
                        }
                    };
                    self.predicate_object_list(&r)?;
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
            self.out.refs.push(Ref {
                span,
                iri,
                role,
                form: Form::Iri,
            });
            return Ok(iri);
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
        self.out.prefix_uses.push(PrefixUse {
            name,
            span,
            resolved,
        });
        if resolved {
            self.out.refs.push(Ref {
                span,
                iri,
                role,
                form: Form::PName {
                    prefix_len: colon as u32,
                },
            });
        }
        Ok(iri)
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

fn has_scheme(s: &str) -> bool {
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

    fn messages(r: &Parsed) -> Vec<&str> {
        r.errors.iter().map(|e| e.message.as_str()).collect()
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
                .any(|e| e.message.contains("Unterminated string"))
        );
        assert!(r.errors.iter().any(|e| e.message.contains("Expected '.'")));
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
        assert_eq!(
            Syntax::detect(None, "file:///x.trig/a.turtle"),
            Syntax::Turtle
        );
        assert_eq!(Syntax::detect(None, "untitled:Untitled-1"), Syntax::Turtle);
        assert_eq!(Syntax::from_extension("C:\\x\\a.trig"), Some(Syntax::TriG));
        assert_eq!(Syntax::from_extension("/x/notes.txt"), None);
    }

    #[test]
    fn string_values() {
        assert_eq!(string_value("\"a\\nb\""), "a\nb");
        assert_eq!(string_value("'''x''y'''"), "x''y");
        assert_eq!(string_value("\"\\u00e9\""), "é");
    }
}
