//! Cross-checks the parser against oxttl (Oxigraph) on every valid fixture:
//! both must produce the same ground triples (quads for TriG, with graph
//! names) and the same count. Turtle fixtures are compared with oxttl's
//! Turtle parser, TriG with its TriG parser, N-Triples with its N-Triples parser.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use turtle_lsp::syntax::{GraphName, Node, PrefixMap, Syntax, parse};

fn node(n: &Node) -> Option<String> {
    match n {
        Node::Iri(i) => Some(format!("<{i}>")),
        Node::Blank(_) => None,
        Node::Literal {
            value,
            datatype,
            lang,
        } => Some(match lang {
            Some(l) => format!("{value:?}@{l}"),
            None => format!("{value:?}^^<{datatype}>"),
        }),
        Node::Triple(_) => None,
    }
}

fn ox_term(t: &oxrdf::Term) -> Option<String> {
    use oxrdf::Term;
    match t {
        Term::NamedNode(n) => Some(format!("<{}>", n.as_str())),
        Term::BlankNode(_) => None,
        Term::Literal(l) => Some(match (l.language(), l.direction()) {
            // Ours keeps the base direction in the tag, as written: `@ar--rtl`.
            (Some(lang), Some(dir)) => format!("{:?}@{lang}--{dir}", l.value()),
            (Some(lang), None) => format!("{:?}@{}", l.value(), lang),
            _ => format!("{:?}^^<{}>", l.value(), l.datatype().as_str()),
        }),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

/// The graph a triple is in: blank graph names compare as "blank".
fn graph(g: Option<GraphName>) -> String {
    match g {
        None => "DEFAULT".into(),
        Some(GraphName::Iri(i)) => format!("<{i}>"),
        Some(GraphName::Blank(_)) => "blank".into(),
    }
}

fn ox_graph(g: &oxrdf::GraphName) -> String {
    match g {
        oxrdf::GraphName::DefaultGraph => "DEFAULT".into(),
        oxrdf::GraphName::NamedNode(n) => format!("<{}>", n.as_str()),
        oxrdf::GraphName::BlankNode(_) => "blank".into(),
    }
}

/// oxttl's quads for `src`, or its first error.
fn oxttl_quads(src: &str, syntax: Syntax) -> Result<Vec<oxrdf::Quad>, String> {
    let e = |e: oxttl::TurtleSyntaxError| e.to_string();
    match syntax {
        Syntax::Turtle => oxttl::TurtleParser::new()
            .for_slice(src)
            .map(|t| {
                t.map(|t| t.in_graph(oxrdf::GraphName::DefaultGraph))
                    .map_err(e)
            })
            .collect(),
        Syntax::TriG => oxttl::TriGParser::new()
            .for_slice(src)
            .map(|q| q.map_err(e))
            .collect(),
        Syntax::NQuads => oxttl::NQuadsParser::new()
            .for_slice(src)
            .map(|q| q.map_err(e))
            .collect(),
        Syntax::NTriples => oxttl::NTriplesParser::new()
            .for_slice(src)
            .map(|t| {
                t.map(|t| t.in_graph(oxrdf::GraphName::DefaultGraph))
                    .map_err(e)
            })
            .collect(),
        Syntax::JsonLd => unreachable!("oxttl reads no JSON-LD"),
    }
}

fn check(name: &str, src: &str) {
    check_as(name, src, Syntax::Turtle)
}

fn check_as(name: &str, src: &str, syntax: Syntax) {
    let ours = parse(src, &PrefixMap::default(), syntax);
    let errors: Vec<_> = ours.errors.iter().map(|e| (&e.message, e.span)).collect();
    assert!(errors.is_empty(), "{name}: unexpected errors {errors:?}");
    let ox = oxttl_quads(src, syntax)
        .unwrap_or_else(|e| panic!("{name}: oxttl rejected a fixture: {e}"));
    assert_eq!(ours.triples.len(), ox.len(), "{name}: triple count differs");
    let a: BTreeSet<String> = ours
        .triples
        .iter()
        .filter_map(|t| {
            Some(format!(
                "{} <{}> {} {}",
                node(&t.s)?,
                t.p,
                node(&t.o)?,
                graph(t.graph)
            ))
        })
        .collect();
    let b: BTreeSet<String> = ox
        .iter()
        .filter_map(|t| {
            let s = match &t.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => format!("<{}>", n.as_str()),
                _ => return None,
            };
            Some(format!(
                "{s} <{}> {} {}",
                t.predicate.as_str(),
                ox_term(&t.object)?,
                ox_graph(&t.graph_name)
            ))
        })
        .collect();
    // Every graph name is used the same way (blank names count per kind).
    let names = |v: Vec<String>| {
        let mut v = v;
        v.sort();
        v
    };
    assert_eq!(
        names(ours.triples.iter().map(|t| graph(t.graph)).collect()),
        names(ox.iter().map(|q| ox_graph(&q.graph_name)).collect()),
        "{name}: graph names differ"
    );
    let only_ours: Vec<_> = a.difference(&b).take(5).collect();
    let only_ox: Vec<_> = b.difference(&a).take(5).collect();
    assert!(
        only_ours.is_empty() && only_ox.is_empty(),
        "{name}: ours-only {only_ours:#?} oxttl-only {only_ox:#?}"
    );
}

#[test]
fn real_world_fixtures_match_oxttl() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/real");
    // Local-only fixtures (gitignored): skip when absent.
    if !dir.join("prefixes.ttl").exists() {
        eprintln!("skipping: test/fixtures/real not present");
        return;
    }
    let header = fs::read_to_string(dir.join("prefixes.ttl")).unwrap();
    let mut n = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if !name.ends_with(".ttl") || name == "prefixes.ttl" {
            continue;
        }
        check(
            &name,
            &(header.clone() + &fs::read_to_string(&path).unwrap()),
        );
        n += 1;
    }
    assert!(n > 30);
}

#[test]
fn workspace_fixtures_match_oxttl() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/workspace");
    for name in ["people.ttl", "org.ttl", "classes.ttl"] {
        check(name, &fs::read_to_string(dir.join(name)).unwrap());
    }
}

#[test]
fn project_fragments_match_oxttl() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/project");
    let header = fs::read_to_string(dir.join("prefixes.ttl")).unwrap();
    for name in [
        "core/vocab.ttl",
        "shapes/asset-shapes.ttl",
        "data/instances.ttl",
        "data/observations.ttl",
    ] {
        check(
            name,
            &(header.clone() + &fs::read_to_string(dir.join(name)).unwrap()),
        );
    }
}

#[test]
fn trig_fixtures_match_oxttl() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
    let header = fs::read_to_string(root.join("datasets/prefixes.ttl")).unwrap();
    let graphs = fs::read_to_string(root.join("datasets/graphs.trig")).unwrap();
    check_as("datasets/graphs.trig", &(header + &graphs), Syntax::TriG);
    let forms = fs::read_to_string(root.join("oracle/forms.trig")).unwrap();
    check_as("oracle/forms.trig", &forms, Syntax::TriG);
    // Plain Turtle is TriG too: every triple lands in the default graph.
    for name in ["people.ttl", "org.ttl", "classes.ttl"] {
        let text = fs::read_to_string(root.join("workspace").join(name)).unwrap();
        check_as(name, &text, Syntax::TriG);
    }
}

#[test]
fn ntriples_fixtures_match_oxttl() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
    for name in ["datasets/people.nt", "oracle/forms.nt"] {
        check_as(
            name,
            &fs::read_to_string(root.join(name)).unwrap(),
            Syntax::NTriples,
        );
    }
}

/// Turtle that is not N-Triples: both parsers reject it, and ours says why.
#[test]
fn turtle_only_constructs_are_not_ntriples() {
    let cases = [
        (
            "@prefix ex: <http://e/> .\n<http://e/a> <http://e/p> <http://e/o> .\n",
            "directives",
        ),
        ("<http://e/a> a <http://e/C> .\n", "'a'"),
        (
            "<http://e/a> <http://e/p> <http://e/o>, <http://e/o2> .\n",
            "';' or ','",
        ),
        ("<http://e/a> <http://e/p> [] .\n", "'[ ]'"),
        ("<http://e/a> <http://e/p> 42 .\n", "must be quoted"),
        ("<a> <http://e/p> <http://e/o> .\n", "absolute IRIs"),
        ("<http://e/a> <http://e/p> 'x' .\n", "double quotes"),
        (
            "<< <http://e/a> <http://e/p> <http://e/o> >> <http://e/q> <http://e/r> .\n",
            "reified",
        ),
    ];
    for (src, want) in cases {
        assert!(
            oxttl_quads(src, Syntax::NTriples).is_err(),
            "oxttl accepted {src:?} as N-Triples"
        );
        let ours = parse(src, &PrefixMap::default(), Syntax::NTriples);
        assert!(
            ours.errors.iter().any(|e| e.message.contains(want)),
            "{src:?}: {:?}",
            ours.errors
        );
        // And it is Turtle (relative IRIs need a base there, so not that one).
        if want != "absolute IRIs" {
            assert!(oxttl_quads(src, Syntax::Turtle).is_ok(), "{src:?}");
        }
    }
}
