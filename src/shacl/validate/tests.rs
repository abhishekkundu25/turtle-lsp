use super::*;
use crate::syntax::{PrefixMap, Syntax, parse};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// The time since the first call, for the SPARQL time limit.
fn now() -> Duration {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed()
}

fn results(body: &str) -> usize {
    let p = parse(
        &format!(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n{body}"
        ),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    let g = Graph::new(&[&p.triples]);
    validate_report(&g, &|| false, None, |_| true, now)
        .unwrap()
        .results
        .len()
}

/// Finding the targets among a few nodes is the same as finding them in
/// the whole graph and keeping those nodes: for class targets (with
/// subclasses), the two `sh:targetSubjectsOf`/`ObjectsOf` forms and a
/// node target, with the graph read as an overlay over a shared base.
#[test]
fn targets_among_some_nodes_are_those_of_the_whole_graph_that_are_among_them() {
    let head = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";
    let mut shared = format!(
        "{head}ex:S1 sh:targetClass ex:C ; sh:property [ sh:path ex:name ; sh:minCount 1 ] .\n\
             ex:S2 sh:targetSubjectsOf ex:knows ; sh:property [ sh:path ex:age ; sh:minCount 1 ] .\n\
             ex:S3 sh:targetObjectsOf ex:knows ; sh:property [ sh:path ex:age ; sh:minCount 1 ] .\n\
             ex:S4 sh:targetNode ex:n0 , ex:n1 ; sh:property [ sh:path ex:id ; sh:minCount 1 ] .\n\
             ex:D rdfs:subClassOf ex:C .\n"
    );
    for i in 0..200 {
        let class = if i % 3 == 0 { "ex:D" } else { "ex:C" };
        shared.push_str(&format!(
            "ex:n{i} a {class} ; ex:knows ex:n{} .\n",
            (i + 1) % 200
        ));
    }
    let data = format!(
        "{head}ex:n3 ex:note 1 .\nex:n4 ex:knows ex:new .\nex:n6 a ex:C .\nex:fresh a ex:D ; ex:knows ex:n9 .\nex:other ex:p ex:n0 .\n"
    );
    let shared = parse(&shared, &PrefixMap::default(), Syntax::Turtle);
    let data = parse(&data, &PrefixMap::default(), Syntax::Turtle);
    let base = Arc::new(Graph::new(&[&shared.triples]));
    let over = Graph::overlay(&base, &data.triples);
    let mut mine: Vec<Id> = over.added().flat_map(|(_, t)| [t.s, t.o]).collect();
    mine.sort_unstable();
    mine.dedup();
    let describe = |g: &Graph, r: &Report| {
        let mut v: Vec<String> = r
            .results
            .iter()
            .map(|r| {
                format!(
                    "{:?} {:?} {:?} {}",
                    g.term(r.focus),
                    r.path,
                    r.value.map(|v| g.term(v)),
                    r.component
                )
            })
            .collect();
        v.sort();
        v
    };
    let keep = |id: Id| mine.binary_search(&id).is_ok();
    let whole = report_over(
        &over,
        Arc::new(|| false),
        &keep,
        None,
        MIN_BATCH,
        Budget::default(),
        |_| true,
        now,
    )
    .unwrap();
    let among = validate_report(&over, &|| false, Some(&mine), |_| true, now).unwrap();
    assert!(!whole.results.is_empty());
    assert_eq!(describe(&over, &whole), describe(&over, &among));
    assert_eq!(among.failures.len(), whole.failures.len());
    // More than the dataset's nodes: the shared ones are all of them.
    let all = validate_report(&over, &|| false, None, |_| true, now).unwrap();
    assert!(all.results.len() > among.results.len());
}

#[test]
fn owl_class_is_an_implicit_target() {
    assert_eq!(
        results(
            "ex:C a owl:Class , sh:NodeShape ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\nex:a a ex:C ."
        ),
        1
    );
}

#[test]
fn pattern_flags() {
    let t = |pattern: &str, flags: &str, value: &str| {
        results(&format!(
            "ex:S sh:targetNode {value} ; sh:pattern {pattern:?} ; sh:flags {flags:?} ."
        ))
    };
    assert_eq!(t("^a b$", "x", "\"ab\""), 0);
    assert_eq!(t("[ ]", "x", "\"a b\""), 0);
    assert_eq!(t("a.c", "q", "\"abc\""), 1);
    assert_eq!(t("a.c", "q", "\"a.c\""), 0);
    assert_eq!(t("^ABC$", "i", "\"abc\""), 0);
    // A pattern that does not compile is ignored.
    assert_eq!(t("(", "", "\"x\""), 0);
}

#[test]
fn recursion_and_cancellation() {
    let body = "ex:S sh:targetNode ex:a ; sh:node ex:S ; sh:property [ sh:path ex:p ; sh:node ex:S ] .\nex:a ex:p ex:a .";
    assert_eq!(results(body), 0);
    let p = parse(
        &format!(
            "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n{body}"
        ),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    let g = Graph::new(&[&p.triples]);
    assert!(validate_report(&g, &|| true, None, |_| true, now).is_none());
}

const HEAD: &str =
    "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://example.com/> .\n";

/// The roots as the validator found them before it read the shape
/// model: the subjects of each target predicate, then of each shape
/// type, as the graph's lookups list them.
fn roots_by_lookup(g: &Graph) -> Vec<Id> {
    let v = vocab::get();
    let mut roots = Vec::new();
    for k in root_kinds() {
        if k == v.node_shape || k == v.property_shape {
            roots.extend(g.iri(k).into_iter().flat_map(|c| g.subjects(v.rdf_type, c)));
        } else {
            roots.extend(g.with_p(k).map(|t| t.s));
        }
    }
    dedup(&mut roots);
    roots
}

/// Validation visits the shapes in the order it did when it looked
/// them up in the graph: by target kind, then by the index order of
/// the triples, an overlay's own before the base's.
#[test]
fn roots_come_in_the_order_the_graph_lists_them() {
    let doc = |body: &str| {
        parse(
            &format!("{HEAD}{body}"),
            &PrefixMap::default(),
            Syntax::Turtle,
        )
    };
    let base = doc("ex:B a sh:PropertyShape ; sh:path ex:p .\n\
             ex:A sh:targetNode ex:x .\n\
             ex:C a sh:NodeShape ; sh:targetClass ex:K .\n\
             ex:D sh:target [ sh:select \"SELECT ?this {}\" ] ; sh:targetNode ex:y .\n\
             ex:A sh:targetClass ex:L .\n\
             ex:E sh:targetObjectsOf ex:p ; a sh:NodeShape .\n\
             ex:H a sh:targetClass , sh:PropertyShape .\n");
    let over = doc("ex:F sh:targetNode ex:z .\n\
             ex:A sh:targetSubjectsOf ex:q .\n\
             ex:G a sh:NodeShape .\n\
             ex:B sh:targetNode ex:w .\n");
    let order = |g: &Graph| {
        let model = Shapes::new(g);
        let found: Vec<Id> = roots(g, &model).iter().map(|s| s.node).collect();
        assert_eq!(found, roots_by_lookup(g));
        found.len()
    };
    assert_eq!(order(&Graph::new(&[&base.triples])), 6);
    assert_eq!(order(&Graph::new(&[&base.triples, &over.triples])), 8);
    let shared = Arc::new(Graph::new(&[&base.triples]));
    assert_eq!(order(&Graph::overlay(&shared, &over.triples)), 8);
}

fn report(
    body: &str,
    cancelled: impl Fn() -> bool + Send + Sync + Clone + 'static,
) -> Option<(usize, Vec<String>, usize)> {
    let p = parse(
        &format!("{HEAD}{body}"),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let g = Graph::new(&[&p.triples]);
    let r = validate_report(&g, &cancelled, None, |_| true, now)?;
    let messages = r
        .results
        .iter()
        .flat_map(|x| x.computed_messages.iter().map(|m| m.0.clone()))
        .collect();
    Some((r.results.len(), messages, r.failures.len()))
}

const RULE: &str = "ex:S sh:targetClass ex:C ; sh:sparql [ sh:message \"{$this} has {?value}\" ; sh:prefixes ex:P ; sh:select \"SELECT $this ?value WHERE { $this ex:p ?value . FILTER (?value > 1) }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n";

#[test]
fn sparql_constraint_with_message_template() {
    let (n, messages, failures) = report(
        &format!("{RULE}ex:a a ex:C ; ex:p 1 , 2 .\nex:b a ex:C ; ex:p 3 ."),
        &|| false,
    )
    .unwrap();
    assert_eq!((n, failures), (2, 0));
    assert!(
        messages.contains(&"http://example.com/a has 2".to_string()),
        "{messages:?}"
    );
}

#[test]
fn deactivated_sparql_constraint() {
    let body = "ex:S sh:targetNode ex:a ; sh:sparql [ sh:deactivated true ; sh:select \"SELECT $this WHERE { }\" ] .";
    assert_eq!(report(body, || false).unwrap().0, 0);
}

#[test]
fn sparql_target() {
    let body = "ex:S sh:target [ a sh:SPARQLTarget ; sh:select \"SELECT ?this WHERE { ?this <http://example.com/p> 1 }\" ] ; sh:sparql [ sh:select \"SELECT $this WHERE { }\" ] .\nex:a ex:p 1 .\nex:b ex:p 2 .";
    assert_eq!(report(body, || false).unwrap().0, 1);
}

#[test]
fn malformed_and_forbidden_queries_are_failures() {
    for q in [
        "SELECT $this WHERE { $this undeclared:p ?o }",
        "SELECT $this WHERE { VALUES ?x { 1 } }",
    ] {
        let body = format!("ex:S sh:targetNode ex:a ; sh:sparql [ sh:select {q:?} ] .");
        assert_eq!(report(&body, || false).unwrap().2, 1, "{q}");
    }
}

/// Two paths joined with nothing in common: minutes of work, stopped at
/// the time limit once, not once per focus node.
#[test]
fn timing_a_slow_sparql_constraint_stops_at_the_time_limit() {
    let mut body = "ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this WHERE { ?a ex:next* ?b . ?c ex:next* ?d . FILTER (STRLEN(CONCAT(STR(?a), STR(?d))) < 0) }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n".to_string();
    for i in 0..400 {
        body.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    for i in 0..20 {
        body.push_str(&format!("ex:n{i} a ex:C .\n"));
    }
    let p = parse(
        &format!("{HEAD}{body}"),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    let g = Graph::new(&[&p.triples]);
    let start = std::time::Instant::now();
    let r = validate_report(&g, &|| false, None, |_| true, now).unwrap();
    let messages: Vec<&str> = r.failures.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(messages, [sparql::TIMED_OUT]);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

/// One reifier `~` per pattern past the widest join planned: planning
/// checks no clock and ran for minutes (chaos seed 34063671672, 74505).
#[test]
fn timing_a_join_too_wide_to_plan_is_a_failure() {
    let reifiers = "~".repeat(crate::sparql::spargebra_input::MAX_JOIN_WIDTH);
    let body = format!(
        "ex:S sh:targetNode ex:a ; sh:sparql [ sh:select \"SELECT $this WHERE {{ $this ?p ?o {reifiers} }}\" ] .\nex:a ex:p 1 .\n"
    );
    let p = parse(
        &format!("{HEAD}{body}"),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    let g = Graph::new(&[&p.triples]);
    let start = std::time::Instant::now();
    let r = validate_report(&g, &|| false, None, |_| true, now).unwrap();
    let messages: Vec<&str> = r.failures.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "the query is too complex: it joins 71 patterns or path steps at once (the most allowed is 70)"
        ]
    );
    assert!(
        start.elapsed() < std::time::Duration::from_millis(500),
        "{:?}",
        start.elapsed()
    );
}

/// A SPARQL target query that hits the time limit leaves the next one
/// to run.
#[test]
fn a_slow_sparql_target_leaves_the_next_one() {
    let mut body = "ex:S1 sh:target [ a sh:SPARQLTarget ; sh:prefixes ex:P ; sh:select \"SELECT ?this WHERE { ?this ex:next* ?b . ?c ex:next* ?d . FILTER (STRLEN(CONCAT(STR(?b), STR(?d))) < 0) }\" ] ; sh:property [ sh:path ex:q ; sh:minCount 1 ] .\nex:S2 sh:target [ a sh:SPARQLTarget ; sh:prefixes ex:P ; sh:select \"SELECT ?this WHERE { ?this ex:p 1 }\" ] ; sh:property [ sh:path ex:q ; sh:minCount 1 ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\nex:a ex:p 1 .\n".to_string();
    for i in 0..400 {
        body.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    let (n, _, failures) = report(&body, || false).unwrap();
    assert_eq!((n, failures), (1, 1));
}

/// Slow for 40 focus nodes at once, fast for each alone: the batch
/// that hits the time limit leaves the focus nodes to run one by one.
#[test]
fn a_slow_batch_runs_one_by_one() {
    let mut body = "ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT DISTINCT $this WHERE { $this ex:next* ?b . ?b ex:next* ?c . FILTER (STRLEN(CONCAT(STR($this), STR(?c))) > 0) }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n".to_string();
    for i in 0..40 {
        body.push_str(&format!("ex:h{i} a ex:C ; ex:next ex:c{i}x0 .\n"));
        for j in 0..60 {
            body.push_str(&format!("ex:c{i}x{j} ex:next ex:c{i}x{} .\n", j + 1));
        }
    }
    let (n, _, failures) = report(&body, || false).unwrap();
    assert_eq!((n, failures), (40, 0));
}

#[test]
fn cancelled_between_queries() {
    // Allow the poll before the focus node, stop at the one before the query.
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let cancelled = move || counted.fetch_add(1, Ordering::Relaxed) >= 1;
    let body = format!("{RULE}ex:a a ex:C ; ex:p 2 .");
    assert!(report(&body, cancelled).is_none());
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

/// Blank focus nodes are pre-bound like IRIs: in triple patterns, in
/// expressions, in EXISTS and as GROUP BY keys.
#[test]
fn sparql_on_blank_focus_nodes() {
    let q = |sel: &str| {
        format!(
            "ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:select \"{sel}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n[] a ex:C ; ex:p 2 .\n[] a ex:C ; ex:p 0 .\nex:i a ex:C ; ex:p 5 .\n"
        )
    };
    let n = |sel: &str| report(&q(sel), || false).unwrap();
    assert_eq!(
        n("SELECT $this ?value WHERE { $this ex:p ?value . FILTER (?value > 1) }").0,
        2
    );
    assert_eq!(n("SELECT $this WHERE { FILTER (isBlank($this)) }").0, 2);
    assert_eq!(
        n("SELECT $this WHERE { ?x ex:p 2 . FILTER (sameTerm(?x, $this)) }").0,
        1
    );
    assert_eq!(
        n("SELECT $this WHERE { FILTER NOT EXISTS { $this ex:p 0 } }").0,
        2
    );
    assert_eq!(
        n("SELECT $this (COUNT(*) AS ?n) WHERE { $this ex:p ?v } GROUP BY $this").0,
        3
    );
    assert_eq!(
        n("SELECT $this ?n WHERE { { SELECT (COUNT(?v) AS ?n) WHERE { $this ex:q ?v } } }").2,
        1
    );
    assert_eq!(n("SELECT $this ?n WHERE { { SELECT $this (COUNT(?v) AS ?n) WHERE { $this ex:q ?v } GROUP BY $this } }").0, 0);
}

/// A NOT EXISTS over a subclass closure, the shape of a type check.
#[test]
fn sparql_not_exists_over_subclasses() {
    let body = "ex:S sh:targetClass ex:Thing ; sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this ?value WHERE { $this ex:kind ?value . FILTER NOT EXISTS { $this a/<http://www.w3.org/2000/01/rdf-schema#subClassOf>* ?value } }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\nex:B <http://www.w3.org/2000/01/rdf-schema#subClassOf> ex:A .\nex:x a ex:Thing , ex:B ; ex:kind ex:A .\nex:y a ex:Thing ; ex:kind ex:A .\n[] a ex:Thing , ex:B ; ex:kind ex:A , ex:B , ex:Thing , ex:Z .";
    assert_eq!(report(body, || false).unwrap().0, 2);
}

/// Queries cover the forms batching joins into (filters, OPTIONAL,
/// UNION, EXISTS, GROUP BY, subqueries, blank focus nodes) and ones
/// that run one by one (LIMIT).
#[test]
fn batched_sparql_matches_one_by_one() {
    let mut data = String::new();
    for i in 0..30 {
        data.push_str(&format!(
            "ex:n{i} a ex:C ; ex:p {} ; ex:q ex:n{} .\n[] a ex:C ; ex:p {} ; ex:q ex:n{i} .\n",
            i % 4,
            (i + 1) % 30,
            i % 3
        ));
    }
    for sel in [
        "SELECT $this ?value WHERE { $this ex:p ?value . FILTER (?value > 1) }",
        "SELECT $this WHERE { FILTER (isBlank($this)) }",
        "SELECT $this WHERE { ?x ex:p 2 . FILTER (sameTerm(?x, $this)) }",
        "SELECT $this WHERE { FILTER NOT EXISTS { $this ex:p 0 } }",
        "SELECT $this (COUNT(*) AS ?n) WHERE { $this ex:p ?v } GROUP BY $this",
        "SELECT $this ?n WHERE { { SELECT $this (COUNT(?v) AS ?n) WHERE { ?v ex:q $this } GROUP BY $this } }",
        "SELECT $this ?value WHERE { ?value ex:p 1 OPTIONAL { $this ex:q ?value } }",
        "SELECT $this ?value WHERE { $this ex:q ?value } LIMIT 1",
        "SELECT $this ?value WHERE { { SELECT $this ?value WHERE { ?value ex:q $this } LIMIT 3 } }",
        "SELECT $this ?value WHERE { { $this ex:p ?value } UNION { ?value ex:q $this } FILTER (?value != 3) }",
        "SELECT DISTINCT $this ?value WHERE { $this ex:q/ex:p ?value . FILTER EXISTS { ?y ex:q $this } }",
        "SELECT $this ?value WHERE { { ?value ex:p ?w FILTER (sameTerm(?value, $this)) } UNION { $this ex:p ?value } }",
        "SELECT $this ?value WHERE { $this ex:p ?value OPTIONAL { ?z ex:q $this FILTER (?value > 1) } FILTER (!BOUND(?z)) }",
        "SELECT $this WHERE { $currentShape sh:targetClass ?c . $this a ?c . FILTER NOT EXISTS { $this ex:q ?z . ?z ex:p 3 } }",
    ] {
        let p = parse(
            &format!(
                "{HEAD}ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:message \"{{?value}}\" ; sh:select \"{sel}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] , [ sh:prefix \"sh\" ; sh:namespace \"http://www.w3.org/ns/shacl#\" ] .\n{data}"
            ),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let g = Graph::new(&[&p.triples]);
        let run = |min_batch| {
            let r = report_over(
                &g,
                Arc::new(|| false),
                &|_| true,
                None,
                min_batch,
                Budget::default(),
                |_| true,
                now,
            )
            .unwrap();
            let mut out: Vec<String> = r.results.iter().map(|x| format!("{x:?}")).collect();
            out.extend(r.failures.iter().map(|x| format!("{x:?}")));
            out.sort();
            out
        };
        let one_by_one = run(usize::MAX);
        assert!(!one_by_one.is_empty(), "{sel}");
        assert_eq!(run(1), one_by_one, "{sel}");
    }
}

/// Few focus nodes in a large graph run one by one: batched, this query
/// starts from the patterns over the whole graph (50x slower).
#[test]
fn few_focus_nodes_in_a_large_graph_run_one_by_one() {
    let mut data = String::new();
    for i in 0..20 {
        data.push_str(&format!("ex:n{i} a ex:C ; ex:p ex:v{i} .\n"));
    }
    for i in 0..100_000 {
        data.push_str(&format!(
            "ex:o{i} ex:q ex:v{} ; ex:r {} .\n",
            i % 5000,
            i % 7
        ));
    }
    let sel = "SELECT $this ?value WHERE { ?value ex:q ?v . ?value ex:r ?r . $this ex:p ?v }";
    let p = parse(
        &format!(
            "{HEAD}ex:S sh:targetClass ex:C ; sh:sparql [ sh:prefixes ex:P ; sh:select \"{sel}\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n{data}"
        ),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    let g = Graph::new(&[&p.triples]);
    assert!(sparse(20, g.len()), "20 focus nodes in {} triples", g.len());
    assert!(!sparse(300, g.len()));
    let run = |min_batch| {
        report_over(
            &g,
            Arc::new(|| false),
            &|_| true,
            None,
            min_batch,
            Budget::default(),
            |_| true,
            now,
        )
        .unwrap()
        .results
        .len()
    };
    assert_eq!(run(1), run(usize::MAX));
    assert_eq!(run(1), 400);
}

/// One SPARQL constraint on 10,000 focus nodes (run with --nocapture
/// for the time).
#[test]
fn timing_sparql_on_ten_thousand_focus_nodes() {
    let mut body = RULE.to_string();
    for i in 0..10_000 {
        body.push_str(&format!("ex:n{i} a ex:C ; ex:p {} .\n", i % 3));
    }
    let start = std::time::Instant::now();
    let (n, _, failures) = report(&body, || false).unwrap();
    let took = start.elapsed();
    println!("10,000 focus nodes, one SPARQL constraint: {took:?}");
    assert_eq!((n, failures), (3333, 0));
    if !cfg!(debug_assertions) {
        // About 30 ms when the query runs once for all focus nodes.
        assert!(took.as_secs_f64() < 0.5, "{took:?}");
    }
}

// Bounds and fail-closed validation.

/// A `Report` for `body` after `HEAD`, within `budget`, cancelled by `cancel`.
fn bounded(
    body: &str,
    budget: Budget,
    cancel: impl Fn() -> bool + Send + Sync + 'static,
) -> Option<Report> {
    let p = parse(
        &format!("{HEAD}{body}"),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let g = Graph::new(&[&p.triples]);
    report_over(
        &g,
        Arc::new(cancel),
        &|_| true,
        None,
        MIN_BATCH,
        budget,
        |_| true,
        now,
    )
}

/// `ex:n0 ex:next ex:n1 ...` over `n` nodes, the first `typed` of them
/// (spread evenly) in `ex:C`.
fn chain(n: usize, typed: usize) -> String {
    let mut data = String::new();
    for i in 0..n {
        data.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
    }
    for k in 0..typed {
        data.push_str(&format!("ex:n{} a ex:C .\n", k * (n / typed)));
    }
    data
}

const STAR: &str = "ex:S sh:targetClass ex:C ; sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:maxCount 5 ] .\n";

#[test]
fn timing_a_closure_over_a_20k_chain_is_fast() {
    let start = std::time::Instant::now();
    let r = bounded(
        &format!("{STAR}{}", chain(20_000, 50)),
        Budget::default(),
        || false,
    )
    .unwrap();
    assert_eq!((r.results.len(), r.failures.len()), (50, 0));
    if !cfg!(debug_assertions) {
        assert!(start.elapsed().as_secs_f64() < 1.0, "{:?}", start.elapsed());
    }
}

/// Every node of a chain asks for its own closure: 200 million values
/// in all. The step budget ends that with a failure, quickly.
#[test]
fn timing_a_closure_from_every_node_of_a_20k_chain_stops_at_the_step_budget() {
    let start = std::time::Instant::now();
    let r = bounded(
        &format!("{STAR}{}", chain(20_000, 20_000)),
        Budget::default(),
        || false,
    )
    .unwrap();
    let cut: Vec<&Failure> = r
        .failures
        .iter()
        .filter(|f| f.kind == FailureKind::Cut)
        .collect();
    assert_eq!(cut.len(), 1, "{:?}", r.failures);
    assert!(cut[0].message.contains("steps"), "{}", cut[0].message);
    if !cfg!(debug_assertions) {
        // Seconds without the budget ends (minutes without it) on a quiet
        // machine well under one; tests run side by side.
        assert!(start.elapsed().as_secs_f64() < 5.0, "{:?}", start.elapsed());
    }
}

#[test]
fn nested_closures_merge() {
    let path = |body: &str| {
        let p = parse(
            &format!("{HEAD}ex:S sh:path {body} ."),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&p.triples]);
        let t = g.with_p(sh("path")).next().unwrap();
        parse_path(&g, t.o)
    };
    let leaf = Path::Predicate(Iri::intern("http://example.com/p"));
    let star = Path::ZeroOrMore(Box::new(leaf.clone()));
    let plus = Path::OneOrMore(Box::new(leaf.clone()));
    let opt = Path::ZeroOrOne(Box::new(leaf.clone()));
    let nest = |outer: &str, inner: &str| path(&format!("[ sh:{outer} [ sh:{inner} ex:p ] ]"));
    assert_eq!(nest("zeroOrMorePath", "zeroOrMorePath"), star);
    assert_eq!(nest("zeroOrMorePath", "oneOrMorePath"), star);
    assert_eq!(nest("zeroOrMorePath", "zeroOrOnePath"), star);
    assert_eq!(nest("oneOrMorePath", "zeroOrMorePath"), star);
    assert_eq!(nest("oneOrMorePath", "zeroOrOnePath"), star);
    assert_eq!(nest("zeroOrOnePath", "oneOrMorePath"), star);
    assert_eq!(nest("zeroOrOnePath", "zeroOrMorePath"), star);
    assert_eq!(nest("oneOrMorePath", "oneOrMorePath"), plus);
    assert_eq!(nest("zeroOrOnePath", "zeroOrOnePath"), opt);
    // Not merged when something sits between.
    assert_eq!(
        path("[ sh:zeroOrMorePath ( [ sh:zeroOrMorePath ex:p ] ex:p ) ]"),
        Path::ZeroOrMore(Box::new(Path::Sequence(vec![star, leaf])))
    );
}

/// Closures nested through sequences cannot be merged; evaluated per
/// node each time they cost the graph's size to the nesting's power
/// (four levels over 300 triples took 17 s).
#[test]
fn timing_four_nested_closures_over_300_triples_finish() {
    let mut data = String::new();
    for i in 0..100 {
        data.push_str(&format!(
            "ex:n{i} ex:a ex:n{} ; ex:b ex:n{} ; ex:c ex:n{} .\n",
            (i + 1) % 100,
            (i + 3) % 100,
            (i + 7) % 100
        ));
    }
    data.push_str("ex:n0 a ex:C .\nex:n1 a ex:C .\n");
    let path = "[ sh:zeroOrMorePath ( [ sh:zeroOrMorePath ( [ sh:zeroOrMorePath ( [ sh:zeroOrMorePath ex:a ] ex:b ) ] ex:c ) ] ex:a ) ]";
    let start = std::time::Instant::now();
    let r = bounded(
        &format!(
            "ex:S sh:targetClass ex:C ; sh:property [ sh:path {path} ; sh:minCount 1 ] .\n{data}"
        ),
        Budget::default(),
        || false,
    )
    .unwrap();
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert!(start.elapsed().as_secs_f64() < 5.0, "{:?}", start.elapsed());
}

#[test]
fn cancelling_stops_inside_one_focus_node() {
    // One focus node, a 100,000-node chain: the only poll between focus
    // nodes is the one before it.
    let body = format!(
        "ex:S sh:targetNode ex:n0 ; sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:maxCount 5 ] .\n{}",
        chain(100_000, 0)
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let cancel = move || counted.fetch_add(1, Ordering::Relaxed) >= 2;
    assert!(bounded(&body, Budget::default(), cancel).is_none());
    // Stopped after a few polls, not after the 100,000 steps.
    assert!(calls.load(Ordering::Relaxed) < 10);
    // And finished when not cancelled.
    let r = bounded(&body, Budget::default(), || false).unwrap();
    assert_eq!(r.results.len(), 1);
}

#[test]
fn timing_equals_over_large_value_sets_is_linear() {
    let mut data =
        String::from("ex:S sh:targetNode ex:x ; sh:property [ sh:path ex:a ; sh:equals ex:b ] .\n");
    for i in 0..30_000 {
        data.push_str(&format!("ex:x ex:a {i} ; ex:b {} .\n", i + 1));
    }
    let start = std::time::Instant::now();
    let r = bounded(&data, Budget::default(), || false).unwrap();
    // 0 is only in a, 30000 only in b.
    assert_eq!(r.results.len(), 2);
    if !cfg!(debug_assertions) {
        assert!(start.elapsed().as_secs_f64() < 1.0, "{:?}", start.elapsed());
    }
}

/// A SELECT whose solutions are every pair of triples: 25 million over
/// 5,000. It used to collect them all (11.7 GB) before the cap.
#[test]
fn timing_a_cartesian_select_stops_at_the_result_budget() {
    let mut data = String::from(
        "ex:S sh:targetNode ex:x ; sh:sparql [ sh:select \"SELECT $this ?value WHERE { ?a ?p ?o . ?value ?q ?r }\" ] .\n",
    );
    for i in 0..5_000 {
        data.push_str(&format!("ex:s{i} ex:p {i} .\n"));
    }
    let start = std::time::Instant::now();
    let r = bounded(&data, Budget::default(), || false).unwrap();
    assert_eq!(r.results.len(), Budget::default().results);
    assert!(r.capped);
    assert!(r.failures.is_empty(), "{:?}", r.failures);
    assert!(start.elapsed().as_secs_f64() < 5.0, "{:?}", start.elapsed());
    // A smaller budget, kept exactly (the cap is not the batch size).
    let budget = Budget {
        results: 100,
        ..Budget::default()
    };
    let r = bounded(&data, budget, || false).unwrap();
    assert_eq!((r.results.len(), r.capped), (100, true));
}

#[test]
fn core_results_stop_at_the_budget_too() {
    let mut data =
        String::from("ex:S sh:targetClass ex:C ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n");
    for i in 0..500 {
        data.push_str(&format!("ex:n{i} a ex:C .\n"));
    }
    let budget = Budget {
        results: 10,
        ..Budget::default()
    };
    let r = bounded(&data, budget, || false).unwrap();
    assert_eq!((r.results.len(), r.capped), (10, true));
    // Exactly at the budget is not over it.
    let r = bounded(
        &data,
        Budget {
            results: 500,
            ..budget
        },
        || false,
    )
    .unwrap();
    assert_eq!((r.results.len(), r.capped), (500, false));
}

/// A query running for good, stopped by the job being superseded rather
/// than by its time limit (200 ms in tests).
#[test]
fn timing_a_superseded_run_stops_inside_one_long_query() {
    let mut body = "ex:S sh:targetNode ex:n0 ; sh:sparql [ sh:prefixes ex:P ; sh:select \"SELECT $this WHERE { ?a ex:next* ?b . ?c ex:next* ?d . FILTER (STRLEN(CONCAT(STR(?a), STR(?d))) < 0) }\" ] .\nex:P sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\" ] .\n".to_string();
    body.push_str(&chain(400, 0));
    let began = std::time::Instant::now();
    let cancel = move || began.elapsed() > std::time::Duration::from_millis(20);
    let r = bounded(&body, Budget::default(), cancel);
    assert!(r.is_none());
    assert!(
        began.elapsed() < sparql::TIME_LIMIT,
        "{:?} is not before the time limit",
        began.elapsed()
    );
}

/// What a sh:not over 100 values says with `steps` of path budget: each
/// value has three nodes along `ex:next*`, so none conforms to ex:T
/// (sh:maxCount 1) and none is a sh:not violation.
fn not_over_values(steps: u64) -> Report {
    let mut data = String::from(
        "ex:S sh:targetNode ex:x ; sh:property [ sh:path ex:p ; sh:not ex:T ] .\nex:T sh:path [ sh:zeroOrMorePath ex:next ] ; sh:maxCount 1 .\n",
    );
    for i in 0..100 {
        data.push_str(&format!(
            "ex:x ex:p ex:v{i} .\nex:v{i} ex:next ex:w{i} .\nex:w{i} ex:next ex:z{i} .\n"
        ));
    }
    let budget = Budget {
        steps,
        ..Budget::default()
    };
    bounded(&data, budget, || false).unwrap()
}

fn cut_by_steps(r: &Report) -> bool {
    r.failures
        .iter()
        .any(|f| f.kind == FailureKind::Cut && f.message.contains("steps"))
}

/// The values not checked once the step budget ran out conformed to
/// ex:T as far as sh:not could tell: a false violation for each.
#[test]
fn a_sh_not_cut_by_the_step_budget_reports_no_false_violation() {
    let whole = not_over_values(Budget::default().steps);
    assert_eq!((whole.results.len(), whole.failures.len()), (0, 0));
    let cut = not_over_values(50);
    assert!(cut.results.is_empty(), "{:?}", cut.results);
    assert!(cut_by_steps(&cut), "{:?}", cut.failures);
}

/// A sequence path whose `ex:next*` the step budget cut short reached no
/// `ex:q`: a false sh:minCount violation.
#[test]
fn a_path_cut_by_the_step_budget_reports_no_false_min_count() {
    let data = format!(
        "ex:S sh:targetNode ex:x ; sh:property [ sh:path ( [ sh:zeroOrMorePath ex:next ] ex:q ) ; sh:minCount 1 ] .\nex:x ex:next ex:n0 .\nex:n200 ex:q 1 .\n{}",
        chain(200, 0)
    );
    let run = |steps| {
        let budget = Budget {
            steps,
            ..Budget::default()
        };
        bounded(&data, budget, || false).unwrap()
    };
    let whole = run(Budget::default().steps);
    assert_eq!((whole.results.len(), whole.failures.len()), (0, 0));
    let cut = run(50);
    assert!(cut.results.is_empty(), "{:?}", cut.results);
    assert!(cut_by_steps(&cut), "{:?}", cut.failures);
}

/// The results of focus nodes checked before the budget ran out stay.
#[test]
fn results_before_the_step_budget_ran_out_are_kept() {
    let mut data = String::from(
        "ex:S sh:targetClass ex:C ; sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:maxCount 1 ] .\n",
    );
    for i in 0..100 {
        data.push_str(&format!(
            "ex:v{i} a ex:C ; ex:next ex:w{i} .\nex:w{i} ex:next ex:z{i} .\n"
        ));
    }
    let budget = Budget {
        steps: 50,
        ..Budget::default()
    };
    let r = bounded(&data, budget, || false).unwrap();
    assert!(cut_by_steps(&r), "{:?}", r.failures);
    // About two steps per focus node: some finished, each a true result.
    assert!((5..100).contains(&r.results.len()), "{}", r.results.len());
}

/// A shape that checks itself through sh:node asks for its SPARQL check
/// first from inside another check, where one result is enough. That
/// ask used to size the batch for every focus node to one solution: too
/// many, so the check ran once per focus node.
#[test]
fn a_nested_check_runs_first_and_the_batch_still_serves_every_focus_node() {
    let mut body = format!(
        "{}ex:S sh:property [ sh:path ex:knows ; sh:node ex:S ] .\nex:n0 ex:knows ex:n1 .\n",
        RULE
    );
    for i in 0..50 {
        body.push_str(&format!("ex:n{i} a ex:C ; ex:p 2 .\n"));
    }
    let p = parse(
        &format!("{HEAD}{body}"),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let g = Graph::new(&[&p.triples]);
    QUERIES_RUN.with(|c| c.set((0, 0)));
    let r = report_over(
        &g,
        Arc::new(|| false),
        &|_| true,
        None,
        1,
        Budget::default(),
        |_| true,
        now,
    )
    .unwrap();
    // A result per focus node, and n0's sh:node one for n1.
    assert_eq!((r.results.len(), r.failures.len()), (51, 0));
    // One batch; n1 runs once on its own, its rows taken by the nested check.
    assert_eq!(QUERIES_RUN.with(|c| c.get()), (1, 1));
}

/// 100 nodes in a chain, all in `ex:C`; every one but n50 has `ex:ok`.
fn deep_chain(targets: &str) -> String {
    let mut data = format!(
        "ex:S sh:targetNode {targets} ; sh:property [ sh:path ex:ok ; sh:minCount 1 ] ; sh:property [ sh:path ex:next ; sh:node ex:S ] .\n"
    );
    for i in 0..100 {
        data.push_str(&format!("ex:n{i} ex:next ex:n{} .\n", i + 1));
        if i != 50 {
            data.push_str(&format!("ex:n{i} ex:ok true .\n"));
        }
    }
    data
}

/// Past 64 nested shapes a check is cut short. It used to count as
/// conforming, silently and for good: the answer for a node depended on
/// which nodes were checked before it.
#[test]
fn the_depth_limit_is_reported_and_never_cached() {
    let run = |targets: &str| {
        let p = parse(
            &format!("{HEAD}{}", deep_chain(targets)),
            &PrefixMap::default(),
            Syntax::Turtle,
        );
        let g = Graph::new(&[&p.triples]);
        let r = report_over(
            &g,
            Arc::new(|| false),
            &|_| true,
            None,
            MIN_BATCH,
            Budget::default(),
            |_| true,
            now,
        )
        .unwrap();
        let name = |id: Id| match g.term(id) {
            Term::Iri(i) => i
                .as_str()
                .trim_start_matches("http://example.com/")
                .to_string(),
            _ => "_".into(),
        };
        let mut found: Vec<String> = r
            .results
            .iter()
            .map(|x| {
                let component = x
                    .component
                    .as_str()
                    .trim_start_matches(crate::shacl::SH)
                    .to_string();
                format!("{} {:?} {component}", name(x.focus), x.value.map(name))
            })
            .collect();
        found.sort();
        (found, r.failures)
    };
    let at20 = |found: &[String]| -> Vec<String> {
        found
            .iter()
            .filter(|f| f.starts_with("n20 "))
            .cloned()
            .collect()
    };
    // n20 reaches the missing ex:ok of n50 in 30 hops, within the limit:
    // its sh:node violation is real.
    let (alone, failures) = run("ex:n20");
    assert!(failures.is_empty(), "{failures:?}");
    assert!(
        alone.contains(&"n20 Some(\"n21\") NodeConstraintComponent".to_string()),
        "{alone:?}"
    );
    // n0 is 50 hops away: cut, reported. What the cut left unfinished
    // was not remembered, so n20 gets the same answer as alone.
    let (forward, failures) = run("ex:n0 , ex:n20");
    assert_eq!(at20(&forward), at20(&alone));
    assert!(
        failures
            .iter()
            .any(|f| f.kind == FailureKind::Cut && f.message.contains("64")),
        "{failures:?}"
    );
    assert!(!forward.iter().any(|f| f.starts_with("n0 ")), "{forward:?}");
    // The other way round n0 reuses what n20 settled and sees further:
    // a true result, never a false one.
    let (backward, _) = run("ex:n20 , ex:n0");
    assert!(forward.iter().all(|f| backward.contains(f)), "{backward:?}");
}

/// ex:x against `sh:not ex:S1`, where ex:S1 to ex:S2999 are a `sh:node`
/// chain, read as an overlay over itself: every shape has its sh:node
/// twice. Past 64 levels nothing is cached, so the checks double with
/// every level and no path step is ever taken.
fn doubled_node_chain(
    budget: Budget,
    cancel: impl Fn() -> bool + Send + Sync + 'static,
) -> Option<Report> {
    let mut body = String::from("ex:S0 sh:targetNode ex:x ; sh:not ex:S1 .\n");
    for i in 1..3_000 {
        body.push_str(&format!("ex:S{i} sh:node ex:S{} .\n", i + 1));
    }
    let p = parse(
        &format!("{HEAD}{body}"),
        &PrefixMap::default(),
        Syntax::Turtle,
    );
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let base = Arc::new(Graph::new(&[&p.triples]));
    let g = Graph::overlay(&base, &p.triples);
    report_over(
        &g,
        Arc::new(cancel),
        &|_| true,
        None,
        MIN_BATCH,
        budget,
        |_| true,
        now,
    )
}

#[test]
fn cancelling_stops_inside_shape_checks() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = polls.clone();
    // The poll before the focus node, two more, then the fourth stops.
    let cancel = move || counted.fetch_add(1, Ordering::Relaxed) >= 3;
    assert!(doubled_node_chain(Budget::default(), cancel).is_none());
    assert_eq!(polls.load(Ordering::Relaxed), 4);
}

#[test]
fn shape_checks_stop_at_the_step_budget() {
    let polls = Arc::new(AtomicUsize::new(0));
    let counted = polls.clone();
    let cancel = move || {
        counted.fetch_add(1, Ordering::Relaxed);
        false
    };
    let budget = Budget {
        steps: 100_000,
        ..Budget::default()
    };
    let r = doubled_node_chain(budget, cancel).unwrap();
    // sh:not over a check the budget cut short would say ex:x conforms
    // to ex:S1: a false violation.
    assert!(r.results.is_empty(), "{:?}", r.results);
    assert!(cut_by_steps(&r), "{:?}", r.failures);
    // The poll before the focus node, then one per POLL_EVERY steps up
    // to the budget: no check ran past it.
    assert_eq!(
        polls.load(Ordering::Relaxed) as u64,
        1 + budget.steps / POLL_EVERY
    );
}

#[test]
fn a_target_may_use_minus() {
    let body = "ex:S sh:target [ a sh:SPARQLTarget ; sh:select \"SELECT ?this WHERE { ?this <http://example.com/p> ?o MINUS { ?this <http://example.com/skip> ?x } }\" ] ; sh:property [ sh:path ex:q ; sh:minCount 1 ] .\nex:a ex:p 1 .\nex:b ex:p 1 ; ex:skip 1 .";
    let (n, _, failures) = report(body, || false).unwrap();
    assert_eq!((n, failures), (1, 0));
    // A constraint may not.
    let body = "ex:S sh:targetNode ex:a ; sh:sparql [ sh:select \"SELECT $this WHERE { $this ?p ?o MINUS { $this ?p 1 } }\" ] .";
    assert_eq!(report(body, || false).unwrap().2, 1);
}

/// Chain of `n` nested `sh:inversePath` steps ending at `ex:p`, as flat
/// triples (a nested blank node syntax would be as deep as the path).
fn long_path(n: usize) -> String {
    let mut body = String::from(
        "ex:S sh:targetNode ex:a ; sh:property [ sh:path _:b0 ; sh:minCount 1 ] .\nex:b ex:p ex:a .\n",
    );
    for i in 0..n {
        body.push_str(&format!("_:b{i} sh:inversePath _:b{} .\n", i + 1));
    }
    // n + 1 inverses in all.
    body.push_str(&format!("_:b{n} sh:inversePath ex:p .\n"));
    body
}

#[test]
fn timing_a_path_too_deep_to_read_is_a_failure_not_a_skip() {
    // 11 steps validate: an odd number of inverses reaches b, backwards
    // along ex:p.
    let r = bounded(&long_path(10), Budget::default(), || false).unwrap();
    assert_eq!(
        (r.results.len(), r.failures.len()),
        (0, 0),
        "{:?}",
        r.failures
    );
    // The same with the value missing is a violation.
    let missing = long_path(10).replace("ex:b ex:p ex:a .", "");
    let r = bounded(&missing, Budget::default(), || false).unwrap();
    assert_eq!((r.results.len(), r.failures.len()), (1, 0));
    // 20,000 steps cannot be read: reported on the shape's sh:path.
    let start = std::time::Instant::now();
    let r = bounded(&long_path(20_000), Budget::default(), || false).unwrap();
    assert_eq!(r.results.len(), 0);
    assert_eq!(r.failures.len(), 1, "{:?}", r.failures);
    assert_eq!(r.failures[0].kind, FailureKind::Path);
    assert!(r.failures[0].triple.is_some());
    assert!(start.elapsed().as_secs_f64() < 5.0);
    // A path that is not a path at all is the same.
    let r = bounded(
        "ex:S sh:targetNode ex:a ; sh:property [ sh:path ( ) ; sh:minCount 1 ] .",
        Budget::default(),
        || false,
    )
    .unwrap();
    assert_eq!(r.failures.len(), 1);
    // Deactivated: skipped by choice.
    let r = bounded(
        "ex:S sh:targetNode ex:a ; sh:property [ sh:path ( ) ; sh:deactivated true ] .",
        Budget::default(),
        || false,
    )
    .unwrap();
    assert!(r.failures.is_empty());
}
