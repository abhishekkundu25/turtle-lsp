//! The OWL/XML frontend against ROBOT (OWL API), opt-in with
//! `OWX_ROBOT=1`: ROBOT converts the OBO Functional files of
//! `tools/corpus/fetch-ofn.sh` to OWL/XML (`tools/corpus/robot-owx.sh`),
//! and turtle-lsp's graph of each OWL/XML file is its graph of the
//! Functional source, compared with blank nodes renamed
//! (`tests/support/oracle.rs`), every difference explained.
//!
//! `tests/owx/forms.robot-1.9.11.owx` is ROBOT's OWL/XML of
//! `tests/owx/forms.ofn`, committed as the evidence of how OWL API writes
//! what the Recommendation does not settle (literals, SWRL rules); its
//! command and versions are in `tests/owx/forms.robot-1.9.11.txt`.

use std::collections::HashSet;
use std::sync::Arc;
use turtle_lsp::analysis::Analysis;
use turtle_lsp::syntax::{Node, Parsed, Syntax, Triple};

#[path = "support/oracle.rs"]
mod oracle;

/// The analysis of `text` named `uri`, whose name and text give its syntax.
fn analysis(uri: &str, text: &str) -> Analysis {
    Analysis::new(uri.to_string(), text, Arc::default(), false)
}

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const SWRL_VARIABLE: &str = "http://www.w3.org/2003/11/swrl#Variable";

/// The classes of IRIs a declaration types them with.
const ENTITY_TYPES: [&str; 6] = [
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
    "http://www.w3.org/2002/07/owl#AnnotationProperty",
    "http://www.w3.org/2002/07/owl#NamedIndividual",
    "http://www.w3.org/2000/01/rdf-schema#Datatype",
];

/// Whether the item that gave `t` starts with `word` in `text`: the
/// keyword of a Functional item, the start tag of an OWL/XML one.
fn from_item(p: &Parsed, text: &str, t: &Triple, word: &str) -> bool {
    let start = p.statements[t.statement as usize].span.start as usize;
    text[start..].starts_with(word)
}

/// An IRI node's value.
fn iri(n: &Node) -> Option<&str> {
    match n {
        Node::Iri(i) => Some(i.as_str()),
        _ => None,
    }
}

/// The OBO files `tools/corpus/fetch-ofn.sh` downloads, with
/// `OWX_ROBOT=1` (after `tools/corpus/robot-owx.sh`): the OWL/XML ROBOT
/// writes for each reads with no error, and its graph is the graph of the
/// Functional source, but for what OWL API changes on the way, counted:
/// - a declaration OWL API adds for each entity the source uses without
///   declaring it (Table 1 gives an entity alone no triple). The smallest
///   input: `Ontology(<http://example.org/o>
///   ClassAssertion(<http://example.org/A> _:x))` comes back with
///   `<Declaration><Class IRI="http://example.org/A"/></Declaration>`, and
///   `SubClassOf(ex:Undeclared ex:A)` in `tests/owx/forms.ofn` with the
///   `Declaration` of `ex:Undeclared` in its ROBOT output. Such a triple
///   `X rdf:type C` (C an entity type, X an IRI the Functional graph names)
///   comes from a `Declaration` element, and the Functional graph does not
///   have it; it is taken out of the OWL/XML graph.
/// - a SWRL variable whose IRI is in `urn:swrl#` is written in
///   `urn:swrl:var#`: `DLSafeRule(Body(ClassAtom(ex:A
///   Variable(<urn:swrl#x>))) Head(ClassAtom(ex:A
///   Variable(<http://example.org/v#y>))))` comes back with `<Variable
///   IRI="urn:swrl:var#x"/>` and `<Variable
///   IRI="http://example.org/v#y"/>` (ROBOT 1.9.11's Functional and Turtle
///   outputs keep `urn:swrl#x`). The OWL/XML is read as written; in the
///   Functional graph, each IRI typed `swrl:Variable` in `urn:swrl#` is
///   renamed so. `hp-pato_import.owl` has 19 such variables, and writes
///   `x`, `y`, `z` and `p` in both namespaces, so its OWL/XML graph has
///   four `swrl:Variable` triples fewer, one per variable merged.
///
/// The SWRL rules (`DLSafeRule`, in `ro-edit.owl` and
/// `hp-pato_import.owl`) are read and compared with the rest, counted.
/// What remains is compared whole: the two graphs are isomorphic.
///
/// OWL API also sorts the operands of an n-ary axiom: `DisjointClasses(ex:C
/// ex:B ex:A)` in `tests/owx/forms.ofn` is written A, B, C, and the
/// `owl:members` list follows. The corpus files are written by OWL API
/// already in that order, so no rule here allows it; a source written in
/// another order would fail on its lists.
#[test]
fn owx_robot_oracle() {
    if std::env::var("OWX_ROBOT").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set OWX_ROBOT=1 (after tools/corpus/fetch-ofn.sh and tools/corpus/robot-owx.sh)"
        );
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = std::fs::read_to_string(root.join("tools/corpus/ofn-sources.tsv")).unwrap();
    let names: Vec<&str> = sources
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert!(!names.is_empty());
    let dir = root.join("test/corpus-ofn");
    for &name in &names {
        let ofn = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}; run tools/corpus/fetch-ofn.sh"));
        let owx = std::fs::read_to_string(dir.join("robot-owx").join(format!("{name}.owx")))
            .unwrap_or_else(|e| panic!("robot-owx/{name}.owx: {e}; run tools/corpus/robot-owx.sh"));

        let start = std::time::Instant::now();
        let a = analysis(&format!("file:///corpus/{name}"), &ofn);
        let took_ofn = start.elapsed();
        assert_eq!(a.parsed.syntax, Syntax::OwlFunctional, "{name}");
        assert!(
            a.parsed.errors.is_empty(),
            "{name}: {:?}",
            a.parsed.errors.first()
        );
        let start = std::time::Instant::now();
        let b = analysis(&format!("file:///corpus/{name}.owx"), &owx);
        let took_owx = start.elapsed();
        assert_eq!(b.parsed.syntax, Syntax::OwlXml, "{name}.owx");

        assert!(
            b.parsed.errors.is_empty(),
            "{name}.owx: {:?}",
            &b.parsed.errors[..b.parsed.errors.len().min(5)]
        );
        let rules = |p: &Parsed, text: &str, word: &str| {
            p.statements
                .iter()
                .filter(|s| text[s.span.start as usize..].starts_with(word))
                .count()
        };
        let rules_ofn = rules(&a.parsed, &ofn, "DLSafeRule");
        assert_eq!(rules(&b.parsed, &owx, "<DLSafeRule"), rules_ofn, "{name}");

        // The declarations OWL API adds, out of the OWL/XML graph.
        let named: HashSet<&str> = a
            .parsed
            .triples
            .iter()
            .flat_map(|t| [iri(&t.s), Some(t.p.as_str()), iri(&t.o)])
            .flatten()
            .collect();
        let declared: HashSet<(&str, &str)> = a
            .parsed
            .triples
            .iter()
            .filter(|t| t.p.as_str() == RDF_TYPE)
            .filter_map(|t| Some((iri(&t.s)?, iri(&t.o)?)))
            .collect();
        let added = |t: &Triple| {
            let (Some(s), Some(o)) = (iri(&t.s), iri(&t.o)) else {
                return false;
            };
            t.p.as_str() == RDF_TYPE
                && ENTITY_TYPES.contains(&o)
                && named.contains(s)
                && !declared.contains(&(s, o))
                && from_item(&b.parsed, &owx, t, "<Declaration")
        };
        let (added, xml): (Vec<&Triple>, Vec<&Triple>) =
            b.parsed.triples.iter().partition(|t| added(t));

        // The variables OWL API moves to `urn:swrl:var#`, renamed in the
        // Functional graph.
        let moved: HashSet<&str> = a
            .parsed
            .triples
            .iter()
            .filter(|t| t.p.as_str() == RDF_TYPE && iri(&t.o) == Some(SWRL_VARIABLE))
            .filter_map(|t| iri(&t.s))
            .filter(|s| s.starts_with("urn:swrl#"))
            .collect();
        let moved_to = |n: &Node| {
            let i = iri(n).filter(|i| moved.contains(i))?;
            let to = format!("urn:swrl:var#{}", &i["urn:swrl#".len()..]);
            Some(oxrdf::NamedNode::new_unchecked(to))
        };
        let functional = oracle::canonical(a.parsed.triples.iter().map(|t| {
            let mut q = oracle::quad(t);
            if let Some(s) = moved_to(&t.s) {
                q.subject = s.into();
            }
            if let Some(o) = moved_to(&t.o) {
                q.object = o.into();
            }
            q
        }));

        let d = oracle::difference(
            &functional,
            "Functional",
            &oracle::ours(xml.iter().copied()),
            "OWL/XML",
            20,
        );
        eprintln!(
            "{name}: Functional {} triples in {took_ofn:?}, OWL/XML {} triples in {took_owx:?}; as OWL API writes otherwise: {} declarations added, {} SWRL variables moved to urn:swrl:var#; {rules_ofn} SWRL rules",
            a.parsed.triples.len(),
            b.parsed.triples.len(),
            added.len(),
            moved.len(),
        );
        assert!(d.is_none(), "{name}: {}", d.unwrap());
    }
    eprintln!("{} files", names.len());
}
