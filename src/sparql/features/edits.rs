//! Edits more than one SPARQL feature makes: adding a `PREFIX` line (the
//! add-prefix quick fixes, and completion of a term whose prefix is not
//! declared).

use super::actions::{Tails, text_edits_with};
use crate::analysis::{Encoding, SparqlAnalysis};
use crate::sparql::{Token, TokenKind};
use crate::syntax::Span;
use crate::syntax::names::{header_end, insertion_point};
use lsp_types::TextEdit;

/// The edit that declares `PREFIX name: <namespace>`, its lines ended by
/// `\n`: at [`insertion_point`] after the last declaration (`BASE`, `PREFIX`
/// or `VERSION`) of the prologue, so it joins the block; with no prologue,
/// after the header comment, followed by a blank line if none follows.
/// `None` when the line would not lex as written there (inside an
/// unclosed expression, its IRI reads as `<`). Calls with one `tails`
/// lex the query after the line once.
pub(crate) fn add_prefix(
    q: &SparqlAnalysis,
    enc: Encoding,
    name: &str,
    namespace: &str,
    tails: &Tails,
) -> Option<TextEdit> {
    let st = q.doc.structure();
    let (text, tokens) = (q.text(), q.doc.tokens());
    let line = format!("PREFIX {name}: <{namespace}>");
    // The prologue ends where the first operation starts.
    let first_op = st.operations.first().map_or(u32::MAX, |o| o.keyword.start);
    let last_decl = st
        .prefixes
        .iter()
        .map(|p| p.span.end)
        .chain(st.bases.iter().map(|b| b.span.end))
        .chain(st.versions.iter().map(|v| v.span.end))
        .filter(|&end| end <= first_op)
        .max();
    let comment = |t: &&Token| t.kind == TokenKind::Comment;
    let anchor = last_decl.map_or_else(
        || header_end(text, tokens.iter().take_while(comment).map(|t| t.span)),
        |end| end as usize,
    );
    let next = tokens[tokens.partition_point(|t| (t.span.start as usize) < anchor)..]
        .iter()
        .find(|t| !comment(t));
    let next_start = next.map_or(text.len(), |t| t.span.start as usize);
    let (at, break_first) = insertion_point(text, anchor, next_start, text.len());
    let new_text = if break_first {
        format!("\n{line}")
    } else if last_decl.is_none() && !text[at..].starts_with(['\r', '\n']) {
        format!("{line}\n\n")
    } else {
        format!("{line}\n")
    };
    let insert = vec![(Span::new(at as u32, at as u32), new_text)];
    let mut edit = text_edits_with(q, enc, insert, tails)?.remove(0);
    // A space would leave the query on the declaration's line.
    if let Some(t) = edit.new_text.strip_suffix(' ') {
        edit.new_text = format!("{t}\n");
    }
    Some(edit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    fn added(text: &str) -> String {
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let e = add_prefix(
            &q,
            Encoding::Utf16,
            "foaf",
            "http://xmlns.com/foaf/0.1/",
            &Tails::default(),
        )
        .unwrap();
        let at = q.offset(Encoding::Utf16, e.range.start).unwrap() as usize;
        format!("{}{}{}", &text[..at], e.new_text, &text[at..])
    }

    const FOAF: &str = "PREFIX foaf: <http://xmlns.com/foaf/0.1/>";

    #[test]
    fn prefix_lines_in_one_place_lex_what_follows_them_once() {
        // The window after the line holds a long variable.
        let text = format!(
            "PREFIX a: <http://a/>\nSELECT ?{} {{ }}\n",
            "v".repeat(50_000)
        );
        let q = SparqlAnalysis::new("file:///q.rq".into(), &text, Arc::new(PrefixMap::default()));
        let tails = Tails::default();
        for (i, name) in ["b", "c", "d", "e"].into_iter().enumerate() {
            let ns = format!("http://{name}/");
            crate::sparql::lexer::LEXED.with(|c| c.set(0));
            let shared = add_prefix(&q, Encoding::Utf8, name, &ns, &tails);
            let lexed = crate::sparql::lexer::LEXED.with(|c| c.get());
            assert_eq!(
                shared,
                add_prefix(&q, Encoding::Utf8, name, &ns, &Tails::default())
            );
            assert!(shared.is_some());
            if i > 0 {
                assert!(lexed < 1_000, "{name}: {lexed} bytes lexed");
            }
        }
    }

    #[test]
    fn joins_the_prefix_block() {
        assert_eq!(
            added("PREFIX ex: <http://e/>\nSELECT * { ?s foaf:name ?n }\n"),
            format!("PREFIX ex: <http://e/>\n{FOAF}\nSELECT * {{ ?s foaf:name ?n }}\n")
        );
    }

    #[test]
    fn goes_after_base() {
        assert_eq!(
            added("BASE <http://b/>\nSELECT * { ?s foaf:name ?n }\n"),
            format!("BASE <http://b/>\n{FOAF}\nSELECT * {{ ?s foaf:name ?n }}\n")
        );
    }

    #[test]
    fn without_a_prologue_goes_first_with_a_blank_line() {
        assert_eq!(
            added("SELECT * { ?s foaf:name ?n }\n"),
            format!("{FOAF}\n\nSELECT * {{ ?s foaf:name ?n }}\n")
        );
        // Below a header comment.
        assert_eq!(
            added("# People\nSELECT * { ?s foaf:name ?n }\n"),
            format!("# People\n{FOAF}\n\nSELECT * {{ ?s foaf:name ?n }}\n")
        );
    }

    #[test]
    fn a_prologue_without_a_final_newline() {
        assert_eq!(
            added("PREFIX ex: <http://e/>"),
            format!("PREFIX ex: <http://e/>\n{FOAF}")
        );
    }

    #[test]
    fn a_prefix_between_update_operations_is_not_the_prologue() {
        let text = "PREFIX ex: <http://e/>\nINSERT DATA { ex:a ex:b ex:c } ;\nPREFIX g: <http://g/>\nINSERT DATA { g:a g:b foaf:c }\n";
        assert!(added(text).starts_with(&format!("PREFIX ex: <http://e/>\n{FOAF}\nINSERT")));
    }

    #[test]
    fn the_prefix_never_lands_in_the_query() {
        for text in [
            "PREFIX ex: <http://e/> SELECT * {\n  ?s foaf:name ?n }",
            "PREFIX ex: <http://e/> SELECT * { ?s ex:p \"\"\"a\nb\"\"\" . ?s foaf:name ?n }",
            "PREFIX ex: <http://e/>\rSELECT * { ?s foaf:name ?n }\r",
        ] {
            let after = added(text);
            let doc = crate::sparql::Document::parse(&after, &PrefixMap::default(), None);
            assert!(!doc.has_errors(), "{after:?}");
            let first_op = doc.structure().operations[0].keyword.start;
            assert!(after[..first_op as usize].contains(FOAF), "{after:?}");
        }
        // `\r` lines: after the first `\r`, before the query.
        assert_eq!(
            added("PREFIX ex: <http://e/>\rSELECT * { ?s foaf:name ?n }\r"),
            format!("PREFIX ex: <http://e/>\r{FOAF}\nSELECT * {{ ?s foaf:name ?n }}\r")
        );
    }

    #[test]
    fn no_prefix_line_where_its_iri_would_read_as_less_than() {
        // In an unclosed `BIND(`, `<` after the string `"1.2"` is the operator.
        let text = "VERSION \"1.2\"BIND(\nVERSION \"1.2\"\n\nSELECT * { ?s foaf:name ?n }\n";
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let foaf = "http://xmlns.com/foaf/0.1/";
        assert!(add_prefix(&q, Encoding::Utf16, "foaf", foaf, &Tails::default()).is_none());
    }

    #[test]
    fn without_a_prologue_goes_below_the_header() {
        assert_eq!(
            added("# licence\n\n# People\nSELECT * { ?s foaf:name ?n }\n"),
            format!("# licence\n{FOAF}\n\n# People\nSELECT * {{ ?s foaf:name ?n }}\n")
        );
    }
}
