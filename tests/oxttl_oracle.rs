//! Cross-checks the parser against oxttl (Oxigraph) on every valid fixture:
//! both must produce the same ground triples and the same triple count.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use turtle_lsp::syntax::{Node, PrefixMap, parse};

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
        Term::Literal(l) => Some(match l.language() {
            Some(lang) => format!("{:?}@{}", l.value(), lang),
            None => format!("{:?}^^<{}>", l.value(), l.datatype().as_str()),
        }),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

fn check(name: &str, src: &str) {
    let ours = parse(src, &PrefixMap::default());
    let errors: Vec<_> = ours.errors.iter().map(|e| (&e.message, e.span)).collect();
    assert!(errors.is_empty(), "{name}: unexpected errors {errors:?}");
    let mut ox = Vec::new();
    for t in oxttl::TurtleParser::new().for_slice(src) {
        ox.push(t.unwrap_or_else(|e| panic!("{name}: oxttl rejected a fixture: {e}")));
    }
    assert_eq!(ours.triples.len(), ox.len(), "{name}: triple count differs");
    let a: BTreeSet<String> = ours
        .triples
        .iter()
        .filter_map(|t| Some(format!("{} <{}> {}", node(&t.s)?, t.p, node(&t.o)?)))
        .collect();
    let b: BTreeSet<String> = ox
        .iter()
        .filter_map(|t| {
            let s = match &t.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => format!("<{}>", n.as_str()),
                _ => return None,
            };
            Some(format!(
                "{s} <{}> {}",
                t.predicate.as_str(),
                ox_term(&t.object)?
            ))
        })
        .collect();
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
    for name in ["core/vocab.ttl", "shapes/asset-shapes.ttl", "data/instances.ttl", "data/observations.ttl"] {
        check(name, &(header.clone() + &fs::read_to_string(dir.join(name)).unwrap()));
    }
}
