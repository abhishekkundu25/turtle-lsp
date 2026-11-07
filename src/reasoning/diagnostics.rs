//! What a run shows in the documents: TurbOWL's findings (contradictions,
//! ill-typed literals) as diagnostics, the note that a run was skipped, and
//! the error of a run that failed.
//!
//! A finding is shown in each workspace statement that describes a term at
//! fault, on the statement's subject: the member of two disjoint classes,
//! not the classes (they are in the proof). Which terms are at fault
//! follows TurbOWL's table of terms per code ([`at_fault`]). A finding
//! about terms no workspace
//! statement describes (a literal) on the triples whose object is one of
//! them (a literal only: an ill-typed literal is shown on itself, not on its
//! datatype). A finding only about imports is not shown. Its code is
//! `turbowl/<rule>`.
//!
//! Its message is one sentence, its IRIs written as the hover writes them
//! in the document it is shown in ([`Naming`]: the document's prefixes,
//! then the project's). When its proof is searched for, the base
//! triples the proof rests on follow, written as `turtle.rs` writes them
//! (an RDF list as one item), in proof order: as related information, each
//! at its place, when the client shows it; otherwise as lines of the
//! message, `file:line  triple`.

use super::turtle::{IriName, term};
use super::{Base, Origin, Proof};
use crate::analysis::Analysis;
use crate::features::diagnostics::SOURCE;
use crate::features::hover::file_label;
use crate::features::prefixes::Naming;
use crate::knowledge::Source;
use crate::syntax::PrefixMap;
use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, NumberOrString, Range,
};
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::term::{NamedOrBlankNode, Term};
use std::sync::Arc;

/// Diagnostics per document URI.
pub(crate) type ByUri = FxHashMap<String, Vec<Diagnostic>>;

/// The note on the open documents that a run was skipped for its size.
pub(crate) const SKIPPED_CODE: &str = "reasoning-skipped";
/// The code of a finding is this followed by its W3C rule.
const CODE_PREFIX: &str = "turbowl/";
/// Findings whose proofs are searched for; the rest show their sentence.
const MAX_EXPLAINED: usize = 100;
/// Findings shown per document; the rest are summarized in one diagnostic.
const MAX_PER_DOC: usize = 1000;

/// The diagnostics of `r`'s last run over `base`, and the explainer's
/// cache their proofs filled. With `related`, the proof's triples are
/// related information; without, lines of the message. `project` holds the
/// project's prefixes of the documents that parse with none.
pub(super) fn of(
    r: &turbowl::Reasoner,
    base: &Base,
    related: bool,
    project: &FxHashMap<String, Arc<PrefixMap>>,
) -> (ByUri, turbowl::ExplainCache) {
    let mut out = ByUri::default();
    let found = r.diagnostics();
    if found.is_empty() {
        return (out, turbowl::ExplainCache::default());
    }
    let mentions = Mentions::of(base, found);
    let explainer = r.explainer().ok();
    let mut explained = 0;
    let mut naming: FxHashMap<String, Naming> = FxHashMap::default();
    for d in found {
        let places = mentions.places(base, d);
        if places.is_empty() {
            continue;
        }
        let proof = explainer
            .as_ref()
            .filter(|_| explained < MAX_EXPLAINED)
            .and_then(|e| e.explain_diagnostic(d))
            .map(|p| Proof::new(p, base));
        explained += 1;
        let severity = match d.severity {
            turbowl::Severity::Error => DiagnosticSeverity::ERROR,
            turbowl::Severity::Warning => DiagnosticSeverity::WARNING,
        };
        // Written once per document, with its prefixes.
        let mut written: FxHashMap<&str, (String, Option<Vec<DiagnosticRelatedInformation>>)> =
            FxHashMap::default();
        for (a, range) in places {
            let (message, related_information) = written
                .entry(a.uri.as_str())
                .or_insert_with(|| {
                    let naming = naming.entry(a.uri.clone()).or_insert_with(|| {
                        let project = project.get(&a.uri).unwrap_or(&a.external);
                        Naming::at(a, u32::MAX, Arc::clone(project))
                    });
                    let iri = |i: &str| naming.name(i);
                    explained_message(d, proof.as_ref(), &iri, related)
                })
                .clone();
            out.entry(a.uri.clone()).or_default().push(Diagnostic {
                range,
                severity: Some(severity),
                code: Some(NumberOrString::String(format!("{CODE_PREFIX}{}", d.code))),
                source: Some(SOURCE.into()),
                message,
                related_information,
                ..Default::default()
            });
        }
    }
    for list in out.values_mut() {
        if list.len() > MAX_PER_DOC {
            let hidden = list.len() - MAX_PER_DOC;
            list.truncate(MAX_PER_DOC);
            list.push(Diagnostic {
                severity: Some(DiagnosticSeverity::INFORMATION),
                code: Some(NumberOrString::String(format!("{CODE_PREFIX}more"))),
                source: Some(SOURCE.into()),
                message: format!("{hidden} more reasoning findings in this file are not shown"),
                ..Default::default()
            });
        }
    }
    let cache = explainer.map(|e| e.into_cache()).unwrap_or_default();
    (out, cache)
}

/// Where the workspace writes the findings' terms: the base triples with
/// each as subject and as object, by (document, index), in order.
struct Mentions {
    subjects: FxHashMap<NamedOrBlankNode, Vec<(u32, u32)>>,
    objects: FxHashMap<Term, Vec<(u32, u32)>>,
}

/// The terms `d` is about, by the order TurbOWL's `Diagnostic` documents per
/// code: the individual, literal or pair at fault, not the classes,
/// properties or axioms that make it a fault (those are in the proof). A
/// code not listed: every term.
fn at_fault(d: &turbowl::Diagnostic) -> impl Iterator<Item = &Term> {
    let at: &[usize] = match d.code {
        "cax-dw" | "cax-adc" | "cls-nothing2" | "dt-not-type" | "prp-npa1" | "prp-npa2" => &[0],
        "prp-irp" | "cls-maxc1" | "cls-maxqc1" | "cls-maxqc2" => &[1],
        "cls-com" | "prp-pdw" | "prp-adp" => &[2],
        "prp-asyp" | "eq-diff2" | "eq-diff3" => &[1, 2],
        "eq-diff1" => &[0, 1],
        _ => &[],
    };
    let all = at.is_empty();
    d.terms
        .iter()
        .enumerate()
        .filter(move |(i, _)| all || at.contains(i))
        .map(|(_, t)| t)
}

fn as_subject(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

impl Mentions {
    /// One pass over the workspace's triples.
    fn of(base: &Base, found: &[turbowl::Diagnostic]) -> Mentions {
        let mut subjects = FxHashMap::default();
        let mut objects = FxHashMap::default();
        for t in found.iter().flat_map(|d| &d.terms) {
            if let Some(s) = as_subject(t) {
                subjects.insert(s, Vec::new());
            }
            objects.insert(t.clone(), Vec::new());
        }
        for (t, place) in base.in_workspace() {
            if let Some(v) = subjects.get_mut(&t.subject) {
                v.push(place);
            }
            if let Some(v) = objects.get_mut(&t.object) {
                v.push(place);
            }
        }
        for v in subjects.values_mut().chain(objects.values_mut()) {
            v.sort_unstable();
        }
        Mentions { subjects, objects }
    }

    /// The (document, range) where `d` is shown, each once, in order.
    fn places<'b>(
        &self,
        base: &'b Base,
        d: &turbowl::Diagnostic,
    ) -> Vec<(&'b Arc<Analysis>, Range)> {
        let enc = base.encoding();
        let mut out = Vec::new();
        let mut statements = FxHashSet::default();
        for place in at_fault(d)
            .filter_map(as_subject)
            .filter_map(|s| self.subjects.get(&s))
            .flatten()
        {
            let (a, t) = base.written(*place);
            if !statements.insert((place.0, t.statement)) {
                continue;
            }
            let range = match a.parsed.statements[t.statement as usize].subject {
                Some((_, span)) => a.range(enc, span),
                // A blank node subject: the triple.
                None => base.place(*place).range,
            };
            out.push((a, range));
        }
        if out.is_empty() {
            let literals = at_fault(d).any(|t| matches!(t, Term::Literal(_)));
            for place in at_fault(d)
                .filter(|t| !literals || matches!(t, Term::Literal(_)))
                .filter_map(|t| self.objects.get(t))
                .flatten()
            {
                let (a, t) = base.written(*place);
                out.push((a, a.range(enc, t.o_span)));
            }
        }
        let mut seen = FxHashSet::default();
        out.retain(|(a, r)| seen.insert((a.uri.as_str(), *r)));
        out
    }
}

/// The message of `d` and its related information: the sentence, and the
/// base triples of `proof` as related information (`related`) or as lines
/// of the message.
fn explained_message(
    d: &turbowl::Diagnostic,
    proof: Option<&Proof>,
    iri: IriName<'_>,
    related: bool,
) -> (String, Option<Vec<DiagnosticRelatedInformation>>) {
    let mut message = sentence(d, iri);
    let Some(proof) = proof else {
        return (message, None);
    };
    let leaves: Vec<(&Origin, String)> = proof
        .leaves()
        .filter_map(|l| {
            let text = match l.rule {
                Some(rule) => format!("{} ({rule})", l.text(iri)),
                None => l.text(iri),
            };
            Some((l.origin()?, text))
        })
        .collect();
    if leaves.is_empty() {
        return (message, None);
    }
    if related {
        let info = leaves
            .into_iter()
            .map(|(o, text)| DiagnosticRelatedInformation {
                location: Location {
                    uri: crate::features::navigation::uri(&o.uri),
                    range: o.range,
                },
                message: text,
            })
            .collect();
        return (message, Some(info));
    }
    for (o, text) in leaves {
        let file = o
            .imported_from
            .clone()
            .unwrap_or_else(|| file_label(&o.uri));
        message.push_str(&format!("\n{file}:{}  {text}", o.line));
    }
    (message, None)
}

/// The sentence of `d`, its terms written with `iri`.
fn sentence(d: &turbowl::Diagnostic, iri: IriName<'_>) -> String {
    let names: Vec<String> = d.terms.iter().map(|t| term(t, iri)).collect();
    match (d.code, names.as_slice()) {
        ("cax-dw" | "cax-adc", [x, c1, c2]) => {
            format!("{x} is both a {c1} and a {c2}, which are disjoint.")
        }
        ("cls-com", [c1, c2, x]) => format!("{x} is both a {c1} and its complement {c2}."),
        ("cls-nothing2", [x]) => format!("{x} is an owl:Nothing, which has no instances."),
        ("prp-pdw" | "prp-adp", [p1, p2, x, y]) => {
            format!("{x} is linked to {y} by both {p1} and {p2}, which are disjoint.")
        }
        ("prp-asyp", [p, x, y]) => {
            format!("{x} {p} {y} and {y} {p} {x}, but {p} is asymmetric.")
        }
        ("prp-irp", [p, x]) => format!("{x} {p} {x}, but {p} is irreflexive."),
        ("dt-not-type", [literal, datatype]) => {
            format!("{literal} is not a valid {datatype}.")
        }
        ("eq-diff1", [x, y]) | ("eq-diff2" | "eq-diff3", [_, x, y]) => {
            format!("{x} and {y} are the same, but declared different.")
        }
        ("prp-npa1" | "prp-npa2", [x, p, y]) => {
            format!("{x} {p} {y}, which a negative property assertion denies.")
        }
        ("cls-maxc1" | "cls-maxqc1" | "cls-maxqc2", [_, x, y]) => {
            format!("{x} has the value {y}, but a restriction it is an instance of allows none.")
        }
        _ => turbowl_message(d, iri),
    }
}

/// TurbOWL's own sentence for `d`, its IRIs written with `iri` and its
/// blank nodes as `[]`.
fn turbowl_message(d: &turbowl::Diagnostic, iri: IriName<'_>) -> String {
    let mut out = String::with_capacity(d.message.len());
    let mut rest = d.message.as_str();
    while let Some(open) = rest.find('<') {
        let Some(len) = rest[open..]
            .find('>')
            .filter(|&n| !rest[open + 1..open + n].contains(' '))
        else {
            break;
        };
        out.push_str(&rest[..open]);
        out.push_str(&iri(&rest[open + 1..open + len]));
        rest = &rest[open + len + 1..];
    }
    out.push_str(rest);
    for t in &d.terms {
        if let Term::BlankNode(_) = t {
            out = out.replace(&t.to_string(), "[]");
        }
    }
    out
}

/// The note on each open document that the run was skipped: the sources
/// have `total` triples, more than `max`.
pub(super) fn skipped(docs: &[(Source, Arc<Analysis>)], total: u64, max: u64) -> ByUri {
    on_open(docs, || Diagnostic {
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String(SKIPPED_CODE.into())),
        source: Some(SOURCE.into()),
        message: format!(
            "Reasoning skipped: the workspace has {total} triples, more than the limit of {max} (setting turtle.reasoning.maxTriples)"
        ),
        ..Default::default()
    })
}

/// The error on each open document that a run panicked.
pub(super) fn crashed(
    docs: &[(Source, Arc<Analysis>)],
    panic: &(dyn std::any::Any + Send),
) -> ByUri {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    on_open(docs, || Diagnostic {
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(format!("{CODE_PREFIX}internal"))),
        source: Some(SOURCE.into()),
        message: format!(
            "turtle-lsp internal error in reasoning: {detail}. Results are not shown; please report this at https://github.com/abhishekkundu25/turtle-lsp/issues"
        ),
        ..Default::default()
    })
}

/// `d` at the start of each open document.
fn on_open(docs: &[(Source, Arc<Analysis>)], d: impl Fn() -> Diagnostic) -> ByUri {
    docs.iter()
        .filter(|(s, _)| *s == Source::Open)
        .map(|(_, a)| (a.uri.clone(), vec![d()]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::base::tests::doc;
    use super::super::{Job, Shared, run_inline};
    use super::*;
    use crate::analysis::Encoding;
    use crate::server::worker::Spawn;
    use crate::settings::Reasoning;

    const URI: &str = "file:///w/a.ttl";

    /// The findings in `text`, shown with related information or not.
    fn findings(text: &str, related: bool) -> Vec<Diagnostic> {
        let shared = Shared::new(Spawn::Thread);
        let job = Job {
            generation: 1,
            docs: vec![doc(Source::Open, URI, text)],
            rules: Reasoning::Default,
            max_triples: 1000,
            enc: Encoding::Utf16,
            related,
            findings: true,
            project: Default::default(),
        };
        let mut out = run_inline(&shared, &job).diagnostics;
        let mut v = out.remove(URI).unwrap_or_default();
        v.sort_by_key(|d| (d.range.start.line, d.range.start.character));
        v
    }

    /// Each related information as `line: message`, lines from 1.
    fn related(d: &Diagnostic) -> Vec<String> {
        d.related_information
            .iter()
            .flatten()
            .map(|r| {
                assert_eq!(r.location.uri.as_str(), URI);
                format!("{}: {}", r.location.range.start.line + 1, r.message)
            })
            .collect()
    }

    const ALL_DISJOINT: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix obj: <http://example.org/obj/> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        [] a owl:AllDisjointClasses ;\n\
        \x20 owl:members ( ex:PhysicalObject ex:SoftwareObject ex:Location ex:Position ) .\n\
        ex:Transmitter rdfs:subClassOf ex:PhysicalObject .\n\
        obj:tt a ex:Transmitter , ex:SoftwareObject .\n";

    #[test]
    fn a_finding_is_shown_on_the_term_at_fault_not_on_its_classes() {
        let text = format!(
            "{ALL_DISJOINT}ex:PhysicalObject rdfs:label \"Physical object\" .\n\
             ex:Position rdfs:label \"Position\" .\n\
             obj:p a ex:PhysicalObject , ex:Position .\n\
             ex:manages a owl:IrreflexiveProperty .\n\
             obj:eve ex:manages obj:eve .\n"
        );
        let found = findings(&text, true);
        let lines: Vec<u32> = found.iter().map(|d| d.range.start.line + 1).collect();
        // obj:tt (8), obj:p (11), obj:eve (13); never the class lines (9, 10),
        // the list (5) or the property (12).
        assert_eq!(lines, [8, 11, 13], "{found:#?}");
    }

    #[test]
    fn a_disjointness_through_a_list_is_one_sentence_with_the_list_as_one_triple() {
        let found = findings(ALL_DISJOINT, true);
        let d = found
            .iter()
            .find(|d| d.range.start.line == 7)
            .unwrap_or_else(|| panic!("{found:#?}"));
        assert_eq!(
            d.code,
            Some(NumberOrString::String("turbowl/cax-adc".into()))
        );
        assert_eq!(
            d.message,
            "obj:tt is both a ex:PhysicalObject and a ex:SoftwareObject, which are disjoint."
        );
        assert_eq!(
            related(d),
            [
                "7: ex:Transmitter rdfs:subClassOf ex:PhysicalObject (cax-sco)",
                "8: obj:tt a ex:Transmitter (cax-sco)",
                "8: obj:tt a ex:SoftwareObject",
                "5: [] a owl:AllDisjointClasses ; owl:members ( ex:PhysicalObject ex:SoftwareObject ex:Location ex:Position )",
            ]
        );
        // The list's item spans its statement, from `a` to the list's end.
        let list = &d.related_information.as_ref().unwrap()[3].location.range;
        assert_eq!((list.start.line, list.start.character), (4, 3));
        assert_eq!((list.end.line, list.end.character), (5, 77));
    }

    #[test]
    fn without_related_information_the_triples_are_lines_of_the_message() {
        let found = findings(ALL_DISJOINT, false);
        let d = found.iter().find(|d| d.range.start.line == 7).unwrap();
        assert_eq!(d.related_information, None);
        assert_eq!(
            d.message,
            "obj:tt is both a ex:PhysicalObject and a ex:SoftwareObject, which are disjoint.\n\
             a.ttl:7  ex:Transmitter rdfs:subClassOf ex:PhysicalObject (cax-sco)\n\
             a.ttl:8  obj:tt a ex:Transmitter (cax-sco)\n\
             a.ttl:8  obj:tt a ex:SoftwareObject\n\
             a.ttl:5  [] a owl:AllDisjointClasses ; owl:members ( ex:PhysicalObject ex:SoftwareObject ex:Location ex:Position )"
        );
    }

    #[test]
    fn names_use_the_prefixes_of_the_document_and_iris_without_one_stay_whole() {
        let text = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            <http://example.org/Cat> owl:disjointWith <http://example.org/Dog> .\n\
            <http://example.org/tom> a <http://example.org/Cat> , <http://example.org/Dog> .\n";
        let found = findings(text, true);
        assert_eq!(
            found[0].message,
            "<http://example.org/tom> is both a <http://example.org/Cat> and a <http://example.org/Dog>, which are disjoint."
        );
    }

    #[test]
    fn an_ill_typed_literal_is_shown_on_itself() {
        let text = "@prefix ex: <http://example.org/> .\n\
            @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
            ex:box2 ex:hasCount \"twelve\"^^xsd:integer .\n";
        let found = findings(text, true);
        assert_eq!(found.len(), 1, "{found:#?}");
        let d = &found[0];
        assert_eq!(
            d.code,
            Some(NumberOrString::String("turbowl/dt-not-type".into()))
        );
        assert_eq!(
            d.message,
            "\"twelve\"^^xsd:integer is not a valid xsd:integer."
        );
        // The literal.
        assert_eq!((d.range.start.line, d.range.start.character), (2, 20));
        assert_eq!((d.range.end.line, d.range.end.character), (2, 41));
        assert_eq!(
            related(d),
            ["3: ex:box2 ex:hasCount \"twelve\"^^xsd:integer"]
        );
    }
}
