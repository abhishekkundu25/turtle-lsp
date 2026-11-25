//! The terminal symbols of OWL 2 Functional-Style Syntax (section 2.2 of the
//! structural specification), one at a time, each with its byte span.
//!
//! White space (space, tab, line feed, carriage return) and comments (`#` to
//! the end of the line, where a token can start) are skipped; a comment's
//! span is kept. After a token that does not end in a delimiter (`=`, `(`,
//! `)`, `<`, `>`, `@`, `^`), white space, a comment, a delimiter or the end
//! must follow (step 6 of the tokenisation): `10abc` and `"a""b"` are
//! errors. A run of characters that is no terminal symbol is one
//! [`Kind::Word`], which the grammar reports with what it expected there;
//! a malformed IRI, string, prefixed name or language tag is reported here
//! and comes out as [`Kind::Bad`], which the grammar passes over in silence.
//! Every character is looked at a bounded number of times, so the time is
//! linear in the text.

use super::{Errors, shown};
use crate::syntax::Span;
use crate::syntax::names::{is_pn_chars, is_pn_chars_u, is_pn_local, is_pn_prefix};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    LParen,
    RParen,
    Equals,
    /// `^^`.
    Carets,
    /// `<…>`, the brackets included.
    FullIri,
    /// `pfx:local` (an abbreviated IRI) or `pfx:` (a prefix name).
    PName,
    /// `_:label`.
    NodeId,
    /// `"…"`, the quotes included.
    String,
    /// `@tag`.
    LangTag,
    /// A run of digits.
    Integer,
    /// A run of ASCII letters: a keyword, if the grammar knows it.
    Keyword,
    /// Any other run of characters up to a delimiter or white space,
    /// reported by the grammar.
    Word,
    /// Malformed text, reported by the lexer.
    Bad,
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub(crate) kind: Kind,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl Token {
    pub(crate) fn span(self) -> Span {
        Span::new(self.start, self.end)
    }
}

/// Whether `c` is a delimiter (section 2.2).
fn is_delimiter(c: u8) -> bool {
    matches!(c, b'=' | b'(' | b')' | b'<' | b'>' | b'@' | b'^')
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

/// Whether a word (a keyword, prefixed name, node ID, integer or language
/// tag) ends before `c`.
fn ends_word(c: u8) -> bool {
    is_space(c) || is_delimiter(c) || c == b'"' || c == b'#'
}

pub(crate) struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    /// The comments skipped so far, in text order.
    pub(crate) comments: Vec<Span>,
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(text: &'a str) -> Lexer<'a> {
        // A byte order mark is no character of the document; offsets stay
        // those of the text as given.
        let pos = if text.starts_with('\u{feff}') { 3 } else { 0 };
        Lexer {
            text,
            bytes: text.as_bytes(),
            pos,
            comments: Vec::new(),
        }
    }

    /// Go on lexing from `pos`, a byte where a token may start (after a
    /// token or a comment that the lexer read before).
    pub(crate) fn seek(&mut self, pos: usize) {
        self.pos = pos.max(self.pos).min(self.text.len());
    }

    fn span(&self, start: usize) -> Span {
        Span::new(start as u32, self.pos as u32)
    }

    fn token(&self, kind: Kind, start: usize) -> Token {
        Token {
            kind,
            start: start as u32,
            end: self.pos as u32,
        }
    }

    /// The next token, [`Kind::Eof`] at the end (at the end of the text).
    pub(crate) fn next(&mut self, errors: &mut Errors) -> Token {
        self.skip_space();
        let start = self.pos;
        let Some(&c) = self.bytes.get(start) else {
            return self.token(Kind::Eof, start);
        };
        let kind = match c {
            b'(' => self.one(Kind::LParen),
            b')' => self.one(Kind::RParen),
            b'=' => self.one(Kind::Equals),
            b'^' => {
                self.pos += 1;
                if self.bytes.get(self.pos) == Some(&b'^') {
                    self.pos += 1;
                    Kind::Carets
                } else {
                    errors.error(self.span(start), "Expected '^^'");
                    Kind::Bad
                }
            }
            b'<' => self.full_iri(errors),
            b'>' => {
                self.pos += 1;
                errors.error(self.span(start), "Unexpected '>'");
                Kind::Bad
            }
            b'"' => self.string(errors),
            b'@' => self.lang_tag(errors),
            _ => self.word(errors),
        };
        let mut token = self.token(kind, start);
        // Step 6: a token that does not end in a delimiter is followed by
        // white space, a comment, a delimiter or the end. A word stops at
        // all of these and at a quote, so only a quote can follow it.
        let ends_in_delimiter = matches!(
            kind,
            Kind::LParen | Kind::RParen | Kind::Equals | Kind::Carets | Kind::FullIri
        );
        if !ends_in_delimiter
            && kind != Kind::Bad
            && let Some(n) = self.text[self.pos..].chars().next()
            && !(n.is_ascii() && (is_space(n as u8) || is_delimiter(n as u8) || n == '#'))
        {
            errors.error(
                Span::new(token.start, (self.pos + n.len_utf8()) as u32),
                format!(
                    "Expected white space or a delimiter after '{}'",
                    shown(&self.text[start..self.pos])
                ),
            );
            token.kind = Kind::Bad;
        }
        token
    }

    fn one(&mut self, kind: Kind) -> Kind {
        self.pos += 1;
        kind
    }

    /// Past white space and comments.
    fn skip_space(&mut self) {
        loop {
            while self.bytes.get(self.pos).is_some_and(|&c| is_space(c)) {
                self.pos += 1;
            }
            if self.bytes.get(self.pos) != Some(&b'#') {
                return;
            }
            let start = self.pos;
            let rest = &self.bytes[start..];
            let len = rest
                .iter()
                .position(|&c| c == b'\n' || c == b'\r')
                .unwrap_or(rest.len());
            self.pos += len;
            self.comments.push(self.span(start));
        }
    }

    /// `<…>`. The characters an IRI reference never holds (a space, `"`,
    /// `{`, `}`, `|`, `^`, a backquote, `\`) are checked when the IRI is
    /// ([`oxiri`]); a line end, `<` or the end of the text before the `>`
    /// leave the IRI unterminated.
    fn full_iri(&mut self, errors: &mut Errors) -> Kind {
        let start = self.pos;
        let rest = &self.bytes[start + 1..];
        match rest
            .iter()
            .position(|&c| matches!(c, b'>' | b'<' | b'\n' | b'\r'))
        {
            Some(i) if rest[i] == b'>' => {
                self.pos = start + 1 + i + 1;
                Kind::FullIri
            }
            stop => {
                self.pos = start + 1 + stop.unwrap_or(rest.len());
                errors.error(self.span(start), "Expected '>' to end the IRI");
                Kind::Bad
            }
        }
    }

    /// `"…"`: `\"` and `\\` are the only escapes (section 2.3), and every
    /// character is an XML `Char`. One error per string for each kind of
    /// fault, so a string of a million bad escapes is one error.
    fn string(&mut self, errors: &mut Errors) -> Kind {
        let start = self.pos;
        self.pos += 1;
        let (mut bad_escape, mut bad_char) = (false, false);
        let mut fine = true;
        loop {
            let rest = &self.bytes[self.pos..];
            let Some(i) = rest
                .iter()
                .position(|&c| c == b'"' || c == b'\\' || (c < 0x20 && !is_space(c)) || c == 0xEF)
            else {
                self.pos = self.bytes.len();
                errors.error(
                    Span::new(start as u32, start as u32 + 1),
                    "Expected '\"' to end the string, found the end of the document",
                );
                return Kind::Bad;
            };
            self.pos += i;
            match self.bytes[self.pos] {
                b'"' => {
                    self.pos += 1;
                    return if fine { Kind::String } else { Kind::Bad };
                }
                b'\\' => {
                    let at = self.pos;
                    match self.bytes.get(at + 1) {
                        Some(b'"' | b'\\') => self.pos += 2,
                        next => {
                            let len = next.map_or(1, |_| {
                                1 + self.text[at + 1..].chars().next().map_or(0, char::len_utf8)
                            });
                            self.pos += len;
                            if !bad_escape {
                                bad_escape = true;
                                fine = false;
                                errors.error(
                                    Span::new(at as u32, (at + len) as u32),
                                    format!(
                                        "Invalid escape '{}': a string escapes only '\"' and '\\' (as \\\" and \\\\)",
                                        &self.text[at..at + len]
                                    ),
                                );
                            }
                        }
                    }
                }
                0xEF => {
                    // U+FFFE and U+FFFF are no XML `Char`s.
                    let c = self.text[self.pos..].chars().next().unwrap_or(' ');
                    let len = c.len_utf8();
                    if matches!(c, '\u{fffe}' | '\u{ffff}') && !bad_char {
                        bad_char = true;
                        fine = false;
                        errors.error(
                            Span::new(self.pos as u32, (self.pos + len) as u32),
                            format!("A string cannot hold U+{:04X}", c as u32),
                        );
                    }
                    self.pos += len;
                }
                c => {
                    if !bad_char {
                        bad_char = true;
                        fine = false;
                        errors.error(
                            Span::new(self.pos as u32, self.pos as u32 + 1),
                            format!("A string cannot hold the control character U+{c:04X}"),
                        );
                    }
                    self.pos += 1;
                }
            }
        }
    }

    /// The end of the word that starts at `self.pos`.
    fn word_end(&self) -> usize {
        let rest = &self.bytes[self.pos..];
        self.pos
            + rest
                .iter()
                .position(|&c| ends_word(c))
                .unwrap_or(rest.len())
    }

    /// `@` and a BCP 47 tag.
    fn lang_tag(&mut self, errors: &mut Errors) -> Kind {
        let start = self.pos;
        self.pos += 1;
        self.pos = self.word_end();
        let tag = &self.text[start + 1..self.pos];
        if tag.is_empty() {
            errors.error(self.span(start), "Expected a language tag after '@'");
            return Kind::Bad;
        }
        if let Err(e) = oxilangtag::LanguageTag::parse(tag) {
            errors.error(
                self.span(start),
                format!("Invalid language tag '{}': {e}", shown(tag)),
            );
            return Kind::Bad;
        }
        Kind::LangTag
    }

    /// A keyword, prefixed name, node ID or integer; else a [`Kind::Word`].
    fn word(&mut self, errors: &mut Errors) -> Kind {
        let start = self.pos;
        self.pos = self.word_end();
        let word = &self.text[start..self.pos];
        if let Some(label) = word.strip_prefix("_:") {
            if is_blank_label(label) {
                return Kind::NodeId;
            }
            errors.error(
                self.span(start),
                format!("Invalid node ID '{}'", shown(word)),
            );
            return Kind::Bad;
        }
        if let Some((prefix, local)) = word.split_once(':') {
            if is_pn_prefix(prefix) && (local.is_empty() || is_pn_local(local)) {
                return Kind::PName;
            }
            let why = if !is_pn_prefix(prefix) {
                "its prefix is no prefix name"
            } else {
                "its local part is no local name"
            };
            errors.error(
                self.span(start),
                format!("Invalid abbreviated IRI '{}': {why}", shown(word)),
            );
            return Kind::Bad;
        }
        if word.bytes().all(|c| c.is_ascii_digit()) {
            Kind::Integer
        } else if word.bytes().all(|c| c.is_ascii_alphabetic()) {
            Kind::Keyword
        } else {
            Kind::Word
        }
    }
}

/// SPARQL's `BLANK_NODE_LABEL` after the `_:`.
fn is_blank_label(label: &str) -> bool {
    let mut chars = label.chars();
    match chars.next() {
        Some(c) if is_pn_chars_u(c) || c.is_ascii_digit() => {}
        _ => return false,
    }
    let rest = chars.as_str();
    rest.chars().all(|c| c == '.' || is_pn_chars(c)) && !rest.ends_with('.')
}
