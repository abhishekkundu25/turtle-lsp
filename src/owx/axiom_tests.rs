//! The reader's expressions, data ranges and axioms, through what the
//! mapping makes of its trees: each construct against the Functional form
//! of the same ontology, the spans of each group, and the checks of the
//! schema (arity, kinds, attributes, places, nesting). Every tree the
//! reader builds here fills its signatures ([`crate::ofn::signature::check`]).

use super::CODE;
use super::tests::{
    URL, at, clean, errors, functional, ofn, owx, read, refs, same_mapping, statements,
};
use crate::syntax::{Node, Parsed, Role};

/// The prefixes `xsd:` and `rdfs:`, which Functional syntax has built in.
const TABLE2: &str = "<Prefix name=\"xsd\" IRI=\"http://www.w3.org/2001/XMLSchema#\"/>\n<Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n";

fn doc(body: &str) -> String {
    owx(&format!("{TABLE2}{body}"))
}

fn class(n: &str) -> String {
    format!("<Class abbreviatedIRI=\":{n}\"/>")
}

fn op(n: &str) -> String {
    format!("<ObjectProperty abbreviatedIRI=\":{n}\"/>")
}

fn dp(n: &str) -> String {
    format!("<DataProperty abbreviatedIRI=\":{n}\"/>")
}

fn ind(n: &str) -> String {
    format!("<NamedIndividual abbreviatedIRI=\":{n}\"/>")
}

fn anon(n: &str) -> String {
    format!("<AnonymousIndividual nodeID=\"{n}\"/>")
}

fn xsd(n: &str) -> String {
    format!("<Datatype abbreviatedIRI=\"xsd:{n}\"/>")
}

fn lit(v: &str) -> String {
    format!("<Literal>{v}</Literal>")
}

fn int(v: &str) -> String {
    format!("<Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#integer\">{v}</Literal>")
}

fn el(name: &str, inner: &[String]) -> String {
    format!("<{name}>{}</{name}>", inner.concat())
}

fn card(name: &str, n: &str, inner: &[String]) -> String {
    format!("<{name} cardinality=\"{n}\">{}</{name}>", inner.concat())
}

fn sub(a: String, b: String) -> String {
    el("SubClassOf", &[a, b])
}

/// The OWL/XML `body` and the Functional `theirs` read alike, with no
/// error.
#[track_caller]
fn same(body: &str, theirs: &str) -> (String, Parsed) {
    let text = doc(body);
    let p = clean(&text);
    same_mapping(&p, &functional(&ofn(theirs)));
    (text, p)
}

/// The predicate's and the object's text of the triple of `p` whose
/// predicate ends in `name`, the `n`th of them.
#[track_caller]
fn spans<'t>(text: &'t str, p: &Parsed, name: &str, n: usize) -> (&'t str, &'t str) {
    let t = p
        .triples
        .iter()
        .filter(|t| t.p.as_str().ends_with(name))
        .nth(n)
        .unwrap_or_else(|| panic!("no triple {n} of {name}"));
    (at(text, t.p_span), at(text, t.o_span))
}

/// The errors of the OWL/XML `body`: code, text at, message.
#[track_caller]
fn faults(body: &str) -> (String, Parsed) {
    let text = doc(body);
    let p = read(&text, Some(URL));
    (text, p)
}

// ---- the Recommendation's example is in tests.rs ----

// ---- property expressions ----

/// `ObjectInverseOf` in a class expression, a property axiom and an
/// assertion, where the mapping swaps the subject and the object.
#[test]
fn object_inverse_of() {
    let inv = el("ObjectInverseOf", &[op("p")]);
    let body = [
        sub(
            class("A"),
            el("ObjectSomeValuesFrom", &[inv.clone(), class("B")]),
        ),
        el("SubObjectPropertyOf", &[inv.clone(), op("q")]),
        el(
            "ObjectPropertyAssertion",
            &[inv.clone(), ind("x"), ind("y")],
        ),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "SubClassOf(:A ObjectSomeValuesFrom(ObjectInverseOf(:p) :B))\nSubObjectPropertyOf(ObjectInverseOf(:p) :q)\nObjectPropertyAssertion(ObjectInverseOf(:p) :x :y)",
    );
    // `_:i owl:inverseOf :p`: at the element's name, the object at the
    // property's value.
    assert_eq!(spans(&text, &p, "#inverseOf", 0), ("ObjectInverseOf", ":p"));
    assert_eq!(
        spans(&text, &p, "#onProperty", 0),
        ("ObjectSomeValuesFrom", inv.as_str())
    );
    // `:y :p :x`, the predicate at the property's value, the object at the
    // subject's.
    let t = p.triples.last().unwrap();
    assert_eq!(t.p.as_str(), "http://example.org/o#p");
    assert_eq!((at(&text, t.p_span), at(&text, t.o_span)), (":p", ":x"));
    let r: Vec<_> = refs(&text, &p)[1..].iter().map(|r| (r.0, r.2)).collect();
    assert_eq!(
        r,
        [
            (":A", Role::Subject),
            (":p", Role::Object),
            (":B", Role::Object),
            (":p", Role::Object),
            (":q", Role::Object),
            (":p", Role::Predicate),
            (":x", Role::Object),
            (":y", Role::Subject),
        ]
    );
}

// ---- data ranges ----

#[test]
fn data_ranges() {
    let range = |r: String| el("DataPropertyRange", &[dp("d"), r]);
    let facet = |f: &str, v: &str| {
        format!(
            "<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#{f}\">{}</FacetRestriction>",
            int(v)
        )
    };
    let body = [
        range(el("DataIntersectionOf", &[xsd("integer"), xsd("decimal")])),
        range(el("DataUnionOf", &[xsd("integer"), xsd("string")])),
        range(el("DataComplementOf", &[xsd("integer")])),
        range(el("DataOneOf", &[lit("a"), int("1")])),
        range(el(
            "DatatypeRestriction",
            &[xsd("integer"), facet("minInclusive", "0")],
        )),
        range(el(
            "DatatypeRestriction",
            &[
                xsd("integer"),
                facet("minInclusive", "0"),
                facet("maxExclusive", "9"),
            ],
        )),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "DataPropertyRange(:d DataIntersectionOf(xsd:integer xsd:decimal))\nDataPropertyRange(:d DataUnionOf(xsd:integer xsd:string))\nDataPropertyRange(:d DataComplementOf(xsd:integer))\nDataPropertyRange(:d DataOneOf(\"a\" \"1\"^^xsd:integer))\nDataPropertyRange(:d DatatypeRestriction(xsd:integer xsd:minInclusive \"0\"^^xsd:integer))\nDataPropertyRange(:d DatatypeRestriction(xsd:integer xsd:minInclusive \"0\"^^xsd:integer xsd:maxExclusive \"9\"^^xsd:integer))",
    );
    assert_eq!(
        spans(&text, &p, "#intersectionOf", 0).0,
        "DataIntersectionOf"
    );
    assert_eq!(
        spans(&text, &p, "#datatypeComplementOf", 0),
        ("DataComplementOf", "xsd:integer")
    );
    assert_eq!(
        spans(&text, &p, "#onDatatype", 0),
        ("DatatypeRestriction", "xsd:integer")
    );
    // A facet: its predicate at the `facet` value, its object at the
    // literal.
    assert_eq!(
        spans(&text, &p, "#maxExclusive", 0),
        (
            "http://www.w3.org/2001/XMLSchema#maxExclusive",
            int("9").as_str()
        )
    );
    let facets: Vec<_> = refs(&text, &p)
        .into_iter()
        .filter(|r| r.1.ends_with("Inclusive") || r.1.ends_with("Exclusive"))
        .map(|r| (r.0, r.2))
        .collect();
    assert_eq!(
        facets,
        [
            (
                "http://www.w3.org/2001/XMLSchema#minInclusive",
                Role::Predicate
            ),
            (
                "http://www.w3.org/2001/XMLSchema#minInclusive",
                Role::Predicate
            ),
            (
                "http://www.w3.org/2001/XMLSchema#maxExclusive",
                Role::Predicate
            ),
        ]
    );
}

// ---- class expressions ----

#[test]
fn class_expressions() {
    let some = |a: String, b: String| sub(class("A"), el("ObjectSomeValuesFrom", &[a, b]));
    let body = [
        sub(
            class("A"),
            el("ObjectIntersectionOf", &[class("B"), class("C")]),
        ),
        sub(
            class("A"),
            el("ObjectUnionOf", &[class("B"), class("C"), class("D")]),
        ),
        sub(class("A"), el("ObjectComplementOf", &[class("B")])),
        sub(class("A"), el("ObjectOneOf", &[ind("x"), anon("b")])),
        some(op("p"), class("B")),
        sub(
            class("A"),
            el("ObjectAllValuesFrom", &[op("p"), class("B")]),
        ),
        sub(class("A"), el("ObjectHasValue", &[op("p"), ind("x")])),
        sub(class("A"), el("ObjectHasValue", &[op("p"), anon("b")])),
        sub(class("A"), el("ObjectHasSelf", &[op("p")])),
        sub(class("A"), card("ObjectMinCardinality", "1", &[op("p")])),
        sub(class("A"), card("ObjectMaxCardinality", "2", &[op("p")])),
        sub(class("A"), card("ObjectExactCardinality", "3", &[op("p")])),
        sub(
            class("A"),
            card("ObjectMinCardinality", "1", &[op("p"), class("B")]),
        ),
        sub(
            class("A"),
            card("ObjectMaxCardinality", "2", &[op("p"), class("B")]),
        ),
        sub(
            class("A"),
            card("ObjectExactCardinality", "3", &[op("p"), class("B")]),
        ),
        sub(
            class("A"),
            el("DataSomeValuesFrom", &[dp("d"), xsd("integer")]),
        ),
        sub(
            class("A"),
            el("DataSomeValuesFrom", &[dp("d"), dp("e"), xsd("integer")]),
        ),
        sub(
            class("A"),
            el("DataAllValuesFrom", &[dp("d"), xsd("integer")]),
        ),
        sub(
            class("A"),
            el("DataAllValuesFrom", &[dp("d"), dp("e"), xsd("integer")]),
        ),
        sub(class("A"), el("DataHasValue", &[dp("d"), lit("v")])),
        sub(class("A"), card("DataMinCardinality", "1", &[dp("d")])),
        sub(class("A"), card("DataMaxCardinality", "2", &[dp("d")])),
        sub(class("A"), card("DataExactCardinality", "3", &[dp("d")])),
        sub(
            class("A"),
            card("DataMinCardinality", "1", &[dp("d"), xsd("integer")]),
        ),
        sub(
            class("A"),
            card("DataMaxCardinality", "2", &[dp("d"), xsd("integer")]),
        ),
        sub(
            class("A"),
            card("DataExactCardinality", "3", &[dp("d"), xsd("integer")]),
        ),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "SubClassOf(:A ObjectIntersectionOf(:B :C))
SubClassOf(:A ObjectUnionOf(:B :C :D))
SubClassOf(:A ObjectComplementOf(:B))
SubClassOf(:A ObjectOneOf(:x _:b))
SubClassOf(:A ObjectSomeValuesFrom(:p :B))
SubClassOf(:A ObjectAllValuesFrom(:p :B))
SubClassOf(:A ObjectHasValue(:p :x))
SubClassOf(:A ObjectHasValue(:p _:b))
SubClassOf(:A ObjectHasSelf(:p))
SubClassOf(:A ObjectMinCardinality(1 :p))
SubClassOf(:A ObjectMaxCardinality(2 :p))
SubClassOf(:A ObjectExactCardinality(3 :p))
SubClassOf(:A ObjectMinCardinality(1 :p :B))
SubClassOf(:A ObjectMaxCardinality(2 :p :B))
SubClassOf(:A ObjectExactCardinality(3 :p :B))
SubClassOf(:A DataSomeValuesFrom(:d xsd:integer))
SubClassOf(:A DataSomeValuesFrom(:d :e xsd:integer))
SubClassOf(:A DataAllValuesFrom(:d xsd:integer))
SubClassOf(:A DataAllValuesFrom(:d :e xsd:integer))
SubClassOf(:A DataHasValue(:d \"v\"))
SubClassOf(:A DataMinCardinality(1 :d))
SubClassOf(:A DataMaxCardinality(2 :d))
SubClassOf(:A DataExactCardinality(3 :d))
SubClassOf(:A DataMinCardinality(1 :d xsd:integer))
SubClassOf(:A DataMaxCardinality(2 :d xsd:integer))
SubClassOf(:A DataExactCardinality(3 :d xsd:integer))",
    );
    // The type of a restriction is at its element's name; an operand at
    // its value, or at its element when it is an expression.
    assert_eq!(
        spans(&text, &p, "#someValuesFrom", 0),
        ("ObjectSomeValuesFrom", ":B")
    );
    assert_eq!(
        spans(&text, &p, "#complementOf", 0),
        ("ObjectComplementOf", ":B")
    );
    assert_eq!(spans(&text, &p, "#hasValue", 1), ("ObjectHasValue", "b"));
    assert_eq!(spans(&text, &p, "#hasSelf", 0).0, "ObjectHasSelf");
    // A cardinality: the object at the attribute's value.
    assert_eq!(
        spans(&text, &p, "#minCardinality", 0),
        ("ObjectMinCardinality", "1")
    );
    assert_eq!(
        spans(&text, &p, "#qualifiedCardinality", 0),
        ("ObjectExactCardinality", "3")
    );
    assert_eq!(
        spans(&text, &p, "#onClass", 0),
        ("ObjectMinCardinality", ":B")
    );
    assert_eq!(
        spans(&text, &p, "#onDataRange", 2),
        ("DataExactCardinality", "xsd:integer")
    );
    // Two data properties: a list, at the element's name.
    assert_eq!(spans(&text, &p, "#onProperties", 0).0, "DataSomeValuesFrom");
    assert_eq!(
        spans(&text, &p, "#hasValue", 2),
        ("DataHasValue", "<Literal>v</Literal>")
    );
}

/// A `cardinality` is any lexical form of `xsd:nonNegativeInteger`, taken
/// as written once its white space is collapsed.
#[test]
fn a_cardinality_is_taken_as_written() {
    let forms = [
        "0",
        "007",
        "+5",
        "-0",
        " 4 ",
        "&#52;2",
        "123456789012345678901234567890",
    ];
    let body: Vec<String> = forms
        .iter()
        .map(|n| sub(class("A"), card("ObjectMinCardinality", n, &[op("p")])))
        .collect();
    let text = doc(&body.join("\n"));
    let p = clean(&text);
    let got: Vec<_> = p
        .triples
        .iter()
        .filter(|t| t.p.as_str().ends_with("#minCardinality"))
        .map(|t| match &t.o {
            Node::Literal {
                value, datatype, ..
            } => (
                format!("\"{value}\"^^<{}>", datatype.as_str()),
                at(&text, t.o_span),
            ),
            o => panic!("{o:?}"),
        })
        .collect();
    let xsd = "http://www.w3.org/2001/XMLSchema#nonNegativeInteger";
    let want: Vec<_> = [
        ("0", "0"),
        ("007", "007"),
        ("+5", "+5"),
        ("-0", "-0"),
        ("4", "4"),
        ("42", "&#52;2"),
        (
            "123456789012345678901234567890",
            "123456789012345678901234567890",
        ),
    ]
    .iter()
    .map(|&(v, at)| (format!("\"{v}\"^^<{xsd}>"), at))
    .collect();
    assert_eq!(got, want);
}

// ---- class axioms ----

#[test]
fn class_axioms() {
    let body = [
        sub(
            el("ObjectIntersectionOf", &[class("A"), class("B")]),
            el("ObjectComplementOf", &[class("C")]),
        ),
        el("EquivalentClasses", &[class("A"), class("B")]),
        el(
            "EquivalentClasses",
            &[
                class("A"),
                el("ObjectComplementOf", &[class("B")]),
                class("C"),
            ],
        ),
        el("DisjointClasses", &[class("A"), class("B")]),
        el("DisjointClasses", &[class("A"), class("B"), class("C")]),
        el(
            "DisjointUnion",
            &[
                class("A"),
                class("B"),
                el("ObjectComplementOf", &[class("C")]),
            ],
        ),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "SubClassOf(ObjectIntersectionOf(:A :B) ObjectComplementOf(:C))\nEquivalentClasses(:A :B)\nEquivalentClasses(:A ObjectComplementOf(:B) :C)\nDisjointClasses(:A :B)\nDisjointClasses(:A :B :C)\nDisjointUnion(:A :B ObjectComplementOf(:C))",
    );
    assert_eq!(
        spans(&text, &p, "#subClassOf", 0),
        (
            "SubClassOf",
            "<ObjectComplementOf><Class abbreviatedIRI=\":C\"/></ObjectComplementOf>"
        )
    );
    assert_eq!(
        spans(&text, &p, "#equivalentClass", 0),
        ("EquivalentClasses", ":B")
    );
    assert_eq!(spans(&text, &p, "#disjointUnionOf", 0).0, "DisjointUnion");
    let s = statements(&text, &p);
    assert_eq!(
        s[3].0,
        "<EquivalentClasses><Class abbreviatedIRI=\":A\"/><ObjectComplementOf><Class abbreviatedIRI=\":B\"/></ObjectComplementOf><Class abbreviatedIRI=\":C\"/></EquivalentClasses>"
    );
}

// ---- object property axioms ----

#[test]
fn object_property_axioms() {
    let characteristics = [
        "FunctionalObjectProperty",
        "InverseFunctionalObjectProperty",
        "ReflexiveObjectProperty",
        "IrreflexiveObjectProperty",
        "SymmetricObjectProperty",
        "AsymmetricObjectProperty",
        "TransitiveObjectProperty",
    ];
    let mut body = vec![
        el("SubObjectPropertyOf", &[op("p"), op("q")]),
        el(
            "SubObjectPropertyOf",
            &[
                el(
                    "ObjectPropertyChain",
                    &[op("p"), el("ObjectInverseOf", &[op("q")]), op("r")],
                ),
                op("s"),
            ],
        ),
        el("EquivalentObjectProperties", &[op("p"), op("q")]),
        el("DisjointObjectProperties", &[op("p"), op("q")]),
        el("DisjointObjectProperties", &[op("p"), op("q"), op("r")]),
        el("InverseObjectProperties", &[op("p"), op("q")]),
        el("ObjectPropertyDomain", &[op("p"), class("A")]),
        el("ObjectPropertyRange", &[op("p"), class("B")]),
    ];
    let mut theirs = vec![
        "SubObjectPropertyOf(:p :q)".to_string(),
        "SubObjectPropertyOf(ObjectPropertyChain(:p ObjectInverseOf(:q) :r) :s)".into(),
        "EquivalentObjectProperties(:p :q)".into(),
        "DisjointObjectProperties(:p :q)".into(),
        "DisjointObjectProperties(:p :q :r)".into(),
        "InverseObjectProperties(:p :q)".into(),
        "ObjectPropertyDomain(:p :A)".into(),
        "ObjectPropertyRange(:p :B)".into(),
    ];
    for c in characteristics {
        body.push(el(c, &[op("p")]));
        theirs.push(format!("{c}(:p)"));
    }
    let (text, p) = same(&body.join("\n"), &theirs.join("\n"));
    assert_eq!(
        spans(&text, &p, "#propertyChainAxiom", 0),
        (
            "SubObjectPropertyOf",
            "<ObjectPropertyChain><ObjectProperty abbreviatedIRI=\":p\"/><ObjectInverseOf><ObjectProperty abbreviatedIRI=\":q\"/></ObjectInverseOf><ObjectProperty abbreviatedIRI=\":r\"/></ObjectPropertyChain>"
        )
    );
    assert_eq!(
        spans(&text, &p, "#inverseOf", 1),
        ("InverseObjectProperties", ":q")
    );
    assert_eq!(
        spans(&text, &p, "#domain", 0),
        ("ObjectPropertyDomain", ":A")
    );
    let kinds: Vec<_> = p
        .triples
        .iter()
        .filter(|t| t.p.as_str().ends_with("#type"))
        .map(|t| at(&text, t.o_span))
        .collect();
    assert_eq!(kinds[kinds.len() - 7..], characteristics);
}

// ---- data property axioms ----

#[test]
fn data_property_axioms() {
    let body = [
        el("SubDataPropertyOf", &[dp("d"), dp("e")]),
        el("EquivalentDataProperties", &[dp("d"), dp("e")]),
        el("DisjointDataProperties", &[dp("d"), dp("e")]),
        el("DisjointDataProperties", &[dp("d"), dp("e"), dp("f")]),
        el("DataPropertyDomain", &[dp("d"), class("A")]),
        el("DataPropertyRange", &[dp("d"), xsd("integer")]),
        el("FunctionalDataProperty", &[dp("d")]),
        el(
            "DatatypeDefinition",
            &[
                "<Datatype abbreviatedIRI=\":t\"/>".into(),
                el("DataUnionOf", &[xsd("integer"), xsd("string")]),
            ],
        ),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "SubDataPropertyOf(:d :e)\nEquivalentDataProperties(:d :e)\nDisjointDataProperties(:d :e)\nDisjointDataProperties(:d :e :f)\nDataPropertyDomain(:d :A)\nDataPropertyRange(:d xsd:integer)\nFunctionalDataProperty(:d)\nDatatypeDefinition(:t DataUnionOf(xsd:integer xsd:string))",
    );
    assert_eq!(
        spans(&text, &p, "#subPropertyOf", 0),
        ("SubDataPropertyOf", ":e")
    );
    assert_eq!(
        spans(&text, &p, "#range", 0),
        ("DataPropertyRange", "xsd:integer")
    );
    assert_eq!(
        spans(&text, &p, "#equivalentClass", 0).0,
        "DatatypeDefinition"
    );
}

// ---- keys ----

/// `HasKey` with object properties only, data properties only, and both:
/// a group of each, from its first property to its last, an empty one
/// zero-width where the element after it starts.
#[test]
fn has_key_and_its_two_groups() {
    let body = [
        el(
            "HasKey",
            &[class("A"), op("p"), el("ObjectInverseOf", &[op("q")])],
        ),
        el("HasKey", &[class("A"), dp("d"), dp("e")]),
        el("HasKey", &[class("A"), op("p"), dp("d")]),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "HasKey(:A (:p ObjectInverseOf(:q)) ())\nHasKey(:A () (:d :e))\nHasKey(:A (:p) (:d))",
    );
    assert_eq!(spans(&text, &p, "#hasKey", 0).0, "HasKey");
    let (tree, _) = super::reader::read(&text, Some(URL));
    let groups: Vec<_> = tree
        .nodes
        .iter()
        .filter(|n| n.kind == crate::ofn::tree::NodeKind::Group)
        .map(|n| {
            let s = at(&text, n.span);
            let rest = &text[n.span.end as usize..];
            (s, &rest[..rest.find('>').map_or(0, |e| e + 1)])
        })
        .collect();
    assert_eq!(
        groups,
        [
            (
                ":p\"/><ObjectInverseOf><ObjectProperty abbreviatedIRI=\":q\"/></ObjectInverseOf>",
                "</HasKey>"
            ),
            ("", "</HasKey>"),
            ("", "<DataProperty abbreviatedIRI=\":d\"/>"),
            (":d\"/><DataProperty abbreviatedIRI=\":e", "\"/>"),
            (":p", "\"/>"),
            (":d", "\"/>"),
        ]
    );
}

// ---- assertions ----

/// The assertions, with an anonymous individual in each place the schema
/// allows one.
#[test]
fn assertions_and_anonymous_individuals_in_each_place() {
    let body = [
        el("SameIndividual", &[ind("x"), anon("a")]),
        el("SameIndividual", &[anon("a"), ind("y"), anon("b")]),
        el("DifferentIndividuals", &[ind("x"), anon("a")]),
        el("DifferentIndividuals", &[ind("x"), ind("y"), anon("b")]),
        el(
            "ClassAssertion",
            &[el("ObjectComplementOf", &[class("A")]), anon("a")],
        ),
        el("ObjectPropertyAssertion", &[op("p"), anon("a"), anon("b")]),
        el("ObjectPropertyAssertion", &[op("p"), ind("x"), ind("y")]),
        el(
            "NegativeObjectPropertyAssertion",
            &[op("p"), anon("a"), anon("b")],
        ),
        el("DataPropertyAssertion", &[dp("d"), anon("a"), lit("v")]),
        el(
            "NegativeDataPropertyAssertion",
            &[dp("d"), anon("b"), lit("w")],
        ),
        sub(class("A"), el("ObjectOneOf", &[anon("a")])),
        sub(class("A"), el("ObjectHasValue", &[op("p"), anon("b")])),
    ]
    .join("\n");
    let (text, p) = same(
        &body,
        "SameIndividual(:x _:a)\nSameIndividual(_:a :y _:b)\nDifferentIndividuals(:x _:a)\nDifferentIndividuals(:x :y _:b)\nClassAssertion(ObjectComplementOf(:A) _:a)\nObjectPropertyAssertion(:p _:a _:b)\nObjectPropertyAssertion(:p :x :y)\nNegativeObjectPropertyAssertion(:p _:a _:b)\nDataPropertyAssertion(:d _:a \"v\")\nNegativeDataPropertyAssertion(:d _:b \"w\")\nSubClassOf(:A ObjectOneOf(_:a))\nSubClassOf(:A ObjectHasValue(:p _:b))",
    );
    assert_eq!(spans(&text, &p, "#sameAs", 0), ("SameIndividual", "a"));
    assert_eq!(spans(&text, &p, "#members", 0).0, "DifferentIndividuals");
    assert_eq!(
        spans(&text, &p, "#sourceIndividual", 0),
        ("NegativeObjectPropertyAssertion", "a")
    );
    assert_eq!(
        spans(&text, &p, "#targetValue", 0),
        ("NegativeDataPropertyAssertion", "<Literal>w</Literal>")
    );
    let t = p
        .triples
        .iter()
        .find(|t| t.p.as_str() == "http://example.org/o#p")
        .unwrap();
    assert_eq!((at(&text, t.p_span), at(&text, t.o_span)), (":p", "b"));
}

// ---- annotated axioms ----

/// Each axiom of the groups above with an annotation, and one with an
/// annotation on its annotation: the reification of the mapping's section
/// 2.3, isomorphic to the Functional form's.
#[test]
fn annotated_axioms() {
    let a = "<Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>c</Literal></Annotation>";
    let nested = "<Annotation><Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><Literal>on it</Literal></Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>c</Literal></Annotation>";
    let ax = |name: &str, inner: &[String]| {
        let mut all = vec![a.to_string()];
        all.extend_from_slice(inner);
        el(name, &all)
    };
    let characteristics = [
        "FunctionalObjectProperty",
        "InverseFunctionalObjectProperty",
        "ReflexiveObjectProperty",
        "IrreflexiveObjectProperty",
        "SymmetricObjectProperty",
        "AsymmetricObjectProperty",
    ];
    let mut body = vec![
        el("SubClassOf", &[nested.to_string(), class("A"), class("B")]),
        ax("EquivalentClasses", &[class("A"), class("B"), class("C")]),
        ax("DisjointClasses", &[class("A"), class("B")]),
        ax("DisjointUnion", &[class("A"), class("B"), class("C")]),
        ax(
            "SubObjectPropertyOf",
            &[el("ObjectPropertyChain", &[op("p"), op("q")]), op("r")],
        ),
        ax("EquivalentObjectProperties", &[op("p"), op("q")]),
        ax("DisjointObjectProperties", &[op("p"), op("q"), op("r")]),
        ax("InverseObjectProperties", &[op("p"), op("q")]),
        ax("ObjectPropertyDomain", &[op("p"), class("A")]),
        ax("ObjectPropertyRange", &[op("p"), class("A")]),
        ax("TransitiveObjectProperty", &[op("p")]),
        ax("SubDataPropertyOf", &[dp("d"), dp("e")]),
        ax("EquivalentDataProperties", &[dp("d"), dp("e")]),
        ax("DisjointDataProperties", &[dp("d"), dp("e"), dp("f")]),
        ax("DataPropertyDomain", &[dp("d"), class("A")]),
        ax("DataPropertyRange", &[dp("d"), xsd("integer")]),
        ax("FunctionalDataProperty", &[dp("d")]),
        ax(
            "DatatypeDefinition",
            &["<Datatype abbreviatedIRI=\":t\"/>".into(), xsd("integer")],
        ),
        ax("HasKey", &[class("A"), op("p"), dp("d")]),
        ax("SameIndividual", &[ind("x"), ind("y"), anon("b")]),
        ax("DifferentIndividuals", &[ind("x"), ind("y")]),
        ax("ClassAssertion", &[class("A"), anon("b")]),
        ax("ObjectPropertyAssertion", &[op("p"), ind("x"), anon("b")]),
        ax(
            "NegativeObjectPropertyAssertion",
            &[op("p"), ind("x"), ind("y")],
        ),
        ax("DataPropertyAssertion", &[dp("d"), ind("x"), lit("v")]),
        ax(
            "NegativeDataPropertyAssertion",
            &[dp("d"), ind("x"), lit("v")],
        ),
        ax("SubObjectPropertyOf", &[op("p"), op("q")]),
    ];
    let c = "Annotation(rdfs:comment \"c\")";
    let mut theirs = vec![format!("SubObjectPropertyOf({c} :p :q)")];
    for k in characteristics {
        body.push(ax(k, &[op("p")]));
        theirs.push(format!("{k}({c} :p)"));
    }
    let (text, p) = same(
        &body.join("\n"),
        &format!(
            "SubClassOf(Annotation(Annotation(rdfs:label \"on it\") rdfs:comment \"c\") :A :B)
EquivalentClasses({c} :A :B :C)
DisjointClasses({c} :A :B)
DisjointUnion({c} :A :B :C)
SubObjectPropertyOf({c} ObjectPropertyChain(:p :q) :r)
EquivalentObjectProperties({c} :p :q)
DisjointObjectProperties({c} :p :q :r)
InverseObjectProperties({c} :p :q)
ObjectPropertyDomain({c} :p :A)
ObjectPropertyRange({c} :p :A)
TransitiveObjectProperty({c} :p)
SubDataPropertyOf({c} :d :e)
EquivalentDataProperties({c} :d :e)
DisjointDataProperties({c} :d :e :f)
DataPropertyDomain({c} :d :A)
DataPropertyRange({c} :d xsd:integer)
FunctionalDataProperty({c} :d)
DatatypeDefinition({c} :t xsd:integer)
HasKey({c} :A (:p) (:d))
SameIndividual({c} :x :y _:b)
DifferentIndividuals({c} :x :y)
ClassAssertion({c} :A _:b)
ObjectPropertyAssertion({c} :p :x _:b)
NegativeObjectPropertyAssertion({c} :p :x :y)
DataPropertyAssertion({c} :d :x \"v\")
NegativeDataPropertyAssertion({c} :d :x \"v\")
{}",
            theirs.join("\n")
        ),
    );
    // The annotation on the annotation: its predicate at its property's
    // value, its object at its literal.
    assert_eq!(
        spans(&text, &p, "#label", 0),
        ("rdfs:label", "<Literal>on it</Literal>")
    );
}

// ---- checks: arity ----

#[test]
fn arity_errors_at_their_spans() {
    let one = |name: &str, x: String| el(name, &[x]);
    let range = |r: String| el("DataPropertyRange", &[dp("d"), r]);
    let mut body = vec![
        sub(class("A"), one("ObjectIntersectionOf", class("B"))),
        sub(class("A"), one("ObjectUnionOf", class("B"))),
        range(one("DataIntersectionOf", xsd("integer"))),
        range(one("DataUnionOf", xsd("integer"))),
    ];
    body.extend([
        one("EquivalentClasses", class("A")),
        one("DisjointClasses", class("A")),
        el("DisjointUnion", &[class("A"), class("B")]),
        one("EquivalentObjectProperties", op("p")),
        one("DisjointObjectProperties", op("p")),
        one("EquivalentDataProperties", dp("d")),
        one("DisjointDataProperties", dp("d")),
        one("SameIndividual", ind("x")),
        one("DifferentIndividuals", ind("x")),
        el(
            "SubObjectPropertyOf",
            &[one("ObjectPropertyChain", op("p")), op("q")],
        ),
        sub(class("A"), el("ObjectOneOf", &[])),
        el("DataPropertyRange", &[dp("d"), el("DataOneOf", &[])]),
        el(
            "DataPropertyRange",
            &[dp("d"), el("DatatypeRestriction", &[xsd("integer")])],
        ),
        el("InverseObjectProperties", &[op("p"), op("q"), op("r")]),
        one("SubClassOf", class("A")),
        el("SubClassOf", &[class("A"), class("B"), class("C")]),
        el("HasKey", &[class("A")]),
        el("HasKey", &[class("A"), dp("d"), op("p")]),
        sub(class("A"), el("DataSomeValuesFrom", &[xsd("integer")])),
        sub(class("A"), el("DataSomeValuesFrom", &[dp("d")])),
        el("Declaration", &[class("Z")]),
    ]);
    let (text, p) = faults(&body.join("\n"));
    let n = |name: &str, k: u8, what: &str| {
        (
            CODE,
            name.to_string(),
            format!(
                "<{name}> needs at least {k} {what}, found {}",
                if k == 2 { 1 } else { 0 }
            ),
        )
    };
    let got: Vec<_> = errors(&text, &p)
        .into_iter()
        .map(|(c, at, m)| (c, at.to_string(), m.to_string()))
        .collect();
    assert_eq!(
        got,
        [
            n("ObjectIntersectionOf", 2, "class expressions"),
            n("ObjectUnionOf", 2, "class expressions"),
            n("DataIntersectionOf", 2, "data ranges"),
            n("DataUnionOf", 2, "data ranges"),
            n("EquivalentClasses", 2, "class expressions"),
            n("DisjointClasses", 2, "class expressions"),
            n("DisjointUnion", 2, "class expressions"),
            n(
                "EquivalentObjectProperties",
                2,
                "object property expressions"
            ),
            n("DisjointObjectProperties", 2, "object property expressions"),
            n("EquivalentDataProperties", 2, "data properties"),
            n("DisjointDataProperties", 2, "data properties"),
            n("SameIndividual", 2, "individuals"),
            n("DifferentIndividuals", 2, "individuals"),
            n("ObjectPropertyChain", 2, "object property expressions"),
            n("ObjectOneOf", 1, "individual"),
            n("DataOneOf", 1, "literal"),
            (
                CODE,
                "</DatatypeRestriction>".into(),
                "Expected a facet, found </DatatypeRestriction>".into()
            ),
            (
                CODE,
                "ObjectProperty".into(),
                "Expected the end of <InverseObjectProperties>, found <ObjectProperty>".into()
            ),
            (
                CODE,
                "</SubClassOf>".into(),
                "Expected a class expression, found </SubClassOf>".into()
            ),
            (
                CODE,
                "Class".into(),
                "Expected the end of <SubClassOf>, found <Class>".into()
            ),
            (
                CODE,
                "HasKey".into(),
                "<HasKey> needs at least one object or data property expression".into()
            ),
            (
                CODE,
                "ObjectProperty".into(),
                "Expected a data property or </HasKey>, found <ObjectProperty>".into()
            ),
            (
                CODE,
                "DataSomeValuesFrom".into(),
                "<DataSomeValuesFrom> needs a data property and a data range".into()
            ),
            (
                CODE,
                "</DataSomeValuesFrom>".into(),
                "Expected a data range, found </DataSomeValuesFrom>".into()
            ),
        ]
    );
    // Only the last axiom is mapped.
    same_mapping(&p, &functional(&ofn("Declaration(Class(:Z))")));
}

// ---- checks: kinds ----

#[test]
fn an_operand_of_another_kind_is_an_error_at_its_name() {
    let body = [
        sub(
            class("A"),
            el("ObjectSomeValuesFrom", &[dp("d"), class("B")]),
        ),
        el("DataPropertyRange", &[dp("d"), class("B")]),
        el("ClassAssertion", &[class("A"), lit("x")]),
        el(
            "DisjointUnion",
            &[
                el("ObjectComplementOf", &[class("A")]),
                class("B"),
                class("C"),
            ],
        ),
        el(
            "SubObjectPropertyOf",
            &[
                el("ObjectInverseOf", &[el("ObjectInverseOf", &[op("p")])]),
                op("q"),
            ],
        ),
        el("EquivalentClasses", &[class("A"), class("B"), op("p")]),
        el("Declaration", &[class("Z")]),
    ]
    .join("\n");
    let (text, p) = faults(&body);
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "DataProperty",
                "Expected an object property expression, found <DataProperty>"
            ),
            (CODE, "Class", "Expected a data range, found <Class>"),
            (CODE, "Literal", "Expected an individual, found <Literal>"),
            (
                CODE,
                "ObjectComplementOf",
                "Expected a class, found <ObjectComplementOf>"
            ),
            (
                CODE,
                "ObjectInverseOf",
                "Expected an object property, found <ObjectInverseOf>"
            ),
            (
                CODE,
                "ObjectProperty",
                "Expected a class expression or </EquivalentClasses>, found <ObjectProperty>"
            ),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:Z))")));
}

// ---- checks: attributes ----

#[test]
fn cardinality_and_facet_attributes_in_error() {
    let min = |attrs: &str| {
        sub(
            class("A"),
            format!(
                "<ObjectMinCardinality{attrs}>{}</ObjectMinCardinality>",
                op("p")
            ),
        )
    };
    let restriction = |facet: &str| {
        el(
            "DataPropertyRange",
            &[
                dp("d"),
                el(
                    "DatatypeRestriction",
                    &[
                        xsd("integer"),
                        format!("<FacetRestriction{facet}>{}</FacetRestriction>", int("1")),
                    ],
                ),
            ],
        )
    };
    let body = [
        min(""),
        min(" cardinality=\"-1\""),
        min(" cardinality=\"one\""),
        min(" cardinality=\"\""),
        min(" cardinality=\"1.0\""),
        min(" cardinality=\"+\""),
        restriction(""),
        restriction(" facet=\"http://example.org/a b\""),
        sub(
            class("A"),
            format!(
                "<ObjectSomeValuesFrom cardinality=\"1\">{}{}</ObjectSomeValuesFrom>",
                op("p"),
                class("B")
            ),
        ),
        el(
            "DataPropertyRange",
            &[
                dp("d"),
                el(
                    "DatatypeRestriction",
                    &[
                        xsd("integer"),
                        format!("<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minLength\">{}</FacetRestriction>", class("B")),
                    ],
                ),
            ],
        ),
        el(
            "DataPropertyRange",
            &[
                dp("d"),
                el(
                    "DatatypeRestriction",
                    &[
                        xsd("integer"),
                        "<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minLength\"/>".into(),
                    ],
                ),
            ],
        ),
        el("Declaration", &[class("Z")]),
    ]
    .join("\n");
    let (text, p) = faults(&body);
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "ObjectMinCardinality",
                "<ObjectMinCardinality> needs the attribute cardinality"
            ),
            (CODE, "-1", "Expected a non-negative integer, found '-1'"),
            (CODE, "one", "Expected a non-negative integer, found 'one'"),
            (CODE, "", "Expected a non-negative integer, found ''"),
            (CODE, "1.0", "Expected a non-negative integer, found '1.0'"),
            (CODE, "+", "Expected a non-negative integer, found '+'"),
            (
                CODE,
                "FacetRestriction",
                "<FacetRestriction> needs the attribute facet"
            ),
            (
                CODE,
                "http://example.org/a b",
                "Invalid IRI: Invalid IRI code point ' '"
            ),
            (
                CODE,
                "cardinality",
                "<ObjectSomeValuesFrom> has no attribute cardinality"
            ),
            (CODE, "Class", "Expected a literal, found <Class>"),
            (
                CODE,
                "<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minLength\"/>",
                "Expected a literal, found </FacetRestriction>"
            ),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:Z))")));
}

// ---- checks: places ----

/// `FacetRestriction` is read only in a `DatatypeRestriction`, and
/// `ObjectPropertyChain` only first in a `SubObjectPropertyOf`.
#[test]
fn a_facet_or_a_chain_outside_its_place_is_an_error() {
    let facet = format!(
        "<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minLength\">{}</FacetRestriction>",
        int("1")
    );
    let chain = el("ObjectPropertyChain", &[op("p"), op("q")]);
    let body = [
        facet.clone(),
        sub(class("A"), facet.clone()),
        el("DataPropertyRange", &[dp("d"), facet]),
        chain.clone(),
        el("SubObjectPropertyOf", &[op("r"), chain.clone()]),
        el("EquivalentObjectProperties", &[chain, op("r")]),
        el("Declaration", &[class("Z")]),
    ]
    .join("\n");
    let (text, p) = faults(&body);
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "FacetRestriction",
                "Expected <Prefix>, <Import>, <Annotation> or an axiom, found <FacetRestriction>"
            ),
            (
                CODE,
                "FacetRestriction",
                "Expected a class expression, found <FacetRestriction>"
            ),
            (
                CODE,
                "FacetRestriction",
                "Expected a data range, found <FacetRestriction>"
            ),
            (
                CODE,
                "ObjectPropertyChain",
                "Expected <Prefix>, <Import>, <Annotation> or an axiom, found <ObjectPropertyChain>"
            ),
            (
                CODE,
                "ObjectPropertyChain",
                "Expected an object property expression, found <ObjectPropertyChain>"
            ),
            (
                CODE,
                "EquivalentObjectProperties",
                "<EquivalentObjectProperties> needs at least 2 object property expressions, found 0"
            ),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:Z))")));
}

// ---- checks: nesting ----

/// An axiom nested to the scanner's limit reads; one past it gives only
/// the scanner's error and nothing, and the parse goes on.
#[test]
fn nesting_at_the_scanner_s_depth_limit() {
    use crate::rdfxml::xml::MAX_DEPTH;
    // `Ontology` and `SubClassOf` are two levels, the class one more.
    let nested = |n: usize| {
        let open = "<ObjectComplementOf>".repeat(n);
        let close = "</ObjectComplementOf>".repeat(n);
        sub(class("A"), format!("{open}{}{close}", class("B")))
    };
    let fits = MAX_DEPTH - 3;
    let text = doc(&format!(
        "{}\n{}",
        nested(fits),
        el("Declaration", &[class("Z")])
    ));
    let p = clean(&text);
    let mut theirs = "ObjectComplementOf(".repeat(fits);
    theirs.push_str(":B");
    theirs.push_str(&")".repeat(fits));
    same_mapping(
        &p,
        &functional(&ofn(&format!(
            "SubClassOf(:A {theirs})\nDeclaration(Class(:Z))"
        ))),
    );
    let text = doc(&format!(
        "{}\n{}",
        nested(fits + 1),
        el("Declaration", &[class("Z")])
    ));
    let p = read(&text, Some(URL));
    let got: Vec<_> = p.errors.iter().map(|e| (e.code, e.message())).collect();
    assert_eq!(
        got,
        [(
            "xml-syntax",
            "elements nest deeper than 256: this one is not read"
        )]
    );
    assert_eq!(at(&text, p.errors[0].span), "Class");
    same_mapping(&p, &functional(&ofn("Declaration(Class(:Z))")));
    // Far past it, nothing overflows.
    let text = doc(&format!(
        "{}\n{}",
        nested(100_000),
        el("Declaration", &[class("Z")])
    ));
    let p = read(&text, Some(URL));
    assert_eq!(p.errors.len(), 1, "{:?}", p.errors.first());
    same_mapping(&p, &functional(&ofn("Declaration(Class(:Z))")));
}
