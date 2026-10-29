//! Statement splice: the parse of a text after one edit, built from the parse
//! before it by parsing again only the statements around the edit. The
//! result equals a full parse of the new text; any case these rules do not
//! cover falls back, and the caller parses in full.
//!
//! - **Re-lex** from a token before the edit until a new token starts where
//!   an old one did, shifted by the edit. The lexer keeps no state between
//!   tokens, so the tokens after that point are the old ones, shifted. A
//!   token can read ahead past its end (`1.` then `5`, a prefixed name's run
//!   of dots, `^^`, `{|`), but never past the next blank, so the re-lex
//!   starts at a blank before the edit.
//! - **Re-parse** from one statement before the token window, in the state
//!   the old parse had there (prefixes, base, graph block), until the parser
//!   is at the top of its loop, past the re-lexed tokens, where the old one
//!   was (the start of a statement, graph block or `@prefix`, or a block's
//!   `}`). Error recovery crosses statement ends, so the stopping point is
//!   where the parser is, not where a statement ended.
//! - **Check** by parsing the old window again from the same state: it must
//!   give what the old parse has there, which proves the state, and its
//!   errors locate the window's errors in the old list.
//! - **Splice** the tokens, comments, statements, triples, references,
//!   prefix uses and graph blocks, and each of the three error groups apart
//!   (the lexer's, the parser's, the N-Triples restrictions), and shift what
//!   follows: spans by the length change, statement indexes by the change in
//!   statements, blank nodes allocated after by the change in blank nodes.
//!
//! The fallbacks ([`Fallback`]) are strict: a fallback costs only speed.

use super::lexer::{Kind, Token, tokenize};
use super::parser::{
    self, Ended, Halt, MAX_NESTING, Restriction, Start, Top, lexer_error, restriction_of,
    statement_restriction, token_restriction,
};
use super::*;

/// The window a splice may parse, as a share of the new text: past it a full
/// parse costs about the same.
const MAX_SHARE: usize = 4;

/// Bytes a window may always cover, however small the text: below this a
/// full parse and a splice both cost next to nothing.
const MIN_WINDOW: usize = 4096;

/// Triples looked at backwards from the window for the blank nodes
/// allocated before it.
const MAX_BLANK_SCAN: usize = 4096;

/// Statements of a graph block looked at for its indentation.
const MAX_INDENT_SCAN: usize = 64;

/// How a text changed: bytes `start..end` of the old text became `new_len`
/// bytes, which start at `start` in the text now. Several edits collapse
/// into one ([`Edit::then`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) new_len: u32,
}

impl Edit {
    /// The bytes `range` of the text were replaced by `new_len` bytes.
    pub(crate) fn new(range: std::ops::Range<usize>, new_len: usize) -> Edit {
        Edit {
            start: range.start as u32,
            end: range.end as u32,
            new_len: new_len as u32,
        }
    }

    /// Where the replacement ends in the text now.
    pub(crate) fn new_end(&self) -> u32 {
        self.start + self.new_len
    }

    /// This edit, then the bytes `range` of the text it left replaced by
    /// `new_len` bytes, as one edit: the smallest that covers both.
    pub(crate) fn then(self, range: std::ops::Range<usize>, new_len: usize) -> Edit {
        let (start, end) = (range.start as u32, range.end as u32);
        let lo = self.start.min(start);
        let hi = self.new_end().max(end);
        Edit {
            start: lo,
            // Past this edit's replacement, the text is the old text moved.
            end: self.end + (hi - self.new_end()),
            new_len: hi - lo - (end - start) + new_len as u32,
        }
    }
}

/// What a splice parsed again: the statements `statements.start..end` of
/// the old parse became `statements.new_len` statements, which start at
/// `statements.start` in the new one, and the bytes `bytes` hold them in
/// each text. What comes before is as it was; what comes after is as it
/// was, moved by the change in length and in statements. Two windows in a
/// row collapse into one ([`Window::then`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Window {
    pub(crate) statements: Edit,
    pub(crate) bytes: Edit,
}

impl Window {
    /// This window, then `next` on the parse it left, as one window.
    pub(crate) fn then(self, next: Window) -> Window {
        let range = |e: Edit| e.start as usize..e.end as usize;
        Window {
            statements: self
                .statements
                .then(range(next.statements), next.statements.new_len as usize),
            bytes: self
                .bytes
                .then(range(next.bytes), next.bytes.new_len as usize),
        }
    }

    /// Where `x`, a position of the old text outside the window, is in the
    /// new one.
    pub(crate) fn moved(&self, x: u32) -> u32 {
        let b = self.bytes;
        if x < b.end {
            x
        } else {
            x + b.new_len - (b.end - b.start)
        }
    }
}

/// The texts before and after one edit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Change<'t> {
    pub(crate) old_text: &'t str,
    pub(crate) new_text: &'t str,
    pub(crate) edit: Edit,
}

/// Why a splice gave up; the caller parses the new text in full.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fallback {
    /// JSON-LD, which has its own frontend.
    Syntax,
    /// The edit does not fit the two texts.
    Edit,
    /// The external prefixes or the document's base changed.
    Context,
    /// The old parse kept no tokens.
    NoTokens,
    /// The old parse has no statements.
    NoStatements,
    /// A directive in the old or the new window.
    Directive,
    /// `@base` more than once: the base in effect at the window is not known.
    Base,
    /// A `_:label` in the old or the new window.
    BlankLabel,
    /// A TriG `{`, `}` or `GRAPH` in the old or new window, or a window in a
    /// block that does not close.
    GraphBlock,
    /// Nesting near the parser's maximum.
    DeepNesting,
    /// The window changed the indentation a TriG block's error recovery
    /// goes by.
    Indent,
    /// The window covers more than a quarter of the text.
    WindowTooLarge,
    /// The number of blank nodes allocated before the window is not known.
    BlankCount,
    /// The old window, parsed again, is not what the old parse has: the
    /// state at the window is not the one the old parse had.
    Unsure,
}

/// The parse of `change.new_text`, from `old`, the full parse of
/// `change.old_text` (with its tokens and comment spans) with the external
/// prefixes `old_external`, and the window it parsed again; or why not. The
/// new parse has `external` and the document's `base`.
pub(crate) fn splice(
    old: Parsed,
    change: Change,
    old_external: &PrefixMap,
    external: &PrefixMap,
    base: Option<&str>,
) -> Result<(Parsed, Window), Fallback> {
    applies(&old, change, old_external, external, base)?;
    if old.tokens.is_empty() {
        return Err(Fallback::NoTokens);
    }
    Splice::new(old, change, change.edit, Start::document(external, base))?.run()
}

/// The fallbacks of [`splice`] that need no more than `old` without its
/// tokens: a caller checks these before it makes the parse the splice takes.
pub(crate) fn applies(
    old: &Parsed,
    change: Change,
    old_external: &PrefixMap,
    external: &PrefixMap,
    base: Option<&str>,
) -> Result<(), Fallback> {
    if old.syntax == Syntax::JsonLd {
        return Err(Fallback::Syntax);
    }
    if old_external != external || old.document_base.as_deref() != base {
        return Err(Fallback::Context);
    }
    fits(change)?;
    if old.statements.is_empty() {
        return Err(Fallback::NoStatements);
    }
    Ok(())
}

/// Whether the edit fits the texts.
fn fits(change: Change) -> Result<(), Fallback> {
    let Change {
        old_text,
        new_text,
        edit,
    } = change;
    let (start, end, new_len) = (
        edit.start as usize,
        edit.end as usize,
        edit.new_len as usize,
    );
    let fits = start <= end
        && end <= old_text.len()
        && old_text.len() - (end - start) + new_len == new_text.len()
        && old_text.is_char_boundary(start)
        && old_text.is_char_boundary(end)
        && new_text.is_char_boundary(start + new_len)
        && u32::try_from(new_text.len()).is_ok();
    if fits { Ok(()) } else { Err(Fallback::Edit) }
}

/// A splice under way. Positions are bytes; "old" ones are in the old text.
struct Splice<'t> {
    p: Parsed,
    old_text: &'t str,
    new_text: &'t str,
    syntax: Syntax,
    /// Length change.
    delta: i64,
    /// Bytes a window may cover.
    limit: usize,
    /// The re-lexed tokens: `k0..k1` of the old tokens became `k0..k1n` of
    /// the new ones. `from` is where the re-lex started.
    k0: usize,
    k1: usize,
    k1n: usize,
    from: u32,
    /// The old tokens `k0..k1`.
    removed: Vec<Token>,
    /// Errors of the old tokens before `k0`, and after `k1`.
    lexer_errors: usize,
    /// The parse window starts at statement `a`, at byte `p0` and token
    /// `t0`, in state `start`.
    a: usize,
    p0: u32,
    t0: usize,
    start: Start,
}

impl<'t> Splice<'t> {
    /// Re-lex, find the start of the parse window and its state, and splice
    /// the tokens.
    fn new(
        mut p: Parsed,
        change: Change<'t>,
        edit: Edit,
        document: Start,
    ) -> Result<Splice<'t>, Fallback> {
        let Change {
            old_text, new_text, ..
        } = change;
        let syntax = p.syntax;
        let delta = edit.new_len as i64 - (edit.end - edit.start) as i64;
        let limit = (new_text.len() / MAX_SHARE).max(MIN_WINDOW);
        let toks = &p.tokens;

        // Back from the edit to a token that a blank follows before the
        // edit: no token up to it read the edited bytes. Re-lex from there.
        let bytes = old_text.as_bytes();
        let mut k0 = toks.partition_point(|t| t.start < edit.start);
        while k0 > 0 && {
            let end = toks[k0 - 1].end;
            end >= edit.start || !matches!(bytes[end as usize], b' ' | b'\t' | b'\r' | b'\n')
        } {
            k0 -= 1;
        }
        let from = if k0 == 0 { 0 } else { toks[k0 - 1].end };
        let new_end = edit.start + edit.new_len;
        let (lexed, k1) = relex(new_text, from, toks, k0, new_end, delta, limit, syntax)?;
        screen(toks[k0..k1].iter().chain(&lexed), syntax)?;

        // One statement before the one holding the first re-lexed token:
        // recovery and lookahead cross statement ends.
        let pivot = toks.get(k0).map_or(old_text.len() as u32, |t| t.start);
        let holder = p.statements.partition_point(|s| s.span.start <= pivot);
        let at = match holder.checked_sub(1) {
            Some(j) => {
                // The header of a graph block reads no further than its `{`.
                let g = p
                    .graphs
                    .partition_point(|g| g.statements.start as usize <= j);
                let opens_block = g.checked_sub(1).is_some_and(|g| {
                    let g = &p.graphs[g].statements;
                    g.start as usize == j && j < g.end as usize
                });
                let a = if opens_block { j } else { j.saturating_sub(1) };
                let t0 = token_at(toks, p.statements[a].span.start).ok_or(Fallback::Unsure)?;
                // Whatever comes before the first statement read its first
                // token: start at the document's start then.
                (opens_block || a > 0 || t0 < k0).then_some((a, t0))
            }
            None => None,
        };
        let (a, p0, t0, start) = match at {
            Some((a, t0)) => {
                let p0 = p.statements[a].span.start;
                let start = state_at(&p, old_text, document, a, p0, t0)?;
                (a, p0, t0, start)
            }
            None => (0, 0, 0, document),
        };

        let k1n = k0 + lexed.len();
        let removed: Vec<Token> = p.tokens.splice(k0..k1, lexed).collect();
        let mut lexer_errors = p.tokens[..k0]
            .iter()
            .filter(|t| t.error().is_some())
            .count();
        for t in &mut p.tokens[k1n..] {
            lexer_errors += usize::from(t.error().is_some());
            t.start = moved(t.start, delta);
            t.end = moved(t.end, delta);
        }
        Ok(Splice {
            p,
            old_text,
            new_text,
            syntax,
            delta,
            limit,
            k0,
            k1,
            k1n,
            from,
            removed,
            lexer_errors,
            a,
            p0,
            t0,
            start,
        })
    }

    /// Where a new position at or after the re-lexed tokens was in the old text.
    fn old_pos(&self, x: u32) -> u32 {
        moved(x, -self.delta)
    }

    fn run(mut self) -> Result<(Parsed, Window), Fallback> {
        let syntax = self.syntax;
        let (k1n, p0, limit, delta) = (self.k1n, self.p0, self.limit, self.delta);

        // The new window: until the parser is at the top of its loop, past
        // the re-lexed tokens, where the old parser was: at the start of
        // every statement, graph block and `@prefix` it kept, and, in a
        // block, at every `}`. Only a statement in a block reads the gap
        // before its first token (for its indentation), so there the token
        // before must be past the re-lexed ones too.
        let in_block = self.start.block.is_some();
        let mut too_large = false;
        let new_win = {
            let (tokens, statements) = (&self.p.tokens, &self.p.statements);
            let (graphs, prefixes) = (&self.p.graphs, &self.p.prefixes);
            let mut until = |top: Top| {
                let Some(t) = tokens.get(top.pos) else {
                    return Halt::Go;
                };
                if (t.start - p0) as usize > limit {
                    too_large = true;
                    return Halt::Stop;
                }
                let old = moved(t.start, -delta);
                let gap_kept = !in_block || top.prev.is_some_and(|q| q >= k1n);
                // Past the start in the old text too, so the errors found
                // before the window and after it never share a position.
                let synced = top.pos >= k1n
                    && old > p0
                    && ((in_block && t.kind == Kind::RBrace)
                        || (gap_kept
                            && statements
                                .binary_search_by_key(&old, |s| s.span.start)
                                .is_ok())
                        || graphs.binary_search_by_key(&old, |g| g.span.start).is_ok()
                        || prefixes
                            .binary_search_by_key(&old, |d| d.span.start)
                            .is_ok());
                if synced { Halt::Stop } else { Halt::Go }
            };
            parser::window(
                self.new_text,
                tokens,
                syntax,
                self.start.clone(),
                &mut until,
            )
        };
        let tokens = &self.p.tokens;
        let (y, c_old) = match new_win.ended {
            _ if too_large => return Err(Fallback::WindowTooLarge),
            Ended::Stopped(y) => {
                let old = self.old_pos(tokens[y].start);
                (y, self.p.statements.partition_point(|s| s.span.start < old))
            }
            Ended::End => {
                if self.new_text.len() - p0 as usize > limit {
                    return Err(Fallback::WindowTooLarge);
                }
                (tokens.len(), self.p.statements.len())
            }
            Ended::Left(at) => {
                let directive = tokens.get(at).is_some_and(|t| is_directive(t.kind));
                return Err(if directive {
                    Fallback::Directive
                } else {
                    Fallback::GraphBlock
                });
            }
        };
        screen(tokens[self.t0..y].iter().chain(&self.removed), syntax)?;
        let y_old = y - k1n + self.k1;

        // The old window again, from the same state, on the old tokens.
        let first = self.start.prev.unwrap_or(self.t0);
        let mut old_toks: Vec<Token> = tokens[first..self.k0].to_vec();
        old_toks.extend_from_slice(&self.removed);
        let margin = lookahead_end(tokens, y);
        old_toks.extend(tokens[k1n..margin].iter().map(|t| {
            let mut t = *t;
            t.start = moved(t.start, -delta);
            t.end = moved(t.end, -delta);
            t
        }));
        let y_rel = y_old - first;
        let mut old_start = self.start.clone();
        old_start.at = self.t0 - first;
        old_start.prev = old_start.prev.map(|q| q - first);
        let mut until = |top: Top| {
            if top.pos >= y_rel {
                Halt::Stop
            } else {
                Halt::Go
            }
        };
        let old_win = parser::window(self.old_text, &old_toks, syntax, old_start, &mut until);
        let expected = match new_win.ended {
            Ended::End => Ended::End,
            _ => Ended::Stopped(y_rel),
        };
        if old_win.ended != expected {
            return Err(Fallback::Unsure);
        }
        // Past a `}` the block's indentation is not used again.
        let closes = tokens.get(y).is_some_and(|t| t.kind == Kind::RBrace);
        if in_block && !closes && old_win.indent != new_win.indent {
            return Err(Fallback::Indent);
        }
        if new_win.deepest.max(old_win.deepest) + 8 >= MAX_NESTING {
            return Err(Fallback::DeepNesting);
        }

        // Where the old window is in each list of the old parse.
        let a = self.a;
        let c_pos = tokens.get(y).map_or(u32::MAX, |t| self.old_pos(t.start));
        let t_lo = self
            .p
            .triples
            .partition_point(|t| (t.statement as usize) < a);
        let t_hi = self
            .p
            .triples
            .partition_point(|t| (t.statement as usize) < c_old);
        let r_lo = self.p.refs.partition_point(|r| r.span.start < p0);
        let r_hi = self.p.refs.partition_point(|r| r.span.start < c_pos);
        let u_lo = self.p.prefix_uses.partition_point(|u| u.span.start < p0);
        let u_hi = self.p.prefix_uses.partition_point(|u| u.span.start < c_pos);

        // Blank nodes: both windows are numbered from 1; `c` were allocated
        // before them.
        let (n_old, n_new) = (old_win.bnodes, new_win.bnodes);
        let mut old_triples = old_win.out.triples;
        let c = if n_old == 0 && n_new == 0 {
            0
        } else {
            match offset(&self.p.triples[t_lo..t_hi], &old_triples) {
                Some(c) => c,
                None => self.allocated_before(t_lo)?,
            }
        };
        for t in &mut old_triples {
            shift_terms(t, 0, c as i64);
        }
        let same = same_statements(&self.p.statements[a..c_old], &old_win.out.statements)
            && same_triples(&self.p.triples[t_lo..t_hi], &old_triples)
            && same_refs(&self.p.refs[r_lo..r_hi], &old_win.out.refs)
            && same_uses(&self.p.prefix_uses[u_lo..u_hi], &old_win.out.prefix_uses);
        if !same {
            return Err(Fallback::Unsure);
        }

        let mut out = new_win.out;
        for t in &mut out.triples {
            shift_terms(t, 0, c as i64);
        }
        let window_errors = std::mem::take(&mut out.errors);
        let errors = self.errors(
            &out.statements,
            window_errors,
            &old_win.out.errors,
            a,
            c_old,
        )?;

        // Splice, and shift what follows.
        let c_end = c_pos.min(self.old_text.len() as u32);
        let window = Window {
            statements: Edit::new(a..c_old, out.statements.len()),
            bytes: Edit::new(
                p0 as usize..c_end as usize,
                moved(c_end, delta) as usize - p0 as usize,
            ),
        };
        let ds = out.statements.len() as i64 - (c_old - a) as i64;
        let db = n_new as i64 - n_old as i64;
        let above = c + n_old;
        let (from, to) = (self.from, self.old_end_of_relex());
        let p = &mut self.p;
        p.errors = errors;

        let n = out.statements.len();
        p.statements.splice(a..c_old, out.statements);
        for s in &mut p.statements[a + n..] {
            s.span = moved_span(s.span, delta);
            if let Some((_, span)) = &mut s.subject {
                *span = moved_span(*span, delta);
            }
            if let Some((g, span)) = &mut s.graph {
                shift_graph_blank(g, above, db);
                *span = moved_span(*span, delta);
            }
        }

        let n = out.triples.len();
        p.triples.splice(t_lo..t_hi, out.triples);
        for t in &mut p.triples[t_lo + n..] {
            t.statement = (t.statement as i64 + ds) as u32;
            t.p_span = moved_span(t.p_span, delta);
            t.o_span = moved_span(t.o_span, delta);
            if db != 0 {
                shift_triple_blanks(t, above, db);
            }
        }

        let n = out.refs.len();
        p.refs.splice(r_lo..r_hi, out.refs);
        for r in &mut p.refs[r_lo + n..] {
            r.span = moved_span(r.span, delta);
        }

        let n = out.prefix_uses.len();
        p.prefix_uses.splice(u_lo..u_hi, out.prefix_uses);
        for u in &mut p.prefix_uses[u_lo + n..] {
            u.span = moved_span(u.span, delta);
        }

        // No directive is in the window: those from its start on are after it.
        let d_lo = p.prefixes.partition_point(|d| d.span.start < p0);
        for d in &mut p.prefixes[d_lo..] {
            d.span = moved_span(d.span, delta);
            d.name_span = moved_span(d.name_span, delta);
            d.iri_span = moved_span(d.iri_span, delta);
        }

        let c_lo = p.comments.partition_point(|s| s.start < from);
        let c_hi = p.comments.partition_point(|s| s.start < to);
        let comments: Vec<Span> = p.tokens[self.k0..k1n]
            .iter()
            .filter(|t| t.kind == Kind::Comment)
            .map(|t| Span::new(t.start, t.end))
            .collect();
        let n = comments.len();
        p.comments.splice(c_lo..c_hi, comments);
        for s in &mut p.comments[c_lo + n..] {
            *s = moved_span(*s, delta);
        }

        // No block starts in the window: those from its end on are after
        // it, and the one before, if it holds the window, ends after it.
        let after = p.graphs.partition_point(|g| g.span.start < c_pos);
        if let Some(g) = after.checked_sub(1).map(|i| &mut p.graphs[i])
            && g.statements.end as usize > a
        {
            g.statements.end = (g.statements.end as i64 + ds) as u32;
            g.span.end = moved(g.span.end, delta);
        }
        for g in &mut p.graphs[after..] {
            g.statements =
                (g.statements.start as i64 + ds) as u32..(g.statements.end as i64 + ds) as u32;
            g.span = moved_span(g.span, delta);
            g.name_span = g.name_span.map(|s| moved_span(s, delta));
            if let Some(name) = &mut g.name {
                shift_graph_blank(name, above, db);
            }
        }
        Ok((self.p, window))
    }

    /// Where the old tokens after the re-lexed ones start, in the old text.
    fn old_end_of_relex(&self) -> u32 {
        match self.p.tokens.get(self.k1n) {
            Some(t) => self.old_pos(t.start),
            None => self.old_text.len() as u32,
        }
    }

    /// The errors of the new parse: each of the three groups of the old
    /// list with its window's errors replaced and the rest shifted.
    fn errors(
        &mut self,
        window: &[Statement],
        window_errors: Vec<SyntaxError>,
        old_window: &[SyntaxError],
        a: usize,
        c_old: usize,
    ) -> Result<Vec<SyntaxError>, Fallback> {
        let quads = self.syntax == Syntax::NQuads;
        let line_based = self.syntax.is_line_based();
        let old = std::mem::take(&mut self.p.errors);
        let len = old.len();

        // The groups: the lexer's, the parser's, then the restrictions on
        // tokens and on statements.
        let lex_end =
            self.lexer_errors + self.removed.iter().filter(|t| t.error().is_some()).count();
        let (mut tok_start, mut st_start) = (len, len);
        if line_based {
            while st_start > 0
                && restriction_of(&old[st_start - 1], quads) == Some(Restriction::Statement)
            {
                st_start -= 1;
            }
            tok_start = st_start;
            while tok_start > 0
                && restriction_of(&old[tok_start - 1], quads) == Some(Restriction::Token)
            {
                tok_start -= 1;
            }
        }
        if lex_end > tok_start {
            return Err(Fallback::Unsure);
        }
        let (from, to) = (self.from, self.old_end_of_relex());
        let new_tokens = &self.p.tokens[self.k0..self.k1n];

        // The lexer's: by token position.
        let lex = &old[..lex_end];
        let lex_cut = lex.partition_point(|e| e.span.start < from)
            ..lex.partition_point(|e| e.span.start < to);
        let was: Vec<SyntaxError> = self.removed.iter().filter_map(lexer_error).collect();
        if !same_errors(&lex[lex_cut.clone()], &was) {
            return Err(Fallback::Unsure);
        }
        let lex_new: Vec<SyntaxError> = new_tokens.iter().filter_map(lexer_error).collect();

        // The parser's: those found before the window's start all start at
        // or before it, and the window's own come next.
        let parsed = &old[lex_end..tok_start];
        let p0 = self.p0;
        let hi = parsed
            .iter()
            .position(|e| e.span.start > p0)
            .unwrap_or(parsed.len());
        let at_start = old_window.iter().take_while(|e| e.span.start == p0).count();
        let i = hi.checked_sub(at_start).ok_or(Fallback::Unsure)?;
        let m = old_window.len();
        if i + m > parsed.len() || !same_errors(&parsed[i..i + m], old_window) {
            return Err(Fallback::Unsure);
        }
        let parse_cut = lex_end + i..lex_end + i + m;

        // The restrictions on tokens: by token position.
        let toks = &old[tok_start..st_start];
        let tok_cut = tok_start + toks.partition_point(|e| e.span.start < from)
            ..tok_start + toks.partition_point(|e| e.span.start < to);
        let (mut tok_new, mut st_new) = (Vec::new(), Vec::new());
        let mut st_cut = st_start..st_start;
        if line_based {
            let was: Vec<SyntaxError> = self
                .removed
                .iter()
                .filter_map(|t| token_restriction(self.old_text, t, quads))
                .collect();
            if !same_errors(&old[tok_cut.clone()], &was) {
                return Err(Fallback::Unsure);
            }
            tok_new = new_tokens
                .iter()
                .filter_map(|t| token_restriction(self.new_text, t, quads))
                .collect();

            // On statements: the window's, and the one after it, which
            // follows another statement now.
            let statements = &self.p.statements;
            let last = c_old.min(statements.len() - 1);
            let prev_end = |j: usize| j.checked_sub(1).map(|i| statements[i].span.end);
            let was: Vec<SyntaxError> = (a..=last)
                .filter_map(|j| {
                    statement_restriction(self.old_text, prev_end(j), statements[j].span, quads)
                })
                .collect();
            let sts = &old[st_start..];
            let c_pos = statements.get(c_old).map_or(u32::MAX, |s| s.span.start);
            st_cut = st_start + sts.partition_point(|e| e.span.start < self.p0)
                ..st_start + sts.partition_point(|e| e.span.start <= c_pos);
            if !same_errors(&old[st_cut.clone()], &was) {
                return Err(Fallback::Unsure);
            }
            let mut prev = prev_end(a);
            let next = statements
                .get(c_old)
                .map(|s| moved_span(s.span, self.delta));
            for span in window.iter().map(|s| s.span).chain(next) {
                st_new.extend(statement_restriction(self.new_text, prev, span, quads));
                prev = Some(span.end);
            }
        }

        let groups = [
            (0..lex_end, lex_cut, lex_new),
            (lex_end..tok_start, parse_cut, window_errors),
            (tok_start..st_start, tok_cut, tok_new),
            (st_start..len, st_cut, st_new),
        ];
        let mut errors = Vec::with_capacity(len);
        let mut old = old.into_iter();
        for (group, cut, new) in groups {
            errors.extend(old.by_ref().take(cut.start - group.start));
            old.by_ref().take(cut.len()).for_each(drop);
            errors.extend(new);
            errors.extend(old.by_ref().take(group.end - cut.end).map(|mut e| {
                e.span = moved_span(e.span, self.delta);
                e
            }));
        }
        Ok(errors)
    }

    /// Blank nodes allocated before the window, read from the last statement
    /// before it that holds one: when that statement has no `_:label`, its
    /// largest is the last allocated, provided no error from there on hides
    /// one (a statement abandoned half way keeps no triple).
    fn allocated_before(&self, t_lo: usize) -> Result<u32, Fallback> {
        let p = &self.p;
        let quads = self.syntax == Syntax::NQuads;
        let blank_graph = |t: &Triple| quads && matches!(t.graph, Some(GraphName::Blank(_)));
        let named_blank = p
            .graphs
            .iter()
            .take_while(|g| g.span.start < self.p0)
            .any(|g| matches!(g.name, Some(GraphName::Blank(_))));
        if named_blank {
            return Err(Fallback::BlankCount);
        }
        let found = p.triples[..t_lo]
            .iter()
            .rev()
            .take(MAX_BLANK_SCAN + 1)
            .enumerate()
            .find(|(_, t)| max_blank(&t.s).max(max_blank(&t.o)) > 0 || blank_graph(t));
        let (c, from) = match found {
            Some((n, _)) if n == MAX_BLANK_SCAN => return Err(Fallback::BlankCount),
            Some((_, t)) => {
                let j = t.statement;
                let span = p.statements[j as usize].span;
                let lo = p.tokens.partition_point(|t| t.start < span.start);
                let hi = p.tokens.partition_point(|t| t.start < span.end);
                if p.tokens[lo..hi].iter().any(|t| t.kind == Kind::BNode) {
                    return Err(Fallback::BlankCount);
                }
                let before = &p.triples[..t_lo];
                let mine = before.partition_point(|t| t.statement < j)
                    ..before.partition_point(|t| t.statement <= j);
                let c = before[mine]
                    .iter()
                    .map(|t| {
                        let g = match t.graph {
                            Some(GraphName::Blank(b)) if quads => b.0,
                            _ => 0,
                        };
                        max_blank(&t.s).max(max_blank(&t.o)).max(g)
                    })
                    .max()
                    .unwrap_or(0);
                (c, span.start)
            }
            None if t_lo > MAX_BLANK_SCAN => return Err(Fallback::BlankCount),
            None => (0, 0),
        };
        let hidden = p
            .errors
            .iter()
            .any(|e| e.span.start >= from && e.span.start <= self.p0);
        if hidden {
            return Err(Fallback::BlankCount);
        }
        Ok(c)
    }
}

/// Lex `text` from `from` until a token at or past `new_end` starts where
/// an old token did (shifted by `delta`): the tokens before it, and the
/// index of that old token (the old token count if none). Lexed in growing
/// pieces; a token counts once a blank follows it inside the piece, as no
/// token reads past the next blank.
#[allow(clippy::too_many_arguments)]
fn relex(
    text: &str,
    from: u32,
    old: &[Token],
    k0: usize,
    new_end: u32,
    delta: i64,
    limit: usize,
    syntax: Syntax,
) -> Result<(Vec<Token>, usize), Fallback> {
    let mut out = Vec::new();
    let mut pos = from as usize;
    let mut piece_len = 256;
    loop {
        if pos - from as usize > limit {
            return Err(Fallback::WindowTooLarge);
        }
        let mut end = (pos + piece_len).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        let piece = &text[pos..end];
        let toks = tokenize(piece);
        let settled = if end == text.len() {
            toks.len()
        } else {
            match piece
                .bytes()
                .rposition(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
            {
                Some(w) => toks.partition_point(|t| t.end as usize <= w),
                None => 0,
            }
        };
        for t in &toks[..settled] {
            let mut t = *t;
            t.start += pos as u32;
            t.end += pos as u32;
            if syntax != Syntax::TriG && t.kind == Kind::Graph {
                t = t.into_unexpected_word();
            }
            if t.start >= new_end {
                let at = moved(t.start, -delta);
                if let Ok(j) = old[k0..].binary_search_by_key(&at, |o| o.start) {
                    return Ok((out, k0 + j));
                }
            }
            out.push(t);
        }
        if end == text.len() {
            return Ok((out, old.len()));
        }
        match settled.checked_sub(1) {
            Some(last) => pos += toks[last].end as usize,
            None => piece_len *= 2,
        }
    }
}

/// The parser's state at the top of its statement loop at statement `a`
/// (at byte `p0`, token `t0`), as the old parse had it.
fn state_at(
    p: &Parsed,
    text: &str,
    document: Start,
    a: usize,
    p0: u32,
    t0: usize,
) -> Result<Start, Fallback> {
    let toks = &p.tokens;
    let mut start = document;
    for d in p.prefixes.iter().take_while(|d| d.span.start < p0) {
        start.prefixes.insert(d.name.clone(), d.iri);
    }
    // One `@base` before the window: the last one, which the parse kept.
    if let Some(last) = &p.base {
        let mut bases = toks
            .iter()
            .filter(|t| matches!(t.kind, Kind::BaseDirective | Kind::SparqlBase));
        let first = bases.next();
        if bases.next().is_some() {
            return Err(Fallback::Base);
        }
        if first.is_some_and(|t| t.start < p0)
            && let Ok(b) = oxiri::Iri::parse(last.clone())
        {
            start.base = Some(b);
        }
    }
    start.statement = a as u32;
    start.at = t0;
    start.prev = prev_token(toks, t0);
    // The graph block around statement `a`, if any.
    let i = p
        .graphs
        .partition_point(|g| g.statements.start as usize <= a);
    if let Some(g) = i.checked_sub(1).map(|i| &p.graphs[i])
        && a < g.statements.end as usize
    {
        let closed = toks
            .binary_search_by_key(&g.span.end, |t| t.end)
            .is_ok_and(|i| toks[i].kind == Kind::RBrace);
        if !closed {
            return Err(Fallback::GraphBlock);
        }
        let before = g.statements.start as usize..a;
        if before.len() > MAX_INDENT_SCAN {
            return Err(Fallback::Unsure);
        }
        let mut indent = None;
        for s in &p.statements[before] {
            let at = token_at(toks, s.span.start).ok_or(Fallback::Unsure)?;
            let from = prev_token(toks, at).map(|i| toks[i].end);
            indent = parser::line_indent(text, from, s.span.start);
            if indent.is_some() {
                break;
            }
        }
        start.block = Some((g.name, indent));
    }
    Ok(start)
}

/// The index of the token starting at `offset`.
fn token_at(toks: &[Token], offset: u32) -> Option<usize> {
    toks.binary_search_by_key(&offset, |t| t.start).ok()
}

/// The token before `i` that is not a comment.
fn prev_token(toks: &[Token], i: usize) -> Option<usize> {
    toks[..i].iter().rposition(|t| t.kind != Kind::Comment)
}

/// The end of the tokens a parse stopping at `y` may have looked at: three
/// more that are not comments.
fn lookahead_end(toks: &[Token], y: usize) -> usize {
    let mut i = y;
    let mut seen = 0;
    while i < toks.len() && seen < 3 {
        seen += usize::from(toks[i].kind != Kind::Comment);
        i += 1;
    }
    i
}

fn is_directive(k: Kind) -> bool {
    matches!(
        k,
        Kind::PrefixDirective
            | Kind::BaseDirective
            | Kind::VersionDirective
            | Kind::SparqlPrefix
            | Kind::SparqlBase
            | Kind::SparqlVersion
    )
}

/// Tokens that change the parser's state past a window, in an old or new
/// window: directives, `_:label`s (their numbering) and, in TriG, graph
/// blocks.
fn screen<'k>(toks: impl Iterator<Item = &'k Token>, syntax: Syntax) -> Result<(), Fallback> {
    let trig = syntax == Syntax::TriG;
    for t in toks {
        match t.kind {
            k if is_directive(k) => return Err(Fallback::Directive),
            Kind::BNode => return Err(Fallback::BlankLabel),
            Kind::LBrace | Kind::RBrace | Kind::Graph if trig => return Err(Fallback::GraphBlock),
            _ => {}
        }
    }
    Ok(())
}

fn moved(x: u32, delta: i64) -> u32 {
    (x as i64 + delta) as u32
}

fn moved_span(s: Span, delta: i64) -> Span {
    Span::new(moved(s.start, delta), moved(s.end, delta))
}

/// The largest blank node in `n`, 0 for none.
fn max_blank(n: &Node) -> u32 {
    match n {
        Node::Blank(b) => b.0,
        Node::Triple(t) => max_blank(&t.0).max(max_blank(&t.2)),
        _ => 0,
    }
}

/// The first blank node in `n`, depth first.
fn first_blank(n: &Node) -> Option<u32> {
    match n {
        Node::Blank(b) => Some(b.0),
        Node::Triple(t) => first_blank(&t.0).or_else(|| first_blank(&t.2)),
        _ => None,
    }
}

/// Add `by` to every blank node over `above`.
fn shift_blanks(n: &mut Node, above: u32, by: i64) {
    match n {
        Node::Blank(b) if b.0 > above => b.0 = (b.0 as i64 + by) as u32,
        Node::Triple(t) => {
            shift_blanks(&mut t.0, above, by);
            shift_blanks(&mut t.2, above, by);
        }
        _ => {}
    }
}

fn shift_graph_blank(g: &mut GraphName, above: u32, by: i64) {
    if let GraphName::Blank(b) = g
        && b.0 > above
    {
        b.0 = (b.0 as i64 + by) as u32;
    }
}

/// Shift the blank nodes in a triple's terms; its graph, a window's
/// block's name, was allocated before the window.
fn shift_terms(t: &mut Triple, above: u32, by: i64) {
    shift_blanks(&mut t.s, above, by);
    shift_blanks(&mut t.o, above, by);
}

fn shift_triple_blanks(t: &mut Triple, above: u32, by: i64) {
    shift_terms(t, above, by);
    if let Some(g) = &mut t.graph {
        shift_graph_blank(g, above, by);
    }
}

/// The blank nodes allocated before a window, from its first blank node in
/// the old parse and in the window parse (numbered from 1).
fn offset(old: &[Triple], window: &[Triple]) -> Option<u32> {
    old.iter().zip(window).find_map(|(o, w)| {
        let first = |t: &Triple| first_blank(&t.s).or_else(|| first_blank(&t.o));
        let (o, w) = (first(o)?, first(w)?);
        o.checked_sub(w)
    })
}

// Field by field, until `Parsed` and its parts have `PartialEq`.

fn same_statements(a: &[Statement], b: &[Statement]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x.span, x.subject, x.graph) == (y.span, y.subject, y.graph))
}

fn same_triples(a: &[Triple], b: &[Triple]) -> bool {
    let key = |t: &Triple| (t.p, t.p_span, t.o_span, t.statement, t.graph, t.unresolved);
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| x.s == y.s && x.o == y.o && key(x) == key(y))
}

fn same_refs(a: &[Ref], b: &[Ref]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x.span, x.iri, x.role, x.form) == (y.span, y.iri, y.role, y.form))
}

fn same_uses(a: &[PrefixUse], b: &[PrefixUse]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x.name, x.span, x.resolved) == (y.name, y.span, y.resolved))
}

fn same_errors(a: &[SyntaxError], b: &[SyntaxError]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            (x.span, x.message(), x.severity, x.code) == (y.span, y.message(), y.severity, y.code)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "file:///w/doc.ttl";
    const HEAD: &str = "@prefix ex: <http://example.org/> .\n";

    fn external() -> PrefixMap {
        [("rdfs", "http://www.w3.org/2000/01/rdf-schema#")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn full(text: &str, syntax: Syntax) -> Parsed {
        parse_with_base(text, &external(), syntax, Some(BASE))
    }

    /// Replace `del` bytes at `at` with `ins`: the splice's parse of the new
    /// text, compared with a full parse of it, or why the splice fell back.
    fn edit(text: &str, syntax: Syntax, at: usize, del: usize, ins: &str) -> Result<(), Fallback> {
        let mut new_text = text.to_string();
        new_text.replace_range(at..at + del, ins);
        let change = Change {
            old_text: text,
            new_text: &new_text,
            edit: Edit {
                start: at as u32,
                end: (at + del) as u32,
                new_len: ins.len() as u32,
            },
        };
        let ext = external();
        let old = full(text, syntax);
        let (spliced, window) = splice(old.clone(), change, &ext, &ext, Some(BASE))?;
        if let Err(diff) = crate::oracle::same_parse(&spliced, &full(&new_text, syntax)) {
            panic!("splice differs from a full parse of {new_text:?}: {diff}");
        }
        outside_the_window_as_it_was(&old, &spliced, window);
        Ok(())
    }

    /// The statements before `window` are the old ones; those after it are
    /// the old ones moved; the window's bytes hold the edit and its
    /// statements.
    fn outside_the_window_as_it_was(old: &Parsed, new: &Parsed, window: Window) {
        let (s, b) = (window.statements, window.bytes);
        let (lo, hi) = (s.start as usize, s.end as usize);
        assert_eq!(old.statements[..lo], new.statements[..lo], "{window:?}");
        let after = &new.statements[lo + s.new_len as usize..];
        assert_eq!(after.len(), old.statements.len() - hi, "{window:?}");
        for (o, n) in old.statements[hi..].iter().zip(after) {
            assert_eq!(window.moved(o.span.start), n.span.start, "{window:?}");
        }
        let inside = |st: &[Statement], end: u32| {
            st.iter()
                .all(|x| x.span.start >= b.start && x.span.start < end)
        };
        assert!(inside(&old.statements[lo..hi], b.end), "{window:?}");
        assert!(
            inside(&new.statements[lo..lo + s.new_len as usize], b.new_end()),
            "{window:?}"
        );
    }

    /// The edit replacing the first `find` with `ins`.
    fn replace(text: &str, syntax: Syntax, find: &str, ins: &str) -> Result<(), Fallback> {
        let at = text.find(find).expect("the text to edit");
        edit(text, syntax, at, find.len(), ins)
    }

    fn turtle(body: &str) -> String {
        format!("{HEAD}{body}")
    }

    #[test]
    fn a_literal_edit() {
        let t = turtle("ex:a ex:p \"hello\" .\nex:b ex:p \"world\" .\nex:c ex:p ex:d .\n");
        assert_eq!(replace(&t, Syntax::Turtle, "hello", "help"), Ok(()));
        assert_eq!(replace(&t, Syntax::Turtle, "world", "wor\\qld"), Ok(()));
    }

    #[test]
    fn a_comment_edit() {
        let t = turtle("ex:a ex:p 1 . # one\n# two\nex:b ex:p 2 .\n");
        assert_eq!(replace(&t, Syntax::Turtle, "one", "uno"), Ok(()));
        assert_eq!(replace(&t, Syntax::Turtle, "# two", "two"), Ok(()));
    }

    #[test]
    fn a_directive_edit_falls_back() {
        let t = turtle("ex:a ex:p 1 .\n@prefix ey: <http://e/> .\ney:b ex:p 2 .\n");
        assert_eq!(
            replace(&t, Syntax::Turtle, "ey:", "ez:"),
            Err(Fallback::Directive)
        );
        assert_eq!(
            replace(&t, Syntax::Turtle, "ex:p 1 .", "ex:p 1 .\n@base <x> ."),
            Err(Fallback::Directive)
        );
        // The first statement after the directives: the window reaches back
        // into them.
        assert_eq!(
            replace(&t, Syntax::Turtle, "ex:a", "ex:z"),
            Err(Fallback::Directive)
        );
    }

    #[test]
    fn opening_and_closing_a_long_string() {
        let t = turtle(
            "ex:a ex:p \"x\" .\nex:b ex:p ex:c .\nex:d ex:p \"\"\"y\"\"\" .\nex:e ex:p 2 .\n",
        );
        // Opened: everything to the next `"""` is one string.
        assert_eq!(replace(&t, Syntax::Turtle, "\"x\"", "\"\"\"x"), Ok(()));
        // Closed: the long string ends early.
        assert_eq!(replace(&t, Syntax::Turtle, "y\"\"\"", "y"), Ok(()));
        assert_eq!(replace(&t, Syntax::Turtle, "\"\"\"y", "\"y"), Ok(()));
    }

    #[test]
    fn a_number_gains_a_fraction() {
        let t = turtle("ex:a ex:p 1.\nex:b ex:p 2 .\n");
        let at = t.find("1.").unwrap() + 2;
        assert_eq!(edit(&t, Syntax::Turtle, at, 0, "5"), Ok(()));
        let t = turtle("ex:a ex:p 1.5 .\nex:b ex:p 2 .\n");
        assert_eq!(replace(&t, Syntax::Turtle, "5", ""), Ok(()));
    }

    #[test]
    fn a_prefixed_name_and_its_trailing_dots() {
        let t = turtle("ex:a ex:p ex:b.c .\nex:d ex:p ex:e .\n");
        assert_eq!(replace(&t, Syntax::Turtle, "c .", " ."), Ok(()));
        // A run of dots after a name, then a name character: one name.
        let t = turtle("ex:a ex:p ex:b...\nex:d ex:p ex:e .\n");
        let at = t.find("...").unwrap() + 3;
        assert_eq!(edit(&t, Syntax::Turtle, at, 0, "c"), Ok(()));
        assert_eq!(edit(&t, Syntax::Turtle, at - 1, 1, ""), Ok(()));
    }

    #[test]
    fn carets_and_annotation_braces_read_ahead() {
        let t = turtle("ex:a ex:p \"1\"^ ex:int .\nex:b ex:p 2 .\n");
        assert_eq!(replace(&t, Syntax::Turtle, "^ ", "^^"), Ok(()));
        let t = turtle("ex:a ex:p ex:o { | ex:q 1 |} .\nex:b ex:p 2 .\n");
        assert_eq!(replace(&t, Syntax::Turtle, "{ |", "{|"), Ok(()));
    }

    #[test]
    fn a_broken_statement_swallows_the_next() {
        let t =
            turtle("ex:a ex:p 1 .\nex:b ex:p 2 .\n  ex:c ex:p 3 .\nex:d ex:p 4 .\nex:e ex:p 5 .\n");
        // Without its `.`, the second statement runs into the indented third.
        assert_eq!(replace(&t, Syntax::Turtle, "2 .", "2"), Ok(()));
        assert_eq!(replace(&t, Syntax::Turtle, "ex:p 2", "ex:p ;;"), Ok(()));
        // And back: the old parse had the swallow.
        let broken = t.replace("2 .", "2");
        assert_eq!(
            replace(&broken, Syntax::Turtle, "ex:p 2", "ex:p 2 ."),
            Ok(())
        );
        // A newline before a column-0 subject moves a statement boundary.
        assert_eq!(replace(&t, Syntax::Turtle, "  ex:c", "ex:c"), Ok(()));
    }

    #[test]
    fn blank_nodes_after_an_added_one_are_renumbered() {
        let t = turtle(
            "ex:a ex:p [ ex:q 1 ] .\nex:b ex:p ex:o .\nex:c ex:p [ ex:q 2 ], ( 1 2 ) .\nex:d ex:p << ex:s ex:p ex:o >> .\n",
        );
        assert_eq!(
            replace(&t, Syntax::Turtle, "ex:p ex:o", "ex:p [ ] , ( 3 )"),
            Ok(())
        );
        assert_eq!(replace(&t, Syntax::Turtle, "[ ex:q 2 ]", "ex:x"), Ok(()));
        assert_eq!(
            replace(&t, Syntax::Turtle, "[ ex:q 1 ]", "[ ex:q [ ex:r 1 ] ]"),
            Ok(())
        );
        // An unclosed `[` allocates a node that no triple keeps.
        assert_eq!(
            replace(&t, Syntax::Turtle, "ex:p ex:o", "ex:p [ ex:q"),
            Ok(())
        );
        let open = t.replace("ex:p ex:o", "ex:p [ ex:q");
        assert_eq!(
            replace(&open, Syntax::Turtle, "[ ex:q .\n", "[ ex:q 1 ] .\n"),
            Ok(())
        );
    }

    #[test]
    fn a_blank_node_label_falls_back() {
        let t = turtle("ex:a ex:p _:x .\nex:b ex:p ex:o .\nex:c ex:p _:x .\n");
        assert_eq!(
            replace(&t, Syntax::Turtle, "ex:p ex:o", "ex:p _:y"),
            Err(Fallback::BlankLabel)
        );
        assert_eq!(
            replace(&t, Syntax::Turtle, "ex:c ex:p _:x", "ex:c ex:p ex:x"),
            Err(Fallback::BlankLabel)
        );
        // A label away from the window is fine.
        let t = turtle(
            "ex:a ex:p _:x .\nex:b ex:p 1 .\nex:c ex:p [] .\nex:d ex:p 2 .\nex:e ex:p 3 .\n",
        );
        assert_eq!(replace(&t, Syntax::Turtle, "2", "[ ex:q 2 ]"), Ok(()));
    }

    #[test]
    fn trig_braces_fall_back_and_edits_inside_a_block_splice() {
        let t = format!(
            "{HEAD}ex:g {{\n  ex:a ex:p 1 .\n  ex:b ex:p \"x\" .\n  ex:c ex:p [ ex:q 3 ] .\n  ex:d ex:p 4 .\n}}\nex:h {{ ex:e ex:p 5 . }}\n"
        );
        assert_eq!(replace(&t, Syntax::TriG, "\"x\"", "\"xy\""), Ok(()));
        assert_eq!(replace(&t, Syntax::TriG, "ex:p 4", "ex:p [] , 4"), Ok(()));
        assert_eq!(replace(&t, Syntax::TriG, "\"x\" .", "\"x\""), Ok(()));
        assert_eq!(
            replace(&t, Syntax::TriG, "ex:p 4 .\n}", "ex:p 4 .\n"),
            Err(Fallback::GraphBlock)
        );
        assert_eq!(
            replace(&t, Syntax::TriG, "ex:p 4 .", "ex:p 4 . }"),
            Err(Fallback::GraphBlock)
        );
        assert_eq!(
            replace(&t, Syntax::TriG, "ex:h {", "GRAPH ex:h {"),
            Err(Fallback::GraphBlock)
        );
        // A block named by a blank node, allocated before the window.
        let t = format!("{HEAD}[] {{ ( ex:x ) ex:p ex:o }}\nex:g {{ ex:a ex:p 1 }}\n");
        assert_eq!(replace(&t, Syntax::TriG, "( e", "<"), Ok(()));
        assert_eq!(
            replace(&t, Syntax::TriG, "ex:p ex:o", "ex:p [ ] , ex:o"),
            Ok(())
        );
        // The first statement on its own line sets the indentation recovery
        // goes by: a window that changes it falls back, unless it ends at
        // the block's `}`.
        let t = format!(
            "{HEAD}ex:g {{ex:a ex:p ex:o .ex:b ex:p 1 .\nex:c ex:p 2 .\nex:d ex:p 3 .\nex:e ex:p 4 .}}\n"
        );
        assert_eq!(
            replace(&t, Syntax::TriG, ".ex:b", ".\nex:b"),
            Err(Fallback::Indent)
        );
        assert_eq!(
            replace(&t, Syntax::TriG, "3 .\nex:e", "3 .\n  ex:e"),
            Ok(())
        );
        let t =
            format!("{HEAD}ex:g {{ex:a ex:p ex:o .ex:b ex:p 1, true}}\nex:h {{ ex:c ex:p 2 }}\n");
        assert_eq!(replace(&t, Syntax::TriG, " .ex:b", ".\nex:b"), Ok(()));
        // A window with no statement, that ends where a block starts.
        let t = "# a comment _:x\ng:a {ex:b ex:c \"d\"}\n";
        assert_eq!(replace(t, Syntax::TriG, ":x", ";"), Ok(()));
        // A statement between blocks.
        let t = format!(
            "{HEAD}ex:g {{ ex:a ex:p 1 . }}\nex:b ex:p 2 .\nex:c ex:p 3 .\nex:h {{ ex:d ex:p 4 . }}\n"
        );
        assert_eq!(replace(&t, Syntax::TriG, "3", "33"), Ok(()));
    }

    #[test]
    fn an_n_quads_line_edit() {
        let t = "<http://e/a> <http://e/p> \"1\" <http://e/g> .\n<http://e/b> <http://e/p> \"2\" .\n<http://e/c> <http://e/p> <http://e/d> <http://e/g> .\n<http://e/e> <http://e/p> \"3\" .\n";
        assert_eq!(replace(t, Syntax::NQuads, "\"2\"", "\"22\"@en"), Ok(()));
        assert_eq!(replace(t, Syntax::NQuads, "<http://e/d>", "ex:d"), Ok(()));
        // Two lines become one: the restriction moves to the next statement.
        assert_eq!(replace(t, Syntax::NQuads, "\"2\" .\n", "\"2\" . "), Ok(()));
        assert_eq!(
            replace(
                t,
                Syntax::NQuads,
                "<http://e/g> .\n<http://e/b>",
                "<http://e/g> <http://e/b>"
            ),
            Ok(())
        );
        let nt = "<http://e/a> <http://e/p> \"1\" .\n<http://e/b> <http://e/p> 'x' .\n<http://e/c> <http://e/p> \"3\" .\n";
        assert_eq!(replace(nt, Syntax::NTriples, "'x'", "\"x\""), Ok(()));
        assert_eq!(
            replace(nt, Syntax::NTriples, "\"1\" .\n", "\"1\"\n"),
            Ok(())
        );
    }

    #[test]
    fn edits_at_the_very_start_and_the_very_end() {
        let t = "ex:a ex:p 1 .\nex:b ex:p 2 .\nex:c ex:p 3 .";
        assert_eq!(edit(t, Syntax::Turtle, 0, 0, "ex:z ex:p 0 .\n"), Ok(()));
        assert_eq!(edit(t, Syntax::Turtle, 0, 1, ""), Ok(()));
        assert_eq!(edit(t, Syntax::Turtle, 0, 0, "  "), Ok(()));
        // The first statement, broken, and shifted by the edit.
        assert_eq!(
            edit(
                "@p ex:a ex:b .\nex:c ex:d ex:e .",
                Syntax::Turtle,
                0,
                0,
                "\r"
            ),
            Ok(())
        );
        assert_eq!(
            edit(t, Syntax::Turtle, t.len(), 0, "\nex:d ex:p 4 ."),
            Ok(())
        );
        assert_eq!(edit(t, Syntax::Turtle, t.len() - 1, 1, ""), Ok(()));
        assert_eq!(edit(t, Syntax::Turtle, t.len(), 0, " # end"), Ok(()));
    }

    #[test]
    fn deleting_everything() {
        let t = "ex:a ex:p 1 .\nex:b ex:p 2 .\n";
        assert_eq!(edit(t, Syntax::Turtle, 0, t.len(), ""), Ok(()));
        let t = turtle("ex:a ex:p 1 .\n");
        assert_eq!(
            edit(&t, Syntax::Turtle, 0, t.len(), ""),
            Err(Fallback::Directive)
        );
    }

    #[test]
    fn what_always_falls_back() {
        let ext = external();
        let parsed = || full("ex:a ex:p 1 .", Syntax::Turtle);
        let change = Change {
            old_text: "ex:a ex:p 1 .",
            new_text: "ex:a ex:p 2 .",
            edit: Edit {
                start: 10,
                end: 11,
                new_len: 1,
            },
        };
        assert_eq!(
            splice(parsed(), change, &ext, &PrefixMap::default(), Some(BASE)).err(),
            Some(Fallback::Context)
        );
        assert_eq!(
            splice(parsed(), change, &ext, &ext, None).err(),
            Some(Fallback::Context)
        );
        let wrong = Change {
            edit: Edit {
                start: 10,
                end: 11,
                new_len: 2,
            },
            ..change
        };
        assert_eq!(
            splice(parsed(), wrong, &ext, &ext, Some(BASE)).err(),
            Some(Fallback::Edit)
        );
        let empty = Change {
            old_text: "# c",
            new_text: "# cd",
            edit: Edit {
                start: 3,
                end: 3,
                new_len: 1,
            },
        };
        let none = full("# c", Syntax::Turtle);
        assert_eq!(
            splice(none, empty, &ext, &ext, Some(BASE)).err(),
            Some(Fallback::NoStatements)
        );
        let mut kept_none = parsed();
        kept_none.tokens.clear();
        assert_eq!(
            splice(kept_none, change, &ext, &ext, Some(BASE)).err(),
            Some(Fallback::NoTokens)
        );
        let jsonld = Parsed {
            syntax: Syntax::JsonLd,
            document_base: Some(BASE.into()),
            ..Parsed::default()
        };
        assert_eq!(
            splice(jsonld, change, &ext, &ext, Some(BASE)).err(),
            Some(Fallback::Syntax)
        );
    }

    #[test]
    fn a_base_before_the_window_resolves_it() {
        let t = "@base <http://e/dir/> .\n<a> <p> <b> .\n<c> <p> <d> .\n<e> <p> <f> .\n";
        assert_eq!(replace(t, Syntax::Turtle, "<d>", "<dd>"), Ok(()));
        let twice =
            "@base <http://e/dir/> .\n<a> <p> <b> .\n@base <x/> .\n<c> <p> <d> .\n<e> <p> <f> .\n";
        assert_eq!(
            replace(twice, Syntax::Turtle, "<f>", "<ff>"),
            Err(Fallback::Base)
        );
    }

    #[test]
    fn deep_nesting_falls_back() {
        let deep = format!(
            "ex:a ex:p {}1{} .\nex:b ex:p 2 .\n",
            "( ".repeat(300),
            " )".repeat(300)
        );
        assert_eq!(
            replace(&deep, Syntax::Turtle, "1", "2"),
            Err(Fallback::DeepNesting)
        );
    }

    /// A text of `n` statements, about 60 bytes each.
    fn statements(n: usize) -> String {
        let mut t = String::from(HEAD);
        for i in 0..n {
            t.push_str(&format!(
                "ex:s{i} a ex:C ; rdfs:label \"thing {i}\"@en ; ex:p [ ex:q {i} ] .\n"
            ));
        }
        t
    }

    #[test]
    fn an_edit_in_a_big_text_lexes_and_parses_a_few_statements() {
        let t = statements(20_000);
        let at = t.find("thing 10000").unwrap() + 3;
        let before = super::super::lexer::LEXED.with(|n| n.get());
        let mut new_text = t.clone();
        new_text.insert(at, 'x');
        let change = Change {
            old_text: &t,
            new_text: &new_text,
            edit: Edit {
                start: at as u32,
                end: at as u32,
                new_len: 1,
            },
        };
        let ext = external();
        let old = full(&t, Syntax::Turtle);
        let lexed_before = super::super::lexer::LEXED.with(|n| n.get());
        let (spliced, _) = splice(old, change, &ext, &ext, Some(BASE)).expect("a splice");
        let lexed = super::super::lexer::LEXED.with(|n| n.get()) - lexed_before;
        assert!(
            lexed_before - before == t.len(),
            "the full parse lexes once"
        );
        assert!(lexed < 2_000, "lexed {lexed} bytes for a one-byte edit");
        let want = full(&new_text, Syntax::Turtle);
        assert_eq!(crate::oracle::same_parse(&spliced, &want), Ok(()));
        // Too big a paste: as cheap to parse in full.
        let paste = statements(9_000);
        assert_eq!(
            edit(&t, Syntax::Turtle, at, 0, &paste[HEAD.len()..]),
            Err(Fallback::WindowTooLarge)
        );
    }

    /// The fixtures the chaos tests read, with their syntax, and two
    /// generated texts past the smallest window.
    fn fixtures() -> Vec<(String, Syntax)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut dirs: Vec<_> = [
            "workspace",
            "project",
            "project/core",
            "project/data",
            "project/shapes",
            "format",
            "datasets",
            "oracle",
            "shacl",
            "query",
            "chaos",
        ]
        .into_iter()
        .map(|d| root.join("test/fixtures").join(d))
        .collect();
        let w3c = root.join("tests/w3c/shacl/sparql");
        dirs.extend(
            std::fs::read_dir(&w3c)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path()),
        );
        dirs.push(w3c);
        let mut paths: Vec<_> = dirs
            .iter()
            .flat_map(|d| {
                std::fs::read_dir(d)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
            })
            .collect();
        paths.sort();
        let mut out: Vec<(String, Syntax)> = paths
            .iter()
            .filter_map(
                |path| match Syntax::from_extension(&path.to_string_lossy()) {
                    Some(Syntax::JsonLd) | None => None,
                    Some(syntax) => Some((std::fs::read_to_string(path).ok()?, syntax)),
                },
            )
            .collect();
        assert!(out.len() > 50, "the fixtures");
        out.push((statements(300), Syntax::Turtle));
        let blocks: String = (0..60)
            .map(|i| format!("ex:g{i} {{\n  ex:a{i} ex:p [ ex:q {i} ] .\n  ex:b{i} ex:p ( {i} ) ; ex:r \"x{i}\" .\n}}\n"))
            .collect();
        out.push((format!("{HEAD}{blocks}"), Syntax::TriG));
        let quads: String = (0..120)
            .map(|i| match i % 3 {
                0 => format!("<http://e/s{i}> <http://e/p> \"v{i}\" <http://e/g> .\n"),
                1 => format!("<http://e/s{i}> <http://e/p> \"w\"@en .\n"),
                _ => format!("<http://e/s{i}> <http://e/p> <http://e/o{i}> .\n"),
            })
            .collect();
        out.push((quads.clone(), Syntax::NQuads));
        out.push((quads.replace(" <http://e/g> .", " ."), Syntax::NTriples));
        out
    }

    /// xorshift: the same edits on every run.
    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    fn boundary(s: &str, mut i: usize) -> usize {
        while !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    #[test]
    fn random_edits_on_the_fixtures_equal_a_full_parse() {
        const SNIPPETS: &[&str] = &[
            "\"\"\"",
            "\"",
            "'",
            "#",
            "<",
            ">",
            "@prefix ex: <http://e/> .",
            "_:b",
            "{",
            "}",
            ".",
            "5",
            "\n",
            "\nex:z ",
            "[ ]",
            "[",
            "]",
            "(",
            ")",
            ";",
            ",",
            "^^",
            "{|",
            "|}",
            "<<",
            ">>",
            " ",
            "a",
            "x",
            "é",
            "\r",
            "ex:",
            ":",
        ];
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut spliced, mut fell) = (0, std::collections::BTreeMap::new());
        for (text, syntax) in fixtures() {
            if text.is_empty() {
                continue;
            }
            for _ in 0..100 {
                let at = boundary(&text, rng.below(text.len() + 1));
                let del = boundary(&text[at..], rng.below(4).min(text.len() - at));
                let ins = if rng.below(3) == 0 {
                    ""
                } else {
                    SNIPPETS[rng.below(SNIPPETS.len())]
                };
                match edit(&text, syntax, at, del, ins) {
                    Ok(()) => spliced += 1,
                    Err(f) => *fell.entry(format!("{f:?}")).or_insert(0) += 1,
                }
            }
        }
        let fallbacks: usize = fell.values().sum();
        assert!(spliced > fallbacks, "spliced {spliced}, fell back {fell:?}");
    }

    #[test]
    fn one_character_edits_inside_literals_splice() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        let (mut spliced, mut fell) = (0, std::collections::BTreeMap::new());
        for (text, syntax) in fixtures() {
            let parsed = full(&text, syntax);
            let inside: Vec<usize> = parsed
                .tokens
                .iter()
                .filter(|t| t.kind == Kind::String && t.end - t.start >= 3)
                .map(|t| (t.start + 1 + rng.below((t.end - t.start - 2) as usize) as u32) as usize)
                .filter(|&at| text.is_char_boundary(at))
                .collect();
            for at in inside.into_iter().take(30) {
                let (del, ins) = match rng.below(3) {
                    0 => (0, "y"),
                    1 => (boundary(&text[at..], 1), ""),
                    _ => (boundary(&text[at..], 1), "z"),
                };
                // Not a quote or a backslash: those change the token.
                if text[at..at + del].contains(['"', '\'', '\\']) {
                    continue;
                }
                match edit(&text, syntax, at, del, ins) {
                    Ok(()) => spliced += 1,
                    Err(f) => *fell.entry(format!("{f:?}")).or_insert(0) += 1,
                }
            }
        }
        let fallbacks: usize = fell.values().sum();
        assert!(spliced > 200, "spliced {spliced}");
        assert!(
            fallbacks * 10 < spliced,
            "spliced {spliced}, fell back {fell:?}"
        );
    }
}
