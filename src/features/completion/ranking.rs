//! Ontology- and shape-aware ranking for completion.
//!
//! Predicate position: the subject's types (from the knowledge index and the
//! `a X` written around the cursor) expand through their superclasses.
//! Properties that SHACL shapes targeting those classes constrain rank first
//! (required ones above the rest), then properties whose `rdfs:domain` is one
//! of the classes.
//!
//! Object position: the `sh:in` values of a matching property shape rank
//! first, then instances of the predicate's range or the shape's `sh:class`,
//! and of their subclasses.
//!
//! Inside `[ ]` the enclosing subjects count too, so a sequence path such as
//! `sh:path ( ex:p ex:q )` on a shape for the outer subject applies to `ex:q`
//! inside `ex:p [ ... ]`.

use crate::analysis::Analysis;
use crate::knowledge::{DocId, Knowledge, PropertyConstraint, Shape, namespace_of};
use crate::syntax::lexer::{Kind, Token};
use crate::syntax::{Iri, Node, PrefixMap, RDF_TYPE, Role, Span, XSD};
use crate::vocab::{TermKind, VOCABS};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{LazyLock, OnceLock};

/// Rank tiers; lower sorts first. Candidates without a hit get [`NONE`].
pub const REQUIRED: u8 = 0;
pub const SHAPE: u8 = 1;
pub const SCHEMA: u8 = 2;
pub const NONE: u8 = 3;

/// Depth limit for superclass / subclass / superproperty walks.
const MAX_DEPTH: usize = 10;
/// At most this many classes per walk.
const MAX_CLASSES: usize = 256;
/// Enclosing subjects considered for sequence paths (the subject itself + 3).
const MAX_LEVELS: usize = 4;
/// Instances offered at most, and references looked at to find them.
pub const MAX_INSTANCES: usize = 1000;
const MAX_REFS_SCANNED: usize = 20_000;
/// Tokens looked at backwards to find the enclosing subject.
const MAX_TOKENS_SCANNED: usize = 20_000;

pub struct Hit {
    pub tier: u8,
    /// Replaces the candidate's usual detail.
    pub detail: Option<String>,
    /// An `sh:in` value or the like, rather than a resource found by type.
    pub enumerated: bool,
}

#[derive(Default)]
pub struct Ranking {
    pub iris: FxHashMap<Iri, Hit>,
    /// Literal `sh:in` values, written as Turtle, with their detail.
    pub literals: Vec<(String, String)>,
    /// Some instances were left out (caps reached).
    pub truncated: bool,
}

impl Ranking {
    fn hit(&mut self, iri: Iri, tier: u8, detail: Option<String>, enumerated: bool) {
        match self.iris.get_mut(&iri) {
            Some(h) if h.tier <= tier => {}
            Some(h) => {
                *h = Hit {
                    tier,
                    detail,
                    enumerated,
                }
            }
            None => {
                self.iris.insert(
                    iri,
                    Hit {
                        tier,
                        detail,
                        enumerated,
                    },
                );
            }
        }
    }
}

struct Preds {
    rdf_type: Iri,
    domain: [Iri; 3],
    sh_property: Iri,
    sh_path: Iri,
    rdf_first: Iri,
    rdf_rest: Iri,
    rdf_nil: Iri,
    rdfs_datatype: Iri,
}

fn preds() -> &'static Preds {
    static P: OnceLock<Preds> = OnceLock::new();
    P.get_or_init(|| {
        let i = Iri::intern;
        Preds {
            rdf_type: i(RDF_TYPE),
            domain: [
                i("http://www.w3.org/2000/01/rdf-schema#domain"),
                i("https://schema.org/domainIncludes"),
                i("http://schema.org/domainIncludes"),
            ],
            sh_property: i("http://www.w3.org/ns/shacl#property"),
            sh_path: i("http://www.w3.org/ns/shacl#path"),
            rdf_first: i("http://www.w3.org/1999/02/22-rdf-syntax-ns#first"),
            rdf_rest: i("http://www.w3.org/1999/02/22-rdf-syntax-ns#rest"),
            rdf_nil: i("http://www.w3.org/1999/02/22-rdf-syntax-ns#nil"),
            rdfs_datatype: i("http://www.w3.org/2000/01/rdf-schema#Datatype"),
        }
    })
}

/// A subject around the cursor: the subject itself (level 0) or one that
/// encloses it, with the predicates leading from it to the cursor's subject.
struct Level {
    node: Option<Node>,
    /// Statement the node's triples are in, for blank nodes.
    statement: Option<u32>,
    path: Vec<Iri>,
}

/// What the ranking needs to know of a subject around the cursor (the
/// subject itself first, then those enclosing it): its types, and the
/// predicates leading from it to the cursor's subject.
pub struct Subject {
    pub types: Vec<Iri>,
    pub path: Vec<Iri>,
}

fn subjects(a: &Analysis, k: &Knowledge, levels: &[Level]) -> Vec<Subject> {
    levels
        .iter()
        .map(|level| Subject {
            types: types(a, k, level),
            path: level.path.clone(),
        })
        .collect()
}

/// Ranking for the predicate written at `offset`.
pub fn predicates(a: &Analysis, k: &Knowledge, offset: u32, prefixes: &PrefixMap) -> Ranking {
    predicates_for(k, &subjects(a, k, &levels(a, offset)), prefixes)
}

/// Ranking for a predicate of `subjects[0]` (the others enclose it).
pub fn predicates_for(k: &Knowledge, subjects: &[Subject], prefixes: &PrefixMap) -> Ranking {
    let mut r = Ranking::default();
    for (depth, level) in subjects.iter().enumerate() {
        let classes = superclasses(k, &level.types);
        for &c in &classes {
            for (doc, shape) in k.shapes_targeting(&c) {
                for pc in &shape.properties {
                    let Some(path) = constraint_path(k, doc, pc) else {
                        continue;
                    };
                    let Some((&last, head)) = path.split_last() else {
                        continue;
                    };
                    if head != level.path.as_slice() {
                        continue;
                    }
                    if pc.min_count.is_some_and(|n| n >= 1) {
                        let detail = format!("required by {}", shape_name(k, doc, shape, prefixes));
                        r.hit(last, REQUIRED, Some(detail), false);
                    } else {
                        r.hit(last, SHAPE, None, false);
                    }
                }
            }
            if depth == 0 && !is_universal(c) {
                for p in with_domain(k, c) {
                    r.hit(p, SCHEMA, None, false);
                    for sub in walk(k, p, |k, i| k.subs(&i)) {
                        r.hit(sub, SCHEMA, None, false);
                    }
                }
            }
        }
    }
    r
}

/// Ranking for the object written at `offset`. `keep` filters instances
/// cheaply (by what the user typed) before they count against the caps.
pub fn objects(
    a: &Analysis,
    k: &Knowledge,
    offset: u32,
    prefixes: &PrefixMap,
    keep: &dyn Fn(Iri) -> bool,
) -> Ranking {
    let Some((pred, pred_start)) = predicate_of_object(a, offset) else {
        return Ranking::default();
    };
    if pred == preds().rdf_type {
        return Ranking::default();
    }
    let levels = levels(a, pred_start);
    let subject = levels.first().and_then(|l| match l.node {
        Some(Node::Iri(i)) => Some(i),
        _ => None,
    });
    objects_for(k, pred, &subjects(a, k, &levels), subject, prefixes, keep)
}

/// Ranking for an object of `pred` whose subject is `subjects[0]` (the
/// others enclose it); `subject` is that subject's IRI, never offered.
pub fn objects_for(
    k: &Knowledge,
    pred: Iri,
    subjects: &[Subject],
    subject: Option<Iri>,
    prefixes: &PrefixMap,
    keep: &dyn Fn(Iri) -> bool,
) -> Ranking {
    let mut r = Ranking::default();
    if pred == preds().rdf_type {
        return r;
    }
    let mut classes: Vec<(Iri, u8)> = Vec::new();
    for level in subjects {
        let types = superclasses(k, &level.types);
        for &c in &types {
            for (doc, shape) in k.shapes_targeting(&c) {
                for pc in &shape.properties {
                    let Some(path) = constraint_path(k, doc, pc) else {
                        continue;
                    };
                    let Some((&last, head)) = path.split_last() else {
                        continue;
                    };
                    if last != pred || head != level.path.as_slice() {
                        continue;
                    }
                    classes.extend(pc.class.iter().map(|c| (*c, SHAPE)));
                    let detail = format!("allowed by {}", shape_name(k, doc, shape, prefixes));
                    for v in pc.in_values.iter().flatten() {
                        match v {
                            Node::Iri(i) => r.hit(*i, REQUIRED, Some(detail.clone()), true),
                            Node::Literal { .. } => {
                                if let Some(text) = turtle_literal(v, prefixes)
                                    && !r.literals.iter().any(|(t, _)| *t == text)
                                {
                                    r.literals.push((text, detail.clone()));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    let mut props = vec![pred];
    // Superproperties pass their range on (rdfs:subPropertyOf).
    props.extend(walk(k, pred, |k, i| k.supers(&i)));
    for p in props {
        classes.extend(k.ranges(&p).into_iter().map(|c| (c, SHAPE)));
    }

    let mut seen: FxHashSet<Iri> = FxHashSet::default();
    let mut found = 0usize;
    let mut scanned = 0usize;
    for (range, tier) in classes {
        if !seen.insert(range) || is_datatype(k, range) || is_universal(range) {
            continue;
        }
        let mut all = vec![range];
        all.extend(walk(k, range, |k, i| k.subs(&i)));
        for class in all {
            for_each_instance(k, class, &mut scanned, |i| {
                if Some(i) != subject && keep(i) && !r.iris.contains_key(&i) {
                    r.hit(i, tier, None, false);
                    found += 1;
                }
                found < MAX_INSTANCES
            });
            if found >= MAX_INSTANCES || scanned >= MAX_REFS_SCANNED {
                r.truncated = true;
                return r;
            }
        }
    }
    r
}

// ---- the subject at the cursor ----

fn tokens_before(a: &Analysis, offset: u32) -> &[Token] {
    let toks = a.tokens();
    &toks[..toks.partition_point(|t| t.end <= offset)]
}

fn is_open(k: Kind) -> bool {
    matches!(
        k,
        Kind::LBracket | Kind::LParen | Kind::LTriple | Kind::LTripleTerm | Kind::LAnnotation
    )
}

fn is_close(k: Kind) -> bool {
    matches!(
        k,
        Kind::RBracket | Kind::RParen | Kind::RTriple | Kind::RTripleTerm | Kind::RAnnotation
    )
}

/// The cursor's subject and the subjects enclosing it, innermost first.
fn levels(a: &Analysis, offset: u32) -> Vec<Level> {
    let mut out = Vec::new();
    let mut path: Vec<Iri> = Vec::new();
    let mut at = offset;
    while out.len() < MAX_LEVELS {
        let (node, statement, parent) = subject_at(a, at);
        out.push(Level {
            node,
            statement,
            path: path.clone(),
        });
        let Some((pred, start)) = parent else { break };
        path.insert(0, pred);
        at = start;
    }
    out
}

/// The subject of a predicate written at `offset`, the statement it is in,
/// and, inside `[ ]`, the predicate (and its start) that the brackets are the
/// object of.
fn subject_at(a: &Analysis, offset: u32) -> (Option<Node>, Option<u32>, Option<(Iri, u32)>) {
    let toks = tokens_before(a, offset);
    let mut depth = 0u32;
    let mut stop = 0u32;
    let mut open = None;
    for (j, t) in toks.iter().enumerate().rev().take(MAX_TOKENS_SCANNED) {
        match t.kind {
            k if is_close(k) => depth += 1,
            Kind::LBracket if depth == 0 => {
                open = Some(j);
                break;
            }
            k if is_open(k) => {
                if depth == 0 {
                    // In a collection, triple term or annotation.
                    return (None, None, None);
                }
                depth -= 1;
            }
            Kind::Dot if depth == 0 => {
                stop = t.end;
                break;
            }
            _ => {}
        }
    }
    let statements = &a.parsed.statements;
    let Some(open) = open else {
        // Top level: the last statement that starts after the previous '.'
        // (in JSON-LD, which has no tokens, the innermost around `offset`).
        let Some(st) = a
            .parsed
            .statement_at(Span::new(offset, offset))
            .filter(|&i| statements[i].span.start >= stop)
        else {
            return (None, None, None);
        };
        let node = match statements[st].subject {
            Some((iri, _)) => Some(Node::Iri(iri)),
            None => statement_triples(a, st as u32)
                .iter()
                .min_by_key(|t| t.p_span.start)
                .map(|t| t.s.clone()),
        };
        return (node, Some(st as u32), None);
    };
    let at = toks[open].start;
    let st = a.parsed.statement_at(Span::new(at, at)).map(|i| i as u32);
    let node = st.and_then(|st| {
        statement_triples(a, st)
            .iter()
            .filter(|t| t.p_span.start > at && t.p_span.start < offset)
            .filter(|t| matches!(t.s, Node::Blank(_)))
            .min_by_key(|t| t.p_span.start)
            .map(|t| t.s.clone())
    });
    let parent = predicate_before(a, &toks[..open]);
    (node, st, parent)
}

/// The predicate whose object list ends at the end of `toks`: the last
/// token itself, or the one before a run of `obj ,` items.
fn predicate_before(a: &Analysis, toks: &[Token]) -> Option<(Iri, u32)> {
    let mut depth = 0u32;
    for t in toks.iter().rev().take(MAX_TOKENS_SCANNED) {
        match t.kind {
            k if is_close(k) => depth += 1,
            k if is_open(k) => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
            }
            _ if depth > 0 => {}
            Kind::Semicolon | Kind::Dot => return None,
            Kind::A | Kind::Iri | Kind::PName => match a.ref_at(t.start) {
                Some(r) if r.role == Role::Predicate => return Some((r.iri, t.start)),
                Some(r) if r.role == Role::Subject => return None,
                _ => {}
            },
            _ => {}
        }
    }
    None
}

/// The predicate of the object being written at `offset`.
fn predicate_of_object(a: &Analysis, offset: u32) -> Option<(Iri, u32)> {
    let toks = tokens_before(a, offset);
    let last = toks.iter().rposition(|t| t.kind != Kind::Comment)?;
    match toks[last].kind {
        Kind::Comma | Kind::Iri | Kind::PName | Kind::A => {}
        _ => return None,
    }
    predicate_before(a, &toks[..=last])
}

fn statement_triples(a: &Analysis, st: u32) -> &[crate::syntax::Triple] {
    let t = &a.parsed.triples;
    let lo = t.partition_point(|x| x.statement < st);
    let hi = t.partition_point(|x| x.statement <= st);
    &t[lo..hi]
}

/// Types of a level's subject: from the knowledge index and from this
/// document (which may be newer than the index).
fn types(a: &Analysis, k: &Knowledge, level: &Level) -> Vec<Iri> {
    let rdf_type = preds().rdf_type;
    let mut out: Vec<Iri> = Vec::new();
    match &level.node {
        Some(Node::Iri(i)) => {
            out.extend(a.types.get(i).into_iter().flatten().copied());
            out.extend(k.types(i));
        }
        Some(n @ Node::Blank(_)) => {
            if let Some(st) = level.statement {
                for t in statement_triples(a, st) {
                    if t.s == *n
                        && t.p == rdf_type
                        && let Node::Iri(o) = t.o
                    {
                        out.push(o);
                    }
                }
            }
        }
        _ => {}
    }
    out
}

// ---- class and property walks ----

/// `start` and everything reachable through `next`, breadth first, without
/// `start` itself unless a cycle leads back to it (it is never repeated).
fn walk(k: &Knowledge, start: Iri, next: impl Fn(&Knowledge, Iri) -> Vec<Iri>) -> Vec<Iri> {
    let mut seen: FxHashSet<Iri> = FxHashSet::default();
    seen.insert(start);
    let mut out = Vec::new();
    let mut frontier = vec![start];
    for _ in 0..MAX_DEPTH {
        let mut nextf = Vec::new();
        for i in frontier {
            for n in next(k, i) {
                if out.len() >= MAX_CLASSES {
                    return out;
                }
                if seen.insert(n) {
                    out.push(n);
                    nextf.push(n);
                }
            }
        }
        if nextf.is_empty() {
            break;
        }
        frontier = nextf;
    }
    out
}

/// The classes and all their superclasses, without duplicates.
fn superclasses(k: &Knowledge, classes: &[Iri]) -> Vec<Iri> {
    let mut out: Vec<Iri> = Vec::new();
    for &c in classes {
        if out.contains(&c) {
            continue;
        }
        out.push(c);
        for s in walk(k, c, |k, i| k.supers(&i)) {
            if !out.contains(&s) {
                out.push(s);
            }
            if out.len() >= MAX_CLASSES {
                return out;
            }
        }
    }
    out
}

/// Every resource is one: as a domain or range it says nothing.
fn is_universal(c: Iri) -> bool {
    matches!(
        &*c,
        "http://www.w3.org/2000/01/rdf-schema#Resource" | "http://www.w3.org/2002/07/owl#Thing"
    )
}

fn is_datatype(k: &Knowledge, c: Iri) -> bool {
    c.starts_with(XSD)
        || matches!(
            &*c,
            "http://www.w3.org/2000/01/rdf-schema#Literal"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#HTML"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral"
                | "http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON"
        )
        || k.types(&c).contains(&preds().rdfs_datatype)
}

/// Where the references to one class are looked for in the document's
/// triples: references come in document order, and so (mostly) do the
/// triples, so the search for each continues where the last one ended.
struct Cursor {
    at: usize,
    last: u32,
}

/// The triple that has `r`, a reference to `class`, as its object. Each
/// triple looked at past the first guess counts in `scanned`.
fn triple_of_object<'a>(
    a: &'a Analysis,
    r: &crate::syntax::Ref,
    class: Iri,
    cursor: &mut Cursor,
    scanned: &mut usize,
) -> Option<&'a crate::syntax::Triple> {
    let all = &a.parsed.triples;
    let hit = |t: &crate::syntax::Triple| {
        matches!(&t.o, Node::Iri(i) if *i == class)
            && t.o_span.start <= r.span.start
            && r.span.end <= t.o_span.end
    };
    // A triple that ends before the reference starts cannot be its triple,
    // nor any later reference's.
    if r.span.start < cursor.last {
        cursor.at = 0;
    }
    cursor.last = r.span.start;
    if cursor.at == 0 {
        cursor.at = all.partition_point(|t| t.o_span.end <= r.span.start);
    }
    while all
        .get(cursor.at)
        .is_some_and(|t| t.o_span.end <= r.span.start)
    {
        cursor.at += 1;
    }
    if let Some(t) = all.get(cursor.at).filter(|t| hit(t)) {
        return Some(t);
    }
    // Not where it is expected (a nested object written before the triples
    // inside it): look through the statement.
    for st in a.parsed.statements_around(r.span) {
        for t in statement_triples(a, st as u32) {
            *scanned += 1;
            if *scanned > MAX_REFS_SCANNED {
                return None;
            }
            if hit(t) {
                return Some(t);
            }
        }
    }
    None
}

/// The triples of `a` that have `class` as object and one of `preds` as
/// predicate. Each reference to `class` looked at counts in `scanned`, which
/// the caller shares across documents; the walk stops at the cap.
fn objects_of<'a>(
    a: &'a Analysis,
    class: Iri,
    preds: &'a [Iri],
    scanned: &'a mut usize,
) -> impl Iterator<Item = &'a crate::syntax::Triple> + 'a {
    let mut cursor = Cursor { at: 0, last: 0 };
    a.refs_by_iri
        .get(&class)
        .into_iter()
        .flatten()
        .map(|&i| i as usize)
        .map_while(move |i| {
            *scanned += 1;
            if *scanned > MAX_REFS_SCANNED {
                return None;
            }
            let r = &a.parsed.refs[i];
            let found = (r.role == Role::Object)
                .then(|| triple_of_object(a, r, class, &mut cursor, scanned))
                .flatten()
                .filter(|t| preds.contains(&t.p));
            Some(found)
        })
        .flatten()
}

/// Resources typed `class` (directly), in workspace order.
fn for_each_instance(
    k: &Knowledge,
    class: Iri,
    scanned: &mut usize,
    mut f: impl FnMut(Iri) -> bool,
) {
    let rdf_type = [preds().rdf_type];
    for (_, _, a) in k.docs_referencing(&class) {
        for t in objects_of(a, class, &rdf_type, scanned) {
            let Node::Iri(s) = t.s else {
                continue;
            };
            if a.types.get(&s).is_some_and(|t| t.contains(&class)) && !f(s) {
                return;
            }
        }
        if *scanned >= MAX_REFS_SCANNED {
            return;
        }
    }
}

/// Properties whose domain is `class`: stated in the workspace, or in a
/// bundled vocabulary.
fn with_domain(k: &Knowledge, class: Iri) -> Vec<Iri> {
    let p = preds();
    let mut out: Vec<Iri> = Vec::new();
    // One budget for all documents, as for the instances.
    let mut scanned = 0;
    for (_, _, a) in k.docs_referencing(&class) {
        for t in objects_of(a, class, &p.domain, &mut scanned) {
            if let Node::Iri(s) = t.s
                && !out.contains(&s)
                && k.domains(&s).contains(&class)
            {
                out.push(s);
            }
        }
        if scanned >= MAX_REFS_SCANNED {
            break;
        }
    }
    if let Some((v, terms)) = vocab_domains(&class) {
        for &t in terms {
            let iri = Iri::intern(&format!("{}{}", v.namespace, v.terms()[t as usize].local));
            if !out.contains(&iri) {
                out.push(iri);
            }
        }
    }
    out
}

type DomainIndex = FxHashMap<Box<str>, Vec<u32>>;

/// Per bundled vocabulary: domain class -> its properties' term indexes.
/// Built on first use for each vocabulary.
static DOMAINS: LazyLock<Vec<OnceLock<DomainIndex>>> =
    LazyLock::new(|| VOCABS.all.iter().map(|_| OnceLock::new()).collect());

/// Properties of the class's own vocabulary with that class as domain.
fn vocab_domains(class: &str) -> Option<(&'static crate::vocab::Vocabulary, &'static [u32])> {
    let (own, _) = VOCABS.lookup(class)?;
    VOCABS
        .all
        .iter()
        .enumerate()
        .find(|(_, v)| v.namespace == own.namespace)
        .and_then(|(i, v)| {
            let idx = DOMAINS[i].get_or_init(|| {
                let mut m: DomainIndex = FxHashMap::default();
                for (n, t) in v.terms().iter().enumerate() {
                    if t.kind == TermKind::Property && !t.deprecated {
                        for d in &t.domain {
                            m.entry(d.as_str().into()).or_default().push(n as u32);
                        }
                    }
                }
                m
            });
            idx.get(class).map(|t| (v, t.as_slice()))
        })
}

// ---- shapes ----

/// The constraint's path as a list of predicates: a single predicate, or a
/// sequence path of predicates.
fn constraint_path(k: &Knowledge, doc: DocId, pc: &PropertyConstraint) -> Option<Vec<Iri>> {
    if let Some(p) = pc.path {
        return Some(vec![p]);
    }
    let p = preds();
    let a = k.doc(doc);
    // The statement whose sh:property is the constraint (in JSON-LD the
    // constraint is a node object nested in it).
    let (triples, shape) = a.parsed.statements_around(pc.span).find_map(|st| {
        let triples = statement_triples(a, st as u32);
        let shape = triples
            .iter()
            .find(|t| t.p == p.sh_property && t.o_span == pc.span)
            .map(|t| t.o.clone())?;
        Some((triples, shape))
    })?;
    let mut node = triples
        .iter()
        .find(|t| t.s == shape && t.p == p.sh_path)
        .map(|t| t.o.clone())?;
    let mut out = Vec::new();
    while node != Node::Iri(p.rdf_nil) {
        if out.len() >= MAX_DEPTH || !matches!(node, Node::Blank(_)) {
            return None;
        }
        let item = |pred: Iri| {
            triples
                .iter()
                .find(|t| t.s == node && t.p == pred)
                .map(|t| t.o.clone())
        };
        let Some(Node::Iri(step)) = item(p.rdf_first) else {
            return None;
        };
        out.push(step);
        node = item(p.rdf_rest)?;
    }
    Some(out)
}

fn shape_name(k: &Knowledge, doc: DocId, shape: &Shape, prefixes: &PrefixMap) -> String {
    match shape.node {
        Some(n) => super::shorten(&n, prefixes).unwrap_or_else(|| {
            let local = &n[namespace_of(&n).len()..];
            if local.is_empty() {
                n.to_string()
            } else {
                local.to_string()
            }
        }),
        None => {
            let uri = &k.doc(doc).uri;
            format!("a shape in {}", uri.rsplit('/').next().unwrap_or(uri))
        }
    }
}

/// A literal written as Turtle, with its datatype shortened where possible.
fn turtle_literal(n: &Node, prefixes: &PrefixMap) -> Option<String> {
    let Node::Literal {
        value,
        datatype,
        lang,
        dir,
    } = n
    else {
        return None;
    };
    let quoted = || {
        let mut s = String::with_capacity(value.len() + 2);
        s.push('"');
        for c in value.chars() {
            match c {
                '"' => s.push_str("\\\""),
                '\\' => s.push_str("\\\\"),
                '\n' => s.push_str("\\n"),
                '\r' => s.push_str("\\r"),
                '\t' => s.push_str("\\t"),
                c => s.push(c),
            }
        }
        s.push('"');
        s
    };
    if let Some(l) = lang {
        return Some(match dir {
            Some(d) => format!("{}@{}--{d}", quoted(), &**l),
            None => format!("{}@{}", quoted(), &**l),
        });
    }
    let local = datatype.strip_prefix(XSD).unwrap_or("");
    let bare = match local {
        "string" => return Some(quoted()),
        "boolean" => matches!(&**value, "true" | "false"),
        "integer" => is_integer(value),
        "decimal" => value.split_once('.').is_some_and(|(i, f)| {
            (i.is_empty() || is_integer(i))
                && !f.is_empty()
                && f.bytes().all(|b| b.is_ascii_digit())
        }),
        _ => false,
    };
    if bare {
        return Some(value.to_string());
    }
    let dt = super::shorten(datatype, prefixes).unwrap_or_else(|| format!("<{}>", &**datatype));
    Some(format!("{}^^{dt}", quoted()))
}

fn is_integer(s: &str) -> bool {
    let d = s.strip_prefix(['+', '-']).unwrap_or(s);
    !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(value: &str, dt: &str, lang: Option<&str>) -> Node {
        Node::Literal {
            value: value.into(),
            datatype: Iri::intern(dt),
            lang: lang.map(Iri::intern),
            dir: None,
        }
    }

    #[test]
    fn literals_are_written_as_turtle() {
        let x = |l: &str| format!("{XSD}{l}");
        let pm = PrefixMap::from_iter([("xsd".to_string(), XSD.to_string())]);
        let t = |n: Node| turtle_literal(&n, &pm).unwrap();
        assert_eq!(t(lit("a \"b\"\n", &x("string"), None)), r#""a \"b\"\n""#);
        assert_eq!(t(lit("Mill", &x("langString"), Some("en"))), r#""Mill"@en"#);
        assert_eq!(t(lit("-12", &x("integer"), None)), "-12");
        assert_eq!(t(lit("1.5", &x("decimal"), None)), "1.5");
        assert_eq!(t(lit("true", &x("boolean"), None)), "true");
        assert_eq!(t(lit("1e3", &x("double"), None)), r#""1e3"^^xsd:double"#);
        assert_eq!(
            turtle_literal(&lit("2026", &x("gYear"), None), &PrefixMap::default()).unwrap(),
            format!("\"2026\"^^<{XSD}gYear>")
        );
    }

    use crate::knowledge::Source;
    use crate::syntax::PrefixMap as Prefixes;
    use std::sync::Arc;

    const HEAD: &str = "@prefix ex: <http://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";

    fn knowledge_of(docs: &[(&str, String)]) -> Knowledge {
        let mut k = Knowledge::default();
        for (uri, text) in docs {
            let a = Arc::new(Analysis::new(
                uri.to_string(),
                text,
                Arc::new(Prefixes::default()),
                true,
            ));
            k.set(uri, Source::Disk, a);
        }
        k
    }

    fn instances(k: &Knowledge, class: &str) -> (Vec<String>, usize) {
        let mut found = Vec::new();
        let mut scanned = 0;
        for_each_instance(k, Iri::intern(class), &mut scanned, |i| {
            found.push(i.to_string());
            true
        });
        found.sort();
        (found, scanned)
    }

    /// The predicate of an object was found by looking back over at most 64
    /// references: past a long list of objects it was lost. It is the
    /// triple's own.
    #[test]
    fn the_predicate_of_an_object_comes_from_its_triple() {
        let mut text = format!("{HEAD}ex:early a ex:Target .\nex:late a ");
        for i in 0..150 {
            text.push_str(&format!("ex:Other{i} , "));
        }
        text.push_str("ex:Target .\nex:linked ex:uses ex:Target .\n");
        text.push_str("ex:nested ex:has [ a ex:Target ] .\n");
        let k = knowledge_of(&[("file:///w/a.ttl", text)]);
        let (found, _) = instances(&k, "http://example.org/Target");
        // Not `ex:linked` (ex:uses), and not `ex:nested` (its blank node is
        // the instance, not the statement's subject).
        assert_eq!(
            found,
            ["http://example.org/early", "http://example.org/late"]
        );
    }

    /// The 20,000 references that may be looked at were counted again for each
    /// document of `with_domain`, so no number of files was ever cut short.
    #[test]
    fn the_scan_budget_is_shared_across_documents() {
        let docs: Vec<(String, String)> = (0..4)
            .map(|d| {
                let mut t = HEAD.to_string();
                for i in 0..8000 {
                    t.push_str(&format!("ex:s{d}_{i} rdfs:comment ex:C .\n"));
                }
                if d == 3 {
                    t.push_str("ex:p rdfs:domain ex:C .\n");
                }
                (format!("file:///w/d{d}.ttl"), t)
            })
            .collect();
        let refs: Vec<(&str, String)> = docs.iter().map(|(u, t)| (u.as_str(), t.clone())).collect();
        let k = knowledge_of(&refs);
        // 32,000 references to look at over four documents, 20,000 allowed: the
        // domain stated in the last one is past the budget.
        assert!(with_domain(&k, Iri::intern("http://example.org/C")).is_empty());
        // Stated in the first, it is found.
        let mut first = docs[0].1.clone();
        first.push_str("ex:p rdfs:domain ex:C .\n");
        let mut all: Vec<(&str, String)> = refs.clone();
        all[0].1 = first;
        let k = knowledge_of(&all);
        let found = with_domain(&k, Iri::intern("http://example.org/C"));
        assert_eq!(found, [Iri::intern("http://example.org/p")]);
    }
}
