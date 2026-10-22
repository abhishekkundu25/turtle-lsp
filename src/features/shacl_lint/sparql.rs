//! SHACL-SPARQL query lint: each `sh:select` / `sh:ask` literal is parsed with
//! the prefixes its node declares through `sh:prefixes` (the `sh:declare`
//! entries of those nodes and the ontologies they `owl:imports`, found in
//! this document or where the workspace describes them). Reported on the literal: syntax errors at the parser's
//! position, undeclared prefixes, and the constructs the specification
//! forbids in queries with pre-bound variables (MINUS, VALUES, SERVICE,
//! `AS ?var` for a pre-bound variable, subqueries that drop one).
//!
//! SPARQL-based targets have no pre-bound `$this`, so only their syntax and
//! prefixes are checked.

use super::Finding;
use crate::analysis::Analysis;
use crate::features::embedded::{self, Lit, Queries};
use crate::knowledge::Knowledge;
use crate::shacl::sparql::{Role, restrictions};
use crate::shacl::vocab;
use crate::sparql::validate::{self, ErrorAt, ErrorKind};
use crate::syntax::{Node, Span};
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::{Query, SparqlParser};

pub(crate) const SYNTAX: &str = "shacl-sparql-syntax";
pub(crate) const PREFIX: &str = "shacl-sparql-prefix";
pub(crate) const UNSUPPORTED: &str = "shacl-sparql-unsupported";

pub(super) fn lint(a: &Analysis, k: &Knowledge, out: &mut Vec<Finding>) {
    let v = vocab::get();
    let triples = &a.parsed.triples;
    let Some(mut qs) = Queries::new(a, k) else {
        return;
    };
    let queries: Vec<usize> = qs
        .triples
        .iter()
        .copied()
        .filter(|&i| embedded::is_select_or_ask(triples[i].p))
        .collect();
    if queries.is_empty() {
        return;
    }

    // Nodes used as validators (with their components) and as targets.
    let mut components: FxHashMap<&Node, Vec<&Node>> = FxHashMap::default();
    let mut targets: FxHashSet<&Node> = FxHashSet::default();
    let mut validators: FxHashSet<&Node> = FxHashSet::default();
    for t in triples {
        if v.is_validator(t.p) {
            components.entry(&t.o).or_default().push(&t.s);
        } else if t.p == v.target {
            targets.insert(&t.o);
        } else if t.p == v.rdf_type
            && let Node::Iri(c) = &t.o
        {
            if *c == v.sparql_target || *c == v.sparql_target_type {
                targets.insert(&t.s);
            } else if *c == v.ask_validator || *c == v.select_validator {
                validators.insert(&t.s);
            }
        }
    }

    for i in queries {
        let t = &triples[i];
        let Some(lit) = qs.lit(i) else {
            continue;
        };
        let value = lit.value;
        let mut parser = SparqlParser::new();
        for (name, ns) in qs.prefixes(&t.s) {
            if let Ok(next) = parser.clone().with_prefix(&name, &ns) {
                parser = next;
            }
        }
        if let Some(why) = crate::sparql::too_complex(value) {
            push(out, lit.span, SYNTAX, why);
            continue;
        }
        let fixed = validate::text_for_spargebra(value, &crate::sparql::lexer::tokenize(value));
        let query = match parser.clone().parse_query(&fixed) {
            Ok(q) => q,
            Err(_) => {
                syntax_error(&lit, parser, out);
                continue;
            }
        };
        let role = if targets.contains(&t.s) {
            Role::Target
        } else {
            let mut vars: Vec<String> = ["this", "shapesGraph", "currentShape"]
                .map(String::from)
                .into();
            let is_validator = validators.contains(&t.s) || components.contains_key(&t.s);
            if is_validator && t.p == v.ask {
                vars.push("value".into());
            }
            for c in components.get(&t.s).into_iter().flatten() {
                for p in qs.ix.objects(c, v.parameter) {
                    for path in qs.ix.objects(p, v.path) {
                        if let Node::Iri(path) = path {
                            vars.push(
                                path[crate::knowledge::namespace_of(path).len()..].to_owned(),
                            );
                        }
                    }
                }
            }
            Role::PreBound(vars)
        };
        unsupported(&lit, &query, &role, out);
    }
}

/// A query that cannot be parsed never runs, so that is an error; forbidden
/// constructs are warnings.
fn push(out: &mut Vec<Finding>, span: Span, code: &'static str, message: String) {
    out.push(Finding {
        span,
        error: code != UNSUPPORTED,
        code,
        message,
    });
}

/// Reports a query that does not parse, where
/// [`validate::locate`](crate::sparql::validate::locate) puts the error.
fn syntax_error(lit: &Lit, parser: SparqlParser, out: &mut Vec<Finding>) {
    let e = validate::locate(lit.value, parser, false);
    let span = match e.at {
        ErrorAt::Unknown => {
            push(out, lit.span, SYNTAX, "SPARQL syntax error".into());
            return;
        }
        // The query ends too early; mark the closing quotes.
        ErrorAt::End => lit
            .offset(lit.value.len())
            .map_or(lit.span, |s| Span::new(s, s + lit.quote)),
        ErrorAt::Token(s) => lit.span(s.start as usize, s.end as usize),
    };
    if let ErrorKind::UndeclaredPrefix { name } = &e.kind {
        push(
            out,
            span,
            PREFIX,
            format!(
                "Prefix {name}: is not declared for this query; declare it with sh:prefixes and sh:declare"
            ),
        );
        return;
    }
    let what = e
        .shown_expected()
        .map_or(String::new(), |x| format!(": expected {x}"));
    push(out, span, SYNTAX, format!("SPARQL syntax error{what}"));
}

fn unsupported(lit: &Lit, q: &Query, role: &Role, out: &mut Vec<Finding>) {
    let f = restrictions(q, role);
    let select = matches!(q, Query::Select { .. });
    let v = lit.value;
    let at_word =
        |w: &str, nth: usize| find_word(v, w, nth).map_or(lit.span, |s| lit.span(s, s + w.len()));
    for (on, word) in [
        (f.minus, "MINUS"),
        (f.values, "VALUES"),
        (f.service, "SERVICE"),
    ] {
        if on {
            push(
                out,
                at_word(word, 0),
                UNSUPPORTED,
                format!("SHACL-SPARQL does not allow {word} in a query with pre-bound variables"),
            );
        }
    }
    if f.lateral {
        push(
            out,
            at_word("LATERAL", 0),
            UNSUPPORTED,
            "LATERAL is not standard SPARQL".to_owned(),
        );
    }
    for var in &f.assigned {
        let span = find_assignment(v, var).map_or(lit.span, |(s, e)| lit.span(s, e));
        push(
            out,
            span,
            UNSUPPORTED,
            format!("SHACL-SPARQL does not allow assigning the pre-bound variable ?{var} with AS"),
        );
    }
    for (n, var) in f.dropped {
        let at = find_word(v, "SELECT", n + usize::from(select));
        // `SELECT *` returns the pre-bound variables once they are bound.
        if at.is_some_and(|i| selects_all(&v[i + 6..])) {
            continue;
        }
        push(
            out,
            at.map_or(lit.span, |s| lit.span(s, s + 6)),
            UNSUPPORTED,
            format!("A subquery must return the pre-bound variable ?{var}"),
        );
    }
}

/// Whether the text after a `SELECT` keyword projects `*`.
fn selects_all(rest: &str) -> bool {
    let mut rest = rest.trim_start();
    for m in ["DISTINCT", "REDUCED"] {
        if rest
            .get(..m.len())
            .is_some_and(|x| x.eq_ignore_ascii_case(m))
        {
            rest = rest[m.len()..].trim_start();
        }
    }
    rest.starts_with('*')
}

fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'?' | b'$' | b'-')
}

/// Byte offset of the `nth` occurrence of keyword `w` in `q`, as a whole word.
fn find_word(q: &str, w: &str, nth: usize) -> Option<usize> {
    let b = q.as_bytes();
    let mut seen = 0;
    (0..b.len().saturating_sub(w.len() - 1)).find(|&i| {
        let hit = b[i..i + w.len()].eq_ignore_ascii_case(w.as_bytes())
            && (i == 0 || !is_word(b[i - 1]))
            && b.get(i + w.len()).is_none_or(|&c| !is_word(c));
        if hit {
            seen += 1;
        }
        hit && seen == nth + 1
    })
}

/// The `AS ?var` that assigns `var`.
fn find_assignment(q: &str, var: &str) -> Option<(usize, usize)> {
    let b = q.as_bytes();
    let mut nth = 0;
    while let Some(i) = find_word(q, "AS", nth) {
        let rest = &q[i + 2..];
        let j = i + 2 + (rest.len() - rest.trim_start().len());
        if matches!(b.get(j), Some(b'?' | b'$'))
            && q[j + 1..].starts_with(var)
            && b.get(j + 1 + var.len()).is_none_or(|&c| !is_word(c))
        {
            return Some((i, j + 1 + var.len()));
        }
        nth += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::lint as shacl_lint;
    use super::*;
    use crate::knowledge::Source;
    use crate::syntax::PrefixMap;
    use std::sync::Arc;

    const HEAD: &str = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                        @prefix ex: <http://example.com/> .\n\
                        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                        ex: sh:declare [ sh:prefix \"ex\" ; sh:namespace \"http://example.com/\"^^xsd:anyURI ] .\n";

    fn analysis(uri: &str, text: &str) -> Arc<Analysis> {
        Arc::new(Analysis::new(
            uri.into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ))
    }

    /// The SPARQL findings for `body` after HEAD, as (code, covered text).
    fn findings(body: &str) -> Vec<(&'static str, String)> {
        findings_with(body, &[])
    }

    fn findings_with(body: &str, others: &[&str]) -> Vec<(&'static str, String)> {
        let text = format!("{HEAD}{body}");
        let a = analysis("file:///w/shapes.ttl", &text);
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        for (n, o) in others.iter().enumerate() {
            let uri = format!("file:///w/other{n}.ttl");
            k.set(&uri, Source::Disk, analysis(&uri, o));
        }
        shacl_lint(&a, &k)
            .into_iter()
            .filter(|f| f.code.starts_with("shacl-sparql"))
            .map(|f| {
                assert_eq!(f.error, f.code != UNSUPPORTED, "{}", f.code);
                (
                    f.code,
                    text[f.span.start as usize..f.span.end as usize].to_owned(),
                )
            })
            .collect()
    }

    fn constraint(query: &str) -> String {
        format!(
            "ex:S a sh:NodeShape ; sh:targetNode ex:x ;\n  sh:sparql [ sh:prefixes ex: ;\n    sh:select {query} ] .\n"
        )
    }

    #[test]
    fn a_valid_query_is_quiet() {
        let q = "\"\"\"\n    SELECT $this ?value WHERE { $this ex:p ?value . FILTER NOT EXISTS { ?value a ex:C } }\n  \"\"\"";
        assert_eq!(findings(&constraint(q)), vec![]);
    }

    #[test]
    fn syntax_error_at_the_parser_position() {
        let q = "\"\"\"\n    SELECT $this WHERE {\n      $this ex:p ?v ) }\"\"\"";
        assert_eq!(findings(&constraint(q)), vec![(SYNTAX, ")".to_owned())]);
        // A one-line literal with escapes before the error.
        let q = "\"SELECT $this WHERE { $this ex:p \\\"a\\\" ; ex:q ?v ) }\"";
        assert_eq!(findings(&constraint(q)), vec![(SYNTAX, ")".to_owned())]);
    }

    #[test]
    fn misspelled_keyword_is_marked() {
        let q = "\"\"\"SELECT $this WHERE {\n  $this ex:p ?v .\n  FILTRE (?v > 1) }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(SYNTAX, "FILTRE".to_owned())]
        );
    }

    /// The W3C SHACL-SPARQL tests: the unsupported-sparql ones are each
    /// warned, every other query is clean.
    #[test]
    fn w3c_sparql_tests() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/w3c/shacl/sparql");
        let mut files = vec![];
        let mut dirs = vec![std::path::PathBuf::from(root)];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "ttl") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 20);
        for p in files {
            let text = std::fs::read_to_string(&p).unwrap();
            let uri = format!("file://{}", p.display());
            let a = analysis(&uri, &text);
            let mut k = Knowledge::default();
            k.set(&uri, Source::Disk, a.clone());
            let found: Vec<_> = shacl_lint(&a, &k)
                .into_iter()
                .filter(|f| f.code.starts_with("shacl-sparql"))
                .map(|f| (f.code, f.message))
                .collect();
            let name = p.file_name().unwrap().to_str().unwrap();
            if name.starts_with("unsupported-sparql-") {
                assert!(
                    found.len() == 1 && found[0].0 == UNSUPPORTED,
                    "{name}: {found:?}"
                );
            } else {
                assert!(found.is_empty(), "{name}: {found:?}");
            }
        }
    }

    #[test]
    fn syntax_error_at_the_end_points_at_the_closing_quotes() {
        let q = "\"\"\"SELECT $this WHERE { $this ex:p ?v \"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(SYNTAX, "\"\"\"".to_owned())]
        );
    }

    #[test]
    fn undeclared_prefix_is_named() {
        let q = "\"\"\"SELECT $this WHERE { $this foaf:name ?v }\"\"\"";
        let f = findings(&constraint(q));
        assert_eq!(f, vec![(PREFIX, "foaf:name".to_owned())]);
        let text = format!("{HEAD}{}", constraint(q));
        let a = analysis("file:///w/s.ttl", &text);
        let msg = &shacl_lint(&a, &Knowledge::default())[0].message;
        assert!(msg.contains("foaf:"), "{msg}");
    }

    #[test]
    fn an_undeclared_prefixes_node_falls_back_to_the_prefixes_in_scope() {
        // Nothing in the workspace declares ex:elsewhere: `ex:` comes from the
        // file's @prefix, `foaf:` from nowhere.
        let body = |q: &str| {
            format!("ex:S sh:sparql [ sh:prefixes ex:elsewhere ;\n  sh:select \"{q}\" ] .\n")
        };
        assert_eq!(
            findings_with(&body("SELECT $this WHERE { $this ex:p ?v }"), &[]),
            vec![]
        );
        assert_eq!(
            findings_with(&body("SELECT $this WHERE { $this foaf:name ?v }"), &[]),
            vec![(PREFIX, "foaf:name".to_owned())]
        );
    }

    #[test]
    fn prefixes_declared_in_another_document() {
        let other = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
                     <http://example.com/onto> sh:declare [ sh:prefix \"foaf\" ; sh:namespace \"http://xmlns.com/foaf/0.1/\" ] .\n";
        let body = "ex:S sh:sparql [ sh:prefixes <http://example.com/onto> ;\n  sh:select \"SELECT $this WHERE { $this foaf:name ?v }\" ] .\n";
        assert_eq!(findings_with(body, &[other]), vec![]);
        assert_eq!(findings_with(body, &[]).len(), 1);
    }

    #[test]
    fn minus_values_service_are_warned() {
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . MINUS { $this ?x \"Value\" } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "MINUS".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { VALUES ?any { true } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "VALUES".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . SERVICE <http://a.example/> { ?a ?b ?c } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "SERVICE".to_owned())]
        );
    }

    #[test]
    fn lateral_is_warned() {
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . LATERAL { SELECT $this ?any WHERE { $this ?b ?any } LIMIT 1 } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "LATERAL".to_owned())]
        );
    }

    #[test]
    fn prebound_assignment_and_subquery_are_warned() {
        let q = "\"\"\"SELECT $this WHERE { BIND (true AS $this) . }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "AS $this".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { $this ?x ?any . { SELECT ?other ?b WHERE { ?other ?b ?c } } }\"\"\"";
        assert_eq!(
            findings(&constraint(q)),
            vec![(UNSUPPORTED, "SELECT".to_owned())]
        );
        let q = "\"\"\"SELECT $this WHERE { { SELECT $this WHERE { $this ?b ?c } } }\"\"\"";
        assert_eq!(findings(&constraint(q)), vec![]);
    }

    #[test]
    fn ask_validator_cannot_assign_value() {
        let body = "ex:C a sh:ConstraintComponent ;\n  sh:parameter [ sh:path ex:lang ] ;\n  sh:validator ex:V .\n\
                    ex:V a sh:SPARQLAskValidator ; sh:ask \"ASK { BIND (true AS ?value) . FILTER (langMatches(lang(?value), $lang)) }\" .\n";
        assert_eq!(findings(body), vec![(UNSUPPORTED, "AS ?value".to_owned())]);
        let body = "ex:C a sh:ConstraintComponent ;\n  sh:parameter [ sh:path ex:lang ] ;\n  sh:validator ex:V .\n\
                    ex:V a sh:SPARQLAskValidator ; sh:ask \"ASK { BIND (\\\"en\\\" AS ?lang) }\" .\n";
        assert_eq!(findings(body), vec![(UNSUPPORTED, "AS ?lang".to_owned())]);
    }

    #[test]
    fn targets_only_get_syntax_checks() {
        let body = "ex:S a sh:NodeShape ; sh:target [ a sh:SPARQLTarget ; sh:prefixes ex: ;\n  sh:select \"SELECT ?this WHERE { ?this a ex:C MINUS { ?this ex:p 1 } }\" ] .\n";
        assert_eq!(findings(body), vec![]);
    }

    #[test]
    fn many_constraints_lint_fast() {
        let mut body = String::new();
        for i in 0..200 {
            body.push_str(&format!(
                "ex:S{i} a sh:NodeShape ; sh:targetClass ex:C ;\n  sh:sparql [ sh:prefixes ex: ; sh:message \"m\" ;\n    sh:select \"\"\"\n      SELECT $this ?value WHERE {{\n        $this ex:p{i} ?value .\n        FILTER NOT EXISTS {{ ?value a ex:D }}\n      }}\"\"\" ] .\n"
            ));
        }
        let text = format!("{HEAD}{body}");
        let a = analysis("file:///w/shapes.ttl", &text);
        let mut k = Knowledge::default();
        k.set("file:///w/shapes.ttl", Source::Open, a.clone());
        shacl_lint(&a, &k);
        let t = std::time::Instant::now();
        let n = shacl_lint(&a, &k)
            .iter()
            .filter(|f| f.code.starts_with("shacl-sparql"))
            .count();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        eprintln!("200 sh:select constraints linted in {ms:.2} ms");
        assert_eq!(n, 0);
        assert!(ms < 20.0, "{ms} ms");
    }
}
