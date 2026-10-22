//! Hover, label inlay hints and semantic tokens for SPARQL documents, with
//! the Turtle presentation (`features::{hover, inlay, semantic}`).

use crate::analysis::{Encoding, SparqlAnalysis};
use crate::features::hover::{describe_iri, md, short};
use crate::features::inlay::{label_hint, local_of};
use crate::features::semantic::{
    self as rdf, CLASS, DATATYPE, DECLARATION, FUNCTION, INDIVIDUAL, KEYWORD, NAMESPACE, PROPERTY,
    Raw, VARIABLE,
};
use crate::knowledge::Knowledge;
use crate::settings::InlayLabels;
use crate::sparql::{Form, PrefixSource, TermRef, TermRole, TokenKind};
use crate::syntax::{RDF_TYPE, Span};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};

/// The term whose span holds `offset`.
fn ref_at(q: &SparqlAnalysis, offset: u32) -> Option<&TermRef> {
    q.doc
        .structure()
        .refs
        .iter()
        .find(|r| r.span.contains(offset))
}

/// `textDocument/hover`: a `PREFIX` name or a prefixed name's prefix shows
/// its namespace; a term what the workspace and the vocabularies say of it,
/// exactly as in Turtle.
pub(crate) fn hover(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
) -> Option<Hover> {
    let offset = q.offset(enc, pos)?;
    let st = q.doc.structure();
    if let Some(p) = st.prefixes.iter().find(|p| p.name_span.contains(offset)) {
        let ns = p.resolved.as_deref().unwrap_or(&p.iri);
        return Some(md(
            format!("**{}:** → `<{ns}>`", p.name),
            q.range(enc, p.name_span),
        ));
    }
    let r = ref_at(q, offset)?;
    let prefixes = q.prefix_map();
    let label = match r.form {
        Form::PName { prefix_len } if offset <= r.span.start + prefix_len => {
            let name = &q.text()[r.span.start as usize..(r.span.start + prefix_len) as usize];
            let origin = match r.prefix? {
                PrefixSource::Declared(_) => "declared in this file",
                PrefixSource::External => "from the project prefixes",
                PrefixSource::Undeclared => return None,
            };
            let ns = prefixes.get(name)?;
            return Some(md(
                format!("**{name}:** → `<{ns}>`  \n_{origin}_"),
                q.range(enc, r.span),
            ));
        }
        Form::A => "a (rdf:type)".to_string(),
        _ => short(r.iri.as_deref()?, &prefixes),
    };
    let body = describe_iri(r.iri.as_deref()?, &label, k, &prefixes)?;
    Some(md(body, q.range(enc, r.span)))
}

/// `textDocument/inlayHint`: label hints after terms in `range`, as in Turtle.
pub(crate) fn inlay_hints(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    range: Range,
    mode: InlayLabels,
) -> Vec<InlayHint> {
    if mode == InlayLabels::Off {
        return vec![];
    }
    let from = q.offset(enc, range.start).unwrap_or(0);
    let to = q.offset(enc, range.end).unwrap_or(u32::MAX).max(from);
    let text = q.text();
    let mut out: Vec<InlayHint> = Vec::new();
    let mut refs: Vec<&TermRef> = q
        .doc
        .structure()
        .refs
        .iter()
        .filter(|r| r.span.end >= from && r.span.start <= to)
        .collect();
    refs.sort_by_key(|r| r.span.start);
    for r in refs {
        let Some(iri) = r.iri.as_deref() else {
            continue;
        };
        let local = match r.form {
            Form::A => continue,
            Form::PName { prefix_len } => {
                &text[(r.span.start + prefix_len + 1) as usize..r.span.end as usize]
            }
            Form::Iri => local_of(iri),
        };
        out.extend(label_hint(k, iri, local, mode, || {
            q.position(enc, r.span.end)
        }));
    }
    out
}

pub(crate) fn semantic_tokens_full(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
) -> SemanticTokens {
    encode(q, enc, &raw_tokens(q, k, 0, u32::MAX))
}

pub(crate) fn semantic_tokens_range(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    range: Range,
) -> SemanticTokens {
    let from = q.offset(enc, range.start).unwrap_or(0);
    let to = q.offset(enc, range.end).unwrap_or(u32::MAX).max(from);
    encode(q, enc, &raw_tokens(q, k, from, to))
}

fn encode(q: &SparqlAnalysis, enc: Encoding, raw: &[Raw]) -> SemanticTokens {
    rdf::encode_with(q.text(), enc, raw, |o| q.position(enc, o))
}

/// Sorted, non-overlapping tokens overlapping `from..to`: keywords,
/// variables, functions, prefixes, and terms classified as in Turtle (what
/// the workspace and the vocabularies say, else the role where written).
pub(super) fn raw_tokens(q: &SparqlAnalysis, k: &Knowledge, from: u32, to: u32) -> Vec<Raw> {
    let st = q.doc.structure();
    let wanted = |s: u32, e: u32| e > from && s <= to && s < e;
    let mut out: Vec<Raw> = Vec::new();
    let mut push = |start: u32, end: u32, ty: u32, mods: u32| {
        if wanted(start, end) {
            out.push(Raw {
                start,
                end,
                ty,
                mods,
            });
        }
    };
    for t in q.doc.tokens() {
        if t.span.start > to {
            break;
        }
        let ty = match t.kind {
            TokenKind::Keyword | TokenKind::A => KEYWORD,
            TokenKind::Var => VARIABLE,
            TokenKind::Function => FUNCTION,
            _ => continue,
        };
        push(t.span.start, t.span.end, ty, 0);
    }
    for p in &st.prefixes {
        // The name without its colon.
        push(
            p.name_span.start,
            p.name_span.end.saturating_sub(1),
            NAMESPACE,
            DECLARATION,
        );
    }

    let rdf_type = Some(RDF_TYPE);
    let refs_by_start: FxHashMap<u32, &TermRef> =
        st.refs.iter().map(|r| (r.span.start, r)).collect();
    let type_objects: FxHashSet<u32> = st
        .triples
        .iter()
        .filter(|t| {
            t.predicate.is_some_and(|p| {
                refs_by_start
                    .get(&p.start)
                    .is_some_and(|r| r.span == p && r.iri.as_deref() == rdf_type)
            })
        })
        .filter_map(|t| t.object.map(|o| o.start))
        .collect();
    let mut classes: FxHashMap<&str, rdf::Class> = FxHashMap::default();
    for r in st
        .refs
        .iter()
        .filter(|r| r.span.end > from && r.span.start <= to)
    {
        let Span { start, end } = r.span;
        let local_start = match r.form {
            // A keyword token already.
            Form::A => continue,
            Form::PName { prefix_len } => {
                push(start, start + prefix_len, NAMESPACE, 0);
                start + prefix_len + 1
            }
            Form::Iri => start,
        };
        let Some(iri) = r.iri.as_deref() else {
            continue;
        };
        let c = *classes.entry(iri).or_insert_with(|| rdf::classify(k, iri));
        let ty = c.ty.unwrap_or(match r.role {
            TermRole::Predicate => PROPERTY,
            TermRole::Datatype => DATATYPE,
            TermRole::Function => FUNCTION,
            _ if type_objects.contains(&start) => CLASS,
            _ => INDIVIDUAL,
        });
        push(local_start, end, ty, c.mods);
    }
    rdf::sort_and_dedupe(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    /// (text, type, modifiers) of each token, decoded from the LSP encoding.
    fn decode(text: &str, enc: Encoding, t: &SemanticTokens) -> Vec<(String, u32, u32)> {
        let lines: Vec<&str> = text.split('\n').collect();
        let (mut line, mut col) = (0u32, 0u32);
        t.data
            .iter()
            .map(|t| {
                if t.delta_line > 0 {
                    col = 0;
                }
                line += t.delta_line;
                col += t.delta_start;
                let l = lines[line as usize];
                let s = match enc {
                    Encoding::Utf8 => l[col as usize..(col + t.length) as usize].to_string(),
                    Encoding::Utf16 => {
                        let u: Vec<u16> = l.encode_utf16().collect();
                        String::from_utf16(&u[col as usize..(col + t.length) as usize]).unwrap()
                    }
                };
                (s, t.token_type, t.token_modifiers_bitset)
            })
            .collect()
    }

    #[test]
    fn tokens_in_both_encodings_and_by_range() {
        let text = "PREFIX ex: <http://e/>\nPREFIX : <http://d/>\nselect ?é WHERE { ?é a ex:Ünï ; ex:p :x FILTER(strlen(?é)) }\n";
        let q = SparqlAnalysis::new("file:///q.rq".into(), text, Arc::new(PrefixMap::default()));
        let k = Knowledge::default();
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            let got = decode(text, enc, &semantic_tokens_full(&q, &k, enc));
            let want: Vec<(&str, u32, u32)> = vec![
                ("PREFIX", KEYWORD, 0),
                ("ex", NAMESPACE, DECLARATION),
                ("PREFIX", KEYWORD, 0),
                ("select", KEYWORD, 0),
                ("?é", VARIABLE, 0),
                ("WHERE", KEYWORD, 0),
                ("?é", VARIABLE, 0),
                ("a", KEYWORD, 0),
                ("ex", NAMESPACE, 0),
                ("Ünï", CLASS, 0),
                ("ex", NAMESPACE, 0),
                ("p", PROPERTY, 0),
                ("x", INDIVIDUAL, 0),
                ("FILTER", KEYWORD, 0),
                ("strlen", FUNCTION, 0),
                ("?é", VARIABLE, 0),
            ];
            let want: Vec<(String, u32, u32)> =
                want.into_iter().map(|(s, t, m)| (s.into(), t, m)).collect();
            assert_eq!(got, want, "{enc:?}");
        }
        // Line 2 from `ex:p` on.
        let at = text.lines().nth(2).unwrap().find("ex:p").unwrap() as u32;
        let range = Range::new(Position::new(2, at), Position::new(2, at + 4));
        let got = decode(
            text,
            Encoding::Utf8,
            &semantic_tokens_range(&q, &k, Encoding::Utf8, range),
        );
        let names: Vec<&str> = got.iter().map(|t| t.0.as_str()).collect();
        assert_eq!(names, ["ex", "p"]);
    }
}
