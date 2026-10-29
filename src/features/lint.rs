//! Workspace lint: references to terms in the workspace's own namespaces that
//! nothing describes (code `undescribed-term`), usually typos.
//!
//! A ref to IRI *x* is flagged when *x*'s namespace is an own namespace (see
//! [`Knowledge::is_own_namespace`]) and no source describes *x*. Refs in
//! subject position are descriptions themselves and never flagged. With a
//! described term in the same namespace within a small edit distance the
//! diagnostic is a warning with a replace fix; otherwise information.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::{SOURCE, TermUse};
use crate::features::quickfix::FixData;
use crate::knowledge::{Knowledge, namespace_of};
use crate::syntax::{Form, Iri, IriKey, PrefixMap, Role, Span, Syntax};
use crate::vocab::{EditBuf, NameTable};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};

pub(crate) const CODE: &str = "undescribed-term";

/// Whether `candidate` could be what the author meant by `local`: same case
/// of the first letter (`ex:Signal` the class and `ex:signal` the property are
/// different terms by convention), and not just different digits
/// (`ex:pump-12` is not a typo of `ex:pump-13`).
fn plausible(local: &str, candidate: &str) -> bool {
    let first_upper = |s: &str| s.chars().next().is_some_and(char::is_uppercase);
    fn letters(s: &str) -> impl Iterator<Item = char> + '_ {
        s.chars().filter(|c| !c.is_ascii_digit())
    }
    first_upper(local) == first_upper(candidate) && !letters(local).eq(letters(candidate))
}

/// The most "Did you mean" searches one document gets: each looks at every
/// described term of a namespace of similar length, so a document full of
/// near-misses to a large namespace would otherwise stall every keystroke.
/// Flagged terms past the cap are reported without a suggestion.
const MAX_SUGGESTION_SEARCHES: usize = 100;

/// The described terms of one namespace, to find the one nearest to a typo.
struct Candidates {
    /// Every candidate, in the order of `table`.
    terms: Vec<Iri>,
    table: NameTable,
    ns_len: usize,
}

impl Candidates {
    /// The terms of `ns` that some source describes.
    fn described(k: &Knowledge, ns: &str) -> Self {
        Self::of(ns, k.terms_in(ns).into_iter().filter(|t| k.is_described(t)))
    }

    fn of(ns: &str, terms: impl Iterator<Item = Iri>) -> Self {
        let terms: Vec<Iri> = terms
            .filter(|t| t.get(ns.len()..).is_some_and(|l| !l.is_empty()))
            .collect();
        let table = NameTable::new(terms.iter().map(|t| &t.as_str()[ns.len()..]));
        Candidates {
            terms,
            table,
            ns_len: ns.len(),
        }
    }

    /// The closest candidate to `local`: edit distance at most 2, at most 1
    /// for local names of 3 to 5 characters, and none for shorter ones
    /// (`ex:p` is not a typo of `ex:s`); ties to the shorter, then smaller,
    /// name.
    fn nearest(&self, local: &str, buf: &mut EditBuf) -> Option<Iri> {
        let max = Self::max_distance(local)?;
        let mut best: Option<(usize, usize, Iri)> = None;
        self.table.search(local, max, buf, |i, d| {
            let t = self.terms[i];
            let l = &t.as_str()[self.ns_len..];
            if !plausible(local, l) {
                return;
            }
            let key = (d, l.len(), t.as_str());
            if best.is_none_or(|b| key < (b.0, b.1, b.2.as_str())) {
                best = Some((d, l.len(), t));
            }
        });
        best.map(|(_, _, t)| t)
    }

    fn max_distance(local: &str) -> Option<usize> {
        match local.chars().count() {
            0..=2 => None,
            3..=5 => Some(1),
            _ => Some(2),
        }
    }
}

/// The "Did you mean" search of one diagnostics run: a candidate table per
/// namespace, built when the first flagged term of that namespace needs it,
/// and the searches left.
struct Suggester {
    tables: FxHashMap<String, Candidates>,
    buf: EditBuf,
    searches: usize,
}

impl Suggester {
    fn new() -> Self {
        Suggester {
            tables: FxHashMap::default(),
            buf: EditBuf::default(),
            searches: MAX_SUGGESTION_SEARCHES,
        }
    }

    fn suggest(&mut self, k: &Knowledge, ns: &str, local: &str) -> Option<Iri> {
        Candidates::max_distance(local)?;
        self.searches = self.searches.checked_sub(1)?;
        if !self.tables.contains_key(ns) {
            self.tables
                .insert(ns.to_string(), Candidates::described(k, ns));
        }
        self.tables[ns].nearest(local, &mut self.buf)
    }
}

/// A local name that can be written after `prefix:` without escapes.
fn plain_local(l: &str) -> bool {
    !l.is_empty()
        && !l.ends_with('.')
        && !l.starts_with(['-', '.'])
        && l.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Predicates whose objects name whole ontologies (or versions of them),
/// which are resolved as imports, not described term by term.
const ONTOLOGY_LINKS: &[&str] = &[
    "imports",
    "versionIRI",
    "priorVersion",
    "backwardCompatibleWith",
    "incompatibleWith",
];

fn ontology_objects(a: &Analysis) -> FxHashSet<u32> {
    a.parsed
        .triples
        .iter()
        .filter(|t| {
            t.p.strip_prefix(OWL)
                .is_some_and(|l| ONTOLOGY_LINKS.contains(&l))
        })
        .map(|t| t.o_span.start)
        .collect()
}

const OWL: &str = "http://www.w3.org/2002/07/owl#";

pub(crate) fn undescribed_terms(a: &Analysis, k: &Knowledge, enc: Encoding) -> Vec<Diagnostic> {
    let mut ontologies: Option<FxHashSet<u32>> = None;
    let mut prefixes = None;
    let uses = a
        .parsed
        .refs
        .iter()
        .filter(|r| r.role != Role::Subject)
        .filter_map(|r| match r.form {
            Form::A => None,
            Form::PName { prefix_len } => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: a
                    .text
                    .get(r.span.start as usize..(r.span.start + prefix_len) as usize),
                bare: None,
            }),
            Form::Iri => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: None,
                bare: None,
            }),
        });
    let jsonld = a.parsed.syntax == Syntax::JsonLd;
    undescribed_written(
        uses,
        &a.text,
        k,
        |span| {
            ontologies
                .get_or_insert_with(|| ontology_objects(a))
                .contains(&span.start)
        },
        |p| {
            prefixes
                .get_or_insert_with(|| {
                    if jsonld {
                        jsonld_prefixes(a)
                    } else {
                        a.prefix_map()
                    }
                })
                .get(p)
                .cloned()
        },
        |span| a.range(enc, span),
        |written, used, s| {
            if jsonld {
                jsonld_iri(written, used, s)
            } else {
                format!("<{s}>")
            }
        },
    )
}

/// The prefixes a JSON-LD document uses, from its compact IRIs: each
/// `prefix:local` it writes expands to the prefix's IRI followed by `local`.
/// (Its prefixes live in `@context`, not in the parse's prefix list.)
pub(crate) fn jsonld_prefixes(a: &Analysis) -> PrefixMap {
    let mut out = PrefixMap::default();
    for r in &a.parsed.refs {
        let Form::PName { prefix_len } = r.form else {
            continue;
        };
        let Some(written) = a.text.get(r.span.start as usize..r.span.end as usize) else {
            continue;
        };
        let (Some(prefix), Some(local)) = (
            written.get(..prefix_len as usize),
            written.get(prefix_len as usize + 1..),
        ) else {
            continue;
        };
        if !out.contains_key(prefix)
            && let Some(ns) = r.iri.strip_suffix(local)
        {
            out.insert(prefix.to_string(), ns.to_string());
        }
    }
    out
}

/// How JSON-LD writes `suggestion` where `written` (expanding to `used`)
/// is: as a term when `written` is one that `@vocab` expanded (`colur` for
/// `.../colur`), else as the absolute IRI (a JSON string, no brackets).
fn jsonld_iri(written: &str, used: &str, suggestion: &str) -> String {
    if !written.contains(':')
        && let Some(vocab) = used.strip_suffix(written)
        && let Some(term) = suggestion.strip_prefix(vocab)
        && plain_local(term)
    {
        return term.to_string();
    }
    suggestion.to_string()
}

/// The `undescribed-term` diagnostics for `uses` (the document decides which
/// of its IRIs count: RDF documents leave out subjects, which describe).
/// `skip` leaves out a flagged use by its span, `namespace` resolves a
/// written prefix name, `range` maps a span. Shared by RDF and SPARQL
/// documents.
pub(crate) fn undescribed<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    text: &str,
    k: &Knowledge,
    skip: impl FnMut(Span) -> bool,
    namespace: impl FnMut(&str) -> Option<String>,
    range: impl Fn(Span) -> Range,
) -> Vec<Diagnostic>
where
    K: std::ops::Deref<Target = str> + IriKey + Copy + Eq + std::hash::Hash,
{
    undescribed_written(uses, text, k, skip, namespace, range, |_, _, s| {
        format!("<{s}>")
    })
}

/// [`undescribed`], where `full(written, used, suggestion)` writes a
/// suggestion that no prefix shortens, in place of `written` (which
/// expands to `used`).
#[allow(clippy::too_many_arguments)]
fn undescribed_written<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    text: &str,
    k: &Knowledge,
    mut skip: impl FnMut(Span) -> bool,
    mut namespace: impl FnMut(&str) -> Option<String>,
    range: impl Fn(Span) -> Range,
    full: impl Fn(&str, &str, &str) -> String,
) -> Vec<Diagnostic>
where
    K: std::ops::Deref<Target = str> + IriKey + Copy + Eq + std::hash::Hash,
{
    let mut out = Vec::new();
    // Per IRI: None = fine, Some(suggestion) = flagged.
    let mut verdicts: FxHashMap<K, Option<Option<Iri>>> = FxHashMap::default();
    let mut suggester = Suggester::new();
    for u in uses {
        let verdict = *verdicts.entry(u.iri).or_insert_with(|| {
            let ns = namespace_of(&u.iri);
            let flagged =
                ns.len() < u.iri.len() && !k.is_described(&u.iri) && k.is_own_namespace(ns);
            flagged.then(|| suggester.suggest(k, ns, &u.iri[ns.len()..]))
        });
        let Some(suggestion) = verdict else { continue };
        if skip(u.span) {
            continue;
        }
        let written = text
            .get(u.span.start as usize..u.span.end as usize)
            .unwrap_or_default();
        let mut d = Diagnostic {
            range: range(u.span),
            severity: Some(DiagnosticSeverity::INFORMATION),
            code: Some(NumberOrString::String(CODE.to_string())),
            source: Some(SOURCE.to_string()),
            message: format!("'{written}' is not described anywhere in the workspace"),
            ..Default::default()
        };
        if let Some(s) = suggestion {
            let pname = u.prefix.and_then(|prefix| {
                namespace(prefix)
                    .and_then(|ns| s.strip_prefix(ns.as_str()).map(str::to_string))
                    .filter(|l| plain_local(l))
                    .map(|l| format!("{prefix}:{l}"))
            });
            let replacement = pname.unwrap_or_else(|| full(written, &u.iri, &s));
            d.severity = Some(DiagnosticSeverity::WARNING);
            d.message
                .push_str(&format!(". Did you mean '{replacement}'?"));
            FixData::Replace {
                replace: replacement,
            }
            .attach(&mut d);
        }
        out.push(d);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vocab::{VOCABS, edit_distance};

    const NS: &str = "http://example.org/ns#";

    /// The search as it was written before the candidate table: every
    /// candidate, a distance each, no pruning.
    fn unpruned(locals: &[String], local: &str) -> Option<Iri> {
        let max = match local.chars().count() {
            0..=2 => return None,
            3..=5 => 1,
            _ => 2,
        };
        let lower = local.to_lowercase();
        locals
            .iter()
            .filter_map(|l| {
                if l.is_empty() || !plausible(local, l) {
                    return None;
                }
                let d = edit_distance(&lower, &l.to_lowercase());
                (d <= max).then_some((d, l.len(), Iri::intern(&format!("{NS}{l}"))))
            })
            .min_by(|x, y| (x.0, x.1, x.2.as_str()).cmp(&(y.0, y.1, y.2.as_str())))
            .map(|(_, _, t)| t)
    }

    #[test]
    fn suggestions_match_the_unpruned_search() {
        let mut locals: Vec<String> = ["foaf", "schema", "skos"]
            .iter()
            .flat_map(|p| VOCABS.by_prefix(p).unwrap().terms().iter().step_by(3))
            .map(|t| t.local.clone())
            .collect();
        locals.extend(
            [
                "pump-12",
                "pump-13",
                "Pump12",
                "Étiquette",
                "étiquette2",
                "fillsPosition",
            ]
            .map(str::to_string),
        );
        let table = Candidates::of(NS, locals.iter().map(|l| Iri::intern(&format!("{NS}{l}"))));
        let mut buf = EditBuf::default();
        let mut probes = vec![
            "pump-14".to_string(),
            "pump-12".to_string(),
            "ETIQUETTE".to_string(),
            "fillsPositon".to_string(),
            "ab".to_string(),
            "nmae".to_string(),
        ];
        for l in locals.iter().step_by(5) {
            probes.push(format!("{l}x"));
            probes.push(l.chars().skip(1).collect());
            probes.push(l.chars().rev().collect());
            probes.push(l.to_uppercase());
            probes.push(format!("{l}zz"));
        }
        let mut found = 0;
        for probe in &probes {
            let got = table.nearest(probe, &mut buf);
            assert_eq!(got, unpruned(&locals, probe), "{probe}");
            found += usize::from(got.is_some());
        }
        assert!(found > probes.len() / 4, "{found} of {}", probes.len());
    }

    #[test]
    fn plausible_ignores_digits_and_needs_the_same_case() {
        assert!(plausible("fillsPositon", "fillsPosition"));
        assert!(!plausible("pump-12", "pump-13"));
        assert!(!plausible("signal", "Signal"));
        assert!(plausible("pump12x", "pump13"));
    }

    #[test]
    fn past_the_cap_a_flagged_term_gets_no_search() {
        let mut s = Suggester::new();
        s.searches = 1;
        let ns = "http://example.org/cap#";
        let k = Knowledge::default();
        // Nothing is described, so there is no answer, but the search ran.
        assert_eq!(s.suggest(&k, ns, "colour"), None);
        assert_eq!(s.searches, 0);
        assert!(s.tables.contains_key(ns));
        // A name too short for a suggestion costs nothing.
        let mut s = Suggester::new();
        assert_eq!(s.suggest(&k, ns, "ab"), None);
        assert_eq!(s.searches, MAX_SUGGESTION_SEARCHES);
    }
}
