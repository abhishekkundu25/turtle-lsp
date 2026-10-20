//! Validate RDF files with the server's SHACL engine and print the results as
//! JSON lines (shapes and data are read from the same files, as the editor
//! does). For comparing with other validators.
//!
//!     cargo run --release --example shacl -- FILE.ttl|FILE.trig ...

use std::time::Instant;
use turtle_lsp::rdf::graph::{Graph, Id, Term};
use turtle_lsp::shacl::validate;
use turtle_lsp::syntax::{Node, PrefixMap, Syntax, parse};

fn term(g: &Graph, id: Id) -> Option<String> {
    match g.term(id) {
        Term::Iri(i) => Some(i.to_string()),
        Term::Blank(_) => None,
        Term::Value(Node::Literal { value, .. }) => Some(value.to_string()),
        Term::Value(_) => None,
    }
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let t = Instant::now();
    let texts: Vec<String> = files
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let parsed: Vec<_> = files
        .iter()
        .zip(&texts)
        .map(|(f, text)| {
            parse(
                text,
                &PrefixMap::default(),
                Syntax::from_extension(f).unwrap_or_default(),
            )
        })
        .collect();
    for (f, p) in files.iter().zip(&parsed) {
        assert!(
            p.errors.is_empty(),
            "{f}: {:?}",
            &p.errors[..p.errors.len().min(3)]
        );
    }
    let docs: Vec<&[_]> = parsed.iter().map(|p| p.triples.as_slice()).collect();
    let g = Graph::new(&docs);
    let loaded = t.elapsed();
    let t = Instant::now();
    let report = validate::validate_report(&g, &|| false, &|_| true).unwrap();
    let validated = t.elapsed();
    for r in &report.results {
        let message = r
            .computed_messages
            .first()
            .map(|(m, _)| m.clone())
            .or_else(|| {
                r.messages
                    .first()
                    .and_then(|m| g.term(*m).literal().map(|l| l.0.to_string()))
            });
        println!(
            "{}",
            serde_json::json!({
                "severity": r.severity.to_string(),
                "focus": term(&g, r.focus),
                "value": r.value.and_then(|v| term(&g, v)),
                "shape": term(&g, r.shape),
                "component": r.component.to_string(),
                "message": message,
            })
        );
    }
    for f in &report.failures {
        eprintln!("failure: {}", f.message);
    }
    eprintln!(
        "{} results, {} failures; load {:.0} ms, validate {:.0} ms",
        report.results.len(),
        report.failures.len(),
        loaded.as_secs_f64() * 1000.0,
        validated.as_secs_f64() * 1000.0
    );
}
