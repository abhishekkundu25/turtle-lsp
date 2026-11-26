//! Completion in RDF/XML.
//!
//! What may come at the cursor is read from the elements open there
//! ([`crate::rdfxml::expected`]), so it holds while the document is half
//! typed:
//!
//! - after `<` where a node element goes: `rdf:Description` and the
//!   classes; where a property element goes: the properties, ranked by the
//!   node's types as Turtle ranks predicates. Each is a qualified name over
//!   the prefixes in scope (a name in the default namespace bare), or over
//!   the prefix of its well-known vocabulary, whose `xmlns:p` declaration is
//!   added to the root element on accept; a prefix bound to another
//!   namespace in scope, or declared on the root, is never declared again.
//!   A local name that is no XML name is left out, and so is the name of an
//!   end tag that would close the new element instead of the element it
//!   closes now. No prefix alone is
//!   offered (`<ex:/>` is no element), and `rdf:Description` keeps its
//!   place under the cap of 200;
//! - in `rdf:about`: the subjects the workspace describes; in
//!   `rdf:resource`: IRIs of the kind the property takes (classes for
//!   `rdf:type` and `rdfs:subClassOf`, properties for `rdfs:subPropertyOf`),
//!   ranked by its range as Turtle ranks objects; in `rdf:datatype`: the
//!   datatypes. Each is written whole, escaped for the value's quotes, or as
//!   `&name;local` in a value that starts with a reference to an entity the
//!   DOCTYPE declares. A part of an IRI typed (`http://example.org/onto#Per`)
//!   or of a local name (`Per`) narrows the list.
//!
//! The item details, the ranking tiers and resolve are Turtle's.

use super::ranking::{self, Ranking, Subject};
use super::{
    Best, Candidate, LIMIT, Slot, finish, fuzzy, guess_kind, items, local_name_candidates_where,
    local_of, namespace_terms_best, rank, starts_with_lowercase, term_item_kind,
};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::ofn::signature::Entity;
use crate::rdfxml::expected::{self, Expected, Node, Value};
use crate::rdfxml::rename::escaped;
use crate::rdfxml::xml::cursor::Document;
use crate::rdfxml::xml::is_nc_name;
use crate::syntax::{Iri, PrefixMap, RDF, RDF_TYPE};
use crate::vocab::{TermKind, VOCABS, split_namespace};
use lsp_types::*;
use rustc_hash::FxHashSet;

/// What item details name as the declaration a term adds.
const DIRECTIVE: &str = "xmlns";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";

/// The properties whose objects are classes, and those whose objects are
/// properties: what an `rdf:resource` of theirs takes.
const CLASS_VALUED: &[&str] = &[
    RDF_TYPE,
    "http://www.w3.org/2000/01/rdf-schema#subClassOf",
    "http://www.w3.org/2000/01/rdf-schema#domain",
    "http://www.w3.org/2002/07/owl#equivalentClass",
    "http://www.w3.org/2002/07/owl#disjointWith",
    "http://www.w3.org/2002/07/owl#complementOf",
    "http://www.w3.org/2002/07/owl#onClass",
];
const PROPERTY_VALUED: &[&str] = &[
    "http://www.w3.org/2000/01/rdf-schema#subPropertyOf",
    "http://www.w3.org/2002/07/owl#equivalentProperty",
    "http://www.w3.org/2002/07/owl#inverseOf",
    "http://www.w3.org/2002/07/owl#onProperty",
    "http://www.w3.org/2002/07/owl#propertyDisjointWith",
];

/// How a term is written where it is offered, and the declaration it adds.
type Written = Option<(String, Option<(String, String)>)>;

pub(super) fn complete(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: usize,
    snippets: bool,
) -> CompletionResponse {
    let text: &str = &a.text;
    let doc = Document::read(text);
    let Some(e) = expected::at(text, &doc, offset) else {
        return CompletionResponse::Array(vec![]);
    };
    let typed = &text[e.span.start as usize..offset];
    // The named prefixes, as the shared helpers write them; the default
    // namespace apart, for bare names.
    let prefixes: PrefixMap = e
        .prefixes
        .iter()
        .filter(|(p, _)| is_nc_name(p))
        .map(|(p, ns)| (p.to_string(), ns.to_string()))
        .collect();
    let range = a.range(enc, e.span);
    match &e.slot {
        expected::Slot::EndTag { name, closed } => {
            return CompletionResponse::List(end_tag(typed, name, *closed, range));
        }
        expected::Slot::Attribute(kinds) => {
            return CompletionResponse::List(attributes(
                a, text, enc, typed, &e, kinds, &prefixes, range, snippets,
            ));
        }
        expected::Slot::Value {
            of: Value::ParseType { allow_triple },
            entity,
            ..
        } => {
            return CompletionResponse::List(parse_types(
                typed,
                entity.is_some(),
                *allow_triple,
                range,
            ));
        }
        _ => {}
    }
    let (cands, incomplete) = match &e.slot {
        expected::Slot::NodeElement => elements(k, typed, &e, None, &prefixes),
        expected::Slot::PropertyElement(node) => elements(k, typed, &e, Some(node), &prefixes),
        expected::Slot::Value { of, quote, entity } => {
            values(k, typed, of, *quote, *entity, &prefixes)
        }
        expected::Slot::EndTag { .. } | expected::Slot::Attribute(_) => unreachable!(),
    };
    let items = items(cands, range, &prefixes, DIRECTIVE, |p, ns| {
        let (at, new_text) = e.declare(text, p, ns)?;
        let pos = a.position(enc, at);
        Some(TextEdit::new(Range::new(pos, pos), new_text))
    });
    debug_assert!(items.len() <= LIMIT);
    CompletionResponse::List(CompletionList {
        is_incomplete: incomplete,
        items,
    })
}

/// Closing-tag completion after `</`: the innermost open element's name,
/// with `>` appended when no `>` already follows.
fn end_tag(typed: &str, name: &str, closed: bool, range: Range) -> CompletionList {
    let mut items = Vec::new();
    if fuzzy(typed, name) {
        let new_text = if closed {
            name.to_string()
        } else {
            format!("{name}>")
        };
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("close element".into()),
            sort_text: Some("0".into()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, new_text))),
            ..Default::default()
        });
    }
    CompletionList {
        is_incomplete: false,
        items,
    }
}

/// Valid RDF/XML syntax attributes for the start tag being written.
#[allow(clippy::too_many_arguments)]
fn attributes(
    a: &Analysis,
    text: &str,
    enc: Encoding,
    typed: &str,
    e: &Expected<'_>,
    kinds: &[expected::AttrKind],
    prefixes: &PrefixMap,
    range: Range,
    snippets: bool,
) -> CompletionList {
    let rdf_prefix: Option<(String, Option<(String, String)>)> =
        if prefixes.get("rdf").map(String::as_str) == Some(RDF) {
            Some(("rdf".into(), None))
        } else if let Some(p) = prefixes
            .iter()
            .filter(|(_, ns)| ns.as_str() == RDF)
            .map(|(p, _)| p)
            .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
        {
            Some((p.clone(), None))
        } else if e.may_declare("rdf") {
            Some(("rdf".into(), Some(("rdf".into(), RDF.into()))))
        } else {
            None
        };
    let mut items = Vec::new();
    for (idx, &kind) in kinds.iter().enumerate() {
        let (label, detail, import) = match kind {
            expected::AttrKind::XmlLang => (
                "xml:lang".to_string(),
                "language tag for literal values",
                None,
            ),
            other => {
                let Some((pfx, imp)) = &rdf_prefix else {
                    continue;
                };
                let (local, desc) = match other {
                    expected::AttrKind::About => ("about", "subject IRI of this node"),
                    expected::AttrKind::Id => {
                        ("ID", "fragment identifier relative to the base IRI")
                    }
                    expected::AttrKind::NodeId => ("nodeID", "blank node identifier"),
                    expected::AttrKind::Type => ("type", "class IRI of this node"),
                    expected::AttrKind::Resource => ("resource", "object IRI of this property"),
                    expected::AttrKind::Datatype => ("datatype", "datatype IRI of this literal"),
                    expected::AttrKind::ParseType => {
                        ("parseType", "Resource, Collection, Literal or Triple")
                    }
                    expected::AttrKind::XmlLang => unreachable!(),
                };
                (format!("{pfx}:{local}"), desc, imp.clone())
            }
        };
        let matches = if typed.contains(':') {
            fuzzy(typed, &label)
        } else {
            fuzzy(typed, &label) || fuzzy(typed, local_of(&label))
        };
        if !matches {
            continue;
        }
        let additional = match import {
            Some((p, ns)) => {
                let Some((at, new_text)) = e.declare(text, &p, &ns) else {
                    continue;
                };
                let pos = a.position(enc, at);
                Some(vec![TextEdit::new(Range::new(pos, pos), new_text)])
            }
            None => None,
        };
        let (new_text, format) = if snippets {
            (format!("{label}=\"$0\""), Some(InsertTextFormat::SNIPPET))
        } else {
            (format!("{label}=\"\""), None)
        };
        items.push(CompletionItem {
            label,
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(detail.into()),
            sort_text: Some(format!("{idx:02}")),
            insert_text_format: format,
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, new_text))),
            additional_text_edits: additional,
            ..Default::default()
        });
    }
    CompletionList {
        is_incomplete: false,
        items,
    }
}

/// The four `rdf:parseType` values of RDF 1.2 XML Syntax.
fn parse_types(typed: &str, in_entity: bool, allow_triple: bool, range: Range) -> CompletionList {
    const VALUES: [(&str, &str); 4] = [
        ("Resource", "blank node with child property elements"),
        ("Collection", "rdf:List of child node elements"),
        ("Literal", "XML literal content (rdf:XMLLiteral)"),
        ("Triple", "RDF 1.2 triple term (one triple)"),
    ];
    let mut items = Vec::new();
    if !in_entity {
        for (idx, (val, desc)) in VALUES.iter().enumerate() {
            if *val == "Triple" && !allow_triple {
                continue;
            }
            if fuzzy(typed, val) {
                items.push(CompletionItem {
                    label: (*val).into(),
                    kind: Some(CompletionItemKind::ENUM_MEMBER),
                    detail: Some((*desc).into()),
                    sort_text: Some(format!("{idx:02}")),
                    text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                        range,
                        (*val).into(),
                    ))),
                    ..Default::default()
                });
            }
        }
    }
    CompletionList {
        is_incomplete: false,
        items,
    }
}

/// The names of a node element (`node` is `None`: `rdf:Description` and
/// the classes) or of a property element of `node` (the properties), and
/// whether the list is cut.
fn elements(
    k: &Knowledge,
    typed: &str,
    e: &Expected<'_>,
    node: Option<&Node>,
    prefixes: &PrefixMap,
) -> (Vec<Candidate>, bool) {
    let default = e.prefixes.get("").map(|ns| &**ns);
    let (slot, ranked) = match node {
        None => (Slot::Class, Ranking::default()),
        Some(node) => {
            let subject = subject(k, node);
            (
                Slot::Predicate,
                ranking::predicates_for(k, &[subject], prefixes),
            )
        }
    };
    let in_ns = |ns: &str, iri: &str, vocab: Option<TermKind>| {
        iri.strip_prefix(ns).is_some_and(is_nc_name) && fits(k, slot, iri, vocab)
    };
    let promoted = |i: Iri| ranked.iris.contains_key(&i);
    let best = Best {
        typed,
        promoted: &promoted,
    };
    let mut cands = Vec::new();
    let mut extras = Vec::new();
    let mut incomplete = false;
    if let Some((pfx, local)) = typed.split_once(':') {
        let (ns, import) = match prefixes.get(pfx) {
            Some(ns) => (ns.clone(), None),
            None => match VOCABS.by_prefix(pfx).filter(|_| e.may_declare(pfx)) {
                Some(v) => (
                    v.namespace.to_string(),
                    Some((pfx.to_string(), v.namespace.to_string())),
                ),
                None => return (vec![], false),
            },
        };
        let label = |l: &str| format!("{pfx}:{l}");
        let fits = |iri: &str, vocab: Option<TermKind>| in_ns(&ns, iri, vocab);
        if node.is_none() && e.allow_description && ns == RDF && fuzzy(local, "Description") {
            extras.push(Candidate {
                iri: Some(format!("{RDF}Description")),
                import: import.clone(),
                ..Candidate::plain(
                    label("Description"),
                    CompletionItemKind::CLASS,
                    "a node, typed by its rdf:type properties".into(),
                    0,
                )
            });
        }
        cands = namespace_terms_best(k, &ns, local, slot, &label, import, Some(best), &fits);
    } else {
        incomplete = true;
        if node.is_none()
            && e.allow_description
            && fuzzy(typed, "Description")
            && let Some((label, import)) = qname(&format!("{RDF}Description"), prefixes, default)
        {
            extras.push(Candidate {
                iri: Some(format!("{RDF}Description")),
                import,
                ..Candidate::plain(
                    label,
                    CompletionItemKind::CLASS,
                    "a node, typed by its rdf:type properties".into(),
                    0,
                )
            });
        }
        let mut seen = FxHashSet::default();
        let named = prefixes
            .iter()
            .map(|(p, ns)| (Some(p.as_str()), ns.as_str()));
        for (p, ns) in default.map(|ns| (None, ns)).into_iter().chain(named) {
            if !seen.insert(ns) {
                continue;
            }
            let label = |l: &str| match p {
                Some(p) => format!("{p}:{l}"),
                None => l.to_string(),
            };
            let fits = |iri: &str, vocab: Option<TermKind>| in_ns(ns, iri, vocab);
            cands.extend(namespace_terms_best(
                k,
                ns,
                typed,
                slot,
                &label,
                None,
                Some(best),
                &fits,
            ));
        }
        let fits = |iri: &str, vocab: Option<TermKind>| {
            VOCABS
                .lookup(iri)
                .is_some_and(|(_, l)| is_nc_name(l) && fits(k, slot, iri, vocab))
        };
        local_name_candidates_where(&mut cands, typed, slot, prefixes, &fits);
    }
    let kind = match node {
        None => CompletionItemKind::CLASS,
        Some(_) => CompletionItemKind::PROPERTY,
    };
    let wanted = |label: &str| match typed.split_once(':') {
        Some((p, l)) => label
            .strip_prefix(p)
            .and_then(|r| r.strip_prefix(':'))
            .is_some_and(|r| fuzzy(l, r)),
        None => fuzzy(typed, local_of(label)),
    };
    let write = |iri: &str| qname(iri, prefixes, default);
    incomplete |= apply_ranking(&mut cands, ranked, kind, k, &wanted, &write);
    // One item per IRI, the first way found to write it; only valid absolute
    // IRIs (skipping prefixes bound to a malformed namespace URI), only the
    // declarations the root can take, and no name an end tag that follows
    // would take.
    let rdf_description = format!("{RDF}Description");
    for list in [&mut cands, &mut extras] {
        list.retain(|c| {
            (e.allow_description || c.iri.as_deref() != Some(rdf_description.as_str()))
                && c.iri
                    .as_deref()
                    .is_none_or(|i| oxiri::Iri::parse(i).is_ok())
                && c.import.as_ref().is_none_or(|(p, _)| e.may_declare(p))
                && !e.taken.contains(&c.label.as_str())
        });
    }
    let mut seen: FxHashSet<String> = extras.iter().filter_map(|c| c.iri.clone()).collect();
    cands.retain(|c| c.iri.as_ref().is_none_or(|i| seen.insert(i.clone())));
    incomplete |= finish(&mut cands, Ranking::default(), slot, typed, k, prefixes);
    incomplete |= finish(&mut extras, Ranking::default(), slot, typed, k, prefixes);
    let room = LIMIT.saturating_sub(extras.len());
    if cands.len() > room {
        cands.truncate(room);
        incomplete = true;
    }
    cands.append(&mut extras);
    finish(&mut cands, Ranking::default(), slot, typed, k, prefixes);
    (cands, incomplete)
}

/// The IRIs offered in an attribute value, and whether the list is cut.
fn values(
    k: &Knowledge,
    typed: &str,
    of: &Value,
    quote: u8,
    entity: Option<(&str, &str)>,
    prefixes: &PrefixMap,
) -> (Vec<Candidate>, bool) {
    // What the value reads as: the entity's replacement text first.
    let iri_typed = match entity {
        Some((name, ns)) => format!("{ns}{}", &typed[name.len() + 2..]),
        None => typed.to_string(),
    };
    let (slot, ranked) = match of {
        Value::About => (Slot::Subject, Ranking::default()),
        Value::Type => (Slot::Class, Ranking::default()),
        Value::Datatype => (Slot::Datatype, Ranking::default()),
        Value::ParseType { .. } => unreachable!(),
        Value::Resource { property, node } => match property.as_deref() {
            Some(p) if CLASS_VALUED.contains(&p) => (Slot::Class, Ranking::default()),
            Some(p) if PROPERTY_VALUED.contains(&p) => (Slot::Predicate, Ranking::default()),
            Some(p) => {
                let ranked = Iri::get(p).map(|pred| {
                    let subject = node.about.as_deref().and_then(Iri::get);
                    let keep = |i: Iri| matches(&iri_typed, &i);
                    ranking::objects_for(
                        k,
                        pred,
                        &[self::subject(k, node)],
                        subject,
                        prefixes,
                        &keep,
                    )
                });
                (Slot::Object, ranked.unwrap_or_default())
            }
            None => (Slot::Object, Ranking::default()),
        },
    };
    let fits = |iri: &str, vocab: Option<TermKind>| fits(k, slot, iri, vocab);
    let promoted = |i: Iri| ranked.iris.contains_key(&i);
    let best = Best {
        typed,
        promoted: &promoted,
    };
    let mut incomplete = false;
    let mut cands = match entity {
        Some((name, ns)) => {
            let label = |l: &str| format!("&{name};{l}");
            let local = &typed[name.len() + 2..];
            namespace_terms_best(k, ns, local, slot, &label, None, Some(best), &fits)
        }
        None => {
            // The bundled vocabularies' terms once something is typed (but
            // as subjects), and the datatypes, which are few.
            let (vocabularies, skipped_vocabularies) = match slot {
                Slot::Subject => (false, false),
                Slot::Datatype => (true, false),
                Slot::Class | Slot::Predicate | Slot::Object => {
                    (!typed.is_empty(), typed.is_empty())
                }
            };
            // Bundled vocabularies are skipped while nothing is typed, and
            // before a `#`, `/` or `:` is typed matching checks only the
            // local name so typing a scheme or namespace can add matches.
            incomplete =
                skipped_vocabularies || (!typed.is_empty() && !typed.contains(['#', '/', ':']));
            iris(k, typed, slot, vocabularies, &fits, best, quote)
        }
    };
    // Not the node itself, as its own object.
    if let Value::Resource {
        node: Node {
            about: Some(about), ..
        },
        ..
    } = of
    {
        cands.retain(|c| c.iri.as_ref() != Some(about));
    }
    let wanted = |_: &str| true;
    let write = |iri: &str| {
        (matches(&iri_typed, iri) && is_absolute(iri))
            .then(|| written(iri, entity, quote))
            .flatten()
            .map(|l| (l, None))
    };
    incomplete |= apply_ranking(
        &mut cands,
        ranked,
        CompletionItemKind::REFERENCE,
        k,
        &wanted,
        &write,
    );
    incomplete |= finish(&mut cands, Ranking::default(), slot, typed, k, prefixes);
    (cands, incomplete)
}

/// `iri` written as a value: after the entity reference it starts with
/// (`None` outside the entity's namespace), else whole, escaped for its
/// `quote`s.
fn written(iri: &str, entity: Option<(&str, &str)>, quote: u8) -> Option<String> {
    match entity {
        Some((name, ns)) => {
            let l = iri.strip_prefix(ns)?;
            (!l.contains(['&', '<', '\'', '"'])).then(|| format!("&{name};{l}"))
        }
        None => Some(escaped(iri, quote)),
    }
}

/// The terms whose IRI matches `typed` ([`matches`]) and `fits` keeps,
/// written whole between `quote`s: those the workspace describes, and with
/// `vocabularies` the bundled vocabularies' too, after them (as Turtle
/// ranks terms found by local name). Only the best ones are built (as
/// [`Best`] says).
fn iris(
    k: &Knowledge,
    typed: &str,
    slot: Slot,
    vocabularies: bool,
    fits: super::Fits,
    best: Best,
    quote: u8,
) -> Vec<Candidate> {
    // (IRI, kind, the bundled vocabulary's term)
    let mut rows: Vec<(String, TermKind, Option<&crate::vocab::Term>)> = Vec::new();
    let mut seen = FxHashSet::default();
    if vocabularies {
        // The IRI typed up to its last `#`, `/` or `:`: the namespaces it may
        // be in, and the local names that may match the rest.
        let cut = typed.rfind(['#', '/', ':']).map_or(0, |i| i + 1);
        let (head, rest) = typed.split_at(cut);
        for v in &VOCABS.all {
            if !(v.namespace.starts_with(head) || head.starts_with(v.namespace)) {
                continue;
            }
            for t in v.terms() {
                if head.is_empty() && !fuzzy(rest, &t.local) {
                    continue;
                }
                let iri = format!("{}{}", v.namespace, t.local);
                if matches(typed, &iri) && fits(&iri, Some(t.kind)) && seen.insert(iri.clone()) {
                    rows.push((iri, t.kind, Some(t)));
                }
            }
        }
    }
    for iri in k.described_terms() {
        if matches(typed, &iri) && !seen.contains(&*iri) && is_absolute(&iri) && fits(&iri, None) {
            rows.push((iri.to_string(), guess_kind(k, &iri), None));
        }
    }
    if rows.len() > LIMIT + 1 {
        let lower = typed.rsplit(':').next().unwrap_or("").to_lowercase();
        let (mut kept, rest): (Vec<_>, Vec<_>) = rows
            .into_iter()
            .partition(|r| Iri::get(&r.0).is_some_and(|i| (best.promoted)(i)));
        let mut keyed: Vec<_> = rest
            .into_iter()
            .map(|r| {
                let typed_first = !starts_with_lowercase(local_of(&r.0), &lower);
                ((ranked(slot, r.1, r.2), typed_first, r.0.len()), r)
            })
            .collect();
        if keyed.len() > LIMIT + 1 {
            keyed.select_nth_unstable_by(LIMIT + 1, |a, b| {
                a.0.cmp(&b.0).then_with(|| a.1.0.cmp(&b.1.0))
            });
            keyed.truncate(LIMIT + 1);
        }
        kept.extend(keyed.into_iter().map(|(_, r)| r));
        rows = kept;
    }
    rows.into_iter()
        .map(|(iri, kind, term)| {
            let detail = term
                .map(|t| t.label.as_str())
                .filter(|l| !l.is_empty())
                .or_else(|| k.label(iri.as_str()))
                .unwrap_or(&iri)
                .to_string();
            Candidate {
                tier: ranking::NONE,
                label: escaped(&iri, quote),
                kind: match term {
                    Some(t) => term_item_kind(t.kind),
                    None => match kind {
                        TermKind::Class => CompletionItemKind::CLASS,
                        TermKind::Property => CompletionItemKind::PROPERTY,
                        TermKind::Individual => CompletionItemKind::REFERENCE,
                    },
                },
                detail,
                rank: ranked(slot, kind, term),
                iri: Some(iri),
                import: None,
            }
        })
        .collect()
}

/// Whether `iri` is an absolute IRI: a workspace's term may be any text a
/// document wrote as one, and a value the grammar does not read as an IRI
/// is an error.
fn is_absolute(iri: &str) -> bool {
    oxiri::Iri::parse(iri).is_ok()
}

/// The rank of a term of `kind` in `slot`: a bundled vocabulary's `term`
/// after the workspace's, a deprecated one last.
fn ranked(slot: Slot, kind: TermKind, term: Option<&crate::vocab::Term>) -> u8 {
    rank(slot, kind) + term.map_or(0, |t| 2 + u8::from(t.deprecated) * 5)
}

/// Whether `iri` matches what was typed of a value: a part of the IRI up
/// to a `#`, `/` or `:` and a part of what follows it
/// (`http://example.org/onto#Per`), or a part of the local name (`Per`).
fn matches(typed: &str, iri: &str) -> bool {
    match typed.rfind(['#', '/', ':']) {
        Some(i) => iri.get(..=i) == Some(&typed[..=i]) && fuzzy(&typed[i + 1..], &iri[i + 1..]),
        None => fuzzy(typed, split_namespace(iri).1),
    }
}

/// What the ranking needs of `node`: the classes its element names and the
/// workspace gives its IRI.
fn subject(k: &Knowledge, node: &Node) -> Subject {
    let mut types: Vec<Iri> = node.types.iter().filter_map(|t| Iri::get(t)).collect();
    if let Some(about) = node.about.as_deref() {
        for t in k.types(about) {
            if !types.contains(&t) {
                types.push(t);
            }
        }
    }
    Subject {
        types,
        path: vec![],
    }
}

/// `iri` as an element name: with a prefix in scope, the default
/// namespace's bare, else with its well-known vocabulary's prefix (declared
/// on accept); `None` when its local name is no XML name.
fn qname(iri: &str, prefixes: &PrefixMap, default: Option<&str>) -> Written {
    let bare = default
        .and_then(|ns| iri.strip_prefix(ns))
        .filter(|l| is_nc_name(l))
        .map(str::to_string);
    let named = prefixes
        .iter()
        .filter_map(|(p, ns)| {
            let l = iri.strip_prefix(ns.as_str()).filter(|l| is_nc_name(l))?;
            Some(format!("{p}:{l}"))
        })
        .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    if let Some(name) = bare.or(named) {
        return Some((name, None));
    }
    let (v, l) = VOCABS.lookup(iri)?;
    (!prefixes.contains_key(v.prefix) && is_nc_name(l)).then(|| {
        (
            format!("{}:{l}", v.prefix),
            Some((v.prefix.to_string(), v.namespace.to_string())),
        )
    })
}

/// Tags the candidates the ranking knows, and adds the ranked IRIs not
/// offered yet that `write` writes and whose label is `wanted`, as items of
/// `kind`. Returns whether the ranking left some out.
fn apply_ranking(
    cands: &mut Vec<Candidate>,
    ranked: Ranking,
    kind: CompletionItemKind,
    k: &Knowledge,
    wanted: &dyn Fn(&str) -> bool,
    write: &dyn Fn(&str) -> Written,
) -> bool {
    let mut present = FxHashSet::default();
    for c in cands.iter_mut() {
        let Some(iri) = c.iri.as_deref().and_then(Iri::get) else {
            continue;
        };
        present.insert(iri);
        if let Some(h) = ranked.iris.get(&iri) {
            c.tier = h.tier;
            if let Some(d) = &h.detail {
                c.detail = d.clone();
            }
        }
    }
    for (iri, h) in &ranked.iris {
        if present.contains(iri) {
            continue;
        }
        let Some((label, import)) = write(iri).filter(|(l, _)| wanted(l)) else {
            continue;
        };
        let vocab = VOCABS
            .lookup(iri)
            .and_then(|(v, l)| v.term(l))
            .map(|t| t.label.as_str())
            .filter(|l| !l.is_empty());
        let detail = h
            .detail
            .clone()
            .unwrap_or_else(|| k.label(iri).or(vocab).unwrap_or(iri).to_string());
        cands.push(Candidate {
            tier: h.tier,
            label,
            kind: if h.enumerated {
                CompletionItemKind::ENUM_MEMBER
            } else {
                kind
            },
            detail,
            iri: Some(iri.to_string()),
            rank: 0,
            import,
        });
    }
    ranked.truncated
}

/// Whether the term `iri` (of kind `vocab` in the bundled vocabulary) may
/// be offered in `slot`: a class where a class goes, a property where a
/// property goes, a datatype where a datatype goes, any term elsewhere.
fn fits(k: &Knowledge, slot: Slot, iri: &str, vocab: Option<TermKind>) -> bool {
    let known = || Iri::get(iri);
    match slot {
        Slot::Class => {
            !is_datatype(k, iri, vocab)
                && match vocab {
                    Some(kind) => kind == TermKind::Class,
                    None => known().is_some_and(|i| {
                        guess_kind(k, &i) == TermKind::Class || !k.instances(&i).is_empty()
                    }),
                }
        }
        Slot::Predicate => match vocab {
            Some(kind) => kind == TermKind::Property,
            None => known().is_some_and(|i| guess_kind(k, &i) == TermKind::Property),
        },
        Slot::Datatype => is_datatype(k, iri, vocab),
        Slot::Subject | Slot::Object => true,
    }
}

/// Whether `iri` names a datatype: one of the reserved vocabulary's (OWL 2,
/// section 2.4), an RDF literal datatype (`rdf:HTML`, `rdf:JSON`,
/// `rdf:dirLangString`), or one the workspace types `rdfs:Datatype`.
fn is_datatype(k: &Knowledge, iri: &str, vocab: Option<TermKind>) -> bool {
    if matches!(
        iri.strip_prefix(RDF),
        Some("HTML" | "JSON" | "dirLangString")
    ) {
        return true;
    }
    match Entity::reserved(iri, vocab == Some(TermKind::Class)) {
        Some(kinds) => kinds.contains(&Entity::Datatype),
        None => k
            .types(iri)
            .iter()
            .any(|t| t.strip_prefix(RDFS) == Some("Datatype")),
    }
}

#[cfg(test)]
mod tests {
    use super::RDFS;
    use crate::analysis::{Analysis, Encoding};
    use crate::knowledge::{Knowledge, Source};
    use crate::syntax::Syntax;
    use lsp_types::*;
    use std::sync::Arc;

    const URI: &str = "file:///w/onto.owl";
    const SKOS: &str = "http://www.w3.org/2004/02/skos/core#";
    const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
    const OWL: &str = "http://www.w3.org/2002/07/owl#";

    /// A small ontology of `http://example.org/` terms: two classes, an
    /// object property with a domain, a data property and an individual;
    /// `body` goes before `</rdf:RDF>`.
    fn onto(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n\
             <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
             \x20        xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n\
             \x20        xmlns:owl=\"http://www.w3.org/2002/07/owl#\"\n\
             \x20        xmlns:ex=\"http://example.org/\">\n\
             \x20 <owl:Class rdf:about=\"http://example.org/Pump\">\n\
             \x20   <rdfs:label>Pump</rdfs:label>\n\
             \x20 </owl:Class>\n\
             \x20 <owl:Class rdf:about=\"http://example.org/Valve\"/>\n\
             \x20 <owl:ObjectProperty rdf:about=\"http://example.org/feeds\">\n\
             \x20   <rdfs:domain rdf:resource=\"http://example.org/Pump\"/>\n\
             \x20   <rdfs:range rdf:resource=\"http://example.org/Valve\"/>\n\
             \x20 </owl:ObjectProperty>\n\
             \x20 <owl:DatatypeProperty rdf:about=\"http://example.org/power\"/>\n\
             \x20 <ex:Valve rdf:about=\"http://example.org/v1\"/>\n\
             {body}</rdf:RDF>\n"
        )
    }

    /// The document `marked` with its `|` the cursor, in a workspace of it
    /// and `others` (URI, text).
    struct At {
        a: Arc<Analysis>,
        k: Knowledge,
        text: String,
        offset: usize,
    }

    fn at(marked: &str) -> At {
        at_with(marked, &[])
    }

    fn at_with(marked: &str, others: &[(&str, &str)]) -> At {
        let offset = marked.find('|').expect("a cursor");
        let text = marked.replacen('|', "", 1);
        let a = Arc::new(Analysis::new(URI.into(), &text, Arc::default(), true));
        assert_eq!(a.parsed.syntax, Syntax::RdfXml);
        let mut k = Knowledge::default();
        k.set(URI, Source::Open, a.clone());
        for (uri, t) in others {
            let o = Arc::new(Analysis::new(uri.to_string(), t, Arc::default(), true));
            k.set(uri, Source::Disk, o);
        }
        At { a, k, text, offset }
    }

    impl At {
        fn list(&self) -> CompletionList {
            let pos = self.a.position(Encoding::Utf16, self.offset as u32);
            match super::super::complete_in(&self.a, &self.k, Encoding::Utf16, pos, true) {
                CompletionResponse::List(l) => l,
                CompletionResponse::Array(items) => CompletionList {
                    is_incomplete: false,
                    items,
                },
            }
        }

        fn items(&self) -> Vec<CompletionItem> {
            self.list().items
        }

        fn labels(&self) -> Vec<String> {
            self.items().into_iter().map(|i| i.label).collect()
        }

        fn item(&self, label: &str) -> CompletionItem {
            let items = self.items();
            let labels: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
            items
                .into_iter()
                .find(|i| i.label == label)
                .unwrap_or_else(|| panic!("{label} not in {labels:?}"))
        }

        /// The text with `item` accepted: its edit and its added ones.
        fn accepted(&self, item: &CompletionItem) -> String {
            let mut edits: Vec<TextEdit> = item.additional_text_edits.clone().unwrap_or_default();
            match &item.text_edit {
                Some(CompletionTextEdit::Edit(e)) => edits.push(e.clone()),
                other => panic!("{other:?}"),
            }
            let offset = |p: Position| self.a.offset(Encoding::Utf16, p).unwrap() as usize;
            edits.sort_by_key(|e| std::cmp::Reverse(offset(e.range.start)));
            let mut text = self.text.clone();
            for e in edits {
                text.replace_range(offset(e.range.start)..offset(e.range.end), &e.new_text);
            }
            text
        }

        /// The text an item replaces.
        fn replaced(&self, label: &str) -> String {
            let Some(CompletionTextEdit::Edit(e)) = self.item(label).text_edit else {
                panic!("no edit")
            };
            let from = self.a.offset(Encoding::Utf16, e.range.start).unwrap() as usize;
            let to = self.a.offset(Encoding::Utf16, e.range.end).unwrap() as usize;
            self.text[from..to].to_string()
        }
    }

    fn has(labels: &[String], want: &str) -> bool {
        labels.iter().any(|l| l == want)
    }

    /// Story 17: after `<` in `rdf:RDF`, `rdf:Description` and the classes.
    #[test]
    fn a_node_element_is_a_class_or_rdf_description() {
        let at = at(&onto("  <|\n"));
        let labels = at.labels();
        for want in [
            "rdf:Description",
            "ex:Pump",
            "ex:Valve",
            "owl:Class",
            "rdfs:Class",
        ] {
            assert!(has(&labels, want), "{want} not in {labels:?}");
        }
        for not in [
            "ex:feeds",
            "ex:power",
            "ex:v1",
            "rdfs:label",
            "xsd:string",
            "rdf:type",
        ] {
            assert!(!has(&labels, not), "{not} in {labels:?}");
        }
        assert_eq!(at.item("ex:Pump").detail.as_deref(), Some("Pump"));
        // Typed: narrowed, and what is typed replaced.
        let at = super::tests::at(&onto("  <owl:Cl|\n"));
        assert_eq!(at.labels()[0], "owl:Class");
        assert_eq!(at.replaced("owl:Class"), "owl:Cl");
        let labels = super::tests::at(&onto("  <Pu|\n")).labels();
        assert!(has(&labels, "ex:Pump"), "{labels:?}");
        // A name typed in the middle, of an empty element: the whole name.
        let at = super::tests::at(&onto(
            "  <owl:Cl|ass rdf:about=\"http://example.org/X\"/>\n",
        ));
        assert_eq!(at.replaced("owl:Class"), "owl:Class");
    }

    /// Story 18: in a node element, the properties, the ones of its type's
    /// domain first.
    #[test]
    fn a_property_element_is_a_property_ranked_by_the_nodes_type() {
        for typed in ["", "ex:"] {
            let at = at(&onto(&format!(
                "  <ex:Pump rdf:about=\"http://example.org/p1\">\n    <{typed}|\n"
            )));
            let labels = at.labels();
            assert_eq!(labels[0], "ex:feeds", "{typed}: {labels:?}");
            assert!(has(&labels, "ex:power"), "{labels:?}");
            for not in [
                "ex:Pump",
                "ex:Valve",
                "ex:v1",
                "rdf:Description",
                "owl:Class",
            ] {
                assert!(!has(&labels, not), "{typed}: {not} in {labels:?}");
            }
            if typed.is_empty() {
                for want in ["rdfs:label", "rdfs:subClassOf", "rdf:type"] {
                    assert!(has(&labels, want), "{want} not in {labels:?}");
                }
            }
        }
        // The type from the workspace: rdf:Description of a typed subject.
        let at = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/p1\">\n    <ex:|\n  </rdf:Description>\n  <ex:Pump rdf:about=\"http://example.org/p1\"/>\n",
        ));
        assert_eq!(at.labels()[0], "ex:feeds", "{:?}", at.labels());
    }

    /// Story 19: the striping at any depth.
    #[test]
    fn the_offer_follows_the_striping_at_any_depth() {
        let nested = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:subClassOf>\n      <owl:Restriction>\n        <|\n",
        );
        let labels = at(&nested).labels();
        assert!(has(&labels, "owl:onProperty"), "{labels:?}");
        assert!(!has(&labels, "owl:Class"), "{labels:?}");
        let collection = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <owl:unionOf rdf:parseType=\"Collection\">\n      <owl:Class rdf:about=\"http://example.org/B\"/>\n      <|\n",
        );
        let labels = at(&collection).labels();
        assert!(has(&labels, "owl:Class"), "{labels:?}");
        assert!(!has(&labels, "rdfs:label"), "{labels:?}");
        let resource = onto(
            "  <ex:Pump rdf:about=\"http://example.org/p1\">\n    <ex:part rdf:parseType=\"Resource\">\n      <|\n",
        );
        let labels = at(&resource).labels();
        assert!(has(&labels, "rdfs:label"), "{labels:?}");
        assert!(!has(&labels, "rdf:Description"), "{labels:?}");
    }

    /// A class nested in a class's property takes the class's name; the
    /// name the next end tag closes is left out, as that end tag would
    /// close the new element instead.
    #[test]
    fn the_name_the_next_end_tag_closes_is_left_out() {
        let nested = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:subClassOf>\n      <owl:Cl|\n    </rdfs:subClassOf>\n  </owl:Class>\n",
        );
        assert!(has(&at(&nested).labels(), "owl:Class"));
        // A start tag where an end tag was: `</owl:Class>` comes next.
        let mixed = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:subClassOf>\n      <Cl|\n  </owl:Class>\n",
        );
        let labels = at(&mixed).labels();
        assert!(!has(&labels, "owl:Class"), "{labels:?}");
        assert!(has(&labels, "rdfs:Class"), "{labels:?}");
    }

    /// Story 20, 29: nothing in an XML literal, text, comments, the
    /// prolog or attribute names.
    #[test]
    fn nothing_where_no_term_goes() {
        for body in [
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:comment rdf:parseType=\"Literal\"><|",
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:comment rdf:parseType=\"Literal\"><b rdf:resource=\"|",
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:label>Pu|</rdfs:label>\n  </owl:Class>\n",
            "  <!-- <owl:Cl| -->\n",
            "  <owl:Class rdf:ab|out=\"http://example.org/A\"/>\n",
            "  <owl:Class rdf:nodeID=\"|\"/>\n",
            "  <owl:Cl|ass rdf:about=\"http://example.org/X\">\n  </owl:Class>\n",
        ] {
            let at = at(&onto(body));
            assert_eq!(at.labels(), Vec::<String>::new(), "{body}");
        }
        let at = at(&onto("").replace("<?xml version", "<?xml ver|sion"));
        assert_eq!(at.labels(), Vec::<String>::new());
    }

    /// Story 21: the default namespace writes bare names; a prefix of an
    /// open element counts.
    #[test]
    fn names_are_written_with_the_prefixes_in_scope() {
        let text = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\" xmlns:dc=\"http://purl.org/dc/terms/\" xmlns=\"http://www.w3.org/2000/01/rdf-schema#\">\n    <|\n",
        );
        let labels = at(&text).labels();
        for want in [
            "label",
            "subClassOf",
            "dc:title",
            "ex:feeds",
            "owl:onProperty",
        ] {
            assert!(has(&labels, want), "{want} not in {labels:?}");
        }
        for not in ["rdfs:label", ":label", "dcterms:title"] {
            assert!(!has(&labels, not), "{not} in {labels:?}");
        }
    }

    fn added(item: &CompletionItem) -> Vec<String> {
        item.additional_text_edits
            .iter()
            .flatten()
            .map(|e| e.new_text.clone())
            .collect()
    }

    /// Story 22: a term of a well-known namespace with no prefix in scope
    /// adds its declaration to the root on accept, a line of its own here.
    #[test]
    fn a_well_known_term_adds_its_namespace_to_the_root() {
        let at = at(&onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <skos:prefL|\n",
        ));
        let item = at.item("skos:prefLabel");
        assert_eq!(added(&item), [format!("\n         xmlns:skos=\"{SKOS}\"")]);
        assert_eq!(
            item.label_details.and_then(|d| d.description).as_deref(),
            Some("+xmlns")
        );
        // By local name, from two characters.
        let at = super::tests::at(&onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <prefL|\n",
        ));
        assert_eq!(added(&at.item("skos:prefLabel")).len(), 1);
        // A prefix bound to another namespace is not declared again.
        let other = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\" xmlns:skos=\"http://example.org/s#\">\n    <skos:prefL|\n",
        );
        let labels = super::tests::at(&other).labels();
        assert!(!has(&labels, "skos:prefLabel"), "{labels:?}");
        let labels = super::tests::at(&other.replace("<skos:prefL", "<prefL")).labels();
        assert!(!has(&labels, "skos:prefLabel"), "{labels:?}");
    }

    /// Story 23: a local name that is no XML name is left out of names.
    #[test]
    fn a_local_name_that_is_no_xml_name_is_left_out() {
        let other = "@prefix ex: <http://example.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.org/123> a owl:Class .\nex:Tank a owl:Class .\n";
        let at = at_with(&onto("  <ex:|\n"), &[("file:///w/other.ttl", other)]);
        let labels = at.labels();
        assert!(has(&labels, "ex:Tank"), "{labels:?}");
        assert!(!labels.iter().any(|l| l.contains("123")), "{labels:?}");
        // In a value, it is an IRI like the others.
        let at = at_with(
            &onto("  <ex:Pump rdf:about=\"http://example.org/12|\"/>\n"),
            &[("file:///w/other.ttl", other)],
        );
        assert!(
            has(&at.labels(), "http://example.org/123"),
            "{:?}",
            at.labels()
        );
    }

    /// Story 24: in `rdf:about`, the subjects described, as full IRIs; the
    /// item replaces the whole value.
    #[test]
    fn rdf_about_offers_the_described_subjects() {
        let at = at(&onto(
            "  <owl:Class rdf:about=\"http://example.org/Pu|mp2\"/>\n",
        ));
        let labels = at.labels();
        assert!(has(&labels, "http://example.org/Pump"), "{labels:?}");
        assert!(
            !has(&labels, "http://www.w3.org/2002/07/owl#Class"),
            "{labels:?}"
        );
        assert_eq!(
            at.replaced("http://example.org/Pump"),
            "http://example.org/Pump2"
        );
        let at = super::tests::at(&onto("  <owl:Class rdf:about=\"|"));
        let labels = at.labels();
        for want in [
            "http://example.org/Pump",
            "http://example.org/feeds",
            "http://example.org/v1",
        ] {
            assert!(has(&labels, want), "{want} not in {labels:?}");
        }
    }

    /// Story 25: `rdf:resource` by the property: classes for `rdf:type` and
    /// `rdfs:subClassOf`, properties for `rdfs:subPropertyOf`, instances of
    /// the range first for the others.
    #[test]
    fn rdf_resource_offers_iris_of_the_propertys_kind() {
        let open = "  <owl:Class rdf:about=\"http://example.org/A\">\n    ";
        // Nothing typed: the workspace's classes.
        let labels = at(&onto(&format!("{open}<rdfs:subClassOf rdf:resource=\"|"))).labels();
        for want in ["http://example.org/Pump", "http://example.org/Valve"] {
            assert!(has(&labels, want), "{want} not in {labels:?}");
        }
        for not in [
            "http://example.org/feeds",
            "http://example.org/v1",
            &format!("{OWL}Thing"),
        ] {
            assert!(!has(&labels, not), "{not} in {labels:?}");
        }
        // Typed: the vocabularies' classes too, after the workspace's.
        let labels = at(&onto(&format!("{open}<rdfs:subClassOf rdf:resource=\"Th|"))).labels();
        assert!(has(&labels, &format!("{OWL}Thing")), "{labels:?}");
        assert!(!has(&labels, &format!("{XSD}string")), "{labels:?}");
        let labels = at(&onto(&format!(
            "{open}<rdf:type rdf:resource=\"http://example.org/|\"/>"
        )))
        .labels();
        assert!(has(&labels, "http://example.org/Valve"), "{labels:?}");
        assert!(!has(&labels, "http://example.org/v1"), "{labels:?}");
        let labels = at(&onto(
            "  <owl:ObjectProperty rdf:about=\"http://example.org/q\">\n    <rdfs:subPropertyOf rdf:resource=\"http://example.org/|",
        ))
        .labels();
        assert_eq!(
            labels,
            ["http://example.org/feeds", "http://example.org/power"]
        );
        // ex:feeds has the range ex:Valve, of which ex:v1 is an instance.
        let labels = at(&onto(
            "  <ex:Pump rdf:about=\"http://example.org/p1\">\n    <ex:feeds rdf:resource=\"http://example.org/|",
        ))
        .labels();
        assert_eq!(labels[0], "http://example.org/v1", "{labels:?}");
    }

    /// Story 26: `rdf:datatype` offers the datatypes.
    #[test]
    fn rdf_datatype_offers_the_datatypes() {
        let at = at(&onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <ex:power rdf:datatype=\"|\">1</ex:power>\n",
        ));
        let labels = at.labels();
        for want in [
            format!("{XSD}string"),
            format!("{XSD}integer"),
            format!("{RDFS}Literal"),
            "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString".into(),
        ] {
            assert!(has(&labels, &want), "{want} not in {labels:?}");
        }
        for not in [
            format!("{XSD}minInclusive"),
            format!("{OWL}Class"),
            "http://example.org/Pump".into(),
        ] {
            assert!(!has(&labels, &not), "{not} in {labels:?}");
        }
        let labels = super::tests::at(&onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <ex:power rdf:datatype=\"http://www.w3.org/2001/XMLSchema#int|\">1</ex:power>\n",
        ))
        .labels();
        assert_eq!(labels[0], format!("{XSD}int"), "{labels:?}");
        assert!(has(&labels, &format!("{XSD}integer")), "{labels:?}");
    }

    /// Story 27: a value that starts with an entity reference: that
    /// namespace's terms, written with it.
    #[test]
    fn a_value_with_an_entity_keeps_it() {
        let marked = onto(
            "  <owl:DatatypeProperty rdf:about=\"http://example.org/p\">\n    <rdfs:range rdf:resource=\"&xsd;str|\"/>\n",
        )
        .replace(
            "<?xml version=\"1.0\"?>\n",
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n  <!ENTITY xsd \"http://www.w3.org/2001/XMLSchema#\">\n  <!ENTITY ex \"http://example.org/\">\n]>\n",
        );
        let at = at(&marked);
        // The document's own `&xsd;str` is a term it uses, as in Turtle.
        assert_eq!(
            at.labels()[..2],
            ["&xsd;str", "&xsd;string"],
            "{:?}",
            at.labels()
        );
        assert_eq!(at.replaced("&xsd;string"), "&xsd;str");
        let at = super::tests::at(&marked.replace("&xsd;str|", "&ex;Pu|"));
        assert!(has(&at.labels(), "&ex;Pump"), "{:?}", at.labels());
        assert!(
            at.labels().iter().all(|l| l.starts_with("&ex;")),
            "{:?}",
            at.labels()
        );
    }

    /// Story 28: a part of an IRI or of its local name narrows the list.
    #[test]
    fn a_part_of_an_iri_or_a_local_name_narrows_the_list() {
        let open = "  <ex:Pump rdf:about=\"http://example.org/p1\">\n    <rdf:type rdf:resource=\"";
        for typed in ["http://example.org/Pu", "Pu", "http://exa"] {
            let labels = at(&onto(&format!("{open}{typed}|\"/>\n"))).labels();
            assert!(
                has(&labels, "http://example.org/Pump"),
                "{typed}: {labels:?}"
            );
            if typed != "http://exa" {
                assert!(
                    !has(&labels, "http://example.org/Valve"),
                    "{typed}: {labels:?}"
                );
            }
        }
        // An IRI with a `&` is escaped for the value.
        let other = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.org/a&b> a owl:Class .\n";
        let at = at_with(
            &onto(&format!("{open}http://example.org/a|\"/>\n")),
            &[("file:///w/o.ttl", other)],
        );
        assert!(
            has(&at.labels(), "http://example.org/a&amp;b"),
            "{:?}",
            at.labels()
        );
    }

    /// Story 29: the offer keeps working with errors around; a value not
    /// closed gets nothing past its line end.
    #[test]
    fn a_document_with_errors_still_completes() {
        let broken = "  </owl:Thing> &nope; <owl:Class rdf:about=\"http://example.org/A\"/></owl:Clas>\n  <owl:Class rdf:about=\"http://example.org/B\">\n    <rdfs:subClassOf rdf:resource=\"http://example.org/Pu|\n    <rdfs:label>B</rdfs:label>\n";
        let labels = at(&onto(broken)).labels();
        assert!(has(&labels, "http://example.org/Pump"), "{labels:?}");
        let past = "  <owl:Class rdf:about=\"http://example.org/B\">\n    <rdfs:subClassOf rdf:resource=\"http://example.org/Pu\n   |  <rdfs:label>x</rdfs:label>\n";
        assert_eq!(at(&onto(past)).labels(), Vec::<String>::new());
    }

    /// Story 36: every item accepted, at three places, gives a document
    /// whose parse has no more errors than before.
    #[test]
    fn an_accepted_item_adds_no_error() {
        let mut declared = false;
        for body in [
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:subClassOf rdf:resource=\"|\"/>\n  </owl:Class>\n",
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <prefL|/>\n  </owl:Class>\n",
            "  <o|/>\n",
        ] {
            let at = at(&onto(body));
            let before = at.a.parsed.errors.len();
            let items = at.items();
            assert!(!items.is_empty(), "{body}");
            declared |= items.iter().any(|i| i.additional_text_edits.is_some());
            for item in &items {
                let after = at.accepted(item);
                let a = Analysis::new(URI.into(), &after, Arc::default(), true);
                assert!(
                    a.parsed.errors.len() <= before,
                    "{} in {body}: {:?}\n{after}",
                    item.label,
                    a.parsed.errors
                );
            }
        }
        assert!(declared, "no item adds a declaration");
    }

    /// `rdf:Description` keeps its place under the cap of a list of many
    /// classes.
    #[test]
    fn many_classes_keep_rdf_description_in_the_list() {
        let mut body = String::new();
        for i in 0..400 {
            body.push_str(&format!(
                "  <owl:Class rdf:about=\"http://example.org/C{i:03}\"/>\n"
            ));
        }
        body.push_str("  <|\n");
        let list = at(&onto(&body)).list();
        let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels.len(), super::LIMIT);
        assert!(list.is_incomplete);
        for want in ["rdf:Description", "ex:C000"] {
            assert!(labels.contains(&want), "{want} not in {labels:?}");
        }
    }
    /// Story 36, at every offset of four texts (one with an entity and CRLF
    /// line ends, one broken, one with names and values being typed): every
    /// item accepted gives a document whose parse has no more errors than
    /// before.
    #[test]
    fn every_item_at_every_offset_adds_no_error() {
        let base = onto(
            "  <owl:Class rdf:about=\"http://example.org/A\">\n    <rdfs:subClassOf rdf:resource=\"http://example.org/Pump\"/>\n    <ex:feeds rdf:resource=\"http://example.org/v1\"/>\n    <rdfs:label xml:lang=\"en\">A</rdfs:label>\n    <rdfs:comment rdf:parseType=\"Literal\"><b>x</b></rdfs:comment>\n  </owl:Class>\n  <ex:Pump rdf:about=\"http://example.org/p9\"><ex:power rdf:datatype=\"http://www.w3.org/2001/XMLSchema#int\">1</ex:power></ex:Pump>\n",
        );
        let entity = base
            .replace(
                "<?xml version=\"1.0\"?>\n",
                "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n  <!ENTITY ex \"http://example.org/\">\n]>\n",
            )
            .replace("\"http://example.org/Pump\"/>", "\"&ex;Pump\"/>")
            .replace('\n', "\r\n");
        let broken = base
            .replace("</owl:Class>\n  <ex:Pump", "\n  <ex:Pump")
            .replace("xml:lang=\"en\"", "xml:lang=\"en");
        // Names being typed: cut, closed by the `>` an editor writes, inside
        // a node and at the end; a value closed by a later attribute's quote.
        let typing = base
            .replace(
                "  </owl:Class>\n",
                "    <rdfs:la\n    <>\n  </owl:Class>\n  <owl:Cl>\n",
            )
            .replace("rdf:datatype=\"", "rdf:datatype=\" xml:lang=\"en\" x=\"")
            .replace("</rdf:RDF>\n", "  <owl:Cl");
        let mut accepted = 0;
        for text in [base, entity, broken, typing] {
            for offset in (0..=text.len()).filter(|&o| text.is_char_boundary(o)) {
                let at = at(&format!("{}|{}", &text[..offset], &text[offset..]));
                let before = at.a.parsed.errors.len();
                for item in at.items() {
                    let after = at.accepted(&item);
                    // Attribute-name snippets leave an unfilled value (`$0`).
                    if item.kind == Some(CompletionItemKind::FIELD) {
                        accepted += 1;
                        continue;
                    }
                    let a = Analysis::new(URI.into(), &after, Arc::default(), true);
                    assert!(
                        a.parsed.errors.len() <= before,
                        "{} at {offset}: {:?}\n{after}",
                        item.label,
                        a.parsed.errors
                    );
                    accepted += 1;
                }
            }
        }
        assert!(accepted > 10_000, "{accepted}");
    }

    /// A value the scanner ends at a `<` or at the end of the text, with no
    /// closing quote: its edit replaces it whole, as the grammar reads it.
    #[test]
    fn a_value_not_closed_is_replaced_whole() {
        let errors = |text: &str| {
            Analysis::new(URI.into(), text, Arc::default(), true)
                .parsed
                .errors
                .len()
        };
        let integer = format!("{XSD}integer");
        let head = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:p rdf:datatype=\"http://www.w3.org/2001/|XMLSchema#integer";
        for tail in ["<ex:q/>\n  </rdf:Description>\n</rdf:RDF>\n", ""] {
            let at = at(&format!("{head}{tail}"));
            assert_eq!(at.replaced(&integer), integer);
            let after = at.accepted(&at.item(&integer));
            assert!(errors(&after) <= errors(&at.text), "{after}");
        }
        // Not closed, over the line end: to the line end, which the value
        // holds before and after.
        let at = at(&onto(
            "  <ex:Pump rdf:about=\"http://example.org/p1\">\n    <rdf:type rdf:resource=\"http://example.org/P|um\n    </ex:Pump>\n",
        ));
        assert_eq!(
            at.replaced("http://example.org/Pump"),
            "http://example.org/Pum"
        );
    }

    /// A value with no closing quote, which the grammar reads to the next
    /// `<`: its edit stops at the first character an IRI cannot hold (a
    /// `>`, white space) and before the `/` of a `/>`, so the end of the
    /// tag and what comes after it stay. The line after accepting, whole.
    #[test]
    fn an_unclosed_value_s_edit_keeps_the_end_of_its_tag() {
        let int = format!("{XSD}integer");
        let pump = "http://example.org/Pump";
        let datatype = |line: &str| {
            onto(&format!(
                "  <rdf:Description rdf:about=\"http://example.org/s\">\n    {line}\n  </rdf:Description>\n"
            ))
        };
        let class = |line: &str| {
            onto(&format!(
                "  <owl:Class rdf:about=\"http://example.org/Hose\">\n    {line}\n  </owl:Class>\n"
            ))
        };
        let top = |line: &str| onto(&format!("  {line}\n"));
        // The whole text ends in the line.
        let last = |line: &str| {
            format!(
                "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n  <rdf:Description rdf:about=\"http://example.org/s\">\n    {line}"
            )
        };
        let cases = [
            // `>` and text after the value.
            (
                datatype(&format!("<ex:age rdf:datatype=\"{XSD}int|>42</ex:age>")),
                int.as_str(),
                format!("    <ex:age rdf:datatype=\"{int}>42</ex:age>"),
            ),
            (
                datatype(&format!("<ex:age rdf:datatype=\"{XSD}in|t>42</ex:age>")),
                &int,
                format!("    <ex:age rdf:datatype=\"{int}>42</ex:age>"),
            ),
            // `/>` after the value, then the line end.
            (
                class("<rdfs:subClassOf rdf:resource=\"http://example.org/P|/>"),
                pump,
                format!("    <rdfs:subClassOf rdf:resource=\"{pump}/>"),
            ),
            (
                class("<rdfs:subClassOf rdf:resource=\"http://example.org/P|um/>"),
                pump,
                format!("    <rdfs:subClassOf rdf:resource=\"{pump}/>"),
            ),
            (
                class("<rdfs:subClassOf rdf:resource=\"|/>"),
                pump,
                format!("    <rdfs:subClassOf rdf:resource=\"{pump}/>"),
            ),
            // `>` and a child element at once.
            (
                top(
                    "<rdf:Description rdf:type=\"http://example.org/P|><rdfs:label>x</rdfs:label></rdf:Description>",
                ),
                pump,
                format!(
                    "  <rdf:Description rdf:type=\"{pump}><rdfs:label>x</rdfs:label></rdf:Description>"
                ),
            ),
            // At the end of the text.
            (
                last(&format!("<ex:p rdf:datatype=\"{XSD}int|/>")),
                &int,
                format!("    <ex:p rdf:datatype=\"{int}/>"),
            ),
            (
                last(&format!("<ex:p rdf:datatype=\"{XSD}int|>")),
                &int,
                format!("    <ex:p rdf:datatype=\"{int}>"),
            ),
            (
                last(&format!("<ex:p rdf:datatype=\"{XSD}int|")),
                &int,
                format!("    <ex:p rdf:datatype=\"{int}"),
            ),
            // White space, then an attribute in the other quotes: to the
            // white space, as before.
            (
                class("<rdfs:subClassOf rdf:resource=\"http://example.org/P| rdf:ID='r1'/>"),
                pump,
                format!("    <rdfs:subClassOf rdf:resource=\"{pump} rdf:ID='r1'/>"),
            ),
            // Closed: between the quotes, as before.
            (
                class("<rdfs:subClassOf rdf:resource=\"http://example.org/P|\"/>"),
                pump,
                format!("    <rdfs:subClassOf rdf:resource=\"{pump}\"/>"),
            ),
            (
                datatype(&format!("<ex:age rdf:datatype=\"{XSD}int|\">42</ex:age>")),
                &int,
                format!("    <ex:age rdf:datatype=\"{int}\">42</ex:age>"),
            ),
        ];
        let mut wrong = Vec::new();
        for (marked, label, want) in cases {
            let at = at(&marked);
            let line = marked[..at.offset].matches('\n').count();
            let Some(item) = at.items().into_iter().find(|i| i.label == label) else {
                wrong.push(format!("{label} not offered in {marked}"));
                continue;
            };
            let after = at.accepted(&item);
            let got = after.lines().nth(line).unwrap_or_default().to_string();
            if got != want {
                wrong.push(format!("want {want}\n got {got}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        // A `>` before the offset: no edit covers it.
        let at = at(&datatype(&format!(
            "<ex:age rdf:datatype=\"{XSD}int>4|2</ex:age>"
        )));
        for item in at.items() {
            let after = at.accepted(&item);
            assert!(after.contains(">42</ex:age>"), "{}: {after}", item.label);
        }
    }

    #[test]
    fn qualified_rdf_prefix_offers_rdf_description_in_node_position() {
        for body in ["  <rdf:Des|/>\n", "  <rdf:Des|\n", "  <rdf:|/>\n"] {
            let labels = at(&onto(body)).labels();
            assert!(has(&labels, "rdf:Description"), "{body}: {labels:?}");
            assert_eq!(
                labels.iter().filter(|l| *l == "rdf:Description").count(),
                1,
                "{body}: {labels:?}"
            );
        }
        // Custom prefix bound to RDF's namespace.
        let custom = onto("  <r:Des|/>\n").replace(
            "xmlns:ex=",
            "xmlns:r=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=",
        );
        assert!(has(&at(&custom).labels(), "r:Description"));
        // Even if the workspace types rdf:Description as a class, no duplicate.
        let ttl = "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\nrdf:Description a owl:Class .\n";
        let labels = at_with(&onto("  <rdf:Des|/>\n"), &[("file:///w/d.ttl", ttl)]).labels();
        assert_eq!(
            labels.iter().filter(|l| *l == "rdf:Description").count(),
            1,
            "{labels:?}"
        );
        // Never in property-element position.
        let prop = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <rdf:Des|/>\n  </rdf:Description>\n",
        ));
        assert!(!has(&prop.labels(), "rdf:Description"));
    }

    #[test]
    fn empty_rdf_resource_is_incomplete_when_vocabularies_are_skipped() {
        let open = "  <ex:Pump rdf:about=\"http://example.org/p1\">\n    ";
        for prop in ["rdf:type", "ex:feeds", "rdfs:subPropertyOf"] {
            let list = at(&onto(&format!("{open}<{prop} rdf:resource=\"|\"/>\n"))).list();
            assert!(list.is_incomplete, "{prop} should be incomplete");
        }
        let labels = at(&onto(&format!("{open}<rdf:type rdf:resource=\"Th|\"/>\n"))).labels();
        assert!(has(&labels, &format!("{OWL}Thing")), "{labels:?}");
    }

    #[test]
    fn rdf_datatypes_are_offered_for_rdf_datatype_and_omitted_as_classes() {
        let rdf = super::RDF;
        let dts = [
            "HTML",
            "JSON",
            "dirLangString",
            "langString",
            "XMLLiteral",
            "PlainLiteral",
        ];
        // Excluded from node-element names and rdf:type resource values.
        for body in [
            "  <|\n",
            "  <rdf:|/>\n",
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <rdf:type rdf:resource=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#|\"/>\n  </rdf:Description>\n",
        ] {
            let labels = at(&onto(body)).labels();
            for dt in dts {
                assert!(
                    !has(&labels, &format!("rdf:{dt}")) && !has(&labels, &format!("{rdf}{dt}")),
                    "{dt} offered in {body}: {labels:?}"
                );
            }
        }
        // Offered in rdf:datatype values.
        let all = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:power rdf:datatype=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#|\">x</ex:power>\n  </rdf:Description>\n",
        ))
        .labels();
        for dt in dts {
            assert!(has(&all, &format!("{rdf}{dt}")), "{dt} not in {all:?}");
        }
        let ht = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:power rdf:datatype=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#HT|\">x</ex:power>\n  </rdf:Description>\n",
        ))
        .labels();
        assert!(has(&ht, &format!("{rdf}HTML")), "{ht:?}");
    }

    #[test]
    fn a_root_node_element_completes_rdf_about() {
        let ttl = "@prefix ex: <http://example.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\nex:Person a owl:Class .\n";
        let marked = "<?xml version=\"1.0\"?>\n<owl:Class xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:owl=\"http://www.w3.org/2002/07/owl#\" rdf:about=\"http://example.org/Pe|\"/>\n";
        let at = at_with(marked, &[("file:///w/terms.ttl", ttl)]);
        let labels = at.labels();
        assert!(has(&labels, "http://example.org/Person"), "{labels:?}");
        let after = at.accepted(&at.item("http://example.org/Person"));
        let parsed = Analysis::new(URI.into(), &after, Arc::default(), true);
        assert!(
            parsed.parsed.errors.is_empty(),
            "{:?}",
            parsed.parsed.errors
        );
        // Root rdf:RDF gets no rdf:about completion.
        let rdf_root = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" rdf:about=\"http://example.org/Pe|\"/>\n";
        assert!(
            at_with(rdf_root, &[("file:///w/terms.ttl", ttl)])
                .labels()
                .is_empty()
        );
    }

    #[test]
    fn a_property_element_with_text_or_property_attrs_offers_no_node_element() {
        let open = "  <rdf:Description rdf:about=\"http://example.org/s\">\n    ";
        for body in [
            format!("{open}<ex:feeds>text<|</ex:feeds>\n  </rdf:Description>\n"),
            format!("{open}<ex:feeds ex:label=\"abc\"><|</ex:feeds>\n  </rdf:Description>\n"),
            format!(
                "{open}<ex:feeds rdf:type=\"http://example.org/Valve\"><|</ex:feeds>\n  </rdf:Description>\n"
            ),
        ] {
            assert_eq!(at(&onto(&body)).labels(), Vec::<String>::new(), "{body}");
        }
        // Whitespace-only text and rdf:ID still allow a nested node element.
        for body in [
            format!("{open}<ex:feeds>  <|</ex:feeds>\n  </rdf:Description>\n"),
            format!("{open}<ex:feeds rdf:ID=\"r1\"><|</ex:feeds>\n  </rdf:Description>\n"),
        ] {
            let labels = at(&onto(&body)).labels();
            assert!(has(&labels, "rdf:Description"), "{body}: {labels:?}");
        }
    }

    #[test]
    fn malformed_namespace_iri_does_not_offer_invalid_element_names() {
        let doc = "<?xml version=\"1.0\"?>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/>22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n\
              <|ex:A rdf:about=\"http://example.org/a\" ex:name=\"a\"/>\n\
            </rdf:RDF>\n";
        let labels = at(doc).labels();
        assert!(!has(&labels, "rdf:about"), "{labels:?}");
        // A prefix name that is not an XML NCName (chaos 27859) is never offered.
        let bad_pfx = "<?xml version=\"1.0\"?>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
                     xmlns:owl=\"http://www.w3.org/2002/07/owl#\"\n\
                     xmlns:&e40;rdfs=\"http://www.w3.org/2000/01/rdf-schema#\">\n\
              <|owl:Class rdf:about=\"http://example.org/A\"/>\n\
            </rdf:RDF>\n";
        let labels = at(bad_pfx).labels();
        assert!(!labels.iter().any(|l| l.starts_with('&')), "{labels:?}");
        // Self-closing typed node in `parseType="Triple"` does not offer `rdf:Description`.
        let triple = onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:feeds rdf:parseType=\"Triple\">\n      <|ex:Pump rdf:about=\"http://example.org/p1\"/>\n    </ex:feeds>\n  </rdf:Description>\n",
        );
        let labels = at(&triple).labels();
        assert!(!has(&labels, "rdf:Description"), "{labels:?}");
        assert!(has(&labels, "ex:Valve"), "{labels:?}");
    }

    #[test]
    fn closing_tag_completes_the_innermost_open_element() {
        let open =
            "  <owl:Class rdf:about=\"http://example.org/Hose\">\n    <rdfs:subClassOf>\n      ";
        // Without `>`: appends `>`.
        let a = at(&onto(&format!("{open}</|\n  </owl:Class>\n")));
        assert_eq!(a.labels(), ["rdfs:subClassOf"]);
        let after = a.accepted(&a.item("rdfs:subClassOf"));
        assert!(after.contains("</rdfs:subClassOf>\n"), "{after}");
        // When already closed below, does not suggest closing it again early.
        let dup = at(&onto(&format!(
            "{open}</|\n    </rdfs:subClassOf>\n  </owl:Class>\n"
        )));
        assert!(dup.labels().is_empty(), "{:?}", dup.labels());
        // With `>` already present: replaces only the name.
        let a = at(&onto(&format!("{open}</rdfs:sub|>\n  </owl:Class>\n")));
        assert_eq!(a.labels(), ["rdfs:subClassOf"]);
        let after = a.accepted(&a.item("rdfs:subClassOf"));
        assert!(
            after.contains("</rdfs:subClassOf>\n  </owl:Class>"),
            "{after}"
        );
        let parsed = Analysis::new(URI.into(), &after, Arc::default(), true);
        assert!(
            parsed.parsed.errors.is_empty(),
            "{:?}",
            parsed.parsed.errors
        );
        // Does not overwrite an existing outer closing tag.
        let outer = at(&onto(
            "  <owl:Class rdf:about=\"http://example.org/Hose\">\n    <rdfs:subClassOf>\n  </|owl:Class>\n",
        ));
        assert!(outer.labels().is_empty(), "{:?}", outer.labels());
    }

    #[test]
    fn attributes_parse_type_and_rdf_type_values() {
        // Node element attributes: identity + rdf:type + xml:lang.
        let node_empty = at(&onto("  <rdf:Description |/>\n"));
        assert_eq!(
            node_empty.labels(),
            ["rdf:about", "rdf:ID", "rdf:nodeID", "rdf:type", "xml:lang"]
        );
        // Once rdf:about is present, rdf:ID and rdf:nodeID are omitted.
        let node_about = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\" |/>\n",
        ));
        assert_eq!(node_about.labels(), ["rdf:type", "xml:lang"]);
        // Property element attributes: object-fixing + rdf:ID + xml:lang.
        let prop_empty = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:feeds |/>\n  </rdf:Description>\n",
        ));
        assert_eq!(
            prop_empty.labels(),
            [
                "rdf:resource",
                "rdf:datatype",
                "rdf:parseType",
                "rdf:nodeID",
                "rdf:ID",
                "xml:lang"
            ]
        );
        // Once rdf:resource is present, mutually exclusive attributes are omitted.
        let prop_res = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:feeds rdf:resource=\"http://example.org/o\" |/>\n  </rdf:Description>\n",
        ));
        assert_eq!(prop_res.labels(), ["rdf:ID", "xml:lang"]);
        // Filtering by local name (`res` -> `rdf:resource`) and snippet vs plain edit.
        let prop_filt = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:feeds res|/>\n  </rdf:Description>\n",
        ));
        assert_eq!(prop_filt.labels(), ["rdf:resource"]);
        let CompletionResponse::List(plain_list) = super::complete(
            &prop_filt.a,
            &prop_filt.k,
            Encoding::Utf16,
            prop_filt.offset,
            false,
        ) else {
            panic!("list");
        };
        let plain = prop_filt.accepted(&plain_list.items[0]);
        assert!(plain.contains("<ex:feeds rdf:resource=\"\"/>"), "{plain}");
        let CompletionResponse::List(snip_list) = super::complete(
            &prop_filt.a,
            &prop_filt.k,
            Encoding::Utf16,
            prop_filt.offset,
            true,
        ) else {
            panic!("list");
        };
        assert_eq!(
            snip_list.items[0].insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
        assert_eq!(
            snip_list.items[0].text_edit,
            Some(CompletionTextEdit::Edit(TextEdit::new(
                prop_filt.a.range(
                    Encoding::Utf16,
                    crate::syntax::Span::new(prop_filt.offset as u32 - 3, prop_filt.offset as u32)
                ),
                "rdf:resource=\"$0\"".into(),
            )))
        );
        // rdf:parseType values.
        let pt = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:feeds rdf:parseType=\"|\"></ex:feeds>\n  </rdf:Description>\n",
        ));
        assert_eq!(pt.labels(), ["Resource", "Collection", "Literal", "Triple"]);
        let pt_r = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\">\n    <ex:feeds rdf:parseType=\"Res|\"></ex:feeds>\n  </rdf:Description>\n",
        ));
        assert_eq!(pt_r.labels(), ["Resource"]);
        // rdf:type attribute values on a node element offer classes (not properties/datatypes).
        let ty = at(&onto(
            "  <rdf:Description rdf:about=\"http://example.org/s\" rdf:type=\"http://example.org/|\"/>\n",
        ));
        let labels = ty.labels();
        assert!(has(&labels, "http://example.org/Pump"), "{labels:?}");
        assert!(has(&labels, "http://example.org/Valve"), "{labels:?}");
        assert!(!has(&labels, "http://example.org/feeds"), "{labels:?}");
    }
}
