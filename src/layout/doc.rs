//! The token document: a token stream split into the tokens a formatter lays
//! out and the comments, each comment attached to a token.
//!
//! A comment on the same line as the token before it trails that token (one
//! per token); every other comment is an own-line comment and leads the next
//! token, or the end of the file. Each comment and token also records whether
//! a blank line separates it from what comes before. [`Doc::attach`] does
//! this again as if some tokens were not there, for tokens a formatter leaves
//! out.
//!
//! Generic over [`LayoutToken`], with adapters for the Turtle-family tokens
//! (`syntax::lexer::Token`) and the SPARQL tokens (`sparql::Token`).

use crate::sparql::{Token as SparqlToken, TokenKind, lexer as sparql_lexer};
use crate::syntax::Span;
use crate::syntax::lexer::{Kind, Token as RdfToken};

/// The index of a token in [`Doc::toks`].
pub(crate) type Ix = u32;

/// Width of something that cannot go on one line.
pub(crate) const BIG: usize = 1 << 24;

/// What the token document needs from a token.
pub(crate) trait LayoutToken: Copy {
    /// The characters that end a line in the whitespace between two tokens.
    /// The Turtle-family formatter counts only `\n`, the SPARQL one also a
    /// lone `\r`.
    const LINE_BREAKS: &'static [char];

    /// The token's byte range in the source.
    fn span(&self) -> Span;

    fn is_comment(&self) -> bool;

    /// The token's text as the formatter writes it: the source text, apart
    /// from SPARQL keywords (upper case), built-in function names (their
    /// grammar spelling) and comments (trailing whitespace left out).
    fn shown<'s>(&self, src: &'s str) -> &'s str;

    /// The width of token text `text` on its line, in characters. The
    /// Turtle-family formatter makes a multi-line token (a long string)
    /// [`BIG`], so nothing shares a line with it; the SPARQL one counts its
    /// first line.
    fn width(text: &str) -> usize;
}

impl LayoutToken for RdfToken {
    const LINE_BREAKS: &'static [char] = &['\n'];

    fn span(&self) -> Span {
        Span::new(self.start, self.end)
    }

    fn is_comment(&self) -> bool {
        self.kind == Kind::Comment
    }

    fn shown<'s>(&self, src: &'s str) -> &'s str {
        self.text(src)
    }

    fn width(text: &str) -> usize {
        if text.contains(Self::LINE_BREAKS) {
            BIG
        } else {
            text.chars().count()
        }
    }
}

impl LayoutToken for SparqlToken {
    const LINE_BREAKS: &'static [char] = &['\n', '\r'];

    fn span(&self) -> Span {
        self.span
    }

    fn is_comment(&self) -> bool {
        self.kind == TokenKind::Comment
    }

    fn shown<'s>(&self, src: &'s str) -> &'s str {
        let s = self.text(src);
        match self.kind {
            TokenKind::Keyword => sparql_lexer::keyword(s).unwrap_or(s),
            TokenKind::Function => sparql_lexer::function(s).unwrap_or(s),
            TokenKind::Comment => s.trim_end(),
            _ => s,
        }
    }

    fn width(text: &str) -> usize {
        text.split(Self::LINE_BREAKS)
            .next()
            .unwrap_or("")
            .chars()
            .count()
    }
}

pub(crate) struct Comment {
    start: u32,
    /// The end of the comment text, trailing whitespace left out.
    end: u32,
    /// A blank line separates it from what comes before.
    pub(crate) blank_before: bool,
}

pub(crate) struct Doc<'a, T> {
    pub(crate) src: &'a str,
    /// Non-comment tokens.
    pub(crate) toks: Vec<T>,
    pub(crate) comments: Vec<Comment>,
    /// Own-line comments above token `i` (index `toks.len()` = end of file),
    /// as a range into `comments`.
    lead: Vec<(u32, u32)>,
    /// The comment after token `i` on the same line.
    pub(crate) trail: Vec<Option<u32>>,
    /// A blank line separates token `i` from what comes before.
    pub(crate) blank_before: Vec<bool>,
    /// Prefix counts of tokens with leading / trailing comments.
    lead_pref: Vec<u32>,
    trail_pref: Vec<u32>,
}

/// Two newlines with only whitespace between them: `gap` is the whitespace
/// between two tokens.
fn has_blank(gap: &str) -> bool {
    gap.split('\n').count() >= 3
}

impl<'a, T: LayoutToken> Doc<'a, T> {
    /// `all` is the whole token stream of `src`, comments included.
    pub(crate) fn new(src: &'a str, all: &[T]) -> Doc<'a, T> {
        let n = all.iter().filter(|t| !t.is_comment()).count();
        let mut d = Doc {
            src,
            toks: Vec::with_capacity(n),
            comments: Vec::new(),
            lead: Vec::with_capacity(n + 1),
            trail: Vec::with_capacity(n),
            blank_before: Vec::with_capacity(n),
            lead_pref: Vec::with_capacity(n + 2),
            trail_pref: Vec::with_capacity(n + 1),
        };
        for t in all {
            let span = t.span();
            if t.is_comment() {
                let text = src[span.start as usize..span.end as usize].trim_end();
                d.comments.push(Comment {
                    start: span.start,
                    end: span.start + text.len() as u32,
                    blank_before: false,
                });
            } else {
                d.toks.push(*t);
            }
        }
        d.attach(&[]);
        d
    }

    /// Attaches the comments again as if the tokens `omit` (ascending) were
    /// not there, for tokens the formatter leaves out. An omitted token gets
    /// no comment, and a comment on its line trails the token before it, if
    /// that token has none yet; one on a line of its own leads the token
    /// after it. A blank line counts only in whitespace, never across an
    /// omitted token.
    pub(crate) fn attach(&mut self, omit: &[Ix]) {
        debug_assert!(omit.is_sorted(), "omitted tokens out of order");
        let n = self.toks.len();
        self.lead.clear();
        self.lead.resize(n + 1, (0, 0));
        self.trail.clear();
        self.trail.resize(n, None);
        self.blank_before.clear();
        self.blank_before.resize(n, false);
        let mut omit = omit.iter().copied().peekable();
        // The end of whatever comes just before, omitted or not.
        let mut prev_end = 0usize;
        let mut seen = false;
        // A blank line before an omitted token since the last thing kept.
        let mut blank_skipped = false;
        let mut last_kept: Option<usize> = None;
        // Comments waiting for the next token start here.
        let mut lead_from = 0u32;
        let (mut i, mut c) = (0usize, 0usize);
        while i < n || c < self.comments.len() {
            let comment = c < self.comments.len()
                && (i == n || self.comments[c].start < self.toks[i].span().start);
            let (start, end) = if comment {
                (self.comments[c].start, self.comments[c].end)
            } else {
                let s = self.toks[i].span();
                (s.start, s.end)
            };
            let gap = &self.src[prev_end..start as usize];
            let blank = seen && (blank_skipped || has_blank(gap));
            if comment {
                let idx = c as u32;
                self.comments[c].blank_before = blank;
                if let Some(last) = last_kept
                    && !gap.contains(T::LINE_BREAKS)
                    && self.trail[last].is_none()
                    && lead_from == idx
                {
                    self.trail[last] = Some(idx);
                    lead_from = idx + 1;
                }
                blank_skipped = false;
                c += 1;
            } else if omit.next_if_eq(&(i as Ix)).is_some() {
                self.lead[i] = (lead_from, lead_from);
                blank_skipped = blank;
                i += 1;
            } else {
                self.lead[i] = (lead_from, c as u32);
                lead_from = c as u32;
                self.blank_before[i] = blank;
                last_kept = Some(i);
                blank_skipped = false;
                i += 1;
            }
            prev_end = end as usize;
            seen = true;
        }
        self.lead[n] = (lead_from, self.comments.len() as u32);
        self.lead_pref.clear();
        let mut acc = 0;
        self.lead_pref.push(0);
        for r in &self.lead {
            acc += u32::from(r.0 < r.1);
            self.lead_pref.push(acc);
        }
        self.trail_pref.clear();
        acc = 0;
        self.trail_pref.push(0);
        for t in &self.trail {
            acc += u32::from(t.is_some());
            self.trail_pref.push(acc);
        }
    }

    /// The text of token `i`.
    pub(crate) fn text(&self, i: Ix) -> &'a str {
        let s = self.toks[i as usize].span();
        &self.src[s.start as usize..s.end as usize]
    }

    /// Token `i` as the formatter writes it ([`LayoutToken::shown`]).
    pub(crate) fn shown(&self, i: Ix) -> &'a str {
        self.toks[i as usize].shown(self.src)
    }

    /// The width of token `i` on its line ([`LayoutToken::width`]).
    pub(crate) fn width(&self, i: Ix) -> usize {
        T::width(self.text(i))
    }

    /// The text of comment `c`, trailing whitespace left out.
    pub(crate) fn comment(&self, c: u32) -> &'a str {
        let c = &self.comments[c as usize];
        &self.src[c.start as usize..c.end as usize]
    }

    /// Any comment that would end up inside the tokens `a..=b`.
    pub(crate) fn comments_inside(&self, a: Ix, b: Ix) -> bool {
        let (a, b) = (a as usize, b as usize);
        self.lead_pref[b + 1] > self.lead_pref[a + 1] || self.trail_pref[b] > self.trail_pref[a]
    }

    /// The own-line comments above token `i` (`toks.len()` = end of file), as
    /// a range into `comments`.
    pub(crate) fn leading(&self, i: Ix) -> (u32, u32) {
        self.lead[i as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the document attached, per token: the texts of its leading
    /// comments, its trailing comment and its blank-line flag; then the
    /// comments left for the end of the file.
    #[derive(Debug, PartialEq)]
    struct Shape {
        toks: Vec<(Vec<String>, Option<String>, bool)>,
        end: Vec<String>,
        comment_blanks: Vec<bool>,
    }

    fn shape<T: LayoutToken>(d: &Doc<T>) -> Shape {
        let lead = |i: usize| {
            let (a, b) = d.leading(i as Ix);
            (a..b).map(|c| d.comment(c).to_string()).collect::<Vec<_>>()
        };
        Shape {
            toks: (0..d.toks.len())
                .map(|i| {
                    (
                        lead(i),
                        d.trail[i].map(|c| d.comment(c).to_string()),
                        d.blank_before[i],
                    )
                })
                .collect(),
            end: lead(d.toks.len()),
            comment_blanks: d.comments.iter().map(|c| c.blank_before).collect(),
        }
    }

    /// The shape through the Turtle-family tokens and through the SPARQL
    /// tokens.
    fn both(src: &str) -> (Shape, Shape) {
        (
            shape(&Doc::new(src, &crate::syntax::lexer::tokenize(src))),
            shape(&Doc::new(src, &crate::sparql::lexer::tokenize(src))),
        )
    }

    fn tok(lead: &[&str], trail: Option<&str>, blank: bool) -> (Vec<String>, Option<String>, bool) {
        (
            lead.iter().map(|s| s.to_string()).collect(),
            trail.map(str::to_string),
            blank,
        )
    }

    #[test]
    fn own_line_and_trailing_comments() {
        let src = "<a> <b> <c> . # after the dot  \n# above d\n<d> <e> <f> .";
        let want = Shape {
            toks: vec![
                tok(&[], None, false),
                tok(&[], None, false),
                tok(&[], None, false),
                tok(&[], Some("# after the dot"), false),
                tok(&["# above d"], None, false),
                tok(&[], None, false),
                tok(&[], None, false),
                tok(&[], None, false),
            ],
            end: vec![],
            comment_blanks: vec![false, false],
        };
        let (rdf, sparql) = both(src);
        assert_eq!(rdf, want);
        assert_eq!(sparql, want);
    }

    #[test]
    fn a_comment_before_the_first_token_leads_it() {
        let (rdf, sparql) = both("# first\n<a> <b> <c> .");
        for s in [rdf, sparql] {
            assert_eq!(s.toks[0], tok(&["# first"], None, false));
            assert_eq!(s.comment_blanks, [false]);
        }
    }

    #[test]
    fn blank_lines_before_a_comment_and_before_a_token() {
        let src = "<a> <b> <c> .\n\n# one\n# two\n\n<d> <e> <f> .";
        let (rdf, sparql) = both(src);
        for s in [&rdf, &sparql] {
            assert_eq!(s.toks[4], tok(&["# one", "# two"], None, true));
            assert_eq!(s.comment_blanks, [true, false]);
            assert!(s.toks[..4].iter().all(|t| !t.2));
        }
        assert_eq!(rdf, sparql);
    }

    #[test]
    fn a_comment_at_the_end_of_the_file() {
        for src in ["<a> <b> <c> .\n# end\n", "<a> <b> <c> .\n\n# end"] {
            let (rdf, sparql) = both(src);
            assert_eq!(rdf.end, ["# end"]);
            assert_eq!(rdf.comment_blanks, [src.contains("\n\n")]);
            assert!(rdf.toks.iter().all(|t| t.0.is_empty() && t.1.is_none()));
            assert_eq!(rdf, sparql);
        }
        // On the last token's line it trails it.
        let src = "<a> <b> <c> . # end";
        let (rdf, sparql) = both(src);
        assert_eq!(rdf.toks[3].1.as_deref(), Some("# end"));
        assert!(rdf.end.is_empty());
        assert_eq!(rdf, sparql);
    }

    #[test]
    fn a_lone_carriage_return_ends_a_line_only_for_sparql() {
        let src = "<a> <b> <c> .\r# c\r<d> <e> <f> .";
        let (rdf, sparql) = both(src);
        assert_eq!(rdf.toks[3].1.as_deref(), Some("# c"));
        assert!(rdf.toks[4].0.is_empty());
        assert_eq!(sparql.toks[3].1, None);
        assert_eq!(sparql.toks[4].0, ["# c"]);
        // `\r\n` is a line break for both.
        let (rdf, sparql) = both("<a> <b> <c> .\r\n# c\r\n<d> <e> <f> .");
        assert_eq!(rdf, sparql);
        assert_eq!(rdf.toks[4].0, ["# c"]);
    }

    #[test]
    fn comments_inside_a_range() {
        let src = "<a> # t\n<b>\n# l\n<c> <d>";
        let toks = crate::syntax::lexer::tokenize(src);
        let rdf = Doc::new(src, &toks);
        let toks = crate::sparql::lexer::tokenize(src);
        let sparql = Doc::new(src, &toks);
        // `# t` trails 0, `# l` leads 2: neither the trailing comment of the
        // last token nor the leading comment of the first is inside.
        let cases = [
            ((0, 0), false),
            ((0, 1), true),
            ((1, 1), false),
            ((1, 2), true),
            ((2, 3), false),
            ((3, 3), false),
        ];
        for ((a, b), want) in cases {
            assert_eq!(rdf.comments_inside(a, b), want, "{a}..={b}");
            assert_eq!(sparql.comments_inside(a, b), want, "{a}..={b}");
        }
    }

    #[test]
    fn token_text() {
        let src = "<a> ex:b \"c\"@en";
        let toks = crate::syntax::lexer::tokenize(src);
        let rdf = Doc::new(src, &toks);
        let toks = crate::sparql::lexer::tokenize(src);
        let sparql = Doc::new(src, &toks);
        assert_eq!((rdf.text(0), rdf.text(1)), ("<a>", "ex:b"));
        assert_eq!((sparql.text(0), sparql.text(1)), ("<a>", "ex:b"));
    }

    #[test]
    fn an_omitted_token_hands_its_comments_to_its_neighbours() {
        let semi = |src: &str| {
            let toks = crate::syntax::lexer::tokenize(src);
            let mut d = Doc::new(src, &toks);
            let omit: Vec<Ix> = (0..d.toks.len() as Ix)
                .filter(|&i| d.text(i) == ";")
                .collect();
            d.attach(&omit);
            let s = shape(&d);
            assert!(
                omit.iter()
                    .all(|&i| s.toks[i as usize] == tok(&[], None, false))
            );
            s
        };
        // On the `;` line: trails the token before.
        let s = semi("<o> ; # c\n.");
        assert_eq!(s.toks[0], tok(&[], Some("# c"), false));
        assert_eq!(s.toks[2], tok(&[], None, false));
        let s = semi("<o>\n ; # c\n .");
        assert_eq!(s.toks[0], tok(&[], Some("# c"), false));
        // That token already has one: leads the token after.
        let s = semi("<o> # c1\n; # c2\n.");
        assert_eq!(s.toks[0], tok(&[], Some("# c1"), false));
        assert_eq!(s.toks[2], tok(&["# c2"], None, false));
        // An own-line comment above the `;` leads the token after it, with no
        // blank line counted across the `;`.
        let s = semi("<o>\n# c\n;\n.");
        assert_eq!(s.toks[2], tok(&["# c"], None, false));
        let s = semi("<o>\n;\n# c\n.");
        assert_eq!(s.toks[2], tok(&["# c"], None, false));
        assert_eq!(s.comment_blanks, [false]);
        // A blank line in the whitespace before the `;` still counts.
        let s = semi("<o>\n\n;\n# c\n.");
        assert_eq!(s.comment_blanks, [true]);
    }

    #[test]
    fn blank_line_rule() {
        // Both formatters' old rules gave these.
        for (gap, want) in [
            ("\n\n", true),
            ("\n  \n", true),
            ("\r\n\r\n", true),
            ("\n", false),
            ("", false),
        ] {
            assert_eq!(has_blank(gap), want, "{gap:?}");
        }
    }
}
