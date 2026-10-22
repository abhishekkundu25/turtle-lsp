//! The printer both formatters write through: lines, indentation, spaces,
//! tokens with their comments, and the layout of triple terms
//! ([`Term`], [`Obj`], [`Po`]). Each formatter walks its own syntax and
//! calls it; [`Style`] holds the two places where their triple layouts
//! differ.

use super::Options;
use super::doc::{Doc, Ix, LayoutToken};
use super::terms::{Obj, Po, Term, objects_width};

/// Whether a blank line goes before a line that starts an item (a statement,
/// a pattern, a clause, a declaration).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Blank {
    /// Never (the first item of a document or group).
    No,
    /// When the author wrote one.
    Author,
    /// Always.
    Forced,
}

/// Where the contents of a broken `[ ]`, `{| |}`, `( )` or object list
/// indent from.
#[derive(Clone, Copy)]
pub(crate) enum Groups {
    /// One level in from the level the term is laid out at, the closing
    /// bracket at that level (Turtle family).
    FromTerm,
    /// One level in from the line the group opens on, the closing bracket at
    /// that line's level (SPARQL).
    FromLine,
}

/// Which `;` after a predicate's objects are written.
#[derive(Clone, Copy)]
pub(crate) enum Semicolons {
    /// The first one between two pairs. Any other (after the last pair, or
    /// the second of a run) only when a comment hangs on it, since moving
    /// that comment elsewhere would not survive a second pass (Turtle
    /// family).
    Needed,
    /// Every one, as written (SPARQL).
    AsWritten,
}

/// How a language lays out triples.
#[derive(Clone, Copy)]
pub(crate) struct Style {
    pub(crate) groups: Groups,
    pub(crate) semicolons: Semicolons,
}

/// Writes laid-out text for the token document `d`.
///
/// A trailing comment waits in `pending` and is written at the end of the
/// line its token lands on; while one waits, `must_break` makes the next
/// space a line break. At most one waits: [`Self::tok`] ends the line before
/// a token when one waits, and [`Self::punct`], which glues `;`, `,` or `.`
/// to the line, ends it only when the punctuation brings a second trailing
/// comment. So a trailing comment can move past a punctuation token, and two
/// never share a line.
pub(crate) struct Printer<'a, T> {
    d: &'a Doc<'a, T>,
    style: Style,
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
    /// A trailing comment waits: the next space becomes a line break.
    must_break: bool,
    /// The first token of the current item, whose comments are already out.
    skip_lead: Option<Ix>,
}

impl<'a, T: LayoutToken> Printer<'a, T> {
    /// Lines end as the first line of the source does (`\r\n` or `\n`).
    pub(crate) fn new(d: &'a Doc<'a, T>, o: &Options, style: Style) -> Printer<'a, T> {
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
            style,
            out: String::with_capacity(d.src.len() + d.src.len() / 8),
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

    /// Indentation level of the current line.
    pub(crate) fn level(&self) -> usize {
        self.level
    }

    /// `w` more columns fit on the current line.
    pub(crate) fn room_for(&self, w: usize) -> bool {
        self.col.saturating_add(w) <= self.width
    }

    /// Fits on this line with room for a ` .` or ` ;` after it.
    fn fits(&self, w: usize) -> bool {
        self.col.saturating_add(w).saturating_add(2) <= self.width
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
    pub(crate) fn line(&mut self, level: usize) {
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

    /// A space, or a line break at `level` when a trailing comment waits.
    pub(crate) fn space(&mut self, level: usize) {
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
            debug_assert!(self.pending.is_none(), "two trailing comments on one line");
            self.pending = Some(c);
            self.must_break = true;
        }
    }

    /// A token, on a new line at `level` if it cannot stay on this one.
    pub(crate) fn tok(&mut self, i: Ix, level: usize) {
        self.leading(i, level);
        if self.must_break {
            self.line(level);
        }
        self.write(self.d.shown(i));
        self.after(i);
    }

    /// ` ;`, ` ,` or ` .` glued to what precedes it.
    pub(crate) fn punct(&mut self, i: Ix, level: usize) {
        self.leading(i, level);
        // Two trailing comments cannot share a line.
        if self.pending.is_some() && self.d.trail[i as usize].is_some() {
            self.line(level);
        }
        if self.has_content {
            self.out.push(' ');
            self.col += 1;
        }
        self.write(self.d.text(i));
        self.after(i);
    }

    /// After a prefix name: spaces up to column `align` (at least one), or a
    /// line break at `level` when a trailing comment waits.
    pub(crate) fn pad_to(&mut self, align: usize, level: usize) {
        if self.must_break {
            self.line(level);
        } else {
            let pad = align.saturating_sub(self.col).max(1);
            for _ in 0..pad {
                self.out.push(' ');
            }
            self.col += pad;
        }
    }

    // ---- items ----

    /// Starts the line of an item whose first token is `i` at `level`, with
    /// its own-line comments above it. Blank lines between those comments
    /// and between them and the item are kept as the author wrote them.
    pub(crate) fn start(&mut self, level: usize, i: Ix, blank: Blank) {
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

    /// The comments before a group's closing `close` (inside it, at
    /// `base + 1`), then `close` on its own line at `base`. `empty`: nothing
    /// was laid out inside the group.
    pub(crate) fn close_group(&mut self, close: Ix, base: usize, empty: bool) {
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

    /// The comments after the last token, then the text, ending in exactly
    /// one line break (or empty).
    pub(crate) fn finish(mut self) -> String {
        let n = self.d.toks.len() as Ix;
        let (a, b) = self.d.leading(n);
        for c in a..b {
            if c > a || n > 0 {
                self.top_line(0, self.d.comments[c as usize].blank_before);
            }
            self.write(self.d.comment(c));
        }
        self.line(0);
        self.out.truncate(self.line_start);
        self.out
    }

    // ---- triple terms ----

    /// The level the contents of a broken group at `level` indent from,
    /// read just after its opening bracket.
    fn base(&self, level: usize) -> usize {
        match self.style.groups {
            Groups::FromTerm => level,
            Groups::FromLine => self.level,
        }
    }

    /// A group that is not forced flat goes on one line when nothing
    /// nested or no comment is in it and it fits.
    fn inline_group(&self, t: &Term, open: Ix, close: Ix) -> bool {
        !self.d.comments_inside(open, close) && t.flat() && self.fits(t.width(self.d))
    }

    /// Term `t` at `level`; `flat` keeps a group on one line.
    pub(crate) fn term(&mut self, t: &Term, level: usize, flat: bool) {
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
                let inline = flat || self.inline_group(t, *open, *close);
                self.tok(*open, level);
                if inline {
                    self.space(level + 1);
                    self.pol_from(pol, level + 1, true, false);
                    self.space(level);
                    self.tok(*close, level);
                } else {
                    let base = self.base(level);
                    self.pol_from(pol, base + 1, false, false);
                    self.line(base);
                    self.tok(*close, base);
                }
            }
            Term::Coll { open, items, close } => {
                let inline = flat || self.inline_group(t, *open, *close);
                self.tok(*open, level);
                let base = self.base(level);
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
    /// by ` ; `.
    pub(crate) fn pol_from(&mut self, pol: &[Po], level: usize, flat: bool, first_inline: bool) {
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
            for (m, &s) in po.semis.iter().enumerate() {
                let needed = m == 0 && !last;
                match self.style.semicolons {
                    Semicolons::Needed if !needed => self.extra_semi(s, level),
                    _ => self.punct(s, level),
                }
            }
        }
    }

    /// A redundant `;`: left out, unless a comment hangs on it.
    fn extra_semi(&mut self, i: Ix, level: usize) {
        let (a, b) = self.d.leading(i);
        if a < b || self.d.trail[i as usize].is_some() {
            self.punct(i, level);
        }
    }

    fn objects(&mut self, objs: &[Obj], level: usize, flat: bool) {
        // A single object lays itself out; several go on one line only when
        // each of them is flat and they fit there together.
        let inline = flat
            || (objs.len() > 1
                && !self
                    .d
                    .comments_inside(objs[0].term.first(), objs[objs.len() - 1].last())
                && objs.iter().all(Obj::flat)
                && self.fits(objects_width(objs, self.d)));
        // Broken: groups follow each other as `] , [`; anything else goes on
        // its own line one level in.
        let base = self.base(level);
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

    const STYLE: Style = Style {
        groups: Groups::FromTerm,
        semicolons: Semicolons::Needed,
    };

    /// Lays out the tokens of `src` (`<a> , <b>` and the like) with `steps`:
    /// `t` a token, `p` a punctuation token, ` ` a space at level 1.
    fn print<T: LayoutToken>(src: &str, all: &[T], steps: &str) -> String {
        let d = Doc::new(src, all);
        let mut p = Printer::new(&d, &Options::default(), STYLE);
        let mut i = 0;
        for s in steps.chars() {
            match s {
                't' => p.tok(i, 1),
                'p' => p.punct(i, 1),
                _ => {
                    p.space(1);
                    continue;
                }
            }
            i += 1;
        }
        p.finish()
    }

    /// Through the Turtle-family tokens and through the SPARQL tokens.
    fn both(src: &str, steps: &str) -> [String; 2] {
        [
            print(src, &crate::syntax::lexer::tokenize(src), steps),
            print(src, &crate::sparql::lexer::tokenize(src), steps),
        ]
    }

    #[test]
    fn a_second_trailing_comment_starts_a_new_line() {
        // On a punctuation token: the line ends before it.
        let src = "<a> # one\n, # two\n<b> # three\n";
        let want = "<a> # one\n    , # two\n    <b> # three\n";
        assert_eq!(both(src, "tp t"), [want, want]);
        // On a token after a space.
        let src = "<a> # one\n<b> # two\n";
        let want = "<a> # one\n    <b> # two\n";
        assert_eq!(both(src, "t t"), [want, want]);
    }

    #[test]
    fn a_trailing_comment_moves_past_punctuation() {
        let src = "<a> # one\n, <b>\n";
        let want = "<a> , # one\n    <b>\n";
        assert_eq!(both(src, "tp t"), [want, want]);
    }

    #[test]
    fn own_line_comments_before_a_token_and_at_the_end() {
        let src = "<a>\n# above\n<b>\n\n# end\n";
        let want = "<a>\n    # above\n    <b>\n\n# end\n";
        assert_eq!(both(src, "tt"), [want, want]);
    }

    #[test]
    fn padding_or_a_break_after_a_prefix_name() {
        let src = "@prefix ex: <https://e.org/> .";
        let d = Doc::new(src, &crate::syntax::lexer::tokenize(src));
        let mut p = Printer::new(&d, &Options::default(), STYLE);
        p.tok(0, 0);
        p.space(1);
        p.tok(1, 1);
        p.pad_to(16, 1);
        p.tok(2, 1);
        p.punct(3, 0);
        assert_eq!(p.finish(), "@prefix ex:     <https://e.org/> .\n");
        let src = "PREFIX ex: # c\n<https://e.org/>";
        let d = Doc::new(src, &crate::sparql::lexer::tokenize(src));
        let mut p = Printer::new(&d, &Options::default(), STYLE);
        p.tok(0, 0);
        p.space(1);
        p.tok(1, 1);
        p.pad_to(16, 1);
        p.tok(2, 1);
        assert_eq!(p.finish(), "PREFIX ex: # c\n    <https://e.org/>\n");
    }
}
