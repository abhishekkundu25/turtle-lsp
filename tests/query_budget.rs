//! `query.maxBufferedRows` counts the rows held in memory at once: a buffer
//! rebuilt for each outer row gives its rows back when it is dropped, so many
//! small buffers in turn stay under a limit their total passes.

use std::sync::Arc;
use turtle_lsp::analysis::Analysis;
use turtle_lsp::query::{Answer, Limits, QueryError, run};
use turtle_lsp::syntax::PrefixMap;

const EX: &str = "http://example.com/";

/// `ex:n0 ex:next ex:n1 . ex:n1 ex:next ex:n2 . ...`, `n` links.
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

fn count(q: &str, max_buffered_rows: usize) -> Result<String, QueryError> {
    let d = chain(200);
    let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
    let limits = Limits {
        max_buffered_rows: Some(max_buffered_rows),
        ..Limits::default()
    };
    match run(&[&d], q, None, &prefixes, &limits)? {
        Answer::Solutions(s) => Ok(format!("{:?}", s.rows)),
        other => panic!("not solutions: {other:?}"),
    }
}

#[test]
fn a_buffer_rebuilt_per_row_counts_only_its_own_rows() {
    // The hash join inside EXISTS is built once per outer row (200 in all),
    // holding one row each time.
    let q = "SELECT (COUNT(*) AS ?n) WHERE { ?a ex:next ?b FILTER EXISTS { ?b ex:next ?c . ?c ex:next ?d } }";
    let answer = count(q, 50).expect("held at once under the limit");
    assert!(answer.contains("\"198\""), "{answer}");
}

#[test]
fn a_buffer_over_the_limit_still_stops() {
    let q = "SELECT ?s WHERE { ?s ex:next ?o } ORDER BY ?s";
    assert_eq!(count(q, 50), Err(QueryError::TooManyRows(50)));
}
