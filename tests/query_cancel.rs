//! A query that keeps too many rows stops soon after it passes
//! `max_buffered_rows`: once the evaluation is canceled, each read of the
//! data returns the cancellation once and ends, so a join build under GROUP
//! BY or ORDER BY does not go on to read (and keep an error for) every
//! remaining triple.
//!
//! [`run`] indexes the documents first, which also takes time in proportion
//! to the triples, so each query is held within 2x a query that reads one
//! triple the same way; reading on to the end took 3x or more. Timings mean nothing in a
//! debug build, so the test runs in release only:
//!
//!     cargo test --release --test query_cancel
#![cfg(not(debug_assertions))]

use std::sync::Arc;
use std::time::{Duration, Instant};
use turtle_lsp::analysis::Analysis;
use turtle_lsp::query::{Limits, QueryError, run};
use turtle_lsp::syntax::PrefixMap;

const EX: &str = "http://example.com/";

/// A chain of `n` `ex:next` links.
fn chain(n: usize) -> Analysis {
    let mut text = format!("@prefix ex: <{EX}> .\n");
    for i in 0..n {
        text.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    Analysis::new(
        "file:///w/chain.ttl".into(),
        &text,
        Arc::new(PrefixMap::default()),
        false,
    )
}

#[test]
fn a_group_by_or_order_by_over_a_large_scan_stops_at_the_row_limit() {
    let d = chain(1_000_000);
    let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
    let limits = Limits {
        max_buffered_rows: Some(10),
        ..Limits::default()
    };
    let timed = |q: &str| {
        let start = Instant::now();
        let answer = run(&[&d], q, None, &prefixes, &limits);
        (answer, start.elapsed())
    };
    let one = (0..3)
        .map(|_| timed("ASK { ?a ex:next ?b }").1)
        .min()
        .unwrap_or(Duration::ZERO);
    let mut failed = Vec::new();
    for q in [
        "SELECT ?a ?c (COUNT(*) AS ?n) WHERE { ?a ex:next ?b . ?c ex:next ?d } GROUP BY ?a ?c",
        "SELECT ?a ?c WHERE { ?a ex:next ?b . ?c ex:next ?d } ORDER BY ?a ?c",
    ] {
        let (answer, took) = timed(q);
        if answer != Err(QueryError::TooManyRows(10)) || took > one * 2 {
            failed.push((q, answer.err(), took, one));
        }
    }
    assert_eq!(failed, []);
}
