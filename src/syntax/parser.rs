//! Resilient recursive-descent parser for Turtle 1.2.
//!
//! On a syntax error the current statement is abandoned and parsing resumes at
//! the next `.`, or at the next line that starts at column 0 with something that
//! can begin a statement. Triples and references found before the error are kept,
//! so completion and navigation keep working while a statement is half typed.

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

pub fn parse(src: &str, external: &PrefixMap) -> Parsed {
    let all = tokenize(src);
    let mut out = Parsed::default();
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
        iri_cache: FxHashMap::default(),
        pname_cache: FxHashMap::default(),
        buf: String::new(),
        v: vocab(),
    };
    p.document();
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
    fn at_line_start(&self, t: Token) -> bool {
        t.start == 0 || self.src.as_bytes()[t.start as usize - 1] == b'\n'
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
                _ => self.statement(),
            };
            if ok.is_err() {
                self.recover(start);
            }
        }
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
            if self.at_line_start(t) && is_starter(t.kind) {
                return;
            }
            self.pos += 1;
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
            Some(t) if self.at_line_start(t) && is_starter(t.kind) => {
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
                if self.peek_kind() != Some(Kind::Dot) {
                    self.predicate_object_list(&s)?;
                }
            }
            Some(Kind::LTriple) => {
                let s = self.reified_triple()?;
                if self.peek_kind() != Some(Kind::Dot) {
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
        parse(src, &PrefixMap::default())
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
        let r = parse("ex:a ex:p ex:o .", &ext);
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
    fn string_values() {
        assert_eq!(string_value("\"a\\nb\""), "a\nb");
        assert_eq!(string_value("'''x''y'''"), "x''y");
        assert_eq!(string_value("\"\\u00e9\""), "é");
    }
}
