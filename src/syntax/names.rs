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

/// A prefixed name replacing the IRI at `span`, with a space added where it
/// would otherwise run into the neighbouring token (`a<x>`, `<x>ex:y`).
pub(crate) fn spaced(text: &str, span: Span, pname: String) -> String {
    let joins = |c: char| is_pn_chars(c) || matches!(c, ':' | '%' | '\\');
    let before = text[..span.start as usize].chars().next_back();
    let mut after = text[span.end as usize..].chars();
    let next = after.next();
    let space_before = before.is_some_and(joins);
    let space_after = match next {
        Some('.') => after.next().is_some_and(joins),
        Some(c) => joins(c),
        None => false,
    };
    match (space_before, space_after) {
        (false, false) => pname,
        (b, s) => format!(
            "{}{pname}{}",
            if b { " " } else { "" },
            if s { " " } else { "" }
        ),
    }
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

/// Where a declaration line goes for text from `before` on: after the last
/// declaration that ends before `before`, else at the top of the file after
/// a leading comment block. The flag says the line must start with a line
/// break (the point is the end of text that has none, or of a declaration a
/// use follows on the same line).
pub(crate) fn declaration_point(a: &Analysis, before: u32) -> (usize, bool) {
    let text: &str = &a.text;
    match a
        .parsed
        .prefixes
        .iter()
        .rev()
        .find(|p| p.span.end <= before)
    {
        Some(p) => {
            let end = p.span.end as usize;
            match text[end..].find('\n') {
                Some(i) if before as usize > end + i => (end + i + 1, false),
                Some(_) => (end, true),
                None => (text.len(), true),
            }
        }
        None => {
            let mut off = 0;
            for line in text.split_inclusive('\n') {
                if line.trim_start().starts_with('#') {
                    off += line.len();
                } else {
                    break;
                }
            }
            (
                off,
                off == text.len() && !text.is_empty() && !text.ends_with('\n'),
            )
        }
    }
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
/// to that first use, so the declaration precedes every use it serves.
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
    let new_text = if break_first {
        let nl = newline(&a.text);
        format!("{nl}{}", lines.trim_end_matches(nl))
    } else {
        lines
    };
    let pos = a.position(enc, at as u32);
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

    #[test]
    fn spacing_keeps_tokens_apart() {
        let t = "_:b<http://e/p>.<http://e/x>";
        let s = Span::new(3, 15);
        assert_eq!(spaced(t, s, "e:p".into()), " e:p");
        let t = "<http://e/p>.ex:a";
        assert_eq!(spaced(t, Span::new(0, 12), "e:p".into()), "e:p ");
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
