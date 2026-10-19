//! Edits more than one SPARQL feature makes: adding a `PREFIX` line (the
//! add-prefix quick fixes, and completion of a term whose prefix is not
//! declared).

use crate::analysis::{Encoding, SparqlAnalysis};
use crate::sparql::TokenKind;
use lsp_types::{Position, Range, TextEdit};

/// The edit that declares `PREFIX name: <namespace>`: on the line after the
/// last declaration (`BASE`, `PREFIX` or `VERSION`) of the prologue, so it
/// joins the block; with no prologue, above the first line of the query
/// that is not a comment, followed by a blank line.
pub fn add_prefix(q: &SparqlAnalysis, enc: Encoding, name: &str, namespace: &str) -> TextEdit {
    let st = q.doc.structure();
    let text = q.text();
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
    let insert = |pos: Position, new_text: String| TextEdit {
        range: Range::new(pos, pos),
        new_text,
    };
    match last_decl {
        Some(end) => {
            let pos = q.position(enc, end);
            // The start of the next line, or the end of a last line without a newline.
            match text[end as usize..].find('\n') {
                Some(_) => insert(Position::new(pos.line + 1, 0), format!("{line}\n")),
                None => insert(q.position(enc, text.len() as u32), format!("\n{line}")),
            }
        }
        None => {
            let first = q
                .doc
                .tokens()
                .iter()
                .find(|t| t.kind != TokenKind::Comment)
                .map_or(0, |t| t.span.start);
            let pos = q.position(enc, first);
            insert(Position::new(pos.line, 0), format!("{line}\n\n"))
        }
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
}
