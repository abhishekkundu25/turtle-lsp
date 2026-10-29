//! Turtle 1.2 / TriG 1.2 / N-Triples 1.2 tokenizer. Never fails: anything
//! unreadable becomes an `Error` token so the parser can report it and keep
//! going. The token set is the union of the three syntaxes; the parser decides
//! what each one allows (`{`, `}` and `GRAPH` only mean something in TriG).

// The tables live in `names`; other modules still reach them here.
pub(crate) use super::names::{is_pn_chars, is_pn_chars_base, is_pn_chars_u};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Iri,
    PName,
    BNode,
    String,
    LangTag,
    Number,
    Boolean,
    A,
    PrefixDirective,  // @prefix
    BaseDirective,    // @base
    VersionDirective, // @version
    SparqlPrefix,     // PREFIX
    SparqlBase,       // BASE
    SparqlVersion,    // VERSION
    Dot,
    Semicolon,
    Comma,
    LBracket,
    RBracket,
    LParen,
    RParen,
    Caret2,      // ^^
    LTriple,     // <<
    RTriple,     // >>
    LTripleTerm, // <<(
    RTripleTerm, // )>>
    LAnnotation, // {|
    RAnnotation, // |}
    LBrace,      // { (TriG graph block)
    RBrace,      // }
    Graph,       // GRAPH (TriG; case-insensitive)
    Tilde,
    Comment,
    Error,
}

/// A token: 12 bytes, because a document holds one per few bytes of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// A [`Fault`] code for a malformed token, 0 for a well-formed one.
    error: u8,
    pub start: u32,
    pub end: u32,
}

const _: () = assert!(std::mem::size_of::<Token>() == 12);

impl Token {
    /// A well-formed token.
    pub fn new(kind: Kind, start: u32, end: u32) -> Token {
        Token {
            kind,
            error: 0,
            start,
            end,
        }
    }

    /// What is wrong with a malformed token (unterminated string, bad IRI,
    /// stray character), if anything.
    pub fn error(&self) -> Option<&'static str> {
        (self.error != 0).then(|| MESSAGES[self.error as usize])
    }

    /// The token as a stray word: `GRAPH` outside TriG.
    pub(super) fn into_unexpected_word(self) -> Token {
        Token {
            kind: Kind::Error,
            error: Fault::UnexpectedWord as u8,
            ..self
        }
    }

    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start as usize..self.end as usize]
    }
}

/// The message for a bare word that is not a keyword.
pub(crate) const UNEXPECTED_WORD: &str = "Unexpected word (did you forget a prefix, e.g. `ex:`?)";

/// What is wrong with a malformed token. A token keeps the code, and
/// [`MESSAGES`] holds the text at that index (0 is no fault).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Fault {
    UnexpectedCharacter = 1,
    UnexpectedWord,
    UnexpectedAt,
    UnterminatedString,
    UnterminatedLongString,
    BadStringEscape,
    UnterminatedIri,
    BadIriCharacter,
    NoBlankNodeLabel,
}

/// The text of each [`Fault`], at its code.
static MESSAGES: [&str; 10] = [
    "",
    "Unexpected character",
    UNEXPECTED_WORD,
    "Unexpected '@'",
    "Unterminated string",
    "Unterminated long string",
    "Invalid escape: use \\t \\b \\n \\r \\f \\\" \\' \\\\, \\uXXXX or \\UXXXXXXXX (no surrogates)",
    "Unterminated IRI: missing '>'",
    "Invalid character in IRI",
    "Blank node label expected after '_:'",
];

const LOCAL_ESC: &[u8] = b"_~.-!$&'()*+,;=/?#@%";

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    out: Vec<Token>,
}

#[cfg(test)]
thread_local! {
    /// Bytes this thread has lexed: an edit's cost without a clock.
    pub(crate) static LEXED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub fn tokenize(src: &str) -> Vec<Token> {
    #[cfg(test)]
    LEXED.with(|n| n.set(n.get() + src.len()));
    let mut lx = Lexer {
        src,
        b: src.as_bytes(),
        i: 0,
        out: Vec::with_capacity(src.len() / 6),
    };
    lx.run();
    lx.out
}

impl<'a> Lexer<'a> {
    fn push(&mut self, kind: Kind, end: usize, fault: Option<Fault>) {
        self.out.push(Token {
            kind,
            error: fault.map_or(0, |f| f as u8),
            start: self.i as u32,
            end: end as u32,
        });
        self.i = end;
    }

    fn char_at(&self, i: usize) -> Option<char> {
        self.src.get(i..).and_then(|s| s.chars().next())
    }

    fn starts(&self, s: &str) -> bool {
        self.b[self.i..].starts_with(s.as_bytes())
    }

    fn run(&mut self) {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            match c {
                b' ' | b'\t' | b'\r' | b'\n' => self.i += 1,
                b'#' => {
                    let end = self.b[self.i..]
                        .iter()
                        .position(|&b| b == b'\n' || b == b'\r')
                        .map_or(self.b.len(), |p| self.i + p);
                    self.push(Kind::Comment, end, None);
                }
                b'"' | b'\'' => self.string(c),
                b'<' => self.lt(),
                b'@' => self.at(),
                b'_' if self.b.get(self.i + 1) == Some(&b':') => self.bnode(),
                b'.' if !self.b.get(self.i + 1).is_some_and(|d| d.is_ascii_digit()) => {
                    self.push(Kind::Dot, self.i + 1, None)
                }
                b'0'..=b'9' | b'+' | b'-' | b'.' => {
                    if let Some(end) = self.number() {
                        self.push(Kind::Number, end, None)
                    } else {
                        self.push(Kind::Error, self.i + 1, Some(Fault::UnexpectedCharacter))
                    }
                }
                b';' => self.push(Kind::Semicolon, self.i + 1, None),
                b',' => self.push(Kind::Comma, self.i + 1, None),
                b'[' => self.push(Kind::LBracket, self.i + 1, None),
                b']' => self.push(Kind::RBracket, self.i + 1, None),
                b'(' => self.push(Kind::LParen, self.i + 1, None),
                b')' if self.starts(")>>") => self.push(Kind::RTripleTerm, self.i + 3, None),
                b')' => self.push(Kind::RParen, self.i + 1, None),
                b'^' if self.starts("^^") => self.push(Kind::Caret2, self.i + 2, None),
                b'>' if self.starts(">>") => self.push(Kind::RTriple, self.i + 2, None),
                b'{' if self.starts("{|") => self.push(Kind::LAnnotation, self.i + 2, None),
                b'{' => self.push(Kind::LBrace, self.i + 1, None),
                b'}' => self.push(Kind::RBrace, self.i + 1, None),
                b'|' if self.starts("|}") => self.push(Kind::RAnnotation, self.i + 2, None),
                b'~' => self.push(Kind::Tilde, self.i + 1, None),
                _ => self.name_or_word(),
            }
        }
    }

    fn string(&mut self, q: u8) {
        let long = self.b.len() >= self.i + 3 && self.b[self.i + 1] == q && self.b[self.i + 2] == q;
        let mut j = self.i + if long { 3 } else { 1 };
        let mut bad_escape = false;
        let escape_error = |e: bool| e.then_some(Fault::BadStringEscape);
        while j < self.b.len() {
            let c = self.b[j];
            if c == b'\\' {
                match self.b.get(j + 1) {
                    Some(b'\n' | b'\r') if !long => {
                        return self.push(Kind::String, j + 1, Some(Fault::UnterminatedString));
                    }
                    Some(b't' | b'b' | b'n' | b'r' | b'f' | b'"' | b'\'' | b'\\') => j += 2,
                    Some(&u @ (b'u' | b'U')) => {
                        let n = if u == b'u' { 4 } else { 8 };
                        if self.escaped_char(j + 2, n).is_none() {
                            bad_escape = true;
                        }
                        j += 2;
                    }
                    _ => {
                        bad_escape = true;
                        j += 2;
                    }
                }
                continue;
            }
            if long {
                if c == q && self.b.get(j + 1) == Some(&q) && self.b.get(j + 2) == Some(&q) {
                    return self.push(Kind::String, j + 3, escape_error(bad_escape));
                }
            } else if c == q {
                return self.push(Kind::String, j + 1, escape_error(bad_escape));
            } else if c == b'\n' || c == b'\r' {
                return self.push(Kind::String, j, Some(Fault::UnterminatedString));
            }
            j += 1;
        }
        let end = self.b.len();
        self.push(
            Kind::String,
            end,
            Some(if long {
                Fault::UnterminatedLongString
            } else {
                Fault::UnterminatedString
            }),
        )
    }

    fn lt(&mut self) {
        if self.starts("<<(") {
            return self.push(Kind::LTripleTerm, self.i + 3, None);
        }
        if self.starts("<<") {
            return self.push(Kind::LTriple, self.i + 2, None);
        }
        let mut j = self.i + 1;
        let mut bad = false;
        while j < self.b.len() {
            match self.b[j] {
                b'>' => {
                    return self.push(Kind::Iri, j + 1, bad.then_some(Fault::BadIriCharacter));
                }
                b' ' | b'\t' | b'\r' | b'\n' => break,
                b'<' | b'"' | b'{' | b'}' | b'|' | b'^' | b'`' => bad = true,
                b'\\' => {
                    // UCHAR only, and it must not spell a character IRIs forbid.
                    let c = match self.b.get(j + 1) {
                        Some(b'u') => self.escaped_char(j + 2, 4),
                        Some(b'U') => self.escaped_char(j + 2, 8),
                        _ => None,
                    };
                    if !c.is_some_and(|c| c > ' ' && !"<>\"{}|^`\\".contains(c)) {
                        bad = true
                    }
                }
                c if c < 0x20 => bad = true,
                _ => {}
            }
            j += 1;
        }
        self.push(Kind::Error, j, Some(Fault::UnterminatedIri))
    }

    /// The character `n` hex digits at `at` encode, if they do (surrogates
    /// do not).
    fn escaped_char(&self, at: usize, n: usize) -> Option<char> {
        let hex = self.src.get(at..at + n)?;
        if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        char::from_u32(u32::from_str_radix(hex, 16).ok()?)
    }

    fn hex(&self, at: usize, n: usize) -> bool {
        self.b
            .get(at..at + n)
            .is_some_and(|s| s.iter().all(u8::is_ascii_hexdigit))
    }

    fn at(&mut self) {
        let rest = &self.b[self.i + 1..];
        let word_end = rest
            .iter()
            .position(|c| !c.is_ascii_alphanumeric() && *c != b'-')
            .unwrap_or(rest.len());
        let word = &rest[..word_end];
        let kind = match word {
            b"prefix" => Some(Kind::PrefixDirective),
            b"base" => Some(Kind::BaseDirective),
            b"version" => Some(Kind::VersionDirective),
            _ => None,
        };
        if let Some(k) = kind {
            return self.push(k, self.i + 1 + word_end, None);
        }
        // LANGTAG: @[a-zA-Z]+ ('-' [a-zA-Z0-9]+)* ('--' [a-zA-Z]+)?
        let mut j = self.i + 1;
        let alpha = |c: &u8| c.is_ascii_alphabetic();
        let start = j;
        while self.b.get(j).is_some_and(alpha) {
            j += 1;
        }
        if j == start {
            return self.push(Kind::Error, self.i + 1, Some(Fault::UnexpectedAt));
        }
        loop {
            if self.b.get(j) == Some(&b'-')
                && self.b.get(j + 1).is_some_and(u8::is_ascii_alphanumeric)
            {
                j += 1;
                while self.b.get(j).is_some_and(u8::is_ascii_alphanumeric) {
                    j += 1;
                }
            } else if self.b.get(j) == Some(&b'-')
                && self.b.get(j + 1) == Some(&b'-')
                && self.b.get(j + 2).is_some_and(alpha)
            {
                j += 2;
                while self.b.get(j).is_some_and(alpha) {
                    j += 1;
                }
                break;
            } else {
                break;
            }
        }
        self.push(Kind::LangTag, j, None)
    }

    fn bnode(&mut self) {
        let mut j = self.i + 2;
        match self.char_at(j) {
            Some(c) if is_pn_chars_u(c) || c.is_ascii_digit() => j += c.len_utf8(),
            _ => return self.push(Kind::Error, j, Some(Fault::NoBlankNodeLabel)),
        }
        let mut last_ok = j;
        while let Some(c) = self.char_at(j) {
            if is_pn_chars(c) {
                j += c.len_utf8();
                last_ok = j;
            } else if c == '.' {
                j += 1;
            } else {
                break;
            }
        }
        self.push(Kind::BNode, last_ok, None)
    }

    fn number(&self) -> Option<usize> {
        let b = self.b;
        let mut j = self.i;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let d1 = b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        j += d1;
        let mut d2 = 0;
        let save = j;
        if b.get(j) == Some(&b'.') {
            d2 = b[j + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if d2 > 0 || (d1 > 0 && self.exponent(j + 1).is_some()) {
                j += 1 + d2;
            }
        }
        if d1 == 0 && d2 == 0 {
            return None;
        }
        if let Some(e) = self.exponent(j) {
            return Some(e);
        }
        if j > save && d2 == 0 {
            // "1." without exponent: the dot ends the statement.
            return Some(save);
        }
        Some(j)
    }

    fn exponent(&self, j: usize) -> Option<usize> {
        let b = self.b;
        if !matches!(b.get(j), Some(b'e' | b'E')) {
            return None;
        }
        let mut k = j + 1;
        if matches!(b.get(k), Some(b'+' | b'-')) {
            k += 1;
        }
        let n = b
            .get(k..)?
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
        (n > 0).then_some(k + n)
    }

    /// PNAME_NS / PNAME_LN, or a bare word (a, true, false, PREFIX, BASE, VERSION, GRAPH).
    fn name_or_word(&mut self) {
        let start = self.i;
        let mut j = start;
        // PN_PREFIX
        if let Some(c) = self.char_at(j).filter(|c| is_pn_chars_base(*c)) {
            j += c.len_utf8();
            let mut last_ok = j;
            while let Some(c) = self.char_at(j) {
                if is_pn_chars(c) {
                    j += c.len_utf8();
                    last_ok = j;
                } else if c == '.' {
                    j += 1;
                } else {
                    break;
                }
            }
            j = last_ok;
        }
        if self.b.get(j) != Some(&b':') {
            return self.word(start, j);
        }
        j += 1;
        // PN_LOCAL
        let mut last_ok = j;
        let mut first = true;
        while let Some(c) = self.char_at(j) {
            let plx = self.plx(j);
            if let Some(len) = plx {
                j += len;
                last_ok = j;
            } else if is_pn_chars_u(c)
                || c == ':'
                || c.is_ascii_digit()
                || (!first && is_pn_chars(c))
            {
                j += c.len_utf8();
                last_ok = j;
            } else if c == '.' && !first {
                j += 1;
            } else {
                break;
            }
            first = false;
        }
        self.push(Kind::PName, last_ok, None)
    }

    fn plx(&self, j: usize) -> Option<usize> {
        match self.b.get(j)? {
            b'%' if self.hex(j + 1, 2) => Some(3),
            b'\\' if self.b.get(j + 1).is_some_and(|c| LOCAL_ESC.contains(c)) => Some(2),
            _ => None,
        }
    }

    fn word(&mut self, start: usize, mut j: usize) {
        if j == start {
            // Not a name start: unknown character.
            let len = self.char_at(start).map_or(1, char::len_utf8);
            return self.push(Kind::Error, start + len, Some(Fault::UnexpectedCharacter));
        }
        while self.b.get(j).is_some_and(u8::is_ascii_alphanumeric) {
            j += 1;
        }
        let w = &self.src[start..j];
        let kind = match w {
            "a" => Kind::A,
            "true" | "false" => Kind::Boolean,
            _ if w.eq_ignore_ascii_case("PREFIX") => Kind::SparqlPrefix,
            _ if w.eq_ignore_ascii_case("BASE") => Kind::SparqlBase,
            _ if w.eq_ignore_ascii_case("VERSION") => Kind::SparqlVersion,
            _ if w.eq_ignore_ascii_case("GRAPH") => Kind::Graph,
            _ => return self.push(Kind::Error, j, Some(Fault::UnexpectedWord)),
        };
        self.push(kind, j, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(Kind, &str)> {
        tokenize(s)
            .into_iter()
            .map(|t| (t.kind, t.text(s)))
            .collect()
    }

    #[test]
    fn basics() {
        let k = kinds("@prefix ex: <http://e/> .\nex:a a ex:B ; ex:n 1.5, -2, 3e4, 1. # c\n");
        assert_eq!(k[0], (Kind::PrefixDirective, "@prefix"));
        assert_eq!(k[1], (Kind::PName, "ex:"));
        assert_eq!(k[2], (Kind::Iri, "<http://e/>"));
        assert!(k.contains(&(Kind::Number, "1.5")));
        assert!(k.contains(&(Kind::Number, "-2")));
        assert!(k.contains(&(Kind::Number, "3e4")));
        assert!(k.contains(&(Kind::Number, "1")));
        assert_eq!(k.last().unwrap().0, Kind::Comment);
    }

    #[test]
    fn pname_trailing_dot_and_escapes() {
        let k = kinds("ex:a.b. ex:x\\.y :z");
        assert_eq!(k[0], (Kind::PName, "ex:a.b"));
        assert_eq!(k[1], (Kind::Dot, "."));
        assert_eq!(k[2], (Kind::PName, "ex:x\\.y"));
        assert_eq!(k[3], (Kind::PName, ":z"));
    }

    #[test]
    fn errors_do_not_stop_lexing() {
        let k = kinds("\"open\nex:a <bad iri> ex:b");
        assert_eq!(k[0].0, Kind::String);
        assert!(tokenize("\"open\n")[0].error().is_some());
        assert!(k.contains(&(Kind::PName, "ex:b")));
    }

    #[test]
    fn backslash_newline_ends_a_short_string() {
        for nl in ["\n", "\r", "\r\n"] {
            let src = format!("ex:a ex:p \"x\\{nl}ex:b ex:p ex:o .");
            let k = tokenize(&src);
            let s = k.iter().find(|t| t.kind == Kind::String).unwrap();
            assert_eq!(s.error(), Some("Unterminated string"));
            assert_eq!(s.end as usize, src.find(nl).unwrap(), "{nl:?}");
            assert!(
                k.iter().any(|t| t.text(&src) == "ex:b"),
                "the next statement survives for {nl:?}"
            );
        }
        // A long string still runs across a backslash-newline.
        let src = "\"\"\"a\\\nb\"\"\" .";
        let k = tokenize(src);
        assert_eq!(k[0].text(src), &src[..src.len() - 2]);
    }

    #[test]
    fn comments_end_at_cr_or_lf() {
        for nl in ["\n", "\r", "\r\n"] {
            let src = format!("# c{nl}ex:a ex:p ex:o .");
            let k = tokenize(&src);
            assert_eq!(k[0].kind, Kind::Comment);
            assert_eq!(k[0].text(&src), "# c", "{nl:?}");
            assert_eq!(k[1].text(&src), "ex:a");
        }
    }

    #[test]
    fn escapes_are_checked() {
        let err = |s: &str| tokenize(s)[0].error().is_some();
        assert!(!err(r#""a\tb\u00e9\U0001F600\"\'\\""#));
        assert!(err(r#""a\zb""#));
        assert!(err(r#""\uD83C\uDCA1""#));
        assert!(err(r#""\u12""#));
        assert!(!err(r"<http://e/\u00e9>"));
        assert!(err(r"<http://e/\u0020>"));
        assert!(err(r"<http://e/\uD800>"));
        // A long string's content cannot end with a quote: the fourth `"`
        // opens a new string.
        let k = kinds(r#""""abc"""" ."#);
        assert_eq!(k[0], (Kind::String, r#""""abc""""#));
        assert_eq!(k[1].0, Kind::String);
    }

    #[test]
    fn every_fault_code_reads_back_as_its_text() {
        let cases = [
            ("$", Fault::UnexpectedCharacter, "Unexpected character"),
            ("+x", Fault::UnexpectedCharacter, "Unexpected character"),
            ("foo", Fault::UnexpectedWord, UNEXPECTED_WORD),
            ("@1", Fault::UnexpectedAt, "Unexpected '@'"),
            ("\"open", Fault::UnterminatedString, "Unterminated string"),
            (
                "\"\"\"open",
                Fault::UnterminatedLongString,
                "Unterminated long string",
            ),
            (
                r#""a\zb""#,
                Fault::BadStringEscape,
                r#"Invalid escape: use \t \b \n \r \f \" \' \\, \uXXXX or \UXXXXXXXX (no surrogates)"#,
            ),
            (
                "<http://e/",
                Fault::UnterminatedIri,
                "Unterminated IRI: missing '>'",
            ),
            (
                "<http://e/{>",
                Fault::BadIriCharacter,
                "Invalid character in IRI",
            ),
            (
                "_: ",
                Fault::NoBlankNodeLabel,
                "Blank node label expected after '_:'",
            ),
        ];
        let mut seen = [false; MESSAGES.len()];
        for (src, fault, text) in cases {
            let t = tokenize(src)[0];
            assert_eq!(t.error, fault as u8, "{src}");
            assert_eq!(t.error(), Some(text), "{src}");
            seen[fault as usize] = true;
        }
        assert!(
            seen[1..].iter().all(|&s| s),
            "a code with no case: {seen:?}"
        );
        assert_eq!(Token::new(Kind::Iri, 0, 1).error(), None);
        let word = Token::new(Kind::Graph, 0, 5).into_unexpected_word();
        assert_eq!(
            (word.kind, word.error()),
            (Kind::Error, Some(UNEXPECTED_WORD))
        );
    }

    #[test]
    fn rdf12() {
        let k = kinds(
            "<<( ex:a ex:b ex:c )>> << ex:a ex:b ex:c ~ ex:r >> {| ex:p ex:o |} \"x\"@en--ltr",
        );
        assert_eq!(k[0].0, Kind::LTripleTerm);
        assert_eq!(k[4].0, Kind::RTripleTerm);
        assert!(k.iter().any(|t| t.0 == Kind::Tilde));
        assert!(k.contains(&(Kind::LangTag, "@en--ltr")));
    }

    #[test]
    fn trig_braces_and_graph_keyword() {
        let k = kinds("GRAPH ex:g { ex:a ex:b ex:c {| ex:p ex:o |} }{}graph:x Graph");
        assert_eq!(k[0].0, Kind::Graph);
        assert_eq!(k[2], (Kind::LBrace, "{"));
        assert_eq!(k[6], (Kind::LAnnotation, "{|"));
        assert_eq!(k[9], (Kind::RAnnotation, "|}"));
        assert_eq!(k[10], (Kind::RBrace, "}"));
        assert_eq!(k[11], (Kind::LBrace, "{"));
        assert_eq!(k[12], (Kind::RBrace, "}"));
        assert_eq!(k[13], (Kind::PName, "graph:x"));
        assert_eq!(k[14].0, Kind::Graph);
    }
}
