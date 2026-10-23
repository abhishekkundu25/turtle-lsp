//! SPARQL 1.2 tokenizer. Never fails: a character no token starts with, an
//! unterminated string and a bare word that is no keyword become `Error`
//! tokens, and lexing goes on after them. Comments are tokens. Whitespace is
//! not. Spans are byte ranges, never empty, never overlapping, always on
//! character boundaries.
//!
//! `<` right after an operand (a variable, literal, name, IRI or `)`) inside
//! an expression's parentheses is the `<` or `<=` operator, as `spargebra`
//! reads it (`?a<=ex:b&&?c>?a`). Elsewhere `<` starts an IRI when an IRI
//! reference follows (`<http://e/>`), else it is an operator (`<`, `<=`,
//! `<<`, `<<(`). Signs are their own tokens
//! (`-1` is `-` and `1`), as in the grammar's `INTEGER_NEGATIVE`. A word
//! followed by `:` is a prefixed name (`select:x` is a prefixed name, not a
//! keyword), except `PREFIX:` before an IRI, which is the keyword and the
//! empty prefix.

use super::{Token, TokenKind};
use crate::syntax::Span;
use crate::syntax::names::{is_pn_chars, is_pn_chars_base, is_pn_chars_u};

/// Keywords, upper case. Matched case-insensitively.
const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "AS",
    "ASC",
    "ASK",
    "BASE",
    "BIND",
    "BY",
    "CLEAR",
    "CONSTRUCT",
    "COPY",
    "CREATE",
    "DATA",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DESCRIBE",
    "DISTINCT",
    "DROP",
    "EXISTS",
    "FILTER",
    "FROM",
    "GRAPH",
    "GROUP",
    "HAVING",
    "IN",
    "INSERT",
    "INTO",
    "LATERAL",
    "LIMIT",
    "LOAD",
    "MINUS",
    "MOVE",
    "NAMED",
    "NOT",
    "OFFSET",
    "OPTIONAL",
    "ORDER",
    "PREFIX",
    "REDUCED",
    "SELECT",
    "SEPARATOR",
    "SERVICE",
    "SILENT",
    "TO",
    "UNDEF",
    "UNION",
    "USING",
    "VALUES",
    "VERSION",
    "WHERE",
    "WITH",
];

/// Built-in function names in their grammar spelling (what the formatter
/// writes). Aggregates are listed in [`AGGREGATES`].
const FUNCTIONS: &[&str] = &[
    "ABS",
    "BNODE",
    "BOUND",
    "CEIL",
    "COALESCE",
    "CONCAT",
    "CONTAINS",
    "DATATYPE",
    "DAY",
    "ENCODE_FOR_URI",
    "FLOOR",
    "HOURS",
    "IF",
    "IRI",
    "LANG",
    "LANGDIR",
    "LCASE",
    "MD5",
    "MINUTES",
    "MONTH",
    "NOW",
    "OBJECT",
    "PREDICATE",
    "RAND",
    "REGEX",
    "REPLACE",
    "ROUND",
    "SECONDS",
    "SHA1",
    "SHA256",
    "SHA384",
    "SHA512",
    "STR",
    "STRAFTER",
    "STRBEFORE",
    "STRDT",
    "STRENDS",
    "STRLANG",
    "STRLANGDIR",
    "STRLEN",
    "STRSTARTS",
    "STRUUID",
    "SUBJECT",
    "SUBSTR",
    "TIMEZONE",
    "TRIPLE",
    "TZ",
    "UCASE",
    "URI",
    "UUID",
    "YEAR",
    "hasLANG",
    "hasLANGDIR",
    "isBLANK",
    "isIRI",
    "isLITERAL",
    "isNUMERIC",
    "isTRIPLE",
    "isURI",
    "langMatches",
    "sameTerm",
];

/// Aggregate function names in their grammar spelling.
pub(crate) const AGGREGATES: &[&str] = &[
    "AVG",
    "COUNT",
    "GROUP_CONCAT",
    "MAX",
    "MIN",
    "SAMPLE",
    "SUM",
];

/// The keyword `word` is, in upper case, if it is one.
pub(crate) fn keyword(word: &str) -> Option<&'static str> {
    KEYWORDS
        .iter()
        .find(|k| k.eq_ignore_ascii_case(word))
        .copied()
}

/// The grammar spelling of built-in function or aggregate `word` (`STRLEN`,
/// `isBLANK`, `langMatches`, `GROUP_CONCAT`), if it is one.
pub(crate) fn function(word: &str) -> Option<&'static str> {
    FUNCTIONS
        .iter()
        .chain(AGGREGATES)
        .find(|k| k.eq_ignore_ascii_case(word))
        .copied()
}

/// Whether `word` names an aggregate (`COUNT`, `SUM`, ...).
pub(crate) fn is_aggregate(word: &str) -> bool {
    AGGREGATES.iter().any(|k| k.eq_ignore_ascii_case(word))
}

/// Every keyword (upper case) and every built-in function and aggregate (in
/// its grammar spelling), for completion.
pub(crate) fn words() -> impl Iterator<Item = (&'static str, TokenKind)> {
    KEYWORDS.iter().map(|k| (*k, TokenKind::Keyword)).chain(
        FUNCTIONS
            .iter()
            .chain(AGGREGATES)
            .map(|f| (*f, TokenKind::Function)),
    )
}

fn is_varname_rest(c: char) -> bool {
    is_pn_chars_u(c)
        || c.is_ascii_digit()
        || c == '\u{B7}'
        || ('\u{300}'..='\u{36F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}

/// Characters that end an IRI reference without closing it.
fn bad_in_iri(c: char) -> bool {
    matches!(c, '<' | '"' | '{' | '}' | '|' | '^' | '`') || c <= ' '
}

/// What an open bracket holds.
#[derive(Clone, Copy, PartialEq)]
enum Open {
    /// An expression: `FILTER(`, `BIND(`, a function's arguments, a `(` in a
    /// SELECT clause or a solution modifier, any `(` inside an expression.
    Expr,
    /// A group `{`, with whether a SELECT clause or solution modifier was
    /// open before it.
    Group(bool),
    /// A collection, `[`, `<<(`, a VALUES row.
    Other,
}

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    out: Vec<Token>,
    open: Vec<Open>,
    /// In a SELECT clause or a solution modifier (GROUP BY, HAVING, ORDER BY).
    clause: bool,
}

impl Lexer<'_> {
    fn char_at(&self, i: usize) -> Option<char> {
        self.src.get(i..)?.chars().next()
    }

    fn push(&mut self, kind: TokenKind, start: usize, end: usize) {
        debug_assert!(start < end);
        let text = &self.src[start..end];
        match (kind, text) {
            (TokenKind::Punct, "(") => {
                let kw = |t: Option<&Token>, w: &str| {
                    t.is_some_and(|t| {
                        t.kind == TokenKind::Keyword && t.text(self.src).eq_ignore_ascii_case(w)
                    })
                };
                let mut before = self
                    .out
                    .iter()
                    .rev()
                    .filter(|t| t.kind != TokenKind::Comment);
                let (prev, prev2) = (before.next(), before.next());
                let call = prev
                    .is_some_and(|t| matches!(t.kind, TokenKind::Iri | TokenKind::PName))
                    && kw(prev2, "FILTER");
                let expr = self.open.last() == Some(&Open::Expr)
                    || self.clause
                    || kw(prev, "FILTER")
                    || kw(prev, "BIND")
                    || prev.is_some_and(|t| t.kind == TokenKind::Function)
                    || call;
                self.open.push(if expr { Open::Expr } else { Open::Other });
            }
            (TokenKind::Punct, "[" | "<<(") => self.open.push(Open::Other),
            (TokenKind::Punct, ")" | "]" | ")>>") => {
                if matches!(self.open.last(), Some(Open::Expr | Open::Other)) {
                    self.open.pop();
                }
            }
            (TokenKind::Punct, "{") => {
                self.open.push(Open::Group(self.clause));
                self.clause = false;
            }
            (TokenKind::Punct, "}") => {
                while let Some(o) = self.open.pop() {
                    if let Open::Group(clause) = o {
                        self.clause = clause;
                        break;
                    }
                }
            }
            (TokenKind::Keyword, _) => match keyword(text) {
                Some("SELECT" | "GROUP" | "ORDER" | "HAVING") => self.clause = true,
                Some("WHERE" | "FROM" | "LIMIT" | "OFFSET" | "VALUES") => self.clause = false,
                _ => {}
            },
            _ => {}
        }
        self.out.push(Token {
            kind,
            span: Span::new(start as u32, end as u32),
        });
    }

    /// Whether a `<` here is a comparison: it follows an operand inside an
    /// expression, where `spargebra` reads no IRI.
    fn comparison(&self) -> bool {
        self.open.last() == Some(&Open::Expr)
            && self
                .out
                .iter()
                .rev()
                .find(|t| t.kind != TokenKind::Comment)
                .is_some_and(|t| match t.kind {
                    TokenKind::Var
                    | TokenKind::String
                    | TokenKind::Number
                    | TokenKind::Boolean
                    | TokenKind::LangTag
                    | TokenKind::Iri
                    | TokenKind::PName => true,
                    TokenKind::Punct => matches!(t.text(self.src), ")" | ")>>"),
                    _ => false,
                })
    }

    /// End of the IRI reference starting with `<` at `i`, if one does.
    fn iri_end(&self, i: usize) -> Option<usize> {
        let mut j = i + 1;
        while let Some(c) = self.char_at(j) {
            if c == '>' {
                return Some(j + 1);
            }
            if bad_in_iri(c) {
                return None;
            }
            j += c.len_utf8();
        }
        None
    }

    /// End of a string starting with its quote at `i`; `Err` with the end of
    /// what was read when it is not terminated.
    fn string_end(&self, i: usize) -> Result<usize, usize> {
        let q = self.b[i];
        let long = self.b.get(i + 1) == Some(&q) && self.b.get(i + 2) == Some(&q);
        if long {
            let mut j = i + 3;
            while j < self.b.len() {
                match self.b[j] {
                    b'\\' => j += 2,
                    c if c == q
                        && self.b.get(j + 1) == Some(&q)
                        && self.b.get(j + 2) == Some(&q) =>
                    {
                        // `""""` ends with the last three quotes.
                        let mut end = j + 3;
                        while self.b.get(end) == Some(&q) && end - j < 5 {
                            end += 1;
                        }
                        return Ok(end);
                    }
                    _ => j += 1,
                }
            }
            Err(self.b.len())
        } else if self.b.get(i + 1) == Some(&q) {
            Ok(i + 2)
        } else {
            let mut j = i + 1;
            while j < self.b.len() {
                match self.b[j] {
                    b'\\' if !matches!(self.b.get(j + 1), None | Some(b'\n' | b'\r')) => j += 2,
                    b'\n' | b'\r' => return Err(j),
                    c if c == q => return Ok(j + 1),
                    _ => j += 1,
                }
            }
            Err(self.b.len())
        }
    }

    /// End of `PN_LOCAL` from `j` (`j` itself when there is none).
    fn local_end(&self, mut j: usize) -> usize {
        let start = j;
        let mut end = j;
        while let Some(c) = self.char_at(j) {
            let first = j == start;
            if c == '%' {
                let hex = |k: usize| self.b.get(k).is_some_and(u8::is_ascii_hexdigit);
                if hex(j + 1) && hex(j + 2) {
                    j += 3;
                    end = j;
                    continue;
                }
                break;
            }
            if c == '\\' {
                if self
                    .char_at(j + 1)
                    .is_some_and(|e| "_~.-!$&'()*+,;=/?#@%".contains(e))
                {
                    j += 2;
                    end = j;
                    continue;
                }
                break;
            }
            if c == '.' && !first {
                j += 1;
                continue;
            }
            let ok = if first {
                is_pn_chars_u(c) || c == ':' || c.is_ascii_digit()
            } else {
                is_pn_chars(c) || c == ':'
            };
            if !ok {
                break;
            }
            j += c.len_utf8();
            end = j;
        }
        end
    }

    /// End of `PN_PREFIX` starting at `i` (a `PN_CHARS_BASE` character).
    fn prefix_end(&self, i: usize) -> usize {
        let mut j = i;
        let mut end = i;
        while let Some(c) = self.char_at(j) {
            if c == '.' && j > i {
                j += 1;
                continue;
            }
            if !(if j == i {
                is_pn_chars_base(c)
            } else {
                is_pn_chars(c)
            }) {
                break;
            }
            j += c.len_utf8();
            end = j;
        }
        end
    }

    fn number(&mut self, i: usize) -> usize {
        let b = self.b;
        let digits = |mut k: usize| {
            while b.get(k).is_some_and(u8::is_ascii_digit) {
                k += 1;
            }
            k
        };
        let int_end = digits(i);
        let mut end = int_end;
        let mut frac = false;
        if b.get(end) == Some(&b'.') {
            let f = digits(end + 1);
            if f > end + 1 {
                end = f;
                frac = true;
            } else if int_end > i && exponent(b, end + 1).is_some() {
                // `1.e5`
                end += 1;
                frac = true;
            }
        }
        if (int_end > i || frac)
            && let Some(e) = exponent(b, end)
        {
            end = e;
        }
        end
    }

    fn run(&mut self) {
        let b = self.b;
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => i += 1,
                b'#' => {
                    let end = b[i..]
                        .iter()
                        .position(|&c| c == b'\n' || c == b'\r')
                        .map_or(b.len(), |p| i + p);
                    self.push(TokenKind::Comment, i, end);
                    i = end;
                }
                b'<' => {
                    if self.comparison() {
                        let len = if b[i..].starts_with(b"<=") { 2 } else { 1 };
                        self.push(TokenKind::Punct, i, i + len);
                        i += len;
                    } else if let Some(end) = self.iri_end(i) {
                        self.push(TokenKind::Iri, i, end);
                        i = end;
                    } else {
                        let len = if b[i..].starts_with(b"<<(") {
                            3
                        } else if b[i..].starts_with(b"<<") || b[i..].starts_with(b"<=") {
                            2
                        } else {
                            1
                        };
                        self.push(TokenKind::Punct, i, i + len);
                        i += len;
                    }
                }
                b'"' | b'\'' => match self.string_end(i) {
                    Ok(end) => {
                        self.push(TokenKind::String, i, end);
                        i = end;
                    }
                    Err(end) => {
                        self.push(TokenKind::Error, i, end);
                        i = end;
                    }
                },
                b'0'..=b'9' => {
                    let end = self.number(i);
                    self.push(TokenKind::Number, i, end);
                    i = end;
                }
                b'.' if b.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                    let end = self.number(i);
                    self.push(TokenKind::Number, i, end);
                    i = end;
                }
                b'?' | b'$' => {
                    let first = self
                        .char_at(i + 1)
                        .filter(|&c| is_pn_chars_u(c) || c.is_ascii_digit());
                    if first.is_some() {
                        let mut j = i + 1;
                        while let Some(c) = self.char_at(j).filter(|&c| is_varname_rest(c)) {
                            j += c.len_utf8();
                        }
                        self.push(TokenKind::Var, i, j);
                        i = j;
                    } else {
                        let kind = if c == b'?' {
                            TokenKind::Punct
                        } else {
                            TokenKind::Error
                        };
                        self.push(kind, i, i + 1);
                        i += 1;
                    }
                }
                b'@' => {
                    let letters = |mut k: usize, digits: bool| {
                        let from = k;
                        while b.get(k).is_some_and(|c| {
                            c.is_ascii_alphabetic() || (digits && c.is_ascii_digit())
                        }) {
                            k += 1;
                        }
                        (k > from).then_some(k)
                    };
                    match letters(i + 1, false) {
                        Some(mut end) => {
                            while b.get(end) == Some(&b'-') && b.get(end + 1) != Some(&b'-') {
                                match letters(end + 1, true) {
                                    Some(e) => end = e,
                                    None => break,
                                }
                            }
                            if b[end..].starts_with(b"--")
                                && let Some(e) = letters(end + 2, false)
                            {
                                end = e;
                            }
                            self.push(TokenKind::LangTag, i, end);
                            i = end;
                        }
                        None => {
                            self.push(TokenKind::Error, i, i + 1);
                            i += 1;
                        }
                    }
                }
                b'_' if b.get(i + 1) == Some(&b':') => {
                    let mut j = i + 2;
                    let mut end = j;
                    if self
                        .char_at(j)
                        .is_some_and(|c| is_pn_chars_u(c) || c.is_ascii_digit())
                    {
                        while let Some(c) = self.char_at(j) {
                            if c == '.' {
                                j += 1;
                                continue;
                            }
                            if !is_pn_chars(c) {
                                break;
                            }
                            j += c.len_utf8();
                            end = j;
                        }
                    }
                    let kind = if end > i + 2 {
                        TokenKind::BlankNode
                    } else {
                        TokenKind::Error
                    };
                    self.push(kind, i, end);
                    i = end;
                }
                b':' => {
                    let end = self.local_end(i + 1);
                    self.push(TokenKind::PName, i, end);
                    i = end;
                }
                _ if c >= 0x80 || c.is_ascii_alphabetic() => {
                    let ch = self.char_at(i).unwrap_or('\u{FFFD}');
                    if !is_pn_chars_base(ch) {
                        self.push(TokenKind::Error, i, i + ch.len_utf8());
                        i += ch.len_utf8();
                        continue;
                    }
                    let pend = self.prefix_end(i);
                    let after_colon = self.char_at(pend + 1);
                    let prefix_kw = src_eq(&self.src[i..pend], "PREFIX")
                        && after_colon.is_none_or(|c| c.is_whitespace() || c == '<');
                    if b.get(pend) == Some(&b':') && !prefix_kw {
                        let end = self.local_end(pend + 1);
                        self.push(TokenKind::PName, i, end);
                        i = end;
                        continue;
                    }
                    let wend = b[i..]
                        .iter()
                        .position(|c| !(c.is_ascii_alphanumeric() || *c == b'_'))
                        .map_or(b.len(), |p| i + p);
                    if wend == i {
                        // A non-ASCII name that is no prefixed name.
                        self.push(TokenKind::Error, i, pend);
                        i = pend;
                        continue;
                    }
                    let w = &self.src[i..wend];
                    let kind = if w == "a" {
                        TokenKind::A
                    } else if w.eq_ignore_ascii_case("true") || w.eq_ignore_ascii_case("false") {
                        TokenKind::Boolean
                    } else if keyword(w).is_some() {
                        TokenKind::Keyword
                    } else if function(w).is_some() {
                        TokenKind::Function
                    } else {
                        TokenKind::Error
                    };
                    self.push(kind, i, wend);
                    i = wend;
                }
                _ => {
                    let two = |s: &[u8]| b[i..].starts_with(s);
                    let len = if two(b")>>") {
                        3
                    } else if two(b">>")
                        || two(b">=")
                        || two(b"{|")
                        || two(b"|}")
                        || two(b"||")
                        || two(b"&&")
                        || two(b"!=")
                        || two(b"^^")
                    {
                        2
                    } else if b"{}()[].,;*+-/!^|=~>".contains(&c) {
                        1
                    } else {
                        0
                    };
                    if len == 0 {
                        let n = self.char_at(i).map_or(1, char::len_utf8);
                        self.push(TokenKind::Error, i, i + n);
                        i += n;
                    } else {
                        self.push(TokenKind::Punct, i, i + len);
                        i += len;
                    }
                }
            }
        }
    }
}

fn src_eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// End of an exponent (`e`, sign, digits) at `k`.
fn exponent(b: &[u8], k: usize) -> Option<usize> {
    if !matches!(b.get(k), Some(b'e' | b'E')) {
        return None;
    }
    let mut j = k + 1;
    if matches!(b.get(j), Some(b'+' | b'-')) {
        j += 1;
    }
    let from = j;
    while b.get(j).is_some_and(u8::is_ascii_digit) {
        j += 1;
    }
    (j > from).then_some(j)
}

/// Every token of `src`, comments included, in order.
pub fn tokenize(src: &str) -> Vec<Token> {
    let mut l = Lexer {
        src,
        b: src.as_bytes(),
        out: Vec::with_capacity(src.len() / 4),
        open: Vec::new(),
        clause: false,
    };
    l.run();
    l.out
}

/// (kind, span) of every token of `src`, for `syntax::names::token_edits`.
pub(crate) fn kinds(src: &str) -> Vec<(TokenKind, Span)> {
    tokenize(src).iter().map(|t| (t.kind, t.span)).collect()
}
