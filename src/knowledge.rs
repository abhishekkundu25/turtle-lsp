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
//! property, which documents declare it) each source keeps as a [`Curated`](facts::Curated)
//! summary per IRI.

use crate::analysis::Analysis;
use crate::syntax::{Iri, IriMap};
use rustc_hash::{FxHashMap, FxHashSet};
use std::hash::Hash;
use std::sync::Arc;

mod counts;
use counts::Posting;
mod facts;
pub(crate) use facts::{
    DocFacts, PropertyConstraint, Shape, TERM_CLASS, TERM_CONCEPT, TERM_PROPERTY,
    header_properties, text_key,
};
mod read;
pub(crate) use read::Description;
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
    /// Sources whose [`Curated`](facts::Curated) summary types it `owl:Ontology`.
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
    /// became or stopped being deprecated here; IRIs whose [`TermState`](facts::TermState)
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
}

#[cfg(test)]
mod tests;
