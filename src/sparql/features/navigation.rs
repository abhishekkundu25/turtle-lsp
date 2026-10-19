//! Definition, references and highlights in SPARQL documents. The cursor is
//! on a variable (scoped by the structure), a prefix (the prefix part of a
//! prefixed name, or the name of a `PREFIX` line) or a term (an IRI,
//! prefixed name or `a`), as for Turtle in `features::navigation`.

use crate::analysis::{Analysis, Encoding, SparqlAnalysis};
use crate::features::navigation::{self as rdf, uri};
use crate::knowledge::Knowledge;
use crate::sparql::{Form, PrefixSource, TermRef, VarOcc};
use crate::syntax::{PrefixMap, Span};
use lsp_types::*;
use std::sync::Arc;

pub(super) enum Target<'a> {
    Var(&'a VarOcc),
    /// A prefix name, at `span` (the name without its colon).
    Prefix {
        name: String,
        span: Span,
    },
    Term(&'a TermRef),
}

pub(super) fn target_at(q: &SparqlAnalysis, offset: u32) -> Option<Target<'_>> {
    let st = q.doc.structure();
    if let Some(v) = st.vars.iter().find(|v| v.span.contains(offset)) {
        return Some(Target::Var(v));
    }
    if let Some(p) = st.prefixes.iter().find(|p| p.name_span.contains(offset)) {
        return Some(Target::Prefix {
            name: p.name.clone(),
            span: name_only(p.name_span.start, &p.name),
        });
    }
    let r = st.refs.iter().find(|r| r.span.contains(offset))?;
    if let Form::PName { prefix_len } = r.form
        && offset <= r.span.start + prefix_len
    {
        let span = Span::new(r.span.start, r.span.start + prefix_len);
        return Some(Target::Prefix {
            name: span_text(q, span).to_string(),
            span,
        });
    }
    Some(Target::Term(r))
}

/// The span of prefix `name` written at `start` (its colon left out).
pub(super) fn name_only(start: u32, name: &str) -> Span {
    Span::new(start, start + name.len() as u32)
}

pub(super) fn span_text(q: &SparqlAnalysis, span: Span) -> &str {
    q.text()
        .get(span.start as usize..span.end as usize)
        .unwrap_or_default()
}

/// The prefix of a prefixed name as written (`ex` of `ex:a`).
pub(super) fn prefix_of<'a>(q: &'a SparqlAnalysis, r: &TermRef) -> Option<&'a str> {
    match r.form {
        Form::PName { prefix_len } => Some(span_text(
            q,
            Span::new(r.span.start, r.span.start + prefix_len),
        )),
        _ => None,
    }
}

/// The occurrences of the variable `v` in its scope, in text order.
pub(super) fn occurrences<'a>(q: &'a SparqlAnalysis, v: &VarOcc) -> Vec<&'a VarOcc> {
    q.doc.structure().var_occurrences(v.span.start)
}

/// The prefixes in effect at `offset`: the external ones, then the `PREFIX`
/// lines before it.
pub(super) fn scope_at(q: &SparqlAnalysis, offset: u32) -> PrefixMap {
    let mut m = (*q.external).clone();
    for p in q
        .doc
        .structure()
        .prefixes
        .iter()
        .take_while(|p| p.span.end <= offset)
    {
        m.insert(
            p.name.clone(),
            p.resolved.clone().unwrap_or_else(|| p.iri.clone()),
        );
    }
    m
}

fn loc(q: &SparqlAnalysis, enc: Encoding, span: Span) -> Location {
    Location {
        uri: uri(&q.uri),
        range: q.range(enc, span),
    }
}

/// The prefix spans (`ex` of each `ex:a`) of prefix `name`.
fn prefix_uses<'a>(q: &'a SparqlAnalysis, name: &'a str) -> impl Iterator<Item = Span> + 'a {
    q.doc
        .structure()
        .refs
        .iter()
        .filter(move |r| prefix_of(q, r) == Some(name))
        .map(move |r| name_only(r.span.start, name))
}

fn prefix_decls<'a>(q: &'a SparqlAnalysis, name: &'a str) -> impl Iterator<Item = Span> + 'a {
    q.doc
        .structure()
        .prefixes
        .iter()
        .filter(move |p| p.name == name)
        .map(move |p| name_only(p.name_span.start, name))
}

pub fn definition(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    prefix_file: Option<Arc<Analysis>>,
) -> Vec<Location> {
    let Some(offset) = q.offset(enc, pos) else {
        return vec![];
    };
    let st = q.doc.structure();
    match target_at(q, offset) {
        Some(Target::Var(v)) => occurrences(q, v)
            .first()
            .map(|first| loc(q, enc, first.span))
            .into_iter()
            .collect(),
        Some(Target::Prefix { name, span }) => {
            // The declaration in effect for a prefixed name, else the first.
            let r = st.refs.iter().find(|r| r.span.start == span.start);
            let decl = match r.and_then(|r| r.prefix) {
                Some(PrefixSource::Declared(i)) => st.prefixes.get(i as usize),
                _ => None,
            }
            .or_else(|| st.prefixes.iter().find(|p| p.name == name));
            if let Some(p) = decl {
                return vec![loc(q, enc, p.name_span)];
            }
            prefix_file
                .and_then(|pf| {
                    let p = pf.parsed.prefixes.iter().find(|p| p.name == name)?;
                    Some(Location {
                        uri: uri(&pf.uri),
                        range: pf.range(enc, p.name_span),
                    })
                })
                .into_iter()
                .collect()
        }
        Some(Target::Term(r)) => match r.iri.as_deref() {
            Some(iri) => rdf::descriptions(k, enc, iri, &q.uri),
            None => vec![],
        },
        None => vec![],
    }
}

/// Uses of `iri` in SPARQL documents `docs`.
fn uses_in(docs: &[&SparqlAnalysis], iri: &str, enc: Encoding) -> Vec<Location> {
    let mut out = Vec::new();
    for d in docs {
        for r in &d.doc.structure().refs {
            if r.iri.as_deref() == Some(iri) {
                out.push(loc(d, enc, r.span));
            }
        }
    }
    out
}

/// `q` and the other SPARQL documents of the workspace, each once.
pub(super) fn with_others<'a>(
    q: &'a SparqlAnalysis,
    others: &'a [Arc<SparqlAnalysis>],
) -> Vec<&'a SparqlAnalysis> {
    std::iter::once(q)
        .chain(others.iter().map(|o| &**o).filter(|o| o.uri != q.uri))
        .collect()
}

pub fn references(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
    include_declaration: bool,
    others: &[Arc<SparqlAnalysis>],
) -> Vec<Location> {
    let Some(offset) = q.offset(enc, pos) else {
        return vec![];
    };
    match target_at(q, offset) {
        Some(Target::Var(v)) => occurrences(q, v)
            .iter()
            .skip(usize::from(!include_declaration))
            .map(|o| loc(q, enc, o.span))
            .collect(),
        Some(Target::Prefix { name, .. }) => {
            let mut out: Vec<Location> = Vec::new();
            if include_declaration {
                out.extend(prefix_decls(q, &name).map(|s| loc(q, enc, s)));
            }
            out.extend(prefix_uses(q, &name).map(|s| loc(q, enc, s)));
            out
        }
        Some(Target::Term(r)) => {
            let Some(iri) = r.iri.as_deref() else {
                return vec![];
            };
            let mut out = uses_in(&with_others(q, others), iri, enc);
            out.extend(rdf::term_references(k, enc, iri, include_declaration));
            out
        }
        None => vec![],
    }
}

pub fn highlights(q: &SparqlAnalysis, enc: Encoding, pos: Position) -> Vec<DocumentHighlight> {
    let Some(offset) = q.offset(enc, pos) else {
        return vec![];
    };
    let mk = |span: Span, write: bool| DocumentHighlight {
        range: q.range(enc, span),
        kind: Some(if write {
            DocumentHighlightKind::WRITE
        } else {
            DocumentHighlightKind::READ
        }),
    };
    match target_at(q, offset) {
        Some(Target::Var(v)) => occurrences(q, v)
            .iter()
            .map(|o| mk(o.span, o.role.binds()))
            .collect(),
        Some(Target::Prefix { name, .. }) => prefix_decls(q, &name)
            .map(|s| mk(s, true))
            .chain(prefix_uses(q, &name).map(|s| mk(s, false)))
            .collect(),
        Some(Target::Term(r)) => match r.iri.as_deref() {
            Some(iri) => q
                .doc
                .structure()
                .refs
                .iter()
                .filter(|o| o.iri.as_deref() == Some(iri))
                .map(|o| mk(o.span, false))
                .collect(),
            None => vec![],
        },
        None => vec![],
    }
}

pub fn term_references(
    iri: &str,
    _include_declaration: bool,
    docs: &[Arc<SparqlAnalysis>],
    enc: Encoding,
) -> Vec<Location> {
    let docs: Vec<&SparqlAnalysis> = docs.iter().map(|d| &**d).collect();
    uses_in(&docs, iri, enc)
}
