//! The prefixes in scope at an offset and the edits that declare a prefix.

use crate::analysis::{Analysis, Encoding};
use crate::syntax::lexer::Kind;
use crate::syntax::names::{header_end, insertion_point, newline, shorten, turtle_token_edits};
use crate::syntax::{PrefixDecl, PrefixMap, Span, Syntax};
use lsp_types::{Range, TextEdit};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

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

/// How reasoning names an IRI in a document, in hover and in diagnostics
/// alike, in every RDF syntax: with the document's own prefixes (its
/// declarations, or the prefix terms of a JSON-LD `@context`), then with
/// the project's (`prefixes.ttl` and the `prefixes` setting, which N-Triples
/// and N-Quads documents only name with), then with a well-known
/// vocabulary's, else as `<iri>`.
pub(crate) struct Naming {
    own: PrefixMap,
    project: Arc<PrefixMap>,
}

impl Naming {
    /// Naming at `offset` of `a` (`u32::MAX`: the whole document), with the
    /// project's prefixes for it, `project`.
    pub(crate) fn at(a: &Analysis, offset: u32, project: Arc<PrefixMap>) -> Naming {
        let own = match a.parsed.syntax {
            Syntax::JsonLd => context_prefixes(&a.source_index, offset),
            _ => a
                .parsed
                .prefixes
                .iter()
                .take_while(|p| p.span.end <= offset)
                .map(|p| (p.name.clone(), p.iri.to_string()))
                .collect(),
        };
        Naming { own, project }
    }

    /// `iri` as `prefix:local`, else `<iri>`.
    pub(crate) fn name(&self, iri: &str) -> String {
        shorten(iri, &self.own)
            .or_else(|| shorten(iri, &self.project))
            .unwrap_or_else(|| crate::features::hover::short(iri, &PrefixMap::default()))
    }
}

/// The prefix terms of a JSON-LD document's contexts: the context at
/// `offset` first, then the others in text order; a term's first definition
/// wins.
fn context_prefixes(index: &crate::jsonld::SourceIndex, offset: u32) -> PrefixMap {
    let mut out = PrefixMap::default();
    for ctx in index.context_at(offset).into_iter().chain(index.contexts()) {
        for (term, d) in ctx.terms() {
            let Some(iri) = d.iri.as_deref() else {
                continue;
            };
            if d.prefix
                && !term.contains(':')
                && !iri.starts_with(['@', '_'])
                && !out.contains_key(term)
            {
                out.insert(term.to_string(), iri.to_string());
            }
        }
    }
    out
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

/// The keyword a declaration added to `a` starts with, in the style the
/// document already uses: `PREFIX` when its last declaration is SPARQL-style,
/// else `@prefix`.
pub(crate) fn declaration_keyword(a: &Analysis) -> &'static str {
    let sparql = a.parsed.prefixes.last().is_some_and(|p: &PrefixDecl| {
        a.tokens()
            .iter()
            .any(|t| t.start == p.span.start && t.kind == Kind::SparqlPrefix)
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
        let comments = tokens.iter().take_while(|t| t.kind == Kind::Comment);
        header_end(text, comments.map(|t| Span::new(t.start, t.end)))
    });
    let next = tokens[tokens.partition_point(|t| (t.start as usize) < anchor)..]
        .iter()
        .find(|t| t.kind != Kind::Comment)
        .map_or(text.len(), |t| t.start as usize);
    insertion_point(text, anchor, next, before as usize)
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
    use std::sync::Arc;

    fn analysis(text: &str) -> Analysis {
        Analysis::new(
            "file:///t/doc.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        )
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
