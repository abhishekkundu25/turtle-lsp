//! Context-aware completion.
//!
//! - `prefix:` + partial local name: terms of that namespace, from the bundled
//!   vocabulary and from everything the workspace uses. A well-known prefix that
//!   is not declared yet gets its `@prefix` line added on accept.
//! - Anything else: prefixes (declared, project, well-known), keywords, and
//!   resources described in the workspace.
//! - In the query literal of a SHACL shape: SPARQL completion
//!   ([`super::embedded`]).
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

mod jsonld;
pub(crate) mod ranking;

// `ranking` and the other modules of this directory write IRIs through here.
pub(crate) use crate::syntax::names::shorten;

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::syntax::lexer::Kind;
use crate::syntax::names::{
    Scope, declaration_edit, is_pn_chars, is_pn_local, vocabulary_namespace,
};
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

pub fn complete(a: &Analysis, k: &Knowledge, enc: Encoding, pos: Position) -> CompletionResponse {
    // Everything offered here is a prefixed name or a keyword, none of which
    // N-Triples or N-Quads allows.
    if a.parsed.syntax.is_line_based() {
        return CompletionResponse::Array(vec![]);
    }
    let Some(offset) = a.offset(enc, pos) else {
        return CompletionResponse::Array(vec![]);
    };
    let offset = offset as usize;
    if a.parsed.syntax == Syntax::JsonLd {
        return jsonld::complete(a, k, enc, offset);
    }
    if let Some(r) = super::embedded::completion(a, k, enc, offset as u32) {
        return r;
    }
    // In any other string (a label, a description) nothing: editors suggest
    // while typing in Turtle strings, for the queries.
    let t = a.tokens();
    let i = t.partition_point(|t| t.start < offset as u32);
    if i > 0 && t[i - 1].kind == Kind::String && offset < t[i - 1].end as usize {
        return CompletionResponse::Array(vec![]);
    }
    let text: &str = &a.text;
    let start = word_start(text, offset);
    let end = word_end(text, offset);
    let typed = &text[start..offset];
    let replace = a.range(enc, Span::new(start as u32, end as u32));
    let slot = slot_at(a, start as u32);
    // Only what is declared above the cursor can be written there.
    let prefixes = Scope::at(a, start as u32);

    let ranked = match slot {
        Slot::Predicate => ranking::predicates(a, k, start as u32, &prefixes),
        Slot::Object => ranking::objects(a, k, start as u32, &prefixes, &|iri| {
            may_match(&iri, typed, &prefixes)
        }),
        _ => ranking::Ranking::default(),
    };

    let mut cands: Vec<Candidate> = Vec::new();
    let mut incomplete = false;
    // The ranking moves some terms up; the long lists are cut around those.
    let promoted = |iri: Iri| ranked.iris.contains_key(&iri);
    let best = Best {
        typed,
        promoted: &promoted,
    };

    if let Some(colon) = typed.find(':') {
        match prefixed_terms_best(
            k,
            &typed[..colon],
            &typed[colon + 1..],
            slot,
            &prefixes,
            Some(best),
        ) {
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
        described_candidates_best(&mut cands, typed, slot, k, &prefixes, Some(best));
        if !typed.starts_with('@') {
            local_name_candidates(&mut cands, typed, slot, &prefixes);
        }
    }

    incomplete |= finish(&mut cands, ranked, slot, typed, k, &prefixes);
    let items = items(cands, replace, &prefixes, "@prefix", |p, ns| {
        declaration_edit(a, enc, p, ns, start as u32)
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
    prefixed_terms_best(k, pfx, local, slot, prefixes, None)
}

/// What a completion keeps of a long list of candidates: the best
/// [`LIMIT`] + 1 (one more than it shows, to tell it cut some), by the order
/// [`finish`] sorts in, and every candidate `promoted` says the ranking will
/// move up, which that order does not know. `typed` is what was typed.
#[derive(Clone, Copy)]
pub(crate) struct Best<'a> {
    pub typed: &'a str,
    pub promoted: &'a dyn Fn(Iri) -> bool,
}

/// [`prefixed_terms`], building candidates only for the `best` ones when
/// given (the others cannot be among the first [`LIMIT`] after sorting).
pub(crate) fn prefixed_terms_best(
    k: &Knowledge,
    pfx: &str,
    local: &str,
    slot: Slot,
    prefixes: &PrefixMap,
    best: Option<Best>,
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
    Some(namespace_terms_best(
        k,
        &ns,
        local,
        slot,
        &|l| format!("{pfx}:{l}"),
        import,
        best,
    ))
}

/// The terms of namespace `ns` whose local name matches `local`, labelled
/// `label(local name)`: the bundled vocabulary's, then those the workspace
/// defines or uses.
pub(crate) fn namespace_terms(
    k: &Knowledge,
    ns: &str,
    local: &str,
    slot: Slot,
    label: &dyn Fn(&str) -> String,
    import: Option<(String, String)>,
) -> Vec<Candidate> {
    namespace_terms_best(k, ns, local, slot, label, import, None)
}

fn namespace_terms_best(
    k: &Knowledge,
    ns: &str,
    local: &str,
    slot: Slot,
    label: &dyn Fn(&str) -> String,
    import: Option<(String, String)>,
    best: Option<Best>,
) -> Vec<Candidate> {
    let mut cands = Vec::new();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    if let Some(v) = VOCABS.by_namespace(ns) {
        for t in v.terms() {
            if !fuzzy(local, &t.local) {
                continue;
            }
            seen.insert(t.local.clone());
            cands.push(Candidate {
                tier: ranking::NONE,
                label: label(&t.local),
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
    let mut rows: Vec<(Iri, TermKind)> = Vec::new();
    for iri in k.terms_in(ns) {
        let Some(l) = iri.strip_prefix(ns) else {
            continue;
        };
        if l.is_empty() || seen.contains(l) || !is_pn_local(l) || !fuzzy(local, l) {
            continue;
        }
        rows.push((iri, guess_kind(k, &iri)));
    }
    if let Some(best) = best {
        keep_best(&mut rows, ns, slot, label, best);
    }
    for (iri, kind) in rows {
        let l = iri.strip_prefix(ns).unwrap_or_default();
        let name = k.label(&iri);
        cands.push(Candidate {
            tier: ranking::NONE,
            label: label(l),
            kind: match kind {
                TermKind::Class => CompletionItemKind::CLASS,
                TermKind::Property => CompletionItemKind::PROPERTY,
                TermKind::Individual => CompletionItemKind::REFERENCE,
            },
            detail: name.unwrap_or(&iri).to_string(),
            iri: Some(iri.to_string()),
            rank: rank(slot, kind),
            import: import.clone(),
        });
    }
    cands
}

/// Drops from `rows` (workspace terms of `ns`) those that [`finish`] would
/// cut: all but the best [`LIMIT`] + 1 by its order, and those `best` says
/// are promoted. Keeps all of them when the label is not `label(local name)`
/// with something fixed before the local name.
fn keep_best(
    rows: &mut Vec<(Iri, TermKind)>,
    ns: &str,
    slot: Slot,
    label: &dyn Fn(&str) -> String,
    best: Best,
) {
    if rows.len() <= LIMIT + 1 {
        return;
    }
    let affix = label("");
    if label("zz") != format!("{affix}zz") {
        return;
    }
    let lower = lower_typed(best.typed);
    let key = |&(iri, kind): &(Iri, TermKind)| {
        let l = iri.strip_prefix(ns).unwrap_or_default();
        // The local name of the label: after the first colon of the label.
        let shown = if affix.contains(':') { l } else { local_of(l) };
        (
            ranking::NONE,
            rank(slot, kind),
            !starts_with_lowercase(shown, &lower),
            affix.len() + l.len(),
        )
    };
    let (promoted, rest): (Vec<_>, Vec<_>) = std::mem::take(rows)
        .into_iter()
        .partition(|r| (best.promoted)(r.0));
    let mut keyed: Vec<_> = rest.into_iter().map(|r| (key(&r), r)).collect();
    let order = |a: &((u8, u8, bool, usize), (Iri, TermKind)),
                 b: &((u8, u8, bool, usize), (Iri, TermKind))| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.0.strip_prefix(ns).cmp(&b.1.0.strip_prefix(ns)))
    };
    if keyed.len() > LIMIT + 1 {
        keyed.select_nth_unstable_by(LIMIT + 1, order);
        keyed.truncate(LIMIT + 1);
    }
    rows.extend(promoted);
    rows.extend(keyed.into_iter().map(|(_, r)| r));
}

/// The lower-cased last part of what was typed, as [`finish`] compares.
fn lower_typed(typed: &str) -> String {
    typed.rsplit(':').next().unwrap_or("").to_lowercase()
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
    described_candidates_best(cands, typed, slot, k, prefixes, None)
}

/// [`described_candidates`], building candidates only for the `best` ones
/// when given.
pub(crate) fn described_candidates_best(
    cands: &mut Vec<Candidate>,
    typed: &str,
    slot: Slot,
    k: &Knowledge,
    prefixes: &PrefixMap,
    best: Option<Best>,
) {
    if typed.is_empty() || !matches!(slot, Slot::Subject | Slot::Object | Slot::Class) {
        return;
    }
    let mut rows: Vec<(String, Iri)> = Vec::new();
    for iri in k.described_terms() {
        if let Some(short) = shorten(&iri, prefixes) {
            let local = short.split_once(':').map_or("", |x| x.1);
            if fuzzy(typed, local) {
                rows.push((short, iri));
            }
        }
    }
    if let Some(best) = best
        && rows.len() > LIMIT + 1
    {
        // All of them are the same tier and rank: the order is that of the
        // names, but for those the ranking moves up.
        let lower = lower_typed(best.typed);
        let key = |r: &(String, Iri)| (!starts_with_lowercase(local_of(&r.0), &lower), r.0.len());
        let (promoted, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut rows)
            .into_iter()
            .partition(|r| (best.promoted)(r.1));
        let mut keyed: Vec<_> = rest.into_iter().map(|r| (key(&r), r)).collect();
        if keyed.len() > LIMIT + 1 {
            let order = |a: &((bool, usize), (String, Iri)), b: &((bool, usize), (String, Iri))| {
                a.0.cmp(&b.0).then_with(|| a.1.0.cmp(&b.1.0))
            };
            keyed.select_nth_unstable_by(LIMIT + 1, order);
            keyed.truncate(LIMIT + 1);
        }
        rows.extend(promoted);
        rows.extend(keyed.into_iter().map(|(_, r)| r));
    }
    let mut seen = FxHashSet::default();
    for (short, iri) in rows {
        if seen.insert(iri) {
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
    // Each candidate's sort key once, not at every comparison; ties keep the
    // order the candidates were found in.
    let keys: Vec<(u8, u8, bool, usize)> = cands
        .iter()
        .map(|c| {
            (
                c.tier,
                c.rank,
                !starts_with_lowercase(local_of(&c.label), &lower),
                c.label.len(),
            )
        })
        .collect();
    let order_of = |&a: &u32, &b: &u32| {
        let (x, y) = (a as usize, b as usize);
        (keys[x], &cands[x].label, x).cmp(&(keys[y], &cands[y].label, y))
    };
    let mut order: Vec<u32> = (0..cands.len() as u32).collect();
    if order.len() > LIMIT {
        // Only the first LIMIT are kept: find them, then sort just those.
        order.select_nth_unstable_by(LIMIT, order_of);
        order.truncate(LIMIT);
        incomplete = true;
    }
    order.sort_unstable_by(order_of);
    let mut found: Vec<Option<Candidate>> = cands.drain(..).map(Some).collect();
    cands.extend(order.iter().filter_map(|&i| found[i as usize].take()));
    incomplete
}

/// Whether `local`, lower-cased, starts with `lower` (already lower-cased).
fn starts_with_lowercase(local: &str, lower: &str) -> bool {
    if local.is_ascii() && lower.is_ascii() {
        local
            .as_bytes()
            .get(..lower.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(lower.as_bytes()))
    } else {
        local.to_lowercase().starts_with(lower)
    }
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
        && is_pn_local(local)
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
        None => fuzzy(typed, &iri[vocabulary_namespace(iri).len()..]),
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
    let toks = a.tokens();
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

/// Case-insensitive subsequence match; empty query matches everything.
pub(crate) fn fuzzy(query: &str, cand: &str) -> bool {
    let mut it = cand.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|q| it.any(|c| c == q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::Source;
    use std::sync::Arc;

    const SHAPES: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                          @prefix ex: <http://example.com/> .\n\
                          ex:P sh:declare [ sh:prefix \"my\" ; sh:namespace \"http://example.com/my#\" ] .\n\
                          ex:S sh:sparql [ sh:prefixes ex:P ; sh:select \"\"\"\n\
                          \x20 SELECT $this WHERE {\n\
                          \x20   $this ex:next ?n .\n\
                          \x20   \n\
                          \x20 }\"\"\" ] .\n\
                          ex:T a sh:NodeShape ; \n";

    /// The completion at the end of the first `needle` of `text`, as
    /// (label, edit range).
    fn complete_at(text: &str, needle: &str) -> Vec<(String, Range)> {
        let a = Arc::new(Analysis::new(
            "file:///w/shapes.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        let offset = (text.find(needle).unwrap() + needle.len()) as u32;
        let pos = a.position(Encoding::Utf16, offset);
        let items = match complete(&a, &k, Encoding::Utf16, pos) {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(l) => l.items,
        };
        items
            .into_iter()
            .map(|i| {
                let Some(CompletionTextEdit::Edit(e)) = i.text_edit else {
                    panic!("{} has no edit", i.label)
                };
                (i.label, e.range)
            })
            .collect()
    }

    /// The items offered at the end of the first `needle` of `text`.
    fn items_at(text: &str, needle: &str) -> Vec<CompletionItem> {
        let a = Arc::new(Analysis::new(
            "file:///w/doc.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/doc.ttl", Source::Open, a.clone());
        let offset = (text.find(needle).unwrap() + needle.len()) as u32;
        let pos = a.position(Encoding::Utf16, offset);
        match complete(&a, &k, Encoding::Utf16, pos) {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(l) => l.items,
        }
    }

    #[test]
    fn a_prefix_declared_only_further_down_is_added_at_the_cursor() {
        let foaf = "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n";
        let below = format!("<http://e/a> <http://e/p> foaf:Pers\n{foaf}");
        let items = items_at(&below, "foaf:Pers");
        let person = items.iter().find(|i| i.label == "foaf:Person").unwrap();
        let added = person
            .additional_text_edits
            .as_ref()
            .expect("a declaration");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].new_text, foaf);
        assert_eq!(added[0].range.start, Position::new(0, 0));
        // Declared above the cursor: nothing to add.
        let above = format!("{foaf}<http://e/a> <http://e/p> foaf:Pers\n");
        let items = items_at(&above, "foaf:Pers");
        let person = items.iter().find(|i| i.label == "foaf:Person").unwrap();
        assert!(person.additional_text_edits.is_none());
    }

    #[test]
    fn completion_never_writes_a_name_turtle_cannot_read() {
        let text = "@prefix ex: <http://example.org/> .\n<http://example.org/-x> a ex:Thing .\nex:s ex:p ex:";
        let items = items_at(text, "ex:s ex:p ex:");
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"ex:Thing"), "{labels:?}");
        assert!(!labels.contains(&"ex:-x"), "{labels:?}");
    }

    #[test]
    fn a_shacl_query_gets_sparql_completion() {
        let got = complete_at(SHAPES, "?n .\n    ");
        let labels: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
        for want in ["OPTIONAL", "FILTER", "MINUS", "my:"] {
            assert!(labels.contains(&want), "{want} not in {labels:?}");
        }
        assert!(!labels.contains(&"@prefix"), "{labels:?}");
        let line = SHAPES.lines().position(|l| l == "    ").unwrap() as u32;
        let (_, r) = got.iter().find(|g| g.0 == "OPTIONAL").unwrap();
        assert_eq!(
            *r,
            Range::new(Position::new(line, 4), Position::new(line, 4))
        );

        // The query's variables after `?`, with the range of what is typed.
        let filter = SHAPES.replace("?n .\n    \n", "?n .\n    FILTER (?\n");
        let got = complete_at(&filter, "FILTER (?");
        let labels: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
        assert!(
            labels.contains(&"?n") && labels.contains(&"?this"),
            "{labels:?}"
        );
        let line = filter
            .lines()
            .position(|l| l.ends_with("FILTER (?"))
            .unwrap() as u32;
        let (_, r) = got.iter().find(|g| g.0 == "?n").unwrap();
        assert_eq!(
            *r,
            Range::new(Position::new(line, 12), Position::new(line, 13))
        );

        // The terms of a prefix declared through sh:declare.
        let terms = SHAPES.replace("?n .\n    \n", "?n .\n    ?n my:\n")
            + "<http://example.com/my#knows> ex:p ex:o .\n";
        let got = complete_at(&terms, "?n my:");
        assert!(got.iter().any(|g| g.0 == "my:knows"), "{got:?}");
    }

    #[test]
    fn a_prefix_line_added_to_a_short_string_escapes_its_line_breaks() {
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                    ex:S sh:select \"SELECT $this WHERE { $this foaf:na }\" .\n";
        let a = Analysis::new(
            "file:///w/s.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        );
        let offset = (text.find("foaf:na").unwrap() + 7) as u32;
        let pos = a.position(Encoding::Utf16, offset);
        let CompletionResponse::List(l) = complete(&a, &Knowledge::default(), Encoding::Utf16, pos)
        else {
            panic!()
        };
        let item = l.items.iter().find(|i| i.label == "foaf:name").unwrap();
        let add = &item.additional_text_edits.as_ref().unwrap()[0];
        assert!(
            add.new_text
                .starts_with("PREFIX foaf: <http://xmlns.com/foaf/0.1/>\\n"),
            "{:?}",
            add.new_text
        );
        assert!(!add.new_text.contains('\n'), "{:?}", add.new_text);
        let start = text.find("SELECT").unwrap() as u32;
        assert_eq!(add.range.start, a.position(Encoding::Utf16, start));
    }

    #[test]
    fn outside_queries_completion_is_turtle() {
        let got = complete_at(SHAPES, "ex:T a sh:NodeShape ; ");
        let labels: Vec<&str> = got.iter().map(|g| g.0.as_str()).collect();
        assert!(
            labels.contains(&"a") && labels.contains(&"sh:"),
            "{labels:?}"
        );
        assert!(!labels.contains(&"OPTIONAL"), "{labels:?}");
    }

    #[test]
    fn other_strings_get_no_completion() {
        let text = format!("{SHAPES}rdfs:label \"Ali\" .\n");
        assert_eq!(complete_at(&text, "\"Ali"), []);
    }

    /// A deterministic spread of numbers.
    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed >> 33
    }

    /// The order before: a stable sort of everything by the key computed at
    /// each comparison, then the first [`LIMIT`].
    fn sorted_the_old_way(mut cands: Vec<Candidate>, typed: &str) -> Vec<String> {
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
        cands.truncate(LIMIT);
        cands
            .into_iter()
            .map(|c| format!("{}|{}", c.label, c.detail))
            .collect()
    }

    fn random_candidates(seed: u64, n: usize) -> Vec<Candidate> {
        let mut seed = seed;
        (0..n)
            .map(|i| {
                // Names that tie on length and prefix, duplicates, upper and
                // lower case, and a non-ASCII name now and then.
                let name = match lcg(&mut seed) % 9 {
                    0 => format!("ex:Pé{}", lcg(&mut seed) % 40),
                    1 => format!("ex:P{}", lcg(&mut seed) % 40),
                    2 => format!("ex:p{}", lcg(&mut seed) % 40),
                    3 => format!("ex:q{}", lcg(&mut seed) % 400),
                    _ => format!("ex:p{}", lcg(&mut seed) % 4000),
                };
                Candidate {
                    tier: (lcg(&mut seed) % 4) as u8,
                    rank: (lcg(&mut seed) % 3) as u8,
                    detail: format!("#{i}"),
                    ..Candidate::plain(name, CompletionItemKind::PROPERTY, String::new(), 0)
                }
            })
            .collect()
    }

    /// Sorting computed the lower-cased name of both candidates at every
    /// comparison (31 ms for `ex:p` over 40,000 terms). The order is the same,
    /// ties and duplicate names included, and the cut is as before.
    #[test]
    fn candidates_are_ordered_as_a_full_stable_sort_would() {
        let k = Knowledge::default();
        let prefixes = PrefixMap::default();
        for (seed, n) in [(1, 150), (2, 201), (3, 202), (4, 3000), (5, 20000)] {
            for typed in ["ex:p", "ex:P", "p", "ex:", "ex:pé", ""] {
                let cands = random_candidates(seed, n);
                let expected = sorted_the_old_way(random_candidates(seed, n), typed);
                let mut got = cands;
                let incomplete = finish(
                    &mut got,
                    ranking::Ranking::default(),
                    Slot::Predicate,
                    typed,
                    &k,
                    &prefixes,
                );
                let got: Vec<String> = got
                    .into_iter()
                    .map(|c| format!("{}|{}", c.label, c.detail))
                    .collect();
                assert_eq!(got, expected, "seed {seed} n {n} typed {typed:?}");
                assert_eq!(incomplete, n > LIMIT);
            }
        }
    }

    fn knowledge_of_widgets(n: usize) -> Knowledge {
        let mut k = Knowledge::default();
        for f in 0..n.div_ceil(500) {
            let mut t = String::from(
                "@prefix ex: <http://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n",
            );
            for i in f * 500..((f + 1) * 500).min(n) {
                let ty = match i % 7 {
                    0 => "owl:Class",
                    1 => "owl:ObjectProperty",
                    _ => "ex:Widget",
                };
                t.push_str(&format!("ex:p{i} a {ty} ; rdfs:label \"W{i}\" .\n"));
            }
            let uri = format!("file:///w/w{f}.ttl");
            let a = Arc::new(Analysis::new(
                uri.clone(),
                &t,
                Arc::new(PrefixMap::default()),
                true,
            ));
            k.set(&uri, Source::Disk, a);
        }
        k
    }

    /// Building a candidate for each of 40,000 terms to keep 200 of them was
    /// most of the cost. Only the best (and those the ranking moves up) are
    /// built; the items are the ones a full build gives.
    #[test]
    fn a_long_list_is_cut_to_what_would_be_shown_anyway() {
        let k = knowledge_of_widgets(6000);
        let prefixes: PrefixMap = [("ex".to_string(), "http://example.org/".to_string())]
            .into_iter()
            .collect();
        let ranked = || {
            let mut r = ranking::Ranking::default();
            // Terms far down the plain order, moved up by the ranking.
            for (i, tier) in [
                (5998, ranking::REQUIRED),
                (4001, ranking::SHAPE),
                (3, ranking::SCHEMA),
            ] {
                r.iris.insert(
                    Iri::intern(&format!("http://example.org/p{i}")),
                    ranking::Hit {
                        tier,
                        detail: Some(format!("ranked {i}")),
                        enumerated: false,
                    },
                );
            }
            r
        };
        for slot in [Slot::Predicate, Slot::Object, Slot::Subject, Slot::Class] {
            for typed in ["ex:p", "ex:P1", "ex:p59", "ex:zz"] {
                let (pfx, local) = typed.split_once(':').unwrap();
                let show = |cands: Vec<Candidate>| {
                    let mut cands = cands;
                    let incomplete = finish(&mut cands, ranked(), slot, typed, &k, &prefixes);
                    let items: Vec<String> = cands
                        .into_iter()
                        .map(|c| format!("{}|{}|{}|{}", c.label, c.detail, c.tier, c.rank))
                        .collect();
                    (items, incomplete)
                };
                let whole = show(prefixed_terms(&k, pfx, local, slot, &prefixes).unwrap());
                let r = ranked();
                let promoted = |i: Iri| r.iris.contains_key(&i);
                let best = Best {
                    typed,
                    promoted: &promoted,
                };
                let cut =
                    show(prefixed_terms_best(&k, pfx, local, slot, &prefixes, Some(best)).unwrap());
                assert_eq!(cut, whole, "{slot:?} {typed}");
            }
            // Without a prefix: the resources described, written with the prefixes in scope.
            for typed in ["p", "p1", "P59", "zz"] {
                let show = |best: Option<Best>| {
                    let mut cands = Vec::new();
                    described_candidates_best(&mut cands, typed, slot, &k, &prefixes, best);
                    let incomplete = finish(&mut cands, ranked(), slot, typed, &k, &prefixes);
                    let items: Vec<String> = cands
                        .into_iter()
                        .map(|c| format!("{}|{}|{}", c.label, c.detail, c.tier))
                        .collect();
                    (items, incomplete)
                };
                let r = ranked();
                let promoted = |i: Iri| r.iris.contains_key(&i);
                let best = Best {
                    typed,
                    promoted: &promoted,
                };
                assert_eq!(show(Some(best)), show(None), "{slot:?} {typed}");
            }
        }
    }

    /// Completion of `ex:p` over 40,000 described terms took 31 ms against
    /// 1.3 ms for the same list unfiltered. The work is counted, not timed: the
    /// candidates built are the few that are shown.
    #[test]
    fn typed_completion_over_forty_thousand_terms_builds_few_candidates() {
        let k = knowledge_of_widgets(40_000);
        let prefixes: PrefixMap = [("ex".to_string(), "http://example.org/".to_string())]
            .into_iter()
            .collect();
        let promoted = |_: Iri| false;
        let best = Best {
            typed: "ex:p",
            promoted: &promoted,
        };
        let cands =
            prefixed_terms_best(&k, "ex", "p", Slot::Predicate, &prefixes, Some(best)).unwrap();
        assert!(cands.len() <= LIMIT + 1, "{}", cands.len());
        let mut cands = Vec::new();
        let best = Best {
            typed: "p",
            promoted: &promoted,
        };
        described_candidates_best(&mut cands, "p", Slot::Subject, &k, &prefixes, Some(best));
        assert!(cands.len() <= LIMIT + 1, "{}", cands.len());
        // And the whole answer is the 200 items.
        let text = "@prefix ex: <http://example.org/> .\n\nex:p";
        let a = Arc::new(Analysis::new(
            "file:///w/doc.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ));
        let pos = a.position(Encoding::Utf16, text.len() as u32);
        let CompletionResponse::List(list) = complete(&a, &k, Encoding::Utf16, pos) else {
            panic!("a list")
        };
        assert_eq!(list.items.len(), LIMIT);
        assert!(list.is_incomplete);
    }
}
