//! The `turtle/schema` request: the workspace's classes, each with a row
//! per property, declared (`rdfs:domain`, `schema:domainIncludes`, SHACL
//! property shapes) against observed (the instances that use it), with a
//! state per row. docs/specs/schema-view.md has the whole contract.
//!
//! Each document's facts are computed once per analysis and kept in
//! [`FactsCache`]; a request merges them. Nothing else is kept between
//! requests, and the index is only read.

use crate::analysis::{Analysis, Encoding};
use crate::features::graph::InvalidParams;
use crate::features::hover::short;
use crate::knowledge::Source;
use crate::syntax::{Iri, PrefixMap, Span, XSD};
use crate::vocab::VOCABS;
use lsp_types::{Position, Range};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The most classes a request can ask for.
pub(crate) const MAX_CLASSES: u32 = 2_000;
/// The most rows per class a request can ask for.
pub(crate) const MAX_ROWS: u32 = 512;
/// The longest `search`, in characters.
pub(crate) const MAX_SEARCH: usize = 200;

// ---- the request ----

/// The `turtle/schema` parameters as sent; every field is optional.
/// [`schema`] checks them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SchemaParams {
    /// Document URIs; omitted: every workspace document.
    pub(crate) files: Option<Vec<String>>,
    /// A class or property IRI.
    pub(crate) focus: Option<String>,
    /// The term at a document position (Follow cursor). The server resolves
    /// it to `focus` before the job starts (see `Session::schema_job`).
    pub(crate) at: Option<At>,
    pub(crate) max_classes: Option<u32>,
    pub(crate) max_rows: Option<u32>,
    /// List imported classes; omitted or false: count them only.
    pub(crate) imports: bool,
    /// A class IRI: the class whose rows list their [`Row::examples`], when
    /// it is listed. Omitted or not listed: the focus class, else none.
    pub(crate) examples: Option<String>,
    /// Text to find among all the classes, listed or not (by name, label or
    /// IRI): the answer's `matches`.
    pub(crate) search: Option<String>,
}

/// A position in a document, as `at` sends it.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct At {
    pub(crate) uri: String,
    pub(crate) position: Position,
}

// ---- the answer ----

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SchemaResult {
    pub(crate) classes: Vec<ClassBox>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) focus_row: Option<u32>,
    pub(crate) total: SchemaTotal,
    /// Imported classes not listed (`imports` false).
    pub(crate) imported: usize,
    /// The classes `search` finds, best first, at most [`MAX_MATCHES`];
    /// empty without `search`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) matches: Vec<Match>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SchemaTotal {
    /// Before the cap.
    pub(crate) classes: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ClassKind {
    /// Declared or used in a workspace document.
    Class,
    /// Declared only by an imported ontology.
    Imported,
    /// Used as a class (a type, a domain, a target) and declared nowhere.
    Undefined,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClassBox {
    pub(crate) id: String,
    /// A prefixed name, else `<iri>`.
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Where the class is defined, when a workspace document does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range: Option<Range>,
    pub(crate) kind: ClassKind,
    /// Indexes into `classes` (rdfs:subClassOf), those listed.
    pub(crate) parents: Vec<u32>,
    /// Typed with the class or one of its subclasses.
    pub(crate) instances: usize,
    /// Rows whose state is not ok, plus rows with `missing` > 0.
    pub(crate) problems: usize,
    pub(crate) rows: Vec<Row>,
    /// Rows past `maxRows`.
    pub(crate) more_rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RowState {
    Ok,
    /// Declared; no instance uses it.
    Unused,
    /// Used by instances; declared for neither the class nor an ancestor.
    Undeclared,
    /// Literal values outside the one declared datatype.
    BadValues,
    /// `rdfs:range` and the shape's datatype or class disagree.
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DeclaredBy {
    Domain,
    DomainIncludes,
    Shape,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Declared {
    pub(crate) how: DeclaredBy,
    pub(crate) uri: String,
    pub(crate) range: Range,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Row {
    pub(crate) property: String,
    pub(crate) name: String,
    /// An object property: its values are resources.
    pub(crate) link: bool,
    /// The range class's index in `classes`, when it is listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range_name: Option<String>,
    pub(crate) declared: Vec<Declared>,
    pub(crate) used: usize,
    pub(crate) of: usize,
    pub(crate) state: RowState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) bad_values: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) required: Option<Required>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) missing: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conflict: Option<Conflict>,
    /// On a conflict row: the values outside the shape's datatype or class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) shape_bad_values: Option<usize>,
    /// On a not-declared row: the classes that do declare the property
    /// (their names, sorted, at most [`MAX_DECLARED_FOR`]), so the view can
    /// say "declared for Pipe, not Gasket".
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) declared_for: Vec<String>,
    /// Where the row's problems are: the first [`MAX_EXAMPLES`] of each
    /// kind, in document order (by URI) and text order. Only on the rows of
    /// one class (`examples`, else the focus); empty on every other row.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) examples: Vec<Example>,
}

/// The most examples of each kind a row lists.
pub(crate) const MAX_EXAMPLES: usize = 10;

/// What an example shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ExampleKind {
    /// An instance with fewer than `required.min` values: where the
    /// instance is first described.
    Missing,
    /// A value outside the declared datatype: the value.
    BadValue,
    /// A use of a property nothing declares: the predicate and object of the
    /// instance's first use.
    Undeclared,
    /// On a conflict row, a value outside the shape's datatype or class: the
    /// value.
    ShapeValue,
}

/// A class `search` found.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Match {
    pub(crate) id: String,
    pub(crate) name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) label: Option<String>,
    /// Its index in `classes`, when it is listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) index: Option<u32>,
}

/// The most classes `search` returns.
pub(crate) const MAX_MATCHES: usize = 20;

/// The most classes a row's `declared_for` names.
pub(crate) const MAX_DECLARED_FOR: usize = 5;

/// One place a row's problem shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Example {
    pub(crate) kind: ExampleKind,
    pub(crate) uri: String,
    pub(crate) range: Range,
    /// The instance: a prefixed name, `<iri>` or `_:b<n>`.
    pub(crate) name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Required {
    pub(crate) min: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Conflict {
    /// What `rdfs:range` says.
    pub(crate) declared: String,
    /// What the shape says (`sh:datatype` or `sh:class`).
    pub(crate) shape: String,
}

mod facts;
#[cfg(test)]
mod tests;

pub(crate) use facts::FactsCache;
use facts::{DocFacts, NO_SUBJECT, ShapePath, Subj, Value, terms};

const DEFAULT_MAX_CLASSES: u32 = 300;
const DEFAULT_MAX_ROWS: u32 = 64;
/// No subject, no row.
const NONE: u32 = u32::MAX;
/// In the row index: no row, as the property is declared for an ancestor.
const COVERED: u32 = u32::MAX - 1;

/// `turtle/schema`: the schema of the documents in `sources` (external
/// sources are the imports), as `params` asks. `focus` is `params.focus`,
/// or the IRI the server resolved `params.at` to.
pub(crate) fn schema(
    sources: &[(String, Source, Arc<Analysis>)],
    enc: Encoding,
    params: SchemaParams,
    focus: Option<String>,
    cache: &FactsCache,
) -> Result<SchemaResult, InvalidParams> {
    if params
        .max_classes
        .is_some_and(|n| n == 0 || n > MAX_CLASSES)
    {
        return Err(InvalidParams(format!(
            "maxClasses must be 1 to {MAX_CLASSES}"
        )));
    }
    if params.max_rows.is_some_and(|n| n == 0 || n > MAX_ROWS) {
        return Err(InvalidParams(format!("maxRows must be 1 to {MAX_ROWS}")));
    }
    if params
        .search
        .as_ref()
        .is_some_and(|s| s.chars().count() > MAX_SEARCH)
    {
        return Err(InvalidParams(format!(
            "search must be at most {MAX_SEARCH} characters"
        )));
    }
    let files: Option<FxHashSet<&str>> = params
        .files
        .as_ref()
        .map(|v| v.iter().map(String::as_str).collect());
    let mut wanted: Vec<usize> = (0..sources.len())
        .filter(|&i| {
            let (_, s, a) = &sources[i];
            *s == Source::External || files.as_ref().is_none_or(|f| f.contains(a.uri.as_str()))
        })
        .collect();
    let order = |i: usize| {
        let (key, s, a) = &sources[i];
        (*s == Source::External, a.uri.as_str(), key.as_str())
    };
    wanted.sort_by(|&i, &j| order(i).cmp(&order(j)));
    let facts = cache.facts(sources, &wanted);
    let docs: Vec<Doc> = wanted
        .iter()
        .zip(facts)
        .map(|(&i, facts)| Doc {
            a: &sources[i].2,
            external: sources[i].1 == Source::External,
            facts,
        })
        .collect();
    let q = Query {
        focus: focus.or(params.focus),
        max_classes: params.max_classes.unwrap_or(DEFAULT_MAX_CLASSES) as usize,
        max_rows: params.max_rows.unwrap_or(DEFAULT_MAX_ROWS) as usize,
        imports: params.imports,
        examples: params.examples,
        search: params
            .search
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty()),
    };
    let (merge, pick) = Merge::new(&docs, &q);
    Ok(merge.answer(&docs, enc, &q, pick))
}

/// [`schema`] for the integration tests (tests/chaos.rs): `docs` as the
/// workspace's disk documents and `imports` as its external sources, each
/// keyed by its URI; `params` as a client sends them (`null` for none), the
/// answer as the client gets it, or the message of the parameter error.
/// Every document's facts are computed afresh.
pub fn answer(
    docs: &[Arc<Analysis>],
    imports: &[Arc<Analysis>],
    enc: Encoding,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let params: SchemaParams = match params {
        serde_json::Value::Null => SchemaParams::default(),
        p => serde_json::from_value(p).map_err(|e| e.to_string())?,
    };
    let sources: Vec<(String, Source, Arc<Analysis>)> = docs
        .iter()
        .map(|a| (a, Source::Disk))
        .chain(imports.iter().map(|a| (a, Source::External)))
        .map(|(a, s)| (a.uri.clone(), s, a.clone()))
        .collect();
    let r = schema(&sources, enc, params, None, &FactsCache::default()).map_err(|e| e.0)?;
    serde_json::to_value(r).map_err(|e| e.to_string())
}

/// [`SchemaParams`] checked, with the defaults applied.
struct Query {
    focus: Option<String>,
    max_classes: usize,
    max_rows: usize,
    imports: bool,
    examples: Option<String>,
    /// `search`, trimmed and lowercased; none when blank.
    search: Option<String>,
}

/// What the answer lists and centres on, settled before the counts.
struct Pick {
    /// Per class: how it is listed, if it is.
    kinds: Vec<Option<ClassKind>>,
    /// Imported classes not listed.
    imported: usize,
    /// The classes listed, before the cap.
    listed: Vec<u32>,
    focus: Option<u32>,
    /// The focus property, when the focus is one.
    focus_prop: Option<Iri>,
    /// The class `examples` names, when it can be listed.
    examples: Option<u32>,
}

/// A document of the request: workspace documents by URI, then imports.
struct Doc<'a> {
    a: &'a Analysis,
    external: bool,
    facts: Arc<DocFacts>,
}

// ---- the merge ----

struct Class {
    iri: Iri,
    /// The first workspace document that declares it, and where.
    defined: Option<(u32, Span)>,
    /// The first document, imports included, that declares it.
    label_doc: Option<u32>,
    ext_declared: bool,
    /// A workspace document uses it as a type, domain, target or in
    /// `rdfs:subClassOf`.
    used: bool,
    supers: Vec<u32>,
    subs: Vec<u32>,
    instances: usize,
    rows: Vec<u32>,
}

#[derive(Clone, PartialEq, Eq)]
enum Prop {
    Iri(Iri),
    Written(Box<str>),
}

struct RowAcc {
    prop: Prop,
    declared: Vec<(DeclaredBy, u32, Span)>,
    /// `sh:datatype` and `sh:class` of its shapes, sorted.
    shape_datatypes: Vec<Iri>,
    shape_classes: Vec<Iri>,
    /// The largest `sh:minCount`, the smallest `sh:maxCount`.
    min: Option<u32>,
    max: Option<u32>,
    /// The one datatype literals are checked against.
    check: Option<Iri>,
    /// The row's first `rdfs:range`, and a shape's `sh:datatype` or
    /// `sh:class` that is not among its ranges.
    conflict: Option<(Iri, Iri)>,
    /// On a conflict row: `shape_classes` as class indexes, [`NONE`] for a
    /// class nothing is typed with.
    shape_class_ix: Vec<u32>,
    /// On a conflict row: values outside a shape's datatype or class.
    shape_bad: usize,
    /// Some value is an IRI or a blank node.
    resource: bool,
    used: usize,
    /// Instances with at least `min` values.
    at_least: usize,
    bad: usize,
    /// The last subject counted, and its values so far.
    stamp: u32,
    count: u32,
    /// Its class may list examples: the places below are collected.
    examples: bool,
    /// The first places of each kind of problem.
    missing_at: Firsts,
    bad_at: Firsts,
    undeclared_at: Firsts,
    shape_at: Firsts,
}

/// A place in a document of the request, and the subject it is about.
#[derive(Clone, Copy)]
struct Spot {
    d: u32,
    span: Span,
    subj: Subj,
}

impl Spot {
    fn key(&self) -> (u32, u32, u32) {
        (self.d, self.span.start, self.span.end)
    }
}

/// The first [`MAX_EXAMPLES`] spots offered, by document then text order;
/// of equal spots, the first offered.
#[derive(Default)]
struct Firsts(Vec<Spot>);

impl Firsts {
    fn offer(&mut self, spot: Spot) {
        let key = spot.key();
        let full = self.0.len() == MAX_EXAMPLES;
        if full && self.0[MAX_EXAMPLES - 1].key() <= key {
            return;
        }
        if self.0.is_empty() {
            self.0.reserve_exact(MAX_EXAMPLES);
        }
        if full {
            self.0.pop();
        }
        let at = self.0.partition_point(|x| x.key() <= key);
        self.0.insert(at, spot);
    }
}

impl RowAcc {
    fn new(prop: Prop) -> RowAcc {
        RowAcc {
            prop,
            declared: Vec::new(),
            shape_datatypes: Vec::new(),
            shape_classes: Vec::new(),
            min: None,
            max: None,
            check: None,
            conflict: None,
            shape_class_ix: Vec::new(),
            shape_bad: 0,
            resource: false,
            used: 0,
            at_least: 0,
            bad: 0,
            stamp: NONE,
            count: 0,
            examples: false,
            missing_at: Firsts::default(),
            bad_at: Firsts::default(),
            undeclared_at: Firsts::default(),
            shape_at: Firsts::default(),
        }
    }

    fn required(&self) -> Option<u32> {
        self.min.filter(|&m| m >= 1)
    }
}

#[derive(Default)]
struct Merge {
    classes: Vec<Class>,
    class_ix: FxHashMap<Iri, u32>,
    rows: Vec<RowAcc>,
    /// (class, property) -> row, or [`COVERED`].
    row_ix: FxHashMap<(u32, Iri), u32>,
    /// Property -> its `rdfs:range`s, sorted.
    ranges: FxHashMap<Iri, Vec<Iri>>,
    /// Property -> the classes a domain or a shape declares it for.
    declarers: FxHashMap<Iri, Vec<u32>>,
    object_props: FxHashSet<Iri>,
    datatype_props: FxHashSet<Iri>,
    standard: FxHashMap<Iri, bool>,
    /// Sets of classes (a subject's direct types) -> their member classes:
    /// the types and every ancestor, sorted.
    sets: FxHashMap<Box<[u32]>, u32>,
    set_off: Vec<u32>,
    set_list: Vec<u32>,
    /// Per class: the generation it was last reached in.
    seen: Vec<u32>,
    generation: u32,
}

/// Uses, grouped by subject: the subject's are `list[off[s]..off[s + 1]]`.
struct Grouped<T> {
    off: Vec<u32>,
    list: Vec<T>,
}

impl<T: Copy> Grouped<T> {
    /// `items` yields (subject, item) twice: once to count, once to fill
    /// the slots, which start as `blank`.
    fn new<I: Iterator<Item = (u32, T)>>(n: usize, blank: T, items: impl Fn() -> I) -> Grouped<T> {
        let mut off = vec![0u32; n + 1];
        for (s, _) in items() {
            off[s as usize + 1] += 1;
        }
        for i in 1..off.len() {
            off[i] += off[i - 1];
        }
        let mut fill = off.clone();
        let mut list = vec![blank; off[n] as usize];
        for (s, t) in items() {
            let slot = &mut fill[s as usize];
            list[*slot as usize] = t;
            *slot += 1;
        }
        Grouped { off, list }
    }

    fn of(&self, s: usize) -> &[T] {
        &self.list[self.off[s] as usize..self.off[s + 1] as usize]
    }
}

/// A standard vocabulary's term: one of the bundled vocabularies'.
fn is_standard(iri: &str) -> bool {
    VOCABS.lookup(iri).is_some()
}

/// A datatype a range or `sh:datatype` can name (`rdfs:Literal` included).
fn is_datatype(i: Iri) -> bool {
    let t = terms();
    i.starts_with(XSD) || t.rdf_datatypes.contains(&i) || i == t.rdfs_literal
}

/// A datatype a literal can be checked against.
fn checkable(i: Iri) -> bool {
    is_datatype(i) && i != terms().rdfs_literal
}

fn sorted(mut v: Vec<Iri>) -> Vec<Iri> {
    v.sort_unstable();
    v.dedup();
    v
}

impl Merge {
    /// The merge of `docs`, and what `q` picks. Problem places are
    /// collected only on the rows of the focus class and of the class
    /// `q.examples` names.
    fn new(docs: &[Doc], q: &Query) -> (Merge, Pick) {
        let mut m = Merge::default();

        // Classes, the hierarchy, the declarations.
        let mut type_classes: Vec<Vec<u32>> = Vec::with_capacity(docs.len());
        for (d, doc) in docs.iter().enumerate() {
            let (f, ws) = (&doc.facts, !doc.external);
            let d = d as u32;
            for &(c, span) in &f.classes {
                let k = m.class(c);
                let class = &mut m.classes[k as usize];
                if ws {
                    class.defined.get_or_insert((d, span));
                } else {
                    class.ext_declared = true;
                }
                class.label_doc.get_or_insert(d);
            }
            for &(sub, sup) in &f.subclass {
                let (a, b) = (m.class(sub), m.class(sup));
                if a != b {
                    m.classes[a as usize].supers.push(b);
                    m.classes[b as usize].subs.push(a);
                }
                m.classes[a as usize].used |= ws;
                m.classes[b as usize].used |= ws;
            }
            let mut tc = Vec::with_capacity(f.types.len());
            for &(_, c) in &f.types {
                let k = m.class(c);
                m.classes[k as usize].used |= ws;
                tc.push(k);
            }
            type_classes.push(tc);
            for decl in &f.decls {
                let k = m.class(decl.class);
                m.classes[k as usize].used |= ws;
                let r = m.row(k, decl.p);
                m.rows[r as usize].declared.push((decl.how, d, decl.span));
                m.declarers.entry(decl.p).or_default().push(k);
            }
            for s in &f.shape_rows {
                let k = m.class(s.class);
                m.classes[k as usize].used |= ws;
                let r = match &s.path {
                    ShapePath::Predicate(p) => {
                        m.declarers.entry(*p).or_default().push(k);
                        m.row(k, *p)
                    }
                    ShapePath::Written(w) => m.written_row(k, w),
                };
                let row = &mut m.rows[r as usize];
                row.declared.push((DeclaredBy::Shape, d, s.span));
                row.min = row.min.max(s.min);
                row.max = match (row.max, s.max) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                row.shape_datatypes.extend(s.datatype);
                row.shape_classes.extend(s.of_class);
            }
            for &c in &f.targets {
                let k = m.class(c);
                m.classes[k as usize].used |= ws;
            }
            for &(p, r) in &f.ranges {
                m.ranges.entry(p).or_default().push(r);
            }
            m.object_props.extend(f.object_props.iter().copied());
            m.datatype_props.extend(f.datatype_props.iter().copied());
        }
        for c in &mut m.classes {
            c.supers.sort_unstable();
            c.supers.dedup();
            c.subs.sort_unstable();
            c.subs.dedup();
        }
        for r in m.ranges.values_mut() {
            *r = sorted(std::mem::take(r));
        }
        // The typed subjects, joined across documents: IRIs by IRI, blank
        // nodes per document.
        let mut by_iri: FxHashMap<Iri, u32> = FxHashMap::default();
        let mut n: u32 = 0;
        let mut local: Vec<Vec<u32>> = Vec::with_capacity(docs.len());
        for doc in docs {
            let f = &doc.facts;
            let mut l = vec![NONE; f.subjects.len()];
            for &(s, _) in &f.types {
                let slot = &mut l[s as usize];
                if *slot == NONE {
                    *slot = match f.subjects[s as usize] {
                        facts::Subj::Iri(i) => *by_iri.entry(i).or_insert_with(|| {
                            n += 1;
                            n - 1
                        }),
                        facts::Subj::Blank(_) => {
                            n += 1;
                            n - 1
                        }
                    };
                }
            }
            local.push(l);
        }
        for (doc, l) in docs.iter().zip(&mut local) {
            for (slot, s) in l.iter_mut().zip(&doc.facts.subjects) {
                if *slot == NONE
                    && let facts::Subj::Iri(i) = s
                    && let Some(&g) = by_iri.get(i)
                {
                    *slot = g;
                }
            }
        }
        let n = n as usize;
        // Where each typed subject is first named: its first document, its
        // index there.
        let mut home = vec![(NONE, 0u32); n];
        for (d, l) in local.iter().enumerate() {
            for (j, &g) in l.iter().enumerate() {
                if g != NONE && home[g as usize].0 == NONE {
                    home[g as usize] = (d as u32, j as u32);
                }
            }
        }

        // Each subject's direct types, then its member classes.
        let mut types = Grouped::new(n, 0, || {
            docs.iter().enumerate().flat_map(|(d, doc)| {
                let l = &local[d];
                doc.facts
                    .types
                    .iter()
                    .zip(&type_classes[d])
                    .map(move |(&(s, _), &c)| (l[s as usize], c))
            })
        });
        let mut type_len = vec![0u32; n];
        for (s, len) in type_len.iter_mut().enumerate() {
            let (a, b) = (types.off[s] as usize, types.off[s + 1] as usize);
            let ts = &mut types.list[a..b];
            ts.sort_unstable();
            let mut k = 0;
            for i in 0..ts.len() {
                if i == 0 || ts[i] != ts[k - 1] {
                    ts[k] = ts[i];
                    k += 1;
                }
            }
            *len = k as u32;
        }
        let direct = |s: usize| &types.list[types.off[s] as usize..][..type_len[s] as usize];
        m.seen = vec![0; m.classes.len()];
        let mut subject_set = vec![0u32; n];
        let mut set_count: Vec<usize> = Vec::new();
        for (s, set) in subject_set.iter_mut().enumerate() {
            *set = m.set(direct(s));
            if set_count.len() <= *set as usize {
                set_count.resize(*set as usize + 1, 0);
            }
            set_count[*set as usize] += 1;
        }
        for (set, &count) in set_count.iter().enumerate() {
            let (a, b) = (m.set_off[set] as usize, m.set_off[set + 1] as usize);
            for &c in &m.set_list[a..b] {
                m.classes[c as usize].instances += count;
            }
        }

        // The uses of the typed subjects.
        // Each carries its document and its index in the document's uses.
        let uses = Grouped::new(
            n,
            (crate::shacl::vocab::get().rdf_type, Value::Other, 0, 0),
            || {
                docs.iter().enumerate().flat_map(|(d, doc)| {
                    let l = &local[d];
                    doc.facts
                        .uses
                        .iter()
                        .enumerate()
                        .filter(move |(_, u)| l[u.s as usize] != NONE)
                        .map(move |(i, u)| (l[u.s as usize], (u.p, u.v, d as u32, i as u32)))
                })
            },
        );

        // Rows nothing declares, on the subjects' direct types.
        for s in 0..n {
            for &(p, ..) in uses.of(s) {
                if !m.is_standard_property(p) {
                    for &c in direct(s) {
                        m.undeclared(c, p);
                    }
                }
            }
        }

        // The classes whose rows collect examples.
        let pick = m.pick(q, docs, &type_classes);
        for c in pick.focus.iter().chain(&pick.examples) {
            for &r in &m.classes[*c as usize].rows {
                m.rows[r as usize].examples = true;
            }
        }

        // The datatype each row checks literals against, the rows nothing
        // declares included (a range is the property's).
        for i in 0..m.rows.len() {
            let row = &mut m.rows[i];
            row.shape_datatypes = sorted(std::mem::take(&mut row.shape_datatypes));
            row.shape_classes = sorted(std::mem::take(&mut row.shape_classes));
            let Prop::Iri(p) = row.prop else { continue };
            let mut dts: Vec<Iri> = m
                .ranges
                .get(&p)
                .into_iter()
                .flatten()
                .chain(&row.shape_datatypes)
                .copied()
                .filter(|&d| checkable(d))
                .collect();
            dts.sort_unstable();
            dts.dedup();
            if dts.len() == 1 {
                row.check = Some(dts[0]);
            }
            let shaped = row.declared.iter().any(|d| d.0 == DeclaredBy::Shape);
            if let Some(ranges) = m.ranges.get(&p).filter(|_| shaped)
                && let Some(&shape) = row
                    .shape_datatypes
                    .iter()
                    .chain(&row.shape_classes)
                    .find(|v| !ranges.contains(v))
            {
                row.conflict = Some((ranges[0], shape));
                row.shape_class_ix = row
                    .shape_classes
                    .iter()
                    .map(|c| m.class_ix.get(c).copied().unwrap_or(NONE))
                    .collect();
            }
        }

        // The required rows that collect examples, per class, and whether a
        // set has any.
        let required = Grouped::new(m.classes.len(), 0, || {
            m.classes.iter().enumerate().flat_map(|(c, class)| {
                class
                    .rows
                    .iter()
                    .filter(|&&r| {
                        let row = &m.rows[r as usize];
                        row.examples && matches!(row.prop, Prop::Iri(_)) && row.required().is_some()
                    })
                    .map(move |&r| (c as u32, r))
            })
        });
        let set_required: Vec<bool> = (0..m.set_off.len().saturating_sub(1))
            .map(|set| {
                m.set_list[m.set_off[set] as usize..m.set_off[set + 1] as usize]
                    .iter()
                    .any(|&c| !required.of(c as usize).is_empty())
            })
            .collect();
        // A use's place: its predicate and object, or its object alone.
        let use_spot = |d: u32, i: u32, object: bool| {
            let f = &docs[d as usize].facts;
            let u = &f.uses[i as usize];
            let t = &docs[d as usize].a.parsed.triples[u.t as usize];
            Spot {
                d,
                span: if object {
                    t.o_span
                } else {
                    facts::cover(t.p_span, t.o_span)
                },
                subj: f.subjects[u.s as usize],
            }
        };

        // Whether a value is typed with class `k` or a subclass of it; `d`
        // is the use's document.
        let rdfs_literal = terms().rdfs_literal;
        let typed = |v: Value, d: u32, k: u32| {
            let g = match v {
                Value::Iri(i) => by_iri.get(&i).copied(),
                Value::Blank(j) if j != NO_SUBJECT => Some(local[d as usize][j as usize]),
                _ => None,
            };
            g.filter(|&g| g != NONE).is_some_and(|g| {
                let set = subject_set[g as usize] as usize;
                m.set_list[m.set_off[set] as usize..m.set_off[set + 1] as usize]
                    .binary_search(&k)
                    .is_ok()
            })
        };

        // The counts.
        for (s, &set) in subject_set.iter().enumerate() {
            let set = set as usize;
            let members = &m.set_list[m.set_off[set] as usize..m.set_off[set + 1] as usize];
            for &(p, v, d, i) in uses.of(s) {
                for &c in members {
                    let Some(&r) = m.row_ix.get(&(c, p)) else {
                        continue;
                    };
                    if r == COVERED {
                        continue;
                    }
                    let row = &mut m.rows[r as usize];
                    if row.stamp != s as u32 {
                        row.stamp = s as u32;
                        row.count = 0;
                        row.used += 1;
                        if row.examples && row.declared.is_empty() {
                            row.undeclared_at.offer(use_spot(d, i, false));
                        }
                    }
                    row.count += 1;
                    if row.required() == Some(row.count) {
                        row.at_least += 1;
                    }
                    match v {
                        Value::Literal(dt) => {
                            if row.check.is_some_and(|c| c != dt) {
                                row.bad += 1;
                                if row.examples {
                                    row.bad_at.offer(use_spot(d, i, true));
                                }
                            }
                        }
                        Value::Iri(_) | Value::Blank(_) => row.resource = true,
                        Value::Other => {}
                    }
                    // A conflict row: the value against every datatype and
                    // class of its shapes.
                    if row.conflict.is_some() {
                        let fits = row.shape_datatypes.iter().all(
                            |&dt| matches!(v, Value::Literal(x) if x == dt || dt == rdfs_literal),
                        ) && row
                            .shape_class_ix
                            .iter()
                            .all(|&k| k != NONE && typed(v, d, k));
                        if !fits {
                            row.shape_bad += 1;
                            if row.examples {
                                row.shape_at.offer(use_spot(d, i, true));
                            }
                        }
                    }
                }
            }
            // The required rows this subject has too few values for.
            if set_required[set] {
                let (d, j) = home[s];
                let f = &docs[d as usize].facts;
                let spot = Spot {
                    d,
                    span: f.spans[j as usize],
                    subj: f.subjects[j as usize],
                };
                for &c in members {
                    for &r in required.of(c as usize) {
                        let row = &mut m.rows[r as usize];
                        let min = row.required().unwrap_or(1);
                        if row.stamp != s as u32 || row.count < min {
                            row.missing_at.offer(spot);
                        }
                    }
                }
            }
        }
        (m, pick)
    }

    fn class(&mut self, iri: Iri) -> u32 {
        if let Some(&k) = self.class_ix.get(&iri) {
            return k;
        }
        let k = self.classes.len() as u32;
        self.class_ix.insert(iri, k);
        self.classes.push(Class {
            iri,
            defined: None,
            label_doc: None,
            ext_declared: false,
            used: false,
            supers: Vec::new(),
            subs: Vec::new(),
            instances: 0,
            rows: Vec::new(),
        });
        k
    }

    fn new_row(&mut self, class: u32, prop: Prop) -> u32 {
        let r = self.rows.len() as u32;
        self.rows.push(RowAcc::new(prop));
        self.classes[class as usize].rows.push(r);
        r
    }

    fn row(&mut self, class: u32, p: Iri) -> u32 {
        if let Some(&r) = self.row_ix.get(&(class, p)) {
            return r;
        }
        let r = self.new_row(class, Prop::Iri(p));
        self.row_ix.insert((class, p), r);
        r
    }

    fn written_row(&mut self, class: u32, w: &str) -> u32 {
        let found = self.classes[class as usize]
            .rows
            .iter()
            .copied()
            .find(|&r| matches!(&self.rows[r as usize].prop, Prop::Written(x) if &**x == w));
        found.unwrap_or_else(|| self.new_row(class, Prop::Written(w.into())))
    }

    fn is_standard_property(&mut self, p: Iri) -> bool {
        *self.standard.entry(p).or_insert_with(|| is_standard(&p))
    }

    /// Makes `p` a row of `c` with nothing declaring it, unless `c` or an
    /// ancestor declares it.
    fn undeclared(&mut self, c: u32, p: Iri) {
        if self.row_ix.contains_key(&(c, p)) {
            return;
        }
        let set = self.set(&[c]) as usize;
        let (a, b) = (self.set_off[set] as usize, self.set_off[set + 1] as usize);
        let covered = self.set_list[a..b].iter().any(|&anc| {
            self.row_ix
                .get(&(anc, p))
                .is_some_and(|&r| r != COVERED && !self.rows[r as usize].declared.is_empty())
        });
        if covered {
            self.row_ix.insert((c, p), COVERED);
        } else {
            self.row(c, p);
        }
    }

    /// The set of classes `types` and their ancestors (cycle-safe), made
    /// once per distinct `types`.
    fn set(&mut self, types: &[u32]) -> u32 {
        if let Some(&s) = self.sets.get(types) {
            return s;
        }
        if self.set_off.is_empty() {
            self.set_off.push(0);
        }
        self.generation += 1;
        let start = self.set_list.len();
        for &t in types {
            if self.seen[t as usize] != self.generation {
                self.seen[t as usize] = self.generation;
                self.set_list.push(t);
            }
        }
        let mut i = start;
        while i < self.set_list.len() {
            let c = self.set_list[i] as usize;
            for k in 0..self.classes[c].supers.len() {
                let sup = self.classes[c].supers[k];
                if self.seen[sup as usize] != self.generation {
                    self.seen[sup as usize] = self.generation;
                    self.set_list.push(sup);
                }
            }
            i += 1;
        }
        self.set_list[start..].sort_unstable();
        self.set_off.push(self.set_list.len() as u32);
        let s = (self.set_off.len() - 2) as u32;
        self.sets.insert(types.into(), s);
        s
    }

    // ---- the answer ----

    /// How the class is listed, if it is.
    fn kind(&self, c: &Class) -> Option<ClassKind> {
        if c.defined.is_some() {
            Some(ClassKind::Class)
        } else if c.ext_declared {
            Some(ClassKind::Imported)
        } else if !c.used {
            None
        } else if is_standard(&c.iri) {
            // A standard vocabulary's class (`owl:Class`, `schema:Person`)
            // is listed when the workspace declares properties for it.
            c.rows
                .iter()
                .any(|&r| !self.rows[r as usize].declared.is_empty())
                .then_some(ClassKind::Class)
        } else {
            Some(ClassKind::Undefined)
        }
    }

    /// What `q` lists and centres on. Reads the classes, the declarations
    /// and the rows, not the counts; `type_classes` are the classes of each
    /// document's `types`.
    fn pick(&self, q: &Query, docs: &[Doc], type_classes: &[Vec<u32>]) -> Pick {
        let mut kinds: Vec<Option<ClassKind>> = self.classes.iter().map(|c| self.kind(c)).collect();
        let mut imported = 0;
        if !q.imports {
            for k in &mut kinds {
                if *k == Some(ClassKind::Imported) {
                    *k = None;
                    imported += 1;
                }
            }
        }
        let listed: Vec<u32> = (0..self.classes.len() as u32)
            .filter(|&c| kinds[c as usize].is_some())
            .collect();
        let by_rank = |a: &u32, b: &u32| self.by_rank(a, b);

        // The focus: a listed class, the class a property is a row of, or
        // an instance's first listed type.
        let focus_iri = q.focus.as_deref().and_then(Iri::get);
        let mut focus: Option<u32> = None;
        let mut focus_prop: Option<Iri> = None;
        if let Some(f) = focus_iri {
            if let Some(&c) = self.class_ix.get(&f)
                && kinds[c as usize].is_some()
            {
                focus = Some(c);
            } else {
                let row_of = |c: u32| {
                    self.row_ix
                        .get(&(c, f))
                        .filter(|&&r| r != COVERED)
                        .map(|&r| &self.rows[r as usize])
                };
                focus = listed
                    .iter()
                    .copied()
                    .filter(|&c| row_of(c).is_some())
                    .min_by(|a, b| {
                        let declared = |c: u32| row_of(c).is_some_and(|r| !r.declared.is_empty());
                        declared(*b).cmp(&declared(*a)).then_with(|| by_rank(a, b))
                    });
                focus_prop = focus.map(|_| f);
                if focus.is_none() && !self.class_ix.contains_key(&f) {
                    focus = self.first_type(f, docs, type_classes, &kinds);
                }
            }
        }
        let examples = q
            .examples
            .as_deref()
            .and_then(Iri::get)
            .and_then(|e| self.class_ix.get(&e).copied())
            .filter(|&c| kinds[c as usize].is_some());
        Pick {
            kinds,
            imported,
            listed,
            focus,
            focus_prop,
            examples,
        }
    }

    /// The first listed class `iri` is typed with, as the documents state
    /// their types (by document order, then text order).
    fn first_type(
        &self,
        iri: Iri,
        docs: &[Doc],
        type_classes: &[Vec<u32>],
        kinds: &[Option<ClassKind>],
    ) -> Option<u32> {
        docs.iter().zip(type_classes).find_map(|(doc, tc)| {
            let f = &doc.facts;
            f.types
                .iter()
                .zip(tc)
                .find(|((s, _), k)| {
                    f.subjects[*s as usize] == Subj::Iri(iri) && kinds[**k as usize].is_some()
                })
                .map(|(_, &k)| k)
        })
    }

    /// The classes of `listed` that `search` (lowercased) finds, best first:
    /// the local name equal to it, then the name, local name or label
    /// starting with it, then the name, label or IRI containing it; ties by
    /// rank.
    fn search(
        &self,
        docs: &[Doc],
        names: &Names,
        listed: &[u32],
        out_ix: &[u32],
        search: &str,
    ) -> Vec<Match> {
        let mut found: Vec<(u8, u32, String, Option<String>)> = Vec::new();
        for &c in listed {
            let class = &self.classes[c as usize];
            let iri = class.iri.as_str();
            let name = names.short(iri);
            let label = label(docs, class);
            let local = iri
                .rsplit(['#', '/', ':'])
                .next()
                .unwrap_or(iri)
                .to_lowercase();
            let name_lc = name.to_lowercase();
            let label_lc = label.as_deref().map(str::to_lowercase);
            let label_has = |f: &dyn Fn(&str) -> bool| label_lc.as_deref().is_some_and(f);
            let tier = if local == search {
                0
            } else if name_lc.starts_with(search)
                || local.starts_with(search)
                || label_has(&|l| l.starts_with(search))
            {
                1
            } else if name_lc.contains(search)
                || label_has(&|l| l.contains(search))
                || iri.to_lowercase().contains(search)
            {
                2
            } else {
                continue;
            };
            found.push((tier, c, name, label));
        }
        found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| self.by_rank(&a.1, &b.1)));
        found.truncate(MAX_MATCHES);
        found
            .into_iter()
            .map(|(_, c, name, label)| Match {
                id: self.classes[c as usize].iri.to_string(),
                name,
                label,
                index: Some(out_ix[c as usize]).filter(|&i| i != NONE),
            })
            .collect()
    }

    fn by_rank(&self, a: &u32, b: &u32) -> std::cmp::Ordering {
        let (x, y) = (&self.classes[*a as usize], &self.classes[*b as usize]);
        y.instances
            .cmp(&x.instances)
            .then_with(|| x.iri.as_str().cmp(y.iri.as_str()))
    }

    fn answer(&self, docs: &[Doc], enc: Encoding, q: &Query, pick: Pick) -> SchemaResult {
        let Pick {
            kinds,
            imported,
            listed,
            focus,
            focus_prop,
            examples,
        } = pick;
        let total = listed.len();
        let by_rank = |a: &u32, b: &u32| self.by_rank(a, b);

        // The order: the root, its ancestors and descendants by distance,
        // its link targets, then the rest by instances; ties by IRI.
        let root = focus.or_else(|| listed.iter().copied().min_by(by_rank));
        let mut placed = vec![false; self.classes.len()];
        let mut order: Vec<u32> = Vec::with_capacity(total);
        if let Some(root) = root {
            placed[root as usize] = true;
            order.push(root);
            let mut walked_up = vec![false; self.classes.len()];
            let mut walked_down = vec![false; self.classes.len()];
            walked_up[root as usize] = true;
            walked_down[root as usize] = true;
            let (mut up, mut down) = (vec![root], vec![root]);
            while !(up.is_empty() && down.is_empty()) {
                let mut next_up = Vec::new();
                for &c in &up {
                    for &s in &self.classes[c as usize].supers {
                        if !walked_up[s as usize] {
                            walked_up[s as usize] = true;
                            next_up.push(s);
                        }
                    }
                }
                let mut next_down = Vec::new();
                for &c in &down {
                    for &s in &self.classes[c as usize].subs {
                        if !walked_down[s as usize] {
                            walked_down[s as usize] = true;
                            next_down.push(s);
                        }
                    }
                }
                let mut level: Vec<u32> = next_up
                    .iter()
                    .chain(&next_down)
                    .copied()
                    .filter(|&c| kinds[c as usize].is_some() && !placed[c as usize])
                    .collect();
                level.sort_by(by_rank);
                level.dedup();
                for &c in &level {
                    placed[c as usize] = true;
                }
                order.extend(level);
                (up, down) = (next_up, next_down);
            }
            let mut targets: Vec<u32> = self.classes[root as usize]
                .rows
                .iter()
                .filter_map(|&r| self.target_class(&self.rows[r as usize]))
                .filter(|&c| kinds[c as usize].is_some() && !placed[c as usize])
                .collect();
            targets.sort_by(by_rank);
            targets.dedup();
            for &c in &targets {
                placed[c as usize] = true;
            }
            order.extend(targets);
            let mut rest: Vec<u32> = listed
                .iter()
                .copied()
                .filter(|&c| !placed[c as usize])
                .collect();
            rest.sort_by(by_rank);
            order.extend(rest);
        }
        order.truncate(q.max_classes);
        let mut out_ix = vec![NONE; self.classes.len()];
        for (i, &c) in order.iter().enumerate() {
            out_ix[c as usize] = i as u32;
        }

        // The class whose rows list examples: `examples` when listed, else
        // the focus.
        let examples = examples.filter(|&c| out_ix[c as usize] != NONE).or(focus);
        let names = Names::new(docs);
        let matches = match &q.search {
            Some(search) => self.search(docs, &names, &listed, &out_ix, search),
            None => Vec::new(),
        };
        let mut focus_row = None;
        let classes: Vec<ClassBox> = order
            .iter()
            .map(|&c| {
                let wanted = (Some(c) == focus).then_some(focus_prop).flatten();
                let (b, row) = self.class_box(
                    docs,
                    enc,
                    &names,
                    c,
                    kinds[c as usize].expect("listed"),
                    &out_ix,
                    q.max_rows,
                    wanted,
                    Some(c) == examples,
                );
                if row.is_some() {
                    focus_row = row;
                }
                b
            })
            .collect();
        SchemaResult {
            classes,
            focus: focus.map(|c| out_ix[c as usize]).filter(|&i| i != NONE),
            focus_row,
            total: SchemaTotal { classes: total },
            imported,
            matches,
        }
    }

    /// The class a link row points to: its first `rdfs:range` that is not a
    /// datatype, else its first `sh:class`.
    fn target_class(&self, row: &RowAcc) -> Option<u32> {
        if !self.is_link(row) {
            return None;
        }
        let ranges = match row.prop {
            Prop::Iri(p) => self.ranges.get(&p).map(Vec::as_slice).unwrap_or(&[]),
            Prop::Written(_) => &[],
        };
        let iri = ranges
            .iter()
            .copied()
            .find(|&r| !is_datatype(r))
            .or_else(|| row.shape_classes.first().copied())?;
        self.class_ix.get(&iri).copied()
    }

    /// Its values are resources: by its range, else its OWL type, else its
    /// shapes, else its values.
    fn is_link(&self, row: &RowAcc) -> bool {
        let Prop::Iri(p) = row.prop else {
            return !row.shape_classes.is_empty();
        };
        if let Some(r) = self.ranges.get(&p).and_then(|r| r.first()) {
            return !is_datatype(*r);
        }
        if self.object_props.contains(&p) {
            return true;
        }
        if self.datatype_props.contains(&p) {
            return false;
        }
        if !row.shape_classes.is_empty() {
            return true;
        }
        if !row.shape_datatypes.is_empty() {
            return false;
        }
        row.resource
    }

    #[allow(clippy::too_many_arguments)]
    fn class_box(
        &self,
        docs: &[Doc],
        enc: Encoding,
        names: &Names,
        c: u32,
        kind: ClassKind,
        out_ix: &[u32],
        max_rows: usize,
        focus_prop: Option<Iri>,
        examples: bool,
    ) -> (ClassBox, Option<u32>) {
        let class = &self.classes[c as usize];
        let of = class.instances;
        let mut rows: Vec<(Row, u8)> = class
            .rows
            .iter()
            .map(|&r| {
                let row = &self.rows[r as usize];
                self.row_out(docs, enc, names, row, of, out_ix, examples)
            })
            .collect();
        let problems = rows.iter().filter(|(_, sev)| *sev > 0).count();
        let undeclared = |r: &Row| r.state == RowState::Undeclared;
        rows.sort_by(|(a, _), (b, _)| {
            undeclared(a)
                .cmp(&undeclared(b))
                .then_with(|| a.property.cmp(&b.property))
        });
        let wanted = focus_prop.map(|p| p.as_str());
        let mut more_rows = 0;
        if rows.len() > max_rows {
            more_rows = rows.len() - max_rows;
            let mut rank: Vec<usize> = (0..rows.len()).collect();
            rank.sort_by(|&a, &b| {
                let focused = |i: usize| Some(rows[i].0.property.as_str()) == wanted;
                focused(b)
                    .cmp(&focused(a))
                    .then(rows[b].1.cmp(&rows[a].1))
                    .then(a.cmp(&b))
            });
            let mut keep = vec![false; rows.len()];
            for &i in &rank[..max_rows] {
                keep[i] = true;
            }
            let mut i = 0;
            rows.retain(|_| {
                i += 1;
                keep[i - 1]
            });
        }
        let rows: Vec<Row> = rows.into_iter().map(|(r, _)| r).collect();
        let focus_row = wanted
            .and_then(|w| rows.iter().position(|r| r.property == w))
            .map(|i| i as u32);
        let iri = class.iri;
        let (uri, range) = match class.defined {
            Some((d, span)) => {
                let a = docs[d as usize].a;
                (Some(a.uri.clone()), Some(a.range(enc, span)))
            }
            None => (None, None),
        };
        let mut parents: Vec<u32> = class
            .supers
            .iter()
            .map(|&s| out_ix[s as usize])
            .filter(|&i| i != NONE)
            .collect();
        parents.sort_unstable();
        let b = ClassBox {
            id: iri.to_string(),
            name: names.short(&iri),
            label: label(docs, class),
            uri,
            range,
            kind,
            parents,
            instances: of,
            problems,
            rows,
            more_rows,
        };
        (b, focus_row)
    }

    /// The row as answered, with its examples when `examples`, and how bad
    /// it is (0: no problem).
    #[allow(clippy::too_many_arguments)]
    fn row_out(
        &self,
        docs: &[Doc],
        enc: Encoding,
        names: &Names,
        row: &RowAcc,
        of: usize,
        out_ix: &[u32],
        with_examples: bool,
    ) -> (Row, u8) {
        let (property, name, ranges) = match &row.prop {
            Prop::Iri(p) => (
                p.to_string(),
                names.short(p),
                self.ranges.get(p).map(Vec::as_slice).unwrap_or(&[]),
            ),
            Prop::Written(w) => (w.to_string(), w.to_string(), &[][..]),
        };
        let range = ranges
            .first()
            .or_else(|| row.shape_classes.first())
            .or_else(|| row.shape_datatypes.first())
            .copied();
        let link = self.is_link(row);
        let target = self
            .target_class(row)
            .map(|c| out_ix[c as usize])
            .filter(|&i| i != NONE);
        let conflict = row.conflict.map(|(declared, shape)| Conflict {
            declared: names.short(&declared),
            shape: names.short(&shape),
        });
        let counted = matches!(row.prop, Prop::Iri(_));
        let (used, bad, shape_bad) = if of == 0 {
            (0, 0, 0)
        } else {
            (row.used, row.bad, row.shape_bad)
        };
        let state = if row.declared.is_empty() {
            RowState::Undeclared
        } else if conflict.is_some() {
            RowState::Conflict
        } else if bad > 0 {
            RowState::BadValues
        } else if counted && of > 0 && used == 0 {
            RowState::Unused
        } else {
            RowState::Ok
        };
        let missing = match row.required() {
            Some(_) if counted => Some(of - if of == 0 { 0 } else { row.at_least }),
            _ => None,
        }
        .filter(|&m| m > 0);
        let severity = match state {
            RowState::Conflict => 5,
            RowState::BadValues => 4,
            _ if missing.is_some() => 3,
            RowState::Undeclared => 2,
            RowState::Unused => 1,
            RowState::Ok => 0,
        };
        let declared = row
            .declared
            .iter()
            .map(|&(how, d, span)| {
                let a = docs[d as usize].a;
                Declared {
                    how,
                    uri: a.uri.clone(),
                    range: a.range(enc, span),
                }
            })
            .collect();
        let mut examples = Vec::new();
        let mut add = |kind: ExampleKind, at: &Firsts| {
            if !with_examples {
                return;
            }
            examples.extend(at.0.iter().map(|spot| {
                let a = docs[spot.d as usize].a;
                Example {
                    kind,
                    uri: a.uri.clone(),
                    range: a.range(enc, spot.span),
                    name: match spot.subj {
                        Subj::Iri(i) => names.short(&i),
                        Subj::Blank(b) => format!("_:b{}", b.0),
                    },
                }
            }))
        };
        if missing.is_some() {
            add(ExampleKind::Missing, &row.missing_at);
        }
        match state {
            RowState::BadValues => add(ExampleKind::BadValue, &row.bad_at),
            RowState::Undeclared => add(ExampleKind::Undeclared, &row.undeclared_at),
            RowState::Conflict => add(ExampleKind::ShapeValue, &row.shape_at),
            _ => {}
        }
        let mut declared_for = Vec::new();
        if let (RowState::Undeclared, Prop::Iri(p)) = (state, &row.prop) {
            declared_for = self
                .declarers
                .get(p)
                .into_iter()
                .flatten()
                .map(|&k| names.short(&self.classes[k as usize].iri))
                .collect();
            declared_for.sort_unstable();
            declared_for.dedup();
            declared_for.truncate(MAX_DECLARED_FOR);
        }
        let r = Row {
            property,
            name,
            link,
            target,
            range: range.map(|r| r.to_string()),
            range_name: range.map(|r| names.short(&r)),
            declared,
            used,
            of,
            state,
            bad_values: (bad > 0).then_some(bad),
            required: row.required().map(|min| Required { min, max: row.max }),
            missing,
            conflict,
            shape_bad_values: (shape_bad > 0).then_some(shape_bad),
            declared_for,
            examples,
        };
        (r, severity)
    }
}

/// The label hover shows: the well-known vocabulary's, else the first
/// declaring document's.
fn label(docs: &[Doc], c: &Class) -> Option<String> {
    VOCABS
        .lookup(&c.iri)
        .and_then(|(v, local)| v.term(local))
        .map(|t| t.label.clone())
        .filter(|l| !l.is_empty())
        .or_else(|| {
            let d = c.label_doc?;
            docs[d as usize].a.label(&c.iri).map(str::to_string)
        })
}

/// The prefixes of the workspace documents, the first by URI winning a
/// name two bind differently. A name bound to another namespace elsewhere
/// is kept too, so `ex:` of `…/plant#` still shortens the IRIs of the
/// documents that say so when another document binds `ex:` to `…/`.
struct Names {
    prefixes: PrefixMap,
    /// (name, namespace) bindings the first-wins map lost, in document order.
    others: Vec<(String, String)>,
}

impl Names {
    fn new(docs: &[Doc]) -> Names {
        let mut prefixes = PrefixMap::default();
        let mut others: Vec<(String, String)> = Vec::new();
        for doc in docs.iter().filter(|d| !d.external) {
            for (name, ns) in &doc.facts.prefixes {
                match prefixes.get(name) {
                    None => {
                        prefixes.insert(name.clone(), ns.clone());
                    }
                    Some(have)
                        if have != ns && !others.iter().any(|(n, o)| n == name && o == ns) =>
                    {
                        others.push((name.clone(), ns.clone()));
                    }
                    Some(_) => {}
                }
            }
        }
        Names { prefixes, others }
    }

    fn short(&self, iri: &str) -> String {
        let s = short(iri, &self.prefixes);
        if !s.starts_with('<') {
            return s;
        }
        // The longest namespace among the lost bindings that shortens it.
        self.others
            .iter()
            .filter(|(_, ns)| iri.starts_with(ns.as_str()))
            .max_by_key(|(_, ns)| ns.len())
            .and_then(|(name, ns)| {
                let one = PrefixMap::from_iter([(name.clone(), ns.clone())]);
                Some(short(iri, &one)).filter(|s| !s.starts_with('<'))
            })
            .unwrap_or(s)
    }
}
