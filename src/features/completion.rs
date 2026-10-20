//! Context-aware completion.
//!
//! - `prefix:` + partial local name: terms of that namespace, from the bundled
//!   vocabulary and from everything the workspace uses. A well-known prefix that
//!   is not declared yet gets its `@prefix` line added on accept.
//! - Anything else: prefixes (declared, project, well-known), keywords, and
//!   resources described in the workspace.
//!
//! Candidates are ranked by position: after `a` classes come first, in predicate
//! position properties come first. Before that, [`ranking`] puts what the
//! ontology and the SHACL shapes expect at the cursor on top (and offers it even
//! when nothing is typed yet). Results are capped; `isIncomplete` makes the
//! client ask again as the user types.
//!
//! The candidate sources, the ranking and the item building are shared with
//! SPARQL completion (`sparql::features::completion`), which finds the slot
//! and the subject from the query's structure instead.

pub(crate) mod ranking;

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::prefix_insertion;
use crate::knowledge::{Knowledge, namespace_of};
use crate::syntax::lexer::{Kind, is_pn_chars};
use crate::syntax::{Iri, PrefixMap, RDF_TYPE, Role, Span, Syntax};
use crate::vocab::{TermKind, VOCABS};
use lsp_types::*;
use rustc_hash::FxHashSet;
use serde_json::json;

const LIMIT: usize = 200;
/// Vocabularies searched by local name even when their prefix is not declared.
const COMMON: &[&str] = &[
    "rdf", "rdfs", "owl", "xsd", "skos", "sh", "dcterms", "foaf", "schema", "prov",
];

/// What kind of term belongs at the cursor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Slot {
    Subject,
    Predicate,
    Object,
    Class,
    Datatype,
}

pub(crate) struct Candidate {
    /// Ranking tier (see [`ranking`]); sorts before everything else.
    pub tier: u8,
    pub label: String,
    pub kind: CompletionItemKind,
    pub detail: String,
    pub iri: Option<String>,
    pub rank: u8,
    /// A prefix to declare on accept: (name, namespace).
    pub import: Option<(String, String)>,
}

impl Candidate {
    /// A candidate that is no term (a keyword, a variable, a function).
    pub fn plain(label: String, kind: CompletionItemKind, detail: String, rank: u8) -> Candidate {
        Candidate {
            tier: ranking::NONE,
            label,
            kind,
            detail,
            iri: None,
            rank,
            import: None,
        }
    }
}

/// How the document's syntax declares a prefix, as named in item details:
/// `@prefix` (Turtle, TriG) or `PREFIX` (SPARQL).
pub(crate) type Directive = &'static str;

pub fn complete(
    a: &Analysis,
    text: &str,
    k: &Knowledge,
    enc: Encoding,
    pos: Position,
) -> CompletionResponse {
    // Everything offered here is a prefixed name or a keyword, none of which
    // N-Triples or N-Quads allows. JSON-LD completion is not written yet.
    if a.parsed.syntax.is_line_based() || a.parsed.syntax == Syntax::JsonLd {
        return CompletionResponse::Array(vec![]);
    }
    let Some(offset) = a.offset(enc, pos) else {
        return CompletionResponse::Array(vec![]);
    };
    let offset = offset as usize;
    let start = word_start(text, offset);
    let end = word_end(text, offset);
    let typed = &text[start..offset];
    let replace = a.range(enc, Span::new(start as u32, end as u32));
    let slot = slot_at(a, start as u32);
    let prefixes = a.prefix_map();

    let ranked = match slot {
        Slot::Predicate => ranking::predicates(a, k, start as u32, &prefixes),
        Slot::Object => ranking::objects(a, k, start as u32, &prefixes, &|iri| {
            may_match(&iri, typed, &prefixes)
        }),
        _ => ranking::Ranking::default(),
    };

    let mut cands: Vec<Candidate> = Vec::new();
    let mut incomplete = false;

    if let Some(colon) = typed.find(':') {
        match prefixed_terms(k, &typed[..colon], &typed[colon + 1..], slot, &prefixes) {
            Some(c) => cands = c,
            None => return CompletionResponse::Array(vec![]),
        }
    } else {
        incomplete = true;
        prefix_candidates(&mut cands, typed, &prefixes, "@prefix", false);
        let keywords: &[(&str, &str)] = match slot {
            Slot::Subject => &[
                ("@prefix", "@prefix ex: <http://example.org/> ."),
                ("@base", "@base <> ."),
                ("PREFIX", "PREFIX"),
                ("BASE", "BASE"),
            ],
            Slot::Predicate => &[("a", "rdf:type")],
            _ => &[("true", "xsd:boolean"), ("false", "xsd:boolean")],
        };
        for (k, d) in keywords {
            if typed.is_empty() || k.starts_with(typed) {
                cands.push(Candidate::plain(
                    k.to_string(),
                    CompletionItemKind::KEYWORD,
                    d.to_string(),
                    1,
                ));
            }
        }
        described_candidates(&mut cands, typed, slot, k, &prefixes);
        if !typed.starts_with('@') {
            local_name_candidates(&mut cands, typed, slot, &prefixes);
        }
    }

    incomplete |= finish(&mut cands, ranked, slot, typed, k, &prefixes);
    let items = items(cands, replace, &prefixes, "@prefix", |p, ns| {
        prefix_insertion(a, text, enc, p, ns)
    });
    CompletionResponse::List(CompletionList {
        is_incomplete: incomplete,
        items,
    })
}

fn term_item_kind(kind: TermKind) -> CompletionItemKind {
    match kind {
        TermKind::Class => CompletionItemKind::CLASS,
        TermKind::Property => CompletionItemKind::PROPERTY,
        TermKind::Individual => CompletionItemKind::ENUM_MEMBER,
    }
}

/// The terms of the namespace `pfx` stands for whose local name matches
/// `local`: the bundled vocabulary's, then those the workspace defines or
/// uses. `None` when `pfx` is neither declared nor a well-known prefix (a
/// well-known one is added on accept).
pub(crate) fn prefixed_terms(
    k: &Knowledge,
    pfx: &str,
    local: &str,
    slot: Slot,
    prefixes: &PrefixMap,
) -> Option<Vec<Candidate>> {
    let (ns, import) = match prefixes.get(pfx) {
        Some(ns) => (ns.clone(), None),
        None => {
            let v = VOCABS.by_prefix(pfx)?;
            (
                v.namespace.to_string(),
                Some((pfx.to_string(), v.namespace.to_string())),
            )
        }
    };
    let mut cands = Vec::new();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    if let Some(v) = VOCABS.by_namespace(&ns) {
        for t in v.terms() {
            if !fuzzy(local, &t.local) {
                continue;
            }
            seen.insert(t.local.clone());
            cands.push(Candidate {
                tier: ranking::NONE,
                label: format!("{pfx}:{}", t.local),
                kind: term_item_kind(t.kind),
                detail: if t.label.is_empty() {
                    format!("{}{}", ns, t.local)
                } else {
                    t.label.clone()
                },
                iri: Some(format!("{ns}{}", t.local)),
                rank: rank(slot, t.kind) + if t.deprecated { 5 } else { 0 },
                import: import.clone(),
            });
        }
    }
    // Terms the workspace defines or uses in this namespace.
    for iri in k.terms_in(&ns) {
        let Some(l) = iri.strip_prefix(ns.as_str()) else {
            continue;
        };
        if l.is_empty() || seen.contains(l) || !is_local_name(l) || !fuzzy(local, l) {
            continue;
        }
        let kind = guess_kind(k, &iri);
        let label = k.label(&iri);
        cands.push(Candidate {
            tier: ranking::NONE,
            label: format!("{pfx}:{l}"),
            kind: match kind {
                TermKind::Class => CompletionItemKind::CLASS,
                TermKind::Property => CompletionItemKind::PROPERTY,
                TermKind::Individual => CompletionItemKind::REFERENCE,
            },
            detail: label.unwrap_or(&iri).to_string(),
            iri: Some(iri.to_string()),
            rank: rank(slot, kind),
            import: import.clone(),
        });
    }
    Some(cands)
}

/// Prefixes matching `typed`: those in scope, then the well-known ones not
/// declared yet (added on accept; with nothing typed only when
/// `well_known_when_empty`).
pub(crate) fn prefix_candidates(
    cands: &mut Vec<Candidate>,
    typed: &str,
    prefixes: &PrefixMap,
    directive: Directive,
    well_known_when_empty: bool,
) {
    let mut pfx_seen: FxHashSet<String> = FxHashSet::default();
    for (p, ns) in prefixes {
        if fuzzy(typed, p) {
            pfx_seen.insert(p.clone());
            cands.push(Candidate::plain(
                format!("{p}:"),
                CompletionItemKind::MODULE,
                ns.clone(),
                2,
            ));
        }
    }
    for v in &VOCABS.all {
        if !pfx_seen.contains(v.prefix)
            && (!typed.is_empty() || well_known_when_empty)
            && v.prefix.starts_with(typed)
        {
            cands.push(Candidate {
                import: Some((v.prefix.to_string(), v.namespace.to_string())),
                ..Candidate::plain(
                    format!("{}:", v.prefix),
                    CompletionItemKind::MODULE,
                    format!("{} (adds {directive})", v.namespace),
                    4,
                )
            });
        }
    }
}

/// Resources described in the workspace whose local name matches `typed`,
/// written with the document's prefixes (subject, object and class slots,
/// once something is typed).
pub(crate) fn described_candidates(
    cands: &mut Vec<Candidate>,
    typed: &str,
    slot: Slot,
    k: &Knowledge,
    prefixes: &PrefixMap,
) {
    if typed.is_empty() || !matches!(slot, Slot::Subject | Slot::Object | Slot::Class) {
        return;
    }
    let mut seen = FxHashSet::default();
    for iri in k.described_terms() {
        if let Some(short) = shorten(&iri, prefixes) {
            let local = short.split_once(':').map_or("", |x| x.1);
            if fuzzy(typed, local) && seen.insert(short.clone()) {
                cands.push(Candidate {
                    tier: ranking::NONE,
                    label: short,
                    kind: CompletionItemKind::REFERENCE,
                    detail: k.label(&iri).unwrap_or(&iri).to_string(),
                    iri: Some(iri.to_string()),
                    rank: 3,
                    import: None,
                });
            }
        }
    }
}

/// Vocabulary terms by local name ("Pers" -> foaf:Person), from namespaces in
/// scope plus the common vocabularies (those add their prefix on accept);
/// from two characters typed.
pub(crate) fn local_name_candidates(
    cands: &mut Vec<Candidate>,
    typed: &str,
    slot: Slot,
    prefixes: &PrefixMap,
) {
    if typed.chars().count() < 2 {
        return;
    }
    let lower = typed.to_lowercase();
    let mut seen_ns = FxHashSet::default();
    let in_scope = prefixes
        .iter()
        .filter_map(|(p, ns)| VOCABS.by_namespace(ns).map(|v| (p.clone(), v, None)));
    let common = COMMON.iter().filter_map(|p| VOCABS.by_prefix(p)).map(|v| {
        (
            v.prefix.to_string(),
            v,
            Some((v.prefix.to_string(), v.namespace.to_string())),
        )
    });
    for (pfx, v, import) in in_scope.chain(common) {
        // A common vocabulary already bound under another name is covered by in_scope.
        if !seen_ns.insert(v.namespace) || (import.is_some() && prefixes.contains_key(&pfx)) {
            continue;
        }
        for t in v
            .terms()
            .iter()
            .filter(|t| t.local.to_lowercase().starts_with(&lower))
        {
            cands.push(Candidate {
                tier: ranking::NONE,
                label: format!("{pfx}:{}", t.local),
                kind: term_item_kind(t.kind),
                detail: if t.label.is_empty() {
                    format!("{}{}", v.namespace, t.local)
                } else {
                    t.label.clone()
                },
                iri: Some(format!("{}{}", v.namespace, t.local)),
                rank: rank(slot, t.kind) + 2 + u8::from(t.deprecated) * 5,
                import: import.clone(),
            });
        }
    }
}

/// Applies the ranking, sorts and caps the candidates. Returns whether some
/// were left out (by the cap or by the ranking).
pub(crate) fn finish(
    cands: &mut Vec<Candidate>,
    ranked: ranking::Ranking,
    slot: Slot,
    typed: &str,
    k: &Knowledge,
    prefixes: &PrefixMap,
) -> bool {
    let mut incomplete = apply_ranking(cands, ranked, slot, typed, k, prefixes);
    let lower = typed.rsplit(':').next().unwrap_or("").to_lowercase();
    cands.sort_by(|x, y| {
        let px = !local_of(&x.label).to_lowercase().starts_with(&lower);
        let py = !local_of(&y.label).to_lowercase().starts_with(&lower);
        (x.tier, x.rank, px, x.label.len(), &x.label).cmp(&(
            y.tier,
            y.rank,
            py,
            y.label.len(),
            &y.label,
        ))
    });
    if cands.len() > LIMIT {
        cands.truncate(LIMIT);
        incomplete = true;
    }
    incomplete
}

/// The completion items, in the candidates' order, replacing `replace`. A
/// candidate whose prefix is not declared gets `import_edit(name, namespace)`
/// as an additional edit. Items for terms carry `data.iri` for [`resolve`].
pub(crate) fn items(
    cands: Vec<Candidate>,
    replace: Range,
    prefixes: &PrefixMap,
    directive: Directive,
    import_edit: impl Fn(&str, &str) -> TextEdit,
) -> Vec<CompletionItem> {
    cands
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let additional = c
                .import
                .as_ref()
                .filter(|(p, _)| !prefixes.contains_key(p))
                .map(|(p, ns)| vec![import_edit(p, ns)]);
            CompletionItem {
                filter_text: Some(c.label.clone()),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                    range: replace,
                    new_text: c.label.clone(),
                })),
                label: c.label,
                kind: Some(c.kind),
                label_details: additional.as_ref().map(|_| CompletionItemLabelDetails {
                    detail: None,
                    description: Some(format!("+{directive}")),
                }),
                detail: Some(c.detail),
                sort_text: Some(format!("{i:04}")),
                additional_text_edits: additional,
                data: c.iri.map(|iri| json!({ "iri": iri })),
                ..Default::default()
            }
        })
        .collect()
}

/// Fill in documentation lazily so the list itself stays small.
pub fn resolve(mut item: CompletionItem, k: &Knowledge) -> CompletionItem {
    let Some(iri) = item
        .data
        .as_ref()
        .and_then(|d| d["iri"].as_str())
        .map(str::to_string)
    else {
        return item;
    };
    if let Some(md) = super::hover::describe_iri(&iri, &item.label, k, &Default::default()) {
        item.documentation = Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: md,
        }));
    }
    item
}

/// Tag candidates the ranking knows about, and add the ranked ones that are
/// not candidates yet (with nothing typed there are few). Returns whether
/// the ranking left some out.
fn apply_ranking(
    cands: &mut Vec<Candidate>,
    ranked: ranking::Ranking,
    slot: Slot,
    typed: &str,
    k: &Knowledge,
    prefixes: &PrefixMap,
) -> bool {
    if ranked.iris.is_empty() && ranked.literals.is_empty() {
        return ranked.truncated;
    }
    let mut present: FxHashSet<Iri> = FxHashSet::default();
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
    let wanted = |label: &str| match typed.split_once(':') {
        Some((p, local)) => label
            .strip_prefix(p)
            .and_then(|l| l.strip_prefix(':'))
            .is_some_and(|l| fuzzy(local, l)),
        None => fuzzy(typed, local_of(label)),
    };
    for (iri, h) in &ranked.iris {
        if present.contains(iri) {
            continue;
        }
        let (label, import) = written(iri, prefixes);
        if !wanted(&label) {
            continue;
        }
        let vocab = VOCABS
            .lookup(iri)
            .and_then(|(v, l)| v.term(l))
            .filter(|t| !t.label.is_empty());
        let detail = h.detail.clone().unwrap_or_else(|| {
            k.label(iri)
                .or(vocab.map(|t| t.label.as_str()))
                .unwrap_or(iri)
                .to_string()
        });
        cands.push(Candidate {
            tier: h.tier,
            label,
            kind: if slot == Slot::Predicate {
                CompletionItemKind::PROPERTY
            } else if h.enumerated {
                CompletionItemKind::ENUM_MEMBER
            } else {
                CompletionItemKind::REFERENCE
            },
            detail,
            iri: Some(iri.to_string()),
            rank: 0,
            import,
        });
    }
    for (text, detail) in ranked.literals {
        if typed.is_empty() || fuzzy(typed, &text) {
            cands.push(Candidate {
                tier: ranking::REQUIRED,
                label: text,
                kind: CompletionItemKind::VALUE,
                detail,
                iri: None,
                rank: 0,
                import: None,
            });
        }
    }
    ranked.truncated
}

/// How to write `iri` here: with a prefix in scope, else with a well-known
/// vocabulary's prefix (added on accept), else in full.
fn written(iri: &str, prefixes: &PrefixMap) -> (String, Option<(String, String)>) {
    if let Some(short) = shorten(iri, prefixes) {
        return (short, None);
    }
    if let Some((v, local)) = VOCABS.lookup(iri)
        && !prefixes.contains_key(v.prefix)
        && is_local_name(local)
    {
        return (
            format!("{}:{local}", v.prefix),
            Some((v.prefix.to_string(), v.namespace.to_string())),
        );
    }
    (format!("<{iri}>"), None)
}

/// Whether `iri` could match what was typed, without building its label.
pub(crate) fn may_match(iri: &str, typed: &str, prefixes: &PrefixMap) -> bool {
    match typed.split_once(':') {
        Some((p, local)) => prefixes
            .get(p)
            .is_some_and(|ns| iri.starts_with(ns.as_str()) && fuzzy(local, &iri[ns.len()..])),
        None => fuzzy(typed, &iri[namespace_of(iri).len()..]),
    }
}

fn local_of(label: &str) -> &str {
    label.split_once(':').map_or(label, |x| x.1)
}

fn rank(slot: Slot, kind: TermKind) -> u8 {
    match (slot, kind) {
        (Slot::Class, TermKind::Class) | (Slot::Predicate, TermKind::Property) => 0,
        (Slot::Object | Slot::Subject, TermKind::Individual) => 0,
        (Slot::Object | Slot::Subject | Slot::Datatype, _) => 1,
        _ => 2,
    }
}

fn guess_kind(k: &Knowledge, iri: &crate::syntax::Iri) -> TermKind {
    let types = k.types(iri);
    if types
        .iter()
        .any(|t| t.ends_with("Class") || t.ends_with("NodeShape"))
    {
        TermKind::Class
    } else if types.iter().any(|t| t.ends_with("Property")) || k.used_as_predicate(iri) {
        TermKind::Property
    } else {
        TermKind::Individual
    }
}

/// What kind of term belongs at `offset`, from the tokens before it.
fn slot_at(a: &Analysis, offset: u32) -> Slot {
    let toks = &a.parsed.tokens;
    let i = toks.partition_point(|t| t.end <= offset);
    let prev = toks[..i].iter().rev().find(|t| t.kind != Kind::Comment);
    let Some(prev) = prev else {
        return Slot::Subject;
    };
    match prev.kind {
        // TriG: `{` and `}` delimit graph blocks; `GRAPH` precedes a graph name.
        Kind::Dot | Kind::LBrace | Kind::RBrace | Kind::Graph => Slot::Subject,
        Kind::Semicolon | Kind::LBracket | Kind::LAnnotation => Slot::Predicate,
        Kind::A => Slot::Class,
        Kind::Caret2 => Slot::Datatype,
        Kind::Comma | Kind::LParen => {
            // Object list continues: class if the predicate was rdf:type.
            let pred = a
                .parsed
                .refs
                .iter()
                .rev()
                .find(|r| r.span.end <= prev.start && r.role == Role::Predicate);
            if pred.is_some_and(|p| p.iri == RDF_TYPE) {
                Slot::Class
            } else {
                Slot::Object
            }
        }
        Kind::Iri | Kind::PName => match a.ref_at(prev.start).map(|r| r.role) {
            Some(Role::Subject) => Slot::Predicate,
            Some(Role::Predicate) => {
                if a.ref_at(prev.start).is_some_and(|r| r.iri == RDF_TYPE) {
                    Slot::Class
                } else {
                    Slot::Object
                }
            }
            _ => Slot::Predicate,
        },
        Kind::BNode | Kind::RBracket | Kind::RParen => Slot::Predicate,
        _ => Slot::Object,
    }
}

fn is_word_char(c: char) -> bool {
    is_pn_chars(c) || c == ':' || c == '.' || c == '@' || c == '%' || c == '\\'
}

fn word_start(text: &str, offset: usize) -> usize {
    let mut s = offset;
    for (i, c) in text[..offset].char_indices().rev() {
        if is_word_char(c) {
            s = i;
        } else {
            break;
        }
    }
    // A trailing '.' that ends a statement is not part of the word.
    s
}

fn word_end(text: &str, offset: usize) -> usize {
    let mut e = offset;
    for (i, c) in text[offset..].char_indices() {
        if is_pn_chars(c) || c == ':' {
            e = offset + i + c.len_utf8();
        } else {
            break;
        }
    }
    e
}

pub(crate) fn is_local_name(l: &str) -> bool {
    l.chars().all(|c| is_pn_chars(c) || c == '.') && !l.ends_with('.')
}

/// Case-insensitive subsequence match; empty query matches everything.
pub(crate) fn fuzzy(query: &str, cand: &str) -> bool {
    let mut it = cand.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|q| it.any(|c| c == q))
}

/// `iri` as `prefix:local` using the longest matching namespace.
pub fn shorten(iri: &str, prefixes: &PrefixMap) -> Option<String> {
    prefixes
        .iter()
        .filter(|(_, ns)| {
            !ns.is_empty() && iri.starts_with(ns.as_str()) && is_local_name(&iri[ns.len()..])
        })
        .max_by_key(|(_, ns)| ns.len())
        .map(|(p, ns)| format!("{p}:{}", &iri[ns.len()..]))
}
