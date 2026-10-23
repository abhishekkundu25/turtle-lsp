use super::*;
use structure::{ClauseKind, Position};

const EX: &str = "PREFIX ex: <https://query.example.org/def/>\n";

fn doc(text: &str) -> Document {
    Document::parse(text, &PrefixMap::default(), None)
}

fn at(text: &str, s: Span) -> &str {
    &text[s.start as usize..s.end as usize]
}

/// (kind, text) of every token.
fn lex(text: &str) -> Vec<(TokenKind, &str)> {
    lexer::tokenize(text)
        .into_iter()
        .map(|t| (t.kind, t.text(text)))
        .collect()
}

/// (code, level, covered text) of every diagnostic.
fn diags(text: &str) -> Vec<(&'static str, Level, String)> {
    diags_with(text, &PrefixMap::default())
}

fn diags_with(text: &str, prefixes: &PrefixMap) -> Vec<(&'static str, Level, String)> {
    Document::parse(text, prefixes, None)
        .diagnostics()
        .iter()
        .map(|d| (d.code, d.level, at(text, d.span).to_owned()))
        .collect()
}

fn codes(text: &str) -> Vec<&'static str> {
    diags(text).into_iter().map(|d| d.0).collect()
}

// --- lexer ------------------------------------------------------------------

#[test]
fn keywords_are_case_insensitive_and_words_are_classified() {
    use TokenKind::*;
    assert_eq!(
        lex("select DISTINCT ?x Where { ?x a ex:C FILTER(isBlank(?x) || true) } Limit 1"),
        vec![
            (Keyword, "select"),
            (Keyword, "DISTINCT"),
            (Var, "?x"),
            (Keyword, "Where"),
            (Punct, "{"),
            (Var, "?x"),
            (A, "a"),
            (PName, "ex:C"),
            (Keyword, "FILTER"),
            (Punct, "("),
            (Function, "isBlank"),
            (Punct, "("),
            (Var, "?x"),
            (Punct, ")"),
            (Punct, "||"),
            (Boolean, "true"),
            (Punct, ")"),
            (Punct, "}"),
            (Keyword, "Limit"),
            (Number, "1"),
        ]
    );
    assert_eq!(lexer::keyword("where"), Some("WHERE"));
    assert_eq!(lexer::function("isblank"), Some("isBLANK"));
    assert_eq!(lexer::function("LANGMATCHES"), Some("langMatches"));
    assert_eq!(lexer::function("group_concat"), Some("GROUP_CONCAT"));
    assert!(lexer::is_aggregate("count"));
    // A word that is no keyword is an error token, and lexing goes on.
    assert_eq!(lex("FILTR ?x"), vec![(Error, "FILTR"), (Var, "?x")]);
}

#[test]
fn angle_brackets_are_iris_or_operators() {
    use TokenKind::*;
    assert_eq!(
        lex("FILTER(?a < 3 && ?b <= ?c && ?d >= <http://e/x#y>)"),
        vec![
            (Keyword, "FILTER"),
            (Punct, "("),
            (Var, "?a"),
            (Punct, "<"),
            (Number, "3"),
            (Punct, "&&"),
            (Var, "?b"),
            (Punct, "<="),
            (Var, "?c"),
            (Punct, "&&"),
            (Var, "?d"),
            (Punct, ">="),
            (Iri, "<http://e/x#y>"),
            (Punct, ")"),
        ]
    );
    assert_eq!(
        lex("<<( ?s ?p <o> )>> << ?s ?p ?o ~ ?r >> {| ?p ?o |}"),
        vec![
            (Punct, "<<("),
            (Var, "?s"),
            (Var, "?p"),
            (Iri, "<o>"),
            (Punct, ")>>"),
            (Punct, "<<"),
            (Var, "?s"),
            (Var, "?p"),
            (Var, "?o"),
            (Punct, "~"),
            (Var, "?r"),
            (Punct, ">>"),
            (Punct, "{|"),
            (Var, "?p"),
            (Var, "?o"),
            (Punct, "|}"),
        ]
    );
}

#[test]
fn lexing_from_a_kept_state_gives_the_tokens_of_the_whole_text() {
    let part = "{ ?s<http://e/p>?o FILTER(?o)<http://e/s> ?p ( ?o<http://e/l> ) \
                FILTER(?a<=ex:b&&?c>?a) BIND(ex:f(?x)<3 AS ?y) }\n";
    let text = format!(
        "SELECT ?x (<http://e/f>(?x)<3 AS ?y) {{ {} }} ORDER BY ?x <http://e/f>(?x<?y)",
        part.repeat(60)
    );
    let (all, states) = lexer::tokenize_with_states(&text);
    assert!(states.len() > 10, "{} states", states.len());
    for (at, state) in states {
        let rest = all.iter().filter(|t| t.span.start >= at);
        let want: Vec<_> = rest
            .map(|t| (t.kind, Span::new(t.span.start - at, t.span.end - at)))
            .collect();
        assert_eq!(
            lexer::resume(&text[at as usize..], state).0,
            want,
            "from {at}"
        );
    }
}

#[test]
fn a_less_than_after_an_operand_is_the_operator() {
    use TokenKind::*;
    let tail = |q: &'static str| {
        let all = lex(q);
        all[2..all.len() - 1].to_vec()
    };
    assert_eq!(
        tail("FILTER(?a<=ex:b&&?c>?a)"),
        vec![
            (Var, "?a"),
            (Punct, "<="),
            (PName, "ex:b"),
            (Punct, "&&"),
            (Var, "?c"),
            (Punct, ">"),
            (Var, "?a"),
        ]
    );
    assert_eq!(
        tail("FILTER(?a<<http://e/b>)"),
        vec![(Var, "?a"), (Punct, "<"), (Iri, "<http://e/b>")]
    );
    assert_eq!(
        tail("FILTER(?x < ?y && ?y > ?z)"),
        vec![
            (Var, "?x"),
            (Punct, "<"),
            (Var, "?y"),
            (Punct, "&&"),
            (Var, "?y"),
            (Punct, ">"),
            (Var, "?z"),
        ]
    );
    let iris = |q: &str| lex(q).into_iter().filter(|t| t.0 == Iri).count();
    assert_eq!(iris("{ <http://e/s> <http://e/p> <http://e/o> }"), 3);
    assert_eq!(iris("{ <http://e/s><http://e/p><http://e/o> }"), 3);
    assert_eq!(
        iris("{ ?s<http://e/p>?o FILTER(?o)<http://e/s> ?p ( ?o<http://e/l> ) }"),
        3
    );
    assert_eq!(
        iris("SELECT ?x (<http://e/f>(?x)<3 AS ?y) { } ORDER BY ?x <http://e/f>(?x<?y)"),
        2
    );
    assert_eq!(
        iris("SELECT (EXISTS { ?s ?p (?o<http://e/l>) } AS ?e) { }"),
        1
    );
    assert_eq!(
        iris("ASK { << ?s ?p ?o >> ?q <<( ?s ?p <http://e/o> )>> }"),
        1
    );
}

#[test]
fn strings_comments_and_numbers() {
    use TokenKind::*;
    let text =
        "\"a # b\" 'c' \"\"\"x\n\"y\" \"\"\"\" '''z''' \"e\\\"q\" # { ?x\n1. .5 1e5 1.e5 -2 +3.0";
    assert_eq!(
        lex(text),
        vec![
            (String, "\"a # b\""),
            (String, "'c'"),
            (String, "\"\"\"x\n\"y\" \"\"\"\""),
            (String, "'''z'''"),
            (String, "\"e\\\"q\""),
            (Comment, "# { ?x"),
            (Number, "1"),
            (Punct, "."),
            (Number, ".5"),
            (Number, "1e5"),
            (Number, "1.e5"),
            (Punct, "-"),
            (Number, "2"),
            (Punct, "+"),
            (Number, "3.0"),
        ]
    );
    // An unterminated string ends at the line; the next line lexes again.
    assert_eq!(lex("\"open\n?x"), vec![(Error, "\"open"), (Var, "?x")]);
    // `#` inside an IRI is not a comment.
    assert_eq!(
        lex("<http://e/#a> ?x"),
        vec![(Iri, "<http://e/#a>"), (Var, "?x")]
    );
}

#[test]
fn names_tags_blank_nodes_and_variables() {
    use TokenKind::*;
    assert_eq!(
        lex("ex:a.b ex:c. :x ex: ex:a\\,b ex:%41 a.b:c _:b1 \"x\"@en-GB \"y\"@ar--rtl ?x $y ? $ é"),
        vec![
            (PName, "ex:a.b"),
            (PName, "ex:c"),
            (Punct, "."),
            (PName, ":x"),
            (PName, "ex:"),
            (PName, "ex:a\\,b"),
            (PName, "ex:%41"),
            (PName, "a.b:c"),
            (BlankNode, "_:b1"),
            (String, "\"x\""),
            (LangTag, "@en-GB"),
            (String, "\"y\""),
            (LangTag, "@ar--rtl"),
            (Var, "?x"),
            (Var, "$y"),
            (Punct, "?"),
            (Error, "$"),
            (Error, "é"),
        ]
    );
    // `PREFIX:` before an IRI is the keyword and the empty prefix.
    assert_eq!(
        lex("PREFIX:<http://e/>"),
        vec![(Keyword, "PREFIX"), (PName, ":"), (Iri, "<http://e/>")]
    );
    // After the keyword it is the name declared.
    assert_eq!(
        lex("PREFIX PREFIX:<http://e/>"),
        vec![
            (Keyword, "PREFIX"),
            (PName, "PREFIX:"),
            (Iri, "<http://e/>")
        ]
    );
    assert_eq!(lex("select:x"), vec![(PName, "select:x")]);
}

#[test]
fn tokens_cover_the_text_in_order_on_character_boundaries() {
    let text = "SELECT ?é { ?s <ö> \"日本\"@ja ; ex:ü 😀 \u{200C}x } # ❤\n";
    let mut prev = 0;
    for t in lexer::tokenize(text) {
        assert!(t.span.start >= prev && t.span.start < t.span.end);
        assert!(text.is_char_boundary(t.span.start as usize));
        assert!(text.is_char_boundary(t.span.end as usize));
        prev = t.span.end;
    }
}

// --- structure --------------------------------------------------------------

#[test]
fn prologue_declarations_have_spans() {
    let text =
        "BASE <http://b/>\nPREFIX ex: <rel/>\nPREFIX : <http://e/>\nVERSION \"1.2\"\nSELECT * { }";
    let d = doc(text);
    let st = d.structure();
    assert_eq!(st.bases.len(), 1);
    assert_eq!(at(text, st.bases[0].iri_span), "<http://b/>");
    let ex = &st.prefixes[0];
    assert_eq!(ex.name, "ex");
    assert_eq!(ex.iri, "rel/");
    assert_eq!(ex.resolved.as_deref(), Some("http://b/rel/"));
    assert_eq!(at(text, ex.span), "PREFIX ex: <rel/>");
    assert_eq!(at(text, ex.name_span), "ex:");
    assert_eq!(at(text, ex.iri_span), "<rel/>");
    assert_eq!(st.prefixes[1].name, "");
    assert_eq!(at(text, st.versions[0].value_span), "\"1.2\"");
    assert_eq!(at(text, st.versions[0].span), "VERSION \"1.2\"");
}

#[test]
fn query_or_update_by_the_first_keyword() {
    assert!(!doc("SELECT * {}").is_update());
    assert!(!doc(&format!("{EX}ASK {{}}")).is_update());
    assert!(doc(&format!("{EX}INSERT DATA {{ ex:a ex:b 1 }}")).is_update());
    assert!(doc("with <http://g/> delete { ?s ?p ?o } where { ?s ?p ?o }").is_update());
    // Nothing but a prologue (or nothing at all) is an empty update.
    assert!(doc("").is_update());
    assert!(doc(EX).is_update());
    assert!(diags("").is_empty());
    // Something else is a broken query.
    assert!(!doc("ex:a ex:b ex:c .").is_update());
}

#[test]
fn update_operations_and_their_clauses() {
    let text = format!(
        "{EX}INSERT DATA {{ ex:a ex:b 1 }} ;\nWITH <http://g/> DELETE {{ ?s ex:p ?o }} INSERT {{ ?s ex:q ?o }} USING <http://u/> WHERE {{ ?s ex:p ?o }} ;\n\
         PREFIX f: <http://f/>\nDELETE WHERE {{ ?s f:p ?o }} ;\nLOAD SILENT <http://l/> INTO GRAPH <http://g/> ;\nCLEAR ALL"
    );
    let d = doc(&text);
    let st = d.structure();
    let kinds: Vec<_> = st.operations.iter().map(|o| o.kind).collect();
    use OperationKind::*;
    assert_eq!(kinds, vec![InsertData, Modify, DeleteWhere, Load, Clear]);
    assert_eq!(
        at(&text, st.operations[0].span),
        "INSERT DATA { ex:a ex:b 1 }"
    );
    let clauses: Vec<_> = st.operations[1]
        .clauses
        .iter()
        .map(|c| (c.kind, at(&text, c.span)))
        .collect();
    use ClauseKind::*;
    assert_eq!(
        clauses,
        vec![
            (With, "WITH <http://g/>"),
            (Delete, "DELETE { ?s ex:p ?o }"),
            (Insert, "INSERT { ?s ex:q ?o }"),
            (Using, "USING <http://u/>"),
            (Where, "WHERE { ?s ex:p ?o }"),
        ]
    );
    // Each operation is its own scope.
    let s: Vec<_> = st
        .vars
        .iter()
        .filter(|v| v.name == "s")
        .map(|v| v.scope)
        .collect();
    assert_ne!(s.first(), s.last());
    let groups: Vec<_> = st.groups.iter().map(|g| g.kind).collect();
    use GroupKind as G;
    assert_eq!(
        groups,
        vec![G::Data, G::Template, G::Template, G::Where, G::Where]
    );
    assert!(diags(&text).is_empty(), "{:?}", diags(&text));
}

#[test]
fn query_clauses_groups_and_nesting() {
    let text = format!(
        "{EX}SELECT ?x WHERE {{\n  ?x a ex:C .\n  OPTIONAL {{ ?x ex:p ?y }}\n  {{ ?x ex:q 1 }} UNION {{ ?x ex:q 2 }}\n  MINUS {{ ?x ex:r 3 }}\n  GRAPH ?g {{ ?x ex:s ?z }}\n  FILTER NOT EXISTS {{ ?x ex:t 4 }}\n  VALUES ?v {{ 1 2 }}\n}}\nGROUP BY ?x HAVING (COUNT(?y) > 1) ORDER BY DESC(?x) LIMIT 5 OFFSET 2"
    );
    let d = doc(&text);
    let st = d.structure();
    let op = &st.operations[0];
    use ClauseKind::*;
    let kinds: Vec<_> = op.clauses.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![Form, Where, GroupBy, Having, OrderBy, Limit, Offset]
    );
    assert_eq!(at(&text, op.clauses[0].span), "SELECT ?x");
    assert_eq!(at(&text, op.clauses[4].span), "ORDER BY DESC(?x)");
    use GroupKind as G;
    let groups: Vec<_> = st.groups.iter().map(|g| (g.kind, g.depth)).collect();
    assert_eq!(
        groups,
        vec![
            (G::Where, 0),
            (G::Optional, 1),
            (G::Group, 1),
            (G::Union, 1),
            (G::Minus, 1),
            (G::Graph, 1),
            (G::Exists, 1),
            (G::Values, 1),
        ]
    );
    assert_eq!(at(&text, st.groups[3].keyword.unwrap()), "UNION");
    assert!(st.groups.iter().all(|g| g.close.is_some()));
    // group_at finds the innermost group.
    let inner = text.find("ex:p ?y").unwrap() as u32;
    assert_eq!(st.group_at(inner), Some(1));
}

#[test]
fn an_unclosed_group_runs_to_the_last_token() {
    let text = format!("{EX}SELECT ?x WHERE {{\n  ?x a ex:C .\n  OPTIONAL {{ ?x ex:p ?y \n");
    let d = doc(&text);
    let st = d.structure();
    assert_eq!(st.groups.len(), 2);
    assert!(st.groups.iter().all(|g| g.close.is_none()));
    assert_eq!(st.groups[0].span.end as usize, text.trim_end().len());
    // Still: every variable and IRI.
    assert_eq!(st.vars.len(), 4);
    assert_eq!(st.refs.len(), 3);
}

#[test]
fn a_subquery_is_its_own_scope_except_what_it_projects() {
    let text = "SELECT ?a WHERE {\n  ?a <p> ?b .\n  { SELECT ?a (COUNT(?c) AS ?n) WHERE { ?a <q> ?c . ?a <r> ?b } }\n  ?n <s> ?c .\n}";
    let d = doc(text);
    let st = d.structure();
    let scope_of = |name: &str, nth: usize| {
        st.vars
            .iter()
            .filter(|v| v.name == name)
            .nth(nth)
            .unwrap()
            .scope
    };
    let top = st.operations[0].scope;
    // ?a is projected: one variable throughout.
    assert!((0..5).all(|i| scope_of("a", i) == top));
    // ?n is projected through an expression.
    assert_eq!(scope_of("n", 0), top);
    assert_eq!(scope_of("n", 1), top);
    // ?b and ?c inside the subquery are not the outer ones.
    assert_eq!(scope_of("b", 0), top);
    assert_ne!(scope_of("b", 1), top);
    assert_ne!(scope_of("c", 0), top);
    assert_eq!(scope_of("c", 2), top);
    let sub = &st.selects[1];
    assert_eq!(sub.items.len(), 2);
    assert_eq!(at(text, sub.items[1].expr.unwrap()), "(COUNT(?c) AS ?n)");
    assert_eq!(
        st.groups[sub.group.unwrap() as usize].kind,
        GroupKind::Subquery
    );
    assert_eq!(st.scopes[sub.scope as usize].parent, Some(top));
    // Occurrences in scope from any one of them.
    let b = text.find("?b").unwrap() as u32;
    assert_eq!(st.var_occurrences(b).len(), 1);
    let a = text.rfind("?a").unwrap() as u32;
    assert_eq!(st.var_occurrences(a).len(), 5);
    // `SELECT *` shares every variable.
    let star = doc("SELECT ?x { { SELECT * { ?x <p> ?y } } ?y <q> 1 }");
    let st = star.structure();
    assert!(st.vars.iter().all(|v| v.scope == st.operations[0].scope));
}

#[test]
fn iris_have_roles_and_resolve() {
    let text = format!(
        "BASE <http://b/>\n{EX}SELECT * FROM <g> WHERE {{\n  ?s a ex:C ; ex:p/ex:q|^<r> \"x\"^^ex:dt , [ ex:in ex:o ] .\n  << ?s ex:k ex:v >> ex:m 1 .\n  FILTER(ex:fn(?s) && ?s != ex:val)\n  GRAPH ex:g {{ }}\n  ?s ex:u foo:bar .\n}}"
    );
    let d = doc(&text);
    let st = d.structure();
    let role = |name: &str| {
        let r = st
            .refs
            .iter()
            .find(|r| at(&text, r.span) == name)
            .unwrap_or_else(|| panic!("{name}"));
        (r.role, r.quoted)
    };
    use TermRole::*;
    assert_eq!(role("<g>"), (Graph, false));
    assert_eq!(role("a"), (Predicate, false));
    assert_eq!(role("ex:C"), (Object, false));
    assert_eq!(role("ex:p"), (Predicate, false));
    assert_eq!(role("ex:q"), (Predicate, false));
    assert_eq!(role("<r>"), (Predicate, false));
    assert_eq!(role("ex:dt"), (Datatype, false));
    assert_eq!(role("ex:in"), (Predicate, false));
    assert_eq!(role("ex:o"), (Object, false));
    assert_eq!(role("ex:k"), (Predicate, true));
    assert_eq!(role("ex:v"), (Object, true));
    assert_eq!(role("ex:m"), (Predicate, false));
    assert_eq!(role("ex:fn"), (Function, false));
    assert_eq!(role("ex:val"), (Value, false));
    assert_eq!(role("ex:g"), (Graph, false));
    let r = |name: &str| st.refs.iter().find(|r| at(&text, r.span) == name).unwrap();
    assert_eq!(r("a").iri.as_deref(), Some(crate::syntax::RDF_TYPE));
    assert_eq!(r("a").form, Form::A);
    assert_eq!(r("<r>").iri.as_deref(), Some("http://b/r"));
    assert_eq!(
        r("ex:C").iri.as_deref(),
        Some("https://query.example.org/def/C")
    );
    assert_eq!(r("ex:C").prefix, Some(PrefixSource::Declared(0)));
    assert_eq!(at(&text, r("ex:C").prefix_span().unwrap()), "ex:");
    assert_eq!(r("foo:bar").prefix, Some(PrefixSource::Undeclared));
    assert_eq!(r("foo:bar").iri, None);
    // Predicates and objects know their subject.
    assert_eq!(at(&text, r("ex:C").subject.unwrap()), "?s");
    assert_eq!(at(&text, r("ex:o").subject.unwrap()), "[");
}

#[test]
fn external_prefixes_resolve_and_escapes_are_undone() {
    let mut pm = PrefixMap::default();
    pm.insert("p".into(), "http://p/".into());
    let text = "SELECT * { ?s p:a\\,b <http://e/\\u0041> }";
    let d = Document::parse(text, &pm, None);
    let st = d.structure();
    assert_eq!(st.refs[0].prefix, Some(PrefixSource::External));
    assert_eq!(st.refs[0].iri.as_deref(), Some("http://p/a,b"));
    assert_eq!(st.refs[1].iri.as_deref(), Some("http://e/A"));
    assert!(d.diagnostics().is_empty(), "{:?}", d.diagnostics());
}

#[test]
fn triples_are_recorded_as_far_as_written() {
    let text = "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT * { ?p a foaf:Person ; foaf: }";
    let d = doc(text);
    let st = d.structure();
    let t: Vec<_> = st
        .triples
        .iter()
        .map(|t| {
            (
                at(text, t.subject),
                t.predicate.map(|p| at(text, p)),
                t.object.map(|o| at(text, o)),
            )
        })
        .collect();
    assert_eq!(
        t,
        vec![
            ("?p", Some("a"), Some("foaf:Person")),
            ("?p", Some("foaf:"), None),
        ]
    );
}

#[test]
fn variable_roles() {
    let text = "SELECT ?a (STR(?b) AS ?c) WHERE { ?a <p> ?d . BIND(?d AS ?e) VALUES ?f { 1 } GRAPH ?g { } FILTER(?h) }";
    let d = doc(text);
    let role = |n: &str| {
        d.structure()
            .vars
            .iter()
            .find(|v| v.name == n)
            .unwrap()
            .role
    };
    use VarRole::*;
    assert_eq!(role("a"), Projected);
    assert_eq!(role("b"), Expression);
    assert_eq!(role("c"), Assigned);
    assert_eq!(role("d"), Pattern(Position::Object));
    assert_eq!(role("e"), Assigned);
    assert_eq!(role("f"), Values);
    assert_eq!(role("g"), Graph);
    assert_eq!(role("h"), Expression);
}

// --- diagnostics --------------------------------------------------------------

#[test]
fn a_syntax_error_marks_the_token_where_the_query_stops_making_sense() {
    let text = format!("{EX}SELECT ?x WHERE {{ ?x ex:name }}\n");
    let d = doc(&text);
    let e = &d.diagnostics()[0];
    assert_eq!((e.code, e.level), ("syntax", Level::Error));
    assert_eq!(e.span.start as usize, text.find("}\n").unwrap());
    assert!(e.message.contains("expected"), "{}", e.message);
    assert_eq!(d.diagnostics().len(), 1);
    // A keyword the parser would have read past is no excuse: `?x a }` is
    // wrong at the `}`, not at `?x`.
    let text = "SELECT ?x WHERE { ?x a }\n";
    assert_eq!(diags(text), vec![("syntax", Level::Error, "}".to_owned())]);
    // A misspelled keyword is marked as a whole.
    let text = format!("{EX}SELECT ?x WHERE {{\n  ?x a ex:Person .\n  FILTR(?x != ex:acme)\n}}\n");
    assert_eq!(
        diags(&text),
        vec![("syntax", Level::Error, "FILTR".to_owned())]
    );
    // The end of the text: the last token.
    let text = format!("{EX}SELECT ?x WHERE {{\n  ?x a ex:Person .\n");
    assert_eq!(diags(&text), vec![("syntax", Level::Error, ".".to_owned())]);
    // Comments do not move positions.
    let text = "SELECT * WHERE { # note\n  ?s ?p # more\n}";
    assert_eq!(diags(text), vec![("syntax", Level::Error, "}".to_owned())]);
}

#[test]
fn every_undeclared_prefix_is_reported_and_not_as_a_syntax_error() {
    let text = "SELECT * WHERE {\n  ?s foaf:name ?n ;\n    dc:title ?t .\n}\n";
    let d = doc(text);
    let u: Vec<_> = d
        .diagnostics()
        .iter()
        .map(|d| (d.code, d.level, at(text, d.span), d.message.as_str()))
        .collect();
    assert_eq!(
        u,
        vec![
            (
                "undeclared-prefix",
                Level::Error,
                "foaf:name",
                "Undeclared prefix 'foaf:'"
            ),
            (
                "undeclared-prefix",
                Level::Error,
                "dc:title",
                "Undeclared prefix 'dc:'"
            ),
        ]
    );
    // A syntax error after one is still found.
    let text = "SELECT * WHERE {\n  ?s foaf:name ?n .\n  ?s ?p\n}\n";
    assert_eq!(codes(text), vec!["undeclared-prefix", "syntax"]);
    // The empty prefix.
    assert_eq!(diags("SELECT * { ?s :p ?o }")[0].2, ":p".to_owned());
    // Prefixes the document does not declare but the project does.
    let mut pm = PrefixMap::default();
    pm.insert("foaf".into(), "http://xmlns.com/foaf/0.1/".into());
    assert!(diags_with("SELECT * { ?s foaf:name ?n }", &pm).is_empty());
    // In an update, a prefix is declared for what follows its declaration.
    // (`spargebra` takes a prologue only at the start; the grammar allows
    // one before any operation.)
    let text = "INSERT DATA { f:a f:b 1 } ;\nPREFIX f: <http://f/>\nINSERT DATA { f:c f:d 2 }";
    assert_eq!(
        diags(text),
        vec![
            ("undeclared-prefix", Level::Error, "f:a".to_owned()),
            ("undeclared-prefix", Level::Error, "f:b".to_owned()),
        ]
    );
    // Syntax errors after such a prologue are still found where they are.
    let text = "INSERT DATA { <a:a> <a:b> 1 } ;\nPREFIX f: <http://f/>\nINSERT DATA { f:c f:d }";
    assert_eq!(diags(text), vec![("syntax", Level::Error, "}".to_owned())]);
}

#[test]
fn unused_duplicate_and_redefined_prefixes() {
    let text = format!(
        "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n{EX}SELECT * WHERE {{ ?s ex:name ?o }}\n"
    );
    let d = doc(&text);
    let u = &d.diagnostics()[0];
    assert_eq!((u.code, u.level), ("unused-prefix", Level::Hint));
    assert_eq!(
        at(&text, u.span),
        "PREFIX foaf: <http://xmlns.com/foaf/0.1/>"
    );
    assert_eq!(u.message, "Prefix 'foaf:' is never used");
    let text = format!("{EX}{EX}SELECT * WHERE {{ ?s ex:name ?o }}\n");
    let d = doc(&text);
    assert_eq!(d.diagnostics().len(), 1);
    let dup = &d.diagnostics()[0];
    assert_eq!((dup.code, dup.level), ("duplicate-prefix", Level::Hint));
    assert_eq!(dup.span.start as usize, EX.len());
    assert_eq!(dup.message, "Prefix 'ex:' is declared twice");
    let text = format!(
        "{EX}PREFIX ex: <https://other.example.org/>\nSELECT * WHERE {{ ?s ex:name ?o }}\n"
    );
    let d = doc(&text);
    assert_eq!(d.diagnostics().len(), 1);
    let re = &d.diagnostics()[0];
    assert_eq!((re.code, re.level), ("redefined-prefix", Level::Warning));
    assert_eq!(at(&text, re.span), "ex:");
    assert_eq!(re.span.start as usize, EX.len() + 7);
}

#[test]
fn projected_variables_the_where_never_binds() {
    let text = format!("{EX}SELECT ?person ?missing WHERE {{ ?person ex:name ?name }}\n");
    let d = doc(&text);
    let u = &d.diagnostics()[0];
    assert_eq!((u.code, u.level), ("unbound-projection", Level::Warning));
    assert_eq!(at(&text, u.span), "?missing");
    assert!(u.message.contains("?missing"));
    assert_eq!(d.diagnostics().len(), 1);
    // Bound by BIND, VALUES (inside or after), OPTIONAL, a subquery's
    // projection, GRAPH, GROUP BY ... AS.
    let text = format!(
        "{EX}SELECT ?a ?b ?c ?d ?f ?g ?h ?k WHERE {{\n  ?p ex:name ?n .\n  BIND(STRLEN(?n) AS ?a)\n  VALUES ?b {{ 1 2 }}\n  OPTIONAL {{ ?p ex:worksFor ?c }}\n  {{ SELECT ?d WHERE {{ ?d a ex:Person }} }}\n  GRAPH ?g {{ ?p ex:name ?f }}\n}} GROUP BY ?a ?b ?c ?d ?f ?g (STR(?p) AS ?h) VALUES ?k {{ 1 }}\n"
    );
    assert!(diags(&text).is_empty(), "{:?}", diags(&text));
    // Not `*`, not expressions.
    assert!(diags(&format!("{EX}SELECT * {{ ?s ex:p ?o }}")).is_empty());
    let agg = format!(
        "{EX}SELECT ?org (COUNT(?p) AS ?n) WHERE {{ ?p ex:worksFor ?org }} GROUP BY ?org\n"
    );
    assert!(diags(&agg).is_empty(), "{:?}", diags(&agg));
    // FILTER, MINUS and EXISTS bind nothing; a subquery's unprojected
    // variable is not the outer one.
    let text = format!(
        "{EX}SELECT ?f ?m ?e ?s WHERE {{ ?x ex:p ?y FILTER(?f) MINUS {{ ?x ex:q ?m }} FILTER EXISTS {{ ?x ex:r ?e }} {{ SELECT ?x {{ ?x ex:t ?s }} }} }}"
    );
    let found: Vec<_> = diags(&text).into_iter().map(|d| d.2).collect();
    assert_eq!(found, vec!["?f", "?m", "?e", "?s"]);
    // A subquery's own projection is checked too.
    let text = format!("{EX}SELECT * WHERE {{ {{ SELECT ?z WHERE {{ ?x ex:p ?y }} }} }}");
    assert_eq!(
        diags(&text),
        vec![("unbound-projection", Level::Warning, "?z".to_owned())]
    );
}

#[test]
fn errors_the_structure_catches_that_spargebra_does_not() {
    let pfx = "PREFIX : <http://example.com/ns#>\n";
    for (body, bad) in [
        (
            "SELECT * { VALUES ?x { <<( <<(:s :p :o )>> :q :z )>> } }",
            "<<(",
        ),
        (
            "SELECT * { BIND( <<( \"literal\" :q :z )>> AS ?X ) }",
            "\"literal\"",
        ),
        ("SELECT (SUM(COUNT(*)) AS ?c) WHERE {}", "COUNT"),
    ] {
        let text = format!("{pfx}{body}");
        let d: Vec<_> = diags(&text)
            .into_iter()
            .filter(|d| d.1 == Level::Error)
            .collect();
        assert_eq!(d.len(), 1, "{body}: {d:?}");
        assert_eq!((d[0].0, d[0].1), ("syntax", Level::Error));
        assert_eq!(d[0].2, bad);
    }
    // In a pattern a triple term may be a triple term's subject.
    let text = format!("{pfx}SELECT * {{ <<( <<(:s :p :o )>> :q :z )>> :r 1 }}");
    assert!(diags(&text).is_empty(), "{:?}", diags(&text));
}

#[test]
fn a_blank_node_shared_by_two_data_blocks_is_marked() {
    let text = "INSERT DATA { _:b <p:x> 1 } ; INSERT DATA { _:b <p:x> 2 }";
    let d = diags(text);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].0, "syntax");
    assert_eq!(d[0].2, "_:b");
}

#[test]
fn diagnostics_stay_on_character_boundaries() {
    for text in [
        "SELECT * { ?s ?p \u{FFFD}:o }",
        "SELECT * { ?s ?p é }",
        "SELECT * { ?s ?p \"日本",
        "PREFIX 日本: <http://e/>\nSELECT * { ?s 日本:p 日本 }",
        // A dot in the name is escaped for the parser, which then reports
        // the emoji one byte further on than it is in the text.
        "PREFIX ex: <http://e/>\nSELECT * { ?s ex:a.😀 ?o }",
        "PREFIX ex: <http://e/>\nSELECT * { ?s ex:a.é.😀 }",
    ] {
        for d in doc(text).diagnostics() {
            assert!(text.is_char_boundary(d.span.start as usize), "{text}");
            assert!(text.is_char_boundary(d.span.end as usize), "{text}");
        }
    }
}

#[test]
fn timing_a_long_query_is_checked_fast() {
    let mut text = String::from(EX);
    text.push_str("PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT ?s ?label (COUNT(?o) AS ?n) WHERE {\n");
    for i in 0..300 {
        text.push_str(&format!(
            "  ?s ex:p{i} ?o{i} . OPTIONAL {{ ?o{i} rdfs:label ?label FILTER(LANG(?label) = \"en\") }}\n"
        ));
    }
    text.push_str("  ?s ex:q ?o .\n} GROUP BY ?s ?label ORDER BY ?label LIMIT 10\n");
    let d = doc(&text);
    assert!(d.diagnostics().is_empty(), "{:?}", &d.diagnostics()[..1]);
    let t = std::time::Instant::now();
    let runs = 10;
    for _ in 0..runs {
        doc(&text);
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    eprintln!(
        "a {}-line query parsed and checked in {ms:.2} ms",
        text.lines().count()
    );
    // Generous for unoptimised builds; the release build is far under 10 ms.
    assert!(ms < 100.0, "{ms} ms");
}

#[test]
fn a_query_too_deep_for_the_parser_is_an_error_not_a_crash() {
    for text in [
        format!(
            "SELECT * {{ FILTER({}1{}) }}",
            "(".repeat(5000),
            ")".repeat(5000)
        ),
        format!("SELECT * {}{}", "{".repeat(5000), "}".repeat(5000)),
        format!("SELECT * {}", "{ ?s ?p ?o . ".repeat(20000)),
        format!("SELECT * {{ ?s ?p {} }}", "[ ?p ".repeat(5000)),
        format!(
            "SELECT * {{ {} }}",
            "?s <p:p> ?o . OPTIONAL { ?o <q:q> ?x } ".repeat(30000)
        ),
    ] {
        // On a small stack, as a worker thread may have.
        let found = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let d = doc(&text);
                d.diagnostics()
                    .iter()
                    .filter(|d| d.code == "syntax")
                    .map(|d| d.message.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(found.len(), 1);
        assert!(
            found[0].contains("nested too deeply") || found[0].contains("too large"),
            "{found:?}"
        );
    }
}

/// The scoping the navigation and rename tests expect (line:column of each
/// occurrence of the variable at a position).
#[test]
fn variable_scopes_as_navigation_sees_them() {
    let text = format!(
        "{EX}SELECT ?p ?n WHERE {{\n  ?p ex:name ?n .\n  {{\n    SELECT ?p (COUNT(?o) AS ?count) WHERE {{\n      ?p ex:worksFor ?o .\n      ?o ex:name ?n .\n    }}\n    GROUP BY ?p\n  }}\n  FILTER(?n != \"x\")\n}}\n"
    );
    let d = doc(&text);
    let st = d.structure();
    let lc = |o: u32| {
        let before = &text[..o as usize];
        let line = before.matches('\n').count();
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1);
        format!("{line}:{col}")
    };
    let occ = |needle: &str, k: usize| {
        let o = (text.find(needle).unwrap() + k) as u32;
        st.var_occurrences(o)
            .iter()
            .map(|v| lc(v.span.start))
            .collect::<Vec<_>>()
    };
    assert_eq!(occ("?p ex:name", 1), ["1:7", "2:2", "4:11", "5:6", "8:13"]);
    let lines = |v: Vec<String>| {
        v.iter()
            .map(|s| s.split(':').next().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(lines(occ("?p ex:name ?n", 12)), ["1", "2", "10"]);
    assert_eq!(lines(occ("?o ex:name ?n", 12)), ["6"]);
    assert_eq!(lines(occ("?o .", 1)), ["4", "5", "6"]);
    assert!(diags(&text).is_empty(), "{:?}", diags(&text));
}

#[test]
fn the_fixture_documents_are_clean() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/test/fixtures/query");
    for f in ["people.rq", "update.ru"] {
        let text = std::fs::read_to_string(format!("{dir}/{f}")).unwrap();
        assert!(diags(&text).is_empty(), "{f}: {:?}", diags(&text));
    }
    let text = std::fs::read_to_string(format!("{dir}/update.ru")).unwrap();
    let d = doc(&text);
    let ops: Vec<_> = d
        .structure()
        .operations
        .iter()
        .map(|o| at(&text, o.keyword))
        .collect();
    assert_eq!(ops, ["INSERT", "DELETE"]);
}

/// Texts the end-to-end tests expect to have no problems.
#[test]
fn valid_documents_have_no_problems() {
    let update = format!(
        "{EX}LOAD SILENT <http://e.org/data> INTO GRAPH <http://e.org/g> ;\nCLEAR GRAPH <http://e.org/g> ;\nCREATE SILENT GRAPH <http://e.org/h> ;\nCOPY DEFAULT TO <http://e.org/h> ;\nMOVE <http://e.org/h> TO <http://e.org/i> ;\nADD <http://e.org/i> TO DEFAULT ;\nDROP ALL ;\nWITH <http://e.org/g> DELETE {{ ?s ex:name ?o }} INSERT {{ ?s ex:name \"x\" }} WHERE {{ ?s ex:name ?o }} ;\nDELETE WHERE {{ ?s ex:worksFor ?o }} ;\nDELETE DATA {{ ex:acme ex:founded 1999 }}\n"
    );
    let d = doc(&update);
    assert!(d.is_update());
    assert_eq!(d.structure().operations.len(), 10);
    assert!(diags(&update).is_empty(), "{:?}", diags(&update));
    for text in [
        format!(
            "{EX}select * where {{\n  # a comment with {{ and ?x\n  ?s ex:name \"a # not a comment\" .\n}} limit 5\n"
        ),
        format!(
            "VERSION \"1.2\"\n{EX}SELECT * WHERE {{\n  ?s ex:name ?o ~ ?r {{| ex:worksFor ?src |}} .\n  << ?s ex:name ?o >> ex:worksFor ?x .\n  BIND(<<( ?s ex:name ?o )>> AS ?t)\n  FILTER(isTRIPLE(?t) && SUBJECT(?t) = ?s)\n}}\n"
        ),
        "BASE <https://query.example.org/>\nSELECT * WHERE { ?s <def/name> ?o }\n".to_owned(),
        format!("{EX}ASK {{ ?s ex:name ?n }}\n"),
    ] {
        let d = doc(&text);
        assert!(!d.is_update());
        let problems: Vec<_> = d
            .diagnostics()
            .iter()
            .filter(|d| d.level != Level::Hint)
            .collect();
        assert!(problems.is_empty(), "{text}\n{problems:?}");
        assert!(d.tokens().iter().all(|t| t.kind != TokenKind::Error));
    }
}

#[test]
fn a_glued_keyword_spargebra_accepts_is_still_an_error() {
    for text in [
        "SELECT ?x WHERE { ?x ?p ?o } GROUPBY ?x",
        "CONSTRUCTWHERE { ?s ?p ?o }",
    ] {
        let d = Document::parse(text, &PrefixMap::default(), None);
        assert!(
            d.diagnostics()
                .iter()
                .any(|x| x.code == "syntax" && x.level == Level::Error),
            "{text}: {:?}",
            d.diagnostics()
        );
    }
}

#[test]
fn too_complex_names_deep_nesting() {
    let deep = format!("SELECT * {}{}", "{ ".repeat(300), "}".repeat(300));
    assert!(crate::sparql::too_complex(&deep).is_some());
    assert!(crate::sparql::too_complex("SELECT * { { ?s ?p ?o } }").is_none());
}

#[test]
fn local_names_with_several_dots_are_valid() {
    // spargebra 0.4 rejects them; the grammar allows them (EC numbers).
    for text in [
        "PREFIX ec: <http://e/> SELECT * { ?s ?p ec:1.1.1.353 }",
        "PREFIX ec: <http://e/> SELECT * { ?s ?p ec:3.-.-.- }",
        "PREFIX ec: <http://e/> SELECT * { ?s ?p ec:a\\.b.c }",
    ] {
        let d = Document::parse(text, &PrefixMap::default(), None);
        assert!(d.diagnostics().is_empty(), "{text}: {:?}", d.diagnostics());
        let out = crate::sparql::format(text, &PrefixMap::default(), None, &Default::default())
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        assert!(out.contains("ec:"), "{out}");
    }
    let refs = Document::parse(
        "PREFIX ec: <http://e/> SELECT * { ?s ?p ec:1.1.1.353 }",
        &PrefixMap::default(),
        None,
    );
    let iris: Vec<_> = refs
        .structure()
        .refs
        .iter()
        .filter_map(|r| r.iri.clone())
        .collect();
    assert!(iris.contains(&"http://e/1.1.1.353".to_string()), "{iris:?}");
}

/// The syntax diagnostics of `text`, read on a thread with the stack the
/// limits were measured on (8 MiB, the main thread's).
fn syntax_on_main_stack(text: String) -> Vec<String> {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(move || {
            doc(&text)
                .diagnostics()
                .iter()
                .filter(|d| d.code == "syntax")
                .map(|d| d.message.clone())
                .collect()
        })
        .unwrap()
        .join()
        .unwrap()
}

/// `n` more steps of a chain in a query that holds it.
fn chain_query(kind: &str, n: usize) -> String {
    match kind {
        "||" | "&&" | "+" | "*" => format!(
            "SELECT * {{ ?s ?p ?o FILTER(?o{} = 1) }}",
            format!(" {kind} ?o").repeat(n)
        ),
        "/" | "|" => format!(
            "PREFIX ex: <http://e/> SELECT * {{ ?s ex:p{} ?o }}",
            format!("{kind}ex:p").repeat(n)
        ),
        "!" => format!("SELECT * {{ ?s ?p ?o FILTER({}?o) }}", "!".repeat(n)),
        _ => unreachable!(),
    }
}

#[test]
fn a_long_chain_is_one_diagnostic_not_a_crash() {
    for kind in ["||", "&&", "+", "*", "/", "|", "!"] {
        let found = syntax_on_main_stack(chain_query(kind, 20_000));
        assert_eq!(found.len(), 1, "{kind}: {found:?}");
        assert!(found[0].contains("too large to check"), "{kind}: {found:?}");
    }
    // The three limits, named.
    let messages = [
        ("||", "operators in one expression"),
        ("/", "steps in one property path"),
        ("!", "unary operators in a row"),
    ];
    for (kind, what) in messages {
        let found = syntax_on_main_stack(chain_query(kind, 20_000));
        assert!(found[0].contains(what), "{kind}: {found:?}");
    }
}

#[test]
fn a_chain_at_its_limit_parses_cleanly() {
    use super::diagnostics::{MAX_OPERATORS, MAX_PATH_STEPS, MAX_UNARY};
    for (kind, max) in [
        ("||", MAX_OPERATORS),
        ("&&", MAX_OPERATORS),
        ("+", MAX_OPERATORS),
        ("*", MAX_OPERATORS),
        ("/", MAX_PATH_STEPS),
        ("|", MAX_PATH_STEPS),
        ("!", MAX_UNARY),
    ] {
        let max = max as usize;
        // Operators, steps or unary operators: the limit minus one, the limit.
        for n in [max - 1, max] {
            let found = syntax_on_main_stack(chain_query(kind, n));
            assert!(found.is_empty(), "{kind} x {n}: {found:?}");
        }
        let found = syntax_on_main_stack(chain_query(kind, max + 1));
        assert_eq!(found.len(), 1, "{kind} x {}: {found:?}", max + 1);
    }
}

#[test]
fn the_chains_in_brackets_count_with_the_chain_around_them() {
    // 200 levels of brackets, each with 1,500 operators: 300,000 deep in all.
    let mut expr = "?o".to_owned();
    for _ in 0..200 {
        expr = format!("(?o{} || {expr})", " || ?o".repeat(1_500));
    }
    let found = syntax_on_main_stack(format!("SELECT * {{ ?s ?p ?o FILTER({expr}) }}"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("operators in one expression"),
        "{found:?}"
    );
    // The same for a run of `!` before each bracket, and for path groups.
    let mut expr = "?o".to_owned();
    for _ in 0..200 {
        expr = format!("{}({expr})", "!".repeat(1_500));
    }
    let found = syntax_on_main_stack(format!("SELECT * {{ ?s ?p ?o FILTER({expr}) }}"));
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("unary operators in a row"), "{found:?}");
    let mut path = "ex:p".to_owned();
    for _ in 0..200 {
        path = format!("({}/{path})", "ex:p/".repeat(1_500).trim_end_matches('/'));
    }
    let found = syntax_on_main_stack(format!(
        "PREFIX ex: <http://e/> SELECT * {{ ?s {path} ?o }}"
    ));
    assert_eq!(found.len(), 1, "{found:?}");
    // A `/` in brackets is also a division: the first limit it reaches.
    assert!(found[0].contains("too large to check"), "{found:?}");
    // Brackets left open count the same.
    let open = format!(
        "SELECT * {{ ?s ?p ?o FILTER({}",
        format!("({}", "?o || ".repeat(20)).repeat(150)
    );
    let found = syntax_on_main_stack(open);
    assert!(
        found
            .iter()
            .any(|m| m.contains("operators in one expression")),
        "{found:?}"
    );
}

#[test]
fn many_short_chains_are_no_long_chain() {
    // Separate expressions and paths add up to no chain.
    let filters = "FILTER(?o || ?o || ?o) ".repeat(5_000);
    let triples = "?s ex:p/ex:q ?o . ".repeat(5_000);
    let args = format!("FILTER(CONCAT(?o{}))", ", ?o || ?o".repeat(5_000));
    let semis = format!("?s ex:p/ex:q ?o{}", " ; ex:p/ex:q ?o".repeat(5_000));
    let groups = "{ ?s ex:p/ex:q ?o } ".repeat(5_000);
    let blocks = "OPTIONAL { ?s ex:p/ex:q ?o } ".repeat(4_000);
    for body in [filters, triples, args, semis, groups, blocks] {
        let found = syntax_on_main_stack(format!(
            "PREFIX ex: <http://e/> SELECT * {{ ?s ?p ?o . {body} }}"
        ));
        assert!(found.is_empty(), "{found:?}");
    }
    assert!(crate::sparql::too_complex("SELECT (COUNT(*) AS ?c) { ?s ?p ?o }").is_none());
}

#[test]
fn too_complex_names_a_long_chain_for_callers_outside_documents() {
    for kind in ["||", "/", "!"] {
        let why = crate::sparql::too_complex(&chain_query(kind, 20_000));
        assert!(
            why.is_some_and(|w| w.contains("too large to check")),
            "{kind}"
        );
    }
}

#[test]
fn true_and_false_are_read_in_any_case() {
    use TokenKind::*;
    assert_eq!(
        lex("TRUE False tRuE FALSE true"),
        vec![
            (Boolean, "TRUE"),
            (Boolean, "False"),
            (Boolean, "tRuE"),
            (Boolean, "FALSE"),
            (Boolean, "true"),
        ]
    );
    for text in [
        "SELECT * { ?s ?p ?o FILTER(TRUE && False) }",
        "SELECT * { ?s ?p TRUE }",
        "SELECT * { ?s ?p ?o FILTER(?o = FALSE) }",
        "ASK { ?s ?p True }",
        "SELECT ?x { BIND(TRUE AS ?x) }",
    ] {
        let d = doc(text);
        assert!(d.diagnostics().is_empty(), "{text}: {:?}", d.diagnostics());
        assert!(d.tokens().iter().all(|t| t.kind != TokenKind::Error));
        let out = crate::sparql::format(text, &PrefixMap::default(), None, &Default::default())
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        assert!(
            out.to_ascii_lowercase().contains("true") || out.to_ascii_lowercase().contains("false"),
            "{out}"
        );
    }
    // A word that is no boolean still is an error.
    assert!(codes("SELECT * { ?s ?p TRUEISH }").contains(&"syntax"));
}

// Ticket 07: nested calls parse in time linear in their depth
// (vendor/spargebra/left-factor-nested-calls.patch).

/// Runs `f` on a 2 MiB stack, as a worker thread may have.
fn on_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(2 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

/// Checks `make(n)` and `make(n / 4)` and asserts the larger takes under four
/// times the smaller plus 5 ms (the fastest of five runs each). Returns the
/// messages of `make(n)`'s errors.
fn assert_linear(n: usize, make: impl Fn(usize) -> String) -> Vec<String> {
    let fastest = |text: &str| {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                doc(text);
                t.elapsed()
            })
            .min()
            .unwrap()
    };
    let (big, small) = (make(n), make(n / 4));
    let (t_big, t_small) = (fastest(&big), fastest(&small));
    assert!(
        t_big < t_small * 4 + std::time::Duration::from_millis(5),
        "{t_big:?} at depth {n}, {t_small:?} at depth {}: {}",
        n / 4,
        &big[..big.len().min(80)]
    );
    doc(&big)
        .diagnostics()
        .iter()
        .filter(|d| d.level == Level::Error)
        .map(|d| d.message.clone())
        .collect()
}

#[test]
fn timing_nested_builtins_parse_in_linear_time() {
    on_small_stack(|| {
        for (open, close) in [
            ("REGEX(", ", \"a\")"),
            ("SUBSTR(", ", 1)"),
            ("REPLACE(", ", \"a\", \"b\")"),
            ("GROUP_CONCAT(", ")"),
            ("GROUP_CONCAT(DISTINCT ", ")"),
            ("!(", ")"),
            ("(!", ")"),
            ("!REGEX(", ", \"a\")"),
            (":f(REGEX(", ", \"a\"))"),
            ("COUNT(DISTINCT:a(", "))"),
        ] {
            // 200 levels, or 100 of a pattern that opens two brackets: under `MAX_NESTING`.
            let n = 200 / open.matches('(').count();
            for closed in [false, true] {
                let found = assert_linear(n, |n| {
                    let tail = if closed {
                        close.repeat(n)
                    } else {
                        String::new()
                    };
                    format!(
                        "PREFIX : <http://example.org/>\nPREFIX DISTINCT: <http://example.org/d#>\nSELECT ({}?x{tail} AS ?y) {{}}",
                        open.repeat(n)
                    )
                });
                if closed {
                    // Only the aggregates break a rule, nesting one in another.
                    assert!(
                        found
                            .iter()
                            .all(|m| m == "An aggregate cannot contain another aggregate"),
                        "{open}: {found:?}"
                    );
                } else {
                    assert!(!found.is_empty(), "{open}");
                }
            }
        }
        // The two chaos inputs: iteration 107183 of seed 1792799834 and 107745
        // of seed 1792817485.
        for make in [
            (|n| format!("SELECT (AVG({}DISTINCT ?x) AS ?y) {{}}", "REGEX(".repeat(n)))
                as fn(usize) -> String,
            |n| {
                format!(
                    "PREFIX : <http://example/>\nSELECT (:{}RE",
                    "REGEX(".repeat(n)
                )
            },
        ]
        .into_iter()
        .zip([223, 188])
        {
            assert!(!assert_linear(make.1, make.0).is_empty());
        }
    });
}

#[test]
fn timing_every_builtin_nests_in_linear_time() {
    on_small_stack(|| {
        let names: Vec<&str> = lexer::words()
            .filter(|(_, kind)| *kind == TokenKind::Function)
            .map(|(name, _)| name)
            .collect();
        assert!(names.len() > 60, "{names:?}");
        for name in names {
            // What follows the nested argument; `None` for a function whose
            // argument cannot be a call.
            let rest = match name {
                "BOUND" | "NOW" | "RAND" | "UUID" | "STRUUID" => None,
                "REGEX" | "CONTAINS" | "STRSTARTS" | "STRENDS" | "STRBEFORE" | "STRAFTER"
                | "STRLANG" | "STRDT" | "langMatches" | "sameTerm" => Some(", \"a\""),
                "SUBSTR" => Some(", 1"),
                "IF" => Some(", 1, 2"),
                "REPLACE" => Some(", \"a\", \"b\""),
                "STRLANGDIR" => Some(", \"en\", \"ltr\""),
                "TRIPLE" => Some(", <http://example.org/p>, 1"),
                _ => Some(""),
            };
            let Some(rest) = rest else { continue };
            let found = assert_linear(64, |n| {
                format!(
                    "SELECT ({}?x{} AS ?y) {{}}",
                    format!("{name}(").repeat(n),
                    format!("{rest})").repeat(n)
                )
            });
            let aggregate = lexer::is_aggregate(name);
            assert!(
                found
                    .iter()
                    .all(|m| aggregate && m == "An aggregate cannot contain another aggregate"),
                "{name}: {found:?}"
            );
        }
    });
}

/// `DISTINCT` that starts a prefixed name is the name, as spargebra 0.4.7 read
/// it; the keyword before a sign or a decimal stays the keyword.
#[test]
fn distinct_that_starts_a_prefixed_name_is_the_name() {
    let text = "PREFIX distinct1: <http://e/> SELECT (COUNT(distinct1:x) AS ?c) {}";
    assert!(diags(text).is_empty(), "{:?}", diags(text));
    for (text, algebra) in [
        (text, "(COUNT(<http://e/x>) AS"),
        (
            "SELECT (COUNT(DISTINCT-1) AS ?c) {}",
            "(COUNT(DISTINCT -\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>) AS",
        ),
        (
            "SELECT (SUM(DISTINCT.5) AS ?s) {}",
            "(SUM(DISTINCT \".5\"^^<http://www.w3.org/2001/XMLSchema#decimal>) AS",
        ),
    ] {
        let query = spargebra::SparqlParser::new().parse_query(text).unwrap();
        assert!(query.to_string().contains(algebra), "{query}");
    }
}

// Ticket 08: parse fuel bounds the work of spargebra's grammar
// (vendor/spargebra/parse-fuel.patch).

const TOO_COMPLEX: &str = "SPARQL syntax error: too large to check (the query is too complex)";

/// (path, text) of every query and update in the W3C suites (`.rq`, `.ru`)
/// and in the SHACL examples of test/corpus (`sh:select`, `sh:ask`,
/// `sh:construct`, `sh:update`), when it is downloaded.
fn known_queries() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    walk(&root.join("tests/w3c/sparql-tests"), &mut files);
    walk(&root.join("test/corpus"), &mut files);
    let corpus = files
        .iter()
        .any(|f| f.starts_with(root.join("test/corpus")));
    let ci = std::env::var("CI").is_ok_and(|v| v == "true");
    assert!(
        corpus || !ci,
        "test/corpus is missing; CI=true does not skip it"
    );
    let mut out = Vec::new();
    for f in files {
        let name = f.display().to_string();
        match f.extension().and_then(|x| x.to_str()) {
            Some("rq" | "ru") => {
                if let Ok(text) = std::fs::read_to_string(&f) {
                    out.push((name, text));
                }
            }
            Some("ttl") if f.starts_with(root.join("test/corpus")) => {
                let bytes = std::fs::read(&f).unwrap();
                for t in oxttl::TurtleParser::new().for_slice(&bytes).flatten() {
                    let query = ["select", "ask", "construct", "update"]
                        .iter()
                        .any(|p| t.predicate.as_str() == format!("http://www.w3.org/ns/shacl#{p}"));
                    if let (true, oxrdf::Term::Literal(l)) = (query, &t.object) {
                        out.push((name.clone(), l.value().to_owned()));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The messages of `text`'s error diagnostics, spargebra given `fuel` per byte.
fn errors_with_fuel(text: &str, fuel: usize) -> Vec<(Span, String)> {
    diagnostics::FUEL_PER_BYTE.set(Some(fuel));
    let found = doc(text)
        .diagnostics()
        .iter()
        .filter(|d| d.level == Level::Error)
        .map(|d| (d.span, d.message.clone()))
        .collect();
    diagnostics::FUEL_PER_BYTE.set(None);
    found
}

/// The largest rule visits per byte of a known query is under a quarter of
/// [`spargebra::FUEL_PER_BYTE`]: every one parses with a quarter of it.
#[test]
fn a_quarter_of_the_fuel_parses_every_known_query() {
    let queries = known_queries();
    assert!(queries.len() > 600, "{}", queries.len());
    for (path, text) in queries {
        let found = errors_with_fuel(&text, spargebra::FUEL_PER_BYTE / 4);
        assert!(found.iter().all(|(_, m)| m != TOO_COMPLEX), "{path}");
    }
}

/// With [`spargebra::FUEL_PER_BYTE`], ticket 07's nested calls left open, and
/// its two chaos inputs, give no "too complex" error (closed, ticket 07's
/// tests assert their exact errors).
#[test]
fn the_fuel_parses_nested_calls_left_open() {
    on_small_stack(|| {
        let mut texts: Vec<String> = [
            "REGEX(", "SUBSTR(", "REPLACE(", "GROUP_CONCAT(", "GROUP_CONCAT(DISTINCT ", "!(", "(!",
            "!REGEX(", ":f(REGEX(", "COUNT(DISTINCT:a(",
        ]
        .iter()
        .map(|open| {
            let n = 200 / open.matches('(').count();
            format!(
                "PREFIX : <http://example.org/>\nPREFIX DISTINCT: <http://example.org/d#>\nSELECT ({}?x AS ?y) {{}}",
                open.repeat(n)
            )
        })
        .collect();
        texts.push(format!(
            "SELECT (AVG({}DISTINCT ?x) AS ?y) {{}}",
            "REGEX(".repeat(223)
        ));
        texts.push(format!(
            "PREFIX : <http://example/>\nSELECT (:{}RE",
            "REGEX(".repeat(188)
        ));
        for text in texts {
            let found = doc(&text).diagnostics().to_vec();
            assert!(
                !found.is_empty() && found.iter().all(|d| d.message != TOO_COMPLEX),
                "{text}"
            );
        }
    });
}

/// Out of fuel, a query of about 200 tokens that parses with one visit per
/// byte is refused with the "too complex" error on a token.
#[test]
fn a_query_out_of_fuel_is_too_complex() {
    let text = format!(
        "SELECT * {{ FILTER({}?x{}) }}",
        "STR(".repeat(65),
        ")".repeat(65)
    );
    assert!(errors_with_fuel(&text, 1).is_empty());
    let found = errors_with_fuel(&text, 0);
    assert_eq!(found.len(), 1, "{found:?}");
    let (span, message) = &found[0];
    assert_eq!(message, TOO_COMPLEX);
    assert!(span.start < span.end, "{span:?}");
}
