//! The Functional frontend: each grammar area parses with exact spans, each
//! error is reported at its span, recovery reads on past an error, the
//! limits hold, and the W3C test inputs and the real files parse.

use super::grammar::{self, MAX_NESTING};
use super::tree::{Document, Keyword, NodeKind};
use super::*;

/// The prefixes of the documents below.
const HEAD: &str = "Prefix(:=<http://example.org/o#>)\nPrefix(ex:=<http://example.org/>)\n";

fn doc(text: &str) -> Document {
    grammar::parse(text)
}

/// `text` in an ontology, after [`HEAD`].
fn in_ontology(body: &str) -> String {
    format!("{HEAD}Ontology(<http://example.org/o>\n{body}\n)\n")
}

/// The errors of `text`: the text at each span, and the message.
fn errors(text: &str) -> Vec<(String, String)> {
    doc(text)
        .errors
        .iter()
        .map(|e| {
            assert_eq!(e.code, CODE);
            assert_eq!(e.severity, Severity::Error);
            (at(text, e.span).to_string(), e.message().to_string())
        })
        .collect()
}

fn at(text: &str, span: Span) -> &str {
    &text[span.start as usize..span.end as usize]
}

#[track_caller]
fn clean(text: &str) -> Document {
    let d = doc(text);
    let shown: Vec<_> = d
        .errors
        .iter()
        .map(|e| (at(text, e.span), e.message()))
        .collect();
    assert!(shown.is_empty(), "{shown:#?}\n{text}");
    d
}

/// The one error of `body` in an ontology: its text and message.
#[track_caller]
fn one_error(body: &str) -> (String, String) {
    let text = in_ontology(body);
    let mut e = errors(&text);
    assert_eq!(e.len(), 1, "{e:#?}\n{text}");
    e.remove(0)
}

/// The keyword of each top-level construct.
fn keywords(d: &Document) -> Vec<Keyword> {
    d.top()
        .filter_map(|i| match d.nodes[i].kind {
            NodeKind::Construct(k) => Some(k),
            _ => None,
        })
        .collect()
}

/// The text of node `i` and of each of its children.
fn spans(text: &str, d: &Document, i: usize) -> (String, Vec<String>) {
    (
        at(text, d.nodes[i].span).to_string(),
        d.children(i)
            .map(|c| at(text, d.nodes[c].span).to_string())
            .collect(),
    )
}

// ---- the document ----

/// The head OWL API writes: every standard prefix declared, `xml:`, a
/// version IRI, imports, annotations, comment lines.
const OWL_API: &str = "Prefix(:=<http://example.org/o.owl#>)
Prefix(owl:=<http://www.w3.org/2002/07/owl#>)
Prefix(rdf:=<http://www.w3.org/1999/02/22-rdf-syntax-ns#>)
Prefix(xml:=<http://www.w3.org/XML/1998/namespace>)
Prefix(xsd:=<http://www.w3.org/2001/XMLSchema#>)
Prefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)


Ontology(<http://example.org/o.owl>
<http://example.org/o/2024-01-01/o.owl>
Import(<http://example.org/imports/a.owl>)
Annotation(rdfs:comment \"An example.\"@en)
Annotation(owl:versionInfo \"2024-01-01\")

Declaration(Class(:A))
Declaration(Class(:B))
Declaration(AnnotationProperty(rdfs:label))
############################
#   Classes
############################

# Class: :A (a thing)

AnnotationAssertion(rdfs:label :A \"a thing\"@en)
SubClassOf(:A :B)
)";

#[test]
fn a_document_as_owl_api_writes_it_has_no_error() {
    let d = clean(OWL_API);
    let names: Vec<_> = d.prefixes.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["", "owl", "rdf", "xml", "xsd", "rdfs"]);
    let p = &d.prefixes[3];
    assert_eq!(at(OWL_API, p.name_span), "xml:");
    assert_eq!(
        at(OWL_API, p.iri_span),
        "<http://www.w3.org/XML/1998/namespace>"
    );
    assert_eq!(
        at(OWL_API, p.span),
        "Prefix(xml:=<http://www.w3.org/XML/1998/namespace>)"
    );
    assert_eq!(p.iri.as_str(), "http://www.w3.org/XML/1998/namespace");
    let ontology = at(OWL_API, d.ontology.unwrap());
    assert!(ontology.starts_with("Ontology(<http://example.org/o.owl>"));
    assert!(ontology.ends_with("SubClassOf(:A :B)\n)"));
    let node = |i: Option<usize>| at(OWL_API, d.nodes[i.unwrap()].span);
    assert_eq!(node(d.ontology_iri), "<http://example.org/o.owl>");
    assert_eq!(
        node(d.version_iri),
        "<http://example.org/o/2024-01-01/o.owl>"
    );
    use Keyword as K;
    assert_eq!(
        keywords(&d),
        [
            K::Import,
            K::Annotation,
            K::Annotation,
            K::Declaration,
            K::Declaration,
            K::Declaration,
            K::AnnotationAssertion,
            K::SubClassOf
        ]
    );
    let comments: Vec<_> = d.comments.iter().map(|&c| at(OWL_API, c)).collect();
    assert_eq!(
        comments,
        [
            "############################",
            "#   Classes",
            "############################",
            "# Class: :A (a thing)"
        ]
    );
}

#[test]
fn an_ontology_may_be_empty_and_have_no_iri() {
    for text in [
        "Ontology()",
        "Ontology(\n)\n",
        "\u{feff}Ontology()",
        "# only a comment\nOntology()",
        "Ontology(<http://example.org/o>)",
        "Prefix(:=<http://example.org/>)Ontology(:o :v)",
    ] {
        clean(text);
    }
    let d = clean("Ontology()");
    assert_eq!(d.ontology, Some(Span::new(0, 10)));
    assert_eq!((d.ontology_iri, d.version_iri), (None, None));
    assert!(d.nodes.is_empty());
}

// ---- each grammar area ----

#[test]
fn every_axiom_kind_parses() {
    let body = "Declaration(Class(:C))
Declaration(Datatype(:dt))
Declaration(ObjectProperty(:op))
Declaration(DataProperty(:dp))
Declaration(AnnotationProperty(:ap))
Declaration(NamedIndividual(:i))
SubClassOf(:C :D)
EquivalentClasses(:C :D :E)
DisjointClasses(:C :D)
DisjointUnion(:C :D :E)
SubObjectPropertyOf(:op :oq)
SubObjectPropertyOf(ObjectPropertyChain(:op ObjectInverseOf(:oq)) :or)
EquivalentObjectProperties(:op :oq)
DisjointObjectProperties(:op :oq :or)
InverseObjectProperties(:op :oq)
ObjectPropertyDomain(:op :C)
ObjectPropertyRange(:op :C)
FunctionalObjectProperty(:op)
InverseFunctionalObjectProperty(:op)
ReflexiveObjectProperty(:op)
IrreflexiveObjectProperty(:op)
SymmetricObjectProperty(:op)
AsymmetricObjectProperty(:op)
TransitiveObjectProperty(ObjectInverseOf(:op))
SubDataPropertyOf(:dp :dq)
EquivalentDataProperties(:dp :dq)
DisjointDataProperties(:dp :dq)
DataPropertyDomain(:dp :C)
DataPropertyRange(:dp xsd:integer)
FunctionalDataProperty(:dp)
DatatypeDefinition(:dt DatatypeRestriction(xsd:integer xsd:minInclusive \"0\"^^xsd:integer))
HasKey(:C (:op) (:dp))
SameIndividual(:i :j)
DifferentIndividuals(:i :j _:k)
ClassAssertion(:C :i)
ObjectPropertyAssertion(:op :i _:j)
NegativeObjectPropertyAssertion(:op :i :j)
DataPropertyAssertion(:dp :i \"1\"^^xsd:integer)
NegativeDataPropertyAssertion(:dp _:i \"x\")
AnnotationAssertion(rdfs:label :C \"C\"@en)
AnnotationAssertion(rdfs:seeAlso _:x <http://example.org/see>)
AnnotationAssertion(:ap :C _:y)
SubAnnotationPropertyOf(:ap rdfs:label)
AnnotationPropertyDomain(:ap :C)
AnnotationPropertyRange(:ap xsd:string)";
    let text = in_ontology(body);
    let d = clean(&text);
    assert_eq!(keywords(&d).len(), body.lines().count());
    let lines: Vec<&str> = body.lines().collect();
    // Every axiom's span is its line, from the keyword to its `)`.
    let tops: Vec<&str> = d
        .top()
        .skip(1)
        .map(|i| at(&text, d.nodes[i].span))
        .collect();
    assert_eq!(tops, lines);
    for (i, line) in d.top().skip(1).zip(&lines) {
        let NodeKind::Construct(k) = d.nodes[i].kind else {
            panic!("{line}")
        };
        assert!(line.starts_with(&format!("{}(", k.as_str())), "{line}");
    }
}

#[test]
fn class_expressions_nest_with_their_spans() {
    let ces = [
        ":C",
        "owl:Thing",
        "<http://example.org/C>",
        "ObjectIntersectionOf(:C :D)",
        "ObjectUnionOf(:C :D :E)",
        "ObjectComplementOf(:C)",
        "ObjectOneOf(:i)",
        "ObjectOneOf(:i _:j)",
        "ObjectSomeValuesFrom(:p :C)",
        "ObjectAllValuesFrom(ObjectInverseOf(:p) :C)",
        "ObjectHasValue(:p :i)",
        "ObjectHasValue(:p _:i)",
        "ObjectHasSelf(:p)",
        "ObjectMinCardinality(0 :p)",
        "ObjectMinCardinality(1 :p :C)",
        "ObjectMaxCardinality(10 :p)",
        "ObjectExactCardinality(007 :p ObjectComplementOf(:C))",
        "DataSomeValuesFrom(:dp xsd:integer)",
        "DataSomeValuesFrom(:dp :dq DataUnionOf(xsd:integer xsd:string))",
        "DataAllValuesFrom(:dp DataOneOf(\"a\" \"b\"@en))",
        "DataHasValue(:dp \"1\"^^xsd:integer)",
        "DataMinCardinality(1 :dp)",
        "DataMaxCardinality(1 :dp xsd:integer)",
        "DataExactCardinality(2 :dp DatatypeRestriction(xsd:integer xsd:maxExclusive \"5\"^^xsd:integer))",
    ];
    for ce in ces {
        let text = in_ontology(&format!("SubClassOf(:A {ce})"));
        let d = clean(&text);
        let axiom = d.top().nth(1).unwrap();
        let (whole, args) = spans(&text, &d, axiom);
        assert_eq!(whole, format!("SubClassOf(:A {ce})"));
        assert_eq!(args, [":A", ce]);
    }
    // The children of a nested construct, each at its span.
    let text = in_ontology(
        "EquivalentClasses(:A ObjectIntersectionOf(:B ObjectSomeValuesFrom(:p ObjectUnionOf(:C :D))))",
    );
    let d = clean(&text);
    let axiom = d.top().nth(1).unwrap();
    let and = d.children(axiom).nth(1).unwrap();
    assert_eq!(
        spans(&text, &d, and).1,
        [":B", "ObjectSomeValuesFrom(:p ObjectUnionOf(:C :D))"]
    );
    let some = d.children(and).nth(1).unwrap();
    assert_eq!(spans(&text, &d, some).1, [":p", "ObjectUnionOf(:C :D)"]);
    assert!(matches!(
        d.nodes[d.children(some).next().unwrap()].kind,
        NodeKind::Iri {
            prefix_len: Some(0)
        }
    ));
}

#[test]
fn data_ranges_and_literals() {
    for dr in [
        "xsd:integer",
        "rdfs:Literal",
        "DataIntersectionOf(xsd:integer DataComplementOf(xsd:negativeInteger))",
        "DataUnionOf(xsd:string xsd:integer xsd:boolean)",
        "DataComplementOf(xsd:integer)",
        "DataOneOf(\"1\"^^xsd:integer)",
        "DataOneOf(\"a\" \"b\"@en \"c\"@en-GB \"d\"^^xsd:string)",
        "DatatypeRestriction(xsd:integer xsd:minInclusive \"0\"^^xsd:integer xsd:maxInclusive \"9\"^^xsd:integer)",
        "DatatypeRestriction(xsd:string xsd:pattern \"[a-z]+\")",
    ] {
        let text = in_ontology(&format!("DataPropertyRange(:dp {dr})"));
        let d = clean(&text);
        let axiom = d.top().nth(1).unwrap();
        assert_eq!(spans(&text, &d, axiom).1, [":dp", dr]);
    }
    // A literal: its span, the string's end, and its datatype as a child.
    let text = in_ontology(
        "DataPropertyAssertion(:dp :i \"1\"^^xsd:integer)\nDataPropertyAssertion(:dp :i \"a\\\"b\\\\c\"@en)\nDataPropertyAssertion(:dp :i \"two\nlines\")\nDataPropertyAssertion(:dp :i \"10\" ^^ xsd:integer)\nDataPropertyAssertion(:dp :i \"abc\" @en)",
    );
    let d = clean(&text);
    let literals: Vec<_> = d
        .top()
        .skip(1)
        .map(|axiom| {
            let lit = d.children(axiom).nth(2).unwrap();
            let NodeKind::Literal { string_end } = d.nodes[lit].kind else {
                panic!("{:?}", d.nodes[lit])
            };
            let span = d.nodes[lit].span;
            let string = &text[span.start as usize..string_end as usize];
            let datatype: Vec<_> = d
                .children(lit)
                .map(|c| at(&text, d.nodes[c].span))
                .collect();
            (at(&text, span), string, datatype)
        })
        .collect();
    assert_eq!(
        literals,
        [
            ("\"1\"^^xsd:integer", "\"1\"", vec!["xsd:integer"]),
            ("\"a\\\"b\\\\c\"@en", "\"a\\\"b\\\\c\"", vec![]),
            ("\"two\nlines\"", "\"two\nlines\"", vec![]),
            ("\"10\" ^^ xsd:integer", "\"10\"", vec!["xsd:integer"]),
            ("\"abc\" @en", "\"abc\"", vec![]),
        ]
    );
}

#[test]
fn keys_rules_and_annotations() {
    // HasKey's lists are groups; either may be empty, not both.
    let text =
        in_ontology("HasKey(:C () (:dp :dq))\nHasKey(owl:Thing (ObjectInverseOf(:p) :q) ())");
    let d = clean(&text);
    let keys: Vec<_> = d.top().skip(1).map(|i| spans(&text, &d, i).1).collect();
    assert_eq!(
        keys,
        [
            vec![":C", "()", "(:dp :dq)"],
            vec!["owl:Thing", "(ObjectInverseOf(:p) :q)", "()"]
        ]
    );
    let group = d.children(d.top().nth(1).unwrap()).nth(2).unwrap();
    assert_eq!(d.nodes[group].kind, NodeKind::Group);
    assert_eq!(spans(&text, &d, group).1, [":dp", ":dq"]);

    // SWRL rules, as OWL API writes them.
    let rule = "DLSafeRule(Annotation(rdfs:label \"r\") Body(ClassAtom(:C Variable(<urn:swrl:var#x>)) ObjectPropertyAtom(:p Variable(:x) :i) DataPropertyAtom(:dp Variable(:x) Variable(:v)) DataRangeAtom(xsd:integer Variable(:v)) BuiltInAtom(<http://www.w3.org/2003/11/swrlb#greaterThan> Variable(:v) \"18\"^^xsd:integer) SameIndividualAtom(Variable(:x) :j) DifferentIndividualsAtom(_:k Variable(:y)))Head(ClassAtom(ObjectSomeValuesFrom(:p :C) Variable(:x))))";
    let text = in_ontology(&format!("{rule}\nDLSafeRule(Body() Head())"));
    let d = clean(&text);
    assert_eq!(keywords(&d), [Keyword::DLSafeRule, Keyword::DLSafeRule]);
    let first = d.top().nth(1).unwrap();
    let (whole, args) = spans(&text, &d, first);
    assert_eq!(whole, rule);
    assert_eq!(args.len(), 3);
    assert!(args[1].starts_with("Body(ClassAtom(") && args[2].starts_with("Head(ClassAtom("));

    // Annotations on an axiom and on an annotation.
    let axiom = "SubClassOf(Annotation(Annotation(rdfs:comment \"why\") rdfs:label \"a\"@en) Annotation(rdfs:seeAlso <http://example.org/x>) :A :B)";
    let text = in_ontology(axiom);
    let d = clean(&text);
    let i = d.top().nth(1).unwrap();
    assert_eq!(
        spans(&text, &d, i).1,
        [
            "Annotation(Annotation(rdfs:comment \"why\") rdfs:label \"a\"@en)",
            "Annotation(rdfs:seeAlso <http://example.org/x>)",
            ":A",
            ":B"
        ]
    );
    let annotation = d.children(i).next().unwrap();
    assert_eq!(
        spans(&text, &d, annotation).1,
        ["Annotation(rdfs:comment \"why\")", "rdfs:label", "\"a\"@en"]
    );
    // An annotation value may be an anonymous individual.
    clean(&in_ontology("Declaration(Annotation(:ap _:b) Class(:A))"));
}

/// SWRL rules as ROBOT 1.9.11 (OWL API) writes them from a Manchester
/// `Rule:` with an atom of each of the seven kinds.
const ROBOT_RULE: &str = "Prefix(:=<http://example.org/r#>)
Prefix(owl:=<http://www.w3.org/2002/07/owl#>)
Prefix(rdf:=<http://www.w3.org/1999/02/22-rdf-syntax-ns#>)
Prefix(xml:=<http://www.w3.org/XML/1998/namespace>)
Prefix(xsd:=<http://www.w3.org/2001/XMLSchema#>)
Prefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)
Prefix(swrlb:=<http://www.w3.org/2003/11/swrlb#>)


Ontology(<http://example.org/r>

Declaration(Class(:Adult))
Declaration(Class(:Person))
Declaration(ObjectProperty(:p))
Declaration(DataProperty(:age))
Declaration(NamedIndividual(:i))
DLSafeRule(Body(ClassAtom(:Person Variable(<urn:swrl:var#x>)) ObjectPropertyAtom(:p Variable(<urn:swrl:var#x>) Variable(<urn:swrl:var#y>)) DataPropertyAtom(:age Variable(<urn:swrl:var#x>) Variable(<urn:swrl:var#a>)) DataRangeAtom(xsd:integer Variable(<urn:swrl:var#a>)) BuiltInAtom(swrlb:greaterThan Variable(<urn:swrl:var#a>) \"18\"^^xsd:integer) SameIndividualAtom(Variable(<urn:swrl:var#x>) Variable(<urn:swrl:var#y>)) DifferentIndividualsAtom(Variable(<urn:swrl:var#x>) :i))Head(ClassAtom(:Adult Variable(<urn:swrl:var#x>))))
)
";

#[test]
fn every_swrl_atom_as_robot_writes_it_parses() {
    let d = clean(ROBOT_RULE);
    let rule = d.top().last().unwrap();
    assert_eq!(d.nodes[rule].kind, NodeKind::Construct(Keyword::DLSafeRule));
    let body = d.children(rule).next().unwrap();
    let atoms: Vec<Keyword> = d
        .children(body)
        .map(|a| match d.nodes[a].kind {
            NodeKind::Construct(k) => k,
            other => panic!("{other:?}"),
        })
        .collect();
    use Keyword as K;
    assert_eq!(
        atoms,
        [
            K::ClassAtom,
            K::ObjectPropertyAtom,
            K::DataPropertyAtom,
            K::DataRangeAtom,
            K::BuiltInAtom,
            K::SameIndividualAtom,
            K::DifferentIndividualsAtom
        ]
    );
}

/// The tokenisation rules of section 2.2, with its examples.
#[test]
fn tokens_follow_section_2_2() {
    // A `#` inside a string or an IRI starts no comment.
    let text = in_ontology(
        "AnnotationAssertion(rdfs:comment <http://example.org/#comment> \" #comment\\\" \" #comment \"abc\"\n)",
    );
    let d = clean(&text);
    assert_eq!(
        d.comments.iter().map(|&c| at(&text, c)).collect::<Vec<_>>(),
        ["#comment \"abc\""]
    );
    // Matches are greedy: `SubClassOf:ABC` is one abbreviated IRI.
    let text =
        "Prefix(SubClassOf:=<http://example.org/s#>)\nOntology(Declaration(Class(SubClassOf:ABC)))";
    let d = clean(text);
    let iri = d.children(d.children(0).next().unwrap()).next().unwrap();
    assert_eq!(at(text, d.nodes[iri].span), "SubClassOf:ABC");
    assert_eq!(
        d.nodes[iri].kind,
        NodeKind::Iri {
            prefix_len: Some(10)
        }
    );
    // No space is needed after a delimiter, nor around `^^`.
    clean(&in_ontology(
        "SubClassOf(ObjectComplementOf(:A)ObjectComplementOf(:B))",
    ));
    clean(&in_ontology(
        "SubClassOf(:A ObjectComplementOf(:B))DisjointClasses(:A :B)",
    ));
    clean(&in_ontology(
        "DataPropertyAssertion(:dp :i \"10\"^^xsd:integer)",
    ));
    // An abbreviated IRI ends at a comment.
    clean(&in_ontology("SubClassOf(:A :B# B\n)"));
    // Local names as SPARQL 1.1 writes them.
    clean(&in_ontology(
        "SubClassOf(ex:a.b ex:a:b)\nSubClassOf(ex:a-b ex:%20)\nSubClassOf(ex:1 ex:_x)",
    ));
    // White space is the four characters only.
    clean("Ontology(\t\r\n Declaration(Class(<http://example.org/A>))\r\n)");
}

#[test]
fn tokens_the_rules_reject() {
    for (body, want) in [
        // `pref: ABC`: a prefix name, then text that is no token.
        (
            "SubClassOf(ex: ABC)",
            (
                "ex:",
                "Expected an IRI, found the prefix name 'ex:': an abbreviated IRI has a local name after the ':'",
            ),
        ),
        (
            "SubClassOf(:A ObjectMinCardinality(10abc :p))",
            ("10abc", "Expected a non-negative integer, found '10abc'"),
        ),
        (
            "DataPropertyAssertion(:dp :i \"abc\"@ en)",
            ("@", "Expected a language tag after '@'"),
        ),
        (
            "DataPropertyAssertion(:dp :i \"abc\"\"def\")",
            (
                "\"abc\"\"",
                "Expected white space or a delimiter after '\"abc\"'",
            ),
        ),
        (
            "DataPropertyAssertion(:dp :i \"a\"^xsd:string)",
            ("^", "Expected '^^'"),
        ),
    ] {
        let got = one_error(&format!("SubClassOf(:A :B)\n{body}"));
        assert_eq!((got.0.as_str(), got.1.as_str()), want, "{body}");
    }
}

// ---- the errors ----

#[test]
fn an_undeclared_prefix_is_reported_at_each_use() {
    let text =
        in_ontology("SubClassOf(nope:A nope:B)\nSubClassOf(:A rdf:B)\nSubClassOf(:A xml:lang)");
    assert_eq!(
        errors(&text),
        [
            (
                "nope:A".to_string(),
                "Undeclared prefix 'nope:'".to_string()
            ),
            ("nope:B".into(), "Undeclared prefix 'nope:'".into()),
            ("xml:lang".into(), "Undeclared prefix 'xml:'".into()),
        ]
    );
    // The empty prefix too.
    let text = "Ontology(Declaration(Class(:A)))";
    assert_eq!(
        errors(text),
        [(":A".to_string(), "Undeclared prefix ':'".to_string())]
    );
}

#[test]
fn a_prefix_is_declared_once_and_a_standard_one_may_be_bound_again() {
    let text = "Prefix(ex:=<http://example.org/>)\nPrefix(ex:=<http://example.org/>)\nPrefix(ex:=<http://example.com/>)\nPrefix(owl:=<http://www.w3.org/2002/07/owl#>)\nPrefix(rdfs:=<http://example.org/rdfs#>)\nPrefix(rdfs:=<http://example.org/other#>)\nOntology()";
    assert_eq!(
        errors(text),
        [
            (
                "ex:".to_string(),
                "The prefix 'ex:' is declared twice: as <http://example.org/> before, and as <http://example.com/> here".to_string()
            ),
            (
                "rdfs:".into(),
                "The prefix 'rdfs:' is declared twice: as <http://example.org/rdfs#> before, and as <http://example.org/other#> here".into()
            ),
        ]
    );
    // Every declaration is kept, in text order.
    assert_eq!(doc(text).prefixes.len(), 6);
    // A Table 2 name bound to another IRI expands with it: a relative
    // prefix IRI is reported where it is declared, not where it is used.
    let text =
        "Prefix(xsd:=<rel#>)\nOntology(DataPropertyRange(<http://example.org/p> xsd:integer))";
    assert_eq!(errors(text).len(), 1);
}

#[test]
fn a_prefix_with_an_invalid_iri_binds_nothing_for_other_files() {
    // The error is at the IRI; the document's prefixes, which other files
    // read as bindings, leave the declaration out, so no file is offered
    // `@prefix terms: <relative/>` or an IRI with a space.
    for (iri, message) in [
        ("<relative/>", "<relative/> is a relative IRI"),
        ("<http://exa mple.org/>", "Invalid IRI: "),
    ] {
        let text = format!("Prefix(terms:={iri})\nPrefix(ex:=<http://example.org/>)\nOntology()");
        let got = errors(&text);
        assert_eq!(got.len(), 1, "{iri}: {got:?}");
        assert!(got[0].0 == iri && got[0].1.starts_with(message), "{got:?}");
        let names: Vec<_> = doc(&text).prefixes.iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["ex"], "{iri}");
    }
}

#[test]
fn iris_are_absolute_iris() {
    for (body, want) in [
        (
            "SubClassOf(<A> :B)",
            ("<A>", "<A> is a relative IRI: an IRI in OWL 2 is absolute"),
        ),
        (
            "SubClassOf(<#A> :B)",
            (
                "<#A>",
                "<#A> is a relative IRI: an IRI in OWL 2 is absolute",
            ),
        ),
        (
            "SubClassOf(<http://example.org/a{b}> :B)",
            (
                "<http://example.org/a{b}>",
                "Invalid IRI: Invalid IRI code point '{'",
            ),
        ),
        (
            "SubClassOf(<http://example.org/a b> :B)",
            ("<http://example.org/a b>", "Invalid IRI: "),
        ),
        (
            "SubClassOf(<http://example.org/a :B)\nDeclaration(Class(:Z))",
            ("<http://example.org/a :B)", "Expected '>' to end the IRI"),
        ),
    ] {
        let got = one_error(body);
        assert!(
            got.0 == want.0 && got.1.starts_with(want.1),
            "{body}: {got:?}"
        );
    }
    // Each use of a bad IRI is reported, from the cache too.
    let text = in_ontology("SubClassOf(<a> <a>)");
    assert_eq!(errors(&text).len(), 2);
    // A prefix IRI that is no IRI is reported at its declaration, not at
    // each use.
    let text = "Prefix(p:=<rel/>)\nOntology(SubClassOf(p:A p:B))";
    assert_eq!(
        errors(text),
        [(
            "<rel/>".to_string(),
            "<rel/> is a relative IRI: an IRI in OWL 2 is absolute".to_string()
        )]
    );
}

/// An abbreviated IRI is checked by its local name after its prefix's
/// context, not as the whole expansion: the verdict and the message are
/// those of the whole expansion, for a namespace that ends in each part of
/// an IRI and local names with each character that changes the verdict.
#[test]
fn abbreviated_iris_are_checked_as_their_expansion() {
    let namespaces = [
        "urn:",
        "urn:/",
        "urn:x",
        "http://",
        "http://h",
        "http://u@h",
        "http://u:p@h",
        "http://h:",
        "http://h:8",
        "http://u:p@h:8",
        "http://[::1]",
        "http://[::1]:",
        "http://[::1]:8",
        "http://[v1.x]",
        "http://example.org/a",
        "http://example.org/a:b/",
        "http://example.org/a?",
        "http://example.org/a?q",
        "http://example.org/a?q#",
        "http://example.org/a#",
        "http://example.org/a#f",
    ];
    let locals = [
        "x",
        ":",
        "::",
        ":x",
        ":80",
        "x:80",
        "x:y",
        "80",
        "8:0",
        "%41",
        "a.b",
        "é",
        "x·y",
        "\u{F900}",
        "\u{FFF0}",
        "x\u{FFF0}",
        "\u{1FFFE}",
        "\u{EFFFD}",
    ];
    for ns in namespaces {
        for local in locals {
            let text = format!("Prefix(ex:=<{ns}>)\nOntology(Declaration(Class(ex:{local})))");
            let full = format!("{ns}{local}");
            let want: Vec<(String, String)> = match oxiri::Iri::parse(full.as_str()) {
                Ok(_) => vec![],
                Err(e) => vec![(
                    format!("ex:{local}"),
                    format!(
                        "'ex:{local}' expands to <{}>: Invalid IRI: {e}",
                        shown(&full)
                    ),
                )],
            };
            assert_eq!(errors(&text), want, "{text}");
        }
    }
    // A long namespace is shown cut, as the whole expansion is.
    let ns = format!("http://example.org/{}/", "a".repeat(100));
    let text = format!("Prefix(ex:=<{ns}>)\nOntology(Declaration(Class(ex:x\u{FFF0})))");
    let full = format!("{ns}x\u{FFF0}");
    assert_eq!(
        errors(&text),
        [(
            "ex:x\u{FFF0}".to_string(),
            format!(
                "'ex:x\u{FFF0}' expands to <{}>: Invalid IRI: Invalid IRI code point '\u{FFF0}'",
                shown(&full)
            )
        )]
    );
}

#[test]
fn literals_are_checked() {
    for (body, want) in [
        (
            "DataPropertyAssertion(:dp :i \"x\"@en_US)",
            ("@en_US", "Invalid language tag 'en_US': "),
        ),
        (
            "DataPropertyAssertion(:dp :i \"a\\nb\")",
            (
                "\\n",
                "Invalid escape '\\n': a string escapes only '\"' and '\\' (as \\\" and \\\\)",
            ),
        ),
        (
            "DataPropertyAssertion(:dp :i \"a\u{1}b\")",
            ("\u{1}", "A string cannot hold the control character U+0001"),
        ),
        (
            "DataPropertyAssertion(:dp :i \"a\u{ffff}b\")",
            ("\u{ffff}", "A string cannot hold U+FFFF"),
        ),
        (
            "DataPropertyAssertion(:dp :i \"1\"^^)",
            (")", "Expected a datatype IRI after '^^', found ')'"),
        ),
        (
            "DataPropertyAssertion(:dp :i \"1\"^^\"2\")",
            (
                "\"2\"",
                "Expected a datatype IRI after '^^', found a string",
            ),
        ),
    ] {
        let got = one_error(body);
        assert!(
            got.0 == want.0 && got.1.starts_with(want.1),
            "{body}: {got:?}"
        );
    }
    // A string never closed: one error, at its quote.
    let text = in_ontology("AnnotationAssertion(rdfs:label :A \"never closed)\nSubClassOf(:A :B)");
    assert_eq!(
        errors(&text),
        [(
            "\"".to_string(),
            "Expected '\"' to end the string, found the end of the document".to_string()
        )]
    );
}

#[test]
fn arity_and_cardinality_are_checked() {
    for (body, at_text, message) in [
        (
            "SubClassOf(:A ObjectIntersectionOf(:B))",
            "ObjectIntersectionOf(:B)",
            "'ObjectIntersectionOf' needs at least 2 class expressions, found 1",
        ),
        (
            "SubClassOf(:A ObjectUnionOf(:B))",
            "ObjectUnionOf(:B)",
            "'ObjectUnionOf' needs at least 2 class expressions, found 1",
        ),
        (
            "EquivalentClasses(Annotation(rdfs:comment \"x\") :A)",
            "EquivalentClasses(Annotation(rdfs:comment \"x\") :A)",
            "'EquivalentClasses' needs at least 2 class expressions, found 1",
        ),
        (
            "DisjointUnion(:A :B)",
            "DisjointUnion(:A :B)",
            "'DisjointUnion' needs at least 2 class expressions, found 1",
        ),
        (
            "SubClassOf(:A ObjectOneOf())",
            "ObjectOneOf()",
            "'ObjectOneOf' needs at least 1 individual, found 0",
        ),
        (
            "DataPropertyRange(:dp DataOneOf())",
            "DataOneOf()",
            "'DataOneOf' needs at least 1 literal, found 0",
        ),
        (
            "DataPropertyRange(:dp DataUnionOf(xsd:string))",
            "DataUnionOf(xsd:string)",
            "'DataUnionOf' needs at least 2 data ranges, found 1",
        ),
        (
            "SameIndividual(:a)",
            "SameIndividual(:a)",
            "'SameIndividual' needs at least 2 individuals, found 1",
        ),
        (
            "DisjointDataProperties(:dp)",
            "DisjointDataProperties(:dp)",
            "'DisjointDataProperties' needs at least 2 data properties, found 1",
        ),
        (
            "SubObjectPropertyOf(ObjectPropertyChain(:p) :q)",
            "ObjectPropertyChain(:p)",
            "'ObjectPropertyChain' needs at least 2 object property expressions, found 1",
        ),
        (
            "HasKey(:C () ())",
            "HasKey(:C () ())",
            "'HasKey' needs at least one object or data property expression",
        ),
        (
            "SubClassOf(:A DataSomeValuesFrom(xsd:integer))",
            "xsd:integer",
            "'DataSomeValuesFrom' needs a data property and a data range",
        ),
        (
            "SubClassOf(:A DataAllValuesFrom(DataOneOf(\"a\") xsd:string))",
            "DataOneOf(\"a\")",
            "Expected a data property, found 'DataOneOf': only the last argument of 'DataAllValuesFrom' is a data range",
        ),
        (
            "SubClassOf(:A ObjectMinCardinality(-1 :p))",
            "-1",
            "Expected a non-negative integer, found '-1'",
        ),
        (
            "SubClassOf(:A ObjectMaxCardinality(1.5 :p))",
            "1.5",
            "Expected a non-negative integer, found '1.5'",
        ),
        (
            "SubClassOf(:A DataExactCardinality(\"1\" :dp))",
            "\"1\"",
            "Expected a non-negative integer, found a string",
        ),
        (
            "SubClassOf(:A ObjectExactCardinality(:p))",
            ":p",
            "Expected a non-negative integer, found ':p'",
        ),
        (
            "DataPropertyRange(:dp DatatypeRestriction(xsd:integer))",
            ")",
            "Expected a facet (an IRI such as xsd:minInclusive), found ')'",
        ),
        (
            "DataPropertyRange(:dp DatatypeRestriction(xsd:integer xsd:minInclusive))",
            ")",
            "Expected a literal (the facet's value), found ')'",
        ),
    ] {
        let got = one_error(body);
        assert_eq!(
            (got.0.as_str(), got.1.as_str()),
            (at_text, message),
            "{body}"
        );
    }
}

#[test]
fn what_is_expected_is_named() {
    for (body, at_text, message) in [
        (
            "SubClassOf(:A)",
            ")",
            "Expected a class expression, found ')'",
        ),
        (
            "SubClassOf(:A :B :C)",
            ":C",
            "Expected ')' to close 'SubClassOf(', found ':C'",
        ),
        (
            "EquivalentClasses(:A :B \"x\")",
            "\"x\"",
            "Expected a class expression or ')', found a string",
        ),
        (
            "SubClassOf(:A DataIntersectionOf(xsd:string xsd:integer))",
            "DataIntersectionOf",
            "Expected a class expression, found 'DataIntersectionOf'",
        ),
        (
            "SubClassOf :A :B",
            ":A",
            "Expected '(' after 'SubClassOf', found ':A'",
        ),
        (
            "SubClassOff(:A :B)",
            "SubClassOff",
            "Expected an axiom, an annotation or ')', found 'SubClassOff'",
        ),
        (
            "Class(:A)",
            "Class",
            "Expected an axiom, an annotation or ')', found 'Class'",
        ),
        (
            "Declaration(:A)",
            ":A",
            "Expected an entity (Class, Datatype, ObjectProperty, DataProperty, AnnotationProperty or NamedIndividual), found ':A'",
        ),
        (
            "ClassAssertion(:C \"x\")",
            "\"x\"",
            "Expected an individual, found a string",
        ),
        (
            "AnnotationAssertion(rdfs:label \"x\" :A)",
            "\"x\"",
            "Expected an IRI or an anonymous individual, found a string",
        ),
        (
            "HasKey(:C :p)",
            ":p",
            "Expected '(' and the key's object property expressions, found ':p'",
        ),
        (
            "SubClassOf(:A ObjectInverseOf(:p))",
            "ObjectInverseOf",
            "Expected a class expression, found 'ObjectInverseOf'",
        ),
        (
            "SubObjectPropertyOf(:p ObjectInverseOf(ObjectInverseOf(:q)))",
            "ObjectInverseOf",
            "Expected an IRI, found 'ObjectInverseOf'",
        ),
        (
            "DLSafeRule(Head())",
            "Head",
            "Expected 'Body', found 'Head'",
        ),
        (
            "DLSafeRule(Body(:A) Head())",
            ":A",
            "Expected an atom or ')', found ':A'",
        ),
        (
            "Prefix(ex:=<http://example.org/>)",
            "Prefix",
            "A 'Prefix' declaration comes before 'Ontology('",
        ),
        (
            "Declaration(Class(:A))\nImport(<http://example.org/i>)",
            "Import",
            "An 'Import' comes before the ontology's annotations and axioms",
        ),
        (
            "Declaration(Class(:A))\nAnnotation(rdfs:label \"o\")",
            "Annotation",
            "An ontology annotation comes before the axioms",
        ),
        (
            "Ontology()",
            "Ontology",
            "Expected an axiom: a document holds one ontology, found 'Ontology'",
        ),
    ] {
        let got = one_error(body);
        assert_eq!(
            (got.0.as_str(), got.1.as_str()),
            (at_text, message),
            "{body}"
        );
    }
}

#[test]
fn the_document_has_one_ontology_after_its_prefixes() {
    for (text, want) in [
        (
            "",
            vec![(
                "",
                "Expected 'Prefix(' or 'Ontology(', found the end of the document",
            )],
        ),
        (
            "   \n",
            vec![(
                "",
                "Expected 'Prefix(' or 'Ontology(', found the end of the document",
            )],
        ),
        (
            "Prefix(:=<http://example.org/>)",
            vec![(
                ")",
                "Expected 'Prefix(' or 'Ontology(', found the end of the document",
            )],
        ),
        (
            "Ontology(",
            vec![(
                "(",
                "Expected ')' to close 'Ontology(', found the end of the document",
            )],
        ),
        (
            "Ontology(<http://example.org/o>\nDeclaration(Class(<http://example.org/A>))",
            vec![(
                ")",
                "Expected ')' to close 'Ontology(', found the end of the document",
            )],
        ),
        (
            "Ontology() Ontology()",
            vec![(
                "Ontology",
                "Expected the end of the document after the ontology, found 'Ontology'",
            )],
        ),
        (
            "Ontology()\nDeclaration(Class(<http://example.org/A>)))",
            vec![(
                "Declaration",
                "Expected the end of the document after the ontology, found 'Declaration'",
            )],
        ),
        // Axioms with no ontology: one error, and the axioms are read.
        (
            "Declaration(Class(<http://example.org/A>))\nSubClassOf(<a> <http://example.org/B>)",
            vec![
                (
                    "Declaration",
                    "Expected 'Prefix(' or 'Ontology(', found 'Declaration'",
                ),
                ("<a>", "<a> is a relative IRI: an IRI in OWL 2 is absolute"),
            ],
        ),
        (
            "junk Ontology()",
            vec![("junk", "Expected 'Prefix(' or 'Ontology(', found 'junk'")],
        ),
        (
            "Prefix(ex=<http://example.org/>)\nOntology()",
            vec![("ex", "Expected a prefix name such as 'ex:', found 'ex'")],
        ),
        (
            "Prefix(ex:a=<http://example.org/>)\nOntology()",
            vec![("ex:a", "Expected a prefix name such as 'ex:', found 'ex:a'")],
        ),
        // SPARQL's PNAME_NS: a ':' only at the end.
        (
            "Prefix(a:b:=<http://example.org/>)\nOntology(Declaration(Class(a:b:C)))",
            vec![
                ("a:b:", "Expected a prefix name such as 'ex:', found 'a:b:'"),
                ("a:b:C", "Undeclared prefix 'a:'"),
            ],
        ),
        (
            "Prefix(::=<http://example.org/>)\nOntology()",
            vec![("::", "Expected a prefix name such as 'ex:', found '::'")],
        ),
        (
            "Prefix(ex:=ex:a)\nOntology()",
            vec![(
                "ex:a",
                "Expected a full IRI such as <http://example.org/>, found 'ex:a'",
            )],
        ),
        // A declaration that is not closed ends at the next one.
        (
            "Prefix(ex:=<http://example.org/>\nPrefix(a:=<http://example.org/a#>)\nOntology(Declaration(Class(a:A)))",
            vec![("Prefix", "Expected ')' to close 'Prefix(', found 'Prefix'")],
        ),
    ] {
        let got = errors(text);
        let got: Vec<_> = got.iter().map(|(a, m)| (a.as_str(), m.as_str())).collect();
        assert_eq!(got, want, "{text:?}");
    }
}

/// After an error the parse goes on at the next axiom: each typo is one
/// error, and the axioms around it are read.
#[test]
fn each_typo_is_one_error_and_the_rest_is_read() {
    let body = "Declaration(Class(:A))
SubClassOf(:A :B
SubClassOf(:B nope:C)
SubClassOf(:C ObjectSomeValuesFrom(:p :D)
Declaration(Class(:D))
EquivalentClasses(:A ObjectIntersectionOf(:B \"x\" :C))
DisjointClasses(:A :B)
SubClassOf(:A :B))
Declaration(Class(:E))";
    let text = in_ontology(body);
    let got = errors(&text);
    let got: Vec<_> = got.iter().map(|(a, m)| (a.as_str(), m.as_str())).collect();
    assert_eq!(
        got,
        [
            (
                "SubClassOf",
                "Expected ')' to close 'SubClassOf(', found 'SubClassOf'"
            ),
            ("nope:C", "Undeclared prefix 'nope:'"),
            (
                "Declaration",
                "Expected ')' to close 'SubClassOf(', found 'Declaration'"
            ),
            (
                "\"x\"",
                "Expected a class expression or ')', found a string"
            ),
            // The stray `)` closes the ontology: what follows is past it.
            (
                "Declaration",
                "Expected the end of the document after the ontology, found 'Declaration'"
            ),
        ]
    );
    // Every axiom was read, the broken ones as far as they go.
    let d = doc(&text);
    use Keyword as K;
    assert_eq!(
        keywords(&d),
        [
            K::Declaration,
            K::SubClassOf,
            K::SubClassOf,
            K::SubClassOf,
            K::Declaration,
            K::EquivalentClasses,
            K::DisjointClasses,
            K::SubClassOf,
            K::Declaration
        ]
    );
    // A broken axiom's span ends at its last token read.
    let second = d.top().nth(2).unwrap();
    assert_eq!(at(&text, d.nodes[second].span), "SubClassOf(:A :B");
}

/// Nesting deeper than [`MAX_NESTING`] is one error, read on a 2 MiB
/// stack, whatever nests: class expressions, annotations, parentheses.
#[test]
fn deep_nesting_is_an_error_not_a_crash() {
    let nested = |open: &str, close: &str, n: usize| {
        in_ontology(&format!(
            "SubClassOf(:A {}:B{})\nDeclaration(Class(:Z))",
            open.repeat(n),
            close.repeat(n)
        ))
    };
    let annotations = |n: usize| {
        in_ontology(&format!(
            "SubClassOf({}rdfs:label \"x\"{} :A :B)\nDeclaration(Class(:Z))",
            "Annotation(".repeat(n),
            ")".repeat(n)
        ))
    };
    let cases = [
        (
            nested("ObjectComplementOf(", ")", MAX_NESTING as usize - 2),
            0,
        ),
        (nested("ObjectComplementOf(", ")", MAX_NESTING as usize), 1),
        (nested("ObjectComplementOf(", ")", 200_000), 1),
        (
            nested("ObjectSomeValuesFrom(ObjectInverseOf(:p) ", ")", 100_000),
            1,
        ),
        (annotations(100_000), 1),
        (nested("(", ")", 300_000), 1),
        // Never closed: the ontology is not either.
        (in_ontology(&"(".repeat(300_000)), 2),
    ];
    for (text, n) in cases {
        let got = std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                let d = doc(&text);
                let messages: Vec<_> = d.errors.iter().map(|e| e.message().to_string()).collect();
                (messages, keywords(&d).len())
            })
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(got.0.len(), n, "{:?}", &got.0[..got.0.len().min(3)]);
        if n == 1 && got.0[0].starts_with("Nested") {
            assert_eq!(
                got.0[0],
                format!("Nested more than {MAX_NESTING} levels deep")
            );
            // The axiom after it is read.
            assert_eq!(got.1, 2);
        }
    }
}

#[test]
fn a_byte_order_mark_keeps_the_offsets_of_the_text() {
    for bom in ["", "\u{feff}"] {
        let text = format!("{bom}Ontology(SubClassOf(nope:A <http://example.org/B>))");
        let e = errors(&text);
        assert_eq!(
            e,
            [(
                "nope:A".to_string(),
                "Undeclared prefix 'nope:'".to_string()
            )]
        );
    }
}

#[test]
fn errors_stop_at_the_limit_and_say_so() {
    let body = "SubClassOf(nope:A nope:B)\n".repeat(MAX_ERRORS);
    let d = doc(&in_ontology(&body));
    assert_eq!(d.errors.len(), MAX_ERRORS + 1);
    assert_eq!(
        d.errors[MAX_ERRORS].message(),
        format!("More than {MAX_ERRORS} errors; the rest are not reported")
    );
    assert!(
        d.errors
            .windows(2)
            .all(|w| w[0].span.start <= w[1].span.start)
    );
}

/// Long tokens, long runs of one character, and text that is no token at
/// all: each is read once, whatever it is.
#[test]
fn long_tokens_and_runs_end() {
    let n = 1 << 20;
    let x = "x".repeat(n);
    for text in [
        in_ontology(&format!("AnnotationAssertion(rdfs:label :A \"{x}\")")),
        in_ontology(&format!("SubClassOf(<http://example.org/{x}> :B)")),
        in_ontology(&format!("SubClassOf(ex:{x} :B)")),
        in_ontology(&format!("# {x}\n").repeat(4)),
        in_ontology(&"\"".repeat(n)),
        in_ontology(&"<".repeat(n)),
        in_ontology(&"^".repeat(n)),
        in_ontology(&"@".repeat(n)),
        in_ontology(&"\\".repeat(n)),
        in_ontology(&"SubClassOf(".repeat(n / 16)),
        in_ontology(&")".repeat(n)),
        x.clone(),
        "(".repeat(n),
        format!("\"{}", "\\x".repeat(n)),
    ] {
        let d = doc(&text);
        // Every message quotes at most a short part of the text.
        assert!(d.errors.iter().all(|e| e.message().len() < 400));
    }
}

// ---- the parse the workspace sees ----

#[test]
fn the_parse_holds_the_errors_and_nothing_else() {
    let text = in_ontology(
        "Declaration(Class(:A))\nSubClassOf(:A nope:B)\nAnnotationAssertion(rdfs:label :A \"A\")",
    );
    let p = parse(&text, Some("file:///w/o.ofn"));
    assert_eq!(p.syntax, Syntax::OwlFunctional);
    assert_eq!(p.document_base.as_deref(), Some("file:///w/o.ofn"));
    assert_eq!(p.errors.len(), 1);
    assert_eq!(p.errors[0].code, "ofn-syntax");
    assert!(p.triples.is_empty() && p.refs.is_empty() && p.statements.is_empty());
    assert!(p.prefixes.is_empty() && p.prefix_uses.is_empty() && p.tokens.is_empty());
    assert!(p.comments.is_empty() && p.graphs.is_empty());
}

// ---- conformance and real files ----

/// The Functional-syntax texts of the approved W3C OWL 2 test cases
/// (`tests/w3c/owl2/`): every one parses with no error.
#[test]
fn w3c_owl2_functional_inputs_parse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/owl2/fs");
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ofn"))
        .collect();
    names.sort();
    let mut failures = Vec::new();
    for path in &names {
        let text = std::fs::read_to_string(path).unwrap();
        let d = doc(&text);
        if let Some(e) = d.errors.first() {
            failures.push(format!(
                "{}: {} at {:?}",
                path.file_name().unwrap().to_string_lossy(),
                e.message(),
                at(&text, e.span)
            ));
        }
        assert!(d.ontology.is_some());
    }
    eprintln!(
        "W3C OWL 2 Functional inputs: {}/{} parse",
        names.len() - failures.len(),
        names.len()
    );
    assert_eq!(names.len(), 78);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The OBO files `tools/corpus/fetch-ofn.sh` downloads into
/// `test/corpus-ofn/`: with `OFN_REAL=1`, each parses with no error.
#[test]
fn ofn_real_files() {
    if std::env::var("OFN_REAL").as_deref() != Ok("1") {
        eprintln!("skipped: set OFN_REAL=1 (after tools/corpus/fetch-ofn.sh)");
        return;
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let sources = std::fs::read_to_string(root.join("tools/corpus/ofn-sources.tsv")).unwrap();
    let names: Vec<&str> = sources
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split('\t').next().unwrap())
        .collect();
    assert!(!names.is_empty());
    for name in names {
        let path = root.join("test/corpus-ofn").join(name);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{name}: {e}; run tools/corpus/fetch-ofn.sh"));
        assert_eq!(
            crate::syntax::Syntax::of(None, name, &text),
            Syntax::OwlFunctional,
            "{name}"
        );
        let start = std::time::Instant::now();
        let d = doc(&text);
        let took = start.elapsed();
        let shown: Vec<_> = d
            .errors
            .iter()
            .take(5)
            .map(|e| (at(&text, e.span), e.message()))
            .collect();
        assert!(
            d.errors.is_empty(),
            "{name}: {} errors: {shown:#?}",
            d.errors.len()
        );
        eprintln!(
            "{name}: {} bytes, {} axioms, {} nodes, parsed in {took:?}",
            text.len(),
            d.top().count(),
            d.nodes.len()
        );
    }
}

/// Checking abbreviated IRIs takes time linear in the text, valid or not,
/// however long the namespace: a namespace of 40,000 bytes and 2,000
/// distinct names, then four times both.
#[test]
fn timing_abbreviated_iris_with_a_long_namespace_check_in_linear_time() {
    for bad in ["", "\u{FFF0}"] {
        let make = |s: usize| {
            let ns = format!("http://example.org/{}/", "a".repeat(40_000 * s));
            let axioms: String = (0..2_000 * s)
                .map(|k| format!("Declaration(Class(ex:c{k}{bad}))\n"))
                .collect();
            (format!("Prefix(ex:=<{ns}>)\nOntology({axioms})"), 2_000 * s)
        };
        let fastest = |text: &str| {
            (0..3)
                .map(|_| {
                    let start = std::time::Instant::now();
                    let d = doc(text);
                    (start.elapsed(), d.errors.len())
                })
                .min()
                .unwrap()
        };
        let ((small, n_small), (big, n_big)) = (make(1), make(4));
        let ((t_small, e_small), (t_big, e_big)) = (fastest(&small), fastest(&big));
        let want = |n| if bad.is_empty() { 0 } else { n };
        assert_eq!((e_small, e_big), (want(n_small), want(n_big)), "{bad:?}");
        eprintln!("{bad:?}: {t_small:?} at 1x, {t_big:?} at 4x");
        assert!(
            t_big < t_small * 4 + std::time::Duration::from_millis(5),
            "{bad:?}: {t_big:?} at 4x, {t_small:?} at 1x"
        );
    }
}

/// A generated document the size of `hp-edit.owl` (31 MB), in the shape
/// OWL API writes, parses in release mode within the budget.
#[test]
fn timing_a_large_document_parses_within_budget() {
    let text = large(32 << 20);
    let start = std::time::Instant::now();
    let d = doc(&text);
    let took = start.elapsed();
    assert!(
        d.errors.is_empty(),
        "{:?}",
        &d.errors[..d.errors.len().min(3)]
    );
    eprintln!("{} bytes, {} nodes: {took:?}", text.len(), d.nodes.len());
    assert!(took < LARGE_BUDGET, "{took:?} over {LARGE_BUDGET:?}");
}

/// The budget of [`timing_a_large_document_parses_within_budget`]. Measured
/// on 2026-11-22, release build with the shipped profile, on an Apple
/// M-series laptop at load average 2: 69 ms for 32 MiB (1.7 million nodes);
/// `hp-edit.owl` itself (31 MB) took 30 ms. The budget leaves ten times
/// that for a slower runner and a build without fat LTO.
const LARGE_BUDGET: std::time::Duration = std::time::Duration::from_millis(750);

/// A Functional document of about `bytes` bytes: classes with labels,
/// definitions, cross-references, subclass and equivalence axioms.
fn large(bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 4096);
    out.push_str("Prefix(:=<http://purl.obolibrary.org/obo/hp.owl#>)\nPrefix(dc:=<http://purl.org/dc/elements/1.1/>)\nPrefix(obo:=<http://purl.obolibrary.org/obo/>)\nPrefix(owl:=<http://www.w3.org/2002/07/owl#>)\nPrefix(rdf:=<http://www.w3.org/1999/02/22-rdf-syntax-ns#>)\nPrefix(xml:=<http://www.w3.org/XML/1998/namespace>)\nPrefix(xsd:=<http://www.w3.org/2001/XMLSchema#>)\nPrefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)\nPrefix(oboInOwl:=<http://www.geneontology.org/formats/oboInOwl#>)\n\n\nOntology(<http://purl.obolibrary.org/obo/hp.owl>\nImport(<http://purl.obolibrary.org/obo/hp/imports/pato_import.owl>)\nAnnotation(dc:title \"Human Phenotype Ontology\")\n\n");
    let mut i = 0u32;
    while out.len() < bytes {
        let c = format!("obo:HP_{i:07}");
        let p = format!("obo:HP_{:07}", i / 2);
        out.push_str(&format!(
            "# Class: {c} (Abnormality number {i})\n\n\
             AnnotationAssertion(Annotation(oboInOwl:hasDbXref \"PMID:{i}\") Annotation(oboInOwl:hasDbXref \"HPO:probinson\") obo:IAO_0000115 {c} \"An abnormality of a structure, number {i}, with a \\\"quoted\\\" word.\")\n\
             AnnotationAssertion(oboInOwl:created_by {c} \"doe\"^^xsd:string)\n\
             AnnotationAssertion(oboInOwl:creation_date {c} \"2009-01-01T00:00:00Z\"^^xsd:dateTime)\n\
             AnnotationAssertion(oboInOwl:hasExactSynonym {c} \"Abnormality {i}\"@en)\n\
             AnnotationAssertion(rdfs:label {c} \"Abnormality number {i}\"@en)\n\
             SubClassOf({c} {p})\n"
        ));
        if i.is_multiple_of(4) {
            out.push_str(&format!(
                "EquivalentClasses({c} ObjectIntersectionOf(obo:PATO_0000001 ObjectSomeValuesFrom(obo:RO_0000052 {p}) ObjectSomeValuesFrom(obo:RO_0002573 obo:PATO_0000460)))\n"
            ));
        }
        out.push_str(&format!("Declaration(Class({c}))\n\n"));
        i += 1;
    }
    out.push_str(")\n");
    out
}
