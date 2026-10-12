//! Resilient recursive-descent parser for Turtle 1.2.
//!
//! On a syntax error the current statement is abandoned and parsing resumes at
//! the next `.`, or at the next line that starts at column 0 with something that
//! can begin a statement. Triples and references found before the error are kept,
//! so completion and navigation keep working while a statement is half typed.

use super::lexer::{Kind, Token, tokenize};
use super::*;

type R<T> = Result<T, ()>;

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
        prefixes: external.clone(),
        base: None,
        out,
        bnodes: 0,
        quoted: 0,
        subject: None,
    };
    p.document();
    p.out
}

struct P<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    prefixes: PrefixMap,
    base: Option<oxiri::Iri<String>>,
    out: Parsed,
    bnodes: u32,
    quoted: u32,
    subject: Option<(String, Span)>,
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
        Node::Blank(format!("_:g{}", self.bnodes))
    }

    fn emit(&mut self, s: &Node, p: &str, o: Node) {
        self.out.triples.push(Triple {
            s: s.clone(),
            p: p.to_string(),
            o,
        });
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
        let iri = self.resolve_iri(iri_tok);
        self.end_statement(sparql)?;
        self.prefixes.insert(name.clone(), iri.clone());
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
        let iri = self.resolve_iri(iri_tok);
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
                    self.subject = Some((iri.clone(), Span::new(t.start, t.end)));
                }
                Ok(Node::Iri(iri))
            }
            Some(Kind::BNode) => {
                let t = self.bump();
                Ok(Node::Blank(self.text(t).to_string()))
            }
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
            let p = self.verb()?;
            self.object_list(s, &p)?;
            if self.peek_kind() != Some(Kind::Semicolon) {
                return Ok(());
            }
            while self.eat(Kind::Semicolon) {}
            if !matches!(self.peek_kind(), Some(Kind::Iri | Kind::PName | Kind::A)) {
                return Ok(());
            }
        }
    }

    fn verb(&mut self) -> R<String> {
        match self.peek_kind() {
            Some(Kind::A) => {
                let t = self.bump();
                let role = if self.quoted > 0 {
                    Role::Quoted
                } else {
                    Role::Predicate
                };
                self.out.refs.push(Ref {
                    span: Span::new(t.start, t.end),
                    iri: RDF_TYPE.to_string(),
                    role,
                    form: Form::A,
                });
                Ok(RDF_TYPE.to_string())
            }
            Some(Kind::Iri | Kind::PName) => {
                let role = if self.quoted > 0 {
                    Role::Quoted
                } else {
                    Role::Predicate
                };
                self.iri(role)
            }
            _ => self.expected("a predicate"),
        }
    }

    fn object_list(&mut self, s: &Node, p: &str) -> R<()> {
        loop {
            let o = self.object()?;
            self.emit(s, p, o.clone());
            self.annotation(s, p, &o)?;
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
            Some(Kind::BNode) => {
                let t = self.bump();
                Ok(Node::Blank(self.text(t).to_string()))
            }
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
                let dt = if text.contains(['e', 'E']) {
                    "double"
                } else if text.contains('.') {
                    "decimal"
                } else {
                    "integer"
                };
                Ok(Node::Literal {
                    value: text.to_string(),
                    datatype: format!("{XSD}{dt}"),
                    lang: None,
                })
            }
            Some(Kind::Boolean) => {
                let t = self.bump();
                Ok(Node::Literal {
                    value: self.text(t).to_string(),
                    datatype: format!("{XSD}boolean"),
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
        let value = string_value(self.text(t));
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
                    format!("{RDF}dirLangString")
                } else {
                    format!("{RDF}langString")
                };
                Ok(Node::Literal {
                    value,
                    datatype,
                    lang: Some(tag.to_ascii_lowercase()),
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
                datatype: format!("{XSD}string"),
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
                _ => items.push(self.object()?),
            }
        }
        let nil = Node::Iri(format!("{RDF}nil"));
        if items.is_empty() {
            return Ok(nil);
        }
        let head = self.fresh_bnode();
        let mut cur = head.clone();
        let n = items.len();
        for (i, item) in items.into_iter().enumerate() {
            self.emit(&cur, &format!("{RDF}first"), item);
            let next = if i + 1 == n {
                nil.clone()
            } else {
                self.fresh_bnode()
            };
            self.emit(&cur, &format!("{RDF}rest"), next.clone());
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
            let p = self.verb()?;
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
            Some(Kind::BNode) => {
                let t = self.bump();
                Ok(Node::Blank(self.text(t).to_string()))
            }
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
        self.bump();
        self.quoted += 1;
        let r = (|| {
            let s = self.quoted_subject()?;
            let p = self.verb()?;
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
        self.emit(&reifier, &format!("{RDF}reifies"), triple);
        Ok(reifier)
    }

    fn reifier(&mut self) -> R<Node> {
        match self.peek_kind() {
            Some(Kind::Iri | Kind::PName) => Ok(Node::Iri(self.iri(Role::Object)?)),
            Some(Kind::BNode) => {
                let t = self.bump();
                Ok(Node::Blank(self.text(t).to_string()))
            }
            _ => Ok(self.fresh_bnode()),
        }
    }

    /// `~ reifier` and `{| ... |}` after an object.
    fn annotation(&mut self, s: &Node, p: &str, o: &Node) -> R<()> {
        let mut last: Option<Node> = None;
        loop {
            match self.peek_kind() {
                Some(Kind::Tilde) => {
                    self.bump();
                    let r = self.reifier()?;
                    self.emit(
                        &r,
                        &format!("{RDF}reifies"),
                        Node::Triple(Box::new((s.clone(), p.to_string(), o.clone()))),
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
                                &format!("{RDF}reifies"),
                                Node::Triple(Box::new((s.clone(), p.to_string(), o.clone()))),
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
    fn iri(&mut self, role: Role) -> R<String> {
        let t = self.bump();
        let span = Span::new(t.start, t.end);
        if t.kind == Kind::Iri {
            let iri = self.resolve_iri(t);
            self.out.refs.push(Ref {
                span,
                iri: iri.clone(),
                role,
                form: Form::Iri,
            });
            return Ok(iri);
        }
        let text = self.text(t);
        let colon = text.find(':').unwrap_or(text.len());
        let (name, local) = (&text[..colon], &text[colon + 1..]);
        let resolved = self.prefixes.get(name).cloned();
        self.out.prefix_uses.push(PrefixUse {
            name: name.to_string(),
            span,
            resolved: resolved.is_some(),
        });
        match resolved {
            Some(ns) => {
                let iri = format!("{ns}{}", unescape_local(local));
                self.out.refs.push(Ref {
                    span,
                    iri: iri.clone(),
                    role,
                    form: Form::PName {
                        prefix_len: colon as u32,
                    },
                });
                Ok(iri)
            }
            None => Ok(text.to_string()),
        }
    }

    fn resolve_iri(&mut self, t: Token) -> String {
        let text = self.text(t);
        let raw = text
            .strip_prefix('<')
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or(text);
        let raw = if raw.contains('\\') {
            unescape_iri(raw)
        } else {
            raw.to_string()
        };
        match &self.base {
            Some(base) => match oxiri::IriRef::parse(raw.as_str()).and_then(|r| base.resolve(&r)) {
                Ok(iri) => iri.into_inner(),
                Err(e) => {
                    self.warning(Span::new(t.start, t.end), format!("Invalid IRI: {e}"));
                    raw
                }
            },
            None => {
                if has_scheme(&raw)
                    && let Err(e) = oxiri::Iri::parse(raw.as_str())
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

fn unescape_local(local: &str) -> String {
    if !local.contains('\\') {
        return local.to_string();
    }
    let mut out = String::with_capacity(local.len());
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
    out
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
            .filter_map(|s| s.subject.as_ref().map(|x| x.0.as_str()))
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
    fn string_values() {
        assert_eq!(string_value("\"a\\nb\""), "a\nb");
        assert_eq!(string_value("'''x''y'''"), "x''y");
        assert_eq!(string_value("\"\\u00e9\""), "é");
    }
}
