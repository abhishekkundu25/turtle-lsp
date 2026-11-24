//! The workspace knowledge index: what the whole workspace says about each
//! IRI, merged from every source (open documents, files on disk, and
//! read-only external sources such as imported ontologies).
//!
//! Each source's contribution ([`DocFacts`]) is computed once from its
//! analysis and kept, so replacing or removing a source subtracts exactly what
//! it added. Replacing a source walks its old and new contributions side by
//! side, sorted, and changes the index only where they differ: a keystroke
//! costs a pass over that document's facts and a lookup per change, never
//! O(workspace). Each change reports what it changed for other documents'
//! checks ([`Changes`]).
//! Per IRI the index keeps posting lists of the sources that say something
//! about it; the facts themselves are read back from those sources' analyses
//! and facts, so the index stays small.
//!
//! The curation facts (which IRIs are terms, their labels and definitions,
//! their own `owl:deprecated`) come from workspace sources only. Labels and
//! definitions are indexed by a hash of their folded text: per key the index
//! keeps the sources that use it, and each source keeps its own entries.
//! What the curation checks read of other documents about a term or an
//! ontology (its kind, whether it has a label, a comment or each header
//! property, which documents declare it) each source keeps as a [`Curated`]
//! summary per IRI.

use crate::analysis::Analysis;
use crate::syntax::{Iri, IriKey, IriMap, Span};
use crate::vocab::VOCABS;
use rustc_hash::{FxHashMap, FxHashSet};
use std::hash::Hash;
use std::sync::Arc;

mod counts;
use counts::{
    ByIri, Counts, Ided, Posting, counted, diff, differing, entry, merge, ns_step, repost, step,
    summed, toggle,
};
mod facts;
use facts::{Curated, TermState, folded, literal, same_text};
pub(crate) use facts::{
    DocFacts, PropertyConstraint, Shape, TERM_CLASS, TERM_CONCEPT, TERM_PROPERTY,
    header_properties, text_key,
};
mod update;

/// A source in the index. Ids are reused after removal.
pub(crate) type DocId = u32;

/// Where a source comes from. Open documents win over their disk copy (the
/// workspace registers one or the other per URI); external sources contribute
/// facts but are never diagnosed or edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Open,
    Disk,
    External,
}

/// One source's description of an IRI.
pub(crate) struct Description<'a> {
    pub(crate) doc: DocId,
    pub(crate) source: Source,
    pub(crate) analysis: &'a Analysis,
    /// Top-level statements with the IRI as subject, in text order.
    pub(crate) spans: &'a [Span],
}

/// One literal that names or defines an IRI subject.
#[derive(Clone, Copy, Debug)]
struct Keyed {
    /// [`text_key`] of the literal.
    key: u64,
    subject: Iri,
    /// Where its triple is: the statement, and how many triples after the
    /// statement's first ([`DocFacts::firsts`]). An edit before the
    /// statement that adds no statement leaves it as it was.
    statement: u32,
    offset: u32,
}

/// The labels, definitions and comments, each indexed by text key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Index {
    Label,
    Definition,
    Comment,
}

impl Index {
    const ALL: [Index; 3] = [Index::Label, Index::Definition, Index::Comment];

    fn of(self, f: &DocFacts) -> &[Keyed] {
        match self {
            Index::Label => &f.labels,
            Index::Definition => &f.definitions,
            Index::Comment => &f.comments,
        }
    }

    /// What a key of this index collides as: a definition falls back to the
    /// comment, so the two share one.
    fn text(self) -> Text {
        match self {
            Index::Label => Text::Label,
            Index::Definition | Index::Comment => Text::Definition,
        }
    }
}

/// What two terms can share: a label, or a definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Text {
    Label,
    Definition,
}

/// Terms that share a text under one key: (folded text, language, term),
/// sorted.
type Collisions = Vec<(String, Option<Iri>, Iri)>;

struct Slot {
    uri: String,
    source: Source,
    analysis: Arc<Analysis>,
    facts: DocFacts,
}

#[derive(Default)]
struct Entry {
    /// Namespace of the IRI (see [`namespace_of`]).
    ns: Option<Iri>,
    subject: Vec<DocId>,
    object: Vec<DocId>,
    refs: Vec<DocId>,
    /// Workspace (non-external) sources among `refs`.
    refs_ws: u32,
    described_ws: u32,
    described_ext: u32,
    predicate: u32,
}

impl Entry {
    fn is_empty(&self) -> bool {
        self.subject.is_empty()
            && self.object.is_empty()
            && self.refs.is_empty()
            && self.described_ws == 0
            && self.described_ext == 0
            && self.predicate == 0
    }
}

/// The workspace sources that say of an IRI what only the curation checks
/// read: kept apart from its [`Entry`], as few IRIs have any.
#[derive(Default, Debug)]
struct Marks {
    /// Sources that make the IRI a term, give it a `skos:definition`, say
    /// `owl:deprecated true`, say it `false`.
    term: u32,
    defined: u32,
    deprecated: u32,
    undeprecated: u32,
    /// Sources whose [`Curated`] summary types it `owl:Ontology`.
    ontology: u32,
}

impl Marks {
    fn is_empty(&self) -> bool {
        self.term == 0
            && self.defined == 0
            && self.deprecated == 0
            && self.undeprecated == 0
            && self.ontology == 0
    }

    fn deprecated(&self) -> bool {
        self.deprecated > 0 && self.undeprecated == 0
    }
}

#[derive(Default)]
struct Namespace {
    /// Every IRI in this namespace that some source mentions.
    terms: FxHashSet<Iri>,
    /// Distinct terms the workspace (not external sources) describes / references.
    described: u32,
    referenced: u32,
}

/// What one change of the index (a source added, replaced or removed)
/// changed of what other documents' checks read: the reasons
/// [`Knowledge::described_generation`] moves, item by item. Empty exactly
/// when it does not move. Each list is sorted and unique.
#[derive(Default, Debug, PartialEq, Eq)]
pub(crate) struct Changes {
    /// IRIs that became or stopped being described, or a term; terms that
    /// gained their first `skos:definition` or lost their last; IRIs that
    /// became or stopped being deprecated here; IRIs whose [`TermState`]
    /// changed. Sorted by [`Iri::id`].
    pub(crate) iris: Vec<Iri>,
    /// Text keys under which the source's entries changed and the terms
    /// that share a label or definition changed (see [`Knowledge::labelled`]
    /// and [`Knowledge::defined_as`]). A key whose terms change only because
    /// an IRI became or stopped being a term, or a term's comments started
    /// or stopped standing for its definition, is not listed: the IRI is.
    pub(crate) texts: Vec<(Text, u64)>,
    /// Namespaces that became or stopped being own namespaces
    /// ([`Knowledge::is_own_namespace`]). Sorted by [`Iri::id`].
    pub(crate) namespaces: Vec<Iri>,
    /// Prefix names a namespace was bound to for the first time, or for the
    /// last ([`Knowledge::bindings`]). Sorted by [`Iri::id`].
    pub(crate) prefixes: Vec<Iri>,
}

impl Changes {
    fn clear(&mut self) {
        self.iris.clear();
        self.texts.clear();
        self.namespaces.clear();
        self.prefixes.clear();
    }

    fn is_empty(&self) -> bool {
        self.iris.is_empty()
            && self.texts.is_empty()
            && self.namespaces.is_empty()
            && self.prefixes.is_empty()
    }

    /// Sort and dedup each list.
    fn settle(&mut self) {
        for v in [&mut self.iris, &mut self.namespaces, &mut self.prefixes] {
            // Stable: each step of a change notes its IRIs in order, which
            // it finds as runs.
            v.sort_by_key(|i| i.id());
            v.dedup();
        }
        self.texts.sort_unstable();
        self.texts.dedup();
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Key {
    Workspace(String),
    External(String),
}

#[derive(Default)]
pub struct Knowledge {
    slots: Vec<Option<Slot>>,
    free: Vec<DocId>,
    by_key: FxHashMap<Key, DocId>,
    entries: IriMap<Entry>,
    marks: IriMap<Marks>,
    namespaces: FxHashMap<Iri, Namespace>,
    /// Prefix name -> namespace -> number of workspace files.
    bindings: FxHashMap<Iri, FxHashMap<Iri, u32>>,
    generation: u64,
    /// See [`Knowledge::described_generation`].
    described_generation: u64,
    /// What the last change changed: see [`Changes`].
    changes: Changes,
    /// Text key -> workspace sources with an entry under it, per [`Index`].
    labels: FxHashMap<u64, Posting>,
    definitions: FxHashMap<u64, Posting>,
    comments: FxHashMap<u64, Posting>,
    /// IRIs [`Knowledge::deprecated_here`] is true of: while there are none,
    /// it answers without a lookup.
    deprecations: u32,
    /// IRIs [`Knowledge::is_term`] is true of.
    terms: u32,
}

pub(crate) const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";

/// The namespace part of an IRI: up to and including its last `#`, `/` or `:`,
/// or the longer-named well-known vocabulary namespace it starts with (see
/// [`crate::vocab::split_namespace`]).
pub(crate) fn namespace_of(iri: &str) -> &str {
    crate::vocab::split_namespace(iri).0
}

/// The own-namespace rule: at least this many described terms...
const OWN_MIN_DESCRIBED: u32 = 3;
/// ...and at least this percentage of the referenced terms described. Tuned
/// on a real-world corpus of example fragments: at 20% or 50% a namespace whose
/// ontology lives outside the workspace, or an instance namespace, floods the
/// lint; at 80% only namespaces the workspace (nearly) fully describes count.
const OWN_MIN_SHARE_PERCENT: u32 = 80;
/// A looser bar for "the project defines terms here" (SHACL references to
/// classes and shapes): stays quiet for namespaces the workspace only
/// references, but still covers an ontology that is partly in the workspace.
const PROJECT_MIN_SHARE_PERCENT: u32 = 20;

impl Knowledge {
    fn slot(&self, id: DocId) -> &Slot {
        self.slots[id as usize].as_ref().expect("live doc id")
    }

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
    fn counts(&self, index: Index, subject: Iri) -> bool {
        self.marks
            .get(&subject)
            .is_some_and(|m| m.term > 0 && (index != Index::Comment || m.defined == 0))
    }

    /// The texts under `key` that two or more terms share.
    fn collisions(&self, text: Text, key: u64) -> Collisions {
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

    /// What sort of term the workspace makes `iri`, as [`TERM_CLASS`],
    /// [`TERM_PROPERTY`] and [`TERM_CONCEPT`] bits: from the types and the
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

#[cfg(test)]
mod tests;
