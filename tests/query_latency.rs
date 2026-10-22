//! How soon a query stops at its time limit: wall-clock bounds, kept out of
//! the parallel unit tests (src/query/tests.rs checks the errors, counted
//! and ordered). This binary has one test, so its cases run one at a time,
//! and the bounds are generous: each is many times the limit.
//!
//!     cargo test --release --test query_latency

use std::sync::Arc;
use std::time::{Duration, Instant};
use turtle_lsp::analysis::Analysis;
use turtle_lsp::query::{Limits, QueryError, run};
use turtle_lsp::syntax::PrefixMap;

const EX: &str = "http://example.com/";

fn doc(text: String) -> Analysis {
    Analysis::new(
        "file:///w/data.ttl".into(),
        &format!("@prefix ex: <{EX}> .\n{text}"),
        Arc::new(PrefixMap::default()),
        false,
    )
}

/// `ex:n0 ex:next ex:n1 . ex:n1 ex:next ex:n2 . ...`, `n` links.
fn chain(n: usize) -> Analysis {
    doc((0..n)
        .map(|i| format!("ex:n{i} ex:next ex:n{} .\n", i + 1))
        .collect())
}

/// One case: `q` over `d` stops with `want` within `within`.
fn check(
    failed: &mut Vec<String>,
    d: &Analysis,
    q: &str,
    limits: &Limits,
    want: fn(&QueryError) -> bool,
    within: Duration,
) {
    let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
    let start = Instant::now();
    let r = run(&[d], q, None, &prefixes, limits);
    let took = start.elapsed();
    let short = &q[..q.len().min(70)];
    match r {
        Err(e) if want(&e) && took < within => {
            eprintln!("{took:>10.1?} (bound {within:?}): {short}");
        }
        Err(e) if want(&e) => failed.push(format!("{short}: {took:?}, bound {within:?}")),
        other => failed.push(format!("{short}: {other:?}")),
    }
}

#[test]
fn queries_stop_soon_after_their_time_limit() {
    let timeout = |ms| Limits {
        timeout: Some(Duration::from_millis(ms)),
        ..Limits::default()
    };
    let timed_out = |e: &QueryError| *e == QueryError::Timeout;
    let mut failed = Vec::new();

    // A query that runs for many seconds: 150 triples crossed with
    // themselves four times, 500 million rows.
    let big = doc((0..150).map(|i| format!("ex:s{i} ex:p {i} .\n")).collect());
    check(
        &mut failed,
        &big,
        "SELECT (COUNT(*) AS ?n) WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l }",
        &timeout(20),
        timed_out,
        Duration::from_secs(2),
    );

    // Cross products combined in memory, reading no data in between.
    let d = chain(400);
    for q in [
        "SELECT (COUNT(*) AS ?n) WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?a ex:next* ?b . ?c ex:next* ?d }",
    ] {
        check(
            &mut failed,
            &d,
            q,
            &timeout(20),
            timed_out,
            Duration::from_millis(500),
        );
    }

    // A hash join and a left join: the sorted subquery's 320 000 rows meet
    // the other side's 170 million in memory. The limit falls past the paths
    // and the sort (about 0.2 s), in the join.
    let d = chain(800);
    for q in [
        "SELECT (COUNT(*) AS ?n) WHERE { ?a ex:next* ?x . { SELECT ?b ?x WHERE { ?b ex:next* ?x } ORDER BY ?b } }",
        "SELECT (COUNT(*) AS ?n) WHERE { { SELECT ?a ?x WHERE { ?a ex:next* ?x } ORDER BY ?a } OPTIONAL { SELECT ?b ?x WHERE { ?b ex:next* ?x } ORDER BY ?b } }",
    ] {
        check(
            &mut failed,
            &d,
            q,
            &timeout(300),
            timed_out,
            Duration::from_secs(2),
        );
    }

    // A MINUS whose right side is an in-memory cross product of 36 million
    // rows, in each query form.
    let wide = doc((0..3000)
        .map(|i| format!("ex:s{i} ex:p ex:o{i} ; ex:q ex:r{i} .\n"))
        .collect());
    for q in [
        "ASK { VALUES ?d { ex:s2999 } MINUS { ?a ?b ?c . ?d ?e ?f } }",
        "SELECT ?d WHERE { VALUES ?d { ex:s2999 } MINUS { ?a ?b ?c . ?d ?e ?f } }",
        "CONSTRUCT { ?d ex:seen ex:yes } WHERE { VALUES ?d { ex:s2999 } MINUS { ?a ?b ?c . ?d ?e ?f } }",
    ] {
        check(
            &mut failed,
            &wide,
            q,
            &timeout(100),
            timed_out,
            Duration::from_secs(3),
        );
    }

    // A path of 1 000 steps is refused before planning (which would take
    // about 22 s, with no clock check on the way).
    let path = format!("ASK {{ ?s {} ?o }}", vec!["ex:p"; 1000].join("/"));
    check(
        &mut failed,
        &chain(3),
        &path,
        &Limits {
            timeout: Some(Duration::from_secs(1)),
            ..Limits::default()
        },
        |e| matches!(e, QueryError::TooComplex(_)),
        Duration::from_millis(1500),
    );

    assert!(failed.is_empty(), "{}", failed.join("\n"));
}
