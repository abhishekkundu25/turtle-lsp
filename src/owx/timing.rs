//! The reader's wall-clock tests: a large OWL/XML document is scanned, read
//! and mapped within its budget, and reading takes time linear in the text
//! however long a base or a prefix IRI is and however many prefixes there
//! are. Run them alone (`--test-threads=1`) in release mode. The large
//! document is the Functional `large` shape written as OWL/XML by the
//! test-only printer.
//!
//! `owx_real_files` reads the OWL/XML that ROBOT writes from the Functional
//! corpus (`test/corpus-ofn/robot-owx/`, opt-in with `OWX_REAL=1`) and
//! prints the same numbers for each file.

use super::source::OwlXml;
use super::{CODE, printer, reader};
use crate::rdfxml::xml::{Event, Prolog, Scanner};
use crate::syntax::Parsed;
use std::time::{Duration, Instant};

/// The time of each stage of one reading of `text`: the scanner alone,
/// then the reader (which scans as it reads) and the mapping, which are
/// what [`super::parse`] does.
struct Stages {
    scan: Duration,
    read: Duration,
    map: Duration,
    nodes: usize,
    parsed: Parsed,
}

impl Stages {
    fn of(text: &str) -> Stages {
        let start = Instant::now();
        let prolog = Prolog::read(text);
        let mut s = Scanner::new(text, &prolog);
        while !matches!(s.next(), Event::Eof) {}
        let (xml_errors, _) = s.finish();
        let scan = start.elapsed();
        assert!(xml_errors.is_empty(), "{:?}", xml_errors.first());
        let start = Instant::now();
        let (doc, terms) = reader::read(text, None);
        let read = start.elapsed();
        let nodes = doc.nodes.len();
        let start = Instant::now();
        let parsed = crate::ofn::rdf::map_with(&OwlXml::new(text, &terms), doc, None);
        let map = start.elapsed();
        Stages {
            scan,
            read,
            map,
            nodes,
            parsed,
        }
    }

    /// What the budget counts: the reader and the mapping.
    fn total(&self) -> Duration {
        self.read + self.map
    }

    fn show(&self, name: &str, bytes: usize) {
        eprintln!(
            "{name}: {bytes} bytes, {} nodes, {} triples, {} references; scanned alone in {:?}, read (scan included) in {:?}, mapped in {:?}, read and mapped in {:?}",
            self.nodes,
            self.parsed.triples.len(),
            self.parsed.refs.len(),
            self.scan,
            self.read,
            self.map,
            self.total()
        );
    }
}

/// A generated OWL/XML document of 32 MiB, the size of `hp-edit.owl`, in
/// the shape of the Functional `large`, is scanned, read and mapped with no
/// error within [`LARGE_BUDGET`].
#[test]
fn timing_a_large_owl_xml_document_reads_within_budget() {
    let text = large(32 << 20);
    let s = Stages::of(&text);
    s.show("generated", text.len());
    assert!(
        s.parsed.errors.is_empty(),
        "{:?}",
        &s.parsed.errors[..s.parsed.errors.len().min(3)]
    );
    // One statement per item and the header's: every item was mapped. The
    // printer writes each item on a line of its own, between the root's
    // start and end tags.
    let items = text.matches("\n<").count() - 2;
    assert_eq!(s.parsed.statements.len(), items + 1);
    assert!(
        s.total() < LARGE_BUDGET,
        "{:?} over {LARGE_BUDGET:?}",
        s.total()
    );
}

/// The budget of [`timing_a_large_owl_xml_document_reads_within_budget`],
/// the reader and the mapping together. Measured on 2026-11-23, release
/// build with the shipped profile (fat LTO), on an Apple M6 (12 cores),
/// each run alone (`--exact --test-threads=1`) at load average 3.3 (other
/// worktrees building before and after: provisional until a quiet run).
/// Five runs, best and spread, for 33,676,779 bytes (658,106 nodes,
/// 302,013 triples): the scanner alone 48.4 ms (48.4 to 49.3), the reader,
/// which scans as it reads, 82.4 ms (82.4 to 84.4), the mapping 26.6 ms
/// (26.6 to 26.9), the total 109.1 ms (109.1 to 111.1). The printer writes
/// every IRI in full, so the document holds 111 bytes per triple, against
/// 43 in the Functional `large`. The reader limits the total (three
/// quarters of the samples of a profile of 30 readings), and the XML
/// scanner limits the reader (57% of its samples: the name and markup
/// loop, namespace resolution, the attribute vectors it allocates and
/// frees); the IRI checks take about a tenth. `hp-edit.owl` as ROBOT writes
/// it in OWL/XML (52.6 MB, 438,110 triples): read in 133 ms and mapped in
/// 34 ms. The budget is ten times the total.
const LARGE_BUDGET: Duration = Duration::from_millis(1100);

/// The Functional `large` of `src/ofn/tests.rs` with `n` classes: labels,
/// definitions, cross-references, annotated assertions, subclass axioms
/// and, for one class in four, an equivalence with a class expression.
fn functional(n: u32) -> String {
    let mut out = String::from(
        "Prefix(:=<http://purl.obolibrary.org/obo/hp.owl#>)\nPrefix(dc:=<http://purl.org/dc/elements/1.1/>)\nPrefix(obo:=<http://purl.obolibrary.org/obo/>)\nPrefix(owl:=<http://www.w3.org/2002/07/owl#>)\nPrefix(rdf:=<http://www.w3.org/1999/02/22-rdf-syntax-ns#>)\nPrefix(xml:=<http://www.w3.org/XML/1998/namespace>)\nPrefix(xsd:=<http://www.w3.org/2001/XMLSchema#>)\nPrefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)\nPrefix(oboInOwl:=<http://www.geneontology.org/formats/oboInOwl#>)\n\n\nOntology(<http://purl.obolibrary.org/obo/hp.owl>\nImport(<http://purl.obolibrary.org/obo/hp/imports/pato_import.owl>)\nAnnotation(dc:title \"Human Phenotype Ontology\")\n\n",
    );
    for i in 0..n {
        let c = format!("obo:HP_{i:07}");
        let p = format!("obo:HP_{:07}", i / 2);
        out.push_str(&format!(
            "# Class: {c} (Abnormality number {i})\n\n\
             AnnotationAssertion(Annotation(oboInOwl:hasDbXref \"PMID:{i}\") Annotation(oboInOwl:hasDbXref \"HPO:probinson\") obo:IAO_0000115 {c} \"An abnormality of a structure, number {i}, with a \\\"quoted\\\" word.\")\n\
             AnnotationAssertion(oboInOwl:created_by {c} \"doe\"^^xsd:string)\n\
             AnnotationAssertion(oboInOwl:creation_date {c} \"2009-01-01T00:00:00Z\"^^xsd:dateTime)\n\
             AnnotationAssertion(oboInOwl:hasExactSynonym {c} \"Abnormality {i}\"@en)\n\
             AnnotationAssertion(rdfs:label {c} \"Abnormality number {i}\"@en)\n\
             SubClassOf({c} {p})\n"
        ));
        if i.is_multiple_of(4) {
            out.push_str(&format!(
                "EquivalentClasses({c} ObjectIntersectionOf(obo:PATO_0000001 ObjectSomeValuesFrom(obo:RO_0000052 {p}) ObjectSomeValuesFrom(obo:RO_0002573 obo:PATO_0000460)))\n"
            ));
        }
        out.push_str(&format!("Declaration(Class({c}))\n\n"));
    }
    out.push_str(")\n");
    out
}

/// [`functional`] with `n` classes, written as OWL/XML.
fn printed(n: u32) -> String {
    let text = functional(n);
    printer::print(&text, &crate::ofn::tree(&text))
}

/// An OWL/XML document of at least `bytes` bytes: [`functional`] with as
/// many classes as it takes, printed.
fn large(bytes: usize) -> String {
    let (empty, sample) = (printed(0).len(), printed(1000).len());
    let per_class = (sample - empty) / 1000;
    let mut n = ((bytes - empty) / per_class) as u32;
    loop {
        let text = printed(n);
        if text.len() >= bytes {
            return text;
        }
        n += n / 100 + 1;
    }
}

/// The start of a document with an ontology IRI, open inside the root's
/// start tag for more attributes, and its end.
const OPEN: &str = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\"";
const CLOSE: &str = "</Ontology>\n";

/// The fastest of three readings of `text`, and its errors, each
/// `owx-syntax` (none from the scanner, which reports at most 100).
fn fastest_read(text: &str) -> (Duration, usize) {
    (0..3)
        .map(|_| {
            let start = Instant::now();
            // The terms too are dropped after the clock stops.
            let (doc, _terms) = reader::read(text, None);
            let took = start.elapsed();
            assert!(
                doc.errors.iter().all(|e| e.code == CODE),
                "{:?}",
                doc.errors.iter().find(|e| e.code != CODE)
            );
            (took, doc.errors.len())
        })
        .min()
        .unwrap()
}

/// `make(1)` and `make(4)` read in linear time: the larger in under four
/// times the smaller plus 5 ms, with `want` errors each (by the number of
/// IRIs `make` gives). The reader is timed, as the Functional tests time
/// the grammar: the mapping that follows is the Functional one.
#[track_caller]
fn linear(what: &str, make: impl Fn(usize) -> (String, usize), want: impl Fn(usize) -> usize) {
    let ((small, n_small), (big, n_big)) = (make(1), make(4));
    let ((t_small, e_small), (t_big, e_big)) = (fastest_read(&small), fastest_read(&big));
    assert_eq!((e_small, e_big), (want(n_small), want(n_big)), "{what}");
    eprintln!("{what}: {t_small:?} at 1x, {t_big:?} at 4x");
    assert!(
        t_big < t_small * 4 + Duration::from_millis(5),
        "{what}: {t_big:?} at 4x, {t_small:?} at 1x"
    );
}

/// Relative IRIs resolve against a base of 40,000 bytes in time linear in
/// the text, valid or not: 2,000 distinct relative IRIs, then four times
/// both. The reader checks each reference; the mapping, which resolves
/// them, stops at its IRI budget.
#[test]
fn timing_relative_iris_under_a_long_base_read_in_linear_time() {
    for bad in ["", "\u{FFF0}"] {
        let make = |s: usize| {
            let base = format!("http://example.org/{}/", "a".repeat(40_000 * s));
            let items: String = (0..2_000 * s)
                .map(|k| format!("<Declaration><Class IRI=\"c{k}{bad}\"/></Declaration>\n"))
                .collect();
            (
                format!("{OPEN} xml:base=\"{base}\">\n{items}{CLOSE}"),
                2_000 * s,
            )
        };
        let want = |n| if bad.is_empty() { 0 } else { n };
        linear(&format!("base, {bad:?}"), make, want);
    }
}

/// Abbreviated IRIs under a prefix IRI of 40,000 bytes are checked in time
/// linear in the text, valid or not: 2,000 distinct abbreviated IRIs, then
/// four times both. The mapping, which expands them, stops at its IRI
/// budget.
#[test]
fn timing_abbreviated_iris_with_a_long_prefix_iri_read_in_linear_time() {
    for bad in ["", "\u{FFF0}"] {
        let make = |s: usize| {
            let ns = format!("http://example.org/{}/", "a".repeat(40_000 * s));
            let items: String = (0..2_000 * s)
                .map(|k| {
                    format!("<Declaration><Class abbreviatedIRI=\"ex:c{k}{bad}\"/></Declaration>\n")
                })
                .collect();
            (
                format!("{OPEN}>\n<Prefix name=\"ex\" IRI=\"{ns}\"/>\n{items}{CLOSE}"),
                2_000 * s,
            )
        };
        let want = |n| if bad.is_empty() { 0 } else { n };
        linear(&format!("prefix IRI, {bad:?}"), make, want);
    }
}

/// 2,000 `Prefix` elements and ten uses of each are read in time linear in
/// their number: then four times that. A lookup that scanned the prefixes
/// would take 16 times as long, not 4. The sizes keep the reader's maps in
/// the cache: at 20,000 prefixes the same linear work took 4.2 to 5 times as
/// long at 4x, with 1.01 times the instructions and up to 1.38 times the
/// cycles (memory stalls, measured), as the Functional grammar does too.
/// Real files declare tens of prefixes.
#[test]
fn timing_many_prefix_declarations_read_in_linear_time() {
    let make = |s: usize| {
        let n = 2_000 * s;
        let prefixes: String = (0..n)
            .map(|k| format!("<Prefix name=\"p{k}\" IRI=\"http://example.org/{k}/\"/>\n"))
            .collect();
        let items: String = (0..n)
            .flat_map(|k| (0..10).map(move |c| (k, c)))
            .map(|(k, c)| {
                format!("<Declaration><Class abbreviatedIRI=\"p{k}:c{c}\"/></Declaration>\n")
            })
            .collect();
        (format!("{OPEN}>\n{prefixes}{items}{CLOSE}"), n)
    };
    linear("prefixes", make, |_| 0);
}

/// The OWL/XML files under `test/corpus-ofn/robot-owx/`, which ROBOT
/// writes from the Functional corpus (`tools/corpus/fetch-ofn.sh`, then
/// ROBOT's `convert` to `owx`), read and mapped with no error: their size,
/// nodes, triples and references, and the time of each stage, five times.
/// Opt-in with `OWX_REAL=1`.
#[test]
fn owx_real_files() {
    if std::env::var("OWX_REAL").as_deref() != Ok("1") {
        eprintln!("skipped: set OWX_REAL=1 (after ROBOT converted test/corpus-ofn/ to OWL/XML)");
        return;
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus-ofn/robot-owx");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    assert!(!files.is_empty(), "{}: no file", dir.display());
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        for _ in 0..5 {
            let s = Stages::of(&text);
            assert!(
                s.parsed.errors.is_empty(),
                "{name}: {} errors: {:?}",
                s.parsed.errors.len(),
                &s.parsed.errors[..s.parsed.errors.len().min(5)]
            );
            s.show(&name, text.len());
        }
    }
}
