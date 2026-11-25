//! The per-item counts each source contributes and the sorted walks that
//! diff them ([`Counts`], [`merge`], [`diff`]), the text-key postings
//! ([`Posting`], [`repost`]), and the namespace counters ([`ns_step`]): how
//! a change of a source becomes a change of the index.

use super::{
    DocId, Entry, Keyed, Namespace, OWN_MIN_DESCRIBED, OWN_MIN_SHARE_PERCENT,
    PROJECT_MIN_SHARE_PERCENT, namespace_of,
};
use crate::syntax::{Iri, IriMap};
use rustc_hash::FxHashMap;

/// Per item, a count above zero, sorted by the item's id ([`Ided`]).
pub(super) type Counts<T> = Vec<(T, u32)>;

/// IRIs grouped under IRIs: per key IRI, sorted by [`Iri::id`], its values
/// in text order. Two allocations, where a map of vectors makes one per key
/// to build and to drop.
#[derive(Default, Debug)]
pub(super) struct ByIri {
    /// Each key, with the end of its values in `values`.
    keys: Vec<(Iri, u32)>,
    values: Vec<Iri>,
}

impl ByIri {
    /// From (key, value) pairs in text order.
    pub(super) fn new(mut pairs: Vec<(Iri, Iri)>) -> ByIri {
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

    pub(super) fn get(&self, key: Iri) -> Option<&[Iri]> {
        let at = self
            .keys
            .binary_search_by_key(&key.id(), |k| k.0.id())
            .ok()?;
        let start = at.checked_sub(1).map_or(0, |p| self.keys[p].1);
        Some(&self.values[start as usize..self.keys[at].1 as usize])
    }

    /// Each key with how many values it has.
    pub(super) fn counts(&self) -> Counts<Iri> {
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
pub(super) fn summed<T: Ided>(xs: &[(T, u32)], ys: &[(T, u32)]) -> Counts<T> {
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
pub(super) trait Ided: Copy + PartialEq {
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
pub(super) fn counted<T: Ided>(mut v: Vec<(T, u32)>) -> Counts<T> {
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
pub(super) fn merge<'a, T, I: Ided>(
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
pub(super) fn diff<T: Ided>(xs: &[(T, u32)], ys: &[(T, u32)], mut f: impl FnMut(T, bool)) {
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
pub(super) fn differing(
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

/// The sources with an entry under a text key: one, nearly always, kept
/// without an allocation.
#[derive(Debug)]
pub(super) enum Posting {
    One(DocId),
    /// Two or more.
    Many(Vec<DocId>),
}

impl Posting {
    pub(super) fn ids(&self) -> &[DocId] {
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
pub(super) fn repost(
    postings: &mut FxHashMap<u64, Posting>,
    id: DocId,
    xs: &[Keyed],
    ys: &[Keyed],
) {
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

pub(super) fn entry(entries: &mut IriMap<Entry>, iri: Iri) -> &mut Entry {
    entries.entry(iri).or_insert_with(|| Entry {
        ns: Some(Iri::intern(namespace_of(&iri))),
        ..Default::default()
    })
}

pub(super) fn toggle(v: &mut Vec<DocId>, id: DocId, add: bool) {
    if add {
        v.push(id);
    } else if let Some(i) = v.iter().position(|&d| d == id) {
        v.swap_remove(i);
    }
}

pub(super) fn step(n: &mut u32, add: bool) -> bool {
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
    ///
    /// [`Knowledge::is_own_namespace`]: super::Knowledge::is_own_namespace
    pub(super) fn is_own(&self) -> bool {
        self.described >= OWN_MIN_DESCRIBED
            && self.described * 100 >= OWN_MIN_SHARE_PERCENT * self.referenced
    }

    pub(super) fn is_project(&self) -> bool {
        self.described >= OWN_MIN_DESCRIBED
            && self.described * 100 >= PROJECT_MIN_SHARE_PERCENT * self.referenced
    }
}

/// Step one of a namespace's counters, remembering whether it was own before
/// the change began.
pub(super) fn ns_step(
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
