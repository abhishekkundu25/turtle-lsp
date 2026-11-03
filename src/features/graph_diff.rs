//! The `turtle/graphDiff` comparison: the workspace's data against a base
//! version of some of its files, which the client supplies (git HEAD in VS
//! Code, the load-time texts in the browser editor), as triples added and
//! removed per file and terms added, removed, changed and impacted.
//!
//! [`DiffQuery::new`] checks the client's base once (entries, sizes, URIs);
//! [`graph_diff`] trusts it. Nothing here keeps state: the base is parsed
//! for the request and dropped.
//!
//! What the words mean (the spec, `docs/specs/graph-diff.md`):
//!
//! - Triples are compared per file as sets of (subject, predicate, object)
//!   keys: IRIs by IRI, literals by value, datatype, language and
//!   direction, blank nodes by their path from the nearest IRI subject (see
//!   [`blank_keys`]). Triples with an undeclared prefix or a triple term are
//!   left out, as the graph leaves them out.
//! - A *changed row* is a removed and an added triple of one file with the
//!   same subject and predicate, when they are the only removal and the
//!   only addition for that pair.
//! - An *added* (*removed*) term is an IRI that is a subject or object on
//!   the current (base) side only, over the whole workspace.
//! - A *changed* term is on both sides and the subject of an added or
//!   removed triple; for a blank subject, its nearest IRI ancestor.
//! - An *impacted* term is none of those and the subject of a current
//!   statement with an added or changed term as an object, `rdf:type`
//!   objects included, found with the references index.
//!
//! Ids and names are the graph's ([`crate::features::graph`]), on the
//! current side; a literal of a removed row, and a blank node only the base
//! has, get the base side's, its documents numbered by URI as the graph
//! numbers the current ones.

// Until `Session::graph_diff` routes the request (the next slice).
#![cfg_attr(not(test), allow(dead_code))]

use crate::analysis::{Analysis, Encoding};
use crate::features::graph::{InvalidParams, Key, LITERAL_NAME_CHARS, Names};
use crate::knowledge::{Knowledge, Source};
use crate::syntax::{BlankId, Iri, Node, Role, Span, Triple, XSD};
use crate::workspace::{MAX_FILE_BYTES, is_rdf_file, normalize_uri, uri_to_path};
use lsp_types::Range;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The most base entries a request can list.
pub(crate) const MAX_ENTRIES: usize = 5_000;
/// The most rows, and impacted terms, a request can ask for.
pub(crate) const MAX_LIMIT: u32 = 20_000;
const DEFAULT_LIMIT: u32 = 2_000;

// ---- the request ----

/// The `turtle/graphDiff` parameters as sent; [`DiffQuery::new`] checks them.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct GraphDiffParams {
    /// Every file that differs from the base, once.
    pub(crate) base: Vec<BaseEntry>,
    /// Omitted: 2,000.
    #[serde(default)]
    pub(crate) limit: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct BaseEntry {
    pub(crate) uri: String,
    /// `null`: the file is not in the base.
    pub(crate) text: Option<String>,
}

/// What the base holds for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Base {
    /// Not in the base: created since.
    Absent,
    /// The file's text in the base.
    Text(String),
}

/// One file of a checked base: its URI in the workspace's spelling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BaseFile {
    pub(crate) uri: String,
    pub(crate) base: Base,
}

/// [`GraphDiffParams`] checked: workspace URIs, each once, sorted; texts
/// within the file size limit; the limit applied.
#[derive(Clone, Debug)]
pub(crate) struct DiffQuery {
    files: Vec<BaseFile>,
    limit: usize,
}

impl DiffQuery {
    /// `p` checked against the workspace: a URI is a workspace path when
    /// `k` holds it as a workspace document, or when it names an RDF file
    /// under one of `roots` (a file deleted since the base).
    pub(crate) fn new(
        p: GraphDiffParams,
        k: &Knowledge,
        roots: &[PathBuf],
    ) -> Result<DiffQuery, InvalidParams> {
        if p.base.len() > MAX_ENTRIES {
            return Err(InvalidParams(format!(
                "base lists {} files, more than {MAX_ENTRIES}",
                p.base.len()
            )));
        }
        let mut seen = FxHashSet::default();
        let mut files = Vec::with_capacity(p.base.len());
        for e in p.base {
            let uri = normalize_uri(&e.uri);
            if let Some(t) = &e.text
                && t.len() as u64 > MAX_FILE_BYTES
            {
                return Err(InvalidParams(format!(
                    "the base text of {uri} is over {MAX_FILE_BYTES} bytes"
                )));
            }
            let in_workspace = k.get(&uri).is_some()
                || uri_to_path(&uri)
                    .is_some_and(|p| is_rdf_file(&p) && roots.iter().any(|r| p.starts_with(r)));
            if !in_workspace {
                return Err(InvalidParams(format!(
                    "{uri} is not an RDF file of the workspace"
                )));
            }
            if !seen.insert(uri.clone()) {
                return Err(InvalidParams(format!("base lists {uri} twice")));
            }
            let base = match e.text {
                Some(t) => Base::Text(t),
                None => Base::Absent,
            };
            files.push(BaseFile { uri, base });
        }
        files.sort_by(|a, b| a.uri.cmp(&b.uri));
        Ok(DiffQuery {
            files,
            limit: p.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize,
        })
    }

    /// The listed files, by URI.
    pub(crate) fn files(&self) -> &[BaseFile] {
        &self.files
    }
}

// ---- the answer ----

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct GraphDiffResult {
    /// The files with a row, by URI.
    pub(crate) files: Vec<FileRows>,
    /// Added, removed, changed, then impacted; each by id.
    pub(crate) terms: Vec<TermChange>,
    pub(crate) total: DiffTotal,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct FileRows {
    pub(crate) uri: String,
    /// Added and changed rows in the current text's order, then removed
    /// rows in the base text's.
    pub(crate) rows: Vec<Row>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct Row {
    #[serde(flatten)]
    pub(crate) change: Change,
    pub(crate) subject: Term,
    pub(crate) predicate: Term,
    /// A changed row's new object.
    pub(crate) object: Term,
}

/// What happened to a row's triple, and where it is written.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "change", rename_all = "lowercase")]
pub(crate) enum Change {
    Added {
        range: Range,
    },
    Removed {
        #[serde(rename = "baseRange")]
        base_range: Range,
    },
    Changed {
        /// The old object.
        was: Term,
        range: Range,
        #[serde(rename = "baseRange")]
        base_range: Range,
    },
}

/// A term of a row, as the graph names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Term {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) literal: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct TermChange {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) status: Status,
    /// Where the term is described, else first mentioned: in the base for
    /// a removed term, in the workspace for the others.
    pub(crate) uri: String,
    pub(crate) range: Range,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Status {
    Added,
    Removed,
    Changed,
    Impacted,
}

/// The whole change, before the limit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiffTotal {
    /// A changed row counts one added and one removed triple.
    pub(crate) triples_added: usize,
    pub(crate) triples_removed: usize,
    pub(crate) added: usize,
    pub(crate) removed: usize,
    pub(crate) changed: usize,
    pub(crate) impacted: usize,
    /// Rows or impacted terms were left out at the limit.
    pub(crate) cut: bool,
}

/// `turtle/graphDiff`: the workspace documents of `k` against the base `q`
/// lists. `parse` makes a base text into the analysis of the file at that
/// URI, as the workspace parses the file (its syntax, external prefixes and
/// base IRI); it runs once per base text, on several threads. Ranges are in
/// `enc`.
pub(crate) fn graph_diff(
    k: &Knowledge,
    enc: Encoding,
    q: &DiffQuery,
    parse: impl Fn(&str, &str) -> Analysis + Sync,
) -> GraphDiffResult {
    let bases: Vec<Option<Analysis>> = q
        .files
        .par_iter()
        .map(|f| match &f.base {
            Base::Text(t) => Some(parse(&f.uri, t)),
            Base::Absent => None,
        })
        .collect();
    let listed: FxHashSet<&str> = q.files.iter().map(|f| f.uri.as_str()).collect();
    let mut cur_docs: Vec<&Analysis> = k
        .sources()
        .filter(|(_, s, _)| *s != Source::External)
        .map(|(_, _, a)| &**a)
        .collect();
    cur_docs.sort_by(|a, b| a.uri.cmp(&b.uri));
    let mut base_docs: Vec<&Analysis> = cur_docs
        .iter()
        .copied()
        .filter(|a| !listed.contains(a.uri.as_str()))
        .chain(bases.iter().flatten())
        .collect();
    base_docs.sort_by(|a, b| a.uri.cmp(&b.uri));
    let names = Names::new(&cur_docs);

    let mut total = DiffTotal::default();
    let mut budget = q.limit;
    let mut files = Vec::new();
    let mut cur_nodes: FxHashSet<Iri> = FxHashSet::default();
    let mut base_nodes: FxHashSet<Iri> = FxHashSet::default();
    let mut touched: FxHashSet<Iri> = FxHashSet::default();
    for (f, base) in q.files.iter().zip(&bases) {
        let mut paths = Paths::default();
        let cur = doc_index(&cur_docs, &f.uri).map(|d| Side::new(&cur_docs, d, &mut paths));
        let base = base.as_ref().map(|_| {
            let d = doc_index(&base_docs, &f.uri).expect("a base analysis is a base document");
            Side::new(&base_docs, d, &mut paths)
        });
        let added = only(cur.as_ref(), base.as_ref());
        let removed = only(base.as_ref(), cur.as_ref());
        total.triples_added += added.len();
        total.triples_removed += removed.len();
        for (side, nodes) in [(&cur, &mut cur_nodes), (&base, &mut base_nodes)] {
            for (t, _) in side.iter().flat_map(|s| &s.triples) {
                for n in [t.0, t.2] {
                    if let NKey::Iri(i) = n {
                        nodes.insert(i);
                    }
                }
            }
        }
        for (t, _) in added.iter().chain(&removed) {
            match t.0 {
                NKey::Iri(i) => {
                    touched.insert(i);
                }
                NKey::Blank(BKey::Path(id)) => {
                    touched.insert(paths.roots[id as usize]);
                }
                _ => {}
            }
        }

        // (subject, predicate) -> added, removed, the first removal's index.
        let mut pairs: FxHashMap<(NKey, Iri), (u32, u32, usize)> = FxHashMap::default();
        for (j, (t, _)) in removed.iter().enumerate() {
            pairs.entry((t.0, t.1)).or_insert((0, 0, j)).1 += 1;
        }
        for (t, _) in &added {
            pairs.entry((t.0, t.1)).or_insert((0, 0, usize::MAX)).0 += 1;
        }
        let mut rows = Vec::new();
        let mut paired = vec![false; removed.len()];
        for &(t, ti) in &added {
            let pair = pairs
                .get(&(t.0, t.1))
                .filter(|&&(a, r, _)| a == 1 && r == 1)
                .map(|&(_, _, j)| j);
            if let Some(j) = pair {
                paired[j] = true;
            }
            if budget == 0 {
                total.cut = true;
                continue;
            }
            budget -= 1;
            let c = cur.as_ref().expect("an added triple is current");
            let change = match pair {
                Some(j) => {
                    let b = base.as_ref().expect("a removed triple is in the base");
                    let (old, bi) = removed[j];
                    Change::Changed {
                        was: term(&names, old.2, b, bi, cur.as_ref()),
                        range: c.range(enc, ti),
                        base_range: b.range(enc, bi),
                    }
                }
                None => Change::Added {
                    range: c.range(enc, ti),
                },
            };
            rows.push(row(&names, change, t, c, ti, cur.as_ref()));
        }
        for (j, &(t, bi)) in removed.iter().enumerate() {
            if paired[j] {
                continue;
            }
            if budget == 0 {
                total.cut = true;
                continue;
            }
            budget -= 1;
            let b = base.as_ref().expect("a removed triple is in the base");
            let change = Change::Removed {
                base_range: b.range(enc, bi),
            };
            rows.push(row(&names, change, t, b, bi, cur.as_ref()));
        }
        if !rows.is_empty() {
            files.push(FileRows {
                uri: f.uri.clone(),
                rows,
            });
        }
    }

    // A subject or object in a document the base does not list is one on
    // both sides.
    let in_unchanged = |iri: Iri| {
        k.docs_referencing(&iri).any(|(_, s, a)| {
            s != Source::External
                && !listed.contains(a.uri.as_str())
                && a.refs_of(&iri)
                    .any(|r| matches!(r.role, Role::Subject | Role::Object))
        })
    };
    let one_side = |a: &FxHashSet<Iri>, b: &FxHashSet<Iri>| -> FxHashSet<Iri> {
        a.iter()
            .copied()
            .filter(|i| !b.contains(i) && !in_unchanged(*i))
            .collect()
    };
    let added = one_side(&cur_nodes, &base_nodes);
    let removed = one_side(&base_nodes, &cur_nodes);
    let changed: FxHashSet<Iri> = touched
        .into_iter()
        .filter(|i| !added.contains(i) && !removed.contains(i))
        .collect();
    let mut impacted: FxHashSet<Iri> = FxHashSet::default();
    for t in added.iter().chain(&changed) {
        for (_, s, a) in k.docs_referencing(t) {
            if s == Source::External {
                continue;
            }
            for r in a.refs_of(t).filter(|r| r.role == Role::Object) {
                let subject = a
                    .parsed
                    .statement_at(r.span)
                    .and_then(|i| a.parsed.statements[i].subject);
                if let Some((i, _)) = subject
                    && !added.contains(&i)
                    && !removed.contains(&i)
                    && !changed.contains(&i)
                {
                    impacted.insert(i);
                }
            }
        }
    }
    total.added = added.len();
    total.removed = removed.len();
    total.changed = changed.len();
    total.impacted = impacted.len();
    let mut impacted = sorted(impacted);
    if impacted.len() > q.limit {
        impacted.truncate(q.limit);
        total.cut = true;
    }

    let mut terms = Vec::new();
    for (status, iris) in [
        (Status::Added, sorted(added)),
        (Status::Removed, sorted(removed)),
        (Status::Changed, sorted(changed)),
        (Status::Impacted, impacted),
    ] {
        for i in iris {
            let place = match status {
                Status::Removed => base_place(&bases, i),
                _ => cur_place(k, i),
            };
            if let Some((a, span)) = place {
                terms.push(TermChange {
                    id: i.to_string(),
                    name: names.short(&i),
                    status,
                    uri: a.uri.clone(),
                    range: a.range(enc, span),
                });
            }
        }
    }
    GraphDiffResult {
        files,
        terms,
        total,
    }
}

/// The triples of `a` that `b` does not have.
fn only<'a>(a: Option<&Side<'a>>, b: Option<&Side<'a>>) -> Vec<(TKey<'a>, u32)> {
    a.iter()
        .flat_map(|a| &a.triples)
        .filter(|(t, _)| b.is_none_or(|b| !b.set.contains(t)))
        .copied()
        .collect()
}

fn sorted(set: FxHashSet<Iri>) -> Vec<Iri> {
    let mut v: Vec<Iri> = set.into_iter().collect();
    v.sort();
    v
}

/// The index of `uri` among `docs`, which are sorted by URI.
fn doc_index(docs: &[&Analysis], uri: &str) -> Option<u32> {
    docs.binary_search_by(|a| a.uri.as_str().cmp(uri))
        .ok()
        .map(|i| i as u32)
}

/// Where the workspace describes `iri`, else where it first mentions it,
/// as the graph places a node.
fn cur_place(k: &Knowledge, iri: Iri) -> Option<(&Analysis, Span)> {
    k.descriptions(&iri)
        .into_iter()
        .find(|d| d.source != Source::External)
        .and_then(|d| Some((d.analysis, *d.spans.first()?)))
        .or_else(|| {
            k.docs_referencing(&iri)
                .filter(|(_, s, _)| *s != Source::External)
                .filter_map(|(_, _, a)| Some((a, a.refs_of(&iri).next()?.span)))
                .min_by(|x, y| x.0.uri.cmp(&y.0.uri))
        })
}

/// Where the base describes `iri`, else where it first mentions it.
fn base_place(bases: &[Option<Analysis>], iri: Iri) -> Option<(&Analysis, Span)> {
    let bases = || bases.iter().flatten();
    bases()
        .find_map(|a| Some((a, *a.defs.get(&iri)?.first()?)))
        .or_else(|| bases().find_map(|a| Some((a, a.refs_of(&iri).next()?.span))))
}

// ---- keys ----

/// A blank node's key within its file: a path from its nearest IRI
/// ancestor ([`Paths`]), else its number in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BKey {
    Path(u32),
    Pos(BlankId),
}

/// A subject or object as the comparison sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum NKey<'a> {
    Iri(Iri),
    Blank(BKey),
    /// A [`Node::Literal`].
    Literal(&'a Node),
}

type TKey<'a> = (NKey<'a>, Iri, NKey<'a>);

/// The step a path starts from: an IRI, or another path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Up {
    Iri(Iri),
    Path(u32),
}

/// The blank-node paths of one file, shared by its two sides so that the
/// same path is the same number on both: (where from, predicate, index
/// among the blank objects of that subject and predicate) -> number.
#[derive(Default)]
struct Paths {
    ids: FxHashMap<(Up, Iri, u32), u32>,
    /// Each path's IRI ancestor, by number.
    roots: Vec<Iri>,
}

impl Paths {
    fn intern(&mut self, up: Up, p: Iri, i: u32) -> u32 {
        let next = self.roots.len() as u32;
        let id = *self.ids.entry((up, p, i)).or_insert(next);
        if id == next {
            let root = match up {
                Up::Iri(r) => r,
                Up::Path(id) => self.roots[id as usize],
            };
            self.roots.push(root);
        }
        id
    }
}

/// The key of every blank node of `a`'s triples. A blank node's parent is
/// the subject of the first triple, in text order, with it as the object;
/// its step is that triple's predicate and its index among that subject's
/// blank objects for that predicate. A blank node whose parents reach an
/// IRI is keyed by that path; one whose parents end in a blank node with
/// none, or go round, by its number.
fn blank_keys(a: &Analysis, paths: &mut Paths) -> FxHashMap<BlankId, BKey> {
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    enum From {
        Iri(Iri),
        Blank(BlankId),
    }
    let mut parent: FxHashMap<BlankId, (From, Iri, u32)> = FxHashMap::default();
    let mut count: FxHashMap<(From, Iri), u32> = FxHashMap::default();
    let mut all = Vec::new();
    for t in a.parsed.triples.iter().filter(|t| !t.unresolved) {
        if let Node::Blank(b) = t.s {
            all.push(b);
        }
        let Node::Blank(o) = t.o else { continue };
        all.push(o);
        let from = match t.s {
            Node::Iri(i) => From::Iri(i),
            Node::Blank(s) if s != o => From::Blank(s),
            _ => continue,
        };
        let n = count.entry((from, t.p)).or_default();
        let i = *n;
        *n += 1;
        parent.entry(o).or_insert((from, t.p, i));
    }
    let mut keys: FxHashMap<BlankId, BKey> = FxHashMap::default();
    let mut chain = Vec::new();
    let mut on_chain = FxHashSet::default();
    for b in all {
        // Up the parents to an IRI or a node already keyed.
        chain.clear();
        on_chain.clear();
        let mut at = b;
        let mut up = loop {
            match keys.get(&at) {
                Some(BKey::Path(id)) => break Some(Up::Path(*id)),
                Some(BKey::Pos(_)) => break None,
                None => {}
            }
            if !on_chain.insert(at) {
                break None;
            }
            chain.push(at);
            match parent.get(&at) {
                Some((From::Iri(i), ..)) => break Some(Up::Iri(*i)),
                Some((From::Blank(s), ..)) => at = *s,
                None => break None,
            }
        };
        for &n in chain.iter().rev() {
            let key = match up {
                Some(u) => {
                    let (_, p, i) = parent[&n];
                    let id = paths.intern(u, p, i);
                    up = Some(Up::Path(id));
                    BKey::Path(id)
                }
                None => BKey::Pos(n),
            };
            keys.insert(n, key);
        }
    }
    keys
}

/// One side of a file: its analysis and its triples as keys.
struct Side<'a> {
    a: &'a Analysis,
    /// The documents of the side, by URI, as the graph numbers them: the
    /// workspace's, or the base's (the documents the base does not list and
    /// the base texts).
    docs: &'a [&'a Analysis],
    /// The index of `a` in `docs`.
    d: u32,
    by_key: FxHashMap<BKey, BlankId>,
    /// Each key once, with its first triple's index, in text order.
    triples: Vec<(TKey<'a>, u32)>,
    set: FxHashSet<TKey<'a>>,
}

impl<'a> Side<'a> {
    fn new(docs: &'a [&'a Analysis], d: u32, paths: &mut Paths) -> Side<'a> {
        let a = docs[d as usize];
        let blanks = blank_keys(a, paths);
        let node = |n: &'a Node| match n {
            Node::Iri(i) => Some(NKey::Iri(*i)),
            Node::Blank(b) => Some(NKey::Blank(blanks[b])),
            Node::Literal { .. } => Some(NKey::Literal(n)),
            Node::Triple(_) => None,
        };
        let mut set = FxHashSet::default();
        let mut triples = Vec::new();
        for (ti, t) in a.parsed.triples.iter().enumerate() {
            if t.unresolved {
                continue;
            }
            let (Some(s), Some(o)) = (node(&t.s), node(&t.o)) else {
                continue;
            };
            if set.insert((s, t.p, o)) {
                triples.push(((s, t.p, o), ti as u32));
            }
        }
        Side {
            a,
            docs,
            d,
            by_key: blanks.iter().map(|(b, k)| (*k, *b)).collect(),
            triples,
            set,
        }
    }

    /// From the predicate to the end of the object of triple `ti`.
    fn range(&self, enc: Encoding, ti: u32) -> Range {
        let t: &Triple = &self.a.parsed.triples[ti as usize];
        let span = Span::new(
            t.p_span.start.min(t.o_span.start),
            t.p_span.end.max(t.o_span.end),
        );
        self.a.range(enc, span)
    }
}

fn row(names: &Names, change: Change, t: TKey, at: &Side, ti: u32, cur: Option<&Side>) -> Row {
    Row {
        change,
        subject: term(names, t.0, at, ti, cur),
        predicate: Term {
            id: t.1.to_string(),
            name: names.short(&t.1),
            literal: false,
        },
        object: term(names, t.2, at, ti, cur),
    }
}

/// The term `n` of triple `ti` of side `at`, as the graph names it: a blank
/// node by the current side's id when the current file has it, a literal
/// by its side's triple.
fn term(names: &Names, n: NKey, at: &Side, ti: u32, cur: Option<&Side>) -> Term {
    match n {
        NKey::Iri(i) => Term {
            id: i.to_string(),
            name: names.short(&i),
            literal: false,
        },
        NKey::Blank(k) => {
            let (s, b) = cur
                .and_then(|c| Some((c, *c.by_key.get(&k)?)))
                .unwrap_or_else(|| (at, at.by_key[&k]));
            Term {
                id: Key::Blank(s.d, b).id(s.docs),
                name: format!("_:b{}", b.0),
                literal: false,
            }
        }
        NKey::Literal(node) => Term {
            id: Key::Literal(at.d, ti).id(at.docs),
            name: literal_name(names, node),
            literal: true,
        },
    }
}

/// A literal as the graph names it.
fn literal_name(names: &Names, n: &Node) -> String {
    let Node::Literal {
        value,
        datatype,
        lang,
        ..
    } = n
    else {
        unreachable!("a literal key holds a literal")
    };
    let mut name: String = value.chars().take(LITERAL_NAME_CHARS).collect();
    if name.len() < value.len() {
        name.push('…');
    }
    let mut name = format!("\"{name}\"");
    if let Some(l) = lang {
        name.push_str(&format!("@{l}"));
    } else if datatype.as_str() != format!("{XSD}string") {
        name.push_str(&format!("^^{}", names.short(datatype)));
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::graph::{GraphParams, graph};
    use lsp_types::Position;
    use std::sync::Arc;

    const HEAD: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n";

    fn uri(name: &str) -> String {
        format!("file:///w/{name}.ttl")
    }

    fn knowledge(docs: &[(&str, &str)]) -> Knowledge {
        let mut k = Knowledge::default();
        for (name, body) in docs {
            let a = Analysis::new(uri(name), &format!("{HEAD}{body}"), Arc::default(), false);
            k.set(&uri(name), Source::Disk, Arc::new(a));
        }
        k
    }

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/w")]
    }

    fn query(k: &Knowledge, base: &[(&str, Option<&str>)], limit: Option<u32>) -> DiffQuery {
        let base = base
            .iter()
            .map(|(name, body)| BaseEntry {
                uri: uri(name),
                text: body.map(|b| format!("{HEAD}{b}")),
            })
            .collect();
        DiffQuery::new(GraphDiffParams { base, limit }, k, &roots()).expect("valid params")
    }

    fn diff(k: &Knowledge, base: &[(&str, Option<&str>)]) -> GraphDiffResult {
        run(k, &query(k, base, None))
    }

    fn run(k: &Knowledge, q: &DiffQuery) -> GraphDiffResult {
        graph_diff(k, Encoding::Utf16, q, |uri, text| {
            Analysis::new(uri.into(), text, Arc::default(), false)
        })
    }

    /// Each row as `+ s p o`, `- s p o` or `~ s p was → o`, by name.
    fn rows(r: &GraphDiffResult) -> Vec<String> {
        r.files
            .iter()
            .flat_map(|f| &f.rows)
            .map(|row| {
                let (s, p, o) = (&row.subject.name, &row.predicate.name, &row.object.name);
                match &row.change {
                    Change::Added { .. } => format!("+ {s} {p} {o}"),
                    Change::Removed { .. } => format!("- {s} {p} {o}"),
                    Change::Changed { was, .. } => format!("~ {s} {p} {} → {o}", was.name),
                }
            })
            .collect()
    }

    /// The terms with a status, by name.
    fn terms(r: &GraphDiffResult, status: Status) -> Vec<&str> {
        r.terms
            .iter()
            .filter(|t| t.status == status)
            .map(|t| t.name.as_str())
            .collect()
    }

    /// Catches a value change shown as two rows, and rows missed.
    #[test]
    fn added_removed_and_changed_rows() {
        let k = knowledge(&[("a", "ex:a ex:p ex:x . ex:a ex:r ex:d .")]);
        let r = diff(&k, &[("a", Some("ex:a ex:p ex:b . ex:a ex:q ex:c ."))]);
        assert_eq!(
            rows(&r),
            [
                "~ ex:a ex:p ex:b → ex:x",
                "+ ex:a ex:r ex:d",
                "- ex:a ex:q ex:c"
            ]
        );
        assert_eq!(r.files[0].uri, uri("a"));
        let t = r.total;
        assert_eq!((t.triples_added, t.triples_removed, t.cut), (2, 2, false));
        // A literal value change is one row too, with the literal's names.
        let k = knowledge(&[("a", "ex:a rdfs:label \"New\"@en .")]);
        let r = diff(&k, &[("a", Some("ex:a rdfs:label \"Old\"@en ."))]);
        assert_eq!(rows(&r), ["~ ex:a rdfs:label \"Old\"@en → \"New\"@en"]);
        let Change::Changed { was, .. } = &r.files[0].rows[0].change else {
            panic!("changed")
        };
        assert!(was.literal && r.files[0].rows[0].object.literal);
    }

    /// Catches pairing when the subject and predicate have more than one
    /// removal or addition.
    #[test]
    fn an_ambiguous_value_change_is_not_paired() {
        let k = knowledge(&[("a", "ex:a ex:p ex:x, ex:y .")]);
        let r = diff(&k, &[("a", Some("ex:a ex:p ex:b, ex:c ."))]);
        assert_eq!(
            rows(&r),
            [
                "+ ex:a ex:p ex:x",
                "+ ex:a ex:p ex:y",
                "- ex:a ex:p ex:b",
                "- ex:a ex:p ex:c"
            ]
        );
        let k = knowledge(&[("a", "ex:a ex:p ex:x, ex:y .")]);
        let r = diff(&k, &[("a", Some("ex:a ex:p ex:b ."))]);
        assert_eq!(
            rows(&r),
            ["+ ex:a ex:p ex:x", "+ ex:a ex:p ex:y", "- ex:a ex:p ex:b"]
        );
    }

    /// Catches a comparison of text instead of data.
    #[test]
    fn reformatting_reordering_and_prefix_renames_give_no_rows() {
        let k = knowledge(&[(
            "a",
            "ex:a\n    ex:q \"1\" ;\n    ex:p ex:b, ex:c .\n\nex:d ex:p ex:a .",
        )]);
        let base = "@prefix e: <http://example.org/> .\n\
            e:d e:p e:a . e:a e:p e:c ; e:p e:b ; e:q \"1\" .";
        let r = diff(&k, &[("a", Some(base))]);
        assert!(r.files.is_empty(), "{:?}", rows(&r));
        assert!(r.terms.is_empty());
        assert_eq!(r.total, DiffTotal::default());
    }

    /// Catches blank nodes compared by their number in the file.
    #[test]
    fn blank_node_insertion_and_moves_give_no_change() {
        let k = knowledge(&[(
            "a",
            "ex:z ex:p [ ex:q 9 ] .\nex:b ex:p ex:c .\nex:a ex:p [ ex:q 1 ; ex:r [ ex:s 2 ] ] .",
        )]);
        let base = "ex:a ex:p [ ex:q 1 ; ex:r [ ex:s 2 ] ] .\nex:b ex:p ex:c .";
        let r = diff(&k, &[("a", Some(base))]);
        assert_eq!(
            rows(&r),
            ["+ _:b1 ex:q \"9\"^^xsd:integer", "+ ex:z ex:p _:b1"]
        );
        // A changed value under a blank node is one row, and its IRI
        // ancestor is the changed term.
        let k = knowledge(&[("a", "ex:a ex:p [ ex:q 1 ; ex:r [ ex:s 3 ] ] .")]);
        let r = diff(&k, &[("a", Some(base))]);
        let row = rows(&r);
        assert_eq!(row.len(), 2, "{row:?}");
        assert!(
            row[0].starts_with("~ _:b")
                && row[0].ends_with("\"2\"^^xsd:integer → \"3\"^^xsd:integer")
        );
        assert_eq!(terms(&r, Status::Changed), ["ex:a"]);
        assert!(row[1].starts_with("- ex:b"), "{row:?}");
    }

    /// Catches a created or deleted file missed.
    #[test]
    fn created_and_deleted_files() {
        let k = knowledge(&[("new", "ex:n ex:p ex:m .")]);
        let r = diff(&k, &[("new", None), ("gone", Some("ex:g ex:p ex:h ."))]);
        assert_eq!(r.files.len(), 2);
        assert_eq!(r.files[0].uri, uri("gone"));
        assert_eq!(rows(&r), ["- ex:g ex:p ex:h", "+ ex:n ex:p ex:m"]);
        assert_eq!(terms(&r, Status::Added), ["ex:m", "ex:n"]);
        assert_eq!(terms(&r, Status::Removed), ["ex:g", "ex:h"]);
        let gone = r.terms.iter().find(|t| t.name == "ex:g").unwrap();
        assert_eq!(gone.uri, uri("gone"));
        assert_eq!((r.total.added, r.total.removed), (2, 2));
    }

    /// Catches added and removed terms decided per file.
    #[test]
    fn a_term_moved_between_files_is_neither_added_nor_removed() {
        let k = knowledge(&[("a", "ex:o ex:p ex:q ."), ("b", "ex:t a ex:C .")]);
        let r = diff(
            &k,
            &[
                ("a", Some("ex:t a ex:C . ex:o ex:p ex:q .")),
                ("b", Some("")),
            ],
        );
        assert_eq!(rows(&r), ["- ex:t rdf:type ex:C", "+ ex:t rdf:type ex:C"]);
        assert!(terms(&r, Status::Added).is_empty());
        assert!(terms(&r, Status::Removed).is_empty());
        assert_eq!(terms(&r, Status::Changed), ["ex:t"]);
        // A term another, unchanged file still uses is not removed.
        let k = knowledge(&[("a", ""), ("b", "ex:x ex:p ex:t .")]);
        let r = diff(&k, &[("a", Some("ex:t ex:p ex:u ."))]);
        assert_eq!(terms(&r, Status::Removed), ["ex:u"]);
        assert_eq!(terms(&r, Status::Changed), ["ex:t"]);
    }

    /// Catches impacted terms missed through `rdf:type` or an object, or
    /// found without a reference.
    #[test]
    fn impacted_through_type_and_object() {
        let k = knowledge(&[
            (
                "onto",
                "ex:C a owl:Class ; rdfs:label \"C\" .\nex:New a owl:Class .",
            ),
            (
                "data",
                "ex:i a ex:C .\nex:j ex:p [ ex:q ex:C ] .\nex:k ex:p ex:other .\nex:m ex:p ex:New .",
            ),
        ]);
        let r = diff(
            &k,
            &[("onto", Some("ex:C a owl:Class ; rdfs:label \"Old\" ."))],
        );
        assert_eq!(terms(&r, Status::Changed), ["ex:C", "ex:New"]);
        assert_eq!(terms(&r, Status::Impacted), ["ex:i", "ex:j", "ex:m"]);
        assert!(
            terms(&r, Status::Added).is_empty(),
            "ex:New was an object before"
        );
        // A term only the change adds impacts what references it.
        let k = knowledge(&[("onto", "ex:D a owl:Class ."), ("data", "ex:z ex:p ex:y .")]);
        let r = diff(&k, &[("onto", Some(""))]);
        assert_eq!(terms(&r, Status::Added), ["ex:D", "owl:Class"]);
        assert!(terms(&r, Status::Impacted).is_empty());
    }

    /// Catches a limit that is not applied, or totals counted after it.
    #[test]
    fn the_limit_cuts_rows_and_impacted_terms() {
        let body: String = (0..10)
            .map(|i| format!("ex:s{i} ex:p ex:o{i} . "))
            .collect();
        let users: String = (0..10)
            .map(|i| format!("ex:u{i} ex:p ex:s{i} . "))
            .collect();
        let k = knowledge(&[("a", &body), ("b", &users)]);
        let q = query(&k, &[("a", Some(""))], Some(3));
        let r = run(&k, &q);
        assert_eq!(rows(&r).len(), 3);
        assert_eq!(terms(&r, Status::Impacted).len(), 3);
        let t = r.total;
        assert_eq!((t.triples_added, t.impacted, t.cut), (10, 10, true));
        let r = run(&k, &query(&k, &[("a", Some(""))], None));
        assert_eq!((rows(&r).len(), r.total.cut), (10, false));
    }

    /// Catches ranges taken from the wrong side or the wrong text.
    #[test]
    fn ranges_on_both_sides() {
        let k = knowledge(&[("a", "\n\nex:a ex:p ex:x .")]);
        let r = diff(&k, &[("a", Some("ex:a ex:p ex:b . ex:z ex:p ex:y ."))]);
        let line = HEAD.lines().count() as u32;
        let range =
            |l: u32, from: u32, to: u32| Range::new(Position::new(l, from), Position::new(l, to));
        let [changed, removed] = &r.files[0].rows[..] else {
            panic!("{:?}", rows(&r))
        };
        assert_eq!(
            changed.change,
            Change::Changed {
                was: changed_was(changed),
                range: range(line + 2, 5, 14),
                base_range: range(line, 5, 14),
            }
        );
        assert_eq!(
            removed.change,
            Change::Removed {
                base_range: range(line, 22, 31)
            }
        );
        // A removed term's place is in the base text.
        let gone = r.terms.iter().find(|t| t.name == "ex:z").unwrap();
        assert_eq!(
            (gone.status, gone.range),
            (Status::Removed, range(line, 17, 21))
        );
    }

    fn changed_was(row: &Row) -> Term {
        match &row.change {
            Change::Changed { was, .. } => was.clone(),
            _ => panic!("changed"),
        }
    }

    /// Catches ids or names that do not match the graph's nodes.
    #[test]
    fn ids_and_names_match_the_graph() {
        let k = knowledge(&[
            ("a", "ex:o ex:p ex:q ."),
            (
                "b",
                "ex:s a ex:C ; rdfs:label \"S\"@en ; ex:p [ ex:q \"x\" ] .\nex:t ex:p ex:s .",
            ),
        ]);
        let r = diff(&k, &[("b", Some("ex:t ex:p ex:s ."))]);
        let p: GraphParams =
            serde_json::from_value(serde_json::json!({ "literals": true, "types": true })).unwrap();
        let g = graph(&k, Encoding::Utf16, p).unwrap();
        let nodes: FxHashMap<&str, &str> = g
            .nodes
            .iter()
            .map(|n| (n.id.as_str(), n.name.as_str()))
            .collect();
        let mut seen = 0;
        for row in r.files.iter().flat_map(|f| &f.rows) {
            for t in [&row.subject, &row.object] {
                assert_eq!(nodes.get(t.id.as_str()), Some(&t.name.as_str()), "{t:?}");
                seen += 1;
            }
        }
        for t in &r.terms {
            assert_eq!(nodes.get(t.id.as_str()), Some(&t.name.as_str()), "{t:?}");
        }
        assert_eq!(seen, 8);
        assert!(r.files[0].rows.iter().any(|r| r.object.literal));
        assert!(
            r.files[0]
                .rows
                .iter()
                .any(|r| r.subject.id.starts_with("_:1:"))
        );
    }

    /// Catches the answer's JSON drifting from the spec's field names.
    #[test]
    fn the_answer_serializes_as_the_spec_says() {
        let k = knowledge(&[("a", "ex:a ex:p ex:x .")]);
        let r = diff(&k, &[("a", Some("ex:a ex:p ex:b ."))]);
        let v = serde_json::to_value(&r).unwrap();
        let row = &v["files"][0]["rows"][0];
        assert_eq!(row["change"], "changed");
        assert_eq!(row["was"]["name"], "ex:b");
        assert_eq!(row["object"]["id"], "http://example.org/x");
        assert!(row["range"].is_object() && row["baseRange"].is_object());
        assert_eq!(v["terms"][0]["status"], "added");
        assert_eq!(v["terms"][0]["name"], "ex:x");
        assert_eq!(v["total"]["triplesAdded"], 1);
        assert_eq!(v["total"]["cut"], false);
        let k = knowledge(&[("a", "ex:a ex:p ex:x .")]);
        let v = serde_json::to_value(diff(&k, &[("a", None)])).unwrap();
        let row = &v["files"][0]["rows"][0];
        assert_eq!(row["change"], "added");
        assert!(row.get("was").is_none() && row.get("baseRange").is_none());
    }

    // ---- the boundary ----

    fn check(k: &Knowledge, v: serde_json::Value) -> Result<DiffQuery, InvalidParams> {
        let p: GraphDiffParams = serde_json::from_value(v).expect("decodes");
        DiffQuery::new(p, k, &roots())
    }

    /// Catches an unchecked base reaching the comparison.
    #[test]
    fn invalid_params_are_rejected() {
        let k = knowledge(&[("a", "")]);
        let many: Vec<serde_json::Value> = (0..=MAX_ENTRIES)
            .map(|i| serde_json::json!({ "uri": uri(&format!("f{i}")), "text": null }))
            .collect();
        check(&k, serde_json::json!({ "base": many })).expect_err("too many");
        let big = "#".repeat(MAX_FILE_BYTES as usize + 1);
        check(
            &k,
            serde_json::json!({ "base": [{ "uri": uri("a"), "text": big }] }),
        )
        .expect_err("too big");
        check(
            &k,
            serde_json::json!({ "base": [{ "uri": uri("a"), "text": "" }, { "uri": uri("a"), "text": null }] }),
        )
        .expect_err("duplicate");
        for outside in [
            "file:///elsewhere/a.ttl",
            "file:///w/q.rq",
            "file:///w/notes.txt",
            "untitled:Untitled-1",
            "https://example.org/a.ttl",
        ] {
            check(
                &k,
                serde_json::json!({ "base": [{ "uri": outside, "text": "" }] }),
            )
            .expect_err(outside);
        }
        // What serde rejects, the server reports as InvalidParams too.
        for v in [
            serde_json::json!({}),
            serde_json::json!({ "base": [{ "text": "" }] }),
            serde_json::json!({ "base": [], "limit": -1 }),
        ] {
            assert!(serde_json::from_value::<GraphDiffParams>(v).is_err());
        }
        // A deleted file under a root, an open document, the limits.
        let q = check(
            &k,
            serde_json::json!({ "base": [{ "uri": uri("gone"), "text": "" }, { "uri": uri("a"), "text": null }] }),
        )
        .expect("valid");
        assert_eq!(q.files()[0].uri, uri("a"), "sorted by URI");
        assert_eq!(q.files()[0].base, Base::Absent);
        assert_eq!(q.limit, DEFAULT_LIMIT as usize);
        let q = check(&k, serde_json::json!({ "base": [], "limit": u32::MAX })).unwrap();
        assert_eq!(q.limit, MAX_LIMIT as usize);
    }

    /// Catches a client's percent-encoded spelling (VS Code's on Windows)
    /// not matched to the workspace's document, or not seen as a duplicate.
    #[test]
    fn encoded_uris_are_normalized() {
        let key = normalize_uri("file:///c:/w/a.ttl");
        let mut k = Knowledge::default();
        let text = format!("{HEAD}ex:a ex:p ex:x .");
        k.set(
            &key,
            Source::Open,
            Arc::new(Analysis::new(key.clone(), &text, Arc::default(), false)),
        );
        let roots = [uri_to_path("file:///c:/w/").unwrap()];
        let entry = |u: &str| BaseEntry {
            uri: u.into(),
            text: Some(format!("{HEAD}ex:a ex:p ex:b .")),
        };
        let q = DiffQuery::new(
            GraphDiffParams {
                base: vec![entry("file:///c%3A/w/a.ttl")],
                limit: None,
            },
            &k,
            &roots,
        )
        .expect("valid");
        assert_eq!(q.files()[0].uri, key);
        assert_eq!(rows(&run(&k, &q)), ["~ ex:a ex:p ex:b → ex:x"]);
        let twice = GraphDiffParams {
            base: vec![entry("file:///c%3A/w/a.ttl"), entry(&key)],
            limit: None,
        };
        DiffQuery::new(twice, &k, &roots).expect_err("one file, two spellings");
    }
}
