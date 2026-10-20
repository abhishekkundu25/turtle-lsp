//! Semantic tokens: IRIs coloured by what they denote.
//!
//! Each IRI is classified once per request, in this order: the types the
//! workspace gives it (owl/rdfs classes, property types, `sh:NodeShape`,
//! datatypes), the well-known datatypes, the vocabulary term kind, what the
//! workspace does with it (used as a predicate ⇒ property, used as a type ⇒
//! class), and last the role it has where it is written (predicate ⇒
//! property, object of `rdf:type` ⇒ class, datatype ⇒ type, else an
//! individual). The prefix part of a prefixed name is a `namespace` token.
//! Literals, numbers and comments are left to the grammar.

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::{Knowledge, namespace_of};
use crate::syntax::lexer::Kind;
use crate::syntax::{Form, Iri, IriMap, Node, RDF, RDF_TYPE, Role, XSD};
use crate::vocab::{TermKind, VOCABS};
use lsp_types::{
    Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend,
};
use rustc_hash::FxHashSet;

// Token types, as indexes into the legend.
pub const NAMESPACE: u32 = 0;
pub const CLASS: u32 = 1;
pub const PROPERTY: u32 = 2;
pub const INDIVIDUAL: u32 = 3;
pub const DATATYPE: u32 = 4;
pub const KEYWORD: u32 = 5;
/// SPARQL variables and built-in functions, after the RDF types so that
/// their indexes stay put.
pub const VARIABLE: u32 = 6;
pub const FUNCTION: u32 = 7;

// Token modifiers, as bits.
pub const DECLARATION: u32 = 1 << 0;
pub const DEPRECATED: u32 = 1 << 1;
pub const DEFAULT_LIBRARY: u32 = 1 << 2;
pub const UNDESCRIBED: u32 = 1 << 3;

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::CLASS,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::TYPE,
            SemanticTokenType::KEYWORD,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::FUNCTION,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DECLARATION,
            SemanticTokenModifier::DEPRECATED,
            SemanticTokenModifier::DEFAULT_LIBRARY,
            SemanticTokenModifier::new("undescribed"),
        ],
    }
}

/// Tokens for the whole document.
pub fn full(a: &Analysis, k: &Knowledge, enc: Encoding) -> SemanticTokens {
    encode(a, enc, &raw_tokens(a, k, 0, u32::MAX))
}

/// Tokens that overlap `range`.
pub fn range(a: &Analysis, k: &Knowledge, enc: Encoding, range: Range) -> SemanticTokens {
    let from = a.offset(enc, range.start).unwrap_or(0);
    let to = a.offset(enc, range.end).unwrap_or(u32::MAX).max(from);
    encode(a, enc, &raw_tokens(a, k, from, to))
}

/// One token as a byte span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Raw {
    pub start: u32,
    pub end: u32,
    pub ty: u32,
    pub mods: u32,
}

/// What an IRI denotes regardless of where it is written: a token type when
/// one is known, and its modifiers.
#[derive(Clone, Copy)]
pub(crate) struct Class {
    pub ty: Option<u32>,
    pub mods: u32,
}

/// Sorted, non-overlapping tokens overlapping `from..to` (a token that only
/// touches `to` is included, so a range ending at a token's start keeps it).
fn raw_tokens(a: &Analysis, k: &Knowledge, from: u32, to: u32) -> Vec<Raw> {
    let p = &a.parsed;
    if p.syntax == crate::syntax::Syntax::JsonLd {
        return jsonld_tokens(a, k, from, to);
    }
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

    for t in &p.tokens {
        if matches!(
            t.kind,
            Kind::PrefixDirective
                | Kind::BaseDirective
                | Kind::VersionDirective
                | Kind::SparqlPrefix
                | Kind::SparqlBase
                | Kind::SparqlVersion
        ) {
            push(t.start, t.end, KEYWORD, 0);
        }
    }
    for d in &p.prefixes {
        push(d.name_span.start, d.name_span.end, NAMESPACE, DECLARATION);
    }
    // Prefixes that resolve to nothing still read as prefixes.
    for u in p.prefix_uses.iter().filter(|u| !u.resolved) {
        let name_end = u.span.start + u.name.len() as u32;
        push(u.span.start, name_end.min(u.span.end), NAMESPACE, 0);
    }

    // Objects of rdf:type, by start offset.
    let rdf_type = Iri::get(RDF_TYPE);
    let type_objects: FxHashSet<u32> = p
        .triples
        .iter()
        .filter(|t| Some(t.p) == rdf_type && matches!(t.o, Node::Iri(_)))
        .map(|t| t.o_span.start)
        .collect();
    let implied = implied_kinds(p);

    let mut classes: IriMap<Class> = IriMap::default();
    let refs = &p.refs;
    let first = refs.partition_point(|r| r.span.end <= from);
    for r in refs[first..].iter().take_while(|r| r.span.start <= to) {
        let (start, end) = (r.span.start, r.span.end);
        let local_start = match r.form {
            Form::A => {
                push(start, end, KEYWORD, 0);
                continue;
            }
            Form::PName { prefix_len } => {
                push(start, start + prefix_len, NAMESPACE, 0);
                start + prefix_len + 1
            }
            Form::Iri => start,
        };
        let c = *classes
            .entry(r.iri)
            .or_insert_with(|| classify(k, r.iri.as_str()));
        let ty =
            c.ty.or_else(|| implied.get(&r.iri).copied())
                .unwrap_or(match r.role {
                    Role::Predicate => PROPERTY,
                    Role::Datatype => DATATYPE,
                    _ if type_objects.contains(&start) => CLASS,
                    _ => INDIVIDUAL,
                });
        let decl = if r.role == Role::Subject {
            DECLARATION
        } else {
            0
        };
        push(local_start, end, ty, c.mods | decl);
    }

    sort_and_dedupe(&mut out);
    out
}

/// JSON-LD tokens from the source index: keywords and aliases are keywords,
/// keys that expand to an IRI are properties, `@type` values are classes,
/// and term definitions in this document are declarations.
fn jsonld_tokens(a: &Analysis, k: &Knowledge, from: u32, to: u32) -> Vec<Raw> {
    use crate::jsonld::source::{Origin, UseKind, UseRole};
    let idx = &a.source_index;
    let wanted = |s: u32, e: u32| e > from && s <= to && s < e;
    let mut classes: rustc_hash::FxHashMap<std::sync::Arc<str>, Class> = Default::default();
    let mut classify_iri = |iri: &std::sync::Arc<str>| {
        *classes
            .entry(iri.clone())
            .or_insert_with(|| classify(k, iri))
    };
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
    let first = idx.uses.partition_point(|u| u.span.end <= from);
    for u in idx.uses[first..].iter().take_while(|u| u.span.start <= to) {
        let (start, end) = (u.span.start, u.span.end);
        match u.kind {
            UseKind::Keyword | UseKind::Alias { .. } => push(start, end, KEYWORD, 0),
            _ => {
                let Some(iri) = &u.iri else { continue };
                let c = classify_iri(iri);
                match u.role {
                    UseRole::Type => push(start, end, CLASS, c.mods),
                    UseRole::Key => push(start, end, PROPERTY, c.mods),
                    UseRole::Id => {}
                }
            }
        }
    }
    for d in idx.definitions.iter().filter(|d| d.origin == Origin::Document) {
        let Some(iri) = &d.iri else { continue };
        if crate::jsonld::context::is_keyword(iri) {
            push(d.key.start, d.key.end, KEYWORD, DECLARATION);
        } else if !iri.starts_with("_:") {
            let c = classify_iri(iri);
            push(d.key.start, d.key.end, PROPERTY, c.mods | DECLARATION);
        }
    }
    sort_and_dedupe(&mut out);
    out
}

/// Sorts tokens by start and drops any that overlaps the one before it
/// (overlaps only come from damaged text; the first token is kept).
pub(crate) fn sort_and_dedupe(out: &mut Vec<Raw>) {
    if !out.is_sorted_by_key(|t| t.start) {
        out.sort_by_key(|t| t.start);
    }
    let mut end = 0;
    out.retain(|t| {
        let keep = t.start >= end;
        if keep {
            end = t.end;
        }
        keep
    });
}

/// Schema links that say what their subject / object is, e.g. both ends of
/// `rdfs:subClassOf` are classes and the subject of `rdfs:domain` is a property.
const SCHEMA_LINKS: &[(&str, Option<u32>, Option<u32>)] = &[
    (
        "http://www.w3.org/2000/01/rdf-schema#subClassOf",
        Some(CLASS),
        Some(CLASS),
    ),
    (
        "http://www.w3.org/2002/07/owl#equivalentClass",
        Some(CLASS),
        Some(CLASS),
    ),
    (
        "http://www.w3.org/2002/07/owl#disjointWith",
        Some(CLASS),
        Some(CLASS),
    ),
    (
        "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
        Some(PROPERTY),
        Some(PROPERTY),
    ),
    (
        "http://www.w3.org/2002/07/owl#equivalentProperty",
        Some(PROPERTY),
        Some(PROPERTY),
    ),
    (
        "http://www.w3.org/2002/07/owl#inverseOf",
        Some(PROPERTY),
        Some(PROPERTY),
    ),
    (
        "http://www.w3.org/2000/01/rdf-schema#domain",
        Some(PROPERTY),
        Some(CLASS),
    ),
    (
        "http://www.w3.org/2000/01/rdf-schema#range",
        Some(PROPERTY),
        Some(CLASS),
    ),
    (
        "http://www.w3.org/2002/07/owl#onProperty",
        None,
        Some(PROPERTY),
    ),
    ("http://www.w3.org/ns/shacl#targetClass", None, Some(CLASS)),
    ("http://www.w3.org/ns/shacl#class", None, Some(CLASS)),
    ("http://www.w3.org/ns/shacl#path", None, Some(PROPERTY)),
];

/// Term kinds this document's schema links imply, first link wins.
fn implied_kinds(p: &crate::syntax::Parsed) -> IriMap<u32> {
    let links: Vec<(Iri, Option<u32>, Option<u32>)> = SCHEMA_LINKS
        .iter()
        .filter_map(|&(iri, s, o)| Some((Iri::get(iri)?, s, o)))
        .collect();
    let mut out = IriMap::default();
    if links.is_empty() {
        return out;
    }
    for t in &p.triples {
        let Some(&(_, s, o)) = links.iter().find(|l| l.0 == t.p) else {
            continue;
        };
        if let (Node::Iri(i), Some(kind)) = (&t.s, s) {
            out.entry(*i).or_insert(kind);
        }
        if let (Node::Iri(i), Some(kind)) = (&t.o, o) {
            out.entry(*i).or_insert(kind);
        }
    }
    out
}

const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const SH_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";

/// The type an `rdf:type` value gives its subject, for the types that say
/// what kind of term it is.
fn meta_type(t: &str) -> Option<u32> {
    if let Some(local) = t.strip_prefix(OWL) {
        return match local {
            "Class" | "Restriction" => Some(CLASS),
            "NamedIndividual" => Some(INDIVIDUAL),
            _ if local.ends_with("Property") => Some(PROPERTY),
            _ => None,
        };
    }
    match t {
        _ if t == SH_NODE_SHAPE => Some(CLASS),
        _ if t.strip_prefix(RDFS) == Some("Class") => Some(CLASS),
        _ if t.strip_prefix(RDFS) == Some("Datatype") => Some(DATATYPE),
        _ if t.strip_prefix(RDF) == Some("Property") => Some(PROPERTY),
        _ => None,
    }
}

/// Built-in datatypes: XSD's, rdf's literal types and rdfs:Literal.
fn is_datatype(iri: &str) -> bool {
    if let Some(local) = iri.strip_prefix(XSD) {
        return local.chars().next().is_some_and(char::is_alphabetic)
            && VOCABS
                .lookup(iri)
                .and_then(|(v, l)| v.term(l))
                .is_none_or(|t| t.kind == TermKind::Class);
    }
    matches!(
        iri.strip_prefix(RDF),
        Some("langString" | "dirLangString" | "HTML" | "XMLLiteral" | "JSON" | "PlainLiteral")
    ) || iri.strip_prefix(RDFS) == Some("Literal")
}

/// What `iri` denotes, from the workspace and the vocabularies.
pub(crate) fn classify(k: &Knowledge, iri: &str) -> Class {
    let term = VOCABS.lookup(iri);
    let vocab_term = term.and_then(|(v, local)| v.term(local));
    let mut mods = 0;
    if term.is_some() {
        mods |= DEFAULT_LIBRARY;
    }
    if vocab_term.is_some_and(|t| t.deprecated) {
        mods |= DEPRECATED;
    }
    if k.is_own_namespace(namespace_of(iri)) && !k.is_described(iri) {
        mods |= UNDESCRIBED;
    }
    let ty = k
        .types(iri)
        .iter()
        .find_map(|t| meta_type(t))
        .or_else(|| is_datatype(iri).then_some(DATATYPE))
        .or_else(|| {
            vocab_term.map(|t| match t.kind {
                TermKind::Class => CLASS,
                TermKind::Property => PROPERTY,
                TermKind::Individual => INDIVIDUAL,
            })
        })
        .or_else(|| k.used_as_predicate(iri).then_some(PROPERTY))
        .or_else(|| (!k.instances(iri).is_empty()).then_some(CLASS));
    Class { ty, mods }
}

/// Width of `s` in the client's position encoding.
fn width(enc: Encoding, s: &str) -> u32 {
    match enc {
        Encoding::Utf8 => s.len() as u32,
        Encoding::Utf16 if s.is_ascii() => s.len() as u32,
        Encoding::Utf16 => s.chars().map(|c| c.len_utf16() as u32).sum(),
    }
}

/// Relative LSP encoding. Tokens never span lines (IRIs and prefixed names
/// cannot contain line breaks).
fn encode(a: &Analysis, enc: Encoding, raw: &[Raw]) -> SemanticTokens {
    encode_with(&a.text, enc, raw, |o| a.position(enc, o))
}

/// [`encode`] for any document: `position` maps a byte offset of `text`.
pub(crate) fn encode_with(
    text: &str,
    enc: Encoding,
    raw: &[Raw],
    position: impl Fn(u32) -> lsp_types::Position,
) -> SemanticTokens {
    let mut data = Vec::with_capacity(raw.len());
    let (mut line, mut col) = (0, 0);
    for t in raw {
        let s = &text[t.start as usize..t.end as usize];
        if s.contains(['\n', '\r']) {
            continue;
        }
        let p = position(t.start);
        let delta_start = if p.line == line {
            p.character - col
        } else {
            p.character
        };
        data.push(SemanticToken {
            delta_line: p.line - line,
            delta_start,
            length: width(enc, s),
            token_type: t.ty,
            token_modifiers_bitset: t.mods,
        });
        (line, col) = (p.line, p.character);
    }
    SemanticTokens {
        result_id: None,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::Source;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    fn tokens(text: &str, enc: Encoding) -> Vec<(String, u32, u32)> {
        let a = Arc::new(Analysis::new(
            "file:///t.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///t.ttl", Source::Open, a.clone());
        // Decode back to (text, type, modifiers) via absolute positions.
        let lines: Vec<&str> = text.split('\n').collect();
        let (mut line, mut col) = (0u32, 0u32);
        full(&a, &k, enc)
            .data
            .iter()
            .map(|t| {
                if t.delta_line > 0 {
                    col = 0;
                }
                line += t.delta_line;
                col += t.delta_start;
                let l = lines[line as usize];
                let s: String = match enc {
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
    fn classifies_terms_by_knowledge_vocabulary_and_role() {
        let text = "@prefix ex: <http://example.com/ns#> .\n\
                    PREFIX owl: <http://www.w3.org/2002/07/owl#>\n\
                    ex:Pump a owl:Class .\n\
                    ex:p1 a ex:Pump ; ex:pressure \"1\"^^<http://www.w3.org/2001/XMLSchema#int> .\n\
                    ex:pressure ex:note ex:p1 .\n\
                    ex:Big <http://www.w3.org/2000/01/rdf-schema#subClassOf> ex:Thing .\n\
                    ex:note ex:pressure \"n\" .\n";
        let t = tokens(text, Encoding::Utf16);
        let find = |s: &str| {
            t.iter()
                .filter(|x| x.0 == s)
                .map(|x| (x.1, x.2))
                .collect::<Vec<_>>()
        };
        assert_eq!(find("@prefix"), [(KEYWORD, 0)]);
        assert_eq!(find("PREFIX"), [(KEYWORD, 0)]);
        assert_eq!(find("a"), [(KEYWORD, 0), (KEYWORD, 0)]);
        assert_eq!(find("ex")[0], (NAMESPACE, DECLARATION));
        assert_eq!(find("ex")[1], (NAMESPACE, 0));
        // Declared owl:Class: a class everywhere, a declaration as subject.
        assert_eq!(find("Pump"), [(CLASS, DECLARATION), (CLASS, 0)]);
        assert_eq!(find("Class"), [(CLASS, DEFAULT_LIBRARY)]);
        // Used as a predicate somewhere: a property, even as subject.
        assert_eq!(
            find("pressure"),
            [(PROPERTY, 0), (PROPERTY, DECLARATION), (PROPERTY, 0)]
        );
        assert_eq!(find("p1"), [(INDIVIDUAL, DECLARATION), (INDIVIDUAL, 0)]);
        // Both ends of rdfs:subClassOf are classes, even undeclared ones.
        assert_eq!(find("Big"), [(CLASS, DECLARATION)]);
        assert_eq!(find("Thing"), [(CLASS, UNDESCRIBED)]);
        assert_eq!(
            find("<http://www.w3.org/2001/XMLSchema#int>"),
            [(DATATYPE, DEFAULT_LIBRARY)]
        );
    }

    #[test]
    fn utf16_and_utf8_columns_and_lengths() {
        let text =
            "@prefix ex: <http://example.com/ns#> .\nex:é😀 ex:p <http://example.com/ns#ü> .\n";
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            let t = tokens(text, enc);
            let names: Vec<&str> = t.iter().map(|x| x.0.as_str()).collect();
            assert_eq!(
                names,
                [
                    "@prefix",
                    "ex",
                    "ex",
                    "é😀",
                    "ex",
                    "p",
                    "<http://example.com/ns#ü>"
                ],
                "{enc:?}"
            );
        }
    }

    #[test]
    fn deprecated_and_undescribed() {
        let mut text = String::from(
            "@prefix ex: <http://example.com/ns#> .\n@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n",
        );
        // Enough described terms for ex: to count as an own namespace (80%).
        for i in 0..8 {
            text.push_str(&format!("ex:t{i} ex:p \"x\" .\n"));
        }
        text.push_str("ex:p ex:p \"x\" .\nex:q ex:p \"x\" .\n");
        text.push_str("ex:t0 a vcard:Email ; ex:q ex:ghost .\n");
        let t = tokens(&text, Encoding::Utf16);
        let get = |s: &str| t.iter().find(|x| x.0 == s).map(|x| (x.1, x.2)).unwrap();
        assert_eq!(get("Email"), (CLASS, DEPRECATED | DEFAULT_LIBRARY));
        assert_eq!(get("ghost"), (INDIVIDUAL, UNDESCRIBED));
        assert_eq!(get("t1"), (INDIVIDUAL, DECLARATION));
    }

    #[test]
    fn range_keeps_tokens_overlapping_it_and_tokens_never_overlap() {
        let text = "@prefix ex: <http://example.com/ns#> .\nex:a ex:p ex:b .\nex:c ex:p ex:d .\n";
        let a = Analysis::new(
            "file:///t.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        );
        let k = Knowledge::default();
        let r = Range::new(
            lsp_types::Position::new(1, 6),
            lsp_types::Position::new(1, 8),
        );
        let got = range(&a, &k, Encoding::Utf16, r).data;
        // `ex` and `p` of ex:p.
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].delta_line, got[0].delta_start), (1, 5));
        let raw = raw_tokens(&a, &k, 0, u32::MAX);
        assert!(raw.windows(2).all(|w| w[0].end <= w[1].start));
    }
}
