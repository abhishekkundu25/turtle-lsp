//! A query that keeps too many rows stops soon after it passes the limit:
//! once the evaluation is canceled, each read of the data returns the
//! cancellation once and ends, so a join build under GROUP BY or ORDER BY
//! does not go on to read (and keep an error for) every remaining triple.
//!
//! Counted, not timed: the evaluator reads through a dataset that counts the
//! quads it hands out.

use oxrdf::{Dataset, GraphName, NamedNode, Quad, Term};
use spareval::{CancellationToken, InternalQuad, QueryEvaluator, QueryResults, QueryableDataset};
use spargebra::SparqlParser;
use std::cell::Cell;

/// `inner`, counting in `read` every quad a pattern read hands out.
struct Counting<'a> {
    inner: &'a Dataset,
    read: &'a Cell<usize>,
}

impl<'a> QueryableDataset<'a> for Counting<'a> {
    type InternalTerm = <&'a Dataset as QueryableDataset<'a>>::InternalTerm;
    type Error = <&'a Dataset as QueryableDataset<'a>>::Error;

    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Self::InternalTerm>,
        predicate: Option<&Self::InternalTerm>,
        object: Option<&Self::InternalTerm>,
        graph_name: Option<Option<&Self::InternalTerm>>,
    ) -> impl Iterator<Item = Result<InternalQuad<Self::InternalTerm>, Self::Error>> + use<'a> {
        let read = self.read;
        self.inner
            .internal_quads_for_pattern(subject, predicate, object, graph_name)
            .inspect(move |_| read.set(read.get() + 1))
    }

    fn internalize_term(&self, term: Term) -> Result<Self::InternalTerm, Self::Error> {
        self.inner.internalize_term(term)
    }

    fn externalize_term(&self, term: Self::InternalTerm) -> Result<Term, Self::Error> {
        self.inner.externalize_term(term)
    }
}

#[test]
fn a_group_by_or_order_by_over_a_large_scan_stops_reading_at_the_row_limit() {
    const N: usize = 10_000;
    let ex = |i: usize| NamedNode::new_unchecked(format!("http://example.com/n{i}"));
    let next = NamedNode::new_unchecked("http://example.com/next");
    let mut d = Dataset::new();
    for i in 0..N {
        d.insert(&Quad::new(
            ex(i),
            next.clone(),
            ex(i + 1),
            GraphName::DefaultGraph,
        ));
    }
    let mut failed = Vec::new();
    for q in [
        "SELECT ?a ?c (COUNT(*) AS ?n) WHERE { ?a <http://example.com/next> ?b . ?c <http://example.com/next> ?d } GROUP BY ?a ?c",
        "SELECT ?a ?c WHERE { ?a <http://example.com/next> ?b . ?c <http://example.com/next> ?d } ORDER BY ?a ?c",
    ] {
        let read = Cell::new(0);
        let token = CancellationToken::new().with_max_kept_tuples(10);
        let query = SparqlParser::new().parse_query(q).unwrap();
        let evaluator = QueryEvaluator::new().with_cancellation_token(token.clone());
        let data = Counting {
            inner: &d,
            read: &read,
        };
        if let Ok(QueryResults::Solutions(solutions)) = evaluator.prepare(&query).execute(data) {
            solutions.for_each(drop);
        }
        // Reading on after the stop would read every remaining triple.
        if !token.kept_too_many() || read.get() > N / 10 {
            failed.push((q, token.kept_too_many(), read.get()));
        }
    }
    assert_eq!(failed, []);
}
