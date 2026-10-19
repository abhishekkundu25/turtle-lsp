//! The outline (document symbols) and folding ranges of SPARQL documents.

use crate::analysis::{Encoding, SparqlAnalysis};
use crate::sparql::{TokenKind, lexer};
use crate::syntax::Span;
use lsp_types::*;

/// Symbol names longer than this are cut, with an ellipsis.
const MAX_NAME: usize = 80;

/// The tokens from `from` up to the first `{`, the end of `span` or `stop`,
/// written as one line: keywords upper-cased, function names in their
/// grammar spelling, whitespace between tokens collapsed to one space.
fn heading(q: &SparqlAnalysis, from: u32, stop: u32) -> String {
    let text = q.text();
    let toks = q.doc.tokens();
    let mut out = String::new();
    let mut prev_end: Option<u32> = None;
    for t in &toks[toks.partition_point(|t| t.span.start < from)..] {
        if t.span.start >= stop {
            break;
        }
        if t.kind == TokenKind::Comment {
            continue;
        }
        let s = t.text(text);
        if t.kind == TokenKind::Punct && s == "{" {
            break;
        }
        if let Some(e) = prev_end
            && e < t.span.start
        {
            out.push(' ');
        }
        out.push_str(match t.kind {
            TokenKind::Keyword => lexer::keyword(s).unwrap_or(s),
            TokenKind::Function => lexer::function(s).unwrap_or(s),
            _ => s,
        });
        prev_end = Some(t.span.end);
        if out.chars().count() > MAX_NAME {
            let mut cut: String = out.chars().take(MAX_NAME - 1).collect();
            cut.push('…');
            return cut;
        }
    }
    out
}

/// `textDocument/documentSymbol`: the query (named by its form and
/// projection, e.g. `SELECT ?person ?name`) or each update operation (named
/// by its first words), with its clauses as children.
#[allow(deprecated)]
pub fn document_symbols(q: &SparqlAnalysis, enc: Encoding) -> Vec<DocumentSymbol> {
    let st = q.doc.structure();
    st.operations
        .iter()
        .map(|op| {
            // The operation's name stops where its second clause starts.
            let stop = op
                .clauses
                .iter()
                .map(|c| c.keyword.start)
                .find(|&s| s > op.keyword.start)
                .unwrap_or(op.span.end)
                .min(op.span.end);
            let children: Vec<DocumentSymbol> = op
                .clauses
                .iter()
                // The clause the operation starts with names the operation.
                .filter(|c| c.keyword != op.keyword)
                .map(|c| DocumentSymbol {
                    name: heading(q, c.keyword.start, c.span.end),
                    detail: None,
                    kind: SymbolKind::FIELD,
                    tags: None,
                    deprecated: None,
                    range: q.range(enc, c.span),
                    selection_range: q.range(enc, c.keyword),
                    children: None,
                })
                .filter(|c| !c.name.is_empty())
                .collect();
            DocumentSymbol {
                name: heading(q, op.keyword.start, stop),
                detail: None,
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: None,
                range: q.range(enc, op.span),
                selection_range: q.range(enc, op.keyword),
                children: Some(children),
            }
        })
        .filter(|s| !s.name.is_empty())
        .collect()
}

/// `textDocument/foldingRange`: groups (ending the line before a `}` that
/// starts its own line, as in Turtle), runs of prologue declarations (the
/// `PREFIX` block) and runs of comment lines.
pub fn folding_ranges(q: &SparqlAnalysis) -> Vec<FoldingRange> {
    let st = q.doc.structure();
    let toks = q.doc.tokens();
    let line = |off: u32| q.position(Encoding::Utf8, off).line;
    let mut out = Vec::new();
    let mut push = |s: u32, e: u32, kind: Option<FoldingRangeKind>| {
        if e > s {
            out.push(FoldingRange {
                start_line: s,
                end_line: e,
                kind,
                ..Default::default()
            });
        }
    };

    // Declarations on consecutive lines.
    let mut decls: Vec<Span> = st
        .prefixes
        .iter()
        .map(|p| p.span)
        .chain(st.bases.iter().map(|b| b.span))
        .chain(st.versions.iter().map(|v| v.span))
        .collect();
    decls.sort_by_key(|s| s.start);
    let mut run: Option<(u32, u32)> = None; // (first line, last line)
    for d in &decls {
        let (s, e) = (line(d.start), line(d.end));
        run = match run {
            Some((first, last)) if s <= last + 1 => Some((first, e.max(last))),
            Some((first, last)) => {
                push(first, last, Some(FoldingRangeKind::Imports));
                Some((s, e))
            }
            None => Some((s, e)),
        };
    }
    if let Some((first, last)) = run {
        push(first, last, Some(FoldingRangeKind::Imports));
    }

    for g in &st.groups {
        let start = line(g.open.start);
        let end = match g.close {
            Some(c) => {
                let here = line(c.start);
                // The last token before the `}`.
                let i = toks.partition_point(|t| t.span.start < c.start);
                let prev = toks[..i].last().map_or(here, |t| line(t.span.end));
                if here > prev { here - 1 } else { here }
            }
            None => line(g.span.end),
        };
        push(start, end, None);
    }

    // Runs of consecutive comment lines.
    let mut run: Option<(u32, u32)> = None;
    for c in toks.iter().filter(|t| t.kind == TokenKind::Comment) {
        let l = line(c.span.start);
        run = match run {
            Some((s, last)) if l == last + 1 => Some((s, l)),
            Some((s, last)) => {
                push(s, last, Some(FoldingRangeKind::Comment));
                Some((l, l))
            }
            None => Some((l, l)),
        };
    }
    if let Some((s, last)) = run {
        push(s, last, Some(FoldingRangeKind::Comment));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    fn parse(text: &str) -> SparqlAnalysis {
        SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()))
    }

    fn names(syms: &[DocumentSymbol]) -> Vec<String> {
        syms.iter()
            .map(|s| {
                let kids: Vec<&str> = s
                    .children
                    .iter()
                    .flatten()
                    .map(|c| c.name.as_str())
                    .collect();
                format!("{} [{}]", s.name, kids.join(" | "))
            })
            .collect()
    }

    #[test]
    fn a_query_and_its_clauses() {
        let q = parse(
            "PREFIX ex: <http://e/>\nselect distinct ?s (count(?o) as ?n)\nFROM <http://g/>\nWHERE { ?s ex:p ?o }\ngroup by ?s\nORDER BY desc(?n)\nLIMIT 5\n",
        );
        assert_eq!(
            names(&document_symbols(&q, Encoding::Utf16)),
            [
                "SELECT DISTINCT ?s (COUNT(?o) AS ?n) [FROM <http://g/> | WHERE | GROUP BY ?s | ORDER BY DESC(?n) | LIMIT 5]"
            ]
        );
    }

    #[test]
    fn each_update_operation() {
        let q = parse(
            "PREFIX ex: <http://e/>\nINSERT DATA { ex:a ex:b ex:c } ;\nDELETE { ?p ex:n ?o }\nINSERT { ?p ex:n 1 }\nWHERE { ?p ex:n ?o } ;\nLOAD <http://x/> INTO GRAPH <http://g/>\n",
        );
        assert_eq!(
            names(&document_symbols(&q, Encoding::Utf16)),
            [
                "INSERT DATA []",
                "DELETE [INSERT | WHERE]",
                "LOAD <http://x/> INTO GRAPH <http://g/> []"
            ]
        );
    }

    #[test]
    fn folds() {
        let q = parse(
            "# a\n# b\nPREFIX ex: <http://e/>\nBASE <http://b/>\n\nPREFIX ex2: <http://f/>\nSELECT * WHERE {\n  ?s ex:p ?o .\n  OPTIONAL { ?s ex:q ?l }\n  FILTER (?o) }\n",
        );
        let got: Vec<String> = folding_ranges(&q)
            .iter()
            .map(|r| format!("{}-{}", r.start_line, r.end_line))
            .collect();
        // The WHERE group's `}` ends a line with content: the fold keeps it.
        assert_eq!(got, ["2-3", "6-9", "0-1"]);
    }
}
