use super::render::{csv, json, markdown, table, tsv, turtle};
use super::*;
use spargebra::term::{BlankNode, Literal, NamedNode};
use std::sync::Arc;

const EX: &str = "http://example.com/";
const HEAD: &str = "@prefix ex: <http://example.com/> .\n";

fn doc(uri: &str, text: &str) -> Analysis {
    Analysis::new(uri.into(), text, Arc::new(PrefixMap::default()), false)
}

fn ex_prefixes() -> PrefixMap {
    PrefixMap::from_iter([("ex".to_string(), EX.to_string())])
}

fn people() -> Analysis {
    doc(
        "file:///w/people.ttl",
        &format!(
            "{HEAD}ex:alice ex:name \"Alice\" ; ex:age 30 ; ex:knows ex:bob .\n\
             ex:bob ex:name \"Bob\" ; ex:age 25 .\n\
             ex:carol ex:name \"Carol\"@en ; ex:age 41 ; ex:knows ex:alice, ex:bob .\n"
        ),
    )
}

fn run_on(docs: &[&Analysis], q: &str) -> Result<Answer, QueryError> {
    run(docs, q, None, &ex_prefixes(), &Limits::default())
}

fn select(docs: &[&Analysis], q: &str) -> Solutions {
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

fn big() -> Analysis {
    let mut text = HEAD.to_string();
    for i in 0..150 {
        text.push_str(&format!("ex:s{i} ex:p {i} .\n"));
    }
    doc("file:///w/big.ttl", &text)
}

#[test]
fn a_runaway_query_stops_at_the_time_limit() {
    let d = big();
    let start = std::time::Instant::now();
    let r = run(
        &[&d],
        RUNAWAY,
        None,
        &ex_prefixes(),
        &Limits {
            timeout: Some(Duration::from_millis(20)),
            cancel: None,
        },
    );
    assert_eq!(r, Err(QueryError::Timeout));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn a_cancelled_query_stops() {
    let d = big();
    let flag = AtomicBool::new(false);
    let start = std::time::Instant::now();
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            flag.store(true, Ordering::Relaxed);
        });
        run(
            &[&d],
            RUNAWAY,
            None,
            &ex_prefixes(),
            &Limits {
                timeout: None,
                cancel: Some(&flag),
            },
        )
    });
    assert_eq!(r, Err(QueryError::Cancelled));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
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
        &Answer::Triples(vec![t.clone(), t]),
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
    assert_eq!(json(&Answer::Triples(vec![])), None);
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
