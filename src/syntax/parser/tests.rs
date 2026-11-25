use super::*;

fn p(src: &str) -> Parsed {
    parse(src, &PrefixMap::default(), Syntax::Turtle)
}

fn trig(src: &str) -> Parsed {
    parse(src, &PrefixMap::default(), Syntax::TriG)
}

fn nt(src: &str) -> Parsed {
    parse(src, &PrefixMap::default(), Syntax::NTriples)
}

fn nq(src: &str) -> Parsed {
    parse(src, &PrefixMap::default(), Syntax::NQuads)
}
fn messages(r: &Parsed) -> Vec<&str> {
    r.errors.iter().map(|e| e.message()).collect()
}

#[test]
fn fixed_messages_are_borrowed() {
    let r = p("@prefix ex: <http://e/> .\nex:a ex:p \"open\nex:b ex:p ex:o");
    let unterminated = r
        .errors
        .iter()
        .find(|e| e.message() == "Unterminated string")
        .expect("the lexer's error");
    assert!(matches!(unterminated.message, Cow::Borrowed(_)));
    let missing_dot = r
        .errors
        .iter()
        .find(|e| e.message() == "Expected '.' to end the statement")
        .expect("the parser's error");
    assert!(matches!(missing_dot.message, Cow::Borrowed(_)));
}

/// `n` lines, each a statement that parses and one that does not.
fn broken_lines(n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "<http://e/s{i}> <http://e/p> <http://e/o{i}> . <http://e/t{i}> <http://e/p> .\n"
            )
        })
        .collect()
}

#[test]
fn every_error_is_kept_and_parsing_goes_on() {
    let src = broken_lines(1500);
    let r = p(&src);
    assert_eq!(r.errors.len(), 1500);
    let line = |e: &SyntaxError| src[..e.span.start as usize].matches('\n').count();
    for (i, e) in r.errors.iter().enumerate() {
        assert_eq!(line(e), i, "{e:?}");
        assert_eq!(e.message(), "Expected an object, found '.'");
        assert_eq!((e.severity, e.code), (Severity::Error, "syntax"));
    }
    // Every valid statement still gives its triple, the last line's too.
    assert_eq!(r.triples.len(), 1500);
    assert_eq!(r.statements.len(), 3000);
    assert!(
        r.triples
            .iter()
            .any(|t| t.o == Node::Iri(Iri::intern("http://e/o1499")))
    );
}

#[test]
fn errors_come_in_the_order_found_not_the_order_in_the_text() {
    // The lexer's errors are found before the parser's; here they come
    // last in the text.
    let mut src = broken_lines(300);
    for i in 0..300 {
        src.push_str(&format!("<http://e/u{i}> <http://e/p> \"open\n"));
    }
    let r = p(&src);
    // A missing '.' after each unterminated string as well.
    assert_eq!(r.errors.len(), 900);
    let (lexer, parser) = r.errors.split_at(300);
    assert!(lexer.iter().all(|e| e.message() == "Unterminated string"));
    assert!(
        parser[..300]
            .iter()
            .all(|e| e.message() == "Expected an object, found '.'")
    );
    assert!(lexer[0].span.start > parser[299].span.start);
    assert_eq!(r.triples.len(), 600);
}

#[test]
fn triples_and_refs() {
    let r = p(
        "@prefix ex: <http://e/> .\nex:a a ex:B ;\n  ex:p \"x\"@en, 1, [ ex:q ex:r ], (ex:l1 ex:l2) .\n",
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.prefixes[0].name, "ex");
    assert_eq!(r.statements.len(), 1);
    assert_eq!(r.statements[0].subject.as_ref().unwrap().0, "http://e/a");
    assert!(
        r.refs
            .iter()
            .any(|x| x.iri == "http://e/q" && x.role == Role::Predicate)
    );
    // a, B, p x4 objects, q r, l1 l2 + list triples
    assert_eq!(r.triples.len(), 1 + 4 + 1 + 4); // a, 4 objects of ex:p, [ ex:q ], 2-item list
}

#[test]
fn recovers_after_errors() {
    let r = p(
        "@prefix ex: <http://e/> .\nex:a ex:p ex:o .\nex:b ex:p \"open .\nex:c ex:p ex:o\nex:d ex:p ex:o .\nex:e ex:p nope:x .\nex:f ex:p ex:o .\n",
    );
    let subjects: Vec<_> = r
        .statements
        .iter()
        .filter_map(|s| s.subject.map(|x| x.0.as_str()))
        .collect();
    for s in [
        "http://e/a",
        "http://e/c",
        "http://e/d",
        "http://e/e",
        "http://e/f",
    ] {
        assert!(subjects.contains(&s), "{s} missing from {subjects:?}");
    }
    assert!(
        r.errors
            .iter()
            .any(|e| e.message().contains("Unterminated string"))
    );
    assert!(
        r.errors
            .iter()
            .any(|e| e.message().contains("Expected '.'"))
    );
    assert!(
        r.prefix_uses
            .iter()
            .any(|u| u.name == "nope" && !u.resolved)
    );
}

#[test]
fn external_prefixes() {
    let mut ext = PrefixMap::default();
    ext.insert("ex".into(), "http://e/".into());
    let r = parse("ex:a ex:p ex:o .", &ext, Syntax::Turtle);
    assert!(r.errors.is_empty());
    assert_eq!(r.triples[0].p, "http://e/p");
}

#[test]
fn object_on_next_line_is_fine() {
    let r = p("@prefix ex: <http://e/> .\nex:s ex:p\n    ex:o .\n");
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

#[test]
fn rdf12_syntax() {
    let r = p(
        "@prefix ex: <http://e/> .\nex:a ex:p ex:o ~ ex:r {| ex:src ex:s |} .\n<< ex:a ex:p ex:o >> ex:q ex:z .\nex:x ex:y <<( ex:a ex:p ex:o )>> .\n\"x\"@en--ltr .\n",
    );
    assert!(
        r.errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .count()
            >= 1
    ); // last line has no subject
    assert!(r.triples.iter().any(|t| t.p.ends_with("reifies")));
}

#[test]
fn triples_carry_predicate_and_object_spans() {
    let src = "@prefix ex: <http://e/> .\nex:a ex:p \"x\"@en ;\n  ex:q [ ex:r ( ex:l1 ex:l2 ) ] .\nex:b ex:p ex:o ~ ex:r .\n";
    let r = p(src);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let at = |sp: Span| &src[sp.start as usize..sp.end as usize];
    let find = |p: &str| r.triples.iter().find(|t| t.p == p).unwrap();
    let q = find("http://e/q");
    assert_eq!(at(q.p_span), "ex:q");
    assert_eq!(at(q.o_span), "[ ex:r ( ex:l1 ex:l2 ) ]");
    assert_eq!(at(find("http://e/r").o_span), "( ex:l1 ex:l2 )");
    let lit = find("http://e/p");
    assert_eq!(at(lit.o_span), "\"x\"@en");
    assert_eq!(lit.statement, 0);
    let firsts: Vec<_> = r
        .triples
        .iter()
        .filter(|t| t.p.ends_with("#first"))
        .map(|t| at(t.o_span))
        .collect();
    assert_eq!(firsts, ["ex:l1", "ex:l2"]);
    let reifies = r.triples.iter().find(|t| t.p.ends_with("reifies")).unwrap();
    assert_eq!((at(reifies.p_span), at(reifies.o_span)), ("ex:p", "ex:o"));
    assert_eq!(reifies.statement, 1);
    let st = &r.statements[reifies.statement as usize];
    assert_eq!(st.subject.unwrap().0, "http://e/b");
}

#[test]
fn blank_node_labels_are_unique_per_document() {
    let r = p("@prefix ex: <http://e/> .\n_:g1 ex:p [ ex:q _:g1 ] .\n_:b1 ex:p _:g2 .\n");
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let bnode = |n: &Node| match n {
        Node::Blank(b) => *b,
        _ => panic!("not a blank node"),
    };
    let [inner, outer, last] = &r.triples[..] else {
        panic!("{:?}", r.triples)
    };
    // `_:g1` is the same node both times, and not the generated `[ ]` node.
    assert_eq!(bnode(&outer.s), bnode(&inner.o));
    assert_ne!(bnode(&outer.s), bnode(&outer.o));
    assert_eq!(bnode(&outer.o), bnode(&inner.s));
    let all = [&outer.s, &outer.o, &last.s, &last.o].map(bnode);
    let distinct: std::collections::BTreeSet<_> = all.iter().collect();
    assert_eq!(distinct.len(), 4);
}

#[test]
fn trig_graph_blocks() {
    let src = "@prefix ex: <http://e/> .\nex:d ex:p ex:o .\nex:g1 { ex:a ex:p ex:o . ex:b ex:p ex:o }\nGRAPH ex:g2 {\n  ex:c ex:p [ ex:q 1 ] .\n}\n{ ex:e ex:p ex:o }\n_:g { ex:f ex:p ex:o . }\n[] { ex:h ex:p ex:o }\n";
    let r = trig(src);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let graph_of = |s: &str| {
        let t = r
            .triples
            .iter()
            .find(|t| t.s == Node::Iri(Iri::intern(s)))
            .unwrap();
        t.graph
    };
    assert_eq!(graph_of("http://e/d"), None);
    assert_eq!(
        graph_of("http://e/a"),
        Some(GraphName::Iri(Iri::intern("http://e/g1")))
    );
    assert_eq!(
        graph_of("http://e/b"),
        Some(GraphName::Iri(Iri::intern("http://e/g1")))
    );
    assert_eq!(
        graph_of("http://e/c"),
        Some(GraphName::Iri(Iri::intern("http://e/g2")))
    );
    assert_eq!(graph_of("http://e/e"), None);
    assert!(matches!(graph_of("http://e/f"), Some(GraphName::Blank(_))));
    assert!(matches!(graph_of("http://e/h"), Some(GraphName::Blank(_))));
    // The [ ex:q 1 ] triple is in ex:g2 too.
    let q = r.triples.iter().find(|t| t.p == "http://e/q").unwrap();
    assert_eq!(q.graph, Some(GraphName::Iri(Iri::intern("http://e/g2"))));
    assert_eq!(r.graphs.len(), 5);
    let at = |sp: Span| &src[sp.start as usize..sp.end as usize];
    assert_eq!(
        at(r.graphs[0].span),
        "ex:g1 { ex:a ex:p ex:o . ex:b ex:p ex:o }"
    );
    assert_eq!(at(r.graphs[1].name_span.unwrap()), "ex:g2");
    assert!(at(r.graphs[1].span).starts_with("GRAPH") && at(r.graphs[1].span).ends_with('}'));
    assert_eq!(r.graphs[2].name, None);
    // Statements inside blocks have their subjects; names are Graph refs.
    let subjects: Vec<_> = r.graphs[0]
        .statements
        .clone()
        .map(|i| r.statements[i as usize].subject.unwrap().0.as_str())
        .collect();
    assert_eq!(subjects, ["http://e/a", "http://e/b"]);
    assert!(
        r.refs
            .iter()
            .any(|x| x.iri == "http://e/g1" && x.role == Role::Graph)
    );
    assert!(
        r.refs
            .iter()
            .any(|x| x.iri == "http://e/c" && x.role == Role::Subject)
    );
}

#[test]
fn trig_recovers_inside_and_between_blocks() {
    // Broken triple inside a block, unclosed block followed by another block,
    // directive inside a block, nested block, unclosed block at the end.
    let src = "@prefix ex: <http://e/> .\nex:g1 {\n  ex:a ex:p \"open .\n  ex:b ex:p ex:o .\n}\nex:g2 {\n  ex:c ex:p ex:o .\nex:g3 {\n  @prefix x: <http://x/> .\n  ex:d ex:p x:o\n}\nex:after ex:p ex:o .\nex:g4 { ex:e ex:p ex:o .\n";
    let r = trig(src);
    let msgs = messages(&r);
    assert!(
        msgs.iter().any(|m| m.contains("Unterminated string")),
        "{msgs:?}"
    );
    assert!(msgs.iter().any(|m| m.contains("Expected '}'")), "{msgs:?}");
    assert!(
        msgs.iter()
            .any(|m| m.contains("Directives are not allowed")),
        "{msgs:?}"
    );
    assert!(msgs.iter().any(|m| m.contains("Unclosed '{'")), "{msgs:?}");
    let graph_of = |s: &str| {
        r.triples
            .iter()
            .find(|t| t.s == Node::Iri(Iri::intern(s)))
            .unwrap_or_else(|| panic!("{s} lost: {msgs:?}"))
            .graph
    };
    let g = |n: &str| Some(GraphName::Iri(Iri::intern(n)));
    assert_eq!(graph_of("http://e/b"), g("http://e/g1"));
    assert_eq!(graph_of("http://e/c"), g("http://e/g2"));
    assert_eq!(graph_of("http://e/d"), g("http://e/g3"));
    assert_eq!(graph_of("http://e/after"), None);
    assert_eq!(graph_of("http://e/e"), g("http://e/g4"));
    // A stray '}' and a missing graph name are reported, parsing goes on.
    let r = trig("@prefix ex: <http://e/> .\n}\nGRAPH { ex:a ex:p ex:o }\nex:b ex:p ex:o .\n");
    let msgs = messages(&r);
    assert!(msgs.iter().any(|m| m.contains("found '}'")), "{msgs:?}");
    assert!(msgs.iter().any(|m| m.contains("graph name")), "{msgs:?}");
    assert_eq!(r.triples.len(), 2);
    // Cut short anywhere: never a panic.
    for src in [
        "GRAPH",
        "GRAPH ex:g",
        "ex:g {",
        "{ ex:a",
        "_:b",
        "[] {",
        "{ [ ex:p",
    ] {
        trig(src);
    }
}

#[test]
fn braces_and_graph_are_errors_in_turtle() {
    let r =
        p("@prefix ex: <http://e/> .\nex:g { ex:a ex:p ex:o }\nGRAPH ex:g { }\nex:b ex:p ex:o .\n");
    assert!(!r.errors.is_empty());
    assert!(r.graphs.is_empty());
    assert!(r.triples.iter().all(|t| t.graph.is_none()));
    assert!(
        r.statements
            .iter()
            .any(|s| s.subject.is_some_and(|x| x.0 == "http://e/b"))
    );
}

#[test]
fn ntriples_accepts_ntriples() {
    let r = nt(concat!(
        "<http://e/a> <http://e/p> <http://e/o> .\n",
        "# comment\n",
        "_:b1 <http://e/p> \"x\\n\"@en-GB . # trailing\n",
        "<http://e/a> <http://e/p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
        "<http://e/a> <http://e/p> <<( <http://e/s> <http://e/p> \"o\"@en--ltr )>> .\n",
        "\n",
    ));
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.triples.len(), 4);
}

#[test]
fn nquads_puts_each_line_in_its_graph() {
    let r = nq(concat!(
        "<http://e/a> <http://e/p> <http://e/o> <http://e/g> .\n",
        "<http://e/a> <http://e/p> \"x\" .\n",
        "_:s <http://e/p> <<( <http://e/s> <http://e/p> _:o )>> _:g . # c\n",
    ));
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let g = |i: usize| r.triples[i].graph;
    assert_eq!(g(0), Some(GraphName::Iri(Iri::intern("http://e/g"))));
    assert_eq!(g(1), None);
    assert!(matches!(g(2), Some(GraphName::Blank(_))));
    assert_eq!(
        r.statements[0].graph.map(|(_, s)| s),
        Some(Span::new(39, 51))
    );
    assert!(r.graphs.is_empty());
}

#[test]
fn nquads_reports_bad_graph_labels_per_line() {
    let r = nq(concat!(
        "<http://e/a> <http://e/p> <http://e/o> \"g\" .\n",
        "<http://e/a> <http://e/p> <http://e/o> <http://e/g> <http://e/h> .\n",
        "<http://e/a> <http://e/p> <http://e/o> ex:g .\n",
        "<http://e/a> <http://e/p> <http://e/o> <g> .\n",
        "<http://e/b> <http://e/p> <http://e/o> <http://e/g> .\n",
    ));
    let msgs = messages(&r);
    assert!(msgs[0].contains("not a literal"), "{msgs:?}");
    assert!(
        msgs.iter().any(|m| m.contains("at most four terms")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("N-Quads needs full IRIs")),
        "{msgs:?}"
    );
    assert!(
        msgs.iter()
            .any(|m| m.starts_with("N-Quads needs absolute IRIs")),
        "{msgs:?}"
    );
    assert!(msgs.iter().all(|m| !m.contains("N-Triples")), "{msgs:?}");
    let last = r.triples.last().unwrap();
    assert_eq!(last.s, Node::Iri(Iri::intern("http://e/b")));
    // A quad is an error in N-Triples.
    assert!(
        !nt("<http://e/a> <http://e/p> <http://e/o> <http://e/g> .\n")
            .errors
            .is_empty()
    );
}

#[test]
fn ntriples_reports_turtle_only_constructs() {
    let cases = [
        ("@prefix ex: <http://e/> .\n", "directives"),
        ("PREFIX ex: <http://e/>\n", "directives"),
        ("@base <http://e/> .\n", "directives"),
        ("<http://e/a> <http://e/p> ex:o .\n", "prefixed names"),
        ("<http://e/a> a <http://e/C> .\n", "'a'"),
        (
            "<http://e/a> <http://e/p> <http://e/o> ; <http://e/q> <http://e/o> .\n",
            "';' or ','",
        ),
        (
            "<http://e/a> <http://e/p> <http://e/o> , <http://e/o2> .\n",
            "';' or ','",
        ),
        (
            "<http://e/a> <http://e/p> [ <http://e/q> <http://e/o> ] .\n",
            "'[ ]'",
        ),
        ("<http://e/a> <http://e/p> [] .\n", "'[ ]'"),
        (
            "<http://e/a> <http://e/p> ( <http://e/o> ) .\n",
            "collections",
        ),
        ("<http://e/a> <http://e/p> 1 .\n", "must be quoted"),
        ("<http://e/a> <http://e/p> true .\n", "must be quoted"),
        ("<http://e/a> <http://e/p> 'x' .\n", "double quotes"),
        (
            "<http://e/a> <http://e/p> \"\"\"x\"\"\" .\n",
            "double quotes",
        ),
        ("<a> <http://e/p> <http://e/o> .\n", "absolute IRIs"),
        (
            "<< <http://e/a> <http://e/p> <http://e/o> >> <http://e/q> <http://e/r> .\n",
            "reified",
        ),
        (
            "<http://e/a> <http://e/p> <http://e/o> {| <http://e/q> <http://e/r> |} .\n",
            "annotations",
        ),
        (
            "<http://e/a> <http://e/p> <http://e/o> . <http://e/b> <http://e/p> <http://e/o> .\n",
            "one triple per line",
        ),
        (
            "<http://e/a> <http://e/p>\n  <http://e/o> .\n",
            "on one line",
        ),
    ];
    for (src, want) in cases {
        let r = nt(src);
        let msgs = messages(&r);
        assert!(msgs.iter().any(|m| m.contains(want)), "{src:?}: {msgs:?}");
        // The same text is fine as Turtle (apart from the prefixed name).
        if want != "prefixed names" {
            assert!(p(src).errors.is_empty(), "{src:?}: {:?}", p(src).errors);
        }
    }
}

#[test]
fn syntax_detection() {
    assert_eq!(Syntax::of(Some("trig"), "file:///a.ttl", ""), Syntax::TriG);
    assert_eq!(
        Syntax::of(Some("n-triples"), "file:///a", ""),
        Syntax::NTriples
    );
    assert_eq!(
        Syntax::of(Some("ntriples"), "file:///a", ""),
        Syntax::NTriples
    );
    assert_eq!(
        Syntax::of(Some("plaintext"), "file:///a.trig", ""),
        Syntax::TriG
    );
    assert_eq!(Syntax::of(None, "file:///x/a.nt", ""), Syntax::NTriples);
    assert_eq!(Syntax::of(None, "file:///x/a.nq", ""), Syntax::NQuads);
    assert_eq!(Syntax::of(Some("n-quads"), "file:///a", ""), Syntax::NQuads);
    assert_eq!(
        Syntax::of(Some("turtle"), "file:///a.nq", ""),
        Syntax::NQuads
    );
    assert!(Syntax::NQuads.is_line_based() && !Syntax::NQuads.has_prefixes());
    assert_eq!(
        Syntax::of(None, "file:///x.trig/a.turtle", ""),
        Syntax::Turtle
    );
    assert_eq!(Syntax::of(None, "untitled:Untitled-1", ""), Syntax::Turtle);
    // An editor that calls every RDF file "turtle".
    assert_eq!(
        Syntax::of(Some("turtle"), "file:///a.trig", ""),
        Syntax::TriG
    );
    assert_eq!(
        Syntax::of(Some("turtle"), "file:///a.nt", ""),
        Syntax::NTriples
    );
    assert_eq!(
        Syntax::of(Some("turtle"), "file:///a.ttl", ""),
        Syntax::Turtle
    );
    assert_eq!(Syntax::from_extension("C:\\x\\a.trig"), Some(Syntax::TriG));
    assert_eq!(Syntax::from_extension("/x/notes.txt"), None);
}

#[test]
fn jsonld_is_detected_by_extension_and_language_id_but_json_is_not() {
    assert_eq!(Syntax::from_extension("/x/a.jsonld"), Some(Syntax::JsonLd));
    assert_eq!(Syntax::from_language_id("jsonld"), Some(Syntax::JsonLd));
    assert_eq!(Syntax::from_extension("/x/a.json"), None);
    assert_eq!(Syntax::from_language_id("json"), None);
    // Editors that call `.jsonld` files `json` (VS Code) or `turtle`.
    for id in [None, Some("json"), Some("turtle"), Some("jsonld")] {
        assert_eq!(
            Syntax::of(id, "file:///a.jsonld", ""),
            Syntax::JsonLd,
            "{id:?}"
        );
    }
    assert_eq!(
        Syntax::of(Some("jsonld"), "untitled:Untitled-1", ""),
        Syntax::JsonLd
    );
    assert_eq!(
        Syntax::of(Some("json"), "file:///a.json", ""),
        Syntax::Turtle
    );
    assert!(!Syntax::JsonLd.is_line_based() && !Syntax::JsonLd.has_prefixes());
}

/// Parse on a 2 MiB stack, the smallest any host thread gets: a missing
/// depth guard aborts the process here instead of failing an assertion.
fn parse_small_stack(src: String) -> Vec<String> {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(move || messages(&p(&src)).into_iter().map(str::to_owned).collect())
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn deep_nesting_is_one_error_not_an_abort() {
    let n = 100_000;
    let cases = [
        ("{|", format!("<a> <p> <o> {}", "{| <p> <o> ".repeat(n))),
        ("[", format!("<a> <p> {}", "[ <p> ".repeat(n))),
        ("(", format!("<a> <p> {}", "( ".repeat(n))),
        ("<<", format!("{} <p> <o> .", "<< ".repeat(n))),
        ("<<(", format!("<a> <p> {}", "<<( <a> <p> ".repeat(n))),
    ];
    for (open, src) in cases {
        let errors = parse_small_stack(src);
        let deep: Vec<_> = errors
            .iter()
            .filter(|m| m.contains("Nested more than 256 levels deep"))
            .collect();
        assert_eq!(deep.len(), 1, "{open}: {errors:?}");
    }
}

#[test]
fn annotation_nesting_up_to_the_limit_parses() {
    let n = MAX_NESTING as usize;
    let src = format!(
        "<a> <p> <o> {}{}.",
        "{| <p> <o> ".repeat(n),
        "|} ".repeat(n)
    );
    let errors = parse_small_stack(src);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn backslash_newline_in_a_short_string_keeps_the_next_statement() {
    for nl in ["\n", "\r", "\r\n"] {
        let src = format!("<a> <p> \"x\\{nl}<b> <p> <o> .{nl}<c> <p> <o> .{nl}");
        let r = p(&src);
        assert!(
            r.errors
                .iter()
                .any(|e| e.message() == "Unterminated string"),
            "{nl:?}: {:?}",
            r.errors
        );
        let subjects: Vec<_> = r
            .statements
            .iter()
            .filter_map(|s| s.subject.map(|x| x.0.as_str()))
            .collect();
        assert!(subjects.contains(&"b"), "{nl:?}: {subjects:?}");
        assert!(subjects.contains(&"c"), "{nl:?}: {subjects:?}");
    }
}

#[test]
fn cr_only_file_keeps_its_triples_after_a_comment() {
    let src = "# header\r<a> <p> <o> .\r# mid\r<b> <p> <o> .\r";
    let r = p(src);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    assert_eq!(r.triples.len(), 2);
    assert_eq!(r.comments.len(), 2);
    // A statement missing its dot is still recovered at a CR line start.
    let r = p("<a> <p> <o>\r<b> <p> <o> .\r");
    assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
}

#[test]
fn string_values() {
    assert_eq!(string_value("\"a\\nb\""), "a\nb");
    assert_eq!(string_value("'''x''y'''"), "x''y");
    assert_eq!(string_value("\"\\u00e9\""), "é");
}

#[test]
fn a_base_direction_is_kept_apart_from_the_language_tag() {
    let r = p("<http://e/a> <http://e/p> \"x\"@AR--rtl, \"y\"@en--ltr, \"z\"@en, \"w\" .");
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let lits: Vec<_> = r
        .triples
        .iter()
        .map(|t| match &t.o {
            Node::Literal {
                datatype,
                lang,
                dir,
                ..
            } => (
                datatype.rsplit('#').next().unwrap().to_string(),
                lang.map(|l| l.to_string()),
                dir.map(|d| d.to_string()),
            ),
            other => panic!("{other:?}"),
        })
        .collect();
    let some = |s: &str| Some(s.to_string());
    assert_eq!(
        lits,
        vec![
            ("dirLangString".to_string(), some("ar"), some("rtl")),
            ("dirLangString".to_string(), some("en"), some("ltr")),
            ("langString".to_string(), some("en"), None),
            ("string".to_string(), None, None),
        ]
    );
    // An unknown direction is reported and the literal is a plain tagged string.
    let r = p("<http://e/a> <http://e/p> \"x\"@en--up .");
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(matches!(
        &r.triples[0].o,
        Node::Literal {
            lang: Some(_),
            dir: None,
            ..
        }
    ));
}

/// The same text at a line start `n` tokens in: the cost of recovering must
/// not grow with the length of the line.
#[test]
fn timing_recovery_inside_a_long_trig_line_is_linear() {
    let tokens = 200_000;
    let body = "<http://e/a> ".repeat(tokens);
    for src in [
        // An error, then 200k tokens that start statements but are mid-line.
        format!(
            "<http://e/g> {{ <http://e/s> <http://e/p> ) {body} }}\n<http://e/z> <http://e/p> <http://e/o> ."
        ),
        format!("<http://e/s> <http://e/p> ) {body}\n<http://e/z> <http://e/p> <http://e/o> ."),
    ] {
        let started = std::time::Instant::now();
        let r = trig(&src);
        let took = started.elapsed();
        let limit = if cfg!(debug_assertions) { 5.0 } else { 0.5 };
        assert!(took.as_secs_f64() < limit, "{took:?} for {tokens} tokens");
        assert!(!r.errors.is_empty());
        // Recovery still stops at the next line start.
        assert!(
            r.triples
                .iter()
                .any(|t| matches!(t.s, Node::Iri(i) if &*i == "http://e/z")),
            "{:?}",
            r.errors
        );
    }
}

/// Statements in one TriG graph block on one line, valid and without `.`
/// (recovery checks every token for a line start): 4x the statements is
/// 4x the work. Counted, not timed, so a loaded machine cannot fail it.
#[test]
fn one_line_graph_block_costs_work_linear_in_its_statements() {
    let work_for = |statement: &str, n: usize| {
        let src = format!(
            "@prefix ex: <http://e/> . ex:g {{ {}}}",
            statement.repeat(n)
        );
        WORK.with(|w| w.set(0));
        trig(&src);
        WORK.with(|w| w.get()) as f64
    };
    for statement in ["ex:a ex:b ex:c . ", "ex:a ex:b ex:c "] {
        let (small, large) = (work_for(statement, 2_000), work_for(statement, 8_000));
        let ratio = large / small;
        assert!(
            (3.9..4.1).contains(&ratio),
            "{statement:?}: {small} then {large} ({ratio:.2}x)"
        );
    }
}

#[test]
fn line_starts_are_found_from_the_gap_before_the_token() {
    // Indented statements in a block, a multi-line string and a comment
    // between tokens.
    let r = trig("<g> {\n  <a> <p> <o>\n  <b> <p> <o> .\n}\n");
    assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
    let r = p("<a> <p> \"\"\"x\n<b> <p> <o> .\n\"\"\" .\n<c> <p> <o> .");
    assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let r = p("<a> <p> <o> # c\n<b> <p> <o> .");
    assert_eq!(r.triples.len(), 2, "{:?}", r.errors);
}

#[test]
fn a_name_with_an_undeclared_prefix_marks_its_triples() {
    let r = p(
        "@prefix ex: <http://e/> .\nex:a ex:p nope:o , ex:o2 .\nnope:s ex:p ex:o .\nex:a ex:q [ nope:r 1 ] .\nex:a ex:p <<( ex:x ex:y nope:z )>> .\nex:b ex:p ex:o .",
    );
    let flags: Vec<_> = r.triples.iter().map(|t| t.unresolved).collect();
    // ex:a p nope:o | ex:a p ex:o2 | nope:s p o | _:b nope:r 1 | ex:a q _:b | ex:a p <<( )>> | ex:b p o
    assert_eq!(flags, [true, false, true, true, false, true, false]);
    assert_eq!(
        r.prefix_uses.iter().filter(|u| !u.resolved).count(),
        4,
        "{:?}",
        r.prefix_uses
    );
    // A name the document declares is resolved, however late.
    let r = p("ex:a ex:p ex:o .\n@prefix ex: <http://e/> .\nex:a ex:p ex:o .");
    let flags: Vec<_> = r.triples.iter().map(|t| t.unresolved).collect();
    assert_eq!(flags, [true, false]);
}

#[test]
fn a_triple_term_holds_no_reified_triple() {
    let ok = |src: &str| {
        let r = p(&format!("@prefix ex: <http://e/> .\n{src}"));
        assert!(r.errors.is_empty(), "{src}: {:?}", r.errors);
    };
    let bad = |src: &str| {
        let r = p(&format!("@prefix ex: <http://e/> .\n{src}"));
        assert!(
            r.errors.iter().any(|e| e
                .message()
                .contains("triple term <<( )>> cannot contain a reified")),
            "{src}: {:?}",
            r.errors
        );
    };
    // rtSubject and rtObject take reified triples and triple terms.
    ok("<< << ex:a ex:p ex:o >> ex:q << ex:b ex:p ex:o >> >> ex:r ex:s .");
    ok("<< ex:a ex:p <<( ex:b ex:p ex:o )>> >> ex:r ex:s .");
    ok("ex:a ex:p <<( ex:b ex:p <<( ex:c ex:p ex:o )>> )>> .");
    // ttSubject and ttObject do not.
    bad("ex:a ex:p <<( << ex:b ex:p ex:o >> ex:q ex:o )>> .");
    bad("ex:a ex:p <<( ex:b ex:q << ex:b ex:p ex:o >> )>> .");
    bad("ex:a ex:p <<( ex:b ex:q <<( ex:c ex:p << ex:d ex:p ex:o >> )>> )>> .");
    // Inside a reified triple, a triple term still holds none.
    bad("<< ex:a ex:p <<( ex:b ex:q << ex:c ex:p ex:o >> )>> >> ex:r ex:s .");
}

#[test]
fn absolute_iris_resolve_to_themselves_but_lose_dot_segments() {
    let r = parse_with_base(
        "<http://e/a/../b> <http://e/./p> <http://e/o#x> , <urn:a:b> , <http://e> , <http://e/a/b/> .",
        &PrefixMap::default(),
        Syntax::Turtle,
        Some("file:///w/d.ttl"),
    );
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    let iris: Vec<_> = r.refs.iter().map(|r| r.iri.to_string()).collect();
    assert_eq!(
        iris,
        [
            "http://e/b",
            "http://e/p",
            "http://e/o#x",
            "urn:a:b",
            "http://e",
            "http://e/a/b/"
        ]
    );
    // An invalid absolute IRI is still reported once.
    let r = parse_with_base(
        "<http://e/%zz> <http://e/p> <http://e/o> .",
        &PrefixMap::default(),
        Syntax::Turtle,
        Some("file:///w/d.ttl"),
    );
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
}

#[test]
fn a_graph_only_parse_has_the_same_graph_and_nothing_else() {
    let cases = [
        (
            Syntax::Turtle,
            "@prefix ex: <http://e/> . # c1\nex:a ex:p [ ex:q ( 1 2 ) ] ; a ex:C . # c2\n<< ex:a ex:p ex:b >> ex:r nope:x .\n",
        ),
        (
            Syntax::TriG,
            "@prefix ex: <http://e/> .\nGRAPH ex:g { ex:a ex:p ex:b } # c\n{ ex:c ex:p ex:d . }\nex:broken ex:p .\n",
        ),
        (
            Syntax::NTriples,
            "<http://e/a> <http://e/p> \"x\" . # c\n<a> <http://e/p> <http://e/b> .\n",
        ),
        (
            Syntax::NQuads,
            "<http://e/a> <http://e/p> <http://e/b> <http://e/g> .\n",
        ),
        (Syntax::JsonLd, "{}"),
    ];
    let ext: PrefixMap = [("ext".to_string(), "http://ext/".to_string())]
        .into_iter()
        .collect();
    for (syntax, text) in cases {
        let full = parse_with_base(text, &ext, syntax, Some("file:///w/a"));
        let (graph, comments) = Parsed::graph_only(text, &ext, syntax, Some("file:///w/a"), 3);
        let quads = |p: &Parsed| -> Vec<String> {
            p.triples
                .iter()
                .map(|t| {
                    format!(
                        "{:?} {:?} {:?} {:?} {}",
                        t.s, t.p, t.o, t.graph, t.unresolved
                    )
                })
                .collect()
        };
        let errors = |p: &Parsed| -> Vec<String> {
            p.errors
                .iter()
                .map(|e| format!("{:?} {}", e.span, e.message()))
                .collect()
        };
        assert_eq!(quads(&graph), quads(&full), "{syntax:?}");
        assert_eq!(errors(&graph), errors(&full), "{syntax:?}");
        let names = |p: &Parsed| -> Vec<_> { p.graphs.iter().map(|g| g.name).collect() };
        assert_eq!(names(&graph), names(&full), "{syntax:?}");
        assert_eq!(comments, full.comments.len(), "{syntax:?}");
        assert!(
            graph.tokens.is_empty()
                && graph.refs.is_empty()
                && graph.prefix_uses.is_empty()
                && graph.comments.is_empty(),
            "{syntax:?}"
        );
        let keys = |ts: &[Token]| -> Vec<_> {
            ts.iter()
                .map(|t| (t.kind, t.start, t.end, t.error()))
                .collect()
        };
        assert_eq!(keys(&syntax.tokens(text)), keys(&full.tokens), "{syntax:?}");
    }
}
