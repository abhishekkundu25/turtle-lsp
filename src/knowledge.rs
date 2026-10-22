//! The workspace knowledge index: what the whole workspace says about each
//! IRI, merged from every source (open documents, files on disk, and
//! read-only external sources such as imported ontologies).
//!
//! Each source's contribution ([`DocFacts`]) is computed once from its
//! analysis and kept, so replacing or removing a source subtracts exactly what
//! it added. A keystroke costs O(size of that document), never O(workspace).
//! Per IRI the index keeps posting lists of the sources that say something
//! about it; the facts themselves are read back from those sources' analyses
//! and facts, so the index stays small.

use crate::analysis::Analysis;
use crate::shacl::vocab;
use crate::syntax::{self, BlankId, Iri, IriKey, IriMap, Node, Role, Span};
use crate::vocab::VOCABS;
use rustc_hash::{FxHashMap, FxHashSet};
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
    domain: IriMap<Vec<Iri>>,
    range: IriMap<Vec<Iri>>,
    /// Super term -> direct sub terms stated here.
    subs: IriMap<Vec<Iri>>,
    /// Class -> resources typed with it here.
    instances: IriMap<Vec<Iri>>,
    shapes: Vec<Shape>,
    /// Target class -> indexes into `shapes`.
    shapes_by_target: IriMap<Vec<u32>>,
    /// (prefix name, namespace) in effect and used or declared here.
    bindings: Vec<(Iri, Iri)>,
    /// IRIs this source says something about as subject (sorted, unique).
    subjects: Vec<Iri>,
    /// IRIs that are the object of a type, super or target link here.
    objects: Vec<Iri>,
    /// IRIs used in predicate position here.
    predicates: Vec<Iri>,
}

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

#[derive(Default)]
struct Namespace {
    /// Every IRI in this namespace that some source mentions.
    terms: FxHashSet<Iri>,
    /// Distinct terms the workspace (not external sources) describes / references.
    described: u32,
    referenced: u32,
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
    namespaces: FxHashMap<Iri, Namespace>,
    /// Prefix name -> namespace -> number of workspace files.
    bindings: FxHashMap<Iri, FxHashMap<Iri, u32>>,
    generation: u64,
    /// See [`Knowledge::described_generation`].
    described_generation: u64,
    /// Scratch for one change: namespaces whose counts moved, with whether
    /// they were own namespaces before it.
    own_before: FxHashMap<Iri, bool>,
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

fn subj(n: &Node) -> Option<Subj> {
    match n {
        Node::Iri(i) => Some(Subj::Iri(*i)),
        Node::Blank(b) => Some(Subj::Blank(*b)),
        _ => None,
    }
}

fn unique(v: impl IntoIterator<Item = Iri>) -> Vec<Iri> {
    let set: FxHashSet<Iri> = v.into_iter().collect();
    set.into_iter().collect()
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
        for t in triples {
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
                f.instances.entry(o).or_default().push(s);
                if o == v.node_shape {
                    shape_nodes.push(Subj::Iri(s));
                }
            } else if t.p == v.sub_class_of || t.p == v.sub_property_of {
                f.subs.entry(o).or_default().push(s);
            } else if l.domain.contains(&t.p) {
                f.domain.entry(s).or_default().push(o);
            } else if l.range.contains(&t.p) {
                f.range.entry(s).or_default().push(o);
            }
        }
        if !shape_nodes.is_empty() {
            f.shapes(a, shape_nodes);
        }

        f.subjects = unique(
            a.defs
                .keys()
                .chain(a.types.keys())
                .chain(a.supers.keys())
                .chain(f.domain.keys())
                .chain(f.range.keys())
                .copied(),
        );
        f.objects = unique(
            f.instances
                .keys()
                .chain(f.subs.keys())
                .chain(f.shapes_by_target.keys())
                .copied(),
        );
        f.predicates = unique(
            a.parsed
                .refs
                .iter()
                .filter(|r| r.role == Role::Predicate)
                .map(|r| r.iri),
        );

        // Prefix bindings: declared here, or supplied from outside and used.
        let mut bindings: Vec<(Iri, Iri)> = a
            .parsed
            .prefixes
            .iter()
            .map(|d| (Iri::intern(&d.name), d.iri))
            .collect();
        let declared: FxHashSet<&str> = a.parsed.prefixes.iter().map(|d| d.name.as_str()).collect();
        let mut used: FxHashSet<Iri> = FxHashSet::default();
        for u in &a.parsed.prefix_uses {
            if u.resolved
                && !declared.contains(u.name.as_str())
                && used.insert(u.name)
                && let Some(ns) = a.external.get(u.name.as_str())
            {
                bindings.push((u.name, Iri::intern(ns)));
            }
        }
        bindings.sort_unstable_by(|x, y| {
            (x.0.as_str(), x.1.as_str()).cmp(&(y.0.as_str(), y.1.as_str()))
        });
        bindings.dedup();
        f.bindings = bindings;
        f
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
    /// or stops being an own namespace, or a prefix binding appears or goes.
    /// Ordinary keystrokes leave it alone.
    pub(crate) fn described_generation(&self) -> u64 {
        self.described_generation
    }

    /// Bump [`Knowledge::described_generation`] if a namespace changed sides.
    fn settle_namespaces(&mut self) {
        let mut flipped = false;
        for (ns, before) in self.own_before.drain() {
            let after = self.namespaces.get(&ns).is_some_and(Namespace::is_own);
            flipped |= before != after && VOCABS.by_namespace(&ns).is_none();
        }
        if flipped {
            self.described_generation += 1;
        }
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

    fn put(&mut self, key: Key, uri: &str, source: Source, a: Arc<Analysis>, facts: DocFacts) {
        let id = self.free.pop().unwrap_or_else(|| {
            self.slots.push(None);
            (self.slots.len() - 1) as DocId
        });
        let slot = Slot {
            uri: uri.to_string(),
            source,
            analysis: a,
            facts,
        };
        // Add the new version before removing the old one, so entries both
        // share (most of them, on a keystroke) are updated in place instead of
        // being dropped and rebuilt.
        self.apply(id, &slot, true);
        self.slots[id as usize] = Some(slot);
        if let Some(old) = self.by_key.insert(key, id) {
            self.release(old);
        }
        self.settle_namespaces();
        self.generation += 1;
    }

    fn take(&mut self, key: &Key) {
        if let Some(id) = self.by_key.remove(key) {
            self.release(id);
            self.settle_namespaces();
            self.generation += 1;
        }
    }

    fn release(&mut self, id: DocId) {
        let slot = self.slots[id as usize].take().expect("live doc id");
        self.apply(id, &slot, false);
        self.free.push(id);
    }

    /// Add (or subtract) one source's contribution.
    fn apply(&mut self, id: DocId, slot: &Slot, add: bool) {
        let a = &slot.analysis;
        let f = &slot.facts;
        let ws = slot.source != Source::External;
        let namespaces = &mut self.namespaces;
        let own_before = &mut self.own_before;
        let entries = &mut self.entries;
        let mut described_moved = false;
        for &iri in &f.subjects {
            toggle(&mut entry(entries, iri).subject, id, add);
        }
        for &iri in &f.objects {
            toggle(&mut entry(entries, iri).object, id, add);
        }
        for &iri in &f.predicates {
            step(&mut entry(entries, iri).predicate, add);
        }
        for &iri in a.refs_by_iri.keys() {
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
        }
        for &iri in a.defs.keys() {
            let e = entry(entries, iri);
            let ns = e.ns.expect("set on creation");
            let was = e.described_ws + e.described_ext > 0;
            if !ws {
                step(&mut e.described_ext, add);
            } else if step(&mut e.described_ws, add) {
                ns_step(namespaces, own_before, ns, add, |n| &mut n.described);
            }
            described_moved |= was != (e.described_ws + e.described_ext > 0);
        }
        if ws {
            for &(name, ns) in &f.bindings {
                let by_ns = self.bindings.entry(name).or_default();
                let n = by_ns.entry(ns).or_default();
                if step(n, add) {
                    described_moved = true;
                    if !add {
                        by_ns.remove(&ns);
                        if by_ns.is_empty() {
                            self.bindings.remove(&name);
                        }
                    }
                }
            }
        }
        if described_moved {
            self.described_generation += 1;
        }
        if !add {
            let touched = f
                .subjects
                .iter()
                .chain(&f.objects)
                .chain(&f.predicates)
                .chain(a.refs_by_iri.keys())
                .chain(a.defs.keys());
            for iri in touched {
                if let Some(e) = self.entries.get(iri)
                    && e.is_empty()
                {
                    let ns = e.ns;
                    self.entries.remove(iri);
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
        self.collect_objects(iri, |f, k| f.subs.get(&k).map(Vec::as_slice))
    }

    /// Resources typed with this class (workspace only).
    pub(crate) fn instances<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect_objects(iri, |f, k| f.instances.get(&k).map(Vec::as_slice))
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
                    .get(&key)
                    .into_iter()
                    .chain(f.subs.get(&key))
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
        let mut v = self.collect_subject_facts(iri, |f, k| f.domain.get(&k).map(Vec::as_slice));
        with_vocab(&mut v, iri, |t| &t.domain);
        v
    }

    /// rdfs:range from the workspace, then from well-known vocabularies.
    pub(crate) fn ranges<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect_subject_facts(iri, |f, k| f.range.get(&k).map(Vec::as_slice));
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
    #[cfg(test)]
    fn snapshot(&self) -> Vec<String> {
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
                    e.predicate
                )
            })
            .collect();
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
        // The public corpus, when it is downloaded (tools/corpus/fetch.sh).
        let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus");
        for f in ["brick.ttl", "dcat-ap-shacl.ttl", "w3c-shacl.ttl"] {
            if let Ok(text) = std::fs::read_to_string(corpus.join(f)) {
                same_shapes_as_the_model(f, &text, &none);
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
