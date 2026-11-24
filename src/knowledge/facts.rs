//! What one source contributes to the index beyond its analysis' own
//! indexes ([`DocFacts`]), and the pure builder that computes it from the
//! analysis ([`DocFacts::new`]): the SHACL shapes that target classes, the
//! label and definition entries by text key, the curation summaries per IRI
//! ([`Curated`]), and the term kinds and states they feed.

use super::counts::{ByIri, Counts, Ided, counted, summed};
use super::{Index, Keyed, OWL_ONTOLOGY, Slot};
use crate::analysis::Analysis;
use crate::shacl::vocab;
use crate::syntax::{self, BlankId, Iri, IriMap, Node, PrefixUse, Role, Span, Syntax};
use rustc_hash::{FxHashMap, FxHashSet, FxHasher};
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

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

/// What one source contributes beyond its analysis' own indexes.
#[derive(Default, Debug)]
pub(crate) struct DocFacts {
    /// Property -> its domains and ranges stated here.
    pub(super) domain: ByIri,
    pub(super) range: ByIri,
    /// Super term -> direct sub terms stated here.
    pub(super) subs: ByIri,
    /// Class -> resources typed with it here.
    pub(super) instances: ByIri,
    pub(super) shapes: Vec<Shape>,
    /// Target class -> indexes into `shapes`.
    pub(super) shapes_by_target: IriMap<Vec<u32>>,
    /// What this source contributes to each IRI's index entry, by role: per
    /// IRI, how many of its facts give it that role, sorted by [`Iri::id`]
    /// (see [`Counts`]). Replacing a source changes the index only where an
    /// IRI's count moves between zero and non-zero.
    ///
    /// The subject of a [`Curated`] summary (which every subject of a
    /// top-level statement, type or super link has) or of a domain or range
    /// link: one per summary and link.
    pub(super) subjects: Counts<Iri>,
    /// The object of a type, super or target link: one per link.
    pub(super) objects: Counts<Iri>,
    /// Used in predicate position: one per use.
    pub(super) predicates: Counts<Iri>,
    /// Mentioned anywhere: one per reference.
    pub(super) refs: Counts<Iri>,
    /// The subject of a top-level statement, or the name of a TriG graph
    /// block: one per statement or block.
    pub(super) defs: Counts<Iri>,
    /// (prefix name, namespace) in effect and used or declared here: one per
    /// declaration, and one for a namespace supplied from outside and used.
    pub(super) bindings: Counts<(Iri, Iri)>,
    /// IRIs this source makes terms, one per triple: see [`Knowledge::is_term`].
    pub(super) terms: Counts<Iri>,
    /// The first triple of each statement, so that [`Keyed`] positions
    /// can be relative to their statement: kept only when there are
    /// labels, definitions or comments.
    firsts: Box<[u32]>,
    /// `rdfs:label` and `skos:prefLabel` of IRI subjects, in text order.
    pub(super) labels: Vec<Keyed>,
    /// `skos:definition` of IRI subjects, in text order.
    pub(super) definitions: Vec<Keyed>,
    /// `rdfs:comment` of IRI subjects, in text order.
    pub(super) comments: Vec<Keyed>,
    /// Per [`Index`], its entries' positions ordered by key: made on the
    /// first lookup by key ([`DocFacts::under_key`]), which most documents
    /// never get.
    by_key: [OnceLock<Box<[u32]>>; 3],
    /// Lookups by key that scanned instead ([`DocFacts::under_key`]).
    scans: AtomicU32,
    /// Subjects of `definitions`, one per definition.
    pub(super) defined: Counts<Iri>,
    /// `owl:deprecated` with a boolean value (unique, by [`Iri::id`]).
    pub(super) deprecated: Vec<(Iri, bool)>,
    /// What this source says about each IRI subject that the curation
    /// checks read, sorted by [`Iri::id`]; IRIs it says none of it about are
    /// left out.
    pub(super) curated: Vec<(Iri, Curated)>,
}

/// What one workspace source says about an IRI that the curation checks of
/// other documents read ([`Knowledge::term_kind`], [`TermState`]).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct Curated {
    /// [`TERM_CLASS`], [`TERM_PROPERTY`] and [`TERM_CONCEPT`] bits.
    pub(super) kind: u8,
    /// Gives it a type or an IRI super class or property
    /// ([`Analysis::types`], [`Analysis::supers`]).
    pub(super) declares: bool,
    /// The subject of a top-level statement ([`Analysis::defs`]).
    pub(super) describes: bool,
    /// Types it `owl:Ontology`.
    pub(super) ontology: bool,
    /// Gives it a label ([`Analysis::label`]), a comment
    /// ([`Analysis::comment_en`]).
    pub(super) labelled: bool,
    pub(super) commented: bool,
    /// Bit `i`: gives it a property of group `i` of [`header_properties`].
    pub(super) header: u8,
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
pub(super) struct TermState {
    pub(super) kind: u8,
    pub(super) labelled: bool,
    pub(super) commented: bool,
    /// The document that reports `missing-label` and `missing-definition`:
    /// the first by URI of those that describe and declare the term, else of
    /// those that describe it.
    pub(super) reporter: Option<String>,
    /// The document that reports the `missing-ontology-*` checks: the first
    /// by URI of those that describe the IRI and type it `owl:Ontology`.
    pub(super) ontology: Option<String>,
    pub(super) header: u8,
}

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
pub(super) enum Subj {
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
pub(super) fn folded(text: &str) -> impl Iterator<Item = char> + '_ {
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
pub(super) fn literal<'a>(s: &'a Slot, k: &Keyed) -> (&'a str, Option<Iri>) {
    let triple = s.facts.firsts[k.statement as usize] + k.offset;
    match &s.analysis.parsed.triples[triple as usize].o {
        Node::Literal { value, lang, .. } => (value, *lang),
        _ => ("", None),
    }
}

/// Whether two (text, language) pairs fold to the same key.
pub(super) fn same_text(x: (&str, Option<Iri>), y: (&str, Option<Iri>)) -> bool {
    x.1 == y.1 && (x.0 == y.0 || folded(x.0).eq(folded(y.0)))
}

pub(super) fn subj(n: &Node) -> Option<Subj> {
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
        // The prefix of an RDF/XML qualified name is an XML namespace's, not
        // the project prefix of that name. An OWL Functional or OWL/XML
        // document declares its own prefixes and takes none from the
        // project.
        let uses: &[PrefixUse] = match a.parsed.syntax {
            Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads | Syntax::JsonLd => {
                &a.parsed.prefix_uses
            }
            Syntax::RdfXml | Syntax::OwlFunctional | Syntax::OwlXml => &[],
        };
        for u in uses {
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
    pub(super) fn under_key(&self, index: Index, key: u64) -> impl Iterator<Item = &Keyed> {
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
    pub(super) fn drop_curation(&mut self) {
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
    pub(super) fn shapes(&mut self, a: &Analysis, nodes: Vec<Subj>) {
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
