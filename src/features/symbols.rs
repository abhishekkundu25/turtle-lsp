//! Document symbols (outline), workspace symbols and folding ranges.

use crate::analysis::{Analysis, Encoding};
use crate::features::navigation::uri;
use crate::syntax::Span;
use crate::syntax::lexer::Kind;
use lsp_types::*;
use std::sync::Arc;

fn kind_for(a: &Analysis, iri: &str) -> SymbolKind {
    let types = a.types.get(iri).map(Vec::as_slice).unwrap_or(&[]);
    if types.iter().any(|t| t.ends_with("Class")) {
        SymbolKind::CLASS
    } else if types.iter().any(|t| t.ends_with("Property")) {
        SymbolKind::PROPERTY
    } else if types.iter().any(|t| t.ends_with("Shape")) {
        SymbolKind::INTERFACE
    } else if types.iter().any(|t| t.ends_with("Ontology")) {
        SymbolKind::PACKAGE
    } else {
        SymbolKind::OBJECT
    }
}

fn written(text: &str, span: Span) -> &str {
    text.get(span.start as usize..span.end as usize)
        .unwrap_or("")
}

#[allow(deprecated)]
pub fn document_symbols(a: &Analysis, text: &str, enc: Encoding) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    if !a.parsed.prefixes.is_empty() {
        let first = a.parsed.prefixes.first().unwrap().span;
        let last = a.parsed.prefixes.last().unwrap().span;
        out.push(DocumentSymbol {
            name: "prefixes".into(),
            detail: Some(format!("{}", a.parsed.prefixes.len())),
            kind: SymbolKind::NAMESPACE,
            tags: None,
            deprecated: None,
            range: a.range(enc, Span::new(first.start, last.end)),
            selection_range: a.range(enc, first),
            children: Some(
                a.parsed
                    .prefixes
                    .iter()
                    .map(|p| DocumentSymbol {
                        name: format!("{}:", p.name),
                        detail: Some(p.iri.clone()),
                        kind: SymbolKind::NAMESPACE,
                        tags: None,
                        deprecated: None,
                        range: a.range(enc, p.span),
                        selection_range: a.range(enc, p.name_span),
                        children: None,
                    })
                    .collect(),
            ),
        });
    }
    let prefixes = a.prefix_map();
    for st in &a.parsed.statements {
        let Some((iri, span)) = &st.subject else {
            continue;
        };
        let detail = a.labels.get(iri).cloned().or_else(|| {
            a.types.get(iri).map(|ts| {
                ts.iter()
                    .map(|t| {
                        crate::features::completion::shorten(t, &prefixes)
                            .unwrap_or_else(|| t.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        });
        out.push(DocumentSymbol {
            name: written(text, *span).to_string(),
            detail,
            kind: kind_for(a, iri),
            tags: None,
            deprecated: None,
            range: a.range(enc, Span::new(st.span.start, st.span.end.max(span.end))),
            selection_range: a.range(enc, *span),
            children: None,
        });
    }
    out
}

#[allow(deprecated)]
pub fn workspace_symbols(
    all: &[Arc<Analysis>],
    enc: Encoding,
    query: &str,
) -> Vec<SymbolInformation> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for a in all {
        let prefixes = a.prefix_map();
        for (iri, spans) in &a.defs {
            let name = crate::features::completion::shorten(iri, &prefixes)
                .unwrap_or_else(|| format!("<{iri}>"));
            let label = a.labels.get(iri);
            if !q.is_empty()
                && !name.to_lowercase().contains(&q)
                && !label.is_some_and(|l| l.to_lowercase().contains(&q))
            {
                continue;
            }
            out.push(SymbolInformation {
                name,
                kind: kind_for(a, iri),
                tags: None,
                deprecated: None,
                location: Location {
                    uri: uri(&a.uri),
                    range: a.range(enc, spans[0]),
                },
                container_name: label.cloned(),
            });
            if out.len() >= 500 {
                return out;
            }
        }
    }
    out
}

pub fn folding_ranges(a: &Analysis) -> Vec<FoldingRange> {
    let line = |off: u32| a.position(Encoding::Utf8, off).line;
    let mut out = Vec::new();
    let mut push = |start: u32, end: u32, kind: Option<FoldingRangeKind>| {
        let (s, e) = (line(start), line(end));
        if e > s {
            out.push(FoldingRange {
                start_line: s,
                end_line: e,
                kind,
                ..Default::default()
            });
        }
    };
    if let (Some(f), Some(l)) = (a.parsed.prefixes.first(), a.parsed.prefixes.last()) {
        push(f.span.start, l.span.end, Some(FoldingRangeKind::Imports));
    }
    for st in &a.parsed.statements {
        push(st.span.start, st.span.end, None);
    }
    // Nested [ ], ( ), {| |}, << >> blocks. When the closer starts its own line,
    // stop the fold one line earlier so the closer stays visible.
    let mut nested = Vec::new();
    let mut open: Vec<(Kind, u32)> = Vec::new();
    let mut prev_line = 0;
    for t in a.parsed.tokens.iter().filter(|t| t.kind != Kind::Comment) {
        let closes = match t.kind {
            Kind::LBracket | Kind::LParen | Kind::LAnnotation | Kind::LTriple | Kind::LTripleTerm => {
                open.push((t.kind, t.start));
                None
            }
            Kind::RBracket => Some(Kind::LBracket),
            Kind::RParen => Some(Kind::LParen),
            Kind::RAnnotation => Some(Kind::LAnnotation),
            Kind::RTriple => Some(Kind::LTriple),
            Kind::RTripleTerm => Some(Kind::LTripleTerm),
            _ => None,
        };
        let here = line(t.start);
        if let Some(opener) = closes {
            if let Some(i) = open.iter().rposition(|(k, _)| *k == opener) {
                let (_, start) = open.remove(i);
                let end_line = if here > prev_line { here - 1 } else { here };
                if end_line > line(start) {
                    nested.push(FoldingRange { start_line: line(start), end_line, ..Default::default() });
                }
            }
        }
        prev_line = here;
    }
    // Runs of consecutive comment lines.
    let mut run: Option<(u32, u32, u32)> = None; // (start, end, last line)
    for c in &a.parsed.comments {
        let l = line(c.start);
        run = match run {
            Some((s, _, last)) if l == last + 1 => Some((s, c.end, l)),
            Some((s, e, _)) => {
                push(s, e, Some(FoldingRangeKind::Comment));
                Some((c.start, c.end, l))
            }
            None => Some((c.start, c.end, l)),
        };
    }
    if let Some((s, e, _)) = run {
        push(s, e, Some(FoldingRangeKind::Comment));
    }
    out.extend(nested);
    out
}
