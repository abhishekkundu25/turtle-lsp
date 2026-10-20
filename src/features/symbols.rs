//! Document symbols (outline), workspace symbols and folding ranges.

use crate::analysis::{Analysis, Encoding};
use crate::features::navigation::uri;
use crate::knowledge::{DocId, Knowledge, Source};
use crate::syntax::lexer::{Kind, tokenize};
use crate::syntax::{GraphName, Iri, PrefixMap, Span, Statement};
use lsp_types::*;
use rustc_hash::FxHashMap;

fn kind_for(a: &Analysis, iri: &str) -> SymbolKind {
    kind_of(a.types.get(iri).map(Vec::as_slice).unwrap_or(&[]))
}

fn kind_of(types: &[Iri]) -> SymbolKind {
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
                        detail: Some(p.iri.to_string()),
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
    let detail = |iri: &Iri| {
        a.label(iri).map(str::to_string).or_else(|| {
            a.types.get(iri).map(|ts| {
                ts.iter()
                    .map(|t| {
                        crate::features::completion::shorten(t, &prefixes)
                            .unwrap_or_else(|| t.to_string())
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
    };
    let subject = |st: &Statement| {
        // A JSON-LD graph node is shown as its graph, not also as a subject.
        if a.parsed.graphs.iter().any(|g| g.span == st.span) {
            return None;
        }
        let (iri, span) = st.subject.as_ref()?;
        Some(DocumentSymbol {
            name: written(text, *span).to_string(),
            detail: detail(iri),
            kind: kind_for(a, iri),
            tags: None,
            deprecated: None,
            range: a.range(enc, Span::new(st.span.start, st.span.end.max(span.end))),
            selection_range: a.range(enc, *span),
            children: None,
        })
    };
    let statements = &a.parsed.statements;
    if statements.iter().any(|st| st.graph.is_some()) {
        out.extend(quad_symbols(a, text, enc, &subject, &detail));
        return out;
    }
    // TriG graph blocks are containers for the subjects inside them.
    let mut next = 0;
    for g in &a.parsed.graphs {
        let (from, to) = (g.statements.start as usize, g.statements.end as usize);
        if from < next {
            continue; // nested inside the previous graph
        }
        out.extend(statements[next.min(from)..from].iter().filter_map(subject));
        let name_span = g
            .name_span
            .unwrap_or(Span::new(g.span.start, g.span.start + 1));
        out.push(DocumentSymbol {
            name: match g.name_span {
                Some(s) => written(text, s).to_string(),
                None => "{ } default graph".into(),
            },
            detail: Some("graph".into()),
            kind: SymbolKind::NAMESPACE,
            tags: None,
            deprecated: None,
            range: a.range(enc, g.span),
            selection_range: a.range(enc, name_span),
            children: Some(
                statements[from.min(to)..to.min(statements.len())]
                    .iter()
                    .filter_map(subject)
                    .collect(),
            ),
        });
        next = to;
    }
    out.extend(
        statements[next.min(statements.len())..]
            .iter()
            .filter_map(subject),
    );
    out
}

/// N-Quads: one container per graph, where the graph first appears, holding
/// one entry per distinct subject of its quads (wherever they are); the
/// default graph's statements stay at the top level.
#[allow(deprecated)]
fn quad_symbols(
    a: &Analysis,
    text: &str,
    enc: Encoding,
    subject: &dyn Fn(&Statement) -> Option<DocumentSymbol>,
    detail: &dyn Fn(&Iri) -> Option<String>,
) -> Vec<DocumentSymbol> {
    struct Entry<'s> {
        st: &'s Statement,
        name: Span,
        range: Span,
    }
    enum Top<'s> {
        Statement(&'s Statement),
        Graph {
            label: Span,
            range: Span,
            subjects: Vec<Entry<'s>>,
            /// Subject as written -> index in `subjects`.
            index: FxHashMap<&'s str, usize>,
        },
    }
    let union = |x: Span, y: Span| Span::new(x.start.min(y.start), x.end.max(y.end));
    let mut top = Vec::new();
    let mut graphs: FxHashMap<GraphName, usize> = FxHashMap::default();
    for st in &a.parsed.statements {
        let Some((g, label)) = st.graph else {
            top.push(Top::Statement(st));
            continue;
        };
        // The subject as written: an IRI, or a blank node label.
        let name = match st.subject {
            Some((_, span)) => span,
            None => {
                let first = tokenize(written(text, st.span))
                    .into_iter()
                    .find(|t| t.kind != Kind::Comment);
                let Some(t) = first.filter(|t| t.kind == Kind::BNode) else {
                    continue;
                };
                Span::new(st.span.start + t.start, st.span.start + t.end)
            }
        };
        let i = *graphs.entry(g).or_insert_with(|| {
            top.push(Top::Graph {
                label,
                range: st.span,
                subjects: Vec::new(),
                index: FxHashMap::default(),
            });
            top.len() - 1
        });
        let Top::Graph {
            range,
            subjects,
            index,
            ..
        } = &mut top[i]
        else {
            unreachable!()
        };
        *range = union(*range, st.span);
        match index.get(written(text, name)) {
            Some(&k) => subjects[k].range = union(subjects[k].range, st.span),
            None => {
                index.insert(written(text, name), subjects.len());
                subjects.push(Entry {
                    st,
                    name,
                    range: st.span,
                });
            }
        }
    }
    top.into_iter()
        .filter_map(|t| match t {
            Top::Statement(st) => subject(st),
            Top::Graph {
                label,
                range,
                subjects,
                ..
            } => Some(DocumentSymbol {
                name: written(text, label).to_string(),
                detail: Some("graph".into()),
                kind: SymbolKind::NAMESPACE,
                tags: None,
                deprecated: None,
                range: a.range(enc, range),
                selection_range: a.range(enc, label),
                children: Some(
                    subjects
                        .into_iter()
                        .map(|e| {
                            let iri = e.st.subject.as_ref().map(|(iri, _)| iri);
                            DocumentSymbol {
                                name: written(text, e.name).to_string(),
                                detail: iri.and_then(detail),
                                kind: iri.map_or(SymbolKind::OBJECT, |i| kind_for(a, i)),
                                tags: None,
                                deprecated: None,
                                range: a.range(enc, e.range),
                                selection_range: a.range(enc, e.name),
                                children: None,
                            }
                        })
                        .collect(),
                ),
            }),
        })
        .collect()
}

#[allow(deprecated)]
pub fn workspace_symbols(k: &Knowledge, enc: Encoding, query: &str) -> Vec<SymbolInformation> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    // Each description site is a symbol, named with that file's prefixes.
    let mut prefixes: FxHashMap<DocId, PrefixMap> = FxHashMap::default();
    for iri in k.described_terms() {
        let label = k.label(&iri);
        let mut kind = None;
        for d in k.descriptions(&iri) {
            if d.source == Source::External {
                continue;
            }
            let a = d.analysis;
            let p = prefixes.entry(d.doc).or_insert_with(|| a.prefix_map());
            let name =
                crate::features::completion::shorten(&iri, p).unwrap_or_else(|| format!("<{iri}>"));
            if !q.is_empty()
                && !name.to_lowercase().contains(&q)
                && !label.is_some_and(|l| l.to_lowercase().contains(&q))
            {
                continue;
            }
            out.push(SymbolInformation {
                name,
                kind: *kind.get_or_insert_with(|| kind_of(&k.types(&iri))),
                tags: None,
                deprecated: None,
                location: Location {
                    uri: uri(&a.uri),
                    range: a.range(enc, d.spans[0]),
                },
                container_name: label.map(str::to_string),
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
    // Nested [ ], ( ), {| |}, << >> blocks and TriG graph blocks { }. When the closer starts its own line,
    // stop the fold one line earlier so the closer stays visible.
    let mut nested = Vec::new();
    let mut open: Vec<(Kind, u32)> = Vec::new();
    let mut prev_line = 0;
    for t in a.parsed.tokens.iter().filter(|t| t.kind != Kind::Comment) {
        let closes = match t.kind {
            Kind::LBracket
            | Kind::LParen
            | Kind::LAnnotation
            | Kind::LTriple
            | Kind::LTripleTerm
            | Kind::LBrace => {
                open.push((t.kind, t.start));
                None
            }
            Kind::RBrace => Some(Kind::LBrace),
            Kind::RBracket => Some(Kind::LBracket),
            Kind::RParen => Some(Kind::LParen),
            Kind::RAnnotation => Some(Kind::LAnnotation),
            Kind::RTriple => Some(Kind::LTriple),
            Kind::RTripleTerm => Some(Kind::LTripleTerm),
            _ => None,
        };
        let here = line(t.start);
        if let Some(opener) = closes
            && let Some(i) = open.iter().rposition(|(k, _)| *k == opener)
        {
            let (_, start) = open.remove(i);
            let end_line = if here > prev_line { here - 1 } else { here };
            if end_line > line(start) {
                nested.push(FoldingRange {
                    start_line: line(start),
                    end_line,
                    ..Default::default()
                });
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
