//! Turtle 1.2 tokenizer. Never fails: anything unreadable becomes an `Error`
//! token so the parser can report it and keep going.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    Tilde,
    Comment,
    Error,
}

#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub kind: Kind,
    pub start: u32,
    pub end: u32,
    /// Set for malformed tokens (unterminated string, bad IRI, stray character).
    pub error: Option<&'static str>,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start as usize..self.end as usize]
    }
}

pub fn is_pn_chars_base(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

pub fn is_pn_chars_u(c: char) -> bool {
    c == '_' || is_pn_chars_base(c)
}

pub fn is_pn_chars(c: char) -> bool {
    is_pn_chars_u(c)
        || c == '-'
        || c.is_ascii_digit()
        || c == '\u{B7}'
        || ('\u{300}'..='\u{36F}').contains(&c)
        || ('\u{203F}'..='\u{2040}').contains(&c)
}

const LOCAL_ESC: &[u8] = b"_~.-!$&'()*+,;=/?#@%";

struct Lexer<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    out: Vec<Token>,
}

pub fn tokenize(src: &str) -> Vec<Token> {
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
    fn push(&mut self, kind: Kind, end: usize, error: Option<&'static str>) {
        self.out.push(Token {
            kind,
            start: self.i as u32,
            end: end as u32,
            error,
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
                    let end = memchr(b'\n', &self.b[self.i..]).map_or(self.b.len(), |p| self.i + p);
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
                        self.push(Kind::Error, self.i + 1, Some("Unexpected character"))
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
                b'|' if self.starts("|}") => self.push(Kind::RAnnotation, self.i + 2, None),
                b'~' => self.push(Kind::Tilde, self.i + 1, None),
                _ => self.name_or_word(),
            }
        }
    }

    fn string(&mut self, q: u8) {
        let long = self.b.len() >= self.i + 3 && self.b[self.i + 1] == q && self.b[self.i + 2] == q;
        let mut j = self.i + if long { 3 } else { 1 };
        while j < self.b.len() {
            let c = self.b[j];
            if c == b'\\' {
                j += 2;
                continue;
            }
            if long {
                if c == q && self.b.get(j + 1) == Some(&q) && self.b.get(j + 2) == Some(&q) {
                    // A long string may end with up to two extra quotes: """a""""" is valid.
                    let mut end = j + 3;
                    while self.b.get(end) == Some(&q) && end < j + 5 {
                        end += 1;
                    }
                    return self.push(Kind::String, end, None);
                }
            } else if c == q {
                return self.push(Kind::String, j + 1, None);
            } else if c == b'\n' || c == b'\r' {
                return self.push(Kind::String, j, Some("Unterminated string"));
            }
            j += 1;
        }
        let end = self.b.len();
        self.push(
            Kind::String,
            end,
            Some(if long {
                "Unterminated long string"
            } else {
                "Unterminated string"
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
                    return self.push(Kind::Iri, j + 1, bad.then_some("Invalid character in IRI"));
                }
                b' ' | b'\t' | b'\r' | b'\n' => break,
                b'<' | b'"' | b'{' | b'}' | b'|' | b'^' | b'`' => bad = true,
                b'\\' => {
                    let ok = match self.b.get(j + 1) {
                        Some(b'u') => self.hex(j + 2, 4),
                        Some(b'U') => self.hex(j + 2, 8),
                        _ => false,
                    };
                    if !ok {
                        bad = true
                    }
                }
                c if c < 0x20 => bad = true,
                _ => {}
            }
            j += 1;
        }
        self.push(Kind::Error, j, Some("Unterminated IRI: missing '>'"))
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
            return self.push(Kind::Error, self.i + 1, Some("Unexpected '@'"));
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
            _ => return self.push(Kind::Error, j, Some("Blank node label expected after '_:'")),
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

    /// PNAME_NS / PNAME_LN, or a bare word (a, true, false, PREFIX, BASE, VERSION).
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
        loop {
            let Some(c) = self.char_at(j) else { break };
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
            return self.push(Kind::Error, start + len, Some("Unexpected character"));
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
            _ => {
                return self.push(
                    Kind::Error,
                    j,
                    Some("Unexpected word (did you forget a prefix, e.g. `ex:`?)"),
                );
            }
        };
        self.push(kind, j, None)
    }
}

fn memchr(needle: u8, hay: &[u8]) -> Option<usize> {
    hay.iter().position(|c| *c == needle)
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
        assert!(tokenize("\"open\n")[0].error.is_some());
        assert!(k.contains(&(Kind::PName, "ex:b")));
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
}
