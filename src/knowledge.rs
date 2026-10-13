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

use crate::analysis::{Analysis, RDFS_SUBCLASS_OF, RDFS_SUBPROPERTY_OF};
use crate::syntax::{self, BlankId, Iri, IriKey, IriMap, Node, Role, Span};
use crate::vocab::VOCABS;
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, OnceLock};

/// A source in the index. Ids are reused after removal.
pub type DocId = u32;

/// Where a source comes from. Open documents win over their disk copy (the
/// workspace registers one or the other per URI); external sources contribute
/// facts but are never diagnosed or edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Source {
    Open,
    Disk,
    External,
}

/// A SHACL node shape that targets classes (`sh:targetClass`, or implicitly a
/// class that is also a `sh:NodeShape`).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    /// The shape's IRI; `None` for a blank-node shape.
    pub node: Option<Iri>,
    /// The statement that describes the shape.
    pub span: Span,
    pub targets: Vec<Iri>,
    pub properties: Vec<PropertyConstraint>,
}

/// One `sh:property` of a [`Shape`].
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyConstraint {
    /// `sh:path` when it is a single predicate IRI.
    pub path: Option<Iri>,
    pub min_count: Option<u32>,
    pub max_count: Option<u32>,
    pub class: Vec<Iri>,
    pub datatype: Option<Iri>,
    /// The `sh:in` list, if any.
    pub in_values: Option<Vec<Node>>,
    /// The property shape (the object of `sh:property`).
    pub span: Span,
}

/// One source's description of an IRI.
pub struct Description<'a> {
    pub doc: DocId,
    pub source: Source,
    pub analysis: &'a Analysis,
    /// Top-level statements with the IRI as subject, in text order.
    pub spans: &'a [Span],
}

/// What one source contributes beyond its analysis' own indexes.
#[derive(Default, Debug)]
pub struct DocFacts {
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
}

/// The namespace part of an IRI: up to and including its last `#`, `/` or `:`.
pub fn namespace_of(iri: &str) -> &str {
    iri.rfind(['#', '/', ':']).map_or("", |i| &iri[..=i])
}

/// The own-namespace rule: at least this many described terms...
const OWN_MIN_DESCRIBED: u32 = 3;
/// ...and at least 1 in this many referenced terms described (20%).
const OWN_MIN_SHARE: u32 = 5;

struct Preds {
    rdf_type: Iri,
    sub_class_of: Iri,
    sub_property_of: Iri,
    domain: [Iri; 3],
    range: [Iri; 3],
    rdf_first: Iri,
    rdf_rest: Iri,
    rdf_nil: Iri,
    rdfs_class: Iri,
    owl_class: Iri,
    node_shape: Iri,
    target_class: Iri,
    property: Iri,
    path: Iri,
    min_count: Iri,
    max_count: Iri,
    class: Iri,
    datatype: Iri,
    in_list: Iri,
}

fn preds() -> &'static Preds {
    static P: OnceLock<Preds> = OnceLock::new();
    P.get_or_init(|| {
        let i = Iri::intern;
        let rdfs = |l: &str| i(&format!("http://www.w3.org/2000/01/rdf-schema#{l}"));
        let sh = |l: &str| i(&format!("http://www.w3.org/ns/shacl#{l}"));
        let rdf = |l: &str| i(&format!("{}{l}", syntax::RDF));
        Preds {
            rdf_type: i(syntax::RDF_TYPE),
            sub_class_of: i(RDFS_SUBCLASS_OF),
            sub_property_of: i(RDFS_SUBPROPERTY_OF),
            domain: [
                rdfs("domain"),
                i("https://schema.org/domainIncludes"),
                i("http://schema.org/domainIncludes"),
            ],
            range: [
                rdfs("range"),
                i("https://schema.org/rangeIncludes"),
                i("http://schema.org/rangeIncludes"),
            ],
            rdf_first: rdf("first"),
            rdf_rest: rdf("rest"),
            rdf_nil: rdf("nil"),
            rdfs_class: rdfs("Class"),
            owl_class: i("http://www.w3.org/2002/07/owl#Class"),
            node_shape: sh("NodeShape"),
            target_class: sh("targetClass"),
            property: sh("property"),
            path: sh("path"),
            min_count: sh("minCount"),
            max_count: sh("maxCount"),
            class: sh("class"),
            datatype: sh("datatype"),
            in_list: sh("in"),
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
    pub fn new(a: &Analysis) -> DocFacts {
        let p = preds();
        let mut f = DocFacts::default();
        let triples = &a.parsed.triples;
        let mut shape_nodes: Vec<Subj> = Vec::new();
        for t in triples {
            if t.p == p.target_class
                && let Some(s) = subj(&t.s)
            {
                shape_nodes.push(s);
            }
            let (Node::Iri(s), Node::Iri(o)) = (&t.s, &t.o) else {
                continue;
            };
            let (s, o) = (*s, *o);
            if t.p == p.rdf_type {
                f.instances.entry(o).or_default().push(s);
                if o == p.node_shape {
                    shape_nodes.push(Subj::Iri(s));
                }
            } else if t.p == p.sub_class_of || t.p == p.sub_property_of {
                f.subs.entry(o).or_default().push(s);
            } else if p.domain.contains(&t.p) {
                f.domain.entry(s).or_default().push(o);
            } else if p.range.contains(&t.p) {
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

    fn shapes(&mut self, a: &Analysis, nodes: Vec<Subj>) {
        let p = preds();
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
                    Node::Iri(o) if t.p == p.target_class => targets.push(*o),
                    Node::Iri(o) if t.p == p.rdf_type => {
                        is_shape |= *o == p.node_shape;
                        is_class |= *o == p.rdfs_class || *o == p.owl_class;
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
                .filter(|t| t.p == p.property)
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
                            Node::Iri(o) if pt.p == p.path => c.path = Some(*o),
                            Node::Iri(o) if pt.p == p.class => c.class.push(*o),
                            Node::Iri(o) if pt.p == p.datatype => c.datatype = Some(*o),
                            _ if pt.p == p.min_count => c.min_count = count(),
                            _ if pt.p == p.max_count => c.max_count = count(),
                            head if pt.p == p.in_list => {
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

        /// The members of an RDF collection (cycle-safe).
        fn list<'t>(
            head: &Node,
            of: &dyn Fn(Subj) -> &'t [u32],
            triples: &[syntax::Triple],
        ) -> Vec<Node> {
            let p = preds();
            let mut out = Vec::new();
            let mut cur = head.clone();
            for _ in 0..10_000 {
                let Some(s) = subj(&cur) else { break };
                if cur == Node::Iri(p.rdf_nil) {
                    break;
                }
                let mut next = None;
                for &i in of(s) {
                    let t = &triples[i as usize];
                    if t.p == p.rdf_first {
                        out.push(t.o.clone());
                    } else if t.p == p.rdf_rest {
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

impl Knowledge {
    /// Bumped on every change; lets callers tell whether cached results are stale.
    pub fn generation(&self) -> u64 {
        self.generation
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
    pub fn set_prepared(&mut self, uri: &str, source: Source, a: Arc<Analysis>, facts: DocFacts) {
        debug_assert!(source != Source::External, "use add_external");
        self.put(Key::Workspace(uri.to_string()), uri, source, a, facts);
    }

    pub fn remove(&mut self, uri: &str) {
        self.take(&Key::Workspace(uri.to_string()));
    }

    /// Add or replace a read-only source (e.g. an imported ontology) under
    /// `key`. Its facts count; it is never diagnosed, renamed or listed as a
    /// workspace symbol.
    pub fn add_external(&mut self, key: &str, a: Arc<Analysis>) {
        let k = Key::External(key.to_string());
        if self.is_current(&k, Source::External, &a) {
            return;
        }
        let facts = DocFacts::new(&a);
        self.put(k, key, Source::External, a, facts);
    }

    pub fn remove_external(&mut self, key: &str) {
        self.take(&Key::External(key.to_string()));
    }

    /// The analysis registered for workspace document `uri`.
    pub fn get(&self, uri: &str) -> Option<&Arc<Analysis>> {
        let id = self.by_key.get(&Key::Workspace(uri.to_string()))?;
        Some(&self.slot(*id).analysis)
    }

    pub fn doc(&self, id: DocId) -> &Analysis {
        &self.slot(id).analysis
    }

    pub fn source(&self, id: DocId) -> Source {
        self.slot(id).source
    }

    /// Every source, in no particular order.
    pub fn docs(&self) -> impl Iterator<Item = (DocId, Source, &Arc<Analysis>)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.as_ref().map(|s| (i as DocId, s.source, &s.analysis)))
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
        self.generation += 1;
    }

    fn take(&mut self, key: &Key) {
        if let Some(id) = self.by_key.remove(key) {
            self.release(id);
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
        let entries = &mut self.entries;
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
                step(&mut namespaces.entry(ns).or_default().referenced, add);
            }
        }
        for &iri in a.defs.keys() {
            let e = entry(entries, iri);
            let ns = e.ns.expect("set on creation");
            if !ws {
                step(&mut e.described_ext, add);
            } else if step(&mut e.described_ws, add) {
                step(&mut namespaces.entry(ns).or_default().described, add);
            }
        }
        if ws {
            for &(name, ns) in &f.bindings {
                let by_ns = self.bindings.entry(name).or_default();
                let n = by_ns.entry(ns).or_default();
                if step(n, add) && !add {
                    by_ns.remove(&ns);
                    if by_ns.is_empty() {
                        self.bindings.remove(&name);
                    }
                }
            }
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
    pub fn descriptions<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Description<'_>> {
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
    pub fn is_described<K: IriKey + ?Sized>(&self, iri: &K) -> bool {
        self.entries
            .get(iri)
            .is_some_and(|e| e.described_ws + e.described_ext > 0)
    }

    /// Union over sources, each source's types in its own order.
    pub fn types<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect(iri, |s, k| s.analysis.types.get(&k).map(Vec::as_slice))
    }

    /// Direct super classes / properties stated in the workspace, then from
    /// well-known vocabularies.
    pub fn supers<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect(iri, |s, k| s.analysis.supers.get(&k).map(Vec::as_slice));
        with_vocab(&mut v, iri, |t| &t.supers);
        v
    }

    /// Direct sub classes / properties (workspace only).
    pub fn subs<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect_objects(iri, |f, k| f.subs.get(&k).map(Vec::as_slice))
    }

    /// Resources typed with this class (workspace only).
    pub fn instances<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        self.collect_objects(iri, |f, k| f.instances.get(&k).map(Vec::as_slice))
    }

    /// Sources in which `iri` is the object of a type or sub-class/property
    /// link, with the subjects linked to it there.
    pub fn related_by_doc<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<(DocId, Vec<Iri>)> {
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
    pub fn domains<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect_subject_facts(iri, |f, k| f.domain.get(&k).map(Vec::as_slice));
        with_vocab(&mut v, iri, |t| &t.domain);
        v
    }

    /// rdfs:range from the workspace, then from well-known vocabularies.
    pub fn ranges<K: IriKey + ?Sized>(&self, iri: &K) -> Vec<Iri> {
        let mut v = self.collect_subject_facts(iri, |f, k| f.range.get(&k).map(Vec::as_slice));
        with_vocab(&mut v, iri, |t| &t.range);
        v
    }

    /// The label from the workspace: English or untagged preferred, then open
    /// documents, then by URI.
    pub fn label<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.pick(iri, |a, k| a.label_en(&k))
    }

    /// The comment from the workspace, chosen like [`Knowledge::label`].
    pub fn comment<K: IriKey + ?Sized>(&self, iri: &K) -> Option<&str> {
        self.pick(iri, |a, k| a.comment_en(&k))
    }

    /// Used in predicate position anywhere.
    pub fn used_as_predicate<K: IriKey + ?Sized>(&self, iri: &K) -> bool {
        self.entries.get(iri).is_some_and(|e| e.predicate > 0)
    }

    /// SHACL shapes that target this class, with the source they are in.
    pub fn shapes_targeting<K: IriKey + ?Sized>(&self, class: &K) -> Vec<(DocId, &Shape)> {
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
    pub fn docs_referencing<K: IriKey + ?Sized>(
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
    pub fn terms_in(&self, ns: &str) -> Vec<Iri> {
        if namespace_of(ns).len() == ns.len() {
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
    pub fn described_terms(&self) -> impl Iterator<Item = Iri> + '_ {
        self.entries
            .iter()
            .filter(|(_, e)| e.described_ws + e.described_ext > 0)
            .map(|(i, _)| *i)
    }

    /// Namespaces bound to prefix `name` across workspace files, most files first.
    pub fn bindings(&self, name: &str) -> Vec<(Iri, u32)> {
        let Some(by_ns) = Iri::get(name).and_then(|n| self.bindings.get(&n)) else {
            return vec![];
        };
        let mut v: Vec<(Iri, u32)> = by_ns.iter().map(|(ns, n)| (*ns, *n)).collect();
        v.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
        v
    }

    /// An "own" namespace: not a well-known vocabulary, at least 3 terms in
    /// it described in the workspace, and at least 20% of the distinct terms
    /// the workspace references in it described.
    pub fn is_own_namespace(&self, ns: &str) -> bool {
        if VOCABS.by_namespace(ns).is_some() {
            return false;
        }
        Iri::get(ns)
            .and_then(|k| self.namespaces.get(&k))
            .is_some_and(|n| {
                n.described >= OWN_MIN_DESCRIBED && n.described * OWN_MIN_SHARE >= n.referenced
            })
    }

    /// Every own namespace (see [`Knowledge::is_own_namespace`]), sorted.
    pub fn own_namespaces(&self) -> Vec<Iri> {
        let mut v: Vec<Iri> = self
            .namespaces
            .keys()
            .filter(|ns| self.is_own_namespace(ns))
            .copied()
            .collect();
        v.sort();
        v
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
        for id in self.ordered(&e.object) {
            for i in get(&self.slot(id).facts, key).into_iter().flatten() {
                if !out.contains(i) {
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
        for p in syntax::parse(&text, &PrefixMap::default()).prefixes {
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
    fn own_namespace_needs_a_share_of_described_terms() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let mut text = String::from("@prefix ex: <http://ex.org/> .\n");
        for i in 0..3 {
            text.push_str(&format!("ex:t{i} a ex:C .\n"));
        }
        k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
        // 3 described of 4 referenced (t0..t2, C).
        assert!(k.is_own_namespace("http://ex.org/"));
        for i in 0..20 {
            text.push_str(&format!("ex:t0 ex:p ex:x{i} .\n"));
        }
        k.set("file:///a.ttl", Source::Disk, analysis("a", &text, &none));
        // 3 of 25: below 20%.
        assert!(!k.is_own_namespace("http://ex.org/"));
    }
}
