//! The mapping to RDF: each row of Tables 1 and 2 and each case of section
//! 2.3 against the triples the specification gives (graphs compared with
//! blank nodes renamed), the SWRL rules against ROBOT's output, the
//! references, statements and definitions the features read, and the
//! limits. The oracles (the W3C test ontologies that have an RDF/XML text
//! too, and ROBOT on real files) are in `tests/ofn_oracle.rs`.

use super::*;
use crate::syntax::{Form, Iri, Node, Role, Span};
use oxrdf::graph::CanonicalizationAlgorithm;
use oxrdf::{Graph, NamedOrBlankNode, Term};
use std::fmt::Write;

/// The prefixes of the documents below.
const HEAD: &str = "Prefix(:=<http://example.org/o#>)\nPrefix(ex:=<http://example.org/>)\n";

/// The Turtle prefixes of the expected graphs.
const TTL_HEAD: &str = "@prefix : <http://example.org/o#> .\n@prefix ex: <http://example.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n@prefix swrl: <http://www.w3.org/2003/11/swrl#> .\n";

/// The ontology header every expected graph has.
const ONTOLOGY: &str = "<http://example.org/o> a owl:Ontology .\n";

fn in_ontology(body: &str) -> String {
    format!("{HEAD}Ontology(<http://example.org/o>\n{body}\n)\n")
}

fn subject(t: Term) -> NamedOrBlankNode {
    match t {
        Term::NamedNode(n) => n.into(),
        Term::BlankNode(b) => b.into(),
        t => panic!("not a subject: {t}"),
    }
}

fn canonical(triples: impl IntoIterator<Item = oxrdf::Triple>) -> Graph {
    let mut g = Graph::new();
    for t in triples {
        g.insert(&t);
    }
    g.canonicalize(CanonicalizationAlgorithm::Unstable);
    g
}

/// One of our nodes as an oxrdf term (a Functional parse has no triple
/// terms and no directions).
fn term(n: &Node) -> Term {
    use oxrdf::{BlankNode, Literal, NamedNode};
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => BlankNode::new_unchecked(format!("b{}", b.0)).into(),
        Node::Literal {
            value,
            datatype,
            lang,
            ..
        } => match lang {
            Some(l) => Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()).into(),
            None => {
                Literal::new_typed_literal(&**value, NamedNode::new_unchecked(datatype.as_str()))
                    .into()
            }
        },
        Node::Triple(_) => unreachable!("no triple terms"),
    }
}

/// The graph of a parse.
fn graph_of(p: &Parsed) -> Graph {
    canonical(p.triples.iter().map(|t| {
        oxrdf::Triple::new(
            subject(term(&t.s)),
            oxrdf::NamedNode::new_unchecked(t.p.as_str()),
            term(&t.o),
        )
    }))
}

/// The graph of a Turtle text.
fn turtle(ttl: &str) -> Graph {
    let triples = oxttl::TurtleParser::new()
        .for_slice(ttl.as_bytes())
        .map(|t| t.unwrap_or_else(|e| panic!("{e}\n{ttl}")));
    canonical(triples)
}

/// `None` when the graphs are equal, else the triples only one has.
fn difference(ours: &Graph, theirs: &Graph, name: &str) -> Option<String> {
    if ours == theirs {
        return None;
    }
    let mut out = format!("ours {} triples, {name} {}\n", ours.len(), theirs.len());
    for (who, x, y) in [("ours", ours, theirs), (name, theirs, ours)] {
        for t in x.iter().filter(|t| !y.contains(*t)).take(20) {
            writeln!(out, "  only {who}: {t}").unwrap();
        }
    }
    Some(out)
}

/// The parse of `text`, which has no error.
#[track_caller]
fn clean(text: &str) -> Parsed {
    let p = parse(text, Some("file:///w/o.ofn"));
    let shown: Vec<_> = p.errors.iter().map(|e| e.message()).collect();
    assert!(shown.is_empty(), "{shown:#?}\n{text}");
    p
}

/// `body`, in an ontology, maps to the header and `expected` (Turtle,
/// after [`TTL_HEAD`]).
#[track_caller]
fn maps(body: &str, expected: &str) {
    let text = in_ontology(body);
    let ours = graph_of(&clean(&text));
    let theirs = turtle(&format!("{TTL_HEAD}{ONTOLOGY}{expected}"));
    if let Some(d) = difference(&ours, &theirs, "expected") {
        panic!("{body}\n{d}");
    }
}

// ---- Table 1: the ontology ----

#[test]
fn the_ontology_header() {
    let text = format!(
        "{HEAD}Ontology(<http://example.org/o> <http://example.org/o/1>\nImport(<http://example.org/a>)\nImport(ex:b)\nAnnotation(rdfs:comment \"c\")\n)\n"
    );
    let theirs = turtle(&format!(
        "{TTL_HEAD}<http://example.org/o> a owl:Ontology ; owl:versionIRI <http://example.org/o/1> ; owl:imports ex:a, ex:b ; rdfs:comment \"c\" .\n"
    ));
    assert_eq!(
        difference(&graph_of(&clean(&text)), &theirs, "expected"),
        None
    );
    // No IRI: a blank node, which the imports and annotations name.
    let text =
        format!("{HEAD}Ontology(Import(ex:a) Annotation(rdfs:label \"l\") Declaration(Class(:A)))");
    let theirs = turtle(&format!(
        "{TTL_HEAD}[ a owl:Ontology ; owl:imports ex:a ; rdfs:label \"l\" ] .\n:A a owl:Class .\n"
    ));
    assert_eq!(
        difference(&graph_of(&clean(&text)), &theirs, "expected"),
        None
    );
    // Empty.
    let p = clean("Ontology()");
    assert_eq!(p.triples.len(), 1);
}

#[test]
fn declarations_of_the_six_kinds() {
    maps(
        "Declaration(Class(:C)) Declaration(Datatype(:D)) Declaration(ObjectProperty(:op))\nDeclaration(DataProperty(:dp)) Declaration(AnnotationProperty(:ap)) Declaration(NamedIndividual(:a))",
        ":C a owl:Class . :D a rdfs:Datatype . :op a owl:ObjectProperty . :dp a owl:DatatypeProperty . :ap a owl:AnnotationProperty . :a a owl:NamedIndividual .",
    );
}

// ---- Table 1: expressions ----

#[test]
fn property_expressions_and_data_ranges() {
    maps(
        "SubObjectPropertyOf(ObjectInverseOf(:p) :q)",
        "[ owl:inverseOf :p ] rdfs:subPropertyOf :q .",
    );
    maps(
        "DatatypeDefinition(:d DataIntersectionOf(xsd:integer DataComplementOf(xsd:short)))",
        ":d owl:equivalentClass [ a rdfs:Datatype ; owl:intersectionOf ( xsd:integer [ a rdfs:Datatype ; owl:datatypeComplementOf xsd:short ] ) ] .",
    );
    maps(
        "DatatypeDefinition(:d DataUnionOf(xsd:integer xsd:string xsd:boolean))",
        ":d owl:equivalentClass [ a rdfs:Datatype ; owl:unionOf ( xsd:integer xsd:string xsd:boolean ) ] .",
    );
    maps(
        "DatatypeDefinition(:d DataOneOf(\"a\" \"1\"^^xsd:integer \"b\"@en))",
        ":d owl:equivalentClass [ a rdfs:Datatype ; owl:oneOf ( \"a\" 1 \"b\"@en ) ] .",
    );
    maps(
        "DatatypeDefinition(:d DatatypeRestriction(xsd:integer xsd:minInclusive \"1\"^^xsd:integer xsd:maxExclusive \"9\"^^xsd:integer))",
        ":d owl:equivalentClass [ a rdfs:Datatype ; owl:onDatatype xsd:integer ; owl:withRestrictions ( [ xsd:minInclusive 1 ] [ xsd:maxExclusive 9 ] ) ] .",
    );
}

#[test]
fn class_expressions() {
    maps(
        "SubClassOf(:A ObjectIntersectionOf(:B ObjectUnionOf(:C :D :E) ObjectComplementOf(:F)))",
        ":A rdfs:subClassOf [ a owl:Class ; owl:intersectionOf ( :B [ a owl:Class ; owl:unionOf ( :C :D :E ) ] [ a owl:Class ; owl:complementOf :F ] ) ] .",
    );
    maps(
        "SubClassOf(:A ObjectOneOf(:a _:b))",
        ":A rdfs:subClassOf [ a owl:Class ; owl:oneOf ( :a _:b ) ] .",
    );
    maps(
        "SubClassOf(:A ObjectSomeValuesFrom(:p :B)) SubClassOf(:A ObjectAllValuesFrom(ObjectInverseOf(:p) :B))",
        ":A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :p ; owl:someValuesFrom :B ] , [ a owl:Restriction ; owl:onProperty [ owl:inverseOf :p ] ; owl:allValuesFrom :B ] .",
    );
    maps(
        "SubClassOf(:A ObjectHasValue(:p :a)) SubClassOf(:A ObjectHasSelf(:p))",
        ":A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :p ; owl:hasValue :a ] , [ a owl:Restriction ; owl:onProperty :p ; owl:hasSelf true ] .",
    );
}

#[test]
fn cardinalities_with_and_without_qualification() {
    let nni = |n: u32| format!("\"{n}\"^^xsd:nonNegativeInteger");
    for (kw, plain, qualified) in [
        ("Min", "minCardinality", "minQualifiedCardinality"),
        ("Max", "maxCardinality", "maxQualifiedCardinality"),
        ("Exact", "cardinality", "qualifiedCardinality"),
    ] {
        maps(
            &format!(
                "SubClassOf(:A Object{kw}Cardinality(1 :p)) SubClassOf(:A Object{kw}Cardinality(2 :p :B))"
            ),
            &format!(
                ":A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :p ; owl:{plain} {} ] , [ a owl:Restriction ; owl:onProperty :p ; owl:{qualified} {} ; owl:onClass :B ] .",
                nni(1),
                nni(2)
            ),
        );
        maps(
            &format!(
                "SubClassOf(:A Data{kw}Cardinality(0 :d)) SubClassOf(:A Data{kw}Cardinality(3 :d xsd:string))"
            ),
            &format!(
                ":A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :d ; owl:{plain} {} ] , [ a owl:Restriction ; owl:onProperty :d ; owl:{qualified} {} ; owl:onDataRange xsd:string ] .",
                nni(0),
                nni(3)
            ),
        );
    }
}

#[test]
fn data_restrictions_on_one_property_and_on_several() {
    maps(
        "SubClassOf(:A DataSomeValuesFrom(:d xsd:integer)) SubClassOf(:A DataAllValuesFrom(:d :e :f xsd:integer)) SubClassOf(:A DataHasValue(:d \"x\"))",
        ":A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :d ; owl:someValuesFrom xsd:integer ] , [ a owl:Restriction ; owl:onProperties ( :d :e :f ) ; owl:allValuesFrom xsd:integer ] , [ a owl:Restriction ; owl:onProperty :d ; owl:hasValue \"x\" ] .",
    );
    maps(
        "SubClassOf(:A DataSomeValuesFrom(:d :e DataComplementOf(xsd:integer)))",
        ":A rdfs:subClassOf [ a owl:Restriction ; owl:onProperties ( :d :e ) ; owl:someValuesFrom [ a rdfs:Datatype ; owl:datatypeComplementOf xsd:integer ] ] .",
    );
}

// ---- Table 1: axioms ----

#[test]
fn class_axioms_two_operands_and_more() {
    maps(
        "EquivalentClasses(:A :B :C) DisjointClasses(:A :B) DisjointClasses(:A :B :C) DisjointUnion(:A :B :C)",
        ":A owl:equivalentClass :B . :B owl:equivalentClass :C . :A owl:disjointWith :B .\n[ a owl:AllDisjointClasses ; owl:members ( :A :B :C ) ] . :A owl:disjointUnionOf ( :B :C ) .",
    );
}

#[test]
fn object_property_axioms() {
    maps(
        "SubObjectPropertyOf(:p :q) SubObjectPropertyOf(ObjectPropertyChain(:p :q :r) :s)\nEquivalentObjectProperties(:p :q :r) DisjointObjectProperties(:p :q) DisjointObjectProperties(:p :q :r)\nObjectPropertyDomain(:p :A) ObjectPropertyRange(:p :B) InverseObjectProperties(:p :q)",
        ":p rdfs:subPropertyOf :q . :s owl:propertyChainAxiom ( :p :q :r ) .\n:p owl:equivalentProperty :q . :q owl:equivalentProperty :r . :p owl:propertyDisjointWith :q .\n[ a owl:AllDisjointProperties ; owl:members ( :p :q :r ) ] .\n:p rdfs:domain :A ; rdfs:range :B ; owl:inverseOf :q .",
    );
    maps(
        "FunctionalObjectProperty(:p) InverseFunctionalObjectProperty(:p) ReflexiveObjectProperty(:p) IrreflexiveObjectProperty(:p) SymmetricObjectProperty(:p) AsymmetricObjectProperty(:p) TransitiveObjectProperty(ObjectInverseOf(:q))",
        ":p a owl:FunctionalProperty , owl:InverseFunctionalProperty , owl:ReflexiveProperty , owl:IrreflexiveProperty , owl:SymmetricProperty , owl:AsymmetricProperty .\n[ owl:inverseOf :q ] a owl:TransitiveProperty .",
    );
}

#[test]
fn data_property_and_datatype_axioms() {
    maps(
        "SubDataPropertyOf(:d :e) EquivalentDataProperties(:d :e) DisjointDataProperties(:d :e) DisjointDataProperties(:d :e :f)\nDataPropertyDomain(:d :A) DataPropertyRange(:d xsd:string) FunctionalDataProperty(:d) DatatypeDefinition(:t xsd:integer)",
        ":d rdfs:subPropertyOf :e ; owl:equivalentProperty :e ; owl:propertyDisjointWith :e ; rdfs:domain :A ; rdfs:range xsd:string ; a owl:FunctionalProperty .\n[ a owl:AllDisjointProperties ; owl:members ( :d :e :f ) ] . :t owl:equivalentClass xsd:integer .",
    );
}

#[test]
fn keys_and_assertions() {
    maps(
        "HasKey(:A (:p ObjectInverseOf(:q)) (:d)) HasKey(:A () (:d :e))",
        ":A owl:hasKey ( :p [ owl:inverseOf :q ] :d ) , ( :d :e ) .",
    );
    maps(
        "SameIndividual(:a :b :c) DifferentIndividuals(:a :b) DifferentIndividuals(:a :b :c)\nClassAssertion(:A :a) ClassAssertion(ObjectComplementOf(:A) :b)\nObjectPropertyAssertion(:p :a :b) ObjectPropertyAssertion(ObjectInverseOf(:p) :a :c)\nDataPropertyAssertion(:d :a \"1\"^^xsd:integer)",
        ":a owl:sameAs :b . :b owl:sameAs :c . :a owl:differentFrom :b . [ a owl:AllDifferent ; owl:members ( :a :b :c ) ] .\n:a a :A . :b a [ a owl:Class ; owl:complementOf :A ] .\n:a :p :b . :c :p :a . :a :d 1 .",
    );
    maps(
        "NegativeObjectPropertyAssertion(:p :a :b) NegativeDataPropertyAssertion(:d :a \"x\"@en)",
        "[ a owl:NegativePropertyAssertion ; owl:sourceIndividual :a ; owl:assertionProperty :p ; owl:targetIndividual :b ] .\n[ a owl:NegativePropertyAssertion ; owl:sourceIndividual :a ; owl:assertionProperty :d ; owl:targetValue \"x\"@en ] .",
    );
}

#[test]
fn annotation_axioms() {
    maps(
        "AnnotationAssertion(rdfs:label :A \"A\") AnnotationAssertion(rdfs:seeAlso :A :B) AnnotationAssertion(rdfs:comment _:x \"c\")\nSubAnnotationPropertyOf(:ap :aq) AnnotationPropertyDomain(:ap :A) AnnotationPropertyRange(:ap :B)",
        ":A rdfs:label \"A\" ; rdfs:seeAlso :B . _:x rdfs:comment \"c\" .\n:ap rdfs:subPropertyOf :aq ; rdfs:domain :A ; rdfs:range :B .",
    );
}

/// An anonymous individual is one blank node in the whole document; a
/// generated blank node is fresh in each application of a rule.
#[test]
fn an_anonymous_individual_used_twice_is_one_node() {
    maps(
        "ClassAssertion(:A _:x) ObjectPropertyAssertion(:p _:x :b) AnnotationAssertion(rdfs:label _:x \"x\")\nSubClassOf(:A ObjectSomeValuesFrom(:p :B)) SubClassOf(:C ObjectSomeValuesFrom(:p :B))",
        "_:x a :A ; :p :b ; rdfs:label \"x\" .\n:A rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :p ; owl:someValuesFrom :B ] .\n:C rdfs:subClassOf [ a owl:Restriction ; owl:onProperty :p ; owl:someValuesFrom :B ] .",
    );
    let p = clean(&in_ontology(
        "ClassAssertion(:A _:x) ClassAssertion(:B _:x) ClassAssertion(:C _:y)",
    ));
    let s: Vec<_> = p.triples[1..].iter().map(|t| t.s.clone()).collect();
    assert_eq!(s[0], s[1]);
    assert_ne!(s[1], s[2]);
}

#[test]
fn literals_typed_tagged_plain_and_escaped() {
    maps(
        "AnnotationAssertion(rdfs:label :A \"plain\") AnnotationAssertion(rdfs:label :A \"tagged\"@en-GB)\nAnnotationAssertion(rdfs:label :A \"7\"^^xsd:integer) AnnotationAssertion(rdfs:label :A \"say \\\"hi\\\" \\\\ there\")\nAnnotationAssertion(rdfs:label :A \"pl@fr\"^^rdf:PlainLiteral) AnnotationAssertion(rdfs:label :A \"a@b@\"^^rdf:PlainLiteral)\nAnnotationAssertion(rdfs:label :A \"s\"^^xsd:string)",
        ":A rdfs:label \"plain\" , \"tagged\"@en-gb , 7 , \"say \\\"hi\\\" \\\\ there\" , \"pl\"@fr , \"a@b\" , \"s\" .",
    );
    // The tag is lowercased, as the other frontends keep it.
    let p = clean(&in_ontology(
        "AnnotationAssertion(rdfs:label :A \"x\"@EN-gb)",
    ));
    match &p.triples[1].o {
        Node::Literal { value, lang, .. } => {
            assert_eq!(&**value, "x");
            assert_eq!(lang.map(|l| l.as_str()), Some("en-gb"));
        }
        other => panic!("{other:?}"),
    }
}

/// An `rdf:PlainLiteral` whose lexical form is not `"abc@langTag"` (no
/// `@`, or a tag that is not a language tag) has no row in Table 1: an
/// error at its quoted string, its item gives nothing, and the items
/// after it are mapped. The forms Table 1 maps still map (above).
#[test]
fn an_invalid_plain_literal_is_an_error_and_gives_nothing() {
    for (string, message) in [
        (
            "\"Bonn@ x\"",
            "Invalid language tag ' x' in an rdf:PlainLiteral",
        ),
        (
            "\"Bonn@de-\"",
            "Invalid language tag 'de-' in an rdf:PlainLiteral",
        ),
        (
            "\"Bonn\"",
            "An rdf:PlainLiteral is \"text@tag\" or \"text@\": no '@' here",
        ),
    ] {
        let text = in_ontology(&format!(
            "SubClassOf(:A :B)\nAnnotationAssertion(rdfs:label :A {string}^^rdf:PlainLiteral)\nSubClassOf(:B :C)"
        ));
        let p = parse(&text, None);
        assert_eq!(p.errors.len(), 1, "{string}: {:?}", p.errors);
        let e = &p.errors[0];
        assert_eq!(e.code, CODE);
        assert!(
            e.message().starts_with(message),
            "{string}: {}",
            e.message()
        );
        assert_eq!(at(&text, e.span), string);
        let mapped: Vec<_> = p.statements.iter().map(|s| at(&text, s.span)).collect();
        assert_eq!(
            mapped,
            [
                "Ontology(<http://example.org/o>",
                "SubClassOf(:A :B)",
                "SubClassOf(:B :C)"
            ],
            "{string}"
        );
        assert!(
            p.triples
                .iter()
                .all(|t| !matches!(t.o, Node::Literal { .. })),
            "{string}"
        );
    }
    // Among the grammar's errors, in text order.
    let text = in_ontology(
        "SubClassOf(:A nope:B)\nAnnotationAssertion(rdfs:label :A \"x\"^^rdf:PlainLiteral)\nSubClassOf(:A nope:C)",
    );
    let p = parse(&text, None);
    let shown: Vec<_> = p.errors.iter().map(|e| at(&text, e.span)).collect();
    assert_eq!(shown, ["nope:B", "\"x\"", "nope:C"], "{:?}", p.errors);
}

// ---- Table 2 and section 2.3: annotations ----

/// Section 2.2's examples: an annotation, and an annotated annotation, on
/// the ontology and on an axiom.
#[test]
fn annotations_and_annotations_of_annotations() {
    let text = format!(
        "{HEAD}Ontology(<http://example.org/o>\nAnnotation(rdfs:label \"o\")\nAnnotation(Annotation(:author :Seth) rdfs:label \"Peter Griffin\")\n)"
    );
    let theirs = turtle(&format!(
        "{TTL_HEAD}{ONTOLOGY}<http://example.org/o> rdfs:label \"o\" , \"Peter Griffin\" .\n[ a owl:Annotation ; owl:annotatedSource <http://example.org/o> ; owl:annotatedProperty rdfs:label ; owl:annotatedTarget \"Peter Griffin\" ; :author :Seth ] .\n"
    ));
    assert_eq!(
        difference(&graph_of(&clean(&text)), &theirs, "expected"),
        None
    );
    maps(
        "AnnotationAssertion(Annotation(Annotation(:author :Seth) rdfs:label \"Peter Griffin\") rdfs:comment :Peter \"c\")",
        ":Peter rdfs:comment \"c\" .\n_:x a owl:Axiom ; owl:annotatedSource :Peter ; owl:annotatedProperty rdfs:comment ; owl:annotatedTarget \"c\" ; rdfs:label \"Peter Griffin\" .\n[ a owl:Annotation ; owl:annotatedSource _:x ; owl:annotatedProperty rdfs:label ; owl:annotatedTarget \"Peter Griffin\" ; :author :Seth ] .",
    );
}

/// Section 2.3.1's example, and the other axioms with a main triple.
#[test]
fn an_annotated_axiom_with_a_main_triple() {
    maps(
        "SubClassOf(Annotation(rdfs:comment \"Children are people.\") :Child :Person)",
        ":Child rdfs:subClassOf :Person .\n[ a owl:Axiom ; owl:annotatedSource :Child ; owl:annotatedProperty rdfs:subClassOf ; owl:annotatedTarget :Person ; rdfs:comment \"Children are people.\" ] .",
    );
    // A declaration: its rdf:type triple.
    maps(
        "Declaration(Annotation(rdfs:comment \"d\") Class(:E))",
        ":E a owl:Class .\n[ a owl:Axiom ; owl:annotatedSource :E ; owl:annotatedProperty rdf:type ; owl:annotatedTarget owl:Class ; rdfs:comment \"d\" ] .",
    );
    // Two disjoint classes, an inverse assertion, a datatype definition.
    maps(
        "DisjointClasses(Annotation(rdfs:comment \"x\") :A :B) ObjectPropertyAssertion(Annotation(rdfs:comment \"y\") ObjectInverseOf(:p) :a :b)",
        ":A owl:disjointWith :B . :b :p :a .\n[ a owl:Axiom ; owl:annotatedSource :A ; owl:annotatedProperty owl:disjointWith ; owl:annotatedTarget :B ; rdfs:comment \"x\" ] .\n[ a owl:Axiom ; owl:annotatedSource :b ; owl:annotatedProperty :p ; owl:annotatedTarget :a ; rdfs:comment \"y\" ] .",
    );
}

/// Section 2.3.1's examples of side triples: a property chain and a key.
#[test]
fn an_annotated_axiom_with_side_triples() {
    maps(
        "SubObjectPropertyOf(Annotation(rdfs:comment \"An aunt is a mother's sister.\") ObjectPropertyChain(:hasMother :hasSister) :hasAunt)",
        ":hasAunt owl:propertyChainAxiom _:y1 . _:y1 rdf:first :hasMother ; rdf:rest ( :hasSister ) .\n[ a owl:Axiom ; owl:annotatedSource :hasAunt ; owl:annotatedProperty owl:propertyChainAxiom ; owl:annotatedTarget _:y1 ; rdfs:comment \"An aunt is a mother's sister.\" ] .",
    );
    maps(
        "HasKey(Annotation(rdfs:comment \"SSN uniquely determines a person.\") :Person () (:hasSSN))",
        ":Person owl:hasKey _:y . _:y rdf:first :hasSSN ; rdf:rest rdf:nil .\n[ a owl:Axiom ; owl:annotatedSource :Person ; owl:annotatedProperty owl:hasKey ; owl:annotatedTarget _:y ; rdfs:comment \"SSN uniquely determines a person.\" ] .",
    );
    maps(
        "DisjointUnion(Annotation(rdfs:comment \"u\") :A :B :C)",
        ":A owl:disjointUnionOf _:l . _:l rdf:first :B ; rdf:rest ( :C ) .\n[ a owl:Axiom ; owl:annotatedSource :A ; owl:annotatedProperty owl:disjointUnionOf ; owl:annotatedTarget _:l ; rdfs:comment \"u\" ] .",
    );
}

/// Section 2.3.2's example: each pair reified, the annotations repeated.
/// The middle operand is one node in both pairs.
#[test]
fn an_annotated_nary_axiom_is_reified_per_pair() {
    maps(
        "SameIndividual(Annotation(:source :Fox) :Meg :Megan :Megan_Griffin)",
        ":Meg owl:sameAs :Megan . :Megan owl:sameAs :Megan_Griffin .\n[ a owl:Axiom ; owl:annotatedSource :Meg ; owl:annotatedProperty owl:sameAs ; owl:annotatedTarget :Megan ; :source :Fox ] .\n[ a owl:Axiom ; owl:annotatedSource :Megan ; owl:annotatedProperty owl:sameAs ; owl:annotatedTarget :Megan_Griffin ; :source :Fox ] .",
    );
    maps(
        "EquivalentClasses(Annotation(Annotation(rdfs:label \"x\") rdfs:comment \"c\") :A ObjectUnionOf(:B :C) :D)",
        ":A owl:equivalentClass _:u . _:u owl:equivalentClass :D . _:u a owl:Class ; owl:unionOf ( :B :C ) .\n_:x1 a owl:Axiom ; owl:annotatedSource :A ; owl:annotatedProperty owl:equivalentClass ; owl:annotatedTarget _:u ; rdfs:comment \"c\" .\n[ a owl:Annotation ; owl:annotatedSource _:x1 ; owl:annotatedProperty rdfs:comment ; owl:annotatedTarget \"c\" ; rdfs:label \"x\" ] .\n_:x2 a owl:Axiom ; owl:annotatedSource _:u ; owl:annotatedProperty owl:equivalentClass ; owl:annotatedTarget :D ; rdfs:comment \"c\" .\n[ a owl:Annotation ; owl:annotatedSource _:x2 ; owl:annotatedProperty rdfs:comment ; owl:annotatedTarget \"c\" ; rdfs:label \"x\" ] .",
    );
    let p = clean(&in_ontology(
        "EquivalentClasses(:A ObjectUnionOf(:B :C) ObjectSomeValuesFrom(:p :D))",
    ));
    // One union and one restriction: the middle operand is made once.
    let classes = p
        .triples
        .iter()
        .filter(|t| matches!(&t.o, Node::Iri(i) if i.as_str().ends_with("#Class")))
        .count();
    assert_eq!(classes, 1);
}

/// Section 2.3.3's example, and the other axioms with a blank node.
#[test]
fn an_annotated_axiom_on_its_blank_node() {
    maps(
        "NegativeObjectPropertyAssertion(Annotation(:author :Seth) :brotherOf :Chris :Stewie)",
        "[ a owl:NegativePropertyAssertion ; owl:sourceIndividual :Chris ; owl:assertionProperty :brotherOf ; owl:targetIndividual :Stewie ; :author :Seth ] .",
    );
    maps(
        "DisjointClasses(Annotation(rdfs:comment \"d\") :A :B :C) DifferentIndividuals(Annotation(rdfs:comment \"e\") :a :b :c) DisjointDataProperties(Annotation(rdfs:comment \"f\") :d :e :f)",
        "[ a owl:AllDisjointClasses ; owl:members ( :A :B :C ) ; rdfs:comment \"d\" ] .\n[ a owl:AllDifferent ; owl:members ( :a :b :c ) ; rdfs:comment \"e\" ] .\n[ a owl:AllDisjointProperties ; owl:members ( :d :e :f ) ; rdfs:comment \"f\" ] .",
    );
}

// ---- SWRL, as OWL API writes it ----

/// What ROBOT 1.9.11 (OWL API 4.5.29) writes in Turtle for the rule of
/// `ROBOT_RULE` in `tests.rs`, its declarations left out.
const ROBOT_RULE_TTL: &str = "<urn:swrl:var#x> a swrl:Variable . <urn:swrl:var#y> a swrl:Variable . <urn:swrl:var#a> a swrl:Variable .
[ a swrl:Imp ;
  swrl:body [ a swrl:AtomList ; rdf:first [ a swrl:ClassAtom ; swrl:classPredicate <http://example.org/r#Person> ; swrl:argument1 <urn:swrl:var#x> ] ;
    rdf:rest [ a swrl:AtomList ; rdf:first [ a swrl:IndividualPropertyAtom ; swrl:propertyPredicate <http://example.org/r#p> ; swrl:argument1 <urn:swrl:var#x> ; swrl:argument2 <urn:swrl:var#y> ] ;
    rdf:rest [ a swrl:AtomList ; rdf:first [ a swrl:DatavaluedPropertyAtom ; swrl:propertyPredicate <http://example.org/r#age> ; swrl:argument1 <urn:swrl:var#x> ; swrl:argument2 <urn:swrl:var#a> ] ;
    rdf:rest [ a swrl:AtomList ; rdf:first [ a swrl:DataRangeAtom ; swrl:dataRange xsd:integer ; swrl:argument1 <urn:swrl:var#a> ] ;
    rdf:rest [ a swrl:AtomList ; rdf:first [ a swrl:BuiltinAtom ; swrl:builtin <http://www.w3.org/2003/11/swrlb#greaterThan> ; swrl:arguments [ a rdf:List ; rdf:first <urn:swrl:var#a> ; rdf:rest [ a rdf:List ; rdf:first 18 ; rdf:rest rdf:nil ] ] ] ;
    rdf:rest [ a swrl:AtomList ; rdf:first [ a swrl:SameIndividualAtom ; swrl:argument1 <urn:swrl:var#x> ; swrl:argument2 <urn:swrl:var#y> ] ;
    rdf:rest [ a swrl:AtomList ; rdf:first [ a swrl:DifferentIndividualsAtom ; swrl:argument1 <urn:swrl:var#x> ; swrl:argument2 <http://example.org/r#i> ] ;
    rdf:rest rdf:nil ] ] ] ] ] ] ] ;
  swrl:head [ a swrl:AtomList ; rdf:first [ a swrl:ClassAtom ; swrl:classPredicate <http://example.org/r#Adult> ; swrl:argument1 <urn:swrl:var#x> ] ; rdf:rest rdf:nil ]
] .
";

#[test]
fn swrl_rules_map_as_robot_writes_them() {
    let rule = super::tests::ROBOT_RULE;
    let p = clean(rule);
    let theirs = turtle(&format!(
        "{TTL_HEAD}<http://example.org/r> a owl:Ontology .\n<http://example.org/r#Adult> a owl:Class . <http://example.org/r#Person> a owl:Class . <http://example.org/r#p> a owl:ObjectProperty . <http://example.org/r#age> a owl:DatatypeProperty . <http://example.org/r#i> a owl:NamedIndividual .\n{ROBOT_RULE_TTL}"
    ));
    assert_eq!(difference(&graph_of(&p), &theirs, "ROBOT"), None);
    // An empty body, an annotation on the rule, a class expression in an
    // atom: as ROBOT writes them too.
    maps(
        "DLSafeRule(Annotation(rdfs:comment \"r\") Body() Head(ClassAtom(ObjectSomeValuesFrom(:p :B) Variable(<urn:swrl:var#x>))))",
        "<urn:swrl:var#x> a swrl:Variable .\n[ rdfs:comment \"r\" ; a swrl:Imp ; swrl:body rdf:nil ;\n  swrl:head [ a swrl:AtomList ; rdf:first [ a swrl:ClassAtom ; swrl:classPredicate [ a owl:Restriction ; owl:onProperty :p ; owl:someValuesFrom :B ] ; swrl:argument1 <urn:swrl:var#x> ] ; rdf:rest rdf:nil ] ] .",
    );
}

// ---- what the features read ----

fn at(text: &str, span: Span) -> &str {
    &text[span.start as usize..span.end as usize]
}

/// Each IRI written in a mapped item is a reference at its text, in text
/// order, with its role; an IRI is defined at its declaration, or where an
/// item first has it as subject when the document declares it nowhere.
#[test]
fn references_are_exact_with_their_roles_and_definitions() {
    let text = in_ontology(
        "Declaration(Class(:A))\nSubClassOf(:A <http://example.org/o#B>)\nAnnotationAssertion(rdfs:label :A \"a\"^^xsd:string)\nObjectPropertyAssertion(:p :x :y)\nSubClassOf(:C :A)\nClassAssertion(:K :x)\nSubClassOf(:C :K)",
    );
    let p = clean(&text);
    let refs: Vec<_> = p
        .refs
        .iter()
        .map(|r| (at(&text, r.span), r.role, r.form))
        .collect();
    let pn = |n: u32| Form::PName { prefix_len: n };
    use Role::*;
    assert_eq!(
        refs,
        [
            ("<http://example.org/o>", Subject, Form::Iri),
            (":A", Subject, pn(0)),
            (":A", About, pn(0)),
            ("<http://example.org/o#B>", Object, Form::Iri),
            ("rdfs:label", Predicate, pn(4)),
            (":A", About, pn(0)),
            ("xsd:string", Datatype, pn(3)),
            (":p", Predicate, pn(0)),
            (":x", Subject, pn(0)),
            (":y", Object, pn(0)),
            (":C", Subject, pn(0)),
            (":A", Object, pn(0)),
            (":K", Object, pn(0)),
            (":x", About, pn(0)),
            (":C", About, pn(0)),
            (":K", Object, pn(0)),
        ]
    );
    // One statement per item, from its keyword to its `)`, with the IRI it
    // defines; the header from `Ontology` to the ontology IRI.
    let statements: Vec<_> = p
        .statements
        .iter()
        .map(|s| {
            (
                at(&text, s.span),
                s.subject.map(|(i, sp)| (i.to_string(), at(&text, sp))),
            )
        })
        .collect();
    let o = |iri: &str, written: &'static str| Some((iri.to_string(), written));
    assert_eq!(
        statements,
        [
            (
                "Ontology(<http://example.org/o>",
                o("http://example.org/o", "<http://example.org/o>")
            ),
            ("Declaration(Class(:A))", o("http://example.org/o#A", ":A")),
            ("SubClassOf(:A <http://example.org/o#B>)", None),
            ("AnnotationAssertion(rdfs:label :A \"a\"^^xsd:string)", None),
            (
                "ObjectPropertyAssertion(:p :x :y)",
                o("http://example.org/o#x", ":x")
            ),
            ("SubClassOf(:C :A)", o("http://example.org/o#C", ":C")),
            ("ClassAssertion(:K :x)", None),
            ("SubClassOf(:C :K)", None),
        ]
    );
    // Each triple is in its item's statement, its predicate and object at
    // their text.
    let shown: Vec<_> = p
        .triples
        .iter()
        .map(|t| (t.statement, at(&text, t.p_span), at(&text, t.o_span)))
        .collect();
    assert_eq!(
        shown,
        [
            (0, "Ontology", "Ontology"),
            (1, "Class", "Class"),
            (2, "SubClassOf", "<http://example.org/o#B>"),
            (3, "rdfs:label", "\"a\"^^xsd:string"),
            (4, ":p", ":y"),
            (5, "SubClassOf", ":A"),
            (6, "ClassAssertion", ":K"),
            (7, "SubClassOf", ":K"),
        ]
    );
    // The prefixes, as written.
    let prefixes: Vec<_> = p
        .prefixes
        .iter()
        .map(|d| {
            (
                d.name.as_str(),
                d.iri.as_str(),
                at(&text, d.span),
                at(&text, d.name_span),
            )
        })
        .collect();
    assert_eq!(
        prefixes,
        [
            (
                "",
                "http://example.org/o#",
                "Prefix(:=<http://example.org/o#>)",
                ":"
            ),
            (
                "ex",
                "http://example.org/",
                "Prefix(ex:=<http://example.org/>)",
                "ex:"
            ),
        ]
    );
    // References never overlap and are in text order (`ref_at` searches them).
    assert!(p.refs.windows(2).all(|w| w[0].span.end <= w[1].span.start));
    assert!(
        p.statements
            .windows(2)
            .all(|w| w[0].span.end <= w[1].span.start)
    );
    assert_eq!(p, parse(&text, Some("file:///w/o.ofn")));
}

/// An item with an error gives nothing: no triple, reference or
/// statement; the items around it are mapped, and so is one right before
/// an error the lexer meets after its `)`.
#[test]
fn an_item_with_an_error_gives_nothing() {
    let mapped = |text: &str| -> Vec<String> {
        let p = parse(text, None);
        assert!(!p.errors.is_empty(), "{text}");
        p.statements
            .iter()
            .map(|s| at(text, s.span).to_string())
            .collect()
    };
    let header = "Ontology(<http://example.org/o>".to_string();
    for (body, kept) in [
        (
            "SubClassOf(:A :B)\nSubClassOf(:A nope:B)\nSubClassOf(:B :C)",
            vec!["SubClassOf(:A :B)", "SubClassOf(:B :C)"],
        ),
        // Too few operands, deep inside.
        (
            "SubClassOf(:A ObjectUnionOf(:B))\nSubClassOf(:B :C)",
            vec!["SubClassOf(:B :C)"],
        ),
        // A relative IRI; a bad language tag; a missing `)`.
        (
            "SubClassOf(:A <b>)\nAnnotationAssertion(rdfs:label :A \"x\"@1)\nSubClassOf(:A :B\nSubClassOf(:B :C)",
            vec!["SubClassOf(:B :C)"],
        ),
        // Junk after a whole axiom, and a string that is not closed.
        (
            "SubClassOf(:A :B) junk\nSubClassOf(:B :C) \"open",
            vec!["SubClassOf(:A :B)", "SubClassOf(:B :C)"],
        ),
    ] {
        let text = in_ontology(body);
        let mut want = vec![header.clone()];
        want.extend(kept.iter().map(|s| s.to_string()));
        assert_eq!(mapped(&text), want, "{body}");
        let p = parse(&text, None);
        assert!(
            p.triples
                .iter()
                .all(|t| (t.statement as usize) < p.statements.len())
        );
        assert!(p.refs.iter().all(|r| {
            p.statements
                .iter()
                .any(|s| s.span.start <= r.span.start && r.span.end <= s.span.end)
        }));
    }
    // A prefix whose declaration is in error: its IRIs are not known.
    let text = "Prefix(bad:=<relative>)\nPrefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nSubClassOf(bad:A :B)\nSubClassOf(:A :B))";
    assert_eq!(mapped(text), [header.as_str(), "SubClassOf(:A :B)"]);
    // Items outside the ontology, or with no ontology, give nothing.
    let text = format!("{HEAD}Ontology(<http://example.org/o>)\nSubClassOf(:A :B)");
    assert_eq!(mapped(&text), [header.as_str()]);
    let p = parse(&format!("{HEAD}SubClassOf(:A :B)"), None);
    assert!(p.triples.is_empty() && p.statements.is_empty() && p.refs.is_empty());
    // An ontology IRI with an error: the header, its imports and
    // annotations are not mapped; the axioms are.
    let text =
        format!("{HEAD}Ontology(<o> Import(ex:i) Annotation(rdfs:label \"l\") SubClassOf(:A :B))");
    assert_eq!(mapped(&text), ["SubClassOf(:A :B)"]);
    // So when it gives no node (a prefix name with no local name), or the
    // version IRI has the error.
    for header in [
        "ex:",
        "<http://example.org/o> ex:",
        "<http://example.org/o> <v>",
        // An IRI not finished: the lexer ends it at the line end.
        "<http://example.org/o\n",
        "<http://example.org/o><http://example.org/v\n",
        "<http://example.org/o> <http://example.org/v\n",
    ] {
        let text = format!(
            "{HEAD}Ontology({header} Import(ex:i) Declaration(Class(:A)) SubClassOf(:A :B))"
        );
        let want = ["Declaration(Class(:A))", "SubClassOf(:A :B)"];
        assert_eq!(mapped(&text), want, "{header}");
    }
}

/// A `Prefix` declared inside the ontology (an error) binds from there on,
/// as the grammar reads it: the IRIs before it keep the binding before.
#[test]
fn a_prefix_binds_where_it_is_declared() {
    let text = format!(
        "{HEAD}Ontology(<http://example.org/o>\nClassAssertion(owl:Thing :a)\nPrefix(owl:=<http://example.org/owl#>)\nClassAssertion(owl:Thing :b)\n)"
    );
    let p = parse(&text, None);
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    let objects: Vec<_> = p.triples[1..].iter().map(|t| t.o.clone()).collect();
    assert_eq!(
        objects,
        [
            Node::Iri(Iri::intern("http://www.w3.org/2002/07/owl#Thing")),
            Node::Iri(Iri::intern("http://example.org/owl#Thing")),
        ]
    );
}

/// The limits: IRIs of many bytes for few bytes of text, and the triples
/// of an annotated n-ary axiom, which repeat its annotations per pair.
/// Past one, one error says so and the items from there on give nothing.
#[test]
fn the_mapping_stays_in_proportion_to_the_text() {
    let long = "x".repeat(60_000);
    let mut body = String::from("SubClassOf(:A :B)\n");
    for i in 0..40 {
        body.push_str(&format!("ClassAssertion(l:C{i} :a)\n"));
    }
    let text = format!(
        "Prefix(l:=<http://example.org/{long}#>)\n{}",
        in_ontology(&body)
    );
    let p = parse(&text, None);
    let limit: Vec<_> = p
        .errors
        .iter()
        .filter(|e| e.message().contains("bytes per byte"))
        .collect();
    assert_eq!(limit.len(), 1, "{:?}", p.errors);
    assert_eq!(limit[0].code, CODE);
    let mapped = p.statements.len();
    assert!(mapped > 2 && mapped < 40, "{mapped}");
    assert!(p.triples.iter().all(|t| (t.statement as usize) < mapped));
    let operands: String = (0..2000).map(|i| format!(" :i{i}")).collect();
    // IRI values: no literal bytes, so the triples meet their limit first.
    let annotations: String = (0..600).map(|i| format!("Annotation(:p :v{i})")).collect();
    let text = in_ontology(&format!(
        "SubClassOf(:A :B)\nSameIndividual({annotations}{operands})\nSubClassOf(:B :C)"
    ));
    let p = parse(&text, None);
    let limit: Vec<_> = p
        .errors
        .iter()
        .filter(|e| e.message().contains("triples per byte"))
        .collect();
    assert_eq!(limit.len(), 1, "{:?}", p.errors.len());
    let mapped: Vec<_> = p.statements.iter().map(|s| at(&text, s.span)).collect();
    assert_eq!(
        mapped,
        ["Ontology(<http://example.org/o>", "SubClassOf(:A :B)"]
    );
    assert_eq!(p.triples.len(), 2);
}

/// The literals of an annotated n-ary axiom, whose annotations section
/// 2.3.2 repeats per pair, count in bytes: the operands and the
/// annotation's length grown together (the text ×2, the literals ×4
/// unbounded) stay under the budget, past which one error says so.
#[test]
fn the_literal_bytes_stay_in_proportion_to_the_text() {
    let literal_bytes = |p: &Parsed| -> usize {
        p.triples
            .iter()
            .map(|t| match &t.o {
                Node::Literal { value, .. } => value.len(),
                _ => 0,
            })
            .sum()
    };
    // Under the budget, each pair has the annotation.
    let p = clean(&in_ontology(
        "SameIndividual(Annotation(rdfs:comment \"c\") :a :b :c)",
    ));
    assert_eq!(literal_bytes(&p), 2);
    for s in [1, 2] {
        let long = "x".repeat(400_000 * s);
        let operands: String = (0..100 * s).map(|i| format!(" :i{i}")).collect();
        let text = in_ontology(&format!(
            "SubClassOf(:A :B)\nSameIndividual(Annotation(rdfs:comment \"{long}\"){operands})\nSubClassOf(:B :C)"
        ));
        let p = parse(&text, None);
        let bytes = literal_bytes(&p);
        assert!(
            bytes <= text.len() * rdf::LITERAL_BYTES_PER_BYTE + rdf::MIN_LITERAL_BYTES,
            "s={s}: {bytes} bytes of literals for {} of text",
            text.len()
        );
        let limit: Vec<_> = p
            .errors
            .iter()
            .filter(|e| e.message().contains("literals are over"))
            .collect();
        assert_eq!(limit.len(), 1, "s={s}: {:?}", p.errors.len());
        assert_eq!(p.errors.len(), 1);
        let mapped: Vec<_> = p.statements.iter().map(|s| at(&text, s.span)).collect();
        assert_eq!(
            mapped,
            ["Ontology(<http://example.org/o>", "SubClassOf(:A :B)"],
            "s={s}"
        );
    }
}

/// An `Import` or an ontology annotation after the axioms is an item with
/// an error: it gives nothing.
#[test]
fn a_misplaced_import_or_annotation_gives_nothing() {
    let text = in_ontology(
        "Declaration(Class(:A))\nImport(<http://example.org/i>)\nAnnotation(rdfs:label \"l\")",
    );
    let p = parse(&text, None);
    assert_eq!(p.errors.len(), 2, "{:?}", p.errors);
    let mapped: Vec<_> = p.statements.iter().map(|s| at(&text, s.span)).collect();
    assert_eq!(
        mapped,
        ["Ontology(<http://example.org/o>", "Declaration(Class(:A))"]
    );
    assert_eq!(p.triples.len(), 2);
}
