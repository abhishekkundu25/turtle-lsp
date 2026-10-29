//! The findings an open document keeps between edits, for the checks that
//! cost the most on a keystroke ([`Check::kept`]). After an edit only the
//! statements the parse splice took again are checked again
//! ([`Window`]); after a change of the knowledge index, only what its
//! change set names ([`Changes`]). What this cannot follow (a full parse,
//! more than one index change, JSON-LD) checks everything again.
//!
//! By what each kept check reads ([`Reads`]):
//! - statement-local, `invalid-literal` and `label-whitespace`: kept per
//!   triple, checked again for the triples of the window, moved with their
//!   statement after it;
//! - per subject, `multiple-labels`: checked again for every label of a
//!   subject that has a label in the window;
//! - the index, `unknown-term`, `deprecated-term` and `duplicate-label`:
//!   checked again for the window, for the references to an IRI of the
//!   change set, and for the labels under one of its text keys. An IRI
//!   that became or stopped being a term changes which labels count, under
//!   keys the change set leaves out (it lists the IRI): the label checks
//!   run in full then.
//!
//! [`Reads`]: super::diagnostics::Reads

use super::curation::{self, Label, LabelChecks};
use super::diagnostics::{self, Check, Found};
use crate::analysis::Analysis;
use crate::knowledge::{Changes, Knowledge, Text};
use crate::syntax::splice::Window;
use crate::syntax::{Iri, Syntax};
use lsp_types::DiagnosticSeverity;
use rustc_hash::FxHashSet;
use std::sync::{Arc, Weak};

/// The findings of the kept checks of one document, and what they are of.
#[derive(Default)]
pub(crate) struct Kept {
    of: Of,
    /// `unknown-term` and the vocabulary's `deprecated-term` hints, then the
    /// workspace's `deprecated-term` warnings, each in the order of the
    /// references, which is text order.
    vocab: Vec<Found>,
    own: Vec<Found>,
    /// By the index of the triple each is on, in triple order.
    literals: Vec<(u32, Found)>,
    whitespace: Vec<(u32, Found)>,
    duplicate: Vec<(u32, Found)>,
    multiple: Vec<(u32, Found)>,
    /// The labels of declared terms, in triple order.
    labels: Vec<Label>,
    /// What the last update checked.
    #[cfg(test)]
    pub(crate) checked: Checked,
}

/// What the kept findings are of.
#[derive(Default)]
enum Of {
    /// Nothing yet, or nothing the next analysis builds on: the next update
    /// checks everything.
    #[default]
    Nothing,
    /// An analysis, at index generation `generation`, which has `triples`
    /// triples; then the windows `pending` the splice took again since,
    /// which made `head`.
    Analysis {
        head: Weak<Analysis>,
        pending: Option<Window>,
        generation: u64,
        triples: usize,
    },
}

/// How much one update checked: a statement-local check ran on
/// `statements` statements, the term checks on `refs` references, and
/// `duplicate-label` on `labels` labels.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Checked {
    pub(crate) statements: usize,
    pub(crate) refs: usize,
    pub(crate) labels: usize,
}

impl Kept {
    /// The findings of every kept check of `a` with `k`.
    pub(crate) fn of(a: &Analysis, k: &Knowledge) -> Kept {
        let n = a.parsed.triples.len();
        let (vocab, own) = match a.parsed.syntax {
            Syntax::JsonLd => diagnostics::jsonld_term_uses(a, k),
            _ => diagnostics::term_uses(a, k, a.parsed.refs.iter()),
        };
        let mut kept = Kept {
            vocab,
            own,
            literals: literals(a, 0..n),
            whitespace: curation::whitespace(a, 0..n),
            ..Kept::default()
        };
        kept.check_labels(a, k);
        kept.count(a.parsed.statements.len(), a.parsed.refs.len(), 0);
        kept
    }

    /// The findings of `check`, in the order the composer gives them; none
    /// for a check that is not kept.
    pub(crate) fn findings(&self, check: Check) -> Box<dyn Iterator<Item = &Found> + '_> {
        fn by_triple(v: &[(u32, Found)]) -> impl Iterator<Item = &Found> {
            v.iter().map(|(_, f)| f)
        }
        match check {
            Check::TermUses => Box::new(self.vocab.iter().chain(&self.own)),
            Check::Literals => Box::new(by_triple(&self.literals)),
            Check::LabelWhitespace => Box::new(by_triple(&self.whitespace)),
            Check::DuplicateLabel => Box::new(by_triple(&self.duplicate)),
            Check::MultipleLabels => Box::new(by_triple(&self.multiple)),
            _ => Box::new(std::iter::empty()),
        }
    }

    /// The document's analysis is now `now`, which the splice made from
    /// `last` by parsing `window` again.
    pub(crate) fn spliced(&mut self, last: &Analysis, window: Window, now: &Arc<Analysis>) {
        match &mut self.of {
            Of::Analysis { head, pending, .. } if std::ptr::eq(head.as_ptr(), last) => {
                *pending = Some(pending.map_or(window, |p| p.then(window)));
                *head = Arc::downgrade(now);
            }
            _ => self.drop_all(),
        }
    }

    /// The document's analysis is now one the splice did not make: nothing
    /// kept builds on it.
    pub(crate) fn drop_all(&mut self) {
        *self = Kept::default();
    }

    /// Bring the findings up to date with `a`, the document's analysis, and
    /// `k`.
    pub(crate) fn update(&mut self, a: &Arc<Analysis>, k: &Knowledge) {
        let now = k.generation();
        #[cfg(test)]
        {
            self.checked = Checked::default();
        }
        match std::mem::take(&mut self.of) {
            Of::Analysis {
                head,
                pending,
                generation,
                triples,
            } if std::ptr::eq(head.as_ptr(), Arc::as_ptr(a))
                && a.parsed.syntax != Syntax::JsonLd
                && now.checked_sub(generation).is_some_and(|d| d <= 1) =>
            {
                if let Some(w) = pending {
                    self.window(a, k, w, triples);
                }
                if now != generation {
                    self.changed(a, k, k.changes());
                }
            }
            _ => *self = Kept::of(a, k),
        }
        self.of = Of::Analysis {
            head: Arc::downgrade(a),
            pending: None,
            generation: now,
            triples: a.parsed.triples.len(),
        };
    }

    /// Check again the window `w` the splice took again, in `a`, whose
    /// analysis before the window had `old_triples` triples; move what
    /// comes after it.
    fn window(&mut self, a: &Analysis, k: &Knowledge, w: Window, old_triples: usize) {
        let p = &a.parsed;
        let s = w.statements;
        let (s0, s1) = (s.start, s.new_end());
        // The triples before the window are as they were, and so many after it.
        let t0 = p.triples.partition_point(|t| t.statement < s0);
        let t1 = p.triples.partition_point(|t| t.statement < s1);
        let gone = t0..old_triples - (p.triples.len() - t1);
        let moved = |x: &mut u32| *x = (*x as usize + t1 - gone.end) as u32;
        let shift = |v: &mut Vec<(u32, Found)>, new: Vec<(u32, Found)>| {
            let lo = v.partition_point(|(t, _)| (*t as usize) < gone.start);
            let hi = v.partition_point(|(t, _)| (*t as usize) < gone.end);
            for (t, f) in &mut v[hi..] {
                moved(t);
                f.span = moved_span(&w, f.span);
            }
            v.splice(lo..hi, new);
        };
        shift(&mut self.literals, literals(a, t0..t1));
        shift(&mut self.whitespace, curation::whitespace(a, t0..t1));

        // The labels: those of the window again, `duplicate-label` on them,
        // and `multiple-labels` on every label of their subjects.
        let mut lc = LabelChecks::new(a, k);
        let new = lc.labels(t0..t1);
        let lo = self
            .labels
            .partition_point(|l| (l.triple as usize) < gone.start);
        let hi = self
            .labels
            .partition_point(|l| (l.triple as usize) < gone.end);
        let mut subjects: FxHashSet<Iri> = self.labels[lo..hi].iter().map(|l| l.subject).collect();
        subjects.extend(new.iter().map(|l| l.subject));
        for l in &mut self.labels[hi..] {
            moved(&mut l.triple);
        }
        let new_labels = new.len();
        let duplicate = new
            .iter()
            .filter_map(|l| Some((l.triple, lc.duplicate(l)?)))
            .collect();
        self.labels.splice(lo..hi, new);
        shift(&mut self.duplicate, duplicate);
        shift(&mut self.multiple, Vec::new());
        let of_subjects: Vec<Label> = self
            .labels
            .iter()
            .filter(|l| subjects.contains(&l.subject))
            .copied()
            .collect();
        let found = lc.multiple(&of_subjects);
        replace(
            &mut self.multiple,
            &of_subjects,
            of_subjects.iter().zip(found),
        );

        // The references, by where they are.
        let b = w.bytes;
        let r0 = p.refs.partition_point(|r| r.span.start < b.start);
        let r1 = p.refs.partition_point(|r| r.span.start < b.new_end());
        let (vocab, own) = diagnostics::term_uses(a, k, p.refs[r0..r1].iter());
        for (v, new) in [(&mut self.vocab, vocab), (&mut self.own, own)] {
            let lo = v.partition_point(|f| f.span.start < b.start);
            let hi = v.partition_point(|f| f.span.start < b.end);
            for f in &mut v[hi..] {
                f.span = moved_span(&w, f.span);
            }
            v.splice(lo..hi, new);
        }
        self.count(s.new_len as usize, r1 - r0, new_labels);
    }

    /// Check again what the index's change set `c` names: the references
    /// to its IRIs, and the labels under its text keys (all the labels when
    /// it lists an IRI).
    fn changed(&mut self, a: &Analysis, k: &Knowledge, c: &Changes) {
        let mut refs: Vec<u32> = c
            .iris
            .iter()
            .filter_map(|i| a.refs_by_iri.get(i))
            .flatten()
            .copied()
            .collect();
        refs.sort_unstable();
        refs.dedup();
        if !refs.is_empty() {
            let spans: FxHashSet<_> = refs
                .iter()
                .map(|&i| a.parsed.refs[i as usize].span)
                .collect();
            let (vocab, own) =
                diagnostics::term_uses(a, k, refs.iter().map(|&i| &a.parsed.refs[i as usize]));
            for (v, new) in [(&mut self.vocab, vocab), (&mut self.own, own)] {
                v.retain(|f| !spans.contains(&f.span));
                v.extend(new);
                v.sort_by_key(|f| f.span.start);
            }
        }
        if !c.iris.is_empty() {
            self.check_labels(a, k);
        } else {
            let keys: FxHashSet<u64> = c
                .texts
                .iter()
                .filter(|(t, _)| *t == Text::Label)
                .map(|&(_, key)| key)
                .collect();
            let under: Vec<Label> = self
                .labels
                .iter()
                .filter(|l| keys.contains(&l.key))
                .copied()
                .collect();
            if !under.is_empty() {
                let mut lc = LabelChecks::new(a, k);
                let found: Vec<_> = under.iter().map(|l| lc.duplicate(l)).collect();
                replace(&mut self.duplicate, &under, under.iter().zip(found));
            }
            self.count(0, 0, under.len());
        }
        self.count(0, refs.len(), 0);
    }

    /// The label checks of every label of `a`.
    fn check_labels(&mut self, a: &Analysis, k: &Knowledge) {
        let mut lc = LabelChecks::new(a, k);
        self.labels = lc.labels(0..a.parsed.triples.len());
        self.duplicate = (self.labels.iter())
            .filter_map(|l| Some((l.triple, lc.duplicate(l)?)))
            .collect();
        let found = lc.multiple(&self.labels);
        self.multiple = (self.labels.iter().zip(found))
            .filter_map(|(l, f)| Some((l.triple, f?)))
            .collect();
        self.count(0, 0, self.labels.len());
    }

    /// Count what an update checked (for tests).
    fn count(&mut self, statements: usize, refs: usize, labels: usize) {
        #[cfg(test)]
        {
            self.checked.statements += statements;
            self.checked.refs += refs;
            self.checked.labels += labels;
        }
        let _ = (statements, refs, labels);
    }
}

/// `invalid-literal` on the triples `range` of `a`.
fn literals(a: &Analysis, range: std::ops::Range<usize>) -> Vec<(u32, Found)> {
    let code = crate::features::literals::CODE;
    (crate::features::literals::invalid_literals(a, range).into_iter())
        .map(|(t, span, message)| {
            (
                t,
                Found::new(span, DiagnosticSeverity::WARNING, code, message),
            )
        })
        .collect()
}

/// `span`, of the text before window `w` and not in it, in the text after.
fn moved_span(w: &Window, span: crate::syntax::Span) -> crate::syntax::Span {
    crate::syntax::Span::new(w.moved(span.start), w.moved(span.end))
}

/// Replace the findings in `v` on the triples of `labels` with `found`, a
/// finding or none for each.
fn replace<'l>(
    v: &mut Vec<(u32, Found)>,
    labels: &[Label],
    found: impl Iterator<Item = (&'l Label, Option<Found>)>,
) {
    let triples: FxHashSet<u32> = labels.iter().map(|l| l.triple).collect();
    v.retain(|(t, _)| !triples.contains(t));
    v.extend(found.filter_map(|(l, f)| Some((l.triple, f?))));
    v.sort_by_key(|(t, _)| *t);
}
