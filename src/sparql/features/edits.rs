//! Edits more than one SPARQL feature makes: adding a `PREFIX` line (the
//! add-prefix quick fixes, and completion of a term whose prefix is not
//! declared).

use crate::analysis::{Encoding, SparqlAnalysis};
use crate::sparql::lexer::kinds;
use crate::sparql::{Token, TokenKind};
use crate::syntax::Span;
use crate::syntax::names::{header_end, insertion_point, token_edits};
use lsp_types::{Range, TextEdit};

/// The edit that declares `PREFIX name: <namespace>`, its lines ended by
/// `\n`: at [`insertion_point`] after the last declaration (`BASE`, `PREFIX`
/// or `VERSION`) of the prologue, so it joins the block; with no prologue,
/// after the header comment, followed by a blank line if none follows.
pub(crate) fn add_prefix(
    q: &SparqlAnalysis,
    enc: Encoding,
    name: &str,
    namespace: &str,
) -> TextEdit {
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
    // No bracket is open at the anchor, so the lexer may start there.
    let window = &text[anchor..next.map_or(text.len(), |t| t.span.end as usize)];
    let span = Span::new((at - anchor) as u32, (at - anchor) as u32);
    let mut new_text = token_edits(window, vec![(span, new_text)], kinds)
        .remove(0)
        .1;
    // A space would leave the query on the declaration's line.
    if let Some(t) = new_text.strip_suffix(' ') {
        new_text = format!("{t}\n");
    }
    let pos = q.position(enc, at as u32);
    TextEdit {
        range: Range::new(pos, pos),
        new_text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    fn added(text: &str) -> String {
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let e = add_prefix(&q, Encoding::Utf16, "foaf", "http://xmlns.com/foaf/0.1/");
        let at = q.offset(Encoding::Utf16, e.range.start).unwrap() as usize;
        format!("{}{}{}", &text[..at], e.new_text, &text[at..])
    }

    const FOAF: &str = "PREFIX foaf: <http://xmlns.com/foaf/0.1/>";

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
    fn without_a_prologue_goes_below_the_header() {
        assert_eq!(
            added("# licence\n\n# People\nSELECT * { ?s foaf:name ?n }\n"),
            format!("# licence\n{FOAF}\n\n# People\nSELECT * {{ ?s foaf:name ?n }}\n")
        );
    }
}
