use super::render::{csv, json, markdown, table, tsv, turtle};
use super::*;
use crate::sparql::spargebra_input::MAX_JOIN_WIDTH;
use spargebra::term::{BlankNode, Literal, NamedNode};
use std::sync::Arc;

const EX: &str = "http://example.com/";
const HEAD: &str = "@prefix ex: <http://example.com/> .\n";

fn doc(uri: &str, text: &str) -> Arc<Analysis> {
    Arc::new(Analysis::new(
        uri.into(),
        text,
        Arc::new(PrefixMap::default()),
        false,
    ))
}

/// [`super::run`] without inferred triples: its answer, the note checked
/// to be none.
fn run(
    docs: &[&Arc<Analysis>],
    q: &str,
    base: Option<&str>,
    prefixes: &PrefixMap,
    limits: &Limits,
) -> Result<Answer, QueryError> {
    super::run(docs, q, base, prefixes, limits, &Inference::default()).map(|o| {
        assert_eq!(o.inferred, Inferred::None);
        o.answer
    })
}

fn ex_prefixes() -> PrefixMap {
    PrefixMap::from_iter([("ex".to_string(), EX.to_string())])
}

fn people() -> Arc<Analysis> {
    doc(
        "file:///w/people.ttl",
        &format!(
            "{HEAD}ex:alice ex:name \"Alice\" ; ex:age 30 ; ex:knows ex:bob .\n\
             ex:bob ex:name \"Bob\" ; ex:age 25 .\n\
             ex:carol ex:name \"Carol\"@en ; ex:age 41 ; ex:knows ex:alice, ex:bob .\n"
        ),
    )
}

fn run_on(docs: &[&Arc<Analysis>], q: &str) -> Result<Answer, QueryError> {
    run(docs, q, None, &ex_prefixes(), &Limits::default())
}

fn select(docs: &[&Arc<Analysis>], q: &str) -> Solutions {
    match run_on(docs, q) {
        Ok(Answer::Solutions(s)) => s,
        other => panic!("not solutions: {other:?}"),
    }
}

fn iri(local: &str) -> Term {
    NamedNode::new_unchecked(format!("{EX}{local}")).into()
}

/// The rows with each term compacted, unbound as "-".
fn shown(s: &Solutions) -> Vec<Vec<String>> {
    let p = ex_prefixes();
    s.rows
        .iter()
        .map(|r| {
            r.iter()
                .map(|t| t.as_ref().map_or("-".into(), |t| render::term(t, &p)))
                .collect()
        })
        .collect()
}

// --- query forms -------------------------------------------------------------

#[test]
fn select_with_order_by_and_optional() {
    let d = people();
    let s = select(
        &[&d],
        "SELECT ?p ?friend WHERE { ?p ex:age ?age OPTIONAL { ?p ex:knows ?friend } } ORDER BY ?age ?friend",
    );
    assert_eq!(s.variables, ["p", "friend"]);
    assert_eq!(
        shown(&s),
        [
            ["ex:bob", "-"],
            ["ex:alice", "ex:bob"],
            ["ex:carol", "ex:alice"],
            ["ex:carol", "ex:bob"],
        ]
    );
}

#[test]
fn select_with_aggregates() {
    let d = people();
    let s = select(
        &[&d],
        "SELECT (COUNT(?p) AS ?n) (SUM(?a) AS ?total) (MAX(?a) AS ?oldest) WHERE { ?p ex:age ?a }",
    );
    assert_eq!(s.variables, ["n", "total", "oldest"]);
    assert_eq!(shown(&s), [["3", "96", "41"]]);
    let s = select(
        &[&d],
        "SELECT ?p (COUNT(?f) AS ?n) WHERE { ?p ex:knows ?f } GROUP BY ?p ORDER BY DESC(?n)",
    );
    assert_eq!(shown(&s), [["ex:carol", "2"], ["ex:alice", "1"]]);
}

#[test]
fn ask_true_and_false() {
    let d = people();
    assert_eq!(
        run_on(&[&d], "ASK { ex:alice ex:knows ex:bob }"),
        Ok(Answer::Boolean(true))
    );
    assert_eq!(
        run_on(&[&d], "ASK { ex:bob ex:knows ex:alice }"),
        Ok(Answer::Boolean(false))
    );
}

#[test]
fn construct_gives_each_triple_once() {
    let d = people();
    let Ok(Answer::Triples(ts)) = run_on(
        &[&d],
        "CONSTRUCT { ?b ex:knownBy ?a . ?b a ex:Known } WHERE { ?a ex:knows ?b }",
    ) else {
        panic!()
    };
    // ex:bob is known twice: one `a ex:Known` triple for it.
    assert_eq!(ts.len(), 5);
    let set: FxHashSet<String> = ts.iter().map(|t| t.to_string()).collect();
    assert!(set.contains(&format!("<{EX}bob> <{EX}knownBy> <{EX}carol>")));
    assert!(set.contains(&format!(
        "<{EX}bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <{EX}Known>"
    )));
}

#[test]
fn describe_gives_the_resources_triples() {
    let d = people();
    let Ok(Answer::Triples(ts)) = run_on(&[&d], "DESCRIBE ex:bob") else {
        panic!()
    };
    assert_eq!(ts.len(), 2);
    assert!(
        ts.iter()
            .all(|t| t.subject.to_string() == format!("<{EX}bob>"))
    );
}

// --- dataset -----------------------------------------------------------------

#[test]
fn default_graph_is_the_union_of_turtle_and_trig() {
    let ttl = doc(
        "file:///w/a.ttl",
        &format!("{HEAD}ex:a ex:p ex:b .\n_:x ex:p ex:c .\n"),
    );
    let trig = doc(
        "file:///w/b.trig",
        &format!("{HEAD}ex:d ex:p ex:e .\nex:g {{ ex:f ex:p ex:h . _:x ex:p ex:i . }}\n"),
    );
    let s = select(
        &[&ttl, &trig],
        "SELECT ?s ?o WHERE { ?s ex:p ?o } ORDER BY ?o",
    );
    let rows = shown(&s);
    let objects: Vec<&str> = rows.iter().map(|r| r[1].as_str()).collect();
    assert_eq!(objects, ["ex:b", "ex:c", "ex:e", "ex:h", "ex:i"]);
    // `_:x` of one document is not `_:x` of the other.
    let s = select(
        &[&ttl, &trig],
        "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ex:p ?o FILTER isBlank(?s) }",
    );
    assert_eq!(shown(&s), [["2"]]);
}

#[test]
fn graph_patterns_and_from_named_match_nothing_and_from_is_ignored() {
    let trig = doc(
        "file:///w/b.trig",
        &format!("{HEAD}ex:g {{ ex:f ex:p ex:h . }}\n"),
    );
    let s = select(&[&trig], "SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }");
    assert!(s.rows.is_empty());
    let s = select(&[&trig], "SELECT * WHERE { GRAPH ex:g { ?s ?p ?o } }");
    assert!(s.rows.is_empty());
    let s = select(
        &[&trig],
        "SELECT * FROM NAMED ex:g WHERE { GRAPH ?g { ?s ?p ?o } }",
    );
    assert!(s.rows.is_empty());
    let s = select(&[&trig], "SELECT ?s FROM ex:other WHERE { ?s ?p ?o }");
    assert_eq!(shown(&s), [["ex:f"]]);
}

// --- errors ------------------------------------------------------------------

#[test]
fn update_is_unsupported() {
    let d = people();
    let r = run_on(&[&d], "INSERT DATA { ex:a ex:b ex:c }");
    assert!(matches!(r, Err(QueryError::Unsupported(m)) if m.contains("UPDATE")));
    let r = run_on(&[&d], "PREFIX x: <http://x/>\nDELETE WHERE { ?s ?p ?o }");
    assert!(matches!(r, Err(QueryError::Unsupported(_))));
}

#[test]
fn syntax_error_has_its_position() {
    let d = people();
    let q = "SELECT ?s\nWHERE { ?s ex:p ?o .\n  ?s ?p }";
    let Err(QueryError::Syntax {
        offset,
        line,
        column,
        ..
    }) = run_on(&[&d], q)
    else {
        panic!()
    };
    assert_eq!((line, column), (3, 9));
    assert_eq!(&q[offset..], "}");
    // An undeclared prefix is an error at its name.
    let q = "SELECT * WHERE { ?s nope:p ?o }";
    let Err(QueryError::Syntax {
        offset, message, ..
    }) = run_on(&[&d], q)
    else {
        panic!()
    };
    assert_eq!(&q[offset..offset + 6], "nope:p");
    assert!(message.contains("nope:"), "{message}");
}

#[test]
fn prefixes_and_base_resolve_headerless_queries() {
    let d = people();
    let r = run(
        &[&d],
        "ASK { <alice> ex:knows ex:bob }",
        Some(EX),
        &ex_prefixes(),
        &Limits::default(),
    );
    assert_eq!(r, Ok(Answer::Boolean(true)));
}

/// A query that runs for many seconds: the data (150 triples) crossed with
/// itself four times, 500 million rows.
const RUNAWAY: &str = "SELECT (COUNT(*) AS ?n) WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l }";

fn big() -> Arc<Analysis> {
    let mut text = HEAD.to_string();
    for i in 0..150 {
        text.push_str(&format!("ex:s{i} ex:p {i} .\n"));
    }
    doc("file:///w/big.ttl", &text)
}

// The time-limit tests check the error only: how soon a query stops is a
// latency, checked by tests/query_latency.rs in a test binary of its own.
// The cancellations are ordered, not timed: the flag is set once the
// evaluation has made a number of checks ([`cancelled_after`]).

#[test]
fn a_runaway_query_stops_at_the_time_limit() {
    let d = big();
    let r = run(
        &[&d],
        RUNAWAY,
        None,
        &ex_prefixes(),
        &Limits {
            timeout: Some(Duration::from_millis(20)),
            ..Limits::default()
        },
    );
    assert_eq!(r, Err(QueryError::Timeout));
}

/// The watch a test evaluates `q` over `d` with, and the dataset: what
/// [`run`] builds, so a test can read the watch's count of checks.
fn evaluate_with(d: &Analysis, q: &str, limits: &Limits) -> (Result<Answer, QueryError>, u32) {
    let watch = Arc::new(Watch::new(limits));
    let parsed = parse(q, None, &ex_prefixes()).unwrap();
    let triples = [d.parsed.triples.as_slice()];
    let graph = Graph::new(&triples);
    let index = Index::new(&graph, None);
    let r = evaluate(&index, &parsed, &watch, limits);
    (r, watch.checks.load(Ordering::Relaxed))
}

/// Evaluates `q` over `d` with a cancel flag that another thread sets once
/// the evaluation has made `after` checks, so the flag lands while the query
/// runs however loaded the machine is. The watch counts its checks only with
/// a time limit: an hour, never reached.
fn cancelled_after(d: &Analysis, q: &str, after: u32) -> Result<Answer, QueryError> {
    let flag = Arc::new(AtomicBool::new(false));
    let limits = Limits {
        timeout: Some(Duration::from_secs(3600)),
        cancel: Some(flag.clone()),
        ..Limits::default()
    };
    let watch = Arc::new(Watch::new(&limits));
    let parsed = parse(q, None, &ex_prefixes()).unwrap();
    let triples = [d.parsed.triples.as_slice()];
    let graph = Graph::new(&triples);
    let index = Index::new(&graph, None);
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            while watch.checks.load(Ordering::Relaxed) < after && !done.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
            flag.store(true, Ordering::Relaxed);
        });
        let r = evaluate(&index, &parsed, &watch, &limits);
        done.store(true, Ordering::Relaxed);
        r
    })
}

#[test]
fn a_cancelled_query_stops() {
    assert_eq!(
        cancelled_after(&big(), RUNAWAY, 100),
        Err(QueryError::Cancelled)
    );
}

/// A chain of `n` `ex:next` links.
fn chain(n: usize) -> Arc<Analysis> {
    let mut text = HEAD.to_string();
    for i in 0..n {
        text.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    doc("file:///w/chain.ttl", &text)
}

/// Cross products that combine rows in memory, reading no data in between:
/// each row of the first meets the 64 million rows of the other three
/// patterns; the two paths are computed first, then their 80 000 pairs meet.
const IN_MEMORY: [&str; 2] = [
    "SELECT (COUNT(*) AS ?n) WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l }",
    "SELECT (COUNT(*) AS ?n) WHERE { ?a ex:next* ?b . ?c ex:next* ?d }",
];

#[test]
fn a_join_in_memory_stops_at_the_time_limit() {
    let d = chain(400);
    for q in IN_MEMORY {
        let limits = Limits {
            timeout: Some(Duration::from_millis(20)),
            ..Limits::default()
        };
        assert_eq!(
            run(&[&d], q, None, &ex_prefixes(), &limits),
            Err(QueryError::Timeout),
            "{q}"
        );
    }
}

#[test]
fn a_join_in_memory_stops_when_cancelled() {
    let d = chain(400);
    for q in IN_MEMORY {
        // Past the reads of the data (a few thousand checks), in the join.
        assert_eq!(
            cancelled_after(&d, q, 20_000),
            Err(QueryError::Cancelled),
            "{q}"
        );
    }
}

/// Counted, not timed: a cross product combined in memory checks the watch
/// every 1024 rows it combines (the spareval patch, vendor/spareval), so the
/// checks of the whole evaluation are at least the rows / 1024. Reading the
/// data alone checks far less often (one check a triple), so a join that
/// stopped checking fails here at once instead of running for minutes in
/// the tests above. (The paths of `IN_MEMORY[1]` read about as often as
/// their join combines rows: no use for a count.)
#[test]
fn a_join_in_memory_checks_the_watch_as_it_combines_rows() {
    let limits = Limits {
        timeout: Some(Duration::from_secs(3600)),
        ..Limits::default()
    };
    let d = chain(30);
    let (r, checks) = evaluate_with(&d, IN_MEMORY[0], &limits);
    let rows = match r {
        Ok(Answer::Solutions(s)) => match &s.rows[0][0] {
            Some(Term::Literal(l)) => l.value().parse::<u32>().unwrap(),
            other => panic!("{other:?}"),
        },
        other => panic!("{other:?}"),
    };
    assert_eq!(rows, 30 * 30 * 30 * 30);
    // The four patterns' reads.
    let (_, one_pattern) = evaluate_with(&d, "SELECT * WHERE { ?a ?b ?c }", &limits);
    let reads = 4 * one_pattern;
    assert!(
        reads * 2 < rows / 1024,
        "the reads alone ({reads} checks) would pass for {rows} rows"
    );
    assert!(
        checks >= rows / 1024,
        "{checks} checks for {rows} rows combined"
    );
}

/// A hash join and a left join that combine rows in memory: the sorted
/// subquery is computed first, so its 320 000 rows meet the other side's
/// (170 million rows) with no data read. Unchecked they run for seconds.
#[test]
fn a_hash_join_in_memory_stops_at_the_time_limit() {
    let d = chain(800);
    for q in [
        "SELECT (COUNT(*) AS ?n) WHERE { ?a ex:next* ?x . { SELECT ?b ?x WHERE { ?b ex:next* ?x } ORDER BY ?b } }",
        "SELECT (COUNT(*) AS ?n) WHERE { { SELECT ?a ?x WHERE { ?a ex:next* ?x } ORDER BY ?a } OPTIONAL { SELECT ?b ?x WHERE { ?b ex:next* ?x } ORDER BY ?b } }",
    ] {
        let limits = Limits {
            timeout: Some(Duration::from_millis(300)),
            ..Limits::default()
        };
        assert_eq!(
            run(&[&d], q, None, &ex_prefixes(), &limits),
            Err(QueryError::Timeout),
            "{q}"
        );
    }
}

/// Unpatched spareval 0.2.7 panics when `=` compares two directional
/// language-tagged strings (`rdf:dirLangString`): see
/// `vendor/spareval/PATCH.md`. They are equal when the value, the language
/// and the direction all are.
#[test]
fn equality_of_directional_language_strings_is_answered() {
    let d = doc(
        "file:///w/dir.ttl",
        &format!("{HEAD}ex:a ex:p \"x\"@en--ltr .\n"),
    );
    let ask = |filter: &str| run_on(&[&d], &format!("ASK {{ ex:a ex:p ?o FILTER({filter}) }}"));
    assert_eq!(ask("?o = \"x\"@en--ltr"), Ok(Answer::Boolean(true)));
    assert_eq!(ask("?o = \"x\"@en--rtl"), Ok(Answer::Boolean(false)));
    assert_eq!(ask("?o = \"x\"@de--ltr"), Ok(Answer::Boolean(false)));
    assert_eq!(ask("?o = \"y\"@en--ltr"), Ok(Answer::Boolean(false)));
    assert_eq!(ask("?o != \"x\"@en--rtl"), Ok(Answer::Boolean(true)));
}

fn capped(docs: &[&Arc<Analysis>], q: &str, max: usize) -> Answer {
    let limits = Limits {
        max_rows: Some(max),
        ..Limits::default()
    };
    run(docs, q, None, &ex_prefixes(), &limits).unwrap()
}

#[test]
fn the_row_cap_keeps_the_first_rows_and_counts_the_rest() {
    let d = big();
    let q = "SELECT ?s WHERE { ?s ex:p ?o } ORDER BY ?o";
    let Answer::Solutions(all) = run_on(&[&d], q).unwrap() else {
        panic!("not solutions")
    };
    assert_eq!((all.rows.len(), all.omitted), (150, 0));
    let Answer::Solutions(s) = capped(&[&d], q, 10) else {
        panic!("not solutions")
    };
    assert_eq!((s.rows.len(), s.omitted, s.total()), (10, 140, 150));
    assert_eq!(s.rows[..], all.rows[..10]);
    let m = markdown(&Answer::Solutions(s.clone()), &ex_prefixes(), None);
    assert!(m.starts_with("150 rows\n"), "{m}");
    assert!(m.ends_with("_Showing 10 of 150 rows._\n"), "{m}");
    assert!(table(&s, &ex_prefixes(), None).ends_with("showing 10 of 150 rows\n"));
    // Under the cap: nothing omitted.
    let Answer::Solutions(s) = capped(&[&d], q, 1000) else {
        panic!("not solutions")
    };
    assert_eq!((s.rows.len(), s.omitted), (150, 0));
    // Zero keeps none but still counts.
    let Answer::Solutions(s) = capped(&[&d], q, 0) else {
        panic!("not solutions")
    };
    assert_eq!((s.rows.len(), s.total()), (0, 150));
    let m = markdown(&Answer::Solutions(s), &ex_prefixes(), None);
    assert!(m.contains("_Showing 0 of 150 rows._"), "{m}");
}

#[test]
fn the_row_cap_counts_distinct_triples() {
    let d = big();
    // Every ?s twice: the duplicates are not counted.
    let q = "CONSTRUCT { ?s ex:q 1 } WHERE { { ?s ex:p ?o } UNION { ?s ex:p ?o } }";
    let Answer::Triples(ts) = capped(&[&d], q, 5) else {
        panic!("not triples")
    };
    assert_eq!((ts.len(), ts.omitted, ts.total()), (5, 145, 150));
    let m = markdown(&Answer::Triples(ts), &ex_prefixes(), Some(1000));
    assert!(m.starts_with("150 triples\n"), "{m}");
    assert!(m.ends_with("_Showing 5 of 150 triples._\n"), "{m}");
}

#[test]
fn display_prefixes_add_the_querys_own() {
    let p = display_prefixes(
        "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nASK {}",
        &ex_prefixes(),
    );
    assert_eq!(
        p.get("foaf").map(String::as_str),
        Some("http://xmlns.com/foaf/0.1/")
    );
    assert_eq!(p.get("ex").map(String::as_str), Some(EX));
}

// --- renderers ---------------------------------------------------------------

fn lit(v: &str) -> Term {
    Literal::new_simple_literal(v).into()
}

fn typed(v: &str, dt: &str) -> Term {
    Literal::new_typed_literal(v, NamedNode::new_unchecked(dt)).into()
}

fn lang(v: &str, l: &str) -> Term {
    Literal::new_language_tagged_literal_unchecked(v, l).into()
}

const XSD_INT: &str = "http://www.w3.org/2001/XMLSchema#integer";

fn sample() -> Solutions {
    Solutions {
        variables: vec!["s".into(), "v".into()],
        rows: vec![
            vec![Some(iri("a")), Some(lit("x|y"))],
            vec![Some(iri("bee")), Some(typed("7", XSD_INT))],
            vec![Some(BlankNode::new_unchecked("g1").into()), None],
            vec![Some(iri("c")), Some(lang("say \"hi\",\nok", "en"))],
        ],
        omitted: 0,
    }
}

#[test]
fn table_is_aligned_and_compact() {
    let s = sample();
    let t = table(&s, &ex_prefixes(), None);
    assert_eq!(
        t,
        "?s     | ?v\n\
         -------+---------------------\n\
         ex:a   | \"x|y\"\n\
         ex:bee | 7\n\
         _:g1   | \n\
         ex:c   | \"say \\\"hi\\\",\\nok\"@en\n"
    );
    let t = table(&s, &ex_prefixes(), Some(1));
    assert_eq!(
        t,
        "?s   | ?v\n-----+------\nex:a | \"x|y\"\nshowing 1 of 4 rows\n"
    );
}

#[test]
fn markdown_escapes_pipes_and_cuts_at_the_row_limit() {
    let s = sample();
    let m = markdown(&Answer::Solutions(s.clone()), &ex_prefixes(), Some(2));
    assert_eq!(
        m,
        "4 rows\n\n\
         | ?s | ?v |\n\
         |---|---|\n\
         | `ex:a` | `\"x\\|y\"` |\n\
         | `ex:bee` | `7` |\n\
         \n_Showing 2 of 4 rows._\n"
    );
    let m = markdown(&Answer::Solutions(s), &ex_prefixes(), None);
    assert!(m.contains("| `_:g1` |  |\n"), "{m}");
    assert!(m.contains("`\"say \\\"hi\\\",\\nok\"@en`"), "{m}");
    assert!(!m.contains("Showing"));
    let code = markdown(
        &Answer::Solutions(Solutions {
            variables: vec!["v".into()],
            rows: vec![vec![Some(lit("a`b"))]],
            omitted: 0,
        }),
        &PrefixMap::default(),
        None,
    );
    assert!(code.contains("| `` \"a`b\" `` |"), "{code}");
    assert_eq!(
        markdown(&Answer::Boolean(false), &PrefixMap::default(), None),
        "**false**\n"
    );
    assert_eq!(
        markdown(
            &Answer::Solutions(Solutions::default()),
            &PrefixMap::default(),
            None
        ),
        "No results.\n"
    );
}

#[test]
fn markdown_triples_are_a_turtle_block() {
    let t = Triple::new(
        NamedNode::new_unchecked(format!("{EX}a")),
        NamedNode::new_unchecked(format!("{EX}p")),
        lit("v"),
    );
    let m = markdown(
        &Answer::Triples(vec![t.clone(), t].into()),
        &ex_prefixes(),
        Some(1),
    );
    assert_eq!(
        m,
        "2 triples\n\n```turtle\n@prefix ex: <http://example.com/> .\n\nex:a ex:p \"v\" .\n```\n\n_Showing 1 of 2 triples._\n"
    );
}

#[test]
fn json_results_with_datatypes_languages_and_booleans() {
    let j: serde_json::Value =
        serde_json::from_str(&json(&Answer::Solutions(sample())).unwrap()).unwrap();
    assert_eq!(j["head"]["vars"], serde_json::json!(["s", "v"]));
    let b = &j["results"]["bindings"];
    assert_eq!(
        b[0]["s"],
        serde_json::json!({"type": "uri", "value": format!("{EX}a")})
    );
    assert_eq!(
        b[0]["v"],
        serde_json::json!({"type": "literal", "value": "x|y"})
    );
    assert_eq!(
        b[1]["v"],
        serde_json::json!({"type": "literal", "value": "7", "datatype": XSD_INT})
    );
    assert_eq!(
        b[2]["s"],
        serde_json::json!({"type": "bnode", "value": "g1"})
    );
    assert!(b[2].get("v").is_none());
    assert_eq!(
        b[3]["v"],
        serde_json::json!({"type": "literal", "value": "say \"hi\",\nok", "xml:lang": "en"})
    );
    let t: serde_json::Value =
        serde_json::from_str(&json(&Answer::Boolean(true)).unwrap()).unwrap();
    assert_eq!(t, serde_json::json!({"head": {}, "boolean": true}));
    assert_eq!(json(&Answer::Triples(Triples::default())), None);
}

#[test]
fn csv_quotes_and_uses_crlf() {
    assert_eq!(
        csv(&sample()),
        "s,v\r\n\
         http://example.com/a,x|y\r\n\
         http://example.com/bee,7\r\n\
         _:g1,\r\n\
         http://example.com/c,\"say \"\"hi\"\",\nok\"\r\n"
    );
}

#[test]
fn tsv_writes_terms_in_full() {
    let mut s = sample();
    s.rows
        .push(vec![Some(lit("a\tb")), Some(typed("x", "http://e/dt"))]);
    assert_eq!(
        tsv(&s),
        "?s\t?v\n\
         <http://example.com/a>\t\"x|y\"\n\
         <http://example.com/bee>\t7\n\
         _:g1\t\n\
         <http://example.com/c>\t\"say \\\"hi\\\",\\nok\"@en\n\
         \"a\\tb\"\t\"x\"^^<http://e/dt>\n"
    );
}

#[test]
fn turtle_groups_by_subject_with_used_prefixes() {
    let d = people();
    let Ok(Answer::Triples(ts)) = run_on(
        &[&d],
        "CONSTRUCT { ?p ex:label ?n ; ex:age ?a } WHERE { ?p ex:name ?n ; ex:age ?a FILTER(?a > 26) }",
    ) else {
        panic!()
    };
    let mut p = ex_prefixes();
    p.insert("unused".into(), "http://unused/".into());
    let t = turtle(&ts, &p);
    assert!(
        t.starts_with("@prefix ex: <http://example.com/> .\n\n"),
        "{t}"
    );
    assert!(!t.contains("unused"));
    // One statement per subject, a predicate per line.
    assert_eq!(t.matches(" .\n").count(), 3, "{t}");
    assert!(t.contains("ex:alice ex:"), "{t}");
    assert!(t.contains(" ;\n    ex:"), "{t}");
    assert!(t.contains("\"Carol\"@en"), "{t}");
    // Reads back as the same triples.
    let back = crate::syntax::parse(&t, &PrefixMap::default(), crate::syntax::Syntax::Turtle);
    assert!(back.errors.is_empty());
    assert_eq!(back.triples.len(), 4);
}

#[test]
fn turtle_writes_iris_no_prefixed_name_can_carry_in_full() {
    let t = Triple::new(
        NamedNode::new_unchecked(format!("{EX}-x")),
        NamedNode::new_unchecked(format!("{EX}p")),
        typed("1.5", "http://www.w3.org/2001/XMLSchema#decimal"),
    );
    let out = turtle(&[t], &ex_prefixes());
    assert_eq!(
        out,
        "@prefix ex: <http://example.com/> .\n\n<http://example.com/-x> ex:p 1.5 .\n"
    );
}

#[test]
fn turtle_writes_rdf_type_as_a() {
    let t = Triple::new(
        NamedNode::new_unchecked(format!("{EX}alice")),
        NamedNode::new_unchecked(crate::syntax::RDF_TYPE),
        NamedNode::new_unchecked(format!("{EX}Person")),
    );
    let out = turtle(&[t], &ex_prefixes());
    assert_eq!(
        out,
        "@prefix ex: <http://example.com/> .\n\nex:alice a ex:Person .\n"
    );
}

/// The fallback (the diagnostics missed an error) gives the place in the text
/// as written, not in the copy the parser sees (`ex:a\.b\.c`, two bytes
/// longer), and never cuts a character.
#[test]
fn a_fallback_syntax_error_is_placed_in_the_written_text() {
    let text = "SELECT * WHERE { ?s ex:a.b.c ?o . ?s ?p }";
    let doc = Document::parse(text, &ex_prefixes(), None);
    let parser = SparqlParser::new().with_prefix("ex", EX).unwrap();
    let QueryError::Syntax { line, column, .. } = located(text, doc.tokens(), parser) else {
        unreachable!()
    };
    assert_eq!((line, column), (1, text.rfind('}').unwrap() as u32 + 1));
    let QueryError::Syntax { offset, column, .. } = syntax_error("\"é\"", String::new(), 2) else {
        unreachable!()
    };
    assert_eq!((offset, column), (1, 2));
}

/// Each place the evaluator keeps rows counts them against
/// `max_buffered_rows`: over it the query stops, under it or without it the
/// answer is the same.
#[test]
fn a_query_that_keeps_too_many_rows_stops() {
    let d = chain(100);
    let mut failed = Vec::new();
    for q in [
        "SELECT ?s WHERE { ?s ex:next ?o } ORDER BY ?s",
        "SELECT DISTINCT ?s WHERE { ?s ex:next ?o }",
        "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ex:next ?o } GROUP BY ?s",
        "SELECT (COUNT(*) AS ?n) WHERE { ?a ex:next ?b . ?c ex:next ?d }",
        "SELECT ?a WHERE { ?a ex:next ?b MINUS { ?c ex:next ?d } }",
        "SELECT ?a WHERE { ?a ex:next ?b MINUS { ?b ex:next ?c } }",
        "SELECT (COUNT(*) AS ?n) WHERE { { SELECT ?a ?x WHERE { ?a ex:next* ?x } LIMIT 1000000 } { SELECT ?b ?x WHERE { ?b ex:next* ?x } LIMIT 1000000 } }",
        "SELECT (COUNT(*) AS ?n) WHERE { { SELECT ?a ?x WHERE { ?a ex:next* ?x } LIMIT 1000000 } OPTIONAL { SELECT ?b ?x WHERE { ?b ex:next* ?x } LIMIT 1000000 } }",
        "CONSTRUCT { ?s ex:q ?o } WHERE { ?s ex:next ?o }",
    ] {
        let with = |n: Option<usize>| {
            let limits = Limits {
                max_rows: Some(1),
                max_buffered_rows: n,
                ..Limits::default()
            };
            run(&[&d], q, None, &ex_prefixes(), &limits)
        };
        let all = with(None);
        if with(Some(10)) != Err(QueryError::TooManyRows(10))
            || all.is_err()
            || with(Some(10_000_000)) != all
        {
            failed.push(q);
        }
    }
    assert_eq!(failed, Vec::<&str>::new());
}

/// MINUS drops the errors of its right side: a stop there is not an error of
/// the evaluation, and the left side, reading no data, never sees it.
#[test]
fn a_stop_whose_error_is_dropped_is_not_a_partial_answer() {
    let d = chain(100);
    let limits = Limits {
        max_buffered_rows: Some(10),
        ..Limits::default()
    };
    assert_eq!(
        run(
            &[&d],
            "SELECT ?x WHERE { BIND (1 AS ?x) MINUS { ?a ex:next ?b } }",
            None,
            &ex_prefixes(),
            &limits
        ),
        Err(QueryError::TooManyRows(10))
    );
}

/// `n` subjects of two triples each, for a cross product that takes seconds.
fn wide(n: usize) -> Arc<Analysis> {
    let mut text = HEAD.to_string();
    for i in 0..n {
        text.push_str(&format!("ex:s{i} ex:p ex:o{i} ; ex:q ex:r{i} .\n"));
    }
    doc("file:///w/wide.ttl", &text)
}

/// One query per form, each a MINUS whose right side is an in-memory cross
/// product (36 million rows over `wide(3000)`) with a one-row left side. Cut
/// short, the right side ends early and the left side survives: a wrong
/// answer in every form, unless the stop is reported.
const MINUS_CROSS: [&str; 3] = [
    "ASK { VALUES ?d { ex:s2999 } MINUS { ?a ?b ?c . ?d ?e ?f } }",
    "SELECT ?d WHERE { VALUES ?d { ex:s2999 } MINUS { ?a ?b ?c . ?d ?e ?f } }",
    "CONSTRUCT { ?d ex:seen ex:yes } WHERE { VALUES ?d { ex:s2999 } MINUS { ?a ?b ?c . ?d ?e ?f } }",
];

#[test]
fn a_minus_cut_short_by_the_time_limit_is_an_error_in_every_form() {
    let d = wide(3000);
    let limits = Limits {
        timeout: Some(Duration::from_millis(100)),
        ..Limits::default()
    };
    for q in MINUS_CROSS {
        assert_eq!(
            run(&[&d], q, None, &ex_prefixes(), &limits),
            Err(QueryError::Timeout),
            "{q}"
        );
    }
}

#[test]
fn a_minus_cut_short_by_a_cancel_flag_is_an_error_in_every_form() {
    let d = wide(3000);
    for q in MINUS_CROSS {
        assert_eq!(
            cancelled_after(&d, q, 1_000),
            Err(QueryError::Cancelled),
            "{q}"
        );
    }
}

// --- long cells --------------------------------------------------------------

/// One row with a literal of `n` characters (`é`, two bytes each).
fn one_long_cell(n: usize) -> Solutions {
    Solutions {
        variables: vec!["v".into()],
        rows: vec![
            vec![Some(Literal::new_simple_literal("é".repeat(n)).into())],
            vec![Some(iri("short"))],
        ],
        omitted: 0,
    }
}

#[test]
fn the_table_and_markdown_cut_a_long_cell_and_say_so() {
    let s = one_long_cell(render::MAX_CELL_CHARS + 500);
    let t = table(&s, &ex_prefixes(), None);
    // The cut text (its opening quote included) and the mark, no more.
    let cell = t.lines().nth(2).unwrap();
    assert_eq!(cell.chars().count(), render::MAX_CELL_CHARS + 1, "{t}");
    assert!(cell.ends_with('…'), "{t}");
    assert!(t.contains("ex:short"), "{t}");
    assert!(
        t.ends_with("1 cell shortened to 1000 characters\n"),
        "{}",
        &t[t.len() - 60..]
    );

    let m = markdown(&Answer::Solutions(s), &ex_prefixes(), None);
    let cell = m.lines().find(|l| l.contains('é')).unwrap();
    // "| `", the cut text, "…", "` |"
    assert_eq!(
        cell.chars().count(),
        3 + render::MAX_CELL_CHARS + 1 + 3,
        "{cell}"
    );
    assert!(
        m.ends_with("\n_1 cell shortened to 1000 characters._\n"),
        "{m}"
    );
}

#[test]
fn a_cell_at_the_cap_is_whole_and_unremarked() {
    // The cell is the quote, the text and the quote.
    let s = one_long_cell(render::MAX_CELL_CHARS - 2);
    let t = table(&s, &ex_prefixes(), None);
    assert!(!t.contains('…') && !t.contains("shortened"), "{t}");
    let m = markdown(&Answer::Solutions(s), &ex_prefixes(), None);
    assert!(!m.contains('…') && !m.contains("shortened"), "{m}");
}

#[test]
fn the_row_note_and_the_cell_note_both_show() {
    let mut s = one_long_cell(render::MAX_CELL_CHARS + 1);
    s.omitted = 4;
    let t = table(&s, &ex_prefixes(), Some(1));
    assert!(
        t.ends_with("showing 1 of 6 rows\n1 cell shortened to 1000 characters\n"),
        "{}",
        &t[t.len() - 80..]
    );
}

// --- planning limits ---------------------------------------------------------

/// `?s ex:p/ex:p/... ?o` with `n` steps.
fn path_query(n: usize) -> String {
    format!("ASK {{ ?s {} ?o }}", vec!["ex:p"; n].join("/"))
}

fn complexity(q: &str) -> Result<(), QueryError> {
    check_complexity(&parse(q, None, &ex_prefixes()).unwrap())
}

#[test]
fn a_long_path_is_refused_at_once() {
    // Planning is about cubic in the steps: 22 s for 1 000 of them, with no
    // clock check on the way. Refused before the evaluation starts: a cancel
    // flag set beforehand, which the first check would report, is never seen.
    let d = chain(3);
    let limits = Limits {
        cancel: Some(Arc::new(AtomicBool::new(true))),
        ..Limits::default()
    };
    let r = run(&[&d], &path_query(1000), None, &ex_prefixes(), &limits);
    assert!(matches!(r, Err(QueryError::TooComplex(_))), "{r:?}");
}

#[test]
fn the_widest_join_is_counted_where_it_is() {
    let n = MAX_JOIN_WIDTH;
    let body = |n: usize| {
        let ps: Vec<String> = (0..n).map(|i| format!("?v{i} ex:p ?v{}", i + 1)).collect();
        ps.join(" . ")
    };
    let bgp = |n: usize| format!("ASK {{ {} }}", body(n));
    let groups = |n: usize| {
        let gs: Vec<String> = (0..n).map(|i| format!("{{ ?v{i} ex:p ?w{i} }}")).collect();
        format!("ASK {{ {} }}", gs.join(" "))
    };
    let exists = |n: usize| format!("ASK {{ FILTER EXISTS {{ {} }} }}", body(n));
    let optional = |n: usize| format!("SELECT * {{ OPTIONAL {{ {} }} }}", body(n));
    for (what, at, over) in [
        ("path steps", path_query(n), path_query(n + 1)),
        ("a basic graph pattern", bgp(n), bgp(n + 1)),
        ("joined groups", groups(n), groups(n + 1)),
        ("an EXISTS", exists(n), exists(n + 1)),
        ("an OPTIONAL", optional(n), optional(n + 1)),
    ] {
        assert_eq!(complexity(&at), Ok(()), "{what} at the limit");
        assert!(
            matches!(complexity(&over), Err(QueryError::TooComplex(_))),
            "{what} over the limit"
        );
    }
}

/// The slowest shapes to plan, at the widest join allowed: every pattern
/// shares `?s`, which sparopt plans in about quartic time with no clock
/// check. They took 0.06 and 0.1 s on the release build; at 200 patterns,
/// 3.9 and 6.8 s.
#[test]
fn timing_a_star_join_at_the_limit_plans_quickly() {
    let n = MAX_JOIN_WIDTH;
    let star: Vec<String> = (0..n).map(|i| format!("?s ex:p{i} ?o{i}")).collect();
    let star = format!("ASK {{ {} }}", star.join(" . "));
    let reifiers = format!("ASK {{ ?s ?p ?o {}}}", "~ ".repeat(n - 1));
    let d = chain(3);
    for q in [star, reifiers] {
        let start = std::time::Instant::now();
        let r = run(&[&d], &q, None, &ex_prefixes(), &Limits::default());
        assert!(r.is_ok(), "{r:?}");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
    }
}

#[test]
fn unions_alternatives_and_optionals_plan_in_linear_time_and_are_not_counted() {
    let many = 1000;
    let union = (0..many)
        .map(|i| format!("{{ ?s ex:p{i} ?o }}"))
        .collect::<Vec<_>>()
        .join(" UNION ");
    let optional: String = (0..many)
        .map(|i| format!("OPTIONAL {{ ?s ex:p{i} ?o{i} }}"))
        .collect();
    let alt = vec!["ex:p"; many].join("|");
    for q in [
        format!("ASK {{ {union} }}"),
        format!("SELECT * {{ ?s ex:q ?x {optional} }}"),
        format!("ASK {{ ?s {alt} ?o }}"),
    ] {
        assert_eq!(complexity(&q), Ok(()), "{}", &q[..40]);
    }
}

/// The rows SELECT and the triples CONSTRUCT keep count too, also with no
/// row cap.
#[test]
fn results_without_a_row_cap_keep_too_many_rows() {
    let d = chain(100);
    let limits = Limits {
        max_buffered_rows: Some(10),
        ..Limits::default()
    };
    for q in [
        "SELECT ?s WHERE { ?s ex:next ?o }",
        "CONSTRUCT { ?s ex:q ?o } WHERE { ?s ex:next ?o }",
    ] {
        let r = run(&[&d], q, None, &ex_prefixes(), &limits);
        assert_eq!(r, Err(QueryError::TooManyRows(10)), "{q}");
    }
}

// --- inferred triples --------------------------------------------------------

const OWL_HEAD: &str = "@prefix ex: <http://example.com/> .\n\
                        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n";

fn owl(uri: &str, body: &str) -> Arc<Analysis> {
    doc(uri, &format!("{OWL_HEAD}{body}"))
}

fn inference(rules: Reasoning) -> Inference {
    Inference {
        rules,
        imports: Vec::new(),
        max_triples: 1000,
    }
}

/// `q` over `docs` with `inference`.
fn reasoned(docs: &[&Arc<Analysis>], q: &str, inference: &Inference) -> Outcome {
    super::run(docs, q, None, &ex_prefixes(), &Limits::default(), inference).unwrap()
}

/// The rows of SELECT `q` over `docs` with the default rules.
fn inferred_rows(docs: &[&Arc<Analysis>], q: &str) -> Vec<Vec<String>> {
    match reasoned(docs, q, &inference(Reasoning::Default)).answer {
        Answer::Solutions(s) => shown(&s),
        other => panic!("not solutions: {other:?}"),
    }
}

#[test]
fn each_kind_of_inference_matches_with_the_default_rules() {
    let cases = [
        (
            "a class through a range",
            "ex:worksFor rdfs:range ex:Organisation .\nex:bob ex:worksFor ex:alice .\n",
            "SELECT ?x WHERE { ?x a ex:Organisation }",
            "ex:alice",
        ),
        (
            "a subclass chain",
            "ex:Puppy rdfs:subClassOf ex:Dog .\nex:Dog rdfs:subClassOf ex:Animal .\nex:rex a ex:Puppy .\n",
            "SELECT ?x WHERE { ?x a ex:Animal }",
            "ex:rex",
        ),
        (
            "an inverse",
            "ex:parentOf owl:inverseOf ex:childOf .\nex:ann ex:parentOf ex:ben .\n",
            "SELECT ?x WHERE { ex:ben ex:childOf ?x }",
            "ex:ann",
        ),
        (
            "a transitive property",
            "ex:partOf a owl:TransitiveProperty .\nex:a ex:partOf ex:b .\nex:b ex:partOf ex:c .\n",
            "SELECT ?x WHERE { ex:a ex:partOf ?x FILTER (?x != ex:b) }",
            "ex:c",
        ),
        (
            "a property chain",
            "ex:uncleOf owl:propertyChainAxiom ( ex:brotherOf ex:parentOf ) .\n\
             ex:carl ex:brotherOf ex:ann .\nex:ann ex:parentOf ex:ben .\n",
            "SELECT ?x WHERE { ?x ex:uncleOf ex:ben }",
            "ex:carl",
        ),
        (
            "an owl:sameAs merge",
            "ex:bob owl:sameAs ex:robert .\nex:bob ex:name \"Bob\" .\n",
            "SELECT ?n WHERE { ex:robert ex:name ?n }",
            "\"Bob\"",
        ),
    ];
    for (what, body, q, want) in cases {
        let d = owl("file:///w/model.ttl", body);
        let off = select(&[&d], q);
        assert!(off.rows.is_empty(), "{what}: off already matches");
        let rows = inferred_rows(&[&d], q);
        let got: Vec<String> = rows.into_iter().flatten().collect();
        assert_eq!(got.join(" "), want, "{what}");
    }
}

#[test]
fn the_note_says_how_many_triples_were_added_with_which_rules() {
    let d = owl(
        "file:///w/model.ttl",
        "ex:Dog rdfs:subClassOf ex:Animal .\nex:rex a ex:Dog .\n",
    );
    let q = "SELECT ?c WHERE { ex:rex a ?c }";
    for rules in [Reasoning::Default, Reasoning::Complete] {
        let o = reasoned(&[&d], q, &inference(rules));
        let Inferred::Added {
            rules: r,
            triples,
            findings,
        } = o.inferred
        else {
            panic!("{rules:?}: {:?}", o.inferred);
        };
        assert_eq!((r, findings), (rules, 0));
        assert!(triples >= 1, "{rules:?}: {triples}");
        let Answer::Solutions(s) = o.answer else {
            panic!()
        };
        assert!(shown(&s).contains(&vec!["ex:Animal".to_string()]));
    }
}

#[test]
fn off_adds_nothing_and_its_note_is_none() {
    let d = owl(
        "file:///w/model.ttl",
        "ex:worksFor rdfs:range ex:Organisation .\nex:bob ex:worksFor ex:alice .\n",
    );
    let q = "SELECT ?x WHERE { ?x a ex:Organisation }";
    let o = reasoned(&[&d], q, &Inference::default());
    assert_eq!(o.inferred, Inferred::None);
    assert_eq!(o.answer, run_on(&[&d], q).unwrap());
    let Answer::Solutions(s) = o.answer else {
        panic!()
    };
    assert!(s.rows.is_empty());
}

#[test]
fn an_inferred_triple_joins_a_written_one_through_a_blank_node() {
    // Given out of URI order, each with blank nodes: the reasoning base
    // reads them sorted, the query graph in the order given.
    let z = owl(
        "file:///w/z.ttl",
        "ex:zed ex:worksFor _:x .\n_:x ex:name \"Zeta\" .\n_:y ex:name \"unused\" .\n",
    );
    let a = owl(
        "file:///w/a.ttl",
        "ex:worksFor rdfs:range ex:Organisation .\nex:alice ex:worksFor [ ex:name \"Acme\" ] .\n",
    );
    let q =
        "SELECT ?p ?n WHERE { ?p ex:worksFor ?o . ?o a ex:Organisation ; ex:name ?n } ORDER BY ?n";
    let want = [["ex:alice", "\"Acme\""], ["ex:zed", "\"Zeta\""]];
    assert_eq!(inferred_rows(&[&z, &a], q), want);
    assert_eq!(inferred_rows(&[&a, &z], q), want);
}

#[test]
fn an_imported_axiom_infers_but_the_imports_own_triples_are_not_queried() {
    let import = owl(
        "file:///cache/animals.ttl",
        "ex:Dog rdfs:subClassOf ex:Animal .\nex:Dog ex:label \"dog\" .\n_:r ex:about ex:Dog .\n",
    );
    let data = owl(
        "file:///w/data.ttl",
        "_:d ex:about ex:rex .\nex:rex a ex:Dog .\n",
    );
    let with = Inference {
        imports: vec![import],
        ..inference(Reasoning::Default)
    };
    let select = |q: &str| match reasoned(&[&data], q, &with).answer {
        Answer::Solutions(s) => shown(&s),
        other => panic!("{other:?}"),
    };
    assert!(select("SELECT ?c WHERE { ex:rex a ?c }").contains(&vec!["ex:Animal".to_string()]));
    let ask = |q: &str| reasoned(&[&data], q, &with).answer;
    assert_eq!(
        ask("ASK { ex:Dog <http://www.w3.org/2000/01/rdf-schema#subClassOf> ex:Animal }"),
        Answer::Boolean(false)
    );
    assert_eq!(ask("ASK { ex:Dog ex:label ?l }"), Answer::Boolean(false));
    // The import's blank node is not one of the data's.
    assert_eq!(ask("ASK { ?b ex:about ex:Dog }"), Answer::Boolean(false));
    assert_eq!(
        select("SELECT ?b WHERE { ?b ex:about ?x }").len(),
        1,
        "the data's one blank node"
    );
}

#[test]
fn construct_and_ask_answer_over_inferred_triples() {
    let d = owl(
        "file:///w/model.ttl",
        "ex:Dog rdfs:subClassOf ex:Animal .\nex:rex a ex:Dog .\n",
    );
    let on = inference(Reasoning::Default);
    let Answer::Triples(ts) =
        reasoned(&[&d], "CONSTRUCT { ?x a ?c } WHERE { ?x a ?c }", &on).answer
    else {
        panic!()
    };
    let set: FxHashSet<String> = ts.iter().map(|t| t.to_string()).collect();
    let rdf_type = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    assert!(
        set.contains(&format!("<{EX}rex> <{rdf_type}> <{EX}Animal>")),
        "{set:?}"
    );
    let q = "ASK { ex:rex a ex:Animal }";
    assert_eq!(run_on(&[&d], q), Ok(Answer::Boolean(false)));
    assert_eq!(reasoned(&[&d], q, &on).answer, Answer::Boolean(true));
}

#[test]
fn over_the_size_limit_nothing_is_added_and_the_note_says_skipped() {
    let d = owl(
        "file:///w/model.ttl",
        "ex:Dog rdfs:subClassOf ex:Animal .\nex:rex a ex:Dog .\n",
    );
    let import = owl(
        "file:///cache/i.ttl",
        "ex:Cat rdfs:subClassOf ex:Animal .\n",
    );
    let q = "SELECT ?x ?c WHERE { ?x a ?c }";
    let small = Inference {
        imports: vec![import],
        max_triples: 2,
        ..inference(Reasoning::Default)
    };
    let o = reasoned(&[&d], q, &small);
    assert_eq!(
        o.inferred,
        Inferred::Skipped {
            triples: 3,
            limit: 2
        }
    );
    assert_eq!(o.answer, run_on(&[&d], q).unwrap());
}

/// `q` over `d` with the default rules and `limits`, `during` run while
/// the query reasons; and whether a query graph was built.
fn stopped_while_reasoning(
    limits: &Limits,
    during: Box<dyn FnOnce()>,
) -> (Result<Outcome, QueryError>, bool) {
    let d = owl(
        "file:///w/model.ttl",
        "ex:Dog rdfs:subClassOf ex:Animal .\nex:rex a ex:Dog .\n",
    );
    WHILE_REASONING.with(|h| *h.borrow_mut() = Some(during));
    let before = crate::rdf::graph::INDEXED.with(|c| c.get());
    let r = super::run(
        &[&d],
        "SELECT ?c WHERE { ex:rex a ?c }",
        None,
        &ex_prefixes(),
        limits,
        &inference(Reasoning::Default),
    );
    let built = crate::rdf::graph::INDEXED.with(|c| c.get()) != before;
    assert!(WHILE_REASONING.with(|h| h.borrow().is_none()), "reasoned");
    (r, built)
}

#[test]
fn a_timeout_that_passes_while_reasoning_stops_before_evaluation() {
    let limits = Limits {
        timeout: Some(Duration::from_millis(300)),
        ..Limits::default()
    };
    let during = Box::new(|| std::thread::sleep(Duration::from_millis(400)));
    let (r, built) = stopped_while_reasoning(&limits, during);
    assert_eq!(r, Err(QueryError::Timeout));
    assert!(!built, "the query graph was built");
}

#[test]
fn a_cancel_while_reasoning_stops_before_evaluation() {
    let flag = Arc::new(AtomicBool::new(false));
    let limits = Limits {
        cancel: Some(flag.clone()),
        ..Limits::default()
    };
    let during = Box::new(move || flag.store(true, Ordering::Relaxed));
    let (r, built) = stopped_while_reasoning(&limits, during);
    assert_eq!(r, Err(QueryError::Cancelled));
    assert!(!built, "the query graph was built");
}

#[test]
fn a_contradiction_still_answers_and_its_findings_are_counted() {
    let d = owl(
        "file:///w/model.ttl",
        "ex:Cat owl:disjointWith ex:Dog .\nex:Dog rdfs:subClassOf ex:Animal .\nex:tom a ex:Cat , ex:Dog .\n",
    );
    let o = reasoned(
        &[&d],
        "SELECT ?c WHERE { ex:tom a ?c } ORDER BY ?c",
        &inference(Reasoning::Default),
    );
    let Inferred::Added { findings, .. } = o.inferred else {
        panic!("{:?}", o.inferred)
    };
    assert!(findings >= 1, "{findings}");
    let Answer::Solutions(s) = o.answer else {
        panic!()
    };
    assert!(shown(&s).contains(&vec!["ex:Animal".to_string()]), "{s:?}");
}

mod dataset {
    use super::run;
    use crate::analysis::Analysis;
    use crate::query::{Answer, Limits};
    use crate::syntax::PrefixMap;
    use spargebra::term::Term;
    use std::sync::Arc;

    const EX: &str = "http://example.com/";

    fn doc(uri: &str, body: &str) -> Arc<Analysis> {
        let text = format!("@prefix ex: <{EX}> .\n{body}");
        Arc::new(Analysis::new(
            uri.into(),
            &text,
            Arc::new(PrefixMap::default()),
            false,
        ))
    }

    fn rows(docs: &[&Arc<Analysis>], query: &str) -> Vec<Vec<Option<Term>>> {
        let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
        match run(docs, query, None, &prefixes, &Limits::default()) {
            Ok(Answer::Solutions(s)) => s.rows,
            other => panic!("not solutions: {other:?}"),
        }
    }

    fn ask(docs: &[&Arc<Analysis>], query: &str) -> bool {
        let prefixes = PrefixMap::from_iter([("ex".to_string(), EX.to_string())]);
        match run(docs, query, None, &prefixes, &Limits::default()) {
            Ok(Answer::Boolean(b)) => b,
            other => panic!("not a boolean: {other:?}"),
        }
    }

    fn text(t: &Option<Term>) -> String {
        match t {
            Some(Term::Literal(l)) => l.value().to_string(),
            other => panic!("not a literal: {other:?}"),
        }
    }

    #[test]
    fn a_directional_string_has_a_language_and_a_direction() {
        let d = doc(
            "file:///d.ttl",
            "ex:s ex:l \"hi\"@en--rtl , \"hi\"@en , \"salut\"@FR--ltr .\n",
        );
        let r = rows(
            &[&d],
            "SELECT ?v (LANG(?l) AS ?lang) (LANGDIR(?l) AS ?dir) (DATATYPE(?l) AS ?dt) \
             WHERE { ex:s ex:l ?l . BIND(STR(?l) AS ?v) } ORDER BY ?lang ?dir",
        );
        let shown: Vec<_> = r
            .iter()
            .map(|row| {
                let dt = match &row[3] {
                    Some(Term::NamedNode(n)) => n.as_str().rsplit('#').next().unwrap().to_string(),
                    other => panic!("{other:?}"),
                };
                (text(&row[1]), text(&row[2]), dt)
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("en".to_string(), String::new(), "langString".to_string()),
                (
                    "en".to_string(),
                    "rtl".to_string(),
                    "dirLangString".to_string()
                ),
                (
                    "fr".to_string(),
                    "ltr".to_string(),
                    "dirLangString".to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_constant_directional_string_matches_only_its_own_direction() {
        let d = doc("file:///d.ttl", "ex:s ex:l \"hi\"@en--rtl .\n");
        assert!(ask(&[&d], "ASK { ex:s ex:l \"hi\"@en--rtl }"));
        assert!(!ask(&[&d], "ASK { ex:s ex:l \"hi\"@en--ltr }"));
        assert!(!ask(&[&d], "ASK { ex:s ex:l \"hi\"@en }"));
    }

    const JOIN: &str = "SELECT ?t ?v WHERE { ?t ex:r ?tt . BIND(SUBJECT(?tt) AS ?x) ?x ex:p ?v } \
                        ORDER BY ?t";

    #[test]
    fn a_blank_node_in_a_triple_term_is_the_same_node_outside_it() {
        let d = doc(
            "file:///d.ttl",
            "ex:t ex:r <<( _:x ex:q 1 )>> .\n_:x ex:p 2 .\n",
        );
        let r = rows(&[&d], JOIN);
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(text(&r[0][1]), "2");
    }

    #[test]
    fn the_same_blank_label_in_two_documents_stays_two_nodes_in_triple_terms() {
        let a = doc(
            "file:///a.ttl",
            "ex:t ex:r <<( _:x ex:q 1 )>> .\n_:x ex:p 2 .\n",
        );
        let b = doc(
            "file:///b.ttl",
            "ex:u ex:r <<( _:x ex:q 1 )>> .\n_:x ex:p 3 .\n",
        );
        let r = rows(&[&a, &b], JOIN);
        let shown: Vec<_> = r
            .iter()
            .map(|row| match &row[0] {
                Some(Term::NamedNode(n)) => (
                    n.as_str().rsplit('/').next().unwrap().to_string(),
                    text(&row[1]),
                ),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("t".to_string(), "2".to_string()),
                ("u".to_string(), "3".to_string())
            ]
        );
    }

    #[test]
    fn a_relative_iri_is_the_absolute_one_in_query_results() {
        let a = doc("file:///w/a.ttl", "<#me> ex:p 1 .\n");
        let b = doc("file:///w/b.ttl", "<#me> ex:p 2 .\n");
        let r = rows(&[&a, &b], "SELECT ?s ?v WHERE { ?s ex:p ?v } ORDER BY ?v");
        let subjects: Vec<_> = r
            .iter()
            .map(|row| match &row[0] {
                Some(Term::NamedNode(n)) => n.as_str().to_string(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(subjects, ["file:///w/a.ttl#me", "file:///w/b.ttl#me"]);
    }

    #[test]
    fn triples_with_an_undeclared_prefix_are_not_in_the_graph() {
        let d = Arc::new(Analysis::new(
            "file:///d.ttl".into(),
            "@prefix ex: <http://example.com/> .\nex:a ex:p nope:o , ex:o .\nnope:s ex:p ex:o .\n",
            Arc::new(PrefixMap::default()),
            false,
        ));
        assert_eq!(d.parsed.triples.len(), 3);
        let r = rows(&[&d], "SELECT ?s ?o WHERE { ?s ex:p ?o }");
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(!ask(
            &[&d],
            "ASK { ?s ?p ?o FILTER (STRSTARTS(STR(?o), \"nope:\")) }"
        ));
    }
}

/// A model whose reasoning takes many rounds: a long subclass chain with
/// many members, each round moving the types one class further.
fn long_chain() -> Arc<Analysis> {
    let mut body = String::new();
    for i in 0..700 {
        body += &format!("ex:C{i} rdfs:subClassOf ex:C{} .\n", i + 1);
    }
    for j in 0..100 {
        body += &format!("ex:m{j} a ex:C0 .\n");
    }
    owl("file:///w/chain.ttl", &body)
}

#[test]
fn timing_a_timeout_or_a_cancel_stops_reasoning_partway_through() {
    let d = long_chain();
    let q = "ASK { ex:m0 a ex:C700 }";
    let mut big = inference(Reasoning::Default);
    big.max_triples = 1_000_000;
    let run = |limits: &Limits| {
        let start = Instant::now();
        let r = super::run(&[&d], q, None, &ex_prefixes(), limits, &big);
        (r, start.elapsed())
    };
    let (full, whole) = run(&Limits::default());
    assert_eq!(full.unwrap().answer, Answer::Boolean(true));
    assert!(
        whole >= Duration::from_millis(200),
        "too quick to measure: {whole:?}"
    );
    let (r, took) = run(&Limits {
        timeout: Some(whole / 10),
        ..Limits::default()
    });
    assert_eq!(r, Err(QueryError::Timeout));
    assert!(
        took < whole / 2,
        "the timeout stopped it after {took:?} of a {whole:?} run"
    );
    let flag = Arc::new(AtomicBool::new(false));
    let trip = {
        let flag = flag.clone();
        let after = whole / 10;
        std::thread::spawn(move || {
            std::thread::sleep(after);
            flag.store(true, Ordering::Relaxed);
        })
    };
    let (r, took) = run(&Limits {
        cancel: Some(flag),
        ..Limits::default()
    });
    trip.join().unwrap();
    assert_eq!(r, Err(QueryError::Cancelled));
    assert!(
        took < whole / 2,
        "the cancel stopped it after {took:?} of a {whole:?} run"
    );
}
