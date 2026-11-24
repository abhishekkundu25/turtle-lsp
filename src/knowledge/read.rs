//! The index's read side: what the workspace says about an IRI (its
//! descriptions, types, hierarchy, labels and curation marks), the terms
//! and ontologies of a namespace, the prefix bindings, the label and
//! definition collisions, and a sorted dump of the index for tests.

use super::counts::Posting;
use super::facts::{DocFacts, Shape, folded, literal, same_text, text_key};
use super::{
    Collisions, DocId, Index, Keyed, Knowledge, Marks, Namespace, OWL_ONTOLOGY, Slot, Source, Text,
};
use crate::analysis::Analysis;
use crate::syntax::{Iri, IriKey, Span};
use crate::vocab::VOCABS;
use rustc_hash::FxHashMap;

/// One source's description of an IRI.
pub(crate) struct Description<'a> {
    pub(crate) doc: DocId,
    pub(crate) source: Source,
    pub(crate) analysis: &'a Analysis,
    /// Top-level statements with the IRI as subject, in text order.
    pub(crate) spans: &'a [Span],
}

impl Knowledge {
    fn postings(&self, index: Index) -> &FxHashMap<u64, Posting> {
        match index {
            Index::Label => &self.labels,
            Index::Definition => &self.definitions,
            Index::Comment => &self.comments,
        }
    }

    /// Every entry of `index` under `key`, with its source.
    fn keyed(&self, index: Index, key: u64) -> impl Iterator<Item = (&Slot, &Keyed)> {
        self.postings(index)
            .get(&key)
            .into_iter()
            .flat_map(Posting::ids)
            .flat_map(move |&id| {
                let s = self.slot(id);
                s.facts.under_key(index, key).map(move |k| (s, k))
            })
    }

    /// The entries of the terms with a label (or definition) under `key`,
    /// with their sources. A comment counts as a definition only
    /// for a term no workspace source gives a `skos:definition`.
    fn under(&self, text: Text, key: u64) -> Vec<(&Slot, &Keyed)> {
        let indexes: &[Index] = match text {
            Text::Label => &[Index::Label],
            Text::Definition => &[Index::Definition, Index::Comment],
        };
        let mut out = Vec::new();
        for &index in indexes {
            for (s, k) in self.keyed(index, key) {
                if self.counts(index, k.subject) {
                    out.push((s, k));
                }
            }
        }
        out
    }

    /// Whether an entry of `index` about `subject` counts for
    /// [`Knowledge::under`]: the subject is a term, and a comment is about a
    /// term no workspace source gives a `skos:definition`.
    pub(super) fn counts(&self, index: Index, subject: Iri) -> bool {
        self.marks
            .get(&subject)
            .is_some_and(|m| m.term > 0 && (index != Index::Comment || m.defined == 0))
    }

    /// The texts under `key` that two or more terms share.
    pub(super) fn collisions(&self, text: Text, key: u64) -> Collisions {
        let found = self.under(text, key);
        if found.len() < 2 {
            return Vec::new();
        }
        let mut all: Collisions = found
            .into_iter()
            .map(|(s, k)| {
                let (t, lang) = literal(s, k);
                (folded(t).collect(), lang, k.subject)
            })
            .collect();
        all.sort_unstable();
        all.dedup();
        all.chunk_by(|x, y| x.0 == y.0 && x.1 == y.1)
            .filter(|group| group.len() >= 2)
            .flatten()
            .cloned()
            .collect()
    }

    /// The terms whose label (or definition) folds to `text` in `lang` (any
    /// case; `None`: untagged), sorted, unique.
    fn sharing(&self, kind: Text, text: &str, lang: Option<&str>) -> Vec<Iri> {
        let lang = match lang.map(str::to_lowercase) {
            // No literal has a tag that was never interned.
            Some(l) => match Iri::get(&l) {
                Some(l) => Some(l),
                None => return Vec::new(),
            },
            None => None,
        };
        let mut out: Vec<Iri> = self
            .under(kind, text_key(text, lang.as_ref().map(|l| l.as_str())))
            .into_iter()
            .filter(|&(s, k)| same_text(literal(s, k), (text, lang)))
            .map(|(_, k)| k.subject)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    // ---- per-IRI facts ----

    /// Sources in a stable order: open documents first, then by URI.
    fn ordered(&self, ids: &[DocId]) -> Vec<DocId> {
        let mut v = ids.to_vec();
        v.sort_by(|x, y| {
            let (a, b) = (self.slot(*x), self.slot(*y));
            (a.source, &a.uri).cmp(&(b.source, &b.uri))
        });
        v
    }

    fn subject_docs<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<DocId> {
        self.entries
            .get(iri)
            .map(|e| self.ordered(&e.subject))
            .unwrap_or_default()
    }

    /// Where the IRI is described (is the subject of a top-level statement).
    pub(crate) fn descriptions<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Description<'_>> {
        let Some(key) = iri.iri_key() else {
            return vec![];
        };
        self.subject_docs(&key)
            .into_iter()
            .filter_map(|id| {
                let s = self.slot(id);
                s.analysis.defs.get(&key).map(|spans| Description {
                    doc: id,
                    source: s.source,
                    analysis: &s.analysis,
                    spans,
                })
            })
            .collect()
    }

    /// Described in the workspace or an external source.
    pub(crate) fn is_described<K: IriKey + ?Sized>(&self, iri: &K) -> bool {
        self.entries
            .get(iri)
            .is_some_and(|e| e.described_ws + e.described_ext > 0)
    }

    /// Union over sources, each source's types in its own order.
    pub(crate) fn types<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect(iri, |s, k| s.analysis.types.get(&k).map(Vec::as_slice))
    }

    /// Direct super classes / properties stated in the workspace, then from
    /// well-known vocabularies.
    pub(crate) fn supers<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect(iri, |s, k| s.analysis.supers.get(&k).map(Vec::as_slice));
        with_vocab(&mut v, iri, |t| &t.supers);
        v
    }

    /// Direct sub classes / properties (workspace only).
    pub(crate) fn subs<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect_objects(iri, |f, k| f.subs.get(k))
    }

    /// Resources typed with this class (workspace only).
    pub(crate) fn instances<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect_objects(iri, |f, k| f.instances.get(k))
    }

    /// Sources in which `iri` is the object of a type or sub-class/property
    /// link, with the subjects linked to it there.
    pub(crate) fn related_by_doc<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<(DocId, Vec<Iri>)> {
        let Some(key) = iri.iri_key() else {
            return vec![];
        };
        let Some(e) = self.entries.get(&key) else {
            return vec![];
        };
        self.ordered(&e.object)
            .into_iter()
            .filter_map(|id| {
                let f = &self.slot(id).facts;
                let mut v: Vec<Iri> = Vec::new();
                for s in f
                    .instances
                    .get(key)
                    .into_iter()
                    .chain(f.subs.get(key))
                    .flatten()
                {
                    if !v.contains(s) {
                        v.push(*s);
                    }
                }
                (!v.is_empty()).then_some((id, v))
            })
            .collect()
    }

    /// rdfs:domain from the workspace, then from well-known vocabularies.
    pub(crate) fn domains<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect_subject_facts(iri, |f, k| f.domain.get(k));
        with_vocab(&mut v, iri, |t| &t.domain);
        v
    }

    /// rdfs:range from the workspace, then from well-known vocabularies.
    pub(crate) fn ranges<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect_subject_facts(iri, |f, k| f.range.get(k));
        with_vocab(&mut v, iri, |t| &t.range);
        v
    }

    /// The label from the workspace: English or untagged preferred, then open
    /// documents, then by URI.
    pub(crate) fn label<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.pick(iri, |a, k| a.label_en(&k))
    }

    /// The comment from the workspace, chosen like [`Knowledge::label`].
    pub(crate) fn comment<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.pick(iri, |a, k| a.comment_en(&k))
    }

    /// A term: some workspace source types the IRI `owl:Class`,
    /// `rdfs:Class`, `owl:ObjectProperty`, `owl:DatatypeProperty`,
    /// `owl:AnnotationProperty`, `rdf:Property` or `skos:Concept`, or makes
    /// it the subject of `rdfs:subClassOf` or `rdfs:subPropertyOf`.
    pub(crate) fn is_term<K: IriKey + ?Sized>(&self, iri: &K) -> bool {
        self.marks.get(iri).is_some_and(|m| m.term > 0)
    }

    /// What sort of term the workspace makes `iri`, as [`TERM_CLASS`](super::TERM_CLASS),
    /// [`TERM_PROPERTY`](super::TERM_PROPERTY) and [`TERM_CONCEPT`](super::TERM_CONCEPT) bits: from the types and the
    /// `rdfs:subClassOf` and `rdfs:subPropertyOf` statements of workspace
    /// sources (external ones say nothing).
    pub(crate) fn term_kind<K: IriKey + ?Sized>(&self, iri: &K) -> u8 {
        let Some(key) = iri.iri_key() else {
            return 0;
        };
        self.curated(key).fold(0, |bits, (_, c)| bits | c.kind)
    }

    /// The terms with an `rdfs:label` or `skos:prefLabel` that equals
    /// `label` trimmed and ignoring case, in language `lang` (any case;
    /// `None`: untagged). Sorted, unique.
    pub(crate) fn labelled(&self, label: &str, lang: Option<&str>) -> Vec<Iri> {
        self.sharing(Text::Label, label, lang)
    }

    /// The terms whose definition equals `definition` as
    /// [`Knowledge::labelled`] compares: a term's definition is its
    /// `skos:definition`, or its `rdfs:comment` when no workspace source
    /// gives it a `skos:definition`.
    pub(crate) fn defined_as(&self, definition: &str, lang: Option<&str>) -> Vec<Iri> {
        self.sharing(Text::Definition, definition, lang)
    }

    /// [`Knowledge::deprecated_here`] is true of some IRI.
    pub(crate) fn has_deprecations(&self) -> bool {
        self.deprecations > 0
    }

    /// Some workspace source says `owl:deprecated` with a boolean true (`true`,
    /// `"true"^^xsd:boolean`, JSON-LD `true`), and none says false.
    pub(crate) fn deprecated_here<K: IriKey + ?Sized>(&self, iri: &K) -> bool {
        self.deprecations > 0 && self.marks.get(iri).is_some_and(Marks::deprecated)
    }

    /// Used in predicate position anywhere.
    pub(crate) fn used_as_predicate<K: IriKey + ?Sized>(&self, iri: &K) -> bool {
        self.entries.get(iri).is_some_and(|e| e.predicate > 0)
    }

    /// SHACL shapes that target this class, with the source they are in.
    pub(crate) fn shapes_targeting<K: IriKey + ?Sized>(&self, class: &K) -> Vec<(DocId, &Shape)> {
        let Some(key) = class.iri_key() else {
            return vec![];
        };
        let Some(e) = self.entries.get(&key) else {
            return vec![];
        };
        let mut out = Vec::new();
        for id in self.ordered(&e.object) {
            let f = &self.slot(id).facts;
            for &i in f.shapes_by_target.get(&key).into_iter().flatten() {
                out.push((id, &f.shapes[i as usize]));
            }
        }
        out
    }

    /// Sources that mention the IRI anywhere, in stable order.
    pub(crate) fn docs_referencing<K: IriKey + ?Sized>(
        &self,
        iri: &K,
    ) -> impl Iterator<Item = (DocId, Source, &Analysis)> {
        self.entries
            .get(iri)
            .map(|e| self.ordered(&e.refs))
            .unwrap_or_default()
            .into_iter()
            .map(|id| {
                let s = self.slot(id);
                (id, s.source, &*s.analysis)
            })
    }

    // ---- workspace-wide ----

    /// Every IRI some source mentions in namespace `ns`. For a namespace that
    /// does not end in `#`, `/` or `:`, every IRI starting with it.
    pub(crate) fn terms_in(&self, ns: &str) -> Vec<Iri> {
        if ns.is_empty() || ns.ends_with(['#', '/', ':']) {
            return Iri::get(ns)
                .and_then(|k| self.namespaces.get(&k))
                .map(|n| n.terms.iter().copied().collect())
                .unwrap_or_default();
        }
        self.namespaces
            .iter()
            .filter(|(k, _)| k.starts_with(ns) || ns.starts_with(k.as_str()))
            .flat_map(|(_, n)| n.terms.iter().copied())
            .filter(|t| t.len() > ns.len() && t.starts_with(ns))
            .collect()
    }

    /// Every IRI described by some source.
    pub(crate) fn described_terms(&self) -> impl Iterator<Item = Iri> + '_ {
        self.entries
            .iter()
            .filter(|(_, e)| e.described_ws + e.described_ext > 0)
            .map(|(i, _)| *i)
    }

    /// Ontologies (`<iri> a owl:Ontology`) whose describing source, workspace
    /// or external, also describes terms in namespace `ns`, with the URI of
    /// that source; most such terms first. Well-known vocabularies are not
    /// sources, so never appear.
    pub(crate) fn ontologies_describing(&self, ns: &str) -> Vec<(Iri, &str)> {
        if ns.is_empty() {
            return vec![];
        }
        let mut out: Vec<(Iri, &str, usize)> = Vec::new();
        for o in self.instances(OWL_ONTOLOGY) {
            for d in self.descriptions(&o) {
                let n = d
                    .analysis
                    .defs
                    .keys()
                    .filter(|t| **t != o && t.len() > ns.len() && t.starts_with(ns))
                    .count();
                if n > 0 && !out.iter().any(|(x, ..)| *x == o) {
                    out.push((o, d.analysis.uri.as_str(), n));
                }
            }
        }
        out.sort_by(|x, y| y.2.cmp(&x.2).then_with(|| x.0.cmp(&y.0)));
        out.into_iter().map(|(o, u, _)| (o, u)).collect()
    }

    /// Namespaces bound to prefix `name` across workspace files, most files first.
    pub(crate) fn bindings(&self, name: &str) -> Vec<(Iri, u32)> {
        let Some(by_ns) = Iri::get(name).and_then(|n| self.bindings.get(&n)) else {
            return vec![];
        };
        let mut v: Vec<(Iri, u32)> = by_ns.iter().map(|(ns, n)| (*ns, *n)).collect();
        v.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
        v
    }

    /// An "own" namespace: not a well-known vocabulary, at least 3 terms in
    /// it described in the workspace, and at least 80% of the distinct terms
    /// the workspace references in it described.
    pub(crate) fn is_own_namespace(&self, ns: &str) -> bool {
        if VOCABS.by_namespace(ns).is_some() {
            return false;
        }
        Iri::get(ns)
            .and_then(|k| self.namespaces.get(&k))
            .is_some_and(Namespace::is_own)
    }

    /// Whether the project defines terms in `ns` (a looser test than
    /// [`Knowledge::is_own_namespace`]): not a well-known vocabulary, at least
    /// three terms described and at least 20% of those referenced.
    pub(crate) fn is_project_namespace(&self, ns: &str) -> bool {
        if VOCABS.by_namespace(ns).is_some() {
            return false;
        }
        Iri::get(ns)
            .and_then(|k| self.namespaces.get(&k))
            .is_some_and(Namespace::is_project)
    }

    // ---- helpers ----

    fn collect<K: IriKey + ?Sized>(
        &self,
        iri: &K,
        get: impl Fn(&Slot, Iri) -> Option<&[Iri]>,
    ) -> Vec<Iri> {
        let Some(key) = iri.iri_key() else {
            return vec![];
        };
        let mut out: Vec<Iri> = Vec::new();
        for id in self.subject_docs(&key) {
            for i in get(self.slot(id), key).into_iter().flatten() {
                if !out.contains(i) {
                    out.push(*i);
                }
            }
        }
        out
    }

    fn collect_subject_facts<K: IriKey + ?Sized>(
        &self,
        iri: &K,
        get: impl Fn(&DocFacts, Iri) -> Option<&[Iri]>,
    ) -> Vec<Iri> {
        self.collect(iri, |s, k| get(&s.facts, k))
    }

    fn collect_objects<K: IriKey + ?Sized>(
        &self,
        iri: &K,
        get: impl Fn(&DocFacts, Iri) -> Option<&[Iri]>,
    ) -> Vec<Iri> {
        let Some(key) = iri.iri_key() else {
            return vec![];
        };
        let Some(e) = self.entries.get(&key) else {
            return vec![];
        };
        let mut out: Vec<Iri> = Vec::new();
        // A class can have tens of thousands of instances: dedup with a set.
        let mut seen = rustc_hash::FxHashSet::default();
        for id in self.ordered(&e.object) {
            for i in get(&self.slot(id).facts, key).into_iter().flatten() {
                if seen.insert(*i) {
                    out.push(*i);
                }
            }
        }
        out
    }

    fn pick<'a, K: IriKey + ?Sized>(
        &'a self,
        iri: &K,
        get: impl Fn(&'a Analysis, Iri) -> Option<(&'a str, bool)>,
    ) -> Option<&'a str> {
        let key = iri.iri_key()?;
        let mut best: Option<(&str, bool)> = None;
        for id in self.subject_docs(&key) {
            if let Some((text, en)) = get(&self.slot(id).analysis, key) {
                if en {
                    return Some(text);
                }
                best = best.or(Some((text, en)));
            }
        }
        best.map(|b| b.0)
    }

    /// Canonical dump of the whole index, independent of doc ids and of the
    /// order in which sources were added. For consistency tests.
    pub(crate) fn snapshot(&self) -> Vec<String> {
        let uris = |ids: &[DocId]| {
            let mut v: Vec<String> = ids
                .iter()
                .map(|i| {
                    let s = self.slot(*i);
                    format!("{:?}:{}", s.source, s.uri)
                })
                .collect();
            v.sort();
            v.join(",")
        };
        let mut out: Vec<String> = self
            .entries
            .iter()
            .map(|(iri, e)| {
                format!(
                    "E {iri} ns={:?} s=[{}] o=[{}] r=[{}] rws={} d={}/{} p={}",
                    e.ns,
                    uris(&e.subject),
                    uris(&e.object),
                    uris(&e.refs),
                    e.refs_ws,
                    e.described_ws,
                    e.described_ext,
                    e.predicate,
                )
            })
            .collect();
        out.extend(self.marks.iter().map(|(iri, m)| format!("M {iri} {m:?}")));
        if self.terms + self.deprecations > 0 {
            out.push(format!("C {} {}", self.terms, self.deprecations));
        }
        for (ns, n) in &self.namespaces {
            let mut terms: Vec<&str> = n.terms.iter().map(|t| t.as_str()).collect();
            terms.sort();
            out.push(format!(
                "N {ns} d={} r={} t={terms:?}",
                n.described, n.referenced
            ));
        }
        for (name, by_ns) in &self.bindings {
            for (ns, n) in by_ns {
                out.push(format!("B {name} {ns} {n}"));
            }
        }
        for index in Index::ALL {
            for (key, docs) in self.postings(index) {
                out.push(format!("T {index:?} {key} [{}]", uris(docs.ids())));
            }
        }
        out.sort();
        out
    }
}

/// Append the well-known vocabulary's facts for `iri`, without duplicates.
fn with_vocab<K: IriKey + ?Sized>(
    out: &mut Vec<Iri>,
    iri: &K,
    field: impl Fn(&crate::vocab::Term) -> &[String],
) {
    let Some(key) = iri.iri_key() else { return };
    let Some((v, local)) = VOCABS.lookup(&key) else {
        return;
    };
    for s in v.term(local).map(field).unwrap_or(&[]) {
        let i = Iri::intern(s);
        if !out.contains(&i) {
            out.push(i);
        }
    }
}
