//! The OWL Functional frontend against independent readings of the same
//! ontologies, as RDF graphs compared with blank nodes renamed
//! (`tests/support/oracle.rs`):
//! - the approved W3C OWL 2 test cases that have both a Functional and an
//!   RDF/XML text (`tests/w3c/owl2/`): the mapping of one against the
//!   RDF/XML frontend's parse of the other;
//! - with `OFN_ROBOT=1`, the OBO files of `tools/corpus/fetch-ofn.sh`
//!   against what ROBOT (OWL API) writes for them in Turtle
//!   (`tools/corpus/robot-ofn.sh`), every difference explained.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use turtle_lsp::analysis::Analysis;
use turtle_lsp::syntax::Syntax;

#[path = "support/oracle.rs"]
mod oracle;

/// The analysis of `text` named `uri`, whose name and text give its syntax.
fn analysis(uri: &str, text: &str) -> Analysis {
    Analysis::new(uri.to_string(), text, Arc::default(), false)
}

/// Where the RDF/XML text of a W3C test is not the same ontology as its
/// Functional text: the test's name, the edit to the RDF/XML text that
/// makes it so, and why, from Table 1. Each text is checked to differ
/// before the edit and to agree after it.
const W3C_KNOWN: [(&str, &str, &str, &str); 6] = [
    (
        "New-Feature-Keys-007.nonconclusion",
        "<owl:Class rdf:about=\"Man\" />",
        "",
        "the RDF/XML declares :Man a class; the Functional text has no Declaration, and an entity alone gives no triple",
    ),
    (
        "New-Feature-Keys-007.premise",
        "</rdf:RDF>",
        "<rdf:Description rdf:about=\"Man\"><rdfs:subClassOf xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\" rdf:resource=\"Person\"/></rdf:Description>\n</rdf:RDF>",
        "the Functional text has SubClassOf( :Man :Person ), which the RDF/XML lacks",
    ),
    (
        "New-Feature-ObjectPropertyChain-001.conclusion",
        "<owl:Ontology/>",
        "<owl:Ontology/>\n<owl:ObjectProperty rdf:about=\"hasAunt\" />",
        "the Functional text declares :hasAunt, which the RDF/XML does not",
    ),
    (
        "New-Feature-ObjectQCR-001.premise",
        "<rdf:Description rdf:about=\"Stewie\">\n  <owl:differentFrom rdf:resource=\"Chris\" />",
        "<rdf:Description rdf:about=\"Chris\">\n  <owl:differentFrom rdf:resource=\"Stewie\" />",
        "DifferentIndividuals( :Chris :Stewie ) is T(a1) owl:differentFrom T(a2); the RDF/XML has the pair the other way",
    ),
    (
        "New-Feature-Rational-002.premise",
        "rdf:resource=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>",
        "rdf:resource=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#nil\"/>",
        "T(SEQ …) ends in rdf:nil; the RDF/XML list ends in the rdf: namespace IRI",
    ),
    (
        "New-Feature-Rational-003.premise",
        "rdf:resource=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>",
        "rdf:resource=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#nil\"/>",
        "T(SEQ …) ends in rdf:nil; the RDF/XML list ends in the rdf: namespace IRI",
    ),
];

/// The approved W3C OWL 2 test ontologies (`tests/w3c/owl2/`) that have
/// both a Functional text and an RDF/XML text of the same ontology: the
/// mapping of the first is isomorphic to the RDF/XML parse of the second,
/// but for the texts of [`W3C_KNOWN`], which differ, and agree once
/// corrected.
#[test]
fn w3c_functional_and_rdfxml_texts_give_one_graph() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/owl2");
    let mut names: Vec<_> = std::fs::read_dir(root.join("rdfxml"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    names.sort();
    let rdfxml = |text: &str, stem: &str| {
        let a = analysis(&format!("file:///w3c/{stem}.owl"), text);
        assert_eq!(a.parsed.syntax, Syntax::RdfXml, "{stem}");
        assert!(a.parsed.errors.is_empty(), "{stem}: {:?}", a.parsed.errors);
        oracle::ours(&a.parsed.triples)
    };
    let (mut same, mut mismatches) = (0, Vec::new());
    for path in &names {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let fs = std::fs::read_to_string(root.join("fs").join(format!("{stem}.ofn"))).unwrap();
        let xml = std::fs::read_to_string(path).unwrap();
        let a = analysis(&format!("file:///w3c/{stem}.ofn"), &fs);
        assert_eq!(a.parsed.syntax, Syntax::OwlFunctional, "{stem}");
        assert!(a.parsed.errors.is_empty(), "{stem}: {:?}", a.parsed.errors);
        let ours = oracle::ours(&a.parsed.triples);
        let diff = oracle::difference(&ours, "ours", &rdfxml(&xml, &stem), "RDF/XML", 20);
        match W3C_KNOWN.iter().find(|k| k.0 == stem) {
            Some(&(_, from, to, why)) => {
                assert!(
                    diff.is_some(),
                    "{stem} agrees now: take it out of W3C_KNOWN"
                );
                assert_eq!(xml.matches(from).count(), 1, "{stem}: {from}");
                let fixed = xml.replace(from, to);
                let fixed = rdfxml(&fixed, &stem);
                if let Some(d) = oracle::difference(&ours, "ours", &fixed, "corrected RDF/XML", 20)
                {
                    mismatches.push(format!("{stem} ({why}): {d}"));
                }
            }
            None => match diff {
                None => same += 1,
                Some(d) => mismatches.push(format!("{stem}: {d}")),
            },
        }
    }
    eprintln!(
        "W3C OWL 2 Functional and RDF/XML texts: {same}/{} isomorphic, {} known differences in the RDF/XML",
        names.len(),
        W3C_KNOWN.len()
    );
    assert_eq!(names.len(), 57);
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
    assert_eq!(same + W3C_KNOWN.len(), names.len());
}

/// A term as N-Triples text, language tags lowercased; a blank node as
/// the signature `sigs` gives it.
fn shown(t: &oxrdf::Term, sigs: &HashMap<oxrdf::BlankNode, u64>) -> String {
    match t {
        oxrdf::Term::Literal(l) => match l.language() {
            Some(lang) => oxrdf::Literal::new_language_tagged_literal_unchecked(
                l.value(),
                lang.to_ascii_lowercase(),
            )
            .to_string(),
            None => l.to_string(),
        },
        oxrdf::Term::BlankNode(b) => format!("_:{:016x}", sigs[b]),
        o => o.to_string(),
    }
}

/// A signature for each blank node of `triples`: a hash of what it says
/// (its outgoing triples, the blank nodes among their objects by their own
/// signatures), so two blank nodes that describe the same expression have
/// one signature. The graphs of the mapping are acyclic but for anonymous
/// individuals, which name each other: a cycle is cut at the node met
/// again.
fn signatures(triples: &[oxrdf::Triple]) -> HashMap<oxrdf::BlankNode, u64> {
    use std::hash::{Hash, Hasher};
    let mut out: HashMap<oxrdf::BlankNode, Vec<(String, oxrdf::Term)>> = HashMap::new();
    for t in triples {
        if let oxrdf::NamedOrBlankNode::BlankNode(b) = &t.subject {
            out.entry(b.clone())
                .or_default()
                .push((t.predicate.to_string(), t.object.clone()));
        }
        if let oxrdf::Term::BlankNode(b) = &t.object {
            out.entry(b.clone()).or_default();
        }
    }
    fn sig(
        b: &oxrdf::BlankNode,
        out: &HashMap<oxrdf::BlankNode, Vec<(String, oxrdf::Term)>>,
        done: &mut HashMap<oxrdf::BlankNode, u64>,
        open: &mut HashSet<oxrdf::BlankNode>,
    ) -> u64 {
        if let Some(&s) = done.get(b) {
            return s;
        }
        if !open.insert(b.clone()) {
            return 0;
        }
        let mut parts: Vec<String> = out[b]
            .iter()
            .map(|(p, o)| match o {
                oxrdf::Term::BlankNode(c) => format!("{p} _:{:016x}", sig(c, out, done, open)),
                oxrdf::Term::Literal(l) => match l.language() {
                    Some(lang) => format!("{p} \"{}\"@{}", l.value(), lang.to_ascii_lowercase()),
                    None => format!("{p} {l}"),
                },
                o => format!("{p} {o}"),
            })
            .collect();
        parts.sort();
        parts.dedup();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        parts.hash(&mut h);
        let s = h.finish();
        open.remove(b);
        done.insert(b.clone(), s);
        s
    }
    let (mut done, mut open) = (HashMap::new(), HashSet::new());
    let nodes: Vec<_> = out.keys().cloned().collect();
    for b in &nodes {
        sig(b, &out, &mut done, &mut open);
    }
    done
}

/// The classes of IRIs a declaration types them with.
const ENTITY_TYPES: [&str; 6] = [
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
    "http://www.w3.org/2002/07/owl#AnnotationProperty",
    "http://www.w3.org/2002/07/owl#NamedIndividual",
    "http://www.w3.org/2000/01/rdf-schema#Datatype",
];

const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
const RDF_LIST: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#List";
const MEMBERS: &str = "http://www.w3.org/2002/07/owl#members";
const DISTINCT_MEMBERS: &str = "http://www.w3.org/2002/07/owl#distinctMembers";

/// The OBO files `tools/corpus/fetch-ofn.sh` downloads, with
/// `OFN_ROBOT=1` (after `tools/corpus/robot-ofn.sh`): our mapping of each
/// against ROBOT's Turtle of it. The graphs are the same but for what OWL
/// API writes otherwise than the W3C mapping, each class counted:
/// - a declaration (`X rdf:type owl:Class`, …) of each entity the file
///   uses without declaring it (Table 1 gives an entity alone no triple);
/// - `owl:distinctMembers` for the members of `owl:AllDifferent`, OWL 1's
///   word (Table 1: `owl:members`);
/// - `rdf:type rdf:List` on the first node of a list that an `owl:Axiom`
///   names as its target (section 2.3.1's examples have none);
/// - one blank node for a class expression that two axioms with the same
///   subject write alike (two `SubClassOf`s that differ in their
///   annotations): each application of a rule makes a fresh blank node
///   (section 2.1), so the mapping has one per axiom.
///
/// The ground triples are compared as sets; the triples with blank nodes
/// as sets with each blank node by its signature ([`signatures`]), and the
/// blank nodes the mapping has beyond ROBOT's are counted per signature:
/// ROBOT never has more of one.
#[test]
fn ofn_robot_oracle() {
    if std::env::var("OFN_ROBOT").as_deref() != Ok("1") {
        eprintln!(
            "skipped: set OFN_ROBOT=1 (after tools/corpus/fetch-ofn.sh and tools/corpus/robot-ofn.sh)"
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
    for name in names {
        let dir = root.join("test/corpus-ofn");
        let text = std::fs::read_to_string(dir.join(name))
            .unwrap_or_else(|e| panic!("{name}: {e}; run tools/corpus/fetch-ofn.sh"));
        let ttl = std::fs::read(dir.join("robot").join(format!("{name}.ttl")))
            .unwrap_or_else(|e| panic!("{name}: {e}; run tools/corpus/robot-ofn.sh"));
        let start = std::time::Instant::now();
        let a = analysis(&format!("file:///corpus/{name}"), &text);
        let took = start.elapsed();
        assert_eq!(a.parsed.syntax, Syntax::OwlFunctional, "{name}");
        assert!(
            a.parsed.errors.is_empty(),
            "{name}: {:?}",
            a.parsed.errors.first()
        );
        let ours: Vec<oxrdf::Triple> = a
            .parsed
            .triples
            .iter()
            .map(|t| {
                let p = oxrdf::NamedNode::new_unchecked(t.p.as_str());
                oxrdf::Triple::new(oracle::subject(&t.s), p, oracle::term(&t.o))
            })
            .collect();
        let robot: Vec<oxrdf::Triple> = oxttl::TurtleParser::new()
            .for_slice(&ttl)
            .map(|t| t.unwrap_or_else(|e| panic!("{name}: {e}")))
            .collect();
        // OWL API's words, made the mapping's, and counted.
        let (mut distinct, mut listed) = (0, 0);
        let robot: Vec<oxrdf::Triple> = robot
            .into_iter()
            .filter_map(|mut t| {
                if t.predicate.as_str() == RDF_TYPE
                    && matches!(&t.object, oxrdf::Term::NamedNode(n) if n.as_str() == RDF_LIST)
                {
                    listed += 1;
                    return None;
                }
                if t.predicate.as_str() == DISTINCT_MEMBERS {
                    distinct += 1;
                    t.predicate = oxrdf::NamedNode::new_unchecked(MEMBERS);
                }
                Some(t)
            })
            .collect();
        let (s_ours, s_robot) = (signatures(&ours), signatures(&robot));
        let set = |v: &[oxrdf::Triple], sigs| -> BTreeSet<String> {
            v.iter()
                .map(|t| {
                    let s: oxrdf::Term = t.subject.clone().into();
                    format!(
                        "{} {} {}",
                        shown(&s, sigs),
                        t.predicate,
                        shown(&t.object, sigs)
                    )
                })
                .collect()
        };
        let (g_ours, g_robot) = (set(&ours, &s_ours), set(&robot, &s_robot));
        // Declarations OWL API adds: of IRIs the file declares nowhere.
        let declared: HashSet<String> = ours
            .iter()
            .filter(|t| t.predicate.as_str() == RDF_TYPE)
            .map(|t| t.subject.to_string())
            .collect();
        let mut added = 0;
        let mut unexplained: Vec<String> = Vec::new();
        for t in g_robot.difference(&g_ours) {
            let mut parts = t.splitn(3, ' ');
            let (s, p, o) = (
                parts.next().unwrap(),
                parts.next().unwrap(),
                parts.next().unwrap(),
            );
            let declaration = p == format!("<{RDF_TYPE}>")
                && ENTITY_TYPES.iter().any(|c| o == format!("<{c}>"))
                && s.starts_with('<')
                && !declared.contains(s);
            if declaration {
                added += 1;
            } else {
                unexplained.push(format!("only ROBOT: {t}"));
            }
        }
        unexplained.extend(
            g_ours
                .difference(&g_robot)
                .map(|t| format!("only ours: {t}")),
        );
        // Blank nodes per signature: the mapping has at least ROBOT's.
        let count = |sigs: &HashMap<oxrdf::BlankNode, u64>| {
            let mut c: HashMap<u64, usize> = HashMap::new();
            for s in sigs.values() {
                *c.entry(*s).or_default() += 1;
            }
            c
        };
        let (c_ours, c_robot) = (count(&s_ours), count(&s_robot));
        let mut shared = 0;
        for (s, &n) in &c_robot {
            match c_ours.get(s).copied().unwrap_or(0) {
                m if m >= n => shared += m - n,
                m => unexplained.push(format!(
                    "ROBOT has {n} blank nodes of signature {s:016x}, ours {m}"
                )),
            }
        }
        eprintln!(
            "{name}: {} triples in {took:?}, ROBOT {}; as OWL API writes otherwise: {added} declarations added, {distinct} owl:distinctMembers, {listed} lists typed rdf:List, {shared} blank nodes shared",
            ours.len(),
            robot.len() + listed,
        );
        assert!(
            unexplained.is_empty(),
            "{name}: {} unexplained differences:\n{}",
            unexplained.len(),
            unexplained[..unexplained.len().min(20)].join("\n")
        );
        // With no blank node shared, the parts with blank nodes are
        // isomorphic, multiplicity and all.
        if shared == 0 {
            let part = |v: &[oxrdf::Triple]| {
                let blank =
                    |t: &&oxrdf::Triple| t.subject.is_blank_node() || t.object.is_blank_node();
                oracle::canonical(
                    v.iter()
                        .filter(blank)
                        .map(|t| t.clone().in_graph(oxrdf::GraphName::DefaultGraph)),
                )
            };
            let d = oracle::difference(&part(&ours), "ours", &part(&robot), "ROBOT", 10);
            assert!(d.is_none(), "{name}: {}", d.unwrap());
        }
    }
}
