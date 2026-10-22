//! Cross-checks the parser against oxttl (Oxigraph) on every valid fixture:
//! both must produce the same number of triples (quads for TriG) and
//! isomorphic datasets, blank nodes, triple terms and graph names included
//! (tests/support/oracle.rs). Turtle fixtures are compared with oxttl's
//! Turtle parser, TriG with its TriG parser, N-Triples with its N-Triples parser.

use std::fs;
use std::path::Path;
use turtle_lsp::syntax::{PrefixMap, Syntax, parse};

#[path = "support/oracle.rs"]
mod oracle;

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
    let errors: Vec<_> = ours.errors.iter().map(|e| (e.message(), e.span)).collect();
    assert!(errors.is_empty(), "{name}: unexpected errors {errors:?}");
    let ox = oxttl_quads(src, syntax)
        .unwrap_or_else(|e| panic!("{name}: oxttl rejected a fixture: {e}"));
    assert_eq!(ours.triples.len(), ox.len(), "{name}: triple count differs");
    if let Some(diff) = oracle::difference(
        &oracle::ours(&ours.triples),
        "ours",
        &oracle::canonical(ox),
        "oxttl",
        5,
    ) {
        panic!("{name}: {diff}");
    }
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
            ours.errors.iter().any(|e| e.message().contains(want)),
            "{src:?}: {:?}",
            ours.errors
        );
        // And it is Turtle (relative IRIs need a base there, so not that one).
        if want != "absolute IRIs" {
            assert!(oxttl_quads(src, Syntax::Turtle).is_ok(), "{src:?}");
        }
    }
}

/// The oracle's own check: blank nodes match under renaming, a changed or
/// missing quad does not, and the fingerprint of a difference does not
/// depend on blank node labels.
#[test]
fn the_oracle_compares_up_to_blank_node_renaming() {
    let ttl = |src: &str| oracle::canonical(oxttl_quads(src, Syntax::Turtle).unwrap());
    let a = ttl("<http://e/s> <http://e/p> _:x . _:x <http://e/q> \"v\"@EN .\n");
    let renamed = ttl("_:other <http://e/q> \"v\"@en . <http://e/s> <http://e/p> _:other .\n");
    assert_eq!(oracle::difference(&a, "a", &renamed, "b", 5), None);
    assert_eq!(oracle::fingerprint(&a, &renamed), 0);
    let changed = ttl("<http://e/s> <http://e/p> _:x . _:x <http://e/q> \"w\"@en .\n");
    let diff = oracle::difference(&a, "a", &changed, "b", 5).expect("a difference");
    assert!(
        diff.contains("only a (1)") && diff.contains("only b (1)"),
        "{diff}"
    );
    let changed_again = ttl("<http://e/s> <http://e/p> _:y . _:y <http://e/q> \"w\"@en .\n");
    assert_ne!(oracle::fingerprint(&a, &changed), 0);
    assert_eq!(
        oracle::fingerprint(&a, &changed),
        oracle::fingerprint(&a, &changed_again)
    );
    let other = ttl("<http://e/s> <http://e/p> _:x . _:x <http://e/q> \"z\"@en .\n");
    assert_ne!(
        oracle::fingerprint(&a, &changed),
        oracle::fingerprint(&a, &other)
    );
    // Ours, converted: the same as oxttl's.
    let src = "@prefix e: <http://e/> .\ne:s e:p [ e:q \"v\"@en ] .\n";
    let ours = parse(src, &PrefixMap::default(), Syntax::Turtle);
    assert_eq!(
        oracle::difference(&oracle::ours(&ours.triples), "ours", &a, "oxttl", 5),
        None
    );
}
