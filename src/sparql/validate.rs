//! Validation by `spargebra`, and where its errors are in the text.
//!
//! `spargebra` says `error at LINE:COLUMN: expected WHAT` (column in
//! characters) at the furthest position any grammar rule failed. Keywords
//! are matched by reading as many characters as the keyword has and then
//! comparing, so a failed keyword reports its start plus its length; that
//! length is taken back off. The text is parsed with trailing room so that a
//! keyword read near the end does not stop at the end of the text instead.
//! The error then marks the token at that position (a whole prefixed name or
//! word), or the end of the text when only the room is left ([`locate`]).
//!
//! A keyword read that starts a few characters before the real error ends
//! past it and wins as the furthest failure (`{ ?x a }` reads `OPTIONAL`
//! from `?x` over the `}`). [`check_tokens`] avoids that for documents: it
//! parses a copy of the tokens with room after each one (comments left out,
//! which changes no meaning), so that a keyword read never reaches the next
//! token, and maps the position back.

use super::{Token, TokenKind};
use crate::syntax::Span;
use spargebra::SparqlParser;
use std::borrow::Cow;

/// Room after the text for the parser's keyword lookahead; longer than any
/// SPARQL keyword.
const PAD: &str = "                ";

/// Where a syntax error is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorAt {
    /// The token the parser stopped at (byte range in the text).
    Token(Span),
    /// The text ends too early.
    End,
    /// The parser gave no position (an error found after parsing, such as a
    /// blank node shared by two `INSERT DATA` blocks).
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Syntax,
    /// A prefixed name whose prefix the parser was not given (`name` without
    /// the colon); the span covers the prefixed name.
    UndeclaredPrefix {
        name: String,
    },
}

/// A text `spargebra` rejects, located.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub at: ErrorAt,
    pub kind: ErrorKind,
    /// What the parser expected, as it says it (`one of "(", "{"`); empty
    /// when it did not say.
    pub expected: String,
    /// The parser's whole message.
    pub message: String,
    /// The expected items name keywords (so the error is likely a misspelled
    /// one) or only "any character": a list not worth showing.
    pub vague: bool,
}

impl SyntaxError {
    /// The expected items, when worth showing (see [`SyntaxError::vague`]).
    pub fn shown_expected(&self) -> Option<&str> {
        (!self.vague).then_some(self.expected.as_str())
    }
}

fn parse_error(parser: SparqlParser, text: &str, update: bool) -> Option<String> {
    if update {
        parser.parse_update(text).err().map(|e| e.to_string())
    } else {
        parser.parse_query(text).err().map(|e| e.to_string())
    }
}

/// The text of token `t` as `spargebra` is given it: the dots of a prefixed
/// name's local part escaped (`ec:1.1.1.353` becomes `ec:1\.1\.1\.353`, the
/// same IRI). `spargebra` 0.4 rejects a local name with two dots or more,
/// which the grammar allows (real queries use EC numbers so).
pub fn token_for_spargebra<'a>(text: &'a str, t: &Token) -> Cow<'a, str> {
    let written = &text[t.span.start as usize..t.span.end as usize];
    if t.kind != TokenKind::PName {
        return Cow::Borrowed(written);
    }
    let Some(colon) = written.find(':') else {
        return Cow::Borrowed(written);
    };
    let local = &written[colon + 1..];
    let mut out = String::new();
    let mut escaped = false;
    for (i, c) in local.char_indices() {
        if c == '.' && !escaped {
            if out.is_empty() {
                out.push_str(&written[..colon + 1 + i]);
            }
            out.push_str("\\.");
        } else if !out.is_empty() {
            out.push(c);
        }
        escaped = c == '\\' && !escaped;
    }
    if out.is_empty() {
        Cow::Borrowed(written)
    } else {
        Cow::Owned(out)
    }
}

/// `text` (tokenized as `tokens`) as `spargebra` is given it: every token
/// through [`token_for_spargebra`]; borrowed when no token changes.
pub fn text_for_spargebra<'a>(text: &'a str, tokens: &[Token]) -> Cow<'a, str> {
    let mut out = String::new();
    let mut copied = 0;
    for t in tokens {
        if let Cow::Owned(fixed) = token_for_spargebra(text, t) {
            out.push_str(&text[copied..t.span.start as usize]);
            out.push_str(&fixed);
            copied = t.span.end as usize;
        }
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// Parses `text` (tokenized as `tokens`) with `parser` as a query or an
/// update; `None` when it parses, else the error located on the token where
/// the text stops making sense.
pub fn check_tokens(
    text: &str,
    tokens: &[Token],
    parser: SparqlParser,
    update: bool,
) -> Option<SyntaxError> {
    let fixed = text_for_spargebra(text, tokens);
    parse_error(parser.clone(), &fixed, update)?;
    // The tokens, each followed by room: (start in the copy, start in text).
    let mut spaced = String::with_capacity(text.len() + tokens.len() * (PAD.len() + 1));
    let mut map: Vec<(u32, u32)> = Vec::with_capacity(tokens.len());
    for t in tokens.iter().filter(|t| t.kind != TokenKind::Comment) {
        map.push((spaced.len() as u32, t.span.start));
        spaced.push_str(&token_for_spargebra(text, t));
        spaced.push_str(PAD);
    }
    let e = locate(&spaced, parser.clone(), update);
    if e.message.is_empty() {
        // The copy parses (a token the lexer reads differently): fall back.
        return Some(locate(&fixed, parser, update));
    }
    // Offsets in the copy are inside a token (spans never start in room).
    let back = |q: u32| {
        let k = map.partition_point(|&(s, _)| s <= q).saturating_sub(1);
        map.get(k).map_or(q, |&(s, o)| o + (q - s))
    };
    let at = match e.at {
        ErrorAt::Token(s) => {
            let start = back(s.start);
            ErrorAt::Token(Span::new(start, start + (s.end - s.start)))
        }
        other => other,
    };
    Some(SyntaxError { at, ..e })
}

/// Locates the error of a `text` that `parser` rejects (see the module
/// documentation).
pub fn locate(text: &str, parser: SparqlParser, update: bool) -> SyntaxError {
    let padded = format!("{text}{PAD}");
    let msg = parse_error(parser, &padded, update).unwrap_or_default();
    let unknown = |message: String| SyntaxError {
        at: ErrorAt::Unknown,
        kind: ErrorKind::Syntax,
        expected: String::new(),
        message,
        vague: true,
    };
    let parsed = msg.strip_prefix("error at ").and_then(|rest| {
        let (pos, expected) = rest.split_once(": expected ")?;
        let (line, col) = pos.split_once(':')?;
        Some((
            line.parse::<usize>().ok()?,
            col.parse::<usize>().ok()?,
            expected,
        ))
    });
    let Some((line, col, expected)) = parsed else {
        return unknown(msg);
    };
    let line_start = padded
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum::<usize>();
    let mut off = padded
        .get(line_start..)
        .and_then(|l| l.char_indices().nth(col.saturating_sub(1)))
        .map_or(padded.len(), |(i, _)| line_start + i);
    let items: Vec<&str> = expected
        .strip_prefix("one of ")
        .unwrap_or(expected)
        .split(", ")
        .collect();
    let keyword = items
        .iter()
        .filter(|i| !i.is_empty() && i.bytes().all(|c| c.is_ascii_alphabetic()))
        .map(|i| i.len())
        .max();
    if let Some(n) = keyword {
        off = off.saturating_sub(n);
        while !padded.is_char_boundary(off) {
            off -= 1;
        }
    }
    let vague = keyword.is_some() || items.iter().all(|i| *i == "[_]");
    let pname = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '%');
    if items.contains(&"Prefix not found") && off <= text.len() {
        // Reported at the end of the prefixed name.
        let start = text[..off]
            .char_indices()
            .rfind(|&(_, c)| !pname(c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        if let Some(colon) = text[start..off].find(':') {
            return SyntaxError {
                at: ErrorAt::Token(Span::new(start as u32, off as u32)),
                kind: ErrorKind::UndeclaredPrefix {
                    name: text[start..start + colon].to_owned(),
                },
                expected: expected.to_owned(),
                message: msg.clone(),
                vague,
            };
        }
    }
    let rest = text.get(off..).unwrap_or("");
    let at = match rest.char_indices().find(|(_, c)| !c.is_whitespace()) {
        // Only room left: the text ends too early.
        None => ErrorAt::End,
        Some((i, c)) => {
            let from = off + i;
            let len = if pname(c) {
                text[from..]
                    .find(|c: char| !pname(c))
                    .unwrap_or(text.len() - from)
            } else {
                c.len_utf8()
            };
            ErrorAt::Token(Span::new(from as u32, (from + len) as u32))
        }
    };
    SyntaxError {
        at,
        kind: ErrorKind::Syntax,
        expected: expected.to_owned(),
        message: msg.clone(),
        vague,
    }
}
