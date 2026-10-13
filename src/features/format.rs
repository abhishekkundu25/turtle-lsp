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
//! - an object list on one line when it fits the line width, else one object
//!   per line;
//! - `[ ]`, `( )` and `{| |}` on one line when short and flat (nothing nested),
//!   else broken with their contents one level in; `<< >>` and `<<( )>>` always
//!   on one line;
//! - an own-line comment stays above the token after it, a trailing comment
//!   stays at the end of the line its token lands on.

use crate::analysis::{Analysis, Encoding};
use crate::syntax::lexer::{Kind, Token, tokenize};
use crate::syntax::{Parsed, PrefixMap, Severity, Triple, parse};
use lsp_types::{FormattingOptions, Position, Range, TextEdit};
use rustc_hash::FxHashMap;

pub const DEFAULT_LINE_WIDTH: usize = 100;

#[derive(Clone, Debug)]
pub struct Options {
    pub tab_size: u32,
    pub insert_spaces: bool,
    pub line_width: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            tab_size: 4,
            insert_spaces: true,
            line_width: DEFAULT_LINE_WIDTH,
        }
    }
}

impl Options {
    pub fn from_lsp(o: &FormattingOptions, line_width: usize) -> Self {
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
    match format(&a.text, &a.parsed, &a.external, opts) {
        Some(new_text) if new_text != *a.text => vec![TextEdit {
            range: Range::new(Position::new(0, 0), a.position(enc, a.text.len() as u32)),
            new_text,
        }],
        _ => Vec::new(),
    }
}

/// The formatted text, or `None` when the document has syntax errors (or, as a
/// safety net, when the result would not parse to the same triples).
/// `parsed` is the parse of `text` with `external` prefixes; when its tokens
/// were dropped the text is tokenized again.
pub fn format(text: &str, parsed: &Parsed, external: &PrefixMap, opts: &Options) -> Option<String> {
    if parsed.errors.iter().any(|e| e.severity == Severity::Error) {
        return None;
    }
    let local;
    let tokens = if parsed.tokens.is_empty() && !text.trim().is_empty() {
        local = tokenize(text);
        &local
    } else {
        &parsed.tokens
    };
    let doc = Doc::new(text, tokens);
    let items = Cst {
        t: &doc.toks,
        pos: 0,
    }
    .document()?;
    let out = Printer::new(&doc, opts).document(&items);

    let again = parse(&out, external);
    let same = !again.errors.iter().any(|e| e.severity == Severity::Error)
        && again.comments.len() == doc.comments.len()
        && same_triples(&again.triples, &parsed.triples);
    if !same {
        eprintln!(
            "turtle-lsp: formatter bug: the formatted text does not match the original \
             ({} vs {} triples, {} vs {} comments); no edits applied",
            again.triples.len(),
            parsed.triples.len(),
            again.comments.len(),
            doc.comments.len()
        );
        return None;
    }
    Some(out)
}

/// Token order is kept, so the triples (and generated blank node labels) come
/// out in the same order.
fn same_triples(a: &[Triple], b: &[Triple]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.s == y.s && x.p == y.p && x.o == y.o)
}

// ---- tokens and comment attachment ----

type Ix = u32;

struct Comment {
    start: u32,
    end: u32,
    /// A blank line separates it from what comes before.
    blank_before: bool,
}

struct Doc<'a> {
    src: &'a str,
    /// Non-comment tokens.
    toks: Vec<Token>,
    comments: Vec<Comment>,
    /// Own-line comments above token `i` (index `toks.len()` = end of file),
    /// as a range into `comments`.
    lead: Vec<(u32, u32)>,
    /// The comment after token `i` on the same line.
    trail: Vec<Option<u32>>,
    /// A blank line separates token `i` from what comes before.
    blank_before: Vec<bool>,
    /// Prefix counts of tokens with leading / trailing comments.
    lead_pref: Vec<u32>,
    trail_pref: Vec<u32>,
}

fn has_blank(gap: &str) -> bool {
    let mut it = gap.split('\n');
    it.next();
    // Two newlines with only whitespace between them.
    let n = it.count();
    n >= 2
}

impl<'a> Doc<'a> {
    fn new(src: &'a str, all: &[Token]) -> Doc<'a> {
        let n = all.iter().filter(|t| t.kind != Kind::Comment).count();
        let mut d = Doc {
            src,
            toks: Vec::with_capacity(n),
            comments: Vec::new(),
            lead: vec![(0, 0); n + 1],
            trail: vec![None; n],
            blank_before: Vec::with_capacity(n),
            lead_pref: Vec::new(),
            trail_pref: Vec::new(),
        };
        let mut prev_end = 0usize;
        // Comments waiting for the next token.
        let mut lead_from = 0u32;
        for t in all {
            let gap = &src[prev_end..t.start as usize];
            let blank = prev_end > 0 && has_blank(gap);
            if t.kind == Kind::Comment {
                let text = src[t.start as usize..t.end as usize].trim_end();
                let idx = d.comments.len() as u32;
                d.comments.push(Comment {
                    start: t.start,
                    end: t.start + text.len() as u32,
                    blank_before: blank,
                });
                let own_line = d.toks.is_empty() || gap.contains('\n') || prev_end == 0;
                if own_line {
                    // Stays in the pending run for the next token.
                } else {
                    let last = d.toks.len() - 1;
                    if d.trail[last].is_none() && lead_from == idx {
                        d.trail[last] = Some(idx);
                        lead_from = idx + 1;
                    }
                }
            } else {
                let j = d.toks.len();
                d.lead[j] = (lead_from, d.comments.len() as u32);
                lead_from = d.comments.len() as u32;
                d.blank_before.push(blank);
                d.toks.push(*t);
            }
            prev_end = t.end as usize;
        }
        d.lead[n] = (lead_from, d.comments.len() as u32);
        let mut acc = 0;
        d.lead_pref.push(0);
        for r in &d.lead {
            acc += u32::from(r.0 < r.1);
            d.lead_pref.push(acc);
        }
        acc = 0;
        d.trail_pref.push(0);
        for t in &d.trail {
            acc += u32::from(t.is_some());
            d.trail_pref.push(acc);
        }
        d
    }

    fn text(&self, i: Ix) -> &'a str {
        self.toks[i as usize].text(self.src)
    }

    fn comment(&self, c: u32) -> &'a str {
        let c = &self.comments[c as usize];
        &self.src[c.start as usize..c.end as usize]
    }

    /// Any comment that would end up inside the tokens `a..=b`.
    fn comments_inside(&self, a: Ix, b: Ix) -> bool {
        let (a, b) = (a as usize, b as usize);
        self.lead_pref[b + 1] > self.lead_pref[a + 1] || self.trail_pref[b] > self.trail_pref[a]
    }

    fn leading(&self, i: Ix) -> (u32, u32) {
        self.lead[i as usize]
    }
}

// ---- concrete syntax tree ----

enum Term {
    /// Tokens printed without spaces: an IRI, a literal with its language tag
    /// or datatype, `[]`, `()`, `a`, `~`.
    Atom(Ix, Ix),
    /// `[ pol ]` or `{| pol |}`.
    List { open: Ix, pol: Vec<Po>, close: Ix },
    /// `( items )`.
    Coll {
        open: Ix,
        items: Vec<Obj>,
        close: Ix,
    },
    /// `<< s p o ~ r >>` or `<<( s p o )>>`: always on one line.
    Quoted {
        open: Ix,
        parts: Vec<Term>,
        close: Ix,
    },
}

struct Obj {
    term: Term,
    /// `~`, a reifier, `{| ... |}` blocks.
    annots: Vec<Term>,
    comma: Option<Ix>,
}

struct Po {
    verb: Term,
    objs: Vec<Obj>,
    semis: Vec<Ix>,
}

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
        dot: Ix,
    },
}

impl Term {
    fn first(&self) -> Ix {
        match self {
            Term::Atom(a, _) => *a,
            Term::List { open, .. } | Term::Coll { open, .. } | Term::Quoted { open, .. } => *open,
        }
    }
    fn last(&self) -> Ix {
        match self {
            Term::Atom(_, b) => *b,
            Term::List { close, .. } | Term::Coll { close, .. } | Term::Quoted { close, .. } => {
                *close
            }
        }
    }
    fn is_group(&self) -> bool {
        matches!(self, Term::List { .. } | Term::Coll { .. })
    }
    /// Prints the same on one line in any context.
    fn simple(&self) -> bool {
        !self.is_group()
    }
    /// A group with nothing nested in it, or a simple term.
    fn flat(&self) -> bool {
        match self {
            Term::List { pol, .. } => pol.iter().all(|po| po.objs.iter().all(Obj::simple)),
            Term::Coll { items, .. } => items.iter().all(Obj::simple),
            _ => true,
        }
    }
}

impl Obj {
    fn simple(&self) -> bool {
        self.term.simple() && self.annots.iter().all(Term::simple)
    }
    fn flat(&self) -> bool {
        self.term.flat() && self.annots.iter().all(Term::flat)
    }
    fn last(&self) -> Ix {
        self.annots.last().unwrap_or(&self.term).last()
    }
}

impl Item {
    fn first(&self) -> Ix {
        match self {
            Item::Directive { kw, .. } => *kw,
            Item::Triples { subject, .. } => subject.first(),
        }
    }
}

struct Cst<'a> {
    t: &'a [Token],
    pos: usize,
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
                _ => self.triples()?,
            });
        }
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

    fn triples(&mut self) -> Option<Item> {
        let (subject, pol) = match self.kind(0)? {
            Kind::LBracket if self.kind(1) != Some(Kind::RBracket) => {
                let s = self.list(Kind::RBracket)?;
                let pol = if self.kind(0) == Some(Kind::Dot) {
                    Vec::new()
                } else {
                    self.pol()?
                };
                (s, pol)
            }
            Kind::LTriple => {
                let s = self.reified()?;
                let pol = if self.kind(0) == Some(Kind::Dot) {
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
        let dot = self.eat(Kind::Dot)?;
        Some(Item::Triples { subject, pol, dot })
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
                return Some(out);
            }
        }
    }

    fn objects(&mut self) -> Option<Vec<Obj>> {
        let mut out = Vec::new();
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

/// Width of something that cannot go on one line.
const BIG: usize = 1 << 24;

struct Printer<'a> {
    d: &'a Doc<'a>,
    out: String,
    nl: &'static str,
    unit: String,
    unit_w: usize,
    width: usize,
    col: usize,
    level: usize,
    line_start: usize,
    has_content: bool,
    /// The trailing comment to write at the end of the current line.
    pending: Vec<u32>,
    /// A trailing comment was queued: the next space becomes a line break.
    must_break: bool,
    /// The first token of the current item, whose comments are already out.
    skip_lead: Option<Ix>,
}

impl<'a> Printer<'a> {
    fn new(d: &'a Doc<'a>, o: &Options) -> Printer<'a> {
        let nl = match d.src.find('\n') {
            Some(i) if i > 0 && d.src.as_bytes()[i - 1] == b'\r' => "\r\n",
            _ => "\n",
        };
        let (unit, unit_w) = if o.insert_spaces {
            (" ".repeat(o.tab_size as usize), o.tab_size as usize)
        } else {
            ("\t".to_string(), o.tab_size as usize)
        };
        Printer {
            d,
            out: String::with_capacity(d.src.len() + d.src.len() / 8),
            nl,
            unit,
            unit_w,
            width: o.line_width,
            col: 0,
            level: 0,
            line_start: 0,
            has_content: false,
            pending: Vec::new(),
            must_break: false,
            skip_lead: None,
        }
    }

    // ---- low level ----

    fn write(&mut self, s: &str) {
        self.out.push_str(s);
        match s.rfind('\n') {
            Some(i) => self.col = s[i + 1..].chars().count(),
            None => self.col += s.chars().count(),
        }
        self.has_content = true;
    }

    fn indent(&mut self, level: usize) {
        for _ in 0..level {
            self.out.push_str(&self.unit);
        }
        self.level = level;
        self.col = level * self.unit_w;
    }

    fn flush(&mut self) {
        // At most one: `one_pending` breaks the line before a second.
        if let Some(c) = self.pending.pop() {
            if self.has_content {
                self.out.push(' ');
            }
            self.write(self.d.comment(c));
        }
    }

    /// Ends the current line (unless it is still empty) and indents the next.
    fn line(&mut self, level: usize) {
        self.flush();
        if self.has_content {
            self.out.push_str(self.nl);
        } else {
            self.out.truncate(self.line_start);
        }
        self.line_start = self.out.len();
        self.has_content = false;
        self.must_break = false;
        self.indent(level);
    }

    /// Starts a top-level item at column 0, after a blank line if asked.
    fn top_line(&mut self, blank: bool) {
        self.line(0);
        if blank && !self.out.is_empty() {
            self.out.push_str(self.nl);
            self.line_start = self.out.len();
        }
    }

    fn space(&mut self, level: usize) {
        if self.must_break {
            self.line(level);
        } else {
            self.out.push(' ');
            self.col += 1;
        }
    }

    /// Own-line comments above token `i`, each on its own line at `level`.
    fn leading(&mut self, i: Ix, level: usize) {
        let (a, b) = self.d.leading(i);
        if a == b || self.skip_lead == Some(i) {
            return;
        }
        self.line(level);
        for c in a..b {
            self.write(self.d.comment(c));
            self.line(level);
        }
    }

    fn after(&mut self, i: Ix) {
        if let Some(c) = self.d.trail[i as usize] {
            self.pending.push(c);
            self.must_break = true;
        }
    }

    /// A token that starts on a new line at `level` if it cannot stay on this one.
    fn tok(&mut self, i: Ix, level: usize) {
        self.leading(i, level);
        if self.must_break {
            self.line(level);
        }
        self.write(self.d.text(i));
        self.after(i);
    }

    /// ` ;`, ` ,` or ` .` glued to what precedes it.
    fn punct(&mut self, i: Ix, level: usize) {
        self.leading(i, level);
        self.one_pending(i, level);
        if self.has_content {
            self.out.push(' ');
            self.col += 1;
        }
        self.write(self.d.text(i));
        self.after(i);
    }

    /// A redundant `;`: left out, unless a comment hangs on it (moving that
    /// comment elsewhere would not survive a second pass).
    fn extra_semi(&mut self, i: Ix, level: usize) {
        let (a, b) = self.d.leading(i);
        if a < b || self.d.trail[i as usize].is_some() {
            self.punct(i, level);
        }
    }

    /// Two trailing comments cannot share a line: if `i` brings a second one,
    /// it goes on a new line.
    fn one_pending(&mut self, i: Ix, level: usize) {
        if !self.pending.is_empty() && self.d.trail[i as usize].is_some() {
            self.line(level);
        }
    }

    // ---- widths ----

    fn tok_w(&self, i: Ix) -> usize {
        let s = self.d.text(i);
        if s.contains('\n') {
            BIG
        } else {
            s.chars().count()
        }
    }

    fn term_w(&self, t: &Term) -> usize {
        match t {
            Term::Atom(a, b) => (*a..=*b).fold(0, |acc, i| acc.saturating_add(self.tok_w(i))),
            Term::List { open, pol, close } => self
                .tok_w(*open)
                .saturating_add(self.pol_w(pol))
                .saturating_add(self.tok_w(*close) + 2),
            Term::Coll { open, items, close } => {
                let inner = items
                    .iter()
                    .fold(0usize, |acc, o| acc.saturating_add(self.obj_w(o) + 1));
                self.tok_w(*open)
                    .saturating_add(inner)
                    .saturating_add(self.tok_w(*close) + 1)
            }
            Term::Quoted { open, parts, close } => {
                let inner = parts
                    .iter()
                    .fold(0usize, |acc, p| acc.saturating_add(self.term_w(p) + 1));
                self.tok_w(*open)
                    .saturating_add(inner)
                    .saturating_add(self.tok_w(*close) + 1)
            }
        }
    }

    fn obj_w(&self, o: &Obj) -> usize {
        o.annots.iter().fold(self.term_w(&o.term), |acc, a| {
            acc.saturating_add(self.term_w(a) + 1)
        })
    }

    /// Objects separated by ` , `.
    fn objs_w(&self, objs: &[Obj]) -> usize {
        let sum = objs
            .iter()
            .fold(0usize, |acc, o| acc.saturating_add(self.obj_w(o)));
        sum.saturating_add(3 * objs.len().saturating_sub(1))
    }

    /// Pairs separated by ` ; `.
    fn pol_w(&self, pol: &[Po]) -> usize {
        let sum = pol.iter().fold(0usize, |acc, po| {
            acc.saturating_add(self.term_w(&po.verb) + 1)
                .saturating_add(self.objs_w(&po.objs))
        });
        sum.saturating_add(3 * pol.len().saturating_sub(1))
    }

    fn fits(&self, w: usize) -> bool {
        self.col.saturating_add(w).saturating_add(2) <= self.width
    }

    // ---- layout ----

    fn term(&mut self, t: &Term, level: usize, flat: bool) {
        match t {
            Term::Atom(a, b) => {
                self.tok(*a, level);
                for i in a + 1..=*b {
                    self.tok(i, level + 1);
                }
            }
            Term::Quoted { open, parts, close } => {
                self.tok(*open, level);
                for p in parts {
                    self.space(level + 1);
                    self.term(p, level + 1, true);
                }
                self.space(level + 1);
                self.tok(*close, level);
            }
            Term::List { open, pol, close } => {
                let inline = flat
                    || (!self.d.comments_inside(*open, *close)
                        && t.flat()
                        && self.fits(self.term_w(t)));
                self.tok(*open, level);
                if inline {
                    self.space(level + 1);
                    self.pol(pol, level + 1, true);
                    self.space(level);
                } else {
                    self.pol(pol, level + 1, false);
                    self.line(level);
                }
                self.tok(*close, level);
            }
            Term::Coll { open, items, close } => {
                let inline = flat
                    || (!self.d.comments_inside(*open, *close)
                        && t.flat()
                        && self.fits(self.term_w(t)));
                self.tok(*open, level);
                for o in items {
                    if inline {
                        self.space(level + 1);
                    } else {
                        self.line(level + 1);
                    }
                    self.obj(o, level + 1, inline);
                }
                if inline {
                    self.space(level);
                } else {
                    self.line(level);
                }
                self.tok(*close, level);
            }
        }
    }

    fn obj(&mut self, o: &Obj, level: usize, flat: bool) {
        self.term(&o.term, level, flat);
        for a in &o.annots {
            self.space(level + 1);
            self.term(a, level, flat);
        }
    }

    /// Predicate-object pairs. Broken: every pair on its own line at `level`;
    /// inline: separated by ` ; `.
    fn pol(&mut self, pol: &[Po], level: usize, flat: bool) {
        self.pol_from(pol, level, flat, false);
    }

    /// As [`Self::pol`]; `first_inline` keeps the first pair on the current line.
    fn pol_from(&mut self, pol: &[Po], level: usize, flat: bool, first_inline: bool) {
        for (k, po) in pol.iter().enumerate() {
            if flat {
                if k > 0 {
                    self.space(level);
                }
            } else if k > 0 || !first_inline {
                self.line(level);
            }
            self.term(&po.verb, level, flat);
            self.space(level + 1);
            self.objects(&po.objs, level, flat);
            let last = k + 1 == pol.len();
            for (m, s) in po.semis.iter().enumerate() {
                if m == 0 && !last {
                    self.punct(*s, level);
                } else {
                    self.extra_semi(*s, level);
                }
            }
        }
    }

    fn objects(&mut self, objs: &[Obj], level: usize, flat: bool) {
        // A single object lays itself out; several go on one line only when
        // each of them fits there flat.
        let inline = flat
            || (objs.len() > 1
                && !self
                    .d
                    .comments_inside(objs[0].term.first(), objs[objs.len() - 1].last())
                && objs.iter().all(Obj::flat)
                && self.fits(self.objs_w(objs)));
        // Groups in a broken list follow each other as `] , [`; anything else
        // goes on its own line one level in.
        let mut at = level;
        for (k, o) in objs.iter().enumerate() {
            if k > 0 {
                let prev = &objs[k - 1];
                let chained = prev.term.is_group() && prev.annots.is_empty() && o.term.is_group();
                if inline || chained {
                    self.space(at + 1);
                } else {
                    at = level + 1;
                    self.line(at);
                }
            }
            self.obj(o, at, inline);
            if let Some(c) = o.comma {
                self.punct(c, at);
            }
        }
    }

    fn document(mut self, items: &[Item]) -> String {
        let order = hoisted_order(self.d, items);
        let align = items
            .iter()
            .filter_map(|it| match it {
                Item::Directive {
                    kw, name: Some(n), ..
                } => Some(self.tok_w(*kw) + 1 + self.tok_w(*n) + 1),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let mut prev: Option<&Item> = None;
        for &k in &order {
            let it = &items[k];
            let first = it.first();
            if let Some(p) = prev {
                let both_directives =
                    matches!(p, Item::Directive { .. }) && matches!(it, Item::Directive { .. });
                let (a, b) = self.d.leading(first);
                let author_blank = if a < b {
                    self.d.comments[a as usize].blank_before
                } else {
                    self.d.blank_before[first as usize]
                };
                self.top_line(!both_directives || author_blank);
            }
            self.item_leading(first);
            self.skip_lead = Some(first);
            match it {
                Item::Directive {
                    kw,
                    name,
                    value,
                    dot,
                } => {
                    self.tok(*kw, 0);
                    self.space(1);
                    if let Some(n) = name {
                        self.tok(*n, 1);
                        if self.must_break {
                            self.line(1);
                        } else {
                            let pad = align.saturating_sub(self.col).max(1);
                            for _ in 0..pad {
                                self.out.push(' ');
                            }
                            self.col += pad;
                        }
                    }
                    self.tok(*value, 1);
                    if let Some(d) = dot {
                        self.punct(*d, 0);
                    }
                }
                Item::Triples { subject, pol, dot } => {
                    self.term(subject, 0, false);
                    if !pol.is_empty() {
                        self.space(1);
                        self.pol_from(pol, 1, false, true);
                    }
                    self.punct(*dot, 0);
                }
            }
            prev = Some(it);
        }
        // Comments after the last token.
        let n = self.d.toks.len() as Ix;
        let (a, b) = self.d.leading(n);
        for c in a..b {
            if c > a || prev.is_some() {
                self.top_line(self.d.comments[c as usize].blank_before);
            }
            self.write(self.d.comment(c));
        }
        self.line(0);
        self.out.truncate(self.line_start);
        // `line` left the document ending in exactly one newline (or empty).
        self.out
    }

    /// Own-line comments above a top-level item, keeping single blank lines
    /// between them and before the item.
    fn item_leading(&mut self, first: Ix) {
        let (a, b) = self.d.leading(first);
        for c in a..b {
            if c > a {
                self.top_line(self.d.comments[c as usize].blank_before);
            }
            self.write(self.d.comment(c));
        }
        if a < b {
            self.top_line(self.d.blank_before[first as usize]);
        }
    }
}

/// Item order: every directive in the leading block, then directives that come
/// later moved up behind it as long as that cannot change what any IRI means
/// (no `BASE` moves, and no prefix moves above a use of its name), then the
/// rest in original order.
fn hoisted_order(d: &Doc, items: &[Item]) -> Vec<usize> {
    let first_stmt = items
        .iter()
        .position(|it| matches!(it, Item::Triples { .. }))
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
