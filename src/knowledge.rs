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
use crate::shacl::vocab;
use crate::syntax::{self, BlankId, Iri, IriKey, IriMap, Node, Role, Span};
use crate::vocab::VOCABS;
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

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

/// A SHACL shape that targets classes: what completion ranks a typed
/// subject's properties by. It is a shape of the document's SHACL model
/// ([`crate::shacl::model`], which the lint and the validator read) that
/// names a target class: a shape with `sh:targetClass` IRIs, or an IRI shape
/// typed both `sh:NodeShape` and `rdfs:Class` / `owl:Class` (an implicit
/// class target). This is narrower than the model: the other targets
/// (`sh:targetNode`, `sh:targetSubjectsOf`, `sh:targetObjectsOf`,
/// SHACL-SPARQL `sh:target`) name no class, and a shape without a target,
/// such as one reached only through `sh:node` or `sh:property`, applies to
/// no class. The shape's kind is not checked: a property shape with
/// `sh:targetClass` counts too.
///
/// [`DocFacts`] finds these shapes in one pass over the parse at each edit,
/// not through the model: a graph of the document plus its model costs two
/// to four times as much as that pass (owner decision 8 of the wave-3
/// audit). A test checks that both find the same shapes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shape {
    /// The shape's IRI; `None` for a blank-node shape.
    pub(crate) node: Option<Iri>,
    /// The statement that describes the shape.
    pub(crate) span: Span,
    pub(crate) targets: Vec<Iri>,
    pub(crate) properties: Vec<PropertyConstraint>,
}

/// One `sh:property` of a [`Shape`] whose value is an IRI or a blank node.
/// `class` keeps every `sh:class`; each other parameter is the last value
/// written for it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PropertyConstraint {
    /// `sh:path` when it is a single predicate IRI (the last such one).
    pub(crate) path: Option<Iri>,
    pub(crate) min_count: Option<u32>,
    pub(crate) max_count: Option<u32>,
    pub(crate) class: Vec<Iri>,
    pub(crate) datatype: Option<Iri>,
    /// The `sh:in` list, if any.
    pub(crate) in_values: Option<Vec<Node>>,
    /// The property shape (the object of `sh:property`).
    pub(crate) span: Span,
}

/// One source's description of an IRI.
pub(crate) struct Description<'a> {
    pub(crate) doc: DocId,
    pub(crate) source: Source,
    pub(crate) analysis: &'a Analysis,
    /// Top-level statements with the IRI as subject, in text order.
    pub(crate) spans: &'a [Span],
}

/// What one source contributes beyond its analysis' own indexes.
#[derive(Default, Debug)]
pub(crate) struct DocFacts {
    /// Property -> its domains and ranges stated here.
    domain: ByIri,
    range: ByIri,
    /// Super term -> direct sub terms stated here.
    subs: ByIri,
    /// Class -> resources typed with it here.
    instances: ByIri,
    shapes: Vec<Shape>,
    /// Target class -> indexes into `shapes`.
    shapes_by_target: IriMap<Vec<u32>>,
    /// What this source contributes to each IRI's index entry, by role: per
    /// IRI, how many of its facts give it that role, sorted by [`Iri::id`]
    /// (see [`Counts`]). Replacing a source changes the index only where an
    /// IRI's count moves between zero and non-zero.
    ///
    /// The subject of a [`Curated`] summary (which every subject of a
    /// top-level statement, type or super link has) or of a domain or range
    /// link: one per summary and link.
    subjects: Counts<Iri>,
    /// The object of a type, super or target link: one per link.
    objects: Counts<Iri>,
    /// Used in predicate position: one per use.
    predicates: Counts<Iri>,
    /// Mentioned anywhere: one per reference.
    refs: Counts<Iri>,
    /// The subject of a top-level statement, or the name of a TriG graph
    /// block: one per statement or block.
    defs: Counts<Iri>,
    /// (prefix name, namespace) in effect and used or declared here: one per
    /// declaration, and one for a namespace supplied from outside and used.
    bindings: Counts<(Iri, Iri)>,
    /// IRIs this source makes terms, one per triple: see [`Knowledge::is_term`].
    terms: Counts<Iri>,
    /// The first triple of each statement, so that [`Keyed`] positions
    /// can be relative to their statement: kept only when there are
    /// labels, definitions or comments.
    firsts: Box<[u32]>,
    /// `rdfs:label` and `skos:prefLabel` of IRI subjects, in text order.
    labels: Vec<Keyed>,
    /// `skos:definition` of IRI subjects, in text order.
    definitions: Vec<Keyed>,
    /// `rdfs:comment` of IRI subjects, in text order.
    comments: Vec<Keyed>,
    /// Per [`Index`], its entries' positions ordered by key: made on the
    /// first lookup by key ([`DocFacts::under_key`]), which most documents
    /// never get.
    by_key: [OnceLock<Box<[u32]>>; 3],
    /// Lookups by key that scanned instead ([`DocFacts::under_key`]).
    scans: AtomicU32,
    /// Subjects of `definitions`, one per definition.
    defined: Counts<Iri>,
    /// `owl:deprecated` with a boolean value (unique, by [`Iri::id`]).
    deprecated: Vec<(Iri, bool)>,
    /// What this source says about each IRI subject that the curation
    /// checks read, sorted by [`Iri::id`]; IRIs it says none of it about are
    /// left out.
    curated: Vec<(Iri, Curated)>,
}

/// What one workspace source says about an IRI that the curation checks of
/// other documents read ([`Knowledge::term_kind`], [`TermState`]).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Curated {
    /// [`TERM_CLASS`], [`TERM_PROPERTY`] and [`TERM_CONCEPT`] bits.
    kind: u8,
    /// Gives it a type or an IRI super class or property
    /// ([`Analysis::types`], [`Analysis::supers`]).
    declares: bool,
    /// The subject of a top-level statement ([`Analysis::defs`]).
    describes: bool,
    /// Types it `owl:Ontology`.
    ontology: bool,
    /// Gives it a label ([`Analysis::label`]), a comment
    /// ([`Analysis::comment_en`]).
    labelled: bool,
    commented: bool,
    /// Bit `i`: gives it a property of group `i` of [`header_properties`].
    header: u8,
}

impl Curated {
    /// What `other`, read from other triples about the same IRI, adds.
    fn add(&mut self, other: Curated) {
        self.kind |= other.kind;
        self.declares |= other.declares;
        self.describes |= other.describes;
        self.ontology |= other.ontology;
        self.labelled |= other.labelled;
        self.commented |= other.commented;
        self.header |= other.header;
    }
}

/// The term kind bits of [`Knowledge::term_kind`]: a class (typed
/// `owl:Class` or `rdfs:Class`, or the subject of `rdfs:subClassOf`), a
/// property (one of the four property types, or the subject of
/// `rdfs:subPropertyOf`), a concept (`skos:Concept`).
pub(crate) const TERM_CLASS: u8 = 1;
pub(crate) const TERM_PROPERTY: u8 = 2;
pub(crate) const TERM_CONCEPT: u8 = 4;

/// What the curation checks of other documents read about a term or an
/// ontology, from every workspace source: compared before and after a
/// change to decide whether it bumps [`Knowledge::described_generation`].
/// Facts that no check reads (a non-term's kind, a non-ontology's header)
/// are left at their default.
#[derive(Default, Debug, PartialEq, Eq)]
struct TermState {
    kind: u8,
    labelled: bool,
    commented: bool,
    /// The document that reports `missing-label` and `missing-definition`:
    /// the first by URI of those that describe and declare the term, else of
    /// those that describe it.
    reporter: Option<String>,
    /// The document that reports the `missing-ontology-*` checks: the first
    /// by URI of those that describe the IRI and type it `owl:Ontology`.
    ontology: Option<String>,
    header: u8,
}

/// Per item, a count above zero, sorted by the item's id ([`Ided`]).
type Counts<T> = Vec<(T, u32)>;

/// IRIs grouped under IRIs: per key IRI, sorted by [`Iri::id`], its values
/// in text order. Two allocations, where a map of vectors makes one per key
/// to build and to drop.
#[derive(Default, Debug)]
struct ByIri {
    /// Each key, with the end of its values in `values`.
    keys: Vec<(Iri, u32)>,
    values: Vec<Iri>,
}

impl ByIri {
    /// From (key, value) pairs in text order.
    fn new(mut pairs: Vec<(Iri, Iri)>) -> ByIri {
        // Stable, so each key's values stay in text order; keys mostly come
        // in the order they were first interned in, which it finds in one
        // pass.
        pairs.sort_by_key(|p| p.0.id());
        let mut keys: Vec<(Iri, u32)> = Vec::new();
        let values = pairs
            .iter()
            .enumerate()
            .map(|(i, &(k, v))| {
                match keys.last_mut() {
                    Some(last) if last.0 == k => last.1 += 1,
                    _ => keys.push((k, i as u32 + 1)),
                }
                v
            })
            .collect();
        ByIri { keys, values }
    }

    fn get(&self, key: Iri) -> Option<&[Iri]> {
        let at = self
            .keys
            .binary_search_by_key(&key.id(), |k| k.0.id())
            .ok()?;
        let start = at.checked_sub(1).map_or(0, |p| self.keys[p].1);
        Some(&self.values[start as usize..self.keys[at].1 as usize])
    }

    /// Each key with how many values it has.
    fn counts(&self) -> Counts<Iri> {
        let mut start = 0;
        self.keys
            .iter()
            .map(|&(k, end)| {
                let n = end - start;
                start = end;
                (k, n)
            })
            .collect()
    }
}

/// The two [`Counts`] in one, summing the counts of an item both have.
fn summed<T: Ided>(xs: &[(T, u32)], ys: &[(T, u32)]) -> Counts<T> {
    let mut out = Vec::with_capacity(xs.len().max(ys.len()));
    merge(
        xs,
        ys,
        |c| c.0,
        |x, y| {
            let item = x.or(y).expect("one of them").0;
            out.push((item, x.map_or(0, |c| c.1) + y.map_or(0, |c| c.1)));
        },
    );
    out
}

/// What [`Counts`] and [`merge`] order items by.
trait Ided: Copy + PartialEq {
    fn ided(self) -> u64;
}

impl Ided for Iri {
    fn ided(self) -> u64 {
        u64::from(self.id())
    }
}

impl Ided for (Iri, Iri) {
    fn ided(self) -> u64 {
        u64::from(self.0.id()) << 32 | u64::from(self.1.id())
    }
}

impl Ided for (Iri, bool) {
    fn ided(self) -> u64 {
        u64::from(self.0.id()) << 1 | u64::from(self.1)
    }
}

/// `v` as [`Counts`]: sorted, one per item with the sum of its counts. The
/// sort is stable, so items that come in the order they were first interned
/// in (text order, mostly) are found in one pass.
fn counted<T: Ided>(mut v: Vec<(T, u32)>) -> Counts<T> {
    v.sort_by_key(|x| x.0.ided());
    v.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            kept.1 += later.1;
        }
        same
    });
    v
}

/// Walk two lists sorted by [`Ided`] together, one item at most per id in
/// each: `f(x, y)` for every id either holds, with the item of each.
fn merge<'a, T, I: Ided>(
    xs: &'a [T],
    ys: &'a [T],
    id: impl Fn(&T) -> I,
    mut f: impl FnMut(Option<&'a T>, Option<&'a T>),
) {
    let (mut i, mut j) = (0, 0);
    loop {
        match (xs.get(i), ys.get(j)) {
            (Some(x), Some(y)) => {
                let (a, b) = (id(x).ided(), id(y).ided());
                if a == b {
                    f(Some(x), Some(y));
                    (i, j) = (i + 1, j + 1);
                } else if a < b {
                    f(Some(x), None);
                    i += 1;
                } else {
                    f(None, Some(y));
                    j += 1;
                }
            }
            (Some(x), None) => {
                f(Some(x), None);
                i += 1;
            }
            (None, Some(y)) => {
                f(None, Some(y));
                j += 1;
            }
            (None, None) => break,
        }
    }
}

/// `f(item, true)` for each item `ys` counts and `xs` does not, `f(item,
/// false)` for each `xs` counts and `ys` does not: where a count moves
/// between zero and non-zero.
fn diff<T: Ided>(xs: &[(T, u32)], ys: &[(T, u32)], mut f: impl FnMut(T, bool)) {
    if xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| x.0 == y.0) {
        return;
    }
    merge(
        xs,
        ys,
        |c| c.0,
        |x, y| match (x, y) {
            (Some(x), None) => f(x.0, false),
            (None, Some(y)) => f(y.0, true),
            _ => {}
        },
    );
}

/// The positions of the entries of `xs` and of `ys` that differ, as `same`
/// compares them: past the longest run both start with and before the
/// longest both end with, and, when what is left is as long in both, of
/// the pairs that are not the same (several edits far apart change little
/// between them).
fn differing(
    xs: &[Keyed],
    ys: &[Keyed],
    same: impl Fn(&Keyed, &Keyed) -> bool,
) -> (Vec<usize>, Vec<usize>) {
    let head = xs.iter().zip(ys).take_while(|(p, q)| same(p, q)).count();
    let tail = xs[head..]
        .iter()
        .rev()
        .zip(ys[head..].iter().rev())
        .take_while(|(p, q)| same(p, q))
        .count();
    let (xm, ym) = (head..xs.len() - tail, head..ys.len() - tail);
    if xm.len() == ym.len() {
        let pairs = xm.filter(|&i| !same(&xs[i], &ys[i]));
        let at: Vec<usize> = pairs.collect();
        (at.clone(), at)
    } else {
        (xm.collect(), ym.collect())
    }
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

/// The predicates that state a property's domain or range: RDFS's, and
/// schema.org's `domainIncludes` / `rangeIncludes` under both schemes.
struct Links {
    domain: [Iri; 3],
    range: [Iri; 3],
}

fn links() -> &'static Links {
    static L: OnceLock<Links> = OnceLock::new();
    L.get_or_init(|| {
        let v = vocab::get();
        let i = Iri::intern;
        Links {
            domain: [
                v.rdfs_domain,
                i("https://schema.org/domainIncludes"),
                i("http://schema.org/domainIncludes"),
            ],
            range: [
                v.rdfs_range,
                i("https://schema.org/rangeIncludes"),
                i("http://schema.org/rangeIncludes"),
            ],
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Subj {
    Iri(Iri),
    Blank(BlankId),
}

/// The IRIs the curation facts read.
struct Curation {
    /// A subject typed with one of these is a term.
    term_types: [Iri; 7],
    labels: [Iri; 2],
    definition: Iri,
    comment: Iri,
    deprecated: Iri,
    /// The [`TERM_CLASS`], [`TERM_PROPERTY`] and [`TERM_CONCEPT`] types.
    class_types: [Iri; 2],
    property_types: [Iri; 4],
    concept: Iri,
    ontology: Iri,
    header: [[Iri; 3]; 3],
    /// Every predicate [`DocFacts::curation`] and [`curate`] read: a triple
    /// with another one says nothing to them.
    read: FxHashSet<Iri>,
}

/// The properties that give an ontology its title, description and licence,
/// a group each: what the `missing-ontology-*` checks look for.
pub(crate) fn header_properties() -> &'static [[Iri; 3]; 3] {
    &curation().header
}

fn curation() -> &'static Curation {
    static C: OnceLock<Curation> = OnceLock::new();
    C.get_or_init(|| {
        let i = Iri::intern;
        let owl = |l: &str| i(&format!("http://www.w3.org/2002/07/owl#{l}"));
        let rdfs = |l: &str| i(&format!("http://www.w3.org/2000/01/rdf-schema#{l}"));
        let skos = |l: &str| i(&format!("http://www.w3.org/2004/02/skos/core#{l}"));
        let dcterms = |l: &str| i(&format!("http://purl.org/dc/terms/{l}"));
        let dc = |l: &str| i(&format!("http://purl.org/dc/elements/1.1/{l}"));
        let mut c = Curation {
            term_types: [
                owl("Class"),
                rdfs("Class"),
                owl("ObjectProperty"),
                owl("DatatypeProperty"),
                owl("AnnotationProperty"),
                i("http://www.w3.org/1999/02/22-rdf-syntax-ns#Property"),
                skos("Concept"),
            ],
            labels: [rdfs("label"), skos("prefLabel")],
            definition: skos("definition"),
            comment: rdfs("comment"),
            deprecated: owl("deprecated"),
            class_types: [owl("Class"), rdfs("Class")],
            property_types: [
                owl("ObjectProperty"),
                owl("DatatypeProperty"),
                owl("AnnotationProperty"),
                i("http://www.w3.org/1999/02/22-rdf-syntax-ns#Property"),
            ],
            concept: skos("Concept"),
            ontology: i(OWL_ONTOLOGY),
            header: [
                [dcterms("title"), dc("title"), rdfs("label")],
                [dcterms("description"), dc("description"), rdfs("comment")],
                [
                    dcterms("license"),
                    i("http://creativecommons.org/ns#license"),
                    i("http://web.resource.org/cc/license"),
                ],
            ],
            read: FxHashSet::default(),
        };
        let v = vocab::get();
        c.read = [v.rdf_type, v.sub_class_of, v.sub_property_of]
            .into_iter()
            .chain(c.labels)
            .chain([c.definition, c.comment, c.deprecated])
            .chain(c.header.into_iter().flatten())
            .chain(crate::analysis::indexed_predicates().iter().map(|p| p.0))
            .collect();
        c
    })
}

/// A label or definition as compared: trimmed, letter case folded.
fn folded(text: &str) -> impl Iterator<Item = char> + '_ {
    text.trim().chars().flat_map(char::to_lowercase)
}

/// The hash a label or definition is indexed by: of its folded text and its
/// (lowercase) language tag.
pub(crate) fn text_key(text: &str, lang: Option<&str>) -> u64 {
    const CHUNK: usize = 64;
    let mut h = FxHasher::default();
    let trimmed = text.trim();
    if trimmed.is_ascii() {
        // The bytes of what [`folded`] gives, without collecting it.
        let mut buf = [0u8; CHUNK];
        for chunk in trimmed.as_bytes().chunks(CHUNK) {
            let b = &mut buf[..chunk.len()];
            b.copy_from_slice(chunk);
            b.make_ascii_lowercase();
            h.write(b);
        }
    } else {
        let folded: String = folded(text).collect();
        for chunk in folded.as_bytes().chunks(CHUNK) {
            h.write(chunk);
        }
    }
    lang.hash(&mut h);
    h.finish()
}

/// The text and language of the literal `k`, an entry of source `s`, was
/// made from.
fn literal<'a>(s: &'a Slot, k: &Keyed) -> (&'a str, Option<Iri>) {
    let triple = s.facts.firsts[k.statement as usize] + k.offset;
    match &s.analysis.parsed.triples[triple as usize].o {
        Node::Literal { value, lang, .. } => (value, *lang),
        _ => ("", None),
    }
}

/// Whether two (text, language) pairs fold to the same key.
fn same_text(x: (&str, Option<Iri>), y: (&str, Option<Iri>)) -> bool {
    x.1 == y.1 && (x.0 == y.0 || folded(x.0).eq(folded(y.0)))
}

fn subj(n: &Node) -> Option<Subj> {
    match n {
        Node::Iri(i) => Some(Subj::Iri(*i)),
        Node::Blank(b) => Some(Subj::Blank(*b)),
        _ => None,
    }
}

impl DocFacts {
    /// Everything [`Knowledge`] needs from one analysis beyond its own indexes.
    /// Pure, so callers may compute it in parallel.
    pub(crate) fn new(a: &Analysis) -> DocFacts {
        let v = vocab::get();
        let l = links();
        let mut f = DocFacts::default();
        let triples = &a.parsed.triples;
        let mut shape_nodes: Vec<Subj> = Vec::new();
        // One summary per run of triples about one IRI subject, in text
        // order: a subject's triples mostly come together.
        let mut curated: Vec<(Iri, Curated)> = Vec::with_capacity(a.defs.len());
        let read = &curation().read;
        let mut firsts = vec![u32::MAX; a.parsed.statements.len()];
        // (key, value) pairs of `instances`, `subs`, `domain` and `range`.
        let mut links: [Vec<(Iri, Iri)>; 4] = Default::default();
        for (i, t) in triples.iter().enumerate() {
            let i = i as u32;
            let st = t.statement as usize;
            if st >= firsts.len() {
                firsts.resize(st + 1, u32::MAX);
            }
            firsts[st] = firsts[st].min(i);
            let first = firsts[st];
            if let Node::Iri(s) = &t.s {
                let reads = read.contains(&t.p);
                if reads {
                    f.curation(i - first, *s, t);
                }
                let c = match curated.last_mut() {
                    Some((r, c)) if r == s => c,
                    _ => {
                        curated.push((*s, Curated::default()));
                        &mut curated.last_mut().expect("just pushed").1
                    }
                };
                if reads {
                    curate(c, t);
                }
            }
            if t.p == v.target_class
                && let Some(s) = subj(&t.s)
            {
                shape_nodes.push(s);
            }
            let (Node::Iri(s), Node::Iri(o)) = (&t.s, &t.o) else {
                continue;
            };
            let (s, o) = (*s, *o);
            if t.p == v.rdf_type {
                links[0].push((o, s));
                if o == v.node_shape {
                    shape_nodes.push(Subj::Iri(s));
                }
            } else if t.p == v.sub_class_of || t.p == v.sub_property_of {
                links[1].push((o, s));
            } else if l.domain.contains(&t.p) {
                links[2].push((s, o));
            } else if l.range.contains(&t.p) {
                links[3].push((s, o));
            }
        }
        let [instances, subs, domain, range] = links.map(ByIri::new);
        (f.instances, f.subs, f.domain, f.range) = (instances, subs, domain, range);
        if !shape_nodes.is_empty() {
            f.shapes(a, shape_nodes);
        }
        f.terms = counted(std::mem::take(&mut f.terms));
        f.defined = counted(std::mem::take(&mut f.defined));
        f.deprecated.sort_unstable_by_key(|d| d.ided());
        f.deprecated.dedup();
        if f.labels.len() + f.definitions.len() + f.comments.len() > 0 {
            f.firsts = firsts.into();
        }

        f.curated = summaries(curated, a);

        f.defs = counted(a.defs.iter().map(|(&s, v)| (s, v.len() as u32)).collect());
        let summarised: Counts<Iri> = f.curated.iter().map(|c| (c.0, 1)).collect();
        let linked = summed(&f.domain.counts(), &f.range.counts());
        f.subjects = summed(&summarised, &linked);
        let mut objects = summed(&f.instances.counts(), &f.subs.counts());
        if !f.shapes_by_target.is_empty() {
            let targets = f.shapes_by_target.iter().map(|(&o, v)| (o, v.len() as u32));
            objects = summed(&objects, &counted(targets.collect()));
        }
        f.objects = objects;
        // Counted by hash: a document uses a few predicates many times.
        let mut predicates: FxHashMap<Iri, u32> = FxHashMap::default();
        for r in a.parsed.refs.iter().filter(|r| r.role == Role::Predicate) {
            *predicates.entry(r.iri).or_default() += 1;
        }
        f.predicates = counted(predicates.into_iter().collect());
        f.refs = counted(
            a.refs_by_iri
                .iter()
                .map(|(&i, v)| (i, v.len() as u32))
                .collect(),
        );

        // Prefix bindings: declared here, or supplied from outside and used.
        let mut bindings: Vec<((Iri, Iri), u32)> = a
            .parsed
            .prefixes
            .iter()
            .map(|d| ((Iri::intern(&d.name), d.iri), 1))
            .collect();
        // By interned name: a document uses a few names thousands of times.
        let declared: FxHashSet<Iri> = bindings.iter().map(|b| b.0.0).collect();
        let mut used: FxHashSet<Iri> = FxHashSet::default();
        for u in &a.parsed.prefix_uses {
            if u.resolved
                && !declared.contains(&u.name)
                && used.insert(u.name)
                && let Some(ns) = a.external.get(u.name.as_str())
            {
                bindings.push(((u.name, Iri::intern(ns)), 1));
            }
        }
        f.bindings = counted(bindings);
        f
    }

    /// The entries of `index` under `key`, in text order. The first few
    /// lookups (those of a swap, on a keystroke) scan the entries; after
    /// them the order by key is made.
    fn under_key(&self, index: Index, key: u64) -> impl Iterator<Item = &Keyed> {
        const SCANS: u32 = 4;
        let all = index.of(self);
        let lock = &self.by_key[index as usize];
        let scan = lock.get().is_none() && self.scans.fetch_add(1, Ordering::Relaxed) < SCANS;
        let order = (!scan).then(|| {
            lock.get_or_init(|| {
                let mut keys: Vec<(u64, u32)> =
                    all.iter().zip(0..).map(|(k, i)| (k.key, i)).collect();
                keys.sort_unstable();
                keys.into_iter().map(|k| k.1).collect()
            })
        });
        let scanned = scan.then(|| all.iter().filter(move |k| k.key == key));
        let found = order.map(|order| {
            let from = order.partition_point(|&i| all[i as usize].key < key);
            order[from..]
                .iter()
                .map(move |&i| &all[i as usize])
                .take_while(move |k| k.key == key)
        });
        scanned
            .into_iter()
            .flatten()
            .chain(found.into_iter().flatten())
    }

    /// The curation facts of triple `t`, the `offset`th after the first of
    /// its statement, whose subject is the IRI `s`.
    fn curation(&mut self, offset: u32, s: Iri, t: &syntax::Triple) {
        let v = vocab::get();
        let c = curation();
        match &t.o {
            Node::Iri(o) if t.p == v.rdf_type && c.term_types.contains(o) => {
                self.terms.push((s, 1))
            }
            _ if t.p == v.sub_class_of || t.p == v.sub_property_of => self.terms.push((s, 1)),
            Node::Literal {
                value,
                lang,
                datatype,
                ..
            } => {
                let into = if c.labels.contains(&t.p) {
                    &mut self.labels
                } else if t.p == c.definition {
                    self.defined.push((s, 1));
                    &mut self.definitions
                } else if t.p == c.comment {
                    &mut self.comments
                } else {
                    if t.p == c.deprecated && *datatype == v.xsd_boolean {
                        match value.trim() {
                            "true" | "1" => self.deprecated.push((s, true)),
                            "false" | "0" => self.deprecated.push((s, false)),
                            _ => {}
                        }
                    }
                    return;
                };
                into.push(Keyed {
                    key: text_key(value, lang.as_ref().map(|l| l.as_str())),
                    subject: s,
                    statement: t.statement,
                    offset,
                });
            }
            _ => {}
        }
    }

    /// Forget the curation facts, which only workspace sources give.
    fn drop_curation(&mut self) {
        self.terms = Vec::new();
        self.labels = Vec::new();
        self.definitions = Vec::new();
        self.comments = Vec::new();
        self.firsts = Box::default();
        self.defined = Vec::new();
        self.deprecated = Vec::new();
        self.curated = Vec::new();
    }

    /// The [`Shape`]s among `nodes` (the subjects of `sh:targetClass` and
    /// the IRIs typed `sh:NodeShape`, in text order), read straight from the
    /// parse in one pass over its triples.
    fn shapes(&mut self, a: &Analysis, nodes: Vec<Subj>) {
        let v = vocab::get();
        let triples = &a.parsed.triples;
        let mut by_subject: FxHashMap<Subj, Vec<u32>> = FxHashMap::default();
        for (i, t) in triples.iter().enumerate() {
            if let Some(s) = subj(&t.s) {
                by_subject.entry(s).or_default().push(i as u32);
            }
        }
        let of = |s: Subj| by_subject.get(&s).map(Vec::as_slice).unwrap_or(&[]);
        let mut seen = FxHashSet::default();
        for node in nodes {
            if !seen.insert(node) {
                continue;
            }
            let own = of(node);
            let mut targets = Vec::new();
            let mut is_shape = false;
            let mut is_class = false;
            for &i in own {
                let t = &triples[i as usize];
                match &t.o {
                    Node::Iri(o) if t.p == v.target_class => targets.push(*o),
                    Node::Iri(o) if t.p == v.rdf_type => {
                        is_shape |= *o == v.node_shape;
                        is_class |= *o == v.rdfs_class || *o == v.owl_class;
                    }
                    _ => {}
                }
            }
            if let (Subj::Iri(s), true, true) = (node, is_shape, is_class) {
                targets.push(s);
            }
            if targets.is_empty() {
                continue;
            }
            let properties = own
                .iter()
                .map(|&i| &triples[i as usize])
                .filter(|t| t.p == v.property)
                .filter_map(|t| {
                    let ps = subj(&t.o)?;
                    let mut c = PropertyConstraint {
                        path: None,
                        min_count: None,
                        max_count: None,
                        class: Vec::new(),
                        datatype: None,
                        in_values: None,
                        span: t.o_span,
                    };
                    for &j in of(ps) {
                        let pt = &triples[j as usize];
                        let count = || match &pt.o {
                            Node::Literal { value, .. } => value.trim().parse::<u32>().ok(),
                            _ => None,
                        };
                        match &pt.o {
                            Node::Iri(o) if pt.p == v.path => c.path = Some(*o),
                            Node::Iri(o) if pt.p == v.class => c.class.push(*o),
                            Node::Iri(o) if pt.p == v.datatype => c.datatype = Some(*o),
                            _ if pt.p == v.min_count => c.min_count = count(),
                            _ if pt.p == v.max_count => c.max_count = count(),
                            head if pt.p == v.in_list => {
                                c.in_values = Some(list(head, &of, triples))
                            }
                            _ => {}
                        }
                    }
                    Some(c)
                })
                .collect();
            let span = own
                .first()
                .and_then(|&i| {
                    a.parsed
                        .statements
                        .get(triples[i as usize].statement as usize)
                })
                .map_or_else(Span::default, |s| s.span);
            let index = self.shapes.len() as u32;
            targets.sort_unstable();
            targets.dedup();
            for &t in &targets {
                self.shapes_by_target.entry(t).or_default().push(index);
            }
            self.shapes.push(Shape {
                node: match node {
                    Subj::Iri(i) => Some(i),
                    Subj::Blank(_) => None,
                },
                span,
                targets,
                properties,
            });
        }

        /// The members of an RDF collection: every `rdf:first` of a cell,
        /// the last `rdf:rest`, at most 10,000 cells (a cyclic list
        /// repeats). Of a well-formed list, the members that
        /// [`crate::shacl::model::list`] reads.
        fn list<'t>(
            head: &Node,
            of: &dyn Fn(Subj) -> &'t [u32],
            triples: &[syntax::Triple],
        ) -> Vec<Node> {
            let v = vocab::get();
            let mut out = Vec::new();
            let mut cur = head.clone();
            for _ in 0..10_000 {
                let Some(s) = subj(&cur) else { break };
                if cur == Node::Iri(v.rdf_nil) {
                    break;
                }
                let mut next = None;
                for &i in of(s) {
                    let t = &triples[i as usize];
                    if t.p == v.rdf_first {
                        out.push(t.o.clone());
                    } else if t.p == v.rdf_rest {
                        next = Some(t.o.clone());
                    }
                }
                match next {
                    Some(n) => cur = n,
                    None => break,
                }
            }
            out
        }
    }
}

/// The [`DocFacts::curated`] summaries from one per run of triples about an
/// IRI subject (`runs`, in text order), and the subjects of the top-level
/// statements of `a`: sorted, one per IRI, defaults left out.
fn summaries(mut runs: Vec<(Iri, Curated)>, a: &Analysis) -> Vec<(Iri, Curated)> {
    // Text order is mostly the order IRIs were first interned in, which a
    // stable sort finds in one pass.
    runs.sort_by_key(|r| r.0.id());
    let mut out: Vec<(Iri, Curated)> = Vec::with_capacity(runs.len());
    for (s, c) in runs {
        match out.last_mut() {
            Some((r, last)) if *r == s => last.add(c),
            _ => out.push((s, c)),
        }
    }
    let mut described = 0;
    for (s, c) in &mut out {
        c.describes = a.defs.contains_key(s);
        described += usize::from(c.describes);
    }
    // A subject with no triple (a statement cut short by a syntax error).
    if described < a.defs.len() {
        let missing: Vec<Iri> = a
            .defs
            .keys()
            .filter(|s| out.binary_search_by_key(&s.id(), |r| r.0.id()).is_err())
            .copied()
            .collect();
        out.extend(missing.into_iter().map(|s| {
            let c = Curated {
                describes: true,
                ..Curated::default()
            };
            (s, c)
        }));
        out.sort_by_key(|r| r.0.id());
    }
    out.retain(|(_, c)| *c != Curated::default());
    out
}

/// Add triple `t`, about the IRI subject `c` is for, to its [`Curated`]
/// summary: all of it but `describes`. What it says of the indexes of the
/// analysis is what they hold ([`crate::analysis::feeds`]).
fn curate(c: &mut Curated, t: &syntax::Triple) {
    use crate::analysis::Index as Feeds;
    match (crate::analysis::feeds(t.p), &t.o) {
        (Some(Feeds::Type | Feeds::Super), Node::Iri(_)) => c.declares = true,
        (Some(Feeds::Label), Node::Literal { .. }) => c.labelled = true,
        (Some(Feeds::Comment), Node::Literal { .. }) => c.commented = true,
        _ => {}
    }
    let v = vocab::get();
    let k = curation();
    if t.p == v.rdf_type {
        if let Node::Iri(o) = &t.o {
            if k.class_types.contains(o) {
                c.kind |= TERM_CLASS;
            } else if k.property_types.contains(o) {
                c.kind |= TERM_PROPERTY;
            } else if *o == k.concept {
                c.kind |= TERM_CONCEPT;
            } else if *o == k.ontology {
                c.ontology = true;
            }
        }
    } else if t.p == v.sub_class_of {
        c.kind |= TERM_CLASS;
    } else if t.p == v.sub_property_of {
        c.kind |= TERM_PROPERTY;
    }
    for (bit, group) in k.header.iter().enumerate() {
        if group.contains(&t.p) {
            c.header |= 1 << bit;
        }
    }
}

/// The sources with an entry under a text key: one, nearly always, kept
/// without an allocation.
#[derive(Debug)]
enum Posting {
    One(DocId),
    /// Two or more.
    Many(Vec<DocId>),
}

impl Posting {
    fn ids(&self) -> &[DocId] {
        match self {
            Posting::One(id) => std::slice::from_ref(id),
            Posting::Many(ids) => ids,
        }
    }

    /// Add `id` unless it is listed last.
    fn add(&mut self, id: DocId) {
        match self {
            Posting::One(x) if *x == id => {}
            Posting::One(x) => *self = Posting::Many(vec![*x, id]),
            Posting::Many(ids) => {
                if ids.last() != Some(&id) {
                    ids.push(id);
                }
            }
        }
    }

    /// Remove `id`, if listed; false when none is left.
    fn remove(&mut self, id: DocId) -> bool {
        match self {
            Posting::One(x) => *x != id,
            Posting::Many(ids) => {
                if let Some(at) = ids.iter().position(|&d| d == id) {
                    ids.swap_remove(at);
                }
                if let [one] = ids[..] {
                    *self = Posting::One(one);
                }
                true
            }
        }
    }
}

/// Change the postings of one index of source `id` from its entries `xs`
/// to its entries `ys`. A source is listed once under each key it has an
/// entry under. On a swap, only the keys of the entries [`differing`] finds
/// that the source gains or loses are looked up.
fn repost(postings: &mut FxHashMap<u64, Posting>, id: DocId, xs: &[Keyed], ys: &[Keyed]) {
    if xs.is_empty() {
        for k in ys {
            // After an earlier entry with the same key, it is listed last.
            postings
                .entry(k.key)
                .and_modify(|docs| docs.add(id))
                .or_insert(Posting::One(id));
        }
        return;
    }
    if ys.is_empty() {
        for k in xs {
            // Not found after an earlier entry with the same key.
            if let Some(docs) = postings.get_mut(&k.key)
                && !docs.remove(id)
            {
                postings.remove(&k.key);
            }
        }
        return;
    }
    let (gone, came) = differing(xs, ys, |p, q| p.key == q.key);
    if gone.is_empty() && came.is_empty() {
        return;
    }
    // Per key of a differing entry: (key, gone, came).
    let mut keys: Vec<(u64, bool, bool)> = gone
        .iter()
        .map(|&i| (xs[i].key, true, false))
        .chain(came.iter().map(|&i| (ys[i].key, false, true)))
        .collect();
    keys.sort_unstable_by_key(|k| k.0);
    keys.dedup_by(|later, kept| {
        let same = later.0 == kept.0;
        if same {
            kept.1 |= later.1;
            kept.2 |= later.2;
        }
        same
    });
    keys.retain(|k| k.1 != k.2);
    if keys.is_empty() {
        return;
    }
    // A key of an entry both versions have stays.
    let mut gone = gone.into_iter().peekable();
    for (i, k) in xs.iter().enumerate() {
        if gone.next_if_eq(&i).is_some() {
            continue;
        }
        if let Ok(at) = keys.binary_search_by_key(&k.key, |c| c.0) {
            keys[at].1 = keys[at].2;
        }
    }
    for (key, before, after) in keys {
        if before == after {
            continue;
        }
        if before {
            if let Some(docs) = postings.get_mut(&key)
                && !docs.remove(id)
            {
                postings.remove(&key);
            }
        } else {
            postings
                .entry(key)
                .and_modify(|docs| docs.add(id))
                .or_insert(Posting::One(id));
        }
    }
}

fn entry(entries: &mut IriMap<Entry>, iri: Iri) -> &mut Entry {
    entries.entry(iri).or_insert_with(|| Entry {
        ns: Some(Iri::intern(namespace_of(&iri))),
        ..Default::default()
    })
}

fn toggle(v: &mut Vec<DocId>, id: DocId, add: bool) {
    if add {
        v.push(id);
    } else if let Some(i) = v.iter().position(|&d| d == id) {
        v.swap_remove(i);
    }
}

fn step(n: &mut u32, add: bool) -> bool {
    // Returns true when the count crosses zero.
    if add {
        *n += 1;
        *n == 1
    } else {
        *n = n.saturating_sub(1);
        *n == 0
    }
}

impl Namespace {
    /// [`Knowledge::is_own_namespace`] without the well-known vocabulary check.
    fn is_own(&self) -> bool {
        self.described >= OWN_MIN_DESCRIBED
            && self.described * 100 >= OWN_MIN_SHARE_PERCENT * self.referenced
    }

    fn is_project(&self) -> bool {
        self.described >= OWN_MIN_DESCRIBED
            && self.described * 100 >= PROJECT_MIN_SHARE_PERCENT * self.referenced
    }
}

/// Step one of a namespace's counters, remembering whether it was own before
/// the change began.
fn ns_step(
    namespaces: &mut FxHashMap<Iri, Namespace>,
    own_before: &mut FxHashMap<Iri, bool>,
    ns: Iri,
    add: bool,
    field: fn(&mut Namespace) -> &mut u32,
) {
    let n = namespaces.entry(ns).or_default();
    own_before.entry(ns).or_insert_with(|| n.is_own());
    step(field(n), add);
}

impl Knowledge {
    /// Bumped on every change; lets callers tell whether cached results are stale.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// Bumped only when a change can alter what other documents' lints say:
    /// an IRI becomes described or stops being described, a namespace becomes
    /// or stops being an own namespace, a prefix binding appears or goes, an
    /// IRI becomes or stops being a term, a term gains or loses its first
    /// `skos:definition`, the own-deprecated set changes, the terms that
    /// share a label or definition change where two or more share it before
    /// or after, or what the curation checks read of a term or an ontology
    /// changes ([`TermState`]: its kind, whether the workspace gives it a
    /// label, a comment or each header property, which document reports it).
    /// Ordinary keystrokes, inside a label or comment too, leave it alone.
    /// [`Changes`] lists the reasons, item by item.
    pub(crate) fn described_generation(&self) -> u64 {
        self.described_generation
    }

    /// Note each namespace of `own_before` (whether it was own before the
    /// change) that changed sides.
    fn settle_namespaces(&mut self, own_before: FxHashMap<Iri, bool>) {
        for (ns, before) in own_before {
            let after = self.namespaces.get(&ns).is_some_and(Namespace::is_own);
            if before != after && VOCABS.by_namespace(&ns).is_none() {
                self.changes.namespaces.push(ns);
            }
        }
    }

    /// The IRIs whose [`Curated`] summaries differ between two versions of a
    /// source (`None`: no version), sorted by [`Iri::id`]. An IRI that is
    /// not a term and that no version or other source types `owl:Ontology`
    /// is left out: its [`TermState`] stays the default unless it becomes a
    /// term, which is a change anyway.
    fn touched_iris(&self, old: Option<&DocFacts>, new: Option<&DocFacts>) -> Vec<Iri> {
        let xs = old.map_or(&[][..], |f| &f.curated[..]);
        let ys = new.map_or(&[][..], |f| &f.curated[..]);
        let mut out = Vec::new();
        merge(
            xs,
            ys,
            |c| c.0,
            |x, y| {
                let (iri, x, y) = match (x, y) {
                    (Some(x), Some(y)) if x.1 == y.1 => return,
                    (Some(c), _) | (None, Some(c)) => (c.0, x.map(|c| &c.1), y.map(|c| &c.1)),
                    (None, None) => return,
                };
                let read = x.is_some_and(|c| c.ontology)
                    || y.is_some_and(|c| c.ontology)
                    || self
                        .marks
                        .get(&iri)
                        .is_some_and(|m| m.term > 0 || m.ontology > 0);
                if read {
                    out.push(iri);
                }
            },
        );
        out
    }

    /// The text keys whose entries differ between two versions of a source
    /// (`None`: no version), with what they collide as, perhaps more than
    /// once: those of the entries [`differing`] finds, about IRIs that count
    /// under them ([`Knowledge::counts`]). A key whose changed entries do not
    /// count keeps its collisions, unless a term or a term's definition
    /// appears or goes, which is a change anyway.
    ///
    /// Entries match by key and subject: two texts with one key fold to the
    /// same text, but for a collision of the 64-bit hash.
    fn touched_texts(&self, old: Option<&DocFacts>, new: Option<&DocFacts>) -> Vec<(Text, u64)> {
        let mut out = Vec::new();
        if self.terms == 0 {
            // Nothing counts.
            return out;
        }
        for index in Index::ALL {
            let xs = old.map_or(&[][..], |f| index.of(f));
            let ys = new.map_or(&[][..], |f| index.of(f));
            let same = |p: &Keyed, q: &Keyed| p.key == q.key && p.subject == q.subject;
            let (gone, came) = differing(xs, ys, same);
            let changed = gone
                .iter()
                .map(|&i| &xs[i])
                .chain(came.iter().map(|&i| &ys[i]));
            for k in changed {
                if self.counts(index, k.subject) {
                    out.push((index.text(), k.key));
                }
            }
        }
        out
    }

    /// The collisions under each of `keys` before a change (see
    /// [`Knowledge::touched_texts`]).
    fn note_texts(&self, keys: Vec<(Text, u64)>) -> FxHashMap<(Text, u64), Collisions> {
        let mut before = FxHashMap::default();
        for k in keys {
            before.entry(k).or_insert_with(|| self.collisions(k.0, k.1));
        }
        before
    }

    /// Note each key of `before` whose collisions changed.
    fn settle_texts(&mut self, before: FxHashMap<(Text, u64), Collisions>) {
        for ((text, key), was) in before {
            if self.collisions(text, key) != was {
                self.changes.texts.push((text, key));
            }
        }
    }

    /// The state of each of `iris` before a change (see
    /// [`Knowledge::touched_iris`]), in their order.
    fn note_terms(&self, iris: Vec<Iri>) -> Vec<(Iri, TermState)> {
        iris.into_iter().map(|i| (i, self.term_state(i))).collect()
    }

    /// Note each IRI of `before` whose state changed.
    fn settle_terms(&mut self, before: Vec<(Iri, TermState)>) {
        for (iri, was) in before {
            if self.term_state(iri) != was {
                self.changes.iris.push(iri);
            }
        }
    }

    /// The [`Curated`] summaries the workspace sources give `iri`, with the
    /// URI of each source.
    fn curated(&self, iri: Iri) -> impl Iterator<Item = (&str, &Curated)> {
        self.entries
            .get(&iri)
            .into_iter()
            .flat_map(|e| &e.subject)
            .filter_map(move |&id| {
                let s = self.slot(id);
                let all = &s.facts.curated;
                let at = all.binary_search_by_key(&iri.id(), |c| c.0.id()).ok()?;
                Some((s.uri.as_str(), &all[at].1))
            })
    }

    fn term_state(&self, iri: Iri) -> TermState {
        let term = self.is_term(&iri);
        let mut st = TermState::default();
        let (mut declarer, mut describer, mut ontology) = (None, None, None);
        fn first<'a>(x: &mut Option<&'a str>, uri: &'a str) {
            if x.is_none_or(|x| uri < x) {
                *x = Some(uri);
            }
        }
        let mut header = 0;
        for (uri, c) in self.curated(iri) {
            st.kind |= c.kind;
            st.labelled |= c.labelled;
            st.commented |= c.commented;
            header |= c.header;
            if c.describes {
                first(&mut describer, uri);
                if c.declares {
                    first(&mut declarer, uri);
                }
                if c.ontology {
                    first(&mut ontology, uri);
                }
            }
        }
        if !term {
            (st.kind, st.labelled, st.commented) = (0, false, false);
        } else {
            st.reporter = declarer.or(describer).map(str::to_string);
        }
        if ontology.is_some() {
            st.ontology = ontology.map(str::to_string);
            st.header = header;
        }
        st
    }

    // ---- sources ----

    /// Add or replace the workspace document `uri`. A no-op if this exact
    /// analysis is already registered.
    pub fn set(&mut self, uri: &str, source: Source, a: Arc<Analysis>) {
        if self.is_current(&Key::Workspace(uri.to_string()), source, &a) {
            return;
        }
        let facts = DocFacts::new(&a);
        self.set_prepared(uri, source, a, facts);
    }

    /// [`Knowledge::set`] with facts computed beforehand (e.g. in parallel).
    pub(crate) fn set_prepared(
        &mut self,
        uri: &str,
        source: Source,
        a: Arc<Analysis>,
        facts: DocFacts,
    ) {
        debug_assert!(source != Source::External, "use add_external");
        self.put(Key::Workspace(uri.to_string()), uri, source, a, facts);
    }

    pub fn remove(&mut self, uri: &str) {
        self.take(&Key::Workspace(uri.to_string()));
    }

    /// Add or replace a read-only source (e.g. an imported ontology) under
    /// `key`. Its facts count; it is never diagnosed, renamed or listed as a
    /// workspace symbol.
    pub(crate) fn add_external(&mut self, key: &str, a: Arc<Analysis>) {
        let k = Key::External(key.to_string());
        if self.is_current(&k, Source::External, &a) {
            return;
        }
        let facts = DocFacts::new(&a);
        self.put(k, key, Source::External, a, facts);
    }

    /// [`Knowledge::add_external`] with facts computed beforehand (e.g. on a
    /// background thread).
    pub(crate) fn add_external_prepared(&mut self, key: &str, a: Arc<Analysis>, facts: DocFacts) {
        let k = Key::External(key.to_string());
        if self.is_current(&k, Source::External, &a) {
            return;
        }
        self.put(k, key, Source::External, a, facts);
    }

    /// Whether `key` is registered with exactly this analysis.
    pub(crate) fn has_external(&self, key: &str, a: &Arc<Analysis>) -> bool {
        self.is_current(&Key::External(key.to_string()), Source::External, a)
    }

    pub(crate) fn remove_external(&mut self, key: &str) {
        self.take(&Key::External(key.to_string()));
    }

    /// The analysis registered for workspace document `uri`.
    pub(crate) fn get(&self, uri: &str) -> Option<&Arc<Analysis>> {
        let id = self.by_key.get(&Key::Workspace(uri.to_string()))?;
        Some(&self.slot(*id).analysis)
    }

    pub(crate) fn doc(&self, id: DocId) -> &Analysis {
        &self.slot(id).analysis
    }

    /// Every source, in no particular order.
    pub fn docs(&self) -> impl Iterator<Item = (DocId, Source, &Arc<Analysis>)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i as DocId, s.source, &s.analysis)))
    }

    /// Every source with the key it was registered under (the URI of a
    /// workspace document, the key of an external one), in no particular order.
    pub(crate) fn sources(&self) -> impl Iterator<Item = (&str, Source, &Arc<Analysis>)> {
        self.slots
            .iter()
            .flatten()
            .map(|s| (s.uri.as_str(), s.source, &s.analysis))
    }

    fn is_current(&self, key: &Key, source: Source, a: &Arc<Analysis>) -> bool {
        self.by_key.get(key).is_some_and(|&id| {
            let s = self.slot(id);
            s.source == source && Arc::ptr_eq(&s.analysis, a)
        })
    }

    fn slot(&self, id: DocId) -> &Slot {
        self.slots[id as usize].as_ref().expect("live doc id")
    }

    fn put(&mut self, key: Key, uri: &str, source: Source, a: Arc<Analysis>, mut facts: DocFacts) {
        if source == Source::External {
            facts.drop_curation();
        }
        let slot = Slot {
            uri: uri.to_string(),
            source,
            analysis: a,
            facts,
        };
        // A new version keeps its source's id, so that what both versions
        // give (nearly all of it, on a keystroke) is left as it is.
        let id = match self.by_key.get(&key) {
            Some(&id) => id,
            None => {
                let id = self.free.pop().unwrap_or_else(|| {
                    self.slots.push(None);
                    (self.slots.len() - 1) as DocId
                });
                self.by_key.insert(key, id);
                id
            }
        };
        self.change(id, Some(slot));
    }

    fn take(&mut self, key: &Key) {
        if let Some(id) = self.by_key.remove(key) {
            self.change(id, None);
            self.free.push(id);
        }
    }

    /// What the change that moved [`Knowledge::generation`] to where it is
    /// changed (see [`Changes`]): a caller that saw the generation before
    /// it knows what to look at again. A call that changes nothing leaves
    /// both as they are.
    pub(crate) fn changes(&self) -> &Changes {
        &self.changes
    }

    /// Replace the version of source `id` that is registered (if any) with
    /// `new` (`None`: remove it).
    fn change(&mut self, id: DocId, new: Option<Slot>) {
        self.changes.clear();
        let old = self.slots[id as usize].as_ref();
        let ws = old
            .or(new.as_ref())
            .is_some_and(|s| s.source != Source::External);
        let (mut texts, mut terms) = (FxHashMap::default(), Vec::new());
        if ws {
            let (old, new) = (old.map(|s| &s.facts), new.as_ref().map(|s| &s.facts));
            let keys = self.touched_texts(old, new);
            let iris = self.touched_iris(old, new);
            texts = self.note_texts(keys);
            terms = self.note_terms(iris);
        }
        let old = self.slots[id as usize].take();
        let none = DocFacts::default();
        let mut own_before = FxHashMap::default();
        self.apply(
            id,
            ws,
            old.as_ref().map_or(&none, |s| &s.facts),
            new.as_ref().map_or(&none, |s| &s.facts),
            &mut own_before,
        );
        self.slots[id as usize] = new;
        self.settle_namespaces(own_before);
        self.settle_texts(texts);
        self.settle_terms(terms);
        self.changes.settle();
        if !self.changes.is_empty() {
            self.described_generation += 1;
        }
        self.generation += 1;
    }

    /// Change what source `id` contributes from `old` to `new` (an empty
    /// [`DocFacts`]: no version). Only where the two differ is the index
    /// looked up: facts that give the same IRIs and keys in each role, after
    /// a keystroke inside a comment for one, leave it as it is.
    fn apply(
        &mut self,
        id: DocId,
        ws: bool,
        old: &DocFacts,
        new: &DocFacts,
        own_before: &mut FxHashMap<Iri, bool>,
    ) {
        let entries = &mut self.entries;
        let namespaces = &mut self.namespaces;
        let changes = &mut self.changes;
        // IRIs that lost a contribution: their entries may be empty now.
        let mut lost: Vec<Iri> = Vec::new();
        diff(&old.subjects, &new.subjects, |iri, add| {
            toggle(&mut entry(entries, iri).subject, id, add);
            if !add {
                lost.push(iri);
            }
        });
        diff(&old.objects, &new.objects, |iri, add| {
            toggle(&mut entry(entries, iri).object, id, add);
            if !add {
                lost.push(iri);
            }
        });
        diff(&old.predicates, &new.predicates, |iri, add| {
            step(&mut entry(entries, iri).predicate, add);
            if !add {
                lost.push(iri);
            }
        });
        diff(&old.refs, &new.refs, |iri, add| {
            let e = entry(entries, iri);
            let was_empty = e.refs.is_empty();
            toggle(&mut e.refs, id, add);
            let ns = e.ns.expect("set on creation");
            if was_empty != e.refs.is_empty() {
                let n = namespaces.entry(ns).or_default();
                if add {
                    n.terms.insert(iri);
                } else {
                    n.terms.remove(&iri);
                }
            }
            if ws && step(&mut e.refs_ws, add) {
                ns_step(namespaces, own_before, ns, add, |n| &mut n.referenced);
            }
            if !add {
                lost.push(iri);
            }
        });
        diff(&old.defs, &new.defs, |iri, add| {
            let e = entry(entries, iri);
            let ns = e.ns.expect("set on creation");
            let was = e.described_ws + e.described_ext > 0;
            if !ws {
                step(&mut e.described_ext, add);
            } else if step(&mut e.described_ws, add) {
                ns_step(namespaces, own_before, ns, add, |n| &mut n.described);
            }
            if was != (e.described_ws + e.described_ext > 0) {
                changes.iris.push(iri);
            }
            if !add {
                lost.push(iri);
            }
        });
        if ws {
            let bindings = &mut self.bindings;
            diff(&old.bindings, &new.bindings, |(name, ns), add| {
                let by_ns = bindings.entry(name).or_default();
                let n = by_ns.entry(ns).or_default();
                if step(n, add) {
                    changes.prefixes.push(name);
                    if !add {
                        by_ns.remove(&ns);
                        if by_ns.is_empty() {
                            bindings.remove(&name);
                        }
                    }
                }
            });
            self.apply_curation(old, new);
        }
        self.repost(id, old, new);
        for iri in lost {
            if let Some(e) = self.entries.get(&iri)
                && e.is_empty()
            {
                let ns = e.ns;
                self.entries.remove(&iri);
                if let Some(ns) = ns
                    && let Some(n) = self.namespaces.get(&ns)
                    && n.terms.is_empty()
                    && n.described == 0
                    && n.referenced == 0
                {
                    self.namespaces.remove(&ns);
                }
            }
        }
    }

    /// Change one workspace source's curation facts from `old` to `new`,
    /// noting each term that appears or goes, gains its first definition or
    /// loses its last, and each own deprecation that appears or goes.
    fn apply_curation(&mut self, old: &DocFacts, new: &DocFacts) {
        let marks = &mut self.marks;
        let changed = &mut self.changes.iris;
        let (mut terms, mut deprecations) = (self.terms, self.deprecations);
        // Step one count of `iri`'s marks; the marks before and after.
        let mut mark = |iri: Iri, add: bool, count: fn(&mut Marks) -> &mut u32| {
            let m = marks.entry(iri).or_default();
            let (was_term, was_deprecated) = (m.term > 0, m.deprecated());
            let crossed = step(count(m), add);
            let (is_term, is_deprecated) = (m.term > 0, m.deprecated());
            if m.is_empty() {
                marks.remove(&iri);
            }
            (crossed, was_term, is_term, was_deprecated, is_deprecated)
        };
        diff(&old.terms, &new.terms, |iri, add| {
            let (_, was, now, _, _) = mark(iri, add, |m| &mut m.term);
            if was != now {
                changed.push(iri);
                if was {
                    terms -= 1;
                } else {
                    terms += 1;
                }
            }
        });
        diff(&old.defined, &new.defined, |iri, add| {
            let (crossed, _, term, _, _) = mark(iri, add, |m| &mut m.defined);
            if crossed && term {
                changed.push(iri);
            }
        });
        merge(
            &old.deprecated,
            &new.deprecated,
            |d| *d,
            |x, y| {
                let (&(iri, value), add) = match (x, y) {
                    (Some(d), None) => (d, false),
                    (None, Some(d)) => (d, true),
                    _ => return,
                };
                let count: fn(&mut Marks) -> &mut u32 = if value {
                    |m| &mut m.deprecated
                } else {
                    |m| &mut m.undeprecated
                };
                let (_, _, _, was, now) = mark(iri, add, count);
                if was != now {
                    changed.push(iri);
                    if was {
                        deprecations -= 1;
                    } else {
                        deprecations += 1;
                    }
                }
            },
        );
        merge(
            &old.curated,
            &new.curated,
            |c| c.0,
            |x, y| {
                let (before, after) = (
                    x.is_some_and(|c| c.1.ontology),
                    y.is_some_and(|c| c.1.ontology),
                );
                if before != after {
                    let iri = x.or(y).expect("one of them").0;
                    mark(iri, after, |m| &mut m.ontology);
                }
            },
        );
        (self.terms, self.deprecations) = (terms, deprecations);
    }

    /// Change the text postings of source `id` from `old` to `new`.
    fn repost(&mut self, id: DocId, old: &DocFacts, new: &DocFacts) {
        for index in Index::ALL {
            let postings = match index {
                Index::Label => &mut self.labels,
                Index::Definition => &mut self.definitions,
                Index::Comment => &mut self.comments,
            };
            repost(postings, id, index.of(old), index.of(new));
        }
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
mod tests {
    use super::*;
    use crate::syntax::PrefixMap;
    use std::path::Path;

    #[test]
    fn a_namespace_ends_at_the_last_delimiter_unless_a_vocabulary_owns_more() {
        assert_eq!(
            namespace_of("http://example.org/ns#a"),
            "http://example.org/ns#"
        );
        assert_eq!(namespace_of("urn:x:y"), "urn:x:");
        assert_eq!(namespace_of("plain"), "");
        // Open Graph's local names hold a colon.
        assert_eq!(
            namespace_of("http://ogp.me/ns#audio:album"),
            "http://ogp.me/ns#"
        );
        assert_eq!(namespace_of("http://ogp.me/ns#title"), "http://ogp.me/ns#");
    }

    fn analysis(uri: &str, text: &str, external: &Arc<PrefixMap>) -> Arc<Analysis> {
        Arc::new(Analysis::new(uri.into(), text, external.clone(), false))
    }

    fn fixtures() -> Vec<String> {
        fn walk(dir: &Path, out: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();
            for p in paths {
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|e| e == "ttl") {
                    out.push(std::fs::read_to_string(&p).unwrap());
                }
            }
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
        let mut out = Vec::new();
        walk(&root.join("workspace"), &mut out);
        walk(&root.join("project"), &mut out);
        assert!(out.len() >= 8, "fixtures missing");
        out
    }

    fn project_prefixes() -> Arc<PrefixMap> {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/project/prefixes.ttl"),
        )
        .unwrap();
        let mut m = PrefixMap::default();
        for p in syntax::parse(&text, &PrefixMap::default(), syntax::Syntax::Turtle).prefixes {
            m.insert(p.name, p.iri.to_string());
        }
        Arc::new(m)
    }

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// A fixture, sometimes cut short or with a renamed term, so documents
    /// overlap partially and contain syntax errors.
    fn variant(rng: &mut Rng, texts: &[String]) -> String {
        let t = &texts[rng.below(texts.len())];
        match rng.below(4) {
            0 => {
                let mut cut = rng.below(t.len() + 1);
                while !t.is_char_boundary(cut) {
                    cut -= 1;
                }
                t[..cut].to_string()
            }
            1 => t
                .replacen("Asset", "Assett", 1)
                .replacen("alice", "alicia", 2),
            _ => t.clone(),
        }
    }

    #[test]
    fn incremental_updates_match_a_rebuild() {
        let texts = fixtures();
        let ext = [Arc::new(PrefixMap::default()), project_prefixes()];
        for seed in 1..=8u64 {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ seed);
            let mut k = Knowledge::default();
            // The model: what should be registered.
            let mut model: FxHashMap<(bool, String), (Source, Arc<Analysis>)> =
                FxHashMap::default();
            for step in 0..300 {
                let slot = rng.below(10);
                let external = slot >= 8;
                let uri = format!("file:///ws/d{slot}.ttl");
                let key = (external, uri.clone());
                if rng.below(4) == 0 {
                    if external {
                        k.remove_external(&uri);
                    } else {
                        k.remove(&uri);
                    }
                    model.remove(&key);
                } else {
                    let text = variant(&mut rng, &texts);
                    let a = analysis(&uri, &text, &ext[rng.below(2)]);
                    if external {
                        k.add_external(&uri, a.clone());
                        model.insert(key, (Source::External, a));
                    } else {
                        let src = if rng.below(2) == 0 {
                            Source::Open
                        } else {
                            Source::Disk
                        };
                        k.set(&uri, src, a.clone());
                        model.insert(key, (src, a));
                    }
                }
                if step % 10 == 9 || step == 299 {
                    let mut fresh = Knowledge::default();
                    let mut keys: Vec<_> = model.keys().cloned().collect();
                    keys.sort();
                    for key in keys {
                        let (src, a) = &model[&key];
                        if key.0 {
                            fresh.add_external(&key.1, a.clone());
                        } else {
                            fresh.set(&key.1, *src, a.clone());
                        }
                    }
                    assert_eq!(k.snapshot(), fresh.snapshot(), "seed {seed} step {step}");
                }
            }
            // Removing everything leaves nothing behind.
            for (ext, uri) in model.keys() {
                if *ext {
                    k.remove_external(uri);
                } else {
                    k.remove(uri);
                }
            }
            assert!(k.snapshot().is_empty(), "{:?}", k.snapshot());
        }
    }

    fn project() -> Knowledge {
        let ext = project_prefixes();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/project");
        let mut k = Knowledge::default();
        for f in [
            "prefixes.ttl",
            "core/vocab.ttl",
            "shapes/asset-shapes.ttl",
            "data/instances.ttl",
            "data/observations.ttl",
        ] {
            let text = std::fs::read_to_string(root.join(f)).unwrap();
            let e = if f == "prefixes.ttl" {
                Arc::new(PrefixMap::default())
            } else {
                ext.clone()
            };
            k.set(
                &format!("file:///p/{f}"),
                Source::Disk,
                analysis(f, &text, &e),
            );
        }
        k
    }

    #[test]
    fn facts_from_the_project_fixture() {
        let k = project();
        let sd = "https://ontology.example.com/plant/core/";
        let iri = |l: &str| format!("{sd}{l}");
        assert_eq!(k.label(&iri("Asset")), Some("Asset"));
        assert!(k.is_described(&iri("Asset")));
        assert!(!k.is_described(&iri("Assett")));
        assert_eq!(k.descriptions(&iri("Asset")).len(), 1);
        assert_eq!(
            k.supers(&iri("Asset")),
            [Iri::intern(&iri("PhysicalObject"))]
        );
        let mut subs = k.subs(&iri("PhysicalObject"));
        subs.sort();
        assert_eq!(
            subs,
            [
                Iri::intern(&iri("Asset")),
                Iri::intern(&iri("MeasurementPoint"))
            ]
        );
        assert_eq!(
            k.domains(&iri("locatedIn")),
            [Iri::intern(&iri("PhysicalObject"))]
        );
        assert_eq!(k.ranges(&iri("locatedIn")), [Iri::intern(&iri("Position"))]);
        assert!(k.used_as_predicate(&iri("designation")));
        // English label wins over German.
        assert_eq!(
            k.label("https://vocab.example.com/plant/function/Milling"),
            Some("Milling")
        );

        let shapes = k.shapes_targeting(&iri("Asset"));
        assert_eq!(shapes.len(), 1);
        let shape = shapes[0].1;
        assert_eq!(
            shape.node.map(|n| n.to_string()),
            Some("https://shape.example.com/plant/core/AssetShape".into())
        );
        let by_path = |l: &str| {
            shape
                .properties
                .iter()
                .find(|p| p.path.is_some_and(|x| x == iri(l).as_str()))
                .unwrap()
        };
        let d = by_path("designation");
        assert_eq!((d.min_count, d.max_count), (Some(1), Some(1)));
        assert_eq!(
            d.datatype.map(|x| x.to_string()).as_deref(),
            Some("http://www.w3.org/2001/XMLSchema#string")
        );
        let f = by_path("fillsPosition");
        assert_eq!(f.class, [Iri::intern(&iri("Position"))]);
        assert_eq!((f.min_count, f.max_count), (None, Some(1)));
        // A sequence path is not a simple predicate; its sh:in list is kept.
        let seq = shape.properties.iter().find(|p| p.path.is_none()).unwrap();
        assert_eq!(
            seq.in_values,
            Some(vec![Node::Iri(Iri::intern(
                "https://vocab.example.com/plant/function/Milling"
            ))])
        );

        // sd: is bound to the project namespace in prefixes.ttl and used by fragments.
        let b = k.bindings("sd");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].0, sd);
        assert!(b[0].1 >= 4, "{b:?}");
        assert!(k.is_own_namespace(sd));
        assert!(!k.is_own_namespace("http://www.w3.org/2002/07/owl#"));
        assert!(k.terms_in(sd).contains(&Iri::intern(&iri("hasFunction"))));
    }

    #[test]
    fn external_sources_describe_but_do_not_count_as_workspace() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let text = "@prefix ex: <http://ex.org/> .\nex:a a ex:C .\nex:b a ex:C .\nex:c a ex:C .\n";
        k.add_external("cache:onto", analysis("cache:onto", text, &none));
        assert!(k.is_described("http://ex.org/a"));
        assert!(!k.is_own_namespace("http://ex.org/"));
        assert!(k.bindings("ex").is_empty());
        let g = k.generation();
        k.remove_external("cache:onto");
        assert!(!k.is_described("http://ex.org/a"));
        assert!(k.generation() > g);
        assert!(k.snapshot().is_empty());
    }

    #[test]
    fn ontologies_describing_a_namespace_include_external_sources() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let owl = "<http://www.w3.org/2002/07/owl#Ontology>";
        let ext = format!("<http://ex.org/x> a {owl} .\n<http://ex.org/x#A> a <urn:c> .\n");
        k.add_external("import:x", analysis("http://ex.org/x", &ext, &none));
        let ws = format!("<http://ex.org/w> a {owl} .\n<http://ex.org/w#B> a <urn:c> .\n");
        k.set(
            "file:///w.ttl",
            Source::Disk,
            analysis("file:///w.ttl", &ws, &none),
        );
        let data = "<http://ex.org/d#C> a <urn:c> .\n";
        k.set(
            "file:///d.ttl",
            Source::Disk,
            analysis("file:///d.ttl", data, &none),
        );
        let names = |ns: &str| -> Vec<String> {
            k.ontologies_describing(ns)
                .iter()
                .map(|(o, _)| o.to_string())
                .collect()
        };
        assert_eq!(names("http://ex.org/x#"), ["http://ex.org/x"]);
        assert_eq!(names("http://ex.org/w#"), ["http://ex.org/w"]);
        assert!(names("http://ex.org/d#").is_empty());
        assert!(names("http://xmlns.com/foaf/0.1/").is_empty());
    }

    #[test]
    fn own_namespace_needs_a_share_of_described_terms() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let mut text = String::from("@prefix ex: <http://ex.org/> .\n");
        for i in 0..3 {
            text.push_str(&format!("ex:t{i} a ex:C .\n"));
        }
        k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
        // 3 described of 4 referenced (t0..t2, C): below 80%.
        assert!(!k.is_own_namespace("http://ex.org/"));
        text.push_str("ex:C a ex:t0 .\n");
        k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
        // 4 of 4.
        assert!(k.is_own_namespace("http://ex.org/"));
        text.push_str("ex:t0 ex:t1 ex:x .\n");
        k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
        // 4 of 5: exactly 80%.
        assert!(k.is_own_namespace("http://ex.org/"));
        text.push_str("ex:t0 ex:t1 ex:y .\n");
        k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
        // 4 of 6.
        assert!(!k.is_own_namespace("http://ex.org/"));
    }

    #[test]
    fn described_generation_moves_only_when_the_described_set_does() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let header = "@prefix ex: <http://ex.org/> .\n";
        let set = |k: &mut Knowledge, body: &str| {
            let text = format!("{header}{body}");
            k.set("file:///a.ttl", Source::Open, analysis("a", &text, &none));
            k.described_generation()
        };
        let g = set(&mut k, "ex:a a ex:C .\n");
        // Same described set, different text, objects and whitespace.
        assert_eq!(set(&mut k, "ex:a  a ex:C .\n"), g);
        assert_eq!(set(&mut k, "ex:a a ex:D .\n"), g);
        assert_eq!(set(&mut k, "ex:a a ex:Dx ; ex:p ex:q .\n"), g);
        // A new subject.
        let g2 = set(&mut k, "ex:a a ex:C .\nex:b a ex:C .\n");
        assert!(g2 > g);
        // A new prefix binding.
        let g3 = set(
            &mut k,
            "@prefix f: <http://f.org/> .\nex:a a ex:C .\nex:b a ex:C .\n",
        );
        assert!(g3 > g2);
        // A namespace becoming own (3 of 3 described) without a 0<->1 change
        // in what other files see is still a change for them.
        let g4 = set(&mut k, "ex:a a ex:b .\nex:b a ex:c .\nex:c a ex:a .\n");
        assert!(g4 > g3);
        assert!(k.is_own_namespace("http://ex.org/"));
        k.remove("file:///a.ttl");
        assert!(k.described_generation() > g4);
    }

    const CURATION_HEADER: &str = "@prefix ex: <http://ex.org/> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n";

    /// Register (or replace) the Turtle document `name` with `body` after
    /// [`CURATION_HEADER`].
    fn put_ttl(k: &mut Knowledge, name: &str, body: &str) {
        let uri = format!("file:///{name}.ttl");
        let text = format!("{CURATION_HEADER}{body}");
        let none = Arc::new(PrefixMap::default());
        k.set(&uri, Source::Open, analysis(&uri, &text, &none));
    }

    fn names(v: Vec<Iri>) -> Vec<String> {
        v.iter()
            .map(|i| i.replace("http://ex.org/", "ex:"))
            .collect()
    }

    #[test]
    fn the_term_rule() {
        let mut k = Knowledge::default();
        put_ttl(
            &mut k,
            "a",
            "ex:c1 a owl:Class .\nex:c2 a rdfs:Class .\nex:p1 a owl:ObjectProperty .\n\
             ex:p2 a owl:DatatypeProperty .\nex:p3 a owl:AnnotationProperty .\n\
             ex:p4 a rdf:Property .\nex:k a skos:Concept .\n\
             ex:sub rdfs:subClassOf [ a owl:Restriction ] .\nex:subp rdfs:subPropertyOf ex:p1 .\n\
             ex:alice a ex:Person ; rdfs:label \"Alice\" .\n",
        );
        for t in ["c1", "c2", "p1", "p2", "p3", "p4", "k", "sub", "subp"] {
            assert!(k.is_term(&format!("http://ex.org/{t}")), "{t}");
        }
        // Instance data, and the objects of the links, are not terms.
        for t in ["alice", "Person"] {
            assert!(!k.is_term(&format!("http://ex.org/{t}")), "{t}");
        }
        // An external source does not make terms.
        let none = Arc::new(PrefixMap::default());
        let ext = format!("{CURATION_HEADER}ex:e a owl:Class .\n");
        k.add_external("import:e", analysis("import:e", &ext, &none));
        assert!(!k.is_term("http://ex.org/e"));
        // Nor does it stop the workspace's: a term needs one workspace source.
        put_ttl(&mut k, "b", "ex:c1 a owl:Class .\n");
        k.remove("file:///a.ttl");
        assert!(k.is_term("http://ex.org/c1"));
        assert!(!k.is_term("http://ex.org/c2"));
    }

    #[test]
    fn a_keystroke_in_a_comment_leaves_the_index_untouched() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "b", "ex:C a owl:Class ; rdfs:label \"Pump\" .\n");
        let body = |c: &str| {
            format!(
                "# {c}\nex:A a owl:Class ; rdfs:label \"Pump\" . # {c}\n\
                 ex:B a owl:Class ;\n  # {c}\n  rdfs:label \"Pump\" .\n"
            )
        };
        put_ttl(&mut k, "a", &body("Notes"));
        let (index, described) = (k.snapshot(), k.described_generation());
        for c in ["Notes.", "Notes. More", "Notes. More text"] {
            let g = k.generation();
            put_ttl(&mut k, "a", &body(c));
            assert_eq!(k.snapshot(), index, "{c}");
            assert_eq!(k.described_generation(), described, "{c}");
            assert_eq!(k.changes(), &Changes::default(), "{c}");
            // The analyses it hands out are new.
            assert!(k.generation() > g, "{c}");
        }
        assert_eq!(names(k.labelled("pump", None)), ["ex:A", "ex:B", "ex:C"]);
    }

    /// The `T` lines of the index (the sources under each text key) that
    /// are in one of `x` and `y` only.
    fn differing_texts(x: &[String], y: &[String]) -> Vec<String> {
        let only = |a: &[String], b: &[String]| -> Vec<String> {
            a.iter().filter(|l| !b.contains(l)).cloned().collect()
        };
        let mut out = only(x, y);
        out.extend(only(y, x));
        assert!(out.iter().all(|l| l.starts_with("T ")), "{out:?}");
        out.sort();
        out
    }

    #[test]
    fn a_label_edit_changes_only_its_keys() {
        let mut k = Knowledge::default();
        put_ttl(
            &mut k,
            "b",
            "ex:B a owl:Class ; rdfs:label \"Pump\" .\nex:C a owl:Class ; rdfs:label \"Valve\" .\n",
        );
        let body = |l: &str, d: &str| {
            format!(
                "ex:A a owl:Class ; rdfs:label \"{l}\" ; rdfs:comment \"A machine.\" .\n\
                 ex:D a owl:Class ; rdfs:label \"{d}\" .\n"
            )
        };
        let key = |t: &str| text_key(t, None);
        let (a, b) = ("Open:file:///a.ttl", "Open:file:///b.ttl");
        put_ttl(&mut k, "a", &body("Pump", "Drum"));
        let before = k.snapshot();
        // A's label leaves B's and joins C's.
        put_ttl(&mut k, "a", &body("Valve", "Drum"));
        let after = k.snapshot();
        let (pump, valve) = (key("pump"), key("valve"));
        let mut want = vec![
            format!("T Label {pump} [{a},{b}]"),
            format!("T Label {pump} [{b}]"),
            format!("T Label {valve} [{a},{b}]"),
            format!("T Label {valve} [{b}]"),
        ];
        want.sort();
        assert_eq!(differing_texts(&before, &after), want);
        let mut texts = vec![(Text::Label, pump), (Text::Label, valve)];
        texts.sort();
        let want = Changes {
            texts,
            ..Changes::default()
        };
        assert_eq!(k.changes(), &want);
        // D's label shares with nothing, before or after.
        put_ttl(&mut k, "a", &body("Valve", "Drums"));
        let (drum, drums) = (key("drum"), key("drums"));
        let mut want = vec![
            format!("T Label {drum} [{a}]"),
            format!("T Label {drums} [{a}]"),
        ];
        want.sort();
        assert_eq!(differing_texts(&after, &k.snapshot()), want);
        assert_eq!(k.changes(), &Changes::default());
    }

    #[test]
    fn a_ref_added_and_removed_in_one_swap_nets_to_nothing() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "a", "ex:a ex:p ex:x .\nex:b ex:q ex:y .\n");
        let (index, described) = (k.snapshot(), k.described_generation());
        // Each of ex:p, ex:q, ex:x and ex:y goes from one statement and
        // comes in the other.
        for body in [
            "ex:a ex:q ex:y .\nex:b ex:p ex:x .\n",
            // Two references to ex:x, then one.
            "ex:a ex:q ex:y , ex:x .\nex:b ex:p ex:x .\n",
            "ex:a ex:p ex:x .\nex:b ex:q ex:y .\n",
        ] {
            put_ttl(&mut k, "a", body);
            assert_eq!(k.snapshot(), index, "{body}");
            assert_eq!(k.changes(), &Changes::default(), "{body}");
        }
        assert_eq!(k.described_generation(), described);
    }

    #[test]
    fn a_literal_is_found_where_its_statement_is_split_by_another() {
        // JSON-LD gives the triples of a nested node between those of the
        // node it is nested in: A's label comes before B's triples, its
        // `skos:prefLabel` after them.
        let none = Arc::new(PrefixMap::default());
        let json = r#"{"@id": "http://ex.org/A", "@type": "http://www.w3.org/2002/07/owl#Class",
          "http://www.w3.org/2000/01/rdf-schema#label": "pump ",
          "http://www.w3.org/2000/01/rdf-schema#seeAlso": {"@id": "http://ex.org/B",
            "@type": "http://www.w3.org/2002/07/owl#Class",
            "http://www.w3.org/2000/01/rdf-schema#label": "Pump"},
          "http://www.w3.org/2004/02/skos/core#prefLabel": "Pumpe"}"#;
        let mut k = Knowledge::default();
        let uri = "file:///j.jsonld";
        let a = analysis(uri, json, &none);
        let statements: Vec<u32> = a.parsed.triples.iter().map(|t| t.statement).collect();
        assert!(!statements.is_sorted(), "{statements:?}");
        k.set(uri, Source::Open, a);
        assert_eq!(names(k.labelled("PUMP", None)), ["ex:A", "ex:B"]);
        assert_eq!(names(k.labelled("pumpe", None)), ["ex:A"]);
    }

    #[test]
    fn every_described_or_typed_subject_counts_as_a_subject() {
        // The subjects are read from the curation summaries, which cover
        // these: a lookup by subject must find each.
        let none = Arc::new(PrefixMap::default());
        for (i, text) in fixtures().iter().enumerate() {
            let a = analysis(&format!("file:///f{i}.ttl"), text, &none);
            let f = DocFacts::new(&a);
            let has = |iri: &Iri| {
                f.subjects
                    .binary_search_by_key(&iri.id(), |c| c.0.id())
                    .is_ok()
            };
            for iri in a.defs.keys().chain(a.types.keys()).chain(a.supers.keys()) {
                assert!(has(iri), "fixture {i}: {iri}");
            }
        }
    }

    /// What other documents' checks read of `k` about each item a change
    /// set can name: per IRI, per text key, per namespace, per prefix name.
    #[derive(Debug)]
    struct Read {
        iris: Vec<(Iri, IriRead)>,
        texts: Vec<((Text, u64), Collisions)>,
        namespaces: Vec<(Iri, bool)>,
        prefixes: Vec<(Iri, Vec<Iri>)>,
    }

    /// Of an IRI: described, a term, deprecated here, a term with a
    /// `skos:definition`, its [`TermState`].
    type IriRead = (bool, bool, bool, bool, TermState);

    /// Every item a change of the versions `fs` of a source could name.
    fn items(fs: &[&DocFacts]) -> Changes {
        let (mut iris, mut texts, mut names) = (Vec::new(), Vec::new(), Vec::new());
        for f in fs {
            let counted = [
                &f.subjects,
                &f.objects,
                &f.refs,
                &f.defs,
                &f.terms,
                &f.defined,
            ];
            iris.extend(counted.into_iter().flatten().map(|c| c.0));
            iris.extend(f.deprecated.iter().map(|d| d.0));
            iris.extend(f.curated.iter().map(|c| c.0));
            for index in Index::ALL {
                texts.extend(index.of(f).iter().map(|k| (index.text(), k.key)));
            }
            names.extend(f.bindings.iter().map(|b| b.0.0));
        }
        let mut namespaces: Vec<Iri> = iris
            .iter()
            .filter_map(|i| Iri::get(namespace_of(i)))
            .collect();
        for v in [&mut iris, &mut namespaces, &mut names] {
            v.sort_by_key(|i| i.id());
            v.dedup();
        }
        texts.sort();
        texts.dedup();
        Changes {
            iris,
            texts,
            namespaces,
            prefixes: names,
        }
    }

    fn read(k: &Knowledge, items: &Changes) -> Read {
        let state = |i: Iri| {
            let defined = k.marks.get(&i).is_some_and(|m| m.defined > 0);
            (
                k.is_described(&i),
                k.is_term(&i),
                k.deprecated_here(&i),
                k.is_term(&i) && defined,
                k.term_state(i),
            )
        };
        let bound = |name: Iri| {
            let mut ns: Vec<Iri> = k.bindings(&name).into_iter().map(|b| b.0).collect();
            ns.sort();
            ns
        };
        Read {
            iris: items.iris.iter().map(|&i| (i, state(i))).collect(),
            texts: items
                .texts
                .iter()
                .map(|&t| (t, k.collisions(t.0, t.1)))
                .collect(),
            namespaces: items
                .namespaces
                .iter()
                .map(|&n| (n, k.is_own_namespace(&n)))
                .collect(),
            prefixes: items.prefixes.iter().map(|&n| (n, bound(n))).collect(),
        }
    }

    /// The items whose reads differ between `x` and `y`.
    fn moved<T: Copy, S: PartialEq>(x: &[(T, S)], y: &[(T, S)]) -> Vec<T> {
        x.iter()
            .zip(y)
            .filter(|(a, b)| a.1 != b.1)
            .map(|(a, _)| a.0)
            .collect()
    }

    #[test]
    fn the_change_set_names_what_changed_for_other_documents() {
        let texts = fixtures();
        let ext = [Arc::new(PrefixMap::default()), project_prefixes()];
        let mut seen = [0usize; 4];
        for seed in 1..=6u64 {
            let mut rng = Rng(0x2545_F491_4F6C_DD1D ^ seed);
            let mut k = Knowledge::default();
            for step in 0..120 {
                let slot = rng.below(6);
                let external = slot >= 5;
                let uri = format!("file:///ws/d{slot}.ttl");
                let key = if external {
                    Key::External(uri.clone())
                } else {
                    Key::Workspace(uri.clone())
                };
                let a = (rng.below(4) != 0).then(|| {
                    let text = variant(&mut rng, &texts);
                    analysis(&uri, &text, &ext[rng.below(2)])
                });
                let new = a.as_ref().map(|a| DocFacts::new(a));
                let old = k.by_key.get(&key).map(|&id| &k.slot(id).facts);
                let all = items(&old.into_iter().chain(new.as_ref()).collect::<Vec<_>>());
                let before = read(&k, &all);
                let (generation, last) = (k.generation(), k.changes().iris.clone());
                match (a, external) {
                    (Some(a), true) => k.add_external(&uri, a),
                    (Some(a), false) => k.set(&uri, Source::Disk, a),
                    (None, true) => k.remove_external(&uri),
                    (None, false) => k.remove(&uri),
                }
                let after = read(&k, &all);
                let c = k.changes();
                let at = format!("seed {seed} step {step}");
                if k.generation() == generation {
                    // Nothing changed: the set stays the last change's.
                    assert!(moved(&before.iris, &after.iris).is_empty(), "{at}");
                    assert_eq!(c.iris, last, "{at}");
                    continue;
                }
                assert_eq!(c.iris, moved(&before.iris, &after.iris), "{at}");
                assert_eq!(
                    c.namespaces,
                    moved(&before.namespaces, &after.namespaces),
                    "{at}"
                );
                assert_eq!(c.prefixes, moved(&before.prefixes, &after.prefixes), "{at}");
                let texts = moved(&before.texts, &after.texts);
                for t in &c.texts {
                    assert!(texts.contains(t), "{at}: {t:?} listed, unchanged");
                }
                for (was, now) in before.texts.iter().zip(&after.texts) {
                    if was.1 != now.1 && !c.texts.contains(&was.0) {
                        let t = was.0;
                        // Then an IRI under it is listed.
                        let named = was.1.iter().chain(&now.1).any(|x| c.iris.contains(&x.2));
                        assert!(named, "{at}: {t:?} changed, not named");
                    }
                }
                for (n, v) in seen.iter_mut().zip([
                    c.iris.len(),
                    c.texts.len(),
                    c.namespaces.len(),
                    c.prefixes.len(),
                ]) {
                    *n += v;
                }
            }
        }
        // Each kind of item was named somewhere.
        assert!(seen.iter().all(|&n| n > 0), "{seen:?}");
    }

    #[test]
    fn labelled_folds_case_and_trims_and_keeps_languages_apart() {
        let mut k = Knowledge::default();
        put_ttl(
            &mut k,
            "a",
            "ex:A a owl:Class ; rdfs:label \" Pump\\t\"@en, \"Pumpe\"@de .\n\
             ex:B a owl:Class ; skos:prefLabel \"PUMP\"@EN .\n\
             ex:C a owl:Class ; rdfs:label \"pump\" .\n\
             ex:alice rdfs:label \"Pump\"@en .\n\
             ex:D a owl:Class ; rdfs:comment \"Pump\"@en .\n",
        );
        put_ttl(
            &mut k,
            "b",
            "ex:E rdfs:subClassOf ex:A ; rdfs:label \"pump \"@en .\n",
        );
        let en = names(k.labelled("PUMP", Some("En")));
        assert_eq!(en, ["ex:A", "ex:B", "ex:E"]);
        assert_eq!(names(k.labelled(" pump\n", None)), ["ex:C"]);
        assert_eq!(names(k.labelled("pumpe", Some("de"))), ["ex:A"]);
        assert!(k.labelled("pumpe", Some("en")).is_empty());
        assert!(k.labelled("Pump", Some("xx-never-seen")).is_empty());
        // A term that repeats its label is listed once.
        put_ttl(&mut k, "c", "ex:A rdfs:label \"pump\"@en .\n");
        assert_eq!(
            names(k.labelled("Pump", Some("en"))),
            ["ex:A", "ex:B", "ex:E"]
        );
    }

    #[test]
    fn labels_subtract_exactly_on_replace_and_remove() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "a", "ex:A a owl:Class ; rdfs:label \"Pump\" .\n");
        put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Pump\" .\n");
        assert_eq!(names(k.labelled("pump", None)), ["ex:A", "ex:B"]);
        put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
        assert_eq!(names(k.labelled("pump", None)), ["ex:A"]);
        assert_eq!(names(k.labelled("valve", None)), ["ex:B"]);
        // The label stays while the type moves to another document.
        put_ttl(&mut k, "c", "ex:A a owl:Class .\n");
        put_ttl(&mut k, "a", "ex:A rdfs:label \"Pump\" .\n");
        assert_eq!(names(k.labelled("pump", None)), ["ex:A"]);
        k.remove("file:///c.ttl");
        assert!(k.labelled("pump", None).is_empty());
        for d in ["a", "b"] {
            k.remove(&format!("file:///{d}.ttl"));
        }
        assert!(k.snapshot().is_empty(), "{:?}", k.snapshot());
    }

    #[test]
    fn a_label_a_document_gives_twice_stays_until_both_go() {
        let mut k = Knowledge::default();
        let a = "ex:A a owl:Class ; rdfs:label \"Pump\" .\n";
        let b = "ex:B a owl:Class ; rdfs:label \"Pump\" .\n";
        let x = "ex:x rdfs:label \"Other\" .\n";
        put_ttl(&mut k, "a", &format!("{a}{x}{b}"));
        assert_eq!(names(k.labelled("pump", None)), ["ex:A", "ex:B"]);
        // Changes at the start, in the middle and at the end.
        let changes: [(String, &[&str]); 3] = [
            (format!("{x}{b}"), &["ex:B"]),
            (format!("{a}{x}{a}{b}"), &["ex:A", "ex:B"]),
            (format!("{a}{x}"), &["ex:A"]),
        ];
        for (body, want) in changes {
            put_ttl(&mut k, "a", &body);
            assert_eq!(names(k.labelled("pump", None)), want, "{body}");
        }
        put_ttl(&mut k, "a", x);
        assert!(k.labelled("pump", None).is_empty());
        k.remove("file:///a.ttl");
        assert!(k.snapshot().is_empty(), "{:?}", k.snapshot());
    }

    /// 400 documents of instance data that share their comments: adding
    /// one compares no collisions, as no comment is about a term.
    #[cfg(not(debug_assertions))]
    #[test]
    fn timing_shared_comments_are_not_quadratic() {
        let none = Arc::new(PrefixMap::default());
        let docs: Vec<(String, Arc<Analysis>)> = (0..400)
            .map(|f| {
                let mut text = String::from(CURATION_HEADER);
                for i in 0..100 {
                    text += &format!("ex:s{f}_{i} rdfs:comment \"Shared {i}\" .\n");
                }
                let uri = format!("file:///w/{f}.ttl");
                let a = analysis(&uri, &text, &none);
                (uri, a)
            })
            .collect();
        let mut k = Knowledge::default();
        let start = std::time::Instant::now();
        for (uri, a) in docs {
            k.set(&uri, Source::Disk, a);
        }
        let took = start.elapsed();
        assert!(took.as_millis() < 200, "took {took:?}");
    }

    /// A vocabulary of `n` classes, four triples each (the shape of the
    /// `big-vocab` benchmark), with `edit` appended to the label of class
    /// `n / 2`.
    fn vocabulary(n: usize, edit: &str) -> String {
        let mut out = String::from(CURATION_HEADER);
        for i in 0..n {
            let e = if i == n / 2 { edit } else { "" };
            out += &format!(
                "\nex:C{i} a owl:Class ;\n    rdfs:label \"Class {i}{e}\"@en ;\n    \
                 skos:definition \"A class numbered {i}.\"@en ;\n    rdfs:subClassOf ex:C{} .\n",
                (i + 1) % n
            );
        }
        out
    }

    /// Typing in a label of a 17,000-class vocabulary. Swapping the new
    /// facts in walks each of the source's sorted lists once beside the old
    /// one (about ten lists of 17,000 to 34,000 entries, compared without a
    /// lookup) and looks up only the two text keys that changed; removing
    /// the source and adding it again looks every IRI and key up, and
    /// checks every key's collisions, twice. On an M-series laptop: about
    /// 0.12 ms against 4.8 + 1.4 ms (2%); swapping by removing and adding
    /// cost 1.75 ms before. What limits the swap is the memory those walks
    /// read; the analyses stay alive in `versions`, so no drop is timed.
    #[cfg(not(debug_assertions))]
    #[test]
    fn timing_a_label_edit_swaps_in_a_fraction_of_a_rebuild() {
        let none = Arc::new(PrefixMap::default());
        let uri = "file:///vocab.ttl";
        let n = 17_000;
        let versions: Vec<Arc<Analysis>> = (0..9)
            .map(|i| analysis(uri, &vocabulary(n, &"x".repeat(i)), &none))
            .collect();
        let mut k = Knowledge::default();
        k.set(uri, Source::Open, versions[0].clone());
        let (mut swap, mut full) = (Vec::new(), Vec::new());
        for a in &versions[1..] {
            let facts = DocFacts::new(a);
            let t = std::time::Instant::now();
            k.set_prepared(uri, Source::Open, a.clone(), facts);
            swap.push(t.elapsed());
            let facts = DocFacts::new(a);
            let t = std::time::Instant::now();
            k.remove(uri);
            k.set_prepared(uri, Source::Open, a.clone(), facts);
            full.push(t.elapsed());
        }
        // The swaps did their work: the index is the one a rebuild makes.
        let edited = format!("Class {}xxxxxxxx", n / 2);
        assert_eq!(
            names(k.labelled(&edited, Some("en"))),
            [format!("ex:C{}", n / 2)]
        );
        let mut fresh = Knowledge::default();
        fresh.set(uri, Source::Open, versions[8].clone());
        assert_eq!(k.snapshot(), fresh.snapshot());
        swap.sort();
        full.sort();
        let (swap, full) = (swap[swap.len() / 2], full[full.len() / 2]);
        assert!(swap * 10 < full, "swap {swap:?}, remove and add {full:?}");
        assert!(swap.as_micros() < 1_000, "swap {swap:?}");
    }

    #[test]
    fn defined_as_falls_back_to_the_comment() {
        let mut k = Knowledge::default();
        put_ttl(
            &mut k,
            "a",
            "ex:A a owl:Class ; skos:definition \"A machine.\"@en ; rdfs:comment \"Moves water.\"@en .\n\
             ex:B a owl:Class ; rdfs:comment \"a machine. \"@en .\n\
             ex:C a owl:Class ; rdfs:comment \"Moves water.\"@en .\n\
             ex:x rdfs:comment \"Moves water.\"@en .\n",
        );
        assert_eq!(
            names(k.defined_as("A MACHINE.", Some("en"))),
            ["ex:A", "ex:B"]
        );
        // A's comment is not its definition: it has a skos:definition.
        assert_eq!(names(k.defined_as("moves water.", Some("en"))), ["ex:C"]);
        // Once another document gives C a skos:definition, its comment stops counting.
        put_ttl(&mut k, "b", "ex:C skos:definition \"Other.\"@en .\n");
        assert!(k.defined_as("moves water.", Some("en")).is_empty());
        assert_eq!(names(k.defined_as("other.", Some("en"))), ["ex:C"]);
        k.remove("file:///b.ttl");
        assert_eq!(names(k.defined_as("moves water.", Some("en"))), ["ex:C"]);
    }

    #[test]
    fn deprecated_here_reads_boolean_true_and_false_wins() {
        let mut k = Knowledge::default();
        put_ttl(
            &mut k,
            "a",
            "ex:a owl:deprecated true .\nex:b owl:deprecated \"true\"^^xsd:boolean .\n\
             ex:c owl:deprecated false .\nex:d owl:deprecated \"true\" .\nex:e owl:deprecated true .\n",
        );
        let none = Arc::new(PrefixMap::default());
        let json =
            r#"{"@id": "http://ex.org/j", "http://www.w3.org/2002/07/owl#deprecated": true}"#;
        k.set(
            "file:///j.jsonld",
            Source::Disk,
            analysis("file:///j.jsonld", json, &none),
        );
        let dep = |k: &Knowledge, l: &str| k.deprecated_here(&format!("http://ex.org/{l}"));
        for l in ["a", "b", "j", "e"] {
            assert!(dep(&k, l), "{l}");
        }
        // False, and a plain string, are not deprecations.
        assert!(!dep(&k, "c") && !dep(&k, "d"));
        // A false anywhere in the workspace wins.
        put_ttl(&mut k, "b", "ex:e owl:deprecated false .\n");
        assert!(!dep(&k, "e"));
        k.remove("file:///b.ttl");
        assert!(dep(&k, "e"));
        // An external source's deprecation is not the workspace's.
        let ext = format!("{CURATION_HEADER}ex:x owl:deprecated true .\n");
        k.add_external("import:x", analysis("import:x", &ext, &none));
        assert!(!dep(&k, "x"));
    }

    #[test]
    fn described_generation_moves_on_label_collisions_and_curation_facts() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
        let set = |k: &mut Knowledge, body: &str| {
            put_ttl(k, "a", body);
            k.described_generation()
        };
        let x = "ex:x rdfs:label \"Other\" .\n";
        let g = set(
            &mut k,
            &format!("{x}ex:A a owl:Class ; rdfs:label \"Pu\" .\n"),
        );
        // Typing inside a label that collides with nothing.
        for body in ["\"Pum\" .\n", "\"Pump\" .\n", "\"Pump\" . \n"] {
            let g2 = set(&mut k, &format!("{x}ex:A a owl:Class ; rdfs:label {body}"));
            assert_eq!(g2, g, "{body}");
        }
        // Instance data whose label collides is not a term.
        let body = "ex:x rdfs:label \"valve\" .\nex:A a owl:Class ; rdfs:label \"Pump\" .\n";
        assert_eq!(set(&mut k, body), g);
        // A new collision, and its end.
        let g2 = set(&mut k, "ex:A a owl:Class ; rdfs:label \"Valve\" .\n");
        assert!(g2 > g);
        // Editing a collision's case keeps it the same.
        assert_eq!(
            set(&mut k, "ex:A a owl:Class ; rdfs:label \"VALVE\" .\n"),
            g2
        );
        let g3 = set(&mut k, "ex:A a owl:Class ; rdfs:label \"Valv\" .\n");
        assert!(g3 > g2);
        // A new term, and a new own deprecation.
        let g4 = set(
            &mut k,
            "ex:A a owl:Class ; rdfs:label \"Valv\" .\nex:C a owl:Class .\n",
        );
        assert!(g4 > g3);
        let g5 = set(
            &mut k,
            "ex:A a owl:Class ; rdfs:label \"Valv\" .\nex:C a owl:Class ; owl:deprecated true .\n",
        );
        assert!(g5 > g4);
    }

    #[test]
    fn described_generation_moves_on_a_definition_colliding_with_a_comment() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:comment \"Same.\" .\n");
        put_ttl(
            &mut k,
            "a",
            "ex:A a owl:Class ; skos:definition \"Different.\" .\n",
        );
        let g = k.described_generation();
        // Only the definition's text changes: B's comment is its definition.
        put_ttl(
            &mut k,
            "a",
            "ex:A a owl:Class ; skos:definition \"same.\" .\n",
        );
        assert!(k.described_generation() > g);
    }

    #[test]
    fn described_generation_moves_when_a_colliding_label_goes_with_its_document() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Valve\" .\n");
        // A is typed, and labelled in another language, in c.ttl; d.ttl
        // only gives it the colliding label.
        put_ttl(
            &mut k,
            "c",
            "ex:A a owl:Class ; rdfs:label \"Ventil\"@de .\n",
        );
        put_ttl(&mut k, "d", "ex:A rdfs:label \"Valve\" .\n");
        let g = k.described_generation();
        k.remove("file:///d.ttl");
        assert!(k.described_generation() > g);
    }

    #[test]
    fn described_generation_moves_when_a_term_changes_kind() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "b", "ex:B a owl:Class ; rdfs:label \"Pump\" .\n");
        put_ttl(&mut k, "a", "ex:A a owl:Class ; rdfs:label \"Pump\" .\n");
        let g = k.described_generation();
        put_ttl(
            &mut k,
            "a",
            "ex:A a owl:ObjectProperty ; rdfs:label \"Pump\" .\n",
        );
        let g2 = k.described_generation();
        assert!(g2 > g);
        assert_eq!(k.term_kind("http://ex.org/A"), TERM_PROPERTY);
        // A sub-property statement in another document adds to the kind.
        put_ttl(&mut k, "c", "ex:B rdfs:subPropertyOf ex:p .\n");
        assert!(k.described_generation() > g2);
        assert_eq!(k.term_kind("http://ex.org/B"), TERM_CLASS | TERM_PROPERTY);
        // An external source says nothing about kinds.
        let none = Arc::new(PrefixMap::default());
        let ext = format!("{CURATION_HEADER}ex:A a skos:Concept .\n");
        k.add_external("import:e", analysis("import:e", &ext, &none));
        assert_eq!(k.term_kind("http://ex.org/A"), TERM_PROPERTY);
    }

    #[test]
    fn described_generation_moves_when_another_document_labels_or_defines_a_term() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "z", "ex:A a owl:Class .\n");
        put_ttl(&mut k, "b", "ex:A rdfs:seeAlso ex:x .\n");
        let set = |k: &mut Knowledge, body: &str| {
            put_ttl(k, "b", &format!("ex:A rdfs:seeAlso ex:x{body} .\n"));
            k.described_generation()
        };
        let g = k.described_generation();
        let g1 = set(&mut k, " ; rdfs:label \"A\"");
        assert!(g1 > g);
        // Typing inside the label.
        assert_eq!(set(&mut k, " ; rdfs:label \"Ab\""), g1);
        assert_eq!(set(&mut k, " ; rdfs:label \"Abc\"@en"), g1);
        let g2 = set(&mut k, " ; rdfs:label \"Abc\" ; rdfs:comment \"A\"");
        assert!(g2 > g1);
        // Typing inside the comment.
        assert_eq!(
            set(&mut k, " ; rdfs:label \"Abc\" ; rdfs:comment \"An a\""),
            g2
        );
        // The comment goes.
        assert!(set(&mut k, " ; rdfs:label \"Abc\"") > g2);
    }

    #[test]
    fn described_generation_moves_when_the_reporting_document_changes() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "z", "ex:D a owl:Class .\n");
        put_ttl(&mut k, "a", "ex:D rdfs:seeAlso ex:x .\n");
        let g = k.described_generation();
        // a.ttl, before z.ttl by URI, starts declaring D: it reports now.
        put_ttl(&mut k, "a", "ex:D rdfs:subClassOf ex:x .\n");
        assert!(k.described_generation() > g);
    }

    #[test]
    fn described_generation_moves_when_an_ontology_header_changes_elsewhere() {
        let mut k = Knowledge::default();
        put_ttl(&mut k, "o", "ex:onto a owl:Ontology .\n");
        put_ttl(&mut k, "h", "ex:onto rdfs:seeAlso ex:x .\n");
        let set = |k: &mut Knowledge, body: &str| {
            put_ttl(k, "h", &format!("ex:onto rdfs:seeAlso ex:x{body} .\n"));
            k.described_generation()
        };
        let g = k.described_generation();
        let license = " ; <http://purl.org/dc/terms/license> <http://l>";
        let g1 = set(&mut k, license);
        assert!(g1 > g);
        // Another licence changes nothing the checks read.
        assert_eq!(set(&mut k, &format!("{license}, <http://m>")), g1);
        let g2 = set(&mut k, &format!("{license} ; rdfs:label \"O\""));
        assert!(g2 > g1);
        assert_eq!(set(&mut k, &format!("{license} ; rdfs:label \"Onto\"")), g2);
        // An ontology typed in a document before o.ttl by URI moves the report.
        put_ttl(&mut k, "a", "ex:onto a owl:Ontology .\n");
        assert!(k.described_generation() > g2);
    }

    /// A shape as lines of text: its node, statement and targets, then one
    /// line per property constraint, with spans as the text they cover.
    fn describe(text: &str, s: &Shape) -> Vec<String> {
        let at = |sp: Span| &text[sp.start as usize..sp.end as usize];
        let short = |i: &Iri| i.replace("http://example.com/", "ex:");
        let mut out = vec![format!(
            "{} {:?} targets={:?}",
            s.node.as_ref().map_or("[]".into(), short),
            at(s.span).lines().next().unwrap_or(""),
            s.targets.iter().map(short).collect::<Vec<_>>(),
        )];
        for c in &s.properties {
            out.push(format!(
                "{:?} path={:?} min={:?} max={:?} class={:?} datatype={:?} in={:?}",
                at(c.span),
                c.path.as_ref().map(short),
                c.min_count,
                c.max_count,
                c.class.iter().map(short).collect::<Vec<_>>(),
                c.datatype.as_ref().map(short),
                c.in_values,
            ));
        }
        out
    }

    /// `sh:property [ ]` on a class-targeting node shape is an empty
    /// constraint of it; a node shape without a target targets nothing, nor
    /// do its property shapes. Pinned before 33c.
    #[test]
    fn property_shapes_of_targeting_and_untargeted_shapes() {
        let none = Arc::new(PrefixMap::default());
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.com/> .
ex:C a rdfs:Class .
ex:S a sh:NodeShape ; sh:targetClass ex:C ;
  sh:property [ sh:path ex:p ; sh:minCount 1 ; sh:class ex:K ] , [ ] .
ex:U a sh:NodeShape ;
  sh:property [ sh:path ex:q ; sh:minCount 1 ] .
ex:V sh:property [ sh:path ex:r ; sh:maxCount 2 ] .
";
        let mut k = Knowledge::default();
        k.set("file:///s.ttl", Source::Open, analysis("s", text, &none));
        let found: Vec<String> = k
            .shapes_targeting("http://example.com/C")
            .iter()
            .flat_map(|(_, s)| describe(text, s))
            .collect();
        assert_eq!(
            found,
            [
                r#"ex:S "ex:S a sh:NodeShape ; sh:targetClass ex:C ;" targets=["ex:C"]"#,
                r#""[ sh:path ex:p ; sh:minCount 1 ; sh:class ex:K ]" path=Some("ex:p") min=Some(1) max=None class=["ex:K"] datatype=None in=None"#,
                r#""[ ]" path=None min=None max=None class=[] datatype=None in=None"#,
            ]
        );
        for class in ["U", "V", "p", "q", "r", "K", "S"] {
            let iri = format!("http://example.com/{class}");
            assert!(k.shapes_targeting(&iri).is_empty(), "{class}");
        }
        assert_eq!(k.slot(0).facts.shapes.len(), 1);
    }

    /// The [`Shape`]s of `a` as the SHACL model has them: the shapes of
    /// [`model::Shapes`] over a graph of the document's triples alone,
    /// narrowed by the definition on [`Shape`], in the order the document
    /// first names each one's target class or types it `sh:NodeShape`. The
    /// builder must find exactly these; it reads the parse because this
    /// costs more per edit (owner decision 8). Returns the shapes and the
    /// shape indexes by target class.
    fn shapes_from_the_model(a: &Analysis) -> (Vec<Shape>, IriMap<Vec<u32>>) {
        use crate::rdf::graph::{Graph, Id, Term};
        use crate::shacl::model;
        use crate::shacl::vocab::Param;
        use crate::syntax::Triple;
        fn is_node(g: &Graph, id: Id) -> bool {
            matches!(g.term(id), Term::Iri(_) | Term::Blank(_))
        }
        // Every rdf:first of a cell, the last rdf:rest, at most 10,000 cells.
        fn in_list<'t>(g: &Graph, doc: &dyn Fn(u32) -> &'t Triple, head: Id) -> Vec<Node> {
            let v = vocab::get();
            let mut out = Vec::new();
            let mut cur = head;
            for _ in 0..10_000 {
                if !is_node(g, cur) || g.as_iri(cur) == Some(v.rdf_nil) {
                    break;
                }
                let mut next = None;
                for (i, t) in g.out(cur) {
                    if t.p == v.rdf_first {
                        out.push(doc(i).o.clone());
                    } else if t.p == v.rdf_rest {
                        next = Some(t.o);
                    }
                }
                match next {
                    Some(n) => cur = n,
                    None => break,
                }
            }
            out
        }
        let v = vocab::get();
        let triples = &a.parsed.triples;
        // As the lint's model: a triple with an undeclared prefix counts.
        let all: std::borrow::Cow<[Triple]> = if triples.iter().any(|t| t.unresolved) {
            let cleared = |t: &Triple| Triple {
                unresolved: false,
                ..t.clone()
            };
            triples.iter().map(cleared).collect()
        } else {
            triples.into()
        };
        let g = Graph::new(&[&*all]);
        let shapes = model::Shapes::new(&g);
        let doc = |n: u32| &triples[g.origin(n).1 as usize];
        let constraint = |shape: Id, span: Span| {
            let mut c = PropertyConstraint {
                path: None,
                min_count: None,
                max_count: None,
                class: Vec::new(),
                datatype: None,
                in_values: None,
                span,
            };
            for &i in shapes.get(shape).map_or(&[][..], |s| s.params.as_slice()) {
                let t = doc(i);
                let count = || match &t.o {
                    Node::Literal { value, .. } => value.trim().parse::<u32>().ok(),
                    _ => None,
                };
                if t.p == v.path {
                    if let Node::Iri(o) = &t.o {
                        c.path = Some(*o);
                    }
                    continue;
                }
                match (v.param(t.p), &t.o) {
                    (Some(Param::Class), Node::Iri(o)) => c.class.push(*o),
                    (Some(Param::Datatype), Node::Iri(o)) => c.datatype = Some(*o),
                    (Some(Param::MinCount), _) => c.min_count = count(),
                    (Some(Param::MaxCount), _) => c.max_count = count(),
                    (Some(Param::In), _) => c.in_values = Some(in_list(&g, &doc, g.triple_at(i).o)),
                    _ => {}
                }
            }
            c
        };
        let mut kept: Vec<(u32, Shape)> = Vec::new();
        for s in &shapes.shapes {
            let iri = g.as_iri(s.node);
            // The first triple that names a target class, or the type
            // sh:NodeShape of an IRI.
            let mut first = u32::MAX;
            let mut targets = Vec::new();
            for &i in &s.targets {
                let t = g.triple_at(i);
                if t.p == v.target_class {
                    first = first.min(i);
                    targets.extend(g.as_iri(t.o));
                }
            }
            if let Some(iri) = iri
                && s.typed_node_shape
            {
                let mut is_class = false;
                for (i, t) in g.out(s.node).filter(|(_, t)| t.p == v.rdf_type) {
                    match g.as_iri(t.o) {
                        Some(o) if o == v.node_shape => first = first.min(i),
                        Some(o) if o == v.rdfs_class || o == v.owl_class => is_class = true,
                        _ => {}
                    }
                }
                if is_class {
                    targets.push(iri);
                }
            }
            if targets.is_empty() {
                continue;
            }
            targets.sort_unstable();
            targets.dedup();
            let properties = s
                .params
                .iter()
                .map(|&i| (i, g.triple_at(i)))
                .filter(|(_, t)| t.p == v.property && is_node(&g, t.o))
                .map(|(i, t)| constraint(t.o, doc(i).o_span))
                .collect();
            let span = g
                .out(s.node)
                .next()
                .and_then(|(i, _)| a.parsed.statements.get(doc(i).statement as usize))
                .map_or_else(Span::default, |st| st.span);
            let shape = Shape {
                node: iri,
                span,
                targets,
                properties,
            };
            kept.push((first, shape));
        }
        kept.sort_unstable_by_key(|k| k.0);
        let mut by_target = IriMap::<Vec<u32>>::default();
        for (index, (_, shape)) in kept.iter().enumerate() {
            for &t in &shape.targets {
                by_target.entry(t).or_default().push(index as u32);
            }
        }
        (kept.into_iter().map(|k| k.1).collect(), by_target)
    }

    /// The builder's shapes of `text` are the model's.
    fn same_shapes_as_the_model(name: &str, text: &str, external: &Arc<PrefixMap>) -> usize {
        let a = Analysis::new(name.into(), text, external.clone(), false);
        let f = DocFacts::new(&a);
        let (shapes, by_target) = shapes_from_the_model(&a);
        assert_eq!(f.shapes, shapes, "{name}");
        assert_eq!(*f.shapes_by_target, *by_target, "{name}");
        shapes.len()
    }

    /// Shapes the narrower definition keeps or drops, and the corners of
    /// the builder's reading: blank and property shapes with `sh:targetClass`,
    /// literal targets, implicit class targets, repeated parameters,
    /// malformed and cyclic `sh:in` lists, an `sh:property` IRI with no
    /// triples, an undeclared prefix, and shapes that the model finds
    /// earlier than their target (by `sh:targetNode`, `sh:node`, `sh:path`).
    const CORNERS: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.com/> .
ex:Late sh:targetNode ex:x .
ex:Ref sh:node ex:Late2 .
ex:Late2 sh:class ex:K .
ex:PathFirst sh:path ex:p .
ex:A a sh:NodeShape ; sh:targetClass ex:C , ex:C , ex:D , \"lit\" ;
  sh:property [ sh:path ex:p ; sh:path ex:p2 ; sh:path ( ex:s1 ex:s2 ) ;
                sh:minCount 1 ; sh:minCount \"x\" ; sh:maxCount \" 3 \" ;
                sh:class ex:K1 , ex:K2 ; sh:datatype xsd:string ; sh:datatype \"no\" ;
                sh:in ( ex:a \"b\" 1 ) ; sh:in ( ex:c ) ] ,
              [ ] , ex:Empty , ex:Named , \"literal\" ;
  sh:property [ sh:in _:cycle ] , [ sh:in _:forked ] , [ sh:in \"head\" ] , [ sh:in rdf:nil ] .
_:cycle rdf:first ex:loop ; rdf:rest _:cycle .
_:forked rdf:first ex:f1 , ex:f2 ; rdf:rest _:gap , rdf:nil .
_:gap rdf:rest ( ex:after ) .
ex:Named sh:path ex:named ; sh:maxCount 1 .
ex:Implicit a sh:NodeShape , rdfs:Class ; sh:property [ sh:path ex:i ] .
ex:OwlImplicit a owl:Class , sh:NodeShape .
ex:NotAClass a sh:NodeShape ; sh:property [ sh:path ex:n ] .
[ a sh:NodeShape , rdfs:Class ; sh:targetClass ex:B ; sh:property [ sh:path ex:b ] ] .
[ a sh:NodeShape , rdfs:Class ] .
[ sh:path ex:pp ; sh:targetClass ex:C ] .
ex:LiteralOnly sh:targetClass \"C\" .
ex:Late sh:targetClass ex:L ; sh:property ex:Named .
ex:Late2 sh:targetClass ex:L .
ex:PathFirst sh:targetClass ex:L .
ex:Undeclared sh:targetClass nope:C ; sh:property [ sh:path nope:p ; sh:class nope:K ] .
ex:Typed sh:targetClass ex:T .
ex:Typed a sh:NodeShape .
ex:Shared sh:targetClass ex:S1 ; sh:property _:shared .
ex:Shared2 sh:targetClass ex:S2 ; sh:property _:shared .
_:shared sh:path ex:sh ; sh:minCount 0 .
";

    #[test]
    fn the_builder_finds_the_shapes_of_the_model_that_target_classes() {
        let none = Arc::new(PrefixMap::default());
        let project = project_prefixes();
        assert_eq!(same_shapes_as_the_model("corners.ttl", CORNERS, &none), 12);
        // Every fixture, cut short and renamed as the incremental test does.
        let texts = fixtures();
        let mut found = 0;
        for (i, t) in texts.iter().enumerate() {
            for ext in [&none, &project] {
                found += same_shapes_as_the_model(&format!("f{i}.ttl"), t, ext);
            }
        }
        assert!(found > 0);
        let mut rng = Rng(0x5EED_33C0);
        let mut corners = vec![CORNERS.to_string()];
        corners.extend(texts.iter().cloned());
        for i in 0..300 {
            let t = variant(&mut rng, &corners);
            same_shapes_as_the_model(&format!("v{i}.ttl"), &t, &project);
        }
        // The other shape fixtures, in Turtle and JSON-LD.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
        for f in [
            "shacl/vocab.ttl",
            "nested-prefixes/shapes/person-shape.ttl",
            "chaos/select-shapes.ttl",
            "chaos/shapes.jsonld",
        ] {
            let text = std::fs::read_to_string(root.join(f)).unwrap();
            same_shapes_as_the_model(f, &text, &none);
        }
        // The public corpus, when it is downloaded (tools/corpus/fetch.sh). In
        // CI (`CI=true`) a missing file fails the test, as in
        // tests/support/corpus.rs, so a run that skipped it is not `ok`.
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
        let ci = std::env::var("CI").is_ok_and(|v| v == "true");
        for f in ["brick.ttl", "dcat-ap-shacl.ttl", "w3c-shacl.ttl"] {
            match std::fs::read_to_string(corpus.join(f)) {
                Ok(text) => {
                    same_shapes_as_the_model(f, &text, &none);
                }
                Err(e) => assert!(
                    !ci,
                    "test/corpus/{f}: {e} (run tools/corpus/fetch.sh, or npm run corpus); \
                     CI=true does not skip the corpus"
                ),
            }
        }
    }

    /// A shapes document of `n` node shapes with three property shapes each.
    fn shapes_document(n: usize) -> String {
        let mut text = String::from(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n",
        );
        for i in 0..n {
            text.push_str(&format!(
                "ex:S{i} a sh:NodeShape ; sh:targetClass ex:C{i} ;\n\
                 \x20 sh:property [ sh:path ex:p{i} ; sh:minCount 1 ; sh:datatype ex:d ] ,\n\
                 \x20   [ sh:path ex:q{i} ; sh:class ex:C ; sh:maxCount 2 ] ,\n\
                 \x20   [ sh:path ex:r{i} ; sh:in ( ex:a ex:b \"c\" ) ] .\n\
                 ex:x{i} a ex:C{i} ; ex:p{i} \"v\" ; ex:q{i} ex:x0 .\n"
            ));
        }
        text
    }

    /// Per edit, the shapes cost one pass over the parse and no graph; the
    /// model indexes every triple of the document into one.
    #[test]
    fn the_shapes_build_no_graph_per_edit() {
        use crate::rdf::graph::INDEXED;
        let none = Arc::new(PrefixMap::default());
        let a = Analysis::new("s.ttl".into(), &shapes_document(50), none, false);
        let indexed = || INDEXED.with(|c| c.get());
        let before = indexed();
        assert_eq!(DocFacts::new(&a).shapes.len(), 50);
        assert_eq!(indexed() - before, 0);
        assert_eq!(shapes_from_the_model(&a).0.len(), 50);
        assert_eq!(indexed() - before, a.parsed.triples.len() as u64);
    }

    /// The measurement behind owner decision 8, on demand
    /// (`cargo test --release --lib -- --ignored shapes_cost`): the builder
    /// against a graph of the document plus its model, the best of seven
    /// tries each. At 33c the model cost 2.2 to 3.8 times the builder (a
    /// 44,000-triple shapes document: 0.77 ms against 1.72 ms; Brick, 53,646
    /// triples: 0.52 ms against 1.26 ms), so the builder stays.
    #[test]
    #[ignore = "a timing, printed"]
    fn shapes_cost_against_the_model() {
        use crate::rdf::graph::Graph;
        let none = Arc::new(PrefixMap::default());
        let mut texts = vec![("generated".to_string(), shapes_document(2_000))];
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
        for f in ["brick.ttl", "dcat-ap-shacl.ttl"] {
            if let Ok(text) = std::fs::read_to_string(corpus.join(f)) {
                texts.push((f.to_string(), text));
            }
        }
        let best = |f: &dyn Fn()| {
            (0..7)
                .map(|_| {
                    let t0 = std::time::Instant::now();
                    f();
                    t0.elapsed()
                })
                .min()
                .unwrap()
        };
        let v = vocab::get();
        for (name, text) in &texts {
            let a = Analysis::new(name.clone(), text, none.clone(), false);
            let triples = &a.parsed.triples;
            let nodes: Vec<Subj> = triples
                .iter()
                .filter_map(|t| match (&t.s, &t.o) {
                    (s, _) if t.p == v.target_class => subj(s),
                    (Node::Iri(s), Node::Iri(o)) if t.p == v.rdf_type && *o == v.node_shape => {
                        Some(Subj::Iri(*s))
                    }
                    _ => None,
                })
                .collect();
            let builder = best(&|| {
                let mut f = DocFacts::default();
                f.shapes(&a, nodes.clone());
                std::hint::black_box(f);
            });
            let graph = best(&|| {
                std::hint::black_box(Graph::new(&[triples]));
            });
            let model = best(&|| {
                std::hint::black_box(shapes_from_the_model(&a));
            });
            let analysis = best(&|| {
                std::hint::black_box(Analysis::new(name.clone(), text, none.clone(), false));
            });
            eprintln!(
                "{name}: {} triples; builder {builder:?}; model {model:?} (graph {graph:?}); Analysis::new {analysis:?}",
                triples.len()
            );
        }
    }
}
