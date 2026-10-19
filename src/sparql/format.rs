//! Formatting of SPARQL queries and updates. Works from the token stream, as
//! the Turtle formatter does: the tokens are laid out again and their text is
//! copied unchanged (keywords upper-cased, built-in function names in their
//! grammar spelling, `a` left as `a`), so no token is added or removed and
//! every comment stays. A document with errors (syntax, or an undeclared
//! prefix) is left alone, and so is one with text the lexer does not know
//! (`spargebra` reads `GROUPBY` as `GROUP BY`; the lexer cannot).
//!
//! Layout:
//! - the prologue first, one declaration per line, `PREFIX` IRIs aligned
//!   within each run of declarations, and one blank line after it;
//! - indentation from the client's options, one level per `{ }`;
//! - the query form and its projection on the first line (`SELECT DISTINCT
//!   ?a (COUNT(?b) AS ?n)`), each dataset clause, `WHERE {` and solution
//!   modifier (`GROUP BY`, `HAVING`, `ORDER BY`, `LIMIT`, `OFFSET`, a
//!   trailing `VALUES`) on its own line; a group written without `WHERE`
//!   stays on the line before it (`SELECT * {`, `ASK { }`);
//! - group openers (`WHERE {`, `OPTIONAL {`, `GRAPH ?g {`, `INSERT DATA {`,
//!   the `{` of a group or subquery) end their line, the contents one level
//!   in, `}` on its own line at the opener's level; `} UNION {` joined; an
//!   empty group is `{ }`;
//! - one pattern per line: a triple's subject and first predicate on one
//!   line, every further predicate on its own line one level in after ` ;`;
//!   an object list on one line joined by ` , ` when it fits the line width,
//!   else one object per line one level in; the optional ` .` after a
//!   triple kept or left out as written (also after `}`);
//! - `[ ]`, `( )` and `{| |}` in a triple on one line when short and flat
//!   (nothing nested), else broken with their contents one level in and the
//!   closing bracket at the level of the line that opened it; `<< >>` and
//!   `<<( )>>` on one line; property paths without spaces;
//! - expressions with binary operators spaced, unary `!` / `-` / `+` and
//!   function calls without (`FILTER (STRLEN(?n) > 0 && !BOUND(?x))`),
//!   `FILTER (`, `BIND (`, `HAVING (` with a space, `DESC(?x)` without; a
//!   sign written against a number stays against it (`-1`, `?x +1`), one
//!   written apart stays apart, since `?x +1` and `?x + 1` differ;
//! - `VALUES ?v { ... }` on one line when it fits, else one row per line;
//! - the projection and `GROUP BY` / `ORDER BY` / `HAVING` / `DESCRIBE`
//!   lists fill the line width, continuing one level in;
//! - update operations separated by ` ;` and a blank line; `LOAD`, `CLEAR`,
//!   `DROP`, `CREATE`, `ADD`, `MOVE`, `COPY` on one line; `WITH`, `DELETE {`,
//!   `INSERT {`, `USING` and `WHERE {` of a modify operation each on a line;
//! - an own-line comment stays above the token after it, a trailing comment
//!   stays at the end of the line its token lands on (after one space);
//!   blank lines before a pattern, a clause or a comment are kept (a run
//!   collapses to one), none after `{` or before `}`;
//! - a multi-line string counts with its first line for the line width.
//!
//! Safety check: the output is parsed again (same prefixes and base); it
//! must have no error diagnostics, the same tokens and the same comments
//! (each in order), parse with `spargebra` to the same algebra as the input
//! (compared as text with generated names renumbered: blank node labels and
//! the variables `spargebra` invents for aggregates, a random `u128` in hex),
//! and lay out to itself. Otherwise nothing is formatted. `spargebra` is
//! never given text the document checks refuse (nesting past 256 levels,
//! more than 4096 `OPTIONAL` and `MINUS`): such a document has errors, and
//! the output has the same tokens as the input. Printing the algebra
//! recurses once per element of a long group, so the comparison runs on a
//! thread whose stack grows with the document (256 bytes per token, 8 MiB to
//! 1 GiB of address space), joined before returning.
//!
//! Linear in the number of tokens, apart from the `spargebra` checks.

use super::{Document, Token, TokenKind, lexer};
use crate::features::format::Options;
use crate::syntax::PrefixMap;
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::SparqlParser;

/// The formatted text, or why it was not formatted.
pub(super) fn format(
    text: &str,
    prefixes: &PrefixMap,
    base: Option<&str>,
    opts: &Options,
) -> Result<String, String> {
    let doc = Document::parse(text, prefixes, base);
    if doc.has_errors() {
        return Err("the document has errors".into());
    }
    // Words `spargebra` reads as two keywords (`GROUPBY`) and other text
    // the lexer does not know: their layout is unknown.
    if doc.tokens().iter().any(|t| t.kind == TokenKind::Error) {
        return Err("the document has text that is no SPARQL token".into());
    }
    let out = layout(&doc, opts).ok_or("the document could not be laid out")?;
    if out == text {
        return Ok(out);
    }
    let again = Document::parse(&out, prefixes, base);
    if again.has_errors() || again.is_update() != doc.is_update() {
        return Err("formatter bug: the formatted text does not parse".into());
    }
    if !same_tokens(&doc, &again) {
        return Err("formatter bug: the formatted text has other tokens".into());
    }
    // Printing the algebra recurses once per element of a group (a join
    // chain), which the nesting limit does not bound: on a thread whose
    // stack grows with the document.
    let stack = doc
        .tokens()
        .len()
        .saturating_mul(256)
        .clamp(8 << 20, 1 << 30);
    let same = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("sparql-format-check".into())
            .stack_size(stack)
            .spawn_scoped(scope, || {
                Ok::<_, String>(algebra(&doc, prefixes, base)? == algebra(&again, prefixes, base)?)
            })
            .map_err(|e| format!("cannot check the formatted text: {e}"))?
            .join()
            .map_err(|_| "cannot check the formatted text: spargebra panicked".to_string())?
    })?;
    if !same {
        return Err("formatter bug: the formatted text means something else".into());
    }
    if layout(&again, opts).as_deref() != Some(out.as_str()) {
        return Err("formatter bug: formatting the formatted text changes it".into());
    }
    Ok(out)
}

/// A token's text as the formatter writes it.
fn shown<'s>(t: &Token, src: &'s str) -> &'s str {
    let s = t.text(src);
    match t.kind {
        TokenKind::Keyword => lexer::keyword(s).unwrap_or(s),
        TokenKind::Function => lexer::function(s).unwrap_or(s),
        TokenKind::Comment => s.trim_end(),
        _ => s,
    }
}

/// The same tokens in the same order, and the same comments in the same
/// order (a trailing comment may move past a ` .`, ` ;` or ` ,`).
fn same_tokens(a: &Document, b: &Document) -> bool {
    fn part(d: &Document, comments: bool) -> impl Iterator<Item = (TokenKind, &str)> {
        d.tokens()
            .iter()
            .filter(move |t| (t.kind == TokenKind::Comment) == comments)
            .map(move |t| (t.kind, shown(t, d.text())))
    }
    part(a, false).eq(part(b, false)) && part(a, true).eq(part(b, true))
}

/// The namespace a prefix `spargebra` refuses is bound to, as the document
/// checks do.
const PLACEHOLDER: &str = "urn:x-turtle-lsp:undeclared:0:";

/// The document's algebra as `spargebra` prints it, generated names
/// renumbered. Prologues after the first update operation are left out of
/// the text and given to the parser instead, as the document checks do
/// (`spargebra` takes a prologue only at the start).
fn algebra(doc: &Document, prefixes: &PrefixMap, base: Option<&str>) -> Result<String, String> {
    let st = doc.structure();
    let mut parser = SparqlParser::new();
    if let Some(b) = base
        && let Ok(p) = parser.clone().with_base_iri(b)
    {
        parser = p;
    }
    for (name, ns) in prefixes {
        parser = match parser.clone().with_prefix(name, ns) {
            Ok(p) => p,
            Err(_) => parser
                .with_prefix(name, PLACEHOLDER)
                .map_err(|e| e.to_string())?,
        };
    }
    let mut text = doc.text().to_owned();
    if st.is_update
        && let Some(first) = st.operations.first()
    {
        let later = |s: &crate::syntax::Span| s.start > first.keyword.start;
        let spans = (st.prefixes.iter().map(|p| p.span).filter(later))
            .chain(st.bases.iter().map(|b| b.span).filter(later))
            .chain(st.versions.iter().map(|v| v.span).filter(later));
        let mut bytes = text.into_bytes();
        for s in spans {
            for b in &mut bytes[s.start as usize..s.end as usize] {
                if !matches!(*b, b'\n' | b'\r') {
                    *b = b' ';
                }
            }
        }
        // Whole characters became spaces: still UTF-8.
        text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        for p in st.prefixes.iter().filter(|p| later(&p.span)) {
            let ns = p.resolved.as_deref().unwrap_or(PLACEHOLDER);
            if let Ok(next) = parser.clone().with_prefix(&p.name, ns) {
                parser = next;
            }
        }
    }
    let printed = if st.is_update {
        parser.parse_update(&text).map(|u| u.to_string())
    } else {
        parser.parse_query(&text).map(|q| q.to_string())
    };
    let own: FxHashSet<&str> = st.vars.iter().map(|v| v.name.as_str()).collect();
    printed
        .map(|s| canonical(&s, &own))
        .map_err(|e| format!("spargebra rejects it: {e}"))
}

/// `algebra` with generated names renumbered in order of first appearance:
/// blank node labels (fresh on every parse) and the variables `spargebra`
/// makes up for aggregates: a random `u128` in hex, so up to 32 digits
/// (fewer when it starts with zeros), and never one of the document's own
/// variables (`own`).
fn canonical(algebra: &str, own: &FxHashSet<&str>) -> String {
    let mut ids: FxHashMap<&str, usize> = FxHashMap::default();
    let mut out = String::with_capacity(algebra.len());
    let b = algebra.as_bytes();
    let name_end = |from: usize, extra: &[u8]| {
        let mut j = from;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'_' || extra.contains(&b[j]))
        {
            j += 1;
        }
        j
    };
    let (mut i, mut copied) = (0, 0);
    while i < b.len() {
        let end = match b[i] {
            b'_' if b.get(i + 1) == Some(&b':') => name_end(i + 2, b"-"),
            b'?' => {
                let end = name_end(i + 1, b"");
                let name = &algebra[i + 1..end];
                let generated = (1..=32).contains(&name.len())
                    && name.bytes().all(|c| c.is_ascii_hexdigit())
                    && !own.contains(name);
                if !generated {
                    i = end.max(i + 1);
                    continue;
                }
                end
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let sigil = if b[i] == b'?' { "?" } else { "_:" };
        out.push_str(&algebra[copied..i]);
        let n = ids.len();
        let id = *ids.entry(&algebra[i..end]).or_insert(n);
        out.push_str(sigil);
        out.push('g');
        out.push_str(&id.to_string());
        i = end;
        copied = end;
    }
    out.push_str(&algebra[copied..]);
    out
}

/// The laid-out text, or `None` when the tokens do not make a construct the
/// layout knows (which a document without errors always does).
fn layout(doc: &Document, opts: &Options) -> Option<String> {
    let d = Doc::new(doc.text(), doc.tokens())?;
    let mut f = Printer::new(&d, opts);
    f.document(doc.is_update())?;
    Some(f.out)
}

// ---- tokens, brackets and comment attachment ----

type Ix = u32;

/// Not a bracket.
const NONE: Ix = Ix::MAX;

/// Width of something that cannot go on one line.
const BIG: usize = 1 << 24;

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
    /// For an opening bracket, the index of its closing one.
    close: Vec<Ix>,
}

fn has_blank(gap: &str) -> bool {
    gap.split('\n').count() >= 3
}

fn closer(open: &str) -> Option<&'static str> {
    Some(match open {
        "{" => "}",
        "(" => ")",
        "[" => "]",
        "<<" => ">>",
        "<<(" => ")>>",
        "{|" => "|}",
        _ => return None,
    })
}

impl<'a> Doc<'a> {
    /// `None` when the brackets do not match.
    fn new(src: &'a str, all: &[Token]) -> Option<Doc<'a>> {
        let n = all.iter().filter(|t| t.kind != TokenKind::Comment).count();
        let mut d = Doc {
            src,
            toks: Vec::with_capacity(n),
            comments: Vec::new(),
            lead: vec![(0, 0); n + 1],
            trail: vec![None; n],
            blank_before: Vec::with_capacity(n),
            lead_pref: Vec::with_capacity(n + 2),
            trail_pref: Vec::with_capacity(n + 1),
            close: vec![NONE; n],
        };
        let mut prev_end = 0usize;
        // Comments waiting for the next token start here.
        let mut lead_from = 0u32;
        let mut open: Vec<Ix> = Vec::new();
        for t in all {
            let gap = &src[prev_end..t.span.start as usize];
            let blank = prev_end > 0 && has_blank(gap);
            if t.kind == TokenKind::Comment {
                let text = t.text(src).trim_end();
                let idx = d.comments.len() as u32;
                d.comments.push(Comment {
                    start: t.span.start,
                    end: t.span.start + text.len() as u32,
                    blank_before: blank,
                });
                let own_line = d.toks.is_empty() || gap.contains(['\n', '\r']);
                if !own_line {
                    let last = d.toks.len() - 1;
                    if d.trail[last].is_none() && lead_from == idx {
                        d.trail[last] = Some(idx);
                        lead_from = idx + 1;
                    }
                }
            } else {
                let j = d.toks.len() as Ix;
                d.lead[j as usize] = (lead_from, d.comments.len() as u32);
                lead_from = d.comments.len() as u32;
                d.blank_before.push(blank);
                d.toks.push(*t);
                if t.kind == TokenKind::Punct {
                    let s = t.text(src);
                    if closer(s).is_some() {
                        open.push(j);
                    } else if matches!(s, "}" | ")" | "]" | ">>" | ")>>" | "|}") {
                        let o = open.pop()?;
                        if closer(d.toks[o as usize].text(src)) != Some(s) {
                            return None;
                        }
                        d.close[o as usize] = j;
                    }
                }
            }
            prev_end = t.span.end as usize;
        }
        if !open.is_empty() {
            return None;
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
        Some(d)
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

// ---- triples ----

enum Term {
    /// Tokens printed without spaces: a term, a literal with its language tag
    /// or datatype, a signed number, `[]`, `()`, a property path, `~`.
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

struct Triples {
    subject: Term,
    pol: Vec<Po>,
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
        !self.term.is_group() && !self.annots.iter().any(Term::is_group)
    }
    fn flat(&self) -> bool {
        self.term.flat() && self.annots.iter().all(Term::flat)
    }
    fn last(&self) -> Ix {
        self.annots.last().unwrap_or(&self.term).last()
    }
}

/// Whether a blank line goes before a line that starts a pattern or clause.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Blank {
    /// Never (the first pattern of a group).
    No,
    /// When the author wrote one.
    Author,
    /// Always (after the prologue, between update operations).
    Forced,
}

// ---- printing ----

struct Printer<'a> {
    d: &'a Doc<'a>,
    /// The next token to lay out.
    pos: usize,
    out: String,
    nl: &'static str,
    unit: String,
    unit_w: usize,
    width: usize,
    col: usize,
    /// Indentation level of the current line.
    level: usize,
    line_start: usize,
    has_content: bool,
    /// The trailing comment to write at the end of the current line.
    pending: Option<u32>,
    /// A trailing comment was queued: the next space becomes a line break.
    must_break: bool,
    /// The first token of the current line, whose comments are already out.
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
            pos: 0,
            out: String::with_capacity(d.src.len() + d.src.len() / 4),
            nl,
            unit,
            unit_w,
            width: o.line_width,
            col: 0,
            level: 0,
            line_start: 0,
            has_content: false,
            pending: None,
            must_break: false,
            skip_lead: None,
        }
    }

    // ---- token queries ----

    fn n(&self) -> usize {
        self.d.toks.len()
    }

    fn kind(&self, i: usize) -> Option<TokenKind> {
        self.d.toks.get(i).map(|t| t.kind)
    }

    fn raw(&self, i: usize) -> &'a str {
        self.d.toks.get(i).map_or("", |t| t.text(self.d.src))
    }

    fn is_p(&self, i: usize, p: &str) -> bool {
        self.kind(i) == Some(TokenKind::Punct) && self.raw(i) == p
    }

    fn is_kw(&self, i: usize, k: &str) -> bool {
        self.kind(i) == Some(TokenKind::Keyword) && self.raw(i).eq_ignore_ascii_case(k)
    }

    /// The upper-case keyword at `i`, if it is one.
    fn keyword(&self, i: usize) -> Option<&'static str> {
        (self.kind(i) == Some(TokenKind::Keyword))
            .then(|| lexer::keyword(self.raw(i)))
            .flatten()
    }

    /// Nothing between tokens `i` and `j` in the source.
    fn glued(&self, i: usize, j: usize) -> bool {
        self.d.toks[i].span.end == self.d.toks[j].span.start
    }

    /// A sign written against the number after it.
    fn signed_number(&self, i: usize) -> bool {
        (self.is_p(i, "-") || self.is_p(i, "+"))
            && self.kind(i + 1) == Some(TokenKind::Number)
            && self.glued(i, i + 1)
    }

    fn close_of(&self, i: usize) -> Option<usize> {
        let c = *self.d.close.get(i)?;
        (c != NONE).then_some(c as usize)
    }

    fn bump(&mut self) -> Ix {
        self.pos += 1;
        (self.pos - 1) as Ix
    }

    fn eat_p(&mut self, p: &str) -> Option<Ix> {
        self.is_p(self.pos, p).then(|| self.bump())
    }

    fn eat_kw(&mut self, k: &str) -> Option<Ix> {
        self.is_kw(self.pos, k).then(|| self.bump())
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
        if let Some(c) = self.pending.take() {
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

    /// Starts a new line at `level`, after a blank line if asked.
    fn top_line(&mut self, level: usize, blank: bool) {
        self.line(0);
        if blank && !self.out.is_empty() {
            self.out.push_str(self.nl);
            self.line_start = self.out.len();
        }
        self.indent(level);
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
            self.pending = Some(c);
            self.must_break = true;
        }
    }

    /// A token, on a new line at `level` if it cannot stay on this one.
    fn tok(&mut self, i: Ix, level: usize) {
        self.leading(i, level);
        if self.must_break {
            self.line(level);
        }
        let t = self.d.toks[i as usize];
        self.write(shown(&t, self.d.src));
        self.after(i);
    }

    /// ` ;`, ` ,` or ` .` glued to what precedes it.
    fn punct(&mut self, i: Ix, level: usize) {
        self.leading(i, level);
        // Two trailing comments cannot share a line.
        if self.pending.is_some() && self.d.trail[i as usize].is_some() {
            self.line(level);
        }
        if self.has_content {
            self.out.push(' ');
            self.col += 1;
        }
        self.write(self.raw(i as usize));
        self.after(i);
    }

    /// Starts the line of a pattern, clause or declaration whose first token
    /// is `i`, with its own-line comments above it.
    fn start(&mut self, level: usize, i: Ix, blank: Blank) {
        let (a, b) = self.d.leading(i);
        let author = if a < b {
            self.d.comments[a as usize].blank_before
        } else {
            self.d.blank_before[i as usize]
        };
        let want = match blank {
            Blank::No => false,
            Blank::Author => author,
            Blank::Forced => true,
        };
        self.top_line(level, want);
        for c in a..b {
            if c > a {
                self.top_line(level, self.d.comments[c as usize].blank_before);
            }
            self.write(self.d.comment(c));
        }
        if a < b {
            self.top_line(level, self.d.blank_before[i as usize]);
        }
        self.skip_lead = Some(i);
    }

    /// The comments before a group's `}` (inside it, at `base + 1`), then
    /// the `}` on its own line at `base`.
    fn close_group(&mut self, close: usize, base: usize, empty: bool) {
        let close = close as Ix;
        let (a, b) = self.d.leading(close);
        for c in a..b {
            let blank = (c > a || !empty) && self.d.comments[c as usize].blank_before;
            self.top_line(base + 1, blank);
            self.write(self.d.comment(c));
        }
        self.skip_lead = Some(close);
        self.line(base);
        self.tok(close, base);
    }

    // ---- widths ----

    /// Width of a token on its line (a multi-line string counts with its
    /// first line).
    fn tok_w(&self, i: Ix) -> usize {
        let s = self.raw(i as usize);
        s.split(['\n', '\r']).next().unwrap_or("").chars().count()
    }

    /// Width of tokens `a..=b` laid out by [`Self::inline`].
    fn inline_w(&self, a: usize, b: usize) -> usize {
        let mut w = 0usize;
        let mut prev: Option<usize> = None;
        let mut prev_unary = false;
        for i in a..=b {
            if let Some(p) = prev {
                if self.is_p(i, "{") && self.is_kw(p, "EXISTS") {
                    return BIG;
                }
                w += usize::from(self.gap(p, i, prev_unary));
            }
            w = w.saturating_add(self.tok_w(i as Ix));
            prev_unary = self.is_unary(i, prev);
            prev = Some(i);
        }
        w
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

    /// Fits on this line with room for a ` .` or ` ;` after it.
    fn fits(&self, w: usize) -> bool {
        self.col.saturating_add(w).saturating_add(2) <= self.width
    }

    // ---- expressions and other one-line runs ----

    /// Ends an operand: a binary operator may follow it.
    fn operand_end(&self, i: usize) -> bool {
        match self.kind(i) {
            Some(
                TokenKind::Var
                | TokenKind::Iri
                | TokenKind::PName
                | TokenKind::String
                | TokenKind::Number
                | TokenKind::Boolean
                | TokenKind::LangTag
                | TokenKind::BlankNode
                | TokenKind::A,
            ) => true,
            Some(TokenKind::Punct) => matches!(self.raw(i), ")" | "]" | ">>" | ")>>" | "}"),
            Some(TokenKind::Keyword) => self.is_kw(i, "UNDEF"),
            _ => false,
        }
    }

    /// `!`, or a sign where no operand comes before it.
    fn is_unary(&self, i: usize, prev: Option<usize>) -> bool {
        self.is_p(i, "!")
            || ((self.is_p(i, "-") || self.is_p(i, "+"))
                && prev.is_none_or(|p| !self.operand_end(p)))
    }

    /// Whether a space goes between `p` and `c` in an expression (or another
    /// run laid out on one line: `VALUES`, a simple update operation).
    fn gap(&self, p: usize, c: usize, p_unary: bool) -> bool {
        let (pk, ck) = (self.kind(p), self.kind(c));
        let (pt, ct) = (self.raw(p), self.raw(c));
        let pp = pk == Some(TokenKind::Punct);
        let cp = ck == Some(TokenKind::Punct);
        if ck == Some(TokenKind::LangTag) || (cp && ct == "^^") || (pp && pt == "^^") {
            return false;
        }
        if pp && pt == "(" {
            return false;
        }
        if cp && matches!(ct, ")" | "," | ";") {
            return false;
        }
        if p_unary {
            // `-1` stays a number and `- 1` a negation: they differ.
            return pt != "!" && ck == Some(TokenKind::Number) && !self.glued(p, c);
        }
        if pp && matches!(pt, "-" | "+") && ck == Some(TokenKind::Number) && self.glued(p, c) {
            // `?x +1` adds the literal `+1`, not `1`.
            return false;
        }
        if cp && ct == "(" {
            let call = matches!(
                pk,
                Some(TokenKind::Function | TokenKind::Iri | TokenKind::PName)
            ) || self.is_kw(p, "ASC")
                || self.is_kw(p, "DESC");
            return !call;
        }
        true
    }

    /// Tokens from the current one to `b` on one line (an `EXISTS` group
    /// laid out as a group); breaks forced by comments continue at
    /// `level + 1`.
    fn inline(&mut self, b: usize, level: usize) -> Option<()> {
        let mut prev: Option<usize> = None;
        let mut prev_unary = false;
        while self.pos <= b {
            let i = self.pos;
            if let Some(p) = prev {
                if self.is_p(i, "{") && self.is_kw(p, "EXISTS") {
                    self.space(level + 1);
                    self.group()?;
                    prev = Some(self.pos - 1);
                    prev_unary = false;
                    continue;
                }
                if self.gap(p, i, prev_unary) {
                    self.space(level + 1);
                }
            }
            prev_unary = self.is_unary(i, prev);
            let t = self.bump();
            self.tok(t, level + 1);
            prev = Some(i);
        }
        (self.pos == b + 1).then_some(())
    }

    /// The last token of the item (a projected variable or expression, a
    /// condition, a value) starting at `i`.
    fn item_end(&self, i: usize) -> Option<usize> {
        match self.kind(i)? {
            TokenKind::Var | TokenKind::Number | TokenKind::Boolean | TokenKind::BlankNode => {
                Some(i)
            }
            TokenKind::Iri | TokenKind::PName | TokenKind::Function => {
                if self.is_p(i + 1, "(") {
                    self.close_of(i + 1)
                } else {
                    Some(i)
                }
            }
            TokenKind::String => Some(if self.kind(i + 1) == Some(TokenKind::LangTag) {
                i + 1
            } else if self.is_p(i + 1, "^^") {
                i + 2
            } else {
                i
            }),
            TokenKind::Keyword => match self.keyword(i)? {
                "ASC" | "DESC" if self.is_p(i + 1, "(") => self.close_of(i + 1),
                "EXISTS" if self.is_p(i + 1, "{") => self.close_of(i + 1),
                "NOT" if self.is_kw(i + 1, "EXISTS") && self.is_p(i + 2, "{") => {
                    self.close_of(i + 2)
                }
                "UNDEF" => Some(i),
                _ => None,
            },
            TokenKind::Punct => match self.raw(i) {
                "(" | "<<(" => self.close_of(i),
                "*" => Some(i),
                "-" | "+" if self.signed_number(i) => Some(i + 1),
                _ => None,
            },
            _ => None,
        }
    }

    /// Items filling the line after a keyword, continuing at `level + 1`.
    fn fill(&mut self, level: usize, starts: fn(&Self, usize) -> bool) -> Option<()> {
        let mut first = true;
        while self.pos < self.n() && starts(self, self.pos) {
            let a = self.pos;
            let b = self.item_end(a)?;
            let w = self.inline_w(a, b);
            if !first && self.col.saturating_add(1 + w) > self.width {
                self.line(level + 1);
            } else {
                self.space(level + 1);
            }
            self.inline(b, level)?;
            first = false;
        }
        Some(())
    }

    fn projection_item(&self, i: usize) -> bool {
        self.kind(i) == Some(TokenKind::Var) || self.is_p(i, "(")
    }

    fn describe_item(&self, i: usize) -> bool {
        matches!(
            self.kind(i),
            Some(TokenKind::Var | TokenKind::Iri | TokenKind::PName)
        )
    }

    fn condition_item(&self, i: usize) -> bool {
        matches!(
            self.kind(i),
            Some(TokenKind::Var | TokenKind::Iri | TokenKind::PName | TokenKind::Function)
        ) || self.is_p(i, "(")
            || self.is_kw(i, "EXISTS")
            || self.is_kw(i, "NOT")
    }

    fn order_item(&self, i: usize) -> bool {
        self.condition_item(i) || self.is_kw(i, "ASC") || self.is_kw(i, "DESC")
    }

    /// The last token of the constraint of a `FILTER` at `i`.
    fn constraint_end(&self, i: usize) -> Option<usize> {
        match self.kind(i)? {
            TokenKind::Punct if self.is_p(i, "(") => self.close_of(i),
            TokenKind::Function | TokenKind::Iri | TokenKind::PName if self.is_p(i + 1, "(") => {
                self.close_of(i + 1)
            }
            TokenKind::Keyword => self.item_end(i),
            _ => None,
        }
    }

    // ---- documents and queries ----

    fn document(&mut self, update: bool) -> Option<()> {
        if update {
            let mut blank = Blank::Author;
            loop {
                if self.prologue(blank) {
                    blank = Blank::Forced;
                }
                if self.pos >= self.n() {
                    break;
                }
                self.start(0, self.pos as Ix, blank);
                self.operation()?;
                match self.eat_p(";") {
                    Some(s) => {
                        self.punct(s, 0);
                        blank = Blank::Forced;
                    }
                    None => break,
                }
            }
        } else {
            let blank = if self.prologue(Blank::Author) {
                Blank::Forced
            } else {
                Blank::Author
            };
            if self.pos < self.n() {
                self.start(0, self.pos as Ix, blank);
                self.query(0)?;
            }
        }
        if self.pos != self.n() {
            return None;
        }
        // Comments after the last token.
        let n = self.n() as Ix;
        let (a, b) = self.d.leading(n);
        for c in a..b {
            if c > a || n > 0 {
                self.top_line(0, self.d.comments[c as usize].blank_before);
            }
            self.write(self.d.comment(c));
        }
        self.line(0);
        self.out.truncate(self.line_start);
        Some(())
    }

    /// The `PREFIX`, `BASE` and `VERSION` declarations at the current token,
    /// one per line, `PREFIX` IRIs aligned. Whether there were any.
    fn prologue(&mut self, blank: Blank) -> bool {
        let mut j = self.pos;
        let mut align = 0;
        loop {
            if self.is_kw(j, "PREFIX")
                && self.kind(j + 1) == Some(TokenKind::PName)
                && self.kind(j + 2) == Some(TokenKind::Iri)
            {
                align = align.max(self.tok_w(j as Ix) + 1 + self.tok_w(j as Ix + 1) + 1);
                j += 3;
            } else if (self.is_kw(j, "BASE") && self.kind(j + 1) == Some(TokenKind::Iri))
                || (self.is_kw(j, "VERSION") && self.kind(j + 1) == Some(TokenKind::String))
            {
                j += 2;
            } else {
                break;
            }
        }
        let any = j > self.pos;
        let mut blank = blank;
        while self.pos < j {
            let kw = self.bump();
            self.start(0, kw, blank);
            blank = Blank::Author;
            self.tok(kw, 0);
            self.space(1);
            if self.is_kw(kw as usize, "PREFIX") {
                let name = self.bump();
                self.tok(name, 1);
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
            let value = self.bump();
            self.tok(value, 1);
        }
        any
    }

    fn query(&mut self, level: usize) -> Option<()> {
        let kw = self.keyword(self.pos)?;
        if kw == "SELECT" {
            return self.select(level);
        }
        let t = self.bump();
        self.tok(t, level);
        match kw {
            "CONSTRUCT" => {
                if self.is_p(self.pos, "{") {
                    self.space(level + 1);
                    self.group()?;
                }
            }
            "DESCRIBE" => {
                if let Some(star) = self.eat_p("*") {
                    self.space(level + 1);
                    self.tok(star, level + 1);
                } else {
                    self.fill(level, Self::describe_item)?;
                }
            }
            "ASK" => {}
            _ => return None,
        }
        self.dataset(level);
        self.where_clause(level)?;
        self.modifiers(level)
    }

    /// A `SELECT` query (the document's, or a subquery) at `level`.
    fn select(&mut self, level: usize) -> Option<()> {
        let s = self.bump();
        self.tok(s, level);
        if let Some(m) = self.eat_kw("DISTINCT").or_else(|| self.eat_kw("REDUCED")) {
            self.space(level + 1);
            self.tok(m, level + 1);
        }
        if let Some(star) = self.eat_p("*") {
            self.space(level + 1);
            self.tok(star, level + 1);
        } else {
            self.fill(level, Self::projection_item)?;
        }
        self.dataset(level);
        self.where_clause(level)?;
        self.modifiers(level)
    }

    fn dataset(&mut self, level: usize) {
        while self.is_kw(self.pos, "FROM") {
            let from = self.bump();
            self.start(level, from, Blank::Author);
            self.tok(from, level);
            if let Some(named) = self.eat_kw("NAMED") {
                self.space(level + 1);
                self.tok(named, level + 1);
            }
            if self.pos < self.n() {
                self.space(level + 1);
                let iri = self.bump();
                self.tok(iri, level + 1);
            }
        }
    }

    /// `WHERE {` on its own line, or a group written without `WHERE` on the
    /// current line.
    fn where_clause(&mut self, level: usize) -> Option<()> {
        if let Some(w) = self.eat_kw("WHERE") {
            self.start(level, w, Blank::Author);
            self.tok(w, level);
            self.space(level + 1);
            self.group()
        } else if self.is_p(self.pos, "{") {
            self.space(level + 1);
            self.group()
        } else {
            Some(())
        }
    }

    fn modifiers(&mut self, level: usize) -> Option<()> {
        loop {
            let Some(kw) = self.keyword(self.pos) else {
                return Some(());
            };
            if !matches!(
                kw,
                "GROUP" | "HAVING" | "ORDER" | "LIMIT" | "OFFSET" | "VALUES"
            ) {
                return Some(());
            }
            let t = self.pos as Ix;
            self.start(level, t, Blank::Author);
            match kw {
                "GROUP" | "ORDER" => {
                    self.bump();
                    self.tok(t, level);
                    let by = self.eat_kw("BY")?;
                    self.space(level + 1);
                    self.tok(by, level + 1);
                    let starts = if kw == "GROUP" {
                        Self::condition_item
                    } else {
                        Self::order_item
                    };
                    self.fill(level, starts)?;
                }
                "HAVING" => {
                    self.bump();
                    self.tok(t, level);
                    self.fill(level, Self::condition_item)?;
                }
                "LIMIT" | "OFFSET" => {
                    self.bump();
                    self.tok(t, level);
                    if self.kind(self.pos) != Some(TokenKind::Number) {
                        return None;
                    }
                    self.space(level + 1);
                    let v = self.bump();
                    self.tok(v, level + 1);
                }
                _ => self.values(level)?,
            }
        }
    }

    // ---- groups and patterns ----

    /// The group at the current `{`, whose line is already started: its
    /// contents one level in, `}` at the level of the line it opens on.
    fn group(&mut self) -> Option<()> {
        let open = self.pos;
        if !self.is_p(open, "{") {
            return None;
        }
        let close = self.close_of(open)?;
        if close == open + 1 && !self.d.comments_inside(open as Ix, close as Ix) {
            let level = self.level;
            self.tok(open as Ix, level);
            self.space(level);
            self.tok(close as Ix, level);
            self.pos = close + 1;
            return Some(());
        }
        self.tok(open as Ix, self.level);
        let base = self.level;
        self.pos += 1;
        let empty = self.pos == close;
        if self.is_kw(self.pos, "SELECT") {
            self.start(base + 1, self.pos as Ix, Blank::No);
            self.select(base + 1)?;
        } else {
            self.body(base + 1, close)?;
        }
        if self.pos != close {
            return None;
        }
        self.close_group(close, base, empty);
        self.pos = close + 1;
        Some(())
    }

    /// The patterns of a group up to its `}` at `close`, each on its own line
    /// at `level`.
    fn body(&mut self, level: usize, close: usize) -> Option<()> {
        let mut first = true;
        while self.pos < close {
            // The optional `.` after a pattern that is no triple.
            if let Some(dot) = self.eat_p(".") {
                self.punct(dot, level);
                continue;
            }
            let blank = if first { Blank::No } else { Blank::Author };
            self.start(level, self.pos as Ix, blank);
            first = false;
            self.pattern(level)?;
        }
        Some(())
    }

    fn pattern(&mut self, level: usize) -> Option<()> {
        if self.is_p(self.pos, "{") {
            self.group()?;
            while let Some(u) = self.eat_kw("UNION") {
                self.space(level);
                self.tok(u, level);
                self.space(level);
                self.group()?;
            }
            return Some(());
        }
        let Some(kw) = self.keyword(self.pos) else {
            return self.triples(level);
        };
        match kw {
            "OPTIONAL" | "MINUS" | "LATERAL" => {
                let t = self.bump();
                self.tok(t, level);
                self.space(level + 1);
                self.group()
            }
            "GRAPH" | "SERVICE" => {
                let t = self.bump();
                self.tok(t, level);
                if let Some(s) = self.eat_kw("SILENT") {
                    self.space(level + 1);
                    self.tok(s, level + 1);
                }
                if !matches!(
                    self.kind(self.pos),
                    Some(TokenKind::Var | TokenKind::Iri | TokenKind::PName)
                ) {
                    return None;
                }
                self.space(level + 1);
                let name = self.bump();
                self.tok(name, level + 1);
                self.space(level + 1);
                self.group()
            }
            "FILTER" => {
                let t = self.bump();
                self.tok(t, level);
                let end = self.constraint_end(self.pos)?;
                self.space(level + 1);
                self.inline(end, level)
            }
            "BIND" => {
                let t = self.bump();
                self.tok(t, level);
                let end = self
                    .close_of(self.pos)
                    .filter(|_| self.is_p(self.pos, "("))?;
                self.space(level + 1);
                self.inline(end, level)
            }
            "VALUES" => self.values(level),
            _ => None,
        }
    }

    /// `VALUES` header and rows: on one line when they fit, else one row per
    /// line.
    fn values(&mut self, level: usize) -> Option<()> {
        let kw = self.pos;
        let header_end = if self.kind(kw + 1) == Some(TokenKind::Var) {
            kw + 1
        } else if self.is_p(kw + 1, "(") {
            self.close_of(kw + 1)?
        } else {
            return None;
        };
        let open = header_end + 1;
        if !self.is_p(open, "{") {
            return None;
        }
        let close = self.close_of(open)?;
        let w = self.inline_w(kw, close);
        if !self.d.comments_inside(kw as Ix, close as Ix)
            && self.col.saturating_add(w) <= self.width
        {
            return self.inline(close, level);
        }
        self.inline(header_end, level)?;
        self.space(level + 1);
        let o = self.bump();
        self.tok(o, self.level);
        let base = self.level;
        let empty = self.pos == close;
        while self.pos < close {
            self.start(base + 1, self.pos as Ix, Blank::No);
            let end = self.item_end(self.pos).filter(|&e| e < close)?;
            self.inline(end, base + 1)?;
        }
        self.close_group(close, base, empty);
        self.pos = close + 1;
        Some(())
    }

    /// One update operation at the current token.
    fn operation(&mut self) -> Option<()> {
        let i = self.pos;
        let kw = self.keyword(i)?;
        match kw {
            "LOAD" | "CLEAR" | "DROP" | "CREATE" | "ADD" | "MOVE" | "COPY" => {
                let mut end = i;
                while end + 1 < self.n() && !self.is_p(end + 1, ";") {
                    end += 1;
                }
                self.inline(end, 0)
            }
            "INSERT" | "DELETE" if self.is_kw(i + 1, "DATA") || self.is_kw(i + 1, "WHERE") => {
                let a = self.bump();
                self.tok(a, 0);
                self.space(1);
                let b = self.bump();
                self.tok(b, 1);
                self.space(1);
                self.group()
            }
            "WITH" | "INSERT" | "DELETE" => self.modify(),
            _ => None,
        }
    }

    /// `WITH`, `DELETE { }`, `INSERT { }`, `USING` and `WHERE { }`, each on
    /// its own line.
    fn modify(&mut self) -> Option<()> {
        let mut first = true;
        if let Some(w) = self.eat_kw("WITH") {
            self.tok(w, 0);
            self.space(1);
            let g = self.bump();
            self.tok(g, 1);
            first = false;
        }
        for kw in ["DELETE", "INSERT"] {
            if self.is_kw(self.pos, kw) {
                let t = self.bump();
                if !first {
                    self.start(0, t, Blank::Author);
                }
                first = false;
                self.tok(t, 0);
                self.space(1);
                self.group()?;
            }
        }
        while let Some(u) = self.eat_kw("USING") {
            self.start(0, u, Blank::Author);
            self.tok(u, 0);
            if let Some(named) = self.eat_kw("NAMED") {
                self.space(1);
                self.tok(named, 1);
            }
            self.space(1);
            let g = self.bump();
            self.tok(g, 1);
        }
        let w = self.eat_kw("WHERE")?;
        self.start(0, w, Blank::Author);
        self.tok(w, 0);
        self.space(1);
        self.group()
    }

    // ---- triple patterns ----

    fn triples(&mut self, level: usize) -> Option<()> {
        let t = self.statement()?;
        self.term(&t.subject, level, false);
        if !t.pol.is_empty() {
            self.space(level + 1);
            self.pol_from(&t.pol, level + 1, false, true);
        }
        if let Some(dot) = self.eat_p(".") {
            self.punct(dot, level);
        }
        Some(())
    }

    fn statement(&mut self) -> Option<Triples> {
        let subject = self.graph_node()?;
        let pol = match subject {
            Term::Atom(..) => self.pol()?,
            // `spargebra` takes a triple right after a `<< >>` without a
            // property list (in a template, without the `.` between): it
            // starts a new line then.
            _ => {
                let at = self.pos;
                self.pol().unwrap_or_else(|| {
                    self.pos = at;
                    Vec::new()
                })
            }
        };
        Some(Triples { subject, pol })
    }

    fn verb_start(&self) -> bool {
        match self.kind(self.pos) {
            Some(TokenKind::Var | TokenKind::Iri | TokenKind::PName | TokenKind::A) => true,
            Some(TokenKind::Punct) => matches!(self.raw(self.pos), "^" | "!" | "("),
            _ => false,
        }
    }

    fn pol(&mut self) -> Option<Vec<Po>> {
        let mut out = Vec::new();
        loop {
            if !self.verb_start() {
                return None;
            }
            let verb = self.verb()?;
            let objs = self.objects()?;
            let mut semis = Vec::new();
            while let Some(s) = self.eat_p(";") {
                semis.push(s);
            }
            let more = !semis.is_empty() && self.verb_start();
            out.push(Po { verb, objs, semis });
            if !more {
                return Some(out);
            }
        }
    }

    fn objects(&mut self) -> Option<Vec<Obj>> {
        let mut out = Vec::new();
        loop {
            let term = self.graph_node()?;
            let annots = self.annotations()?;
            let comma = self.eat_p(",");
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
            if let Some(t) = self.eat_p("~") {
                out.push(Term::Atom(t, t));
                if let Some(r) = self.reifier() {
                    out.push(r);
                }
            } else if self.is_p(self.pos, "{|") {
                out.push(self.list("|}")?);
            } else {
                return Some(out);
            }
        }
    }

    fn reifier(&mut self) -> Option<Term> {
        match self.kind(self.pos)? {
            TokenKind::Var | TokenKind::Iri | TokenKind::PName | TokenKind::BlankNode => {
                let t = self.bump();
                Some(Term::Atom(t, t))
            }
            TokenKind::Punct if self.is_p(self.pos, "[") && self.is_p(self.pos + 1, "]") => {
                Some(self.pair())
            }
            _ => None,
        }
    }

    fn pair(&mut self) -> Term {
        let a = self.bump();
        let b = self.bump();
        Term::Atom(a, b)
    }

    /// `[ pol ]` / `{| pol |}`; the opening token is current.
    fn list(&mut self, close: &str) -> Option<Term> {
        let open = self.bump();
        let pol = self.pol()?;
        let close = self.eat_p(close)?;
        Some(Term::List { open, pol, close })
    }

    /// A subject, object or collection item.
    fn graph_node(&mut self) -> Option<Term> {
        let i = self.pos;
        match self.kind(i)? {
            TokenKind::Iri
            | TokenKind::PName
            | TokenKind::Var
            | TokenKind::BlankNode
            | TokenKind::Number
            | TokenKind::Boolean => {
                let t = self.bump();
                Some(Term::Atom(t, t))
            }
            TokenKind::String => {
                let a = self.bump();
                let b = if self.kind(self.pos) == Some(TokenKind::LangTag) {
                    self.bump()
                } else if self.eat_p("^^").is_some() {
                    if !matches!(self.kind(self.pos), Some(TokenKind::Iri | TokenKind::PName)) {
                        return None;
                    }
                    self.bump()
                } else {
                    a
                };
                Some(Term::Atom(a, b))
            }
            TokenKind::Punct => match self.raw(i) {
                "-" | "+" if self.signed_number(i) => Some(self.pair()),
                "[" if self.is_p(i + 1, "]") => Some(self.pair()),
                "[" => self.list("]"),
                "(" if self.is_p(i + 1, ")") => Some(self.pair()),
                "(" => {
                    let open = self.bump();
                    let mut items = Vec::new();
                    loop {
                        if let Some(close) = self.eat_p(")") {
                            return Some(Term::Coll { open, items, close });
                        }
                        items.push(Obj {
                            term: self.graph_node()?,
                            annots: Vec::new(),
                            comma: None,
                        });
                    }
                }
                "<<(" => {
                    let open = self.bump();
                    let parts = vec![self.graph_node()?, self.verb()?, self.graph_node()?];
                    let close = self.eat_p(")>>")?;
                    Some(Term::Quoted { open, parts, close })
                }
                "<<" => {
                    let open = self.bump();
                    let mut parts = vec![self.graph_node()?, self.verb()?, self.graph_node()?];
                    if let Some(t) = self.eat_p("~") {
                        parts.push(Term::Atom(t, t));
                        if let Some(r) = self.reifier() {
                            parts.push(r);
                        }
                    }
                    let close = self.eat_p(">>")?;
                    Some(Term::Quoted { open, parts, close })
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// A variable or a property path, as one atom.
    fn verb(&mut self) -> Option<Term> {
        if self.kind(self.pos) == Some(TokenKind::Var) {
            let t = self.bump();
            return Some(Term::Atom(t, t));
        }
        let a = self.pos as Ix;
        self.path_alt()?;
        Some(Term::Atom(a, self.pos as Ix - 1))
    }

    fn path_alt(&mut self) -> Option<()> {
        self.path_seq()?;
        while self.eat_p("|").is_some() {
            self.path_seq()?;
        }
        Some(())
    }

    fn path_seq(&mut self) -> Option<()> {
        self.path_elt()?;
        while self.eat_p("/").is_some() {
            self.path_elt()?;
        }
        Some(())
    }

    fn path_elt(&mut self) -> Option<()> {
        self.eat_p("^");
        match self.kind(self.pos)? {
            TokenKind::Iri | TokenKind::PName | TokenKind::A => {
                self.bump();
            }
            TokenKind::Punct if self.eat_p("!").is_some() => {
                if self.eat_p("(").is_some() {
                    if self.eat_p(")").is_none() {
                        self.path_one()?;
                        while self.eat_p("|").is_some() {
                            self.path_one()?;
                        }
                        self.eat_p(")")?;
                    }
                } else {
                    self.path_one()?;
                }
            }
            TokenKind::Punct if self.eat_p("(").is_some() => {
                self.path_alt()?;
                self.eat_p(")")?;
            }
            _ => return None,
        }
        // A `+` against a number signs the object after the path.
        if self.eat_p("*").is_none()
            && self.eat_p("?").is_none()
            && self.is_p(self.pos, "+")
            && !self.signed_number(self.pos)
        {
            self.bump();
        }
        Some(())
    }

    fn path_one(&mut self) -> Option<()> {
        self.eat_p("^");
        match self.kind(self.pos)? {
            TokenKind::Iri | TokenKind::PName | TokenKind::A => {
                self.bump();
                Some(())
            }
            _ => None,
        }
    }

    // ---- triple layout ----

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
                    self.pol_from(pol, level + 1, true, false);
                    self.space(level);
                    self.tok(*close, level);
                } else {
                    let base = self.level;
                    self.pol_from(pol, base + 1, false, false);
                    self.line(base);
                    self.tok(*close, base);
                }
            }
            Term::Coll { open, items, close } => {
                let inline = flat
                    || (!self.d.comments_inside(*open, *close)
                        && t.flat()
                        && self.fits(self.term_w(t)));
                self.tok(*open, level);
                let base = self.level;
                for o in items {
                    if inline {
                        self.space(level + 1);
                    } else {
                        self.line(base + 1);
                    }
                    self.obj(o, base + 1, inline);
                }
                if inline {
                    self.space(level);
                    self.tok(*close, level);
                } else {
                    self.line(base);
                    self.tok(*close, base);
                }
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

    /// Predicate-object pairs. Broken: every pair on its own line at `level`
    /// (the first on the current line when `first_inline`); flat: separated
    /// by ` ; `. Every `;` is kept.
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
            self.objects_layout(&po.objs, level, flat);
            for s in &po.semis {
                self.punct(*s, level);
            }
        }
    }

    fn objects_layout(&mut self, objs: &[Obj], level: usize, flat: bool) {
        // A single object lays itself out; several go on one line only when
        // each of them is flat and they fit there together.
        let inline = flat
            || (objs.len() > 1
                && !self
                    .d
                    .comments_inside(objs[0].term.first(), objs[objs.len() - 1].last())
                && objs.iter().all(Obj::flat)
                && self.fits(self.objs_w(objs)));
        // Broken: groups follow each other as `] , [`; anything else goes on
        // its own line one level in from the line the list starts on.
        let base = self.level;
        let mut at = level;
        for (k, o) in objs.iter().enumerate() {
            if k > 0 {
                let prev = &objs[k - 1];
                let chained = prev.term.is_group() && prev.annots.is_empty() && o.term.is_group();
                if inline || chained {
                    self.space(at + 1);
                } else {
                    at = base + 1;
                    self.line(at);
                }
            }
            self.obj(o, at, inline);
            if let Some(c) = o.comma {
                self.punct(c, at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn spaces(width: usize) -> Options {
        Options {
            tab_size: 4,
            insert_spaces: true,
            line_width: width,
        }
    }

    fn fmt(text: &str) -> String {
        format(text, &PrefixMap::default(), None, &spaces(100))
            .unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    fn fixture(name: &str) -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("test/fixtures/format")
            .join(name);
        fs::read_to_string(p).unwrap()
    }

    #[test]
    fn golden_files() {
        for (messy, expected) in [("messy.rq", "expected.rq"), ("messy.ru", "expected.ru")] {
            let out = fmt(&fixture(messy));
            assert_eq!(out, fixture(expected), "{messy}");
            assert_eq!(fmt(&out), out, "{expected} formats to itself");
        }
    }

    #[test]
    fn tabs() {
        let text = "PREFIX ex: <https://e.org/>\nSELECT * WHERE { ?s ex:p ?o ; ex:q ?r . OPTIONAL { ?s ex:t ?u } }\n";
        let opts = Options {
            tab_size: 8,
            insert_spaces: false,
            line_width: 100,
        };
        let out = format(text, &PrefixMap::default(), None, &opts).unwrap();
        assert_eq!(
            out,
            "PREFIX ex: <https://e.org/>\n\nSELECT *\nWHERE {\n\t?s ex:p ?o ;\n\t\tex:q ?r .\n\tOPTIONAL {\n\t\t?s ex:t ?u\n\t}\n}\n"
        );
    }

    #[test]
    fn query_forms_keep_their_keywords() {
        for (text, want) in [
            ("ask{}", "ASK { }\n"),
            ("select * {?s ?p ?o}", "SELECT * {\n    ?s ?p ?o\n}\n"),
            ("ask where {?s ?p ?o}", "ASK\nWHERE {\n    ?s ?p ?o\n}\n"),
            (
                "construct{?s ?p ?o}where{?s ?p ?o}",
                "CONSTRUCT {\n    ?s ?p ?o\n}\nWHERE {\n    ?s ?p ?o\n}\n",
            ),
            ("describe <http://e.org/a>", "DESCRIBE <http://e.org/a>\n"),
            (
                "construct where{?s ?p ?o}",
                "CONSTRUCT\nWHERE {\n    ?s ?p ?o\n}\n",
            ),
        ] {
            assert_eq!(fmt(text), want, "{text}");
        }
    }

    #[test]
    fn narrow_width_breaks_projection_and_object_list() {
        let text = "PREFIX ex: <https://e.org/>\nSELECT ?alpha ?bravo ?charlie ?delta ?echo ?foxtrot WHERE { ?alpha ex:p ?bravo , ?charlie , ?delta , ?echo , ?foxtrot }\n";
        let narrow = spaces(40);
        let out = format(text, &PrefixMap::default(), None, &narrow).unwrap();
        for line in out.lines() {
            assert!(line.chars().count() <= 40, "too long: {line}\n{out}");
        }
        assert_eq!(
            format(&out, &PrefixMap::default(), None, &narrow).unwrap(),
            out
        );
        let wide = fmt(text);
        assert!(wide.contains("SELECT ?alpha ?bravo ?charlie ?delta ?echo ?foxtrot\n"));
        assert!(wide.contains("?alpha ex:p ?bravo , ?charlie , ?delta , ?echo , ?foxtrot\n"));
    }

    #[test]
    fn refuses_documents_with_errors() {
        let p = PrefixMap::default();
        assert!(format("SELECT ?x WHERE { ?x ?p }\n", &p, None, &spaces(100)).is_err());
        assert!(
            format(
                "select * where { ?s foaf:name ?n }\n",
                &p,
                None,
                &spaces(100)
            )
            .is_err()
        );
    }

    #[test]
    fn signs_and_expressions() {
        assert_eq!(
            fmt("select * { ?s ?p -1 filter(?s+1>-2 && ?s +1 != - 3 && -?s < 0) }"),
            "SELECT * {\n    ?s ?p -1\n    FILTER (?s +1 > -2 && ?s +1 != - 3 && -?s < 0)\n}\n"
        );
        assert_eq!(
            fmt("select (group_concat(?x;separator=\",\") as ?g) {?s ?p ?x} order by asc(?g)"),
            "SELECT (GROUP_CONCAT(?x; SEPARATOR = \",\") AS ?g) {\n    ?s ?p ?x\n}\nORDER BY ASC(?g)\n"
        );
    }

    #[test]
    fn exists_values_and_updates() {
        assert_eq!(
            fmt("select * {?s ?p ?o filter not exists {?s a ?t} values (?s ?o) {(1 2) (undef 3)}}"),
            "SELECT * {\n    ?s ?p ?o\n    FILTER NOT EXISTS {\n        ?s a ?t\n    }\n    VALUES (?s ?o) { (1 2) (UNDEF 3) }\n}\n"
        );
        assert_eq!(
            fmt(
                "prefix ex: <http://e/> insert data {ex:a ex:b ex:c}; prefix ex2: <http://f/> delete where {?s ex2:p ?o};"
            ),
            "PREFIX ex: <http://e/>\n\nINSERT DATA {\n    ex:a ex:b ex:c\n} ;\n\nPREFIX ex2: <http://f/>\n\nDELETE WHERE {\n    ?s ex2:p ?o\n} ;\n"
        );
    }

    #[test]
    fn comments_stay() {
        let text = "# head\nselect * { # open\n  ?s ?p ?o . # first\n\n\n  # own line\n  ?a ?b ?c\n  # last\n}\n# tail\n";
        assert_eq!(
            fmt(text),
            "# head\nSELECT * { # open\n    ?s ?p ?o . # first\n\n    # own line\n    ?a ?b ?c\n    # last\n}\n# tail\n"
        );
    }

    #[test]
    fn long_query_is_fast() {
        let body: Vec<String> = (0..2000)
            .map(|i| format!("?s{i} <https://e.org/p> ?o{i} ."))
            .collect();
        let text = format!("SELECT * WHERE {{ {} }}\n", body.join(" "));
        let t0 = std::time::Instant::now();
        let out = fmt(&text);
        assert_eq!(out.lines().count(), 2003);
        assert!(t0.elapsed().as_millis() < 2000, "{:?}", t0.elapsed());
    }
}
