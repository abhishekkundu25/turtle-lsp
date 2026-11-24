//! The index's sources and their update: the generations, setting,
//! replacing and removing sources (workspace and external), looking them up
//! by URI or id, and the sorted walks that apply a source's old and new
//! contributions side by side and record what changed ([`Changes`]).

use super::{
    Changes, Collisions, Curated, DocFacts, DocId, Index, Key, Keyed, Knowledge, Marks, Namespace,
    Slot, Source, TermState, Text, diff, differing, entry, merge, ns_step, repost, step, toggle,
};
use crate::analysis::Analysis;
use crate::syntax::Iri;
use crate::vocab::VOCABS;
use rustc_hash::FxHashMap;
use std::sync::Arc;

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
    pub(super) fn curated(&self, iri: Iri) -> impl Iterator<Item = (&str, &Curated)> {
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

    pub(super) fn term_state(&self, iri: Iri) -> TermState {
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
}
