//! Turtle and SPARQL names in one place: the `PN_CHARS` tables, prefix and
//! local-name rules, the two namespace rules, IRI shortening, the prefixes in
//! scope at an offset and where a prefix declaration goes.

use super::{PrefixDecl, PrefixMap, Span};
use crate::analysis::{Analysis, Encoding};
use lsp_types::{Range, TextEdit};
use std::ops::{Deref, DerefMut};

// ---- PN_CHARS tables (Turtle and SPARQL share them) ----

pub(crate) fn is_pn_chars_base(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

pub(crate) fn is_pn_chars_u(c: char) -> bool {
    c == '_' || is_pn_chars_base(c)
}

pub(crate) fn is_pn_chars(c: char) -> bool {
    is_pn_chars_u(c)
        || c == '-'
        || c.is_ascii_digit()
        || c == '\u{B7}'
        || ('\u{300}'..='\u{36F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}

// ---- prefix and local names ----

/// PN_PREFIX (the empty prefix included).
pub(crate) fn is_pn_prefix(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        None => true,
        Some(c) => {
            is_pn_chars_base(c) && chars.all(|c| is_pn_chars(c) || c == '.') && !name.ends_with('.')
        }
    }
}

/// A PN_LOCAL that needs no backslash escapes; `%` only as a `%XX` escape.
pub(crate) fn is_pn_local(local: &str) -> bool {
    let b = local.as_bytes();
    let mut first = true;
    for (i, c) in local.char_indices() {
        let ok = match c {
            '%' => b
                .get(i + 1..i + 3)
                .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)),
            ':' => true,
            '.' => !first,
            c if first => is_pn_chars_u(c) || c.is_ascii_digit(),
            c => is_pn_chars(c),
        };
        if !ok {
            return false;
        }
        first = false;
    }
    !local.ends_with('.')
}

// ---- namespaces ----

/// `iri` up to its last `#` or `/`, when that is past the authority: the
/// namespace a new prefix declaration would bind. For the namespace a
/// vocabulary lookup or the project's namespace index uses, see
/// [`vocabulary_namespace`].
pub(crate) fn declarable_namespace(iri: &str) -> Option<&str> {
    let scheme_end = iri.find(':')?;
    let rest = &iri[scheme_end + 1..];
    let path_start = match rest.strip_prefix("//") {
        Some(auth) => scheme_end + 3 + auth.find(['/', '#', '?'])?,
        None => scheme_end + 1,
    };
    let cut = iri.rfind(['#', '/'])?;
    (cut >= path_start && !iri[..cut].contains('?')).then(|| &iri[..=cut])
}

/// The namespace the bundled vocabularies and the knowledge index file `iri`
/// under (see [`crate::vocab::split_namespace`]); `""` with no delimiter.
pub(crate) fn vocabulary_namespace(iri: &str) -> &str {
    crate::vocab::split_namespace(iri).0
}

// ---- shortening ----

/// `prefix:local` for `iri` using the longest namespace in `scope` whose local
/// part is a PN_LOCAL as is (no backslash escapes needed) and whose prefix
/// name is a PN_PREFIX. Ties go to the shorter prefix name, then the smaller.
pub(crate) fn shorten(iri: &str, scope: &PrefixMap) -> Option<String> {
    scope
        .iter()
        .filter(|(name, ns)| {
            let (name, ns): (&str, &str) = (name, ns);
            !ns.is_empty()
                && iri.starts_with(ns)
                && is_pn_prefix(name)
                && is_pn_local(&iri[ns.len()..])
        })
        .min_by(|(n1, ns1), (n2, ns2)| {
            ns2.len()
                .cmp(&ns1.len())
                .then(n1.len().cmp(&n2.len()))
                .then(n1.cmp(n2))
        })
        .map(|(name, ns)| format!("{name}:{}", &iri[ns.len()..]))
}

/// The Turtle lexer's (kind, span) of each token of `text`, for [`token_edits`].
fn turtle_tokens(text: &str) -> Vec<(super::lexer::Kind, Span)> {
    super::lexer::tokenize(text)
        .iter()
        .map(|t| (t.kind, Span::new(t.start, t.end)))
        .collect()
}

/// [`token_edits`] for Turtle `text`, whose tokens are `tokens`. Only the
/// run of tokens around the replacements up to a gap on either side is
/// lexed again: no Turtle token reaches across whitespace, and a token start
/// is a place to start lexing from, so one IRI costs its neighbours, not the
/// file.
pub(crate) fn turtle_token_edits(
    text: &str,
    tokens: &[super::lexer::Token],
    reps: Vec<(Span, String)>,
) -> Option<Vec<(Span, String)>> {
    let lo = reps.iter().map(|r| r.0.start).min().unwrap_or(0);
    let hi = reps.iter().map(|r| r.0.end).max().unwrap_or(0);
    let (mut i, mut from) = (tokens.partition_point(|t| t.start < lo), lo);
    while i > 0 && tokens[i - 1].end == from {
        i -= 1;
        from = tokens[i].start;
    }
    let (mut j, mut to) = (tokens.partition_point(|t| t.end <= hi), hi);
    while j < tokens.len() && tokens[j].start == to {
        to = tokens[j].end;
        j += 1;
    }
    let reps = reps
        .into_iter()
        .map(|(s, r)| (moved(s, -(from as i64)), r))
        .collect();
    let edits = token_edits(&text[from as usize..to as usize], reps, turtle_tokens)?;
    Some(
        edits
            .into_iter()
            .map(|(s, r)| (moved(s, from as i64), r))
            .collect(),
    )
}

/// Edits of `text` that replace whole tokens: each `(span, new)` replaces the
/// tokens in `span` (an insertion is an empty span, a deletion an empty
/// text). Invariant: `lex` reads the edited text as the token stream before
/// with exactly those tokens changed. Where a replacement would fuse with or
/// split a neighbour (`ex:a.b.` then `sh:V`, `1.` then `e1:x`), a space goes
/// at that end; the edits come back sorted and trimmed to the characters
/// that change. `None` when no spaces make the edited text lex so (an IRI
/// after an operand in a SPARQL expression reads as `<`): such an edit is
/// not to be made.
pub(crate) fn token_edits<K: PartialEq + Copy>(
    text: &str,
    reps: Vec<(Span, String)>,
    lex: impl Fn(&str) -> Vec<(K, Span)>,
) -> Option<Vec<(Span, String)>> {
    token_edits_after(text, reps, &lex, &lex)
}

/// [`token_edits`] where `text` and its edited forms lex by `lex_text`, a
/// lexer that knows what came before `text`, and each replacement alone by
/// `lex`. `lex_text` reads `text` first and, when edits come back, the text
/// they make last.
pub(crate) fn token_edits_after<K: PartialEq + Copy>(
    text: &str,
    mut reps: Vec<(Span, String)>,
    lex: &dyn Fn(&str) -> Vec<(K, Span)>,
    lex_text: &dyn Fn(&str) -> Vec<(K, Span)>,
) -> Option<Vec<(Span, String)>> {
    reps.sort_by_key(|(s, _)| s.start);
    let old = lex_text(text);
    // A space at each end of each replacement: `2 * r` before, `2 * r + 1` after.
    let mut pads = vec![false; 2 * reps.len()];
    let padded = |pads: &[bool], r: usize, rep: &str| {
        let space = |p: bool| if p { " " } else { "" };
        format!("{}{rep}{}", space(pads[2 * r]), space(pads[2 * r + 1]))
    };
    loop {
        // The edited text, the tokens it should lex to, and where each
        // replacement's ends land in it (ascending).
        let (mut new, mut want, mut ends) = (String::new(), Vec::new(), Vec::new());
        let mut old_tokens = old.iter().peekable();
        let mut copied = 0;
        for (r, (span, rep)) in reps.iter().enumerate() {
            let shift = new.len() as i64 - copied as i64;
            while let Some(&(k, s)) = old_tokens.next_if(|(_, s)| s.end <= span.start) {
                want.push((k, moved(s, shift)));
            }
            while old_tokens.next_if(|(_, s)| s.start < span.end).is_some() {}
            new.push_str(&text[copied..span.start as usize]);
            ends.push(new.len());
            let rep = padded(&pads, r, rep);
            let shift = new.len() as i64;
            want.extend(lex(&rep).into_iter().map(|(k, s)| (k, moved(s, shift))));
            new.push_str(&rep);
            ends.push(new.len());
            copied = span.end as usize;
        }
        let shift = new.len() as i64 - copied as i64;
        want.extend(old_tokens.map(|&(k, s)| (k, moved(s, shift))));
        new.push_str(&text[copied..]);
        let got = lex_text(&new);
        // Each run of differing tokens, from where both streams part to where
        // they end a token at the same offset, gets a space at its first end
        // of a replacement that has none, inside the run before at its edge.
        let (mut changed, mut differs) = (false, false);
        let (mut i, mut j) = (0, 0);
        while i < want.len() || j < got.len() {
            if want.get(i) == got.get(j) {
                (i, j) = (i + 1, j + 1);
                continue;
            }
            differs = true;
            let start = |t: Option<&(K, Span)>| t.map_or(usize::MAX, |t| t.1.start as usize);
            let lo = start(want.get(i)).min(start(got.get(j)));
            let mut hi = lo;
            loop {
                let end = |t: Option<&(K, Span)>| t.map(|t| t.1.end as usize);
                match (end(want.get(i)), end(got.get(j))) {
                    (Some(a), Some(b)) if a == b => {
                        (i, j, hi) = (i + 1, j + 1, a);
                        break;
                    }
                    (Some(a), b) if b.is_none_or(|b| a < b) => (i, hi) = (i + 1, hi.max(a)),
                    (_, Some(b)) => (j, hi) = (j + 1, hi.max(b)),
                    _ => break,
                }
            }
            let first = ends.partition_point(|&e| e < lo);
            let run: Vec<usize> = (first..ends.len())
                .take_while(|&e| ends[e] <= hi)
                .filter(|&e| !pads[e])
                .collect();
            let inside = run.iter().find(|&&e| lo < ends[e] && ends[e] < hi);
            if let Some(&e) = inside.or(run.first()) {
                pads[e] = true;
                changed = true;
            }
        }
        // Equal, or no space at a replacement's end can make it so.
        if !changed {
            if differs {
                return None;
            }
            break;
        }
    }
    let edits = reps
        .iter()
        .enumerate()
        .map(|(r, (span, rep))| {
            let old = &text[span.start as usize..span.end as usize];
            trimmed(old, *span, padded(&pads, r, rep))
        })
        .collect();
    Some(edits)
}

fn moved(s: Span, by: i64) -> Span {
    Span::new((s.start as i64 + by) as u32, (s.end as i64 + by) as u32)
}

/// `(span, new)` without the characters `new` shares with the start and the
/// end of `old`, the text at `span`.
fn trimmed(old: &str, span: Span, new: String) -> (Span, String) {
    let head: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let tail: usize = old[head..]
        .chars()
        .rev()
        .zip(new[head..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let text = new[head..new.len() - tail].to_owned();
    (
        Span::new(span.start + head as u32, span.end - tail as u32),
        text,
    )
}

// ---- prefixes in scope ----

/// The prefixes in scope at a position of a document.
#[derive(Clone, Debug, Default)]
pub(crate) struct Scope(PrefixMap);

impl Scope {
    /// The external prefixes (prefixes.ttl and settings) alone, before any
    /// declaration of the document.
    pub(crate) fn outside(a: &Analysis) -> Scope {
        Scope((*a.external).clone())
    }

    /// Prefixes in scope at `offset`: external ones, then the declarations
    /// that end before it.
    pub(crate) fn at(a: &Analysis, offset: u32) -> Scope {
        let mut scope = Scope::outside(a);
        for p in a
            .parsed
            .prefixes
            .iter()
            .take_while(|p| p.span.end <= offset)
        {
            scope.0.insert(p.name.clone(), p.iri.to_string());
        }
        scope
    }

    /// Every prefix of the document: the scope at its end.
    pub(crate) fn whole(a: &Analysis) -> Scope {
        Scope::at(a, u32::MAX)
    }

    /// `iri` as a prefixed name of this scope, else `<iri>`. Does not escape:
    /// callers hold valid IRIs.
    pub(crate) fn write(&self, iri: &str) -> String {
        shorten(iri, &self.0).unwrap_or_else(|| format!("<{iri}>"))
    }
}

impl From<PrefixMap> for Scope {
    fn from(map: PrefixMap) -> Scope {
        Scope(map)
    }
}

/// The declaration of prefix `name` in scope at `offset`: the last one ending
/// before it. `None` when the prefix comes from outside the document, or is
/// not declared yet there.
pub(crate) fn declaration_at<'a>(
    a: &'a Analysis,
    name: &str,
    offset: u32,
) -> Option<&'a PrefixDecl> {
    a.parsed
        .prefixes
        .iter()
        .rev()
        .find(|p| p.name == name && p.span.end <= offset)
}

impl Deref for Scope {
    type Target = PrefixMap;
    fn deref(&self) -> &PrefixMap {
        &self.0
    }
}

impl DerefMut for Scope {
    fn deref_mut(&mut self) -> &mut PrefixMap {
        &mut self.0
    }
}

// ---- line endings and prefix declarations ----

/// The line ending the text uses: that of its first line break (`\n`,
/// `\r\n` or a bare `\r`; `\n` for a text without one).
pub(crate) fn newline(text: &str) -> &'static str {
    match text.find(['\n', '\r']).map(|i| &text[i..]) {
        Some(rest) if rest.starts_with("\r\n") => "\r\n",
        Some(rest) if rest.starts_with('\r') => "\r",
        _ => "\n",
    }
}

/// The keyword a declaration added to `a` starts with, in the style the
/// document already uses: `PREFIX` when its last declaration is SPARQL-style,
/// else `@prefix`.
pub(crate) fn declaration_keyword(a: &Analysis) -> &'static str {
    let sparql = a.parsed.prefixes.last().is_some_and(|p: &PrefixDecl| {
        a.tokens()
            .iter()
            .any(|t| t.start == p.span.start && t.kind == super::lexer::Kind::SparqlPrefix)
    });
    if sparql { "PREFIX" } else { "@prefix" }
}

/// One declaration line in the style the document already uses (see
/// [`declaration_keyword`]), ended by the document's line ending.
pub(crate) fn declaration_line(a: &Analysis, prefix: &str, ns: &str) -> String {
    let nl = newline(&a.text);
    if declaration_keyword(a) == "PREFIX" {
        format!("PREFIX {prefix}: <{ns}>{nl}")
    } else {
        format!("@prefix {prefix}: <{ns}> .{nl}")
    }
}

/// Where a declaration line goes for text from `before` on, by
/// [`insertion_point`]: after the last declaration that ends before `before`
/// outside a TriG graph block, else after the file's header comment. The
/// flag says the line must start with a line break.
pub(crate) fn declaration_point(a: &Analysis, before: u32) -> (usize, bool) {
    use super::lexer::Kind::Comment;
    let (text, tokens, graphs) = (&a.text, a.tokens(), &a.parsed.graphs);
    let in_graph = |s: Span| {
        graphs[..graphs.partition_point(|g| g.span.start <= s.start)]
            .last()
            .is_some_and(|g| s.end <= g.span.end)
    };
    let anchor = a
        .parsed
        .prefixes
        .iter()
        .rev()
        .find(|p| p.span.end <= before && !in_graph(p.span))
        .map(|p| p.span.end as usize);
    let anchor = anchor.unwrap_or_else(|| {
        let comments = tokens.iter().take_while(|t| t.kind == Comment);
        header_end(text, comments.map(|t| Span::new(t.start, t.end)))
    });
    let next = tokens[tokens.partition_point(|t| (t.start as usize) < anchor)..]
        .iter()
        .find(|t| t.kind != Comment)
        .map_or(text.len(), |t| t.start as usize);
    insertion_point(text, anchor, next, before as usize)
}

/// The end of the header: the run of `comments` (the file's leading comment
/// tokens) up to the first gap that holds a blank line; 0 with none.
pub(crate) fn header_end(text: &str, comments: impl Iterator<Item = Span>) -> usize {
    let breaks =
        |s: &str| s.matches('\n').count() + s.matches('\r').count() - s.matches("\r\n").count();
    let mut end = 0;
    for c in comments {
        // The top of the file counts as a line break: a blank first line ends the header.
        if breaks(&text[end..c.start as usize]) + usize::from(end == 0) > 1 {
            break;
        }
        end = c.end as usize;
    }
    end
}

/// Where a declaration goes after the token or header ending at
/// `anchor_end`, when the next token that is no comment starts at
/// `next_start`: the start of the line after the gap's first line break
/// (`\r\n`, `\r` or `\n`; comments hold none), when that is at or before
/// `before`; else `anchor_end`, with the flag that the text must start with
/// a line break; the top of the file without one when `anchor_end` is 0.
/// Either way the point is whitespace between two statements.
pub(crate) fn insertion_point(
    text: &str,
    anchor_end: usize,
    next_start: usize,
    before: usize,
) -> (usize, bool) {
    let gap = &text[anchor_end..next_start];
    if let Some(i) = gap.find(['\r', '\n']) {
        let line = anchor_end + i + if gap[i..].starts_with("\r\n") { 2 } else { 1 };
        if line <= before {
            return (line, false);
        }
    }
    (anchor_end, anchor_end > 0)
}

/// The edit that declares `prefix` as `ns`: see [`declarations_edit`].
pub(crate) fn declaration_edit(
    a: &Analysis,
    enc: Encoding,
    prefix: &str,
    ns: &str,
    before: u32,
) -> TextEdit {
    declarations_edit(a, enc, &[(prefix, ns)], before)
}

/// The edit that declares each of `decls` (prefix, namespace), one line
/// each, for text from `before` on, at [`declaration_point`]. A prefix that
/// is used before `before` without a declaration there moves the point back
/// to that first use, so the declaration precedes every use it serves. The
/// text is an insertion through [`turtle_token_edits`], so it fuses with no
/// neighbour (`.` then `1` is the decimal `.1`).
pub(crate) fn declarations_edit(
    a: &Analysis,
    enc: Encoding,
    decls: &[(&str, &str)],
    before: u32,
) -> TextEdit {
    let first_use = a
        .parsed
        .prefix_uses
        .iter()
        .filter(|u| !u.resolved && decls.iter().any(|(p, _)| *p == &*u.name))
        .map(|u| u.span.start)
        .min();
    let before = first_use.map_or(before, |u| u.min(before));
    let (at, break_first) = declaration_point(a, before);
    let lines: String = decls
        .iter()
        .map(|(p, ns)| declaration_line(a, p, ns))
        .collect();
    let nl = newline(&a.text);
    let text = if break_first {
        format!("{nl}{}", lines.trim_end_matches(nl))
    } else {
        lines
    };
    let at = Span::new(at as u32, at as u32);
    // Never `None`: a space keeps any two Turtle tokens apart.
    let edits = turtle_token_edits(&a.text, a.tokens(), vec![(at, text.clone())]);
    let mut new_text = edits.and_then(|mut e| e.pop()).map_or(text, |e| e.1);
    // A space would leave data on the declaration's line: end it with a line break.
    if let Some(t) = new_text.strip_suffix(' ') {
        new_text = format!("{t}{nl}");
    }
    let pos = a.position(enc, at.start);
    TextEdit {
        range: Range::new(pos, pos),
        new_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Encoding;
    use std::sync::Arc;

    fn analysis(text: &str) -> Analysis {
        Analysis::new(
            "file:///t/doc.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        )
    }

    fn map(pairs: &[(&str, &str)]) -> PrefixMap {
        pairs
            .iter()
            .map(|(p, ns)| (p.to_string(), ns.to_string()))
            .collect()
    }

    #[test]
    fn plain_local_names() {
        for ok in ["", "a", "a.b", "1a", "_x", "a:b", "a%20b", "a-b", "é"] {
            assert!(is_pn_local(ok), "{ok}");
        }
        for bad in ["a.", ".a", "-a", "a/b", "a#b", "a%2", "a?b", "a b", "a~"] {
            assert!(!is_pn_local(bad), "{bad}");
        }
    }

    #[test]
    fn prefix_names() {
        for ok in ["", "a", "ex", "a.b", "a-1", "é"] {
            assert!(is_pn_prefix(ok), "{ok}");
        }
        for bad in ["1x", "a.", "-a", "_a", "a b", "a:b", ".a"] {
            assert!(!is_pn_prefix(bad), "{bad}");
        }
    }

    /// `text` with `edits` applied.
    fn apply(text: &str, mut edits: Vec<(Span, String)>) -> String {
        edits.sort_by_key(|(s, _)| std::cmp::Reverse(s.start));
        let mut out = text.to_owned();
        for (s, t) in edits {
            out.replace_range(s.start as usize..s.end as usize, &t);
        }
        out
    }

    #[test]
    fn token_edits_change_exactly_the_given_tokens() {
        let span = |text: &str, token: &str| {
            let at = text.find(token).unwrap() as u32;
            Span::new(at, at + token.len() as u32)
        };
        let iri = "<http://e/x>";
        // (text, replaced token or "" for an insertion at `at`, new text, after)
        let turtle = [
            (
                "ex:a.b.<http://s/V>",
                "<http://s/V>",
                "sh:V",
                "ex:a.b. sh:V",
            ),
            ("_:b.<http://e/x>", iri, "ex:x", "_:b. ex:x"),
            ("1.<http://g/x>", "<http://g/x>", "e1:x", "1. e1:x"),
            ("a<http://e/C>", "<http://e/C>", "ex:C", "a ex:C"),
            ("<http://e/x>.b", iri, "ex:x", "ex:x .b"),
            ("<http://e/x>:y", iri, "ex:x", "ex:x :y"),
            ("<http://e/x>-1", iri, "ex:x", "ex:x -1"),
            ("@prefix:<http://e/>", ":", "ex:", "@prefix ex:<http://e/>"),
            ("ex:a<http://e/x>ex:b", iri, "", "ex:a ex:b"),
            ("ex:a<http://e/x>", "", "ex:i", "ex:a ex:i<http://e/x>"),
            ("ex:café .", "ex:café", "ex:cafè", "ex:cafè ."),
        ];
        let sparql = [
            (
                "?a<=<http://e/b>&&?c>?a",
                "<http://e/b>",
                "ex:b",
                "?a<= ex:b&&?c>?a",
            ),
            (
                "FILTER(?a<=<http://e/b>&&?c>?a)",
                "<http://e/b>",
                "ex:b",
                "FILTER(?a<=ex:b&&?c>?a)",
            ),
        ];
        let check = |text: &str, token: &str, new: &str, after: &str, sparql: bool| {
            let at = if token.is_empty() {
                Span::new(4, 4)
            } else {
                span(text, token)
            };
            let reps = vec![(at, new.to_owned())];
            let edits = if sparql {
                token_edits(text, reps, crate::sparql::lexer::kinds)
            } else {
                token_edits(text, reps, turtle_tokens)
            }
            .unwrap();
            for (s, _) in &edits {
                assert!(
                    text.is_char_boundary(s.start as usize)
                        && text.is_char_boundary(s.end as usize)
                );
            }
            assert_eq!(apply(text, edits), after, "{text}");
        };
        for (text, token, new, after) in turtle {
            check(text, token, new, after, false);
        }
        for (text, token, new, after) in sparql {
            check(text, token, new, after, true);
        }
        // Two replacements side by side, and the trim on a character boundary.
        let text = "<http://e/a><http://e/b>";
        let reps = vec![
            (Span::new(0, 12), "ex:a".to_owned()),
            (Span::new(12, 24), "ex:b".to_owned()),
        ];
        assert_eq!(
            apply(text, token_edits(text, reps, turtle_tokens).unwrap()),
            "ex:a ex:b"
        );
        let edits = token_edits(
            "ex:café",
            vec![(Span::new(0, 8), "ex:cafè".into())],
            turtle_tokens,
        );
        assert_eq!(edits, Some(vec![(Span::new(6, 8), "è".to_owned())]));
        // An IRI after an operand in an expression reads as `<`, spaced or not.
        let text = "FILTER(?a ex:b)";
        let reps = vec![(span(text, "ex:b"), iri.to_owned())];
        assert_eq!(token_edits(text, reps, crate::sparql::lexer::kinds), None);
    }

    #[test]
    fn shorten_never_writes_a_name_turtle_cannot_read() {
        let scope = map(&[("ex", "http://example.org/")]);
        // A local part starting with `-` or `.` is no PN_LOCAL.
        for bad in ["-a", ".a", "a.", "a b", "a/b", "a#b"] {
            assert_eq!(
                shorten(&format!("http://example.org/{bad}"), &scope),
                None,
                "{bad}"
            );
        }
        assert_eq!(
            shorten("http://example.org/a-b", &scope).as_deref(),
            Some("ex:a-b")
        );
        // A prefix name that is no PN_PREFIX is never written.
        let scope = map(&[("1x", "http://example.org/"), ("a.", "http://example.org/")]);
        assert_eq!(shorten("http://example.org/a", &scope), None);
    }

    #[test]
    fn shorten_ties_are_decided_by_length_then_order_not_by_the_map() {
        // The longest namespace wins; one namespace under two names goes to
        // the shorter name, then the smaller one, whatever the map's order.
        let pairs = [
            ("zz", "http://e.org/"),
            ("b", "http://e.org/"),
            ("a", "http://e.org/"),
            ("long", "http://e.org/ns/"),
        ];
        for rot in 0..pairs.len() {
            let mut rotated = pairs.to_vec();
            rotated.rotate_left(rot);
            let scope: PrefixMap = map(&rotated);
            assert_eq!(shorten("http://e.org/x", &scope).as_deref(), Some("a:x"));
            assert_eq!(
                shorten("http://e.org/ns/x", &scope).as_deref(),
                Some("long:x")
            );
        }
    }

    #[test]
    fn the_two_namespace_rules_differ_where_they_should() {
        assert_eq!(
            declarable_namespace("http://xmlns.com/foaf/0.1/Person"),
            Some("http://xmlns.com/foaf/0.1/")
        );
        assert_eq!(declarable_namespace("http://example.org"), None);
        assert_eq!(declarable_namespace("http://example.org/a?x=/y"), None);
        // The vocabulary rule always gives a namespace, even with no delimiter.
        assert_eq!(vocabulary_namespace("nodelimiter"), "");
        assert_eq!(
            vocabulary_namespace("http://ogp.me/ns#audio:album"),
            "http://ogp.me/ns#"
        );
    }

    #[test]
    fn newline_follows_the_first_line_break() {
        assert_eq!(newline(""), "\n");
        assert_eq!(newline("a"), "\n");
        assert_eq!(newline("a\nb\r\n"), "\n");
        assert_eq!(newline("a\r\nb\n"), "\r\n");
        assert_eq!(newline("a\rb\n"), "\r");
    }

    #[test]
    fn scope_holds_the_declarations_above_the_offset() {
        let text = "@prefix a: <http://a/> .\na:x a:p a:o .\n@prefix a: <http://a2/> .\n@prefix b: <http://b/> .\na:y a:p a:o .\n";
        let a = analysis(text);
        let at = |needle: &str| text.find(needle).unwrap() as u32;
        assert!(Scope::at(&a, 0).is_empty());
        let first = Scope::at(&a, at("a:x"));
        assert_eq!(first.get("a").map(String::as_str), Some("http://a/"));
        assert!(!first.contains_key("b"));
        let last = Scope::at(&a, at("a:y"));
        assert_eq!(last.get("a").map(String::as_str), Some("http://a2/"));
        assert!(last.contains_key("b"));
        assert_eq!(Scope::whole(&a).len(), 2);
        assert_eq!(
            declaration_at(&a, "a", at("a:y")).map(|p| p.iri.to_string()),
            Some("http://a2/".into())
        );
        assert!(declaration_at(&a, "b", at("a:x")).is_none());
    }

    /// The text after inserting the declaration of `ex:` for the first use.
    fn declared(text: &str) -> String {
        let a = analysis(text);
        let e = declaration_edit(&a, Encoding::Utf8, "ex", "http://ex/", u32::MAX);
        let at = a.offset(Encoding::Utf8, e.range.start).unwrap() as usize;
        format!("{}{}{}", &text[..at], e.new_text, &text[at..])
    }

    #[test]
    fn a_declaration_goes_before_the_first_use_it_serves() {
        // After the last declaration above the use, not after the last one.
        assert_eq!(
            declared("@prefix a: <http://a/> .\nex:s a:p a:o .\n@prefix b: <http://b/> .\n"),
            "@prefix a: <http://a/> .\n@prefix ex: <http://ex/> .\nex:s a:p a:o .\n@prefix b: <http://b/> .\n"
        );
        // No declaration above it: the top, below the header comment.
        assert_eq!(
            declared("# header\nex:s ex:p ex:o .\n@prefix b: <http://b/> .\n"),
            "# header\n@prefix ex: <http://ex/> .\nex:s ex:p ex:o .\n@prefix b: <http://b/> .\n"
        );
        // The use shares the declaration's line.
        assert_eq!(
            declared("@prefix a: <http://a/> . ex:s a:p a:o .\n"),
            "@prefix a: <http://a/> .\n@prefix ex: <http://ex/> . ex:s a:p a:o .\n"
        );
        // A last line without a break.
        assert_eq!(
            declared("@prefix a: <http://a/> .\nex:s a:p a:o ."),
            "@prefix a: <http://a/> .\n@prefix ex: <http://ex/> .\nex:s a:p a:o ."
        );
    }

    #[test]
    fn a_declaration_never_splits_a_token_or_statement() {
        let cases = [
            // Data, then a line break inside the statement.
            (
                "@prefix a: <http://a/> . a:s\n  ex:p a:o .\n",
                "@prefix a: <http://a/> .\n@prefix ex: <http://ex/> . a:s\n  ex:p a:o .\n",
            ),
            // A line break inside a string on the declaration's line.
            (
                "@prefix a: <http://a/> . a:s a:p \"\"\"x\ny\"\"\" .\nex:s a:p a:o .\n",
                "@prefix a: <http://a/> .\n@prefix ex: <http://ex/> . a:s a:p \"\"\"x\ny\"\"\" .\nex:s a:p a:o .\n",
            ),
            // `\r` line breaks.
            (
                "@prefix a: <http://a/> .\ra:s a:p \"\"\"x\ny\"\"\" .\rex:s a:p a:o .\r",
                "@prefix a: <http://a/> .\r@prefix ex: <http://ex/> .\ra:s a:p \"\"\"x\ny\"\"\" .\rex:s a:p a:o .\r",
            ),
            (
                "@prefix a: <http://a/> .\r# note\rex:s a:p a:o .\r",
                "@prefix a: <http://a/> .\r@prefix ex: <http://ex/> .\r# note\rex:s a:p a:o .\r",
            ),
            (
                "# c\rex:s a:p a:o .",
                "# c\r@prefix ex: <http://ex/> .\rex:s a:p a:o .",
            ),
            // The header ends at a blank line: a doc comment stays on its statement.
            (
                "# licence\n\n# doc\nex:s a:p a:o .\n",
                "# licence\n@prefix ex: <http://ex/> .\n\n# doc\nex:s a:p a:o .\n",
            ),
            // `.` then `1` would be the decimal `.1`.
            (
                "PREFIX a: <http://a/>1\nex:s a:p a:o .\n@prefix b: <http://b/> .\n",
                "PREFIX a: <http://a/>\n@prefix ex: <http://ex/> .\n1\nex:s a:p a:o .\n@prefix b: <http://b/> .\n",
            ),
        ];
        for (text, after) in cases {
            assert_eq!(declared(text), after, "{text:?}");
        }
        // A declaration inside a TriG graph block is no anchor.
        let text =
            "@prefix a: <http://a/> .\na:g {\n  @prefix b: <http://b/> .\n}\nex:s a:p a:o .\n";
        let a = Analysis::new(
            "file:///t/doc.trig".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        );
        assert_eq!(declaration_point(&a, u32::MAX), (25, false));
    }

    #[test]
    fn inserted_lines_end_like_the_file() {
        assert_eq!(
            declared("@prefix a: <http://a/> .\r\nex:s a:p a:o .\r\n"),
            "@prefix a: <http://a/> .\r\n@prefix ex: <http://ex/> .\r\nex:s a:p a:o .\r\n"
        );
        assert_eq!(
            declared("# c\r\nex:s ex:p ex:o ."),
            "# c\r\n@prefix ex: <http://ex/> .\r\nex:s ex:p ex:o ."
        );
        // A file with no line break yet uses `\n`.
        assert_eq!(
            declared("PREFIX a: <http://a/>"),
            "PREFIX a: <http://a/>\nPREFIX ex: <http://ex/>"
        );
    }
}
