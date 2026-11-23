//! The reader's terms and annotations, through what the mapping makes of
//! its trees: literals (the spec's table of `datatypeIRI` and `xml:lang`,
//! their content, language and datatype), anonymous individuals, the `IRI`
//! and `AbbreviatedIRI` elements, `Annotation` on the ontology, on an
//! axiom and in another annotation, and the four annotation axioms. Each
//! test asserts the graph against the Functional form of the same input,
//! and the spans where they matter; every tree passes the signature check
//! (the helpers of [`super::tests`]).

use super::CODE;
use super::tests::{
    ANCHOR, IRI, URL, at, clean, errors, functional, ofn, owx, pn, read, refs, same_mapping,
    statements, triples,
};
use crate::syntax::{Parsed, Role::*, Span};

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The prefixes of Table 2, which an OWL/XML document declares to use.
const TABLE2: &str = "<Prefix name=\"rdf\" IRI=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"/>\n<Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n<Prefix name=\"xsd\" IRI=\"http://www.w3.org/2001/XMLSchema#\"/>\n<Prefix name=\"owl\" IRI=\"http://www.w3.org/2002/07/owl#\"/>\n";

/// An OWL/XML document of `body`, with the prefixes of [`TABLE2`] too.
fn doc(body: &str) -> String {
    owx(&format!("{TABLE2}{body}"))
}

/// `rdfs:label` of `:A` is `value`.
fn label(value: &str) -> String {
    format!(
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:A</AbbreviatedIRI>{value}</AnnotationAssertion>"
    )
}

/// The same in Functional syntax.
fn ofn_label(value: &str) -> String {
    format!("AnnotationAssertion(rdfs:label :A {value})")
}

/// The OWL/XML `values` and the Functional `theirs`, each a label of `:A`,
/// read alike, with no error.
#[track_caller]
fn labels_read_as(values: &[&str], theirs: &[&str]) -> (String, Parsed) {
    let body: Vec<String> = values.iter().map(|v| label(v)).collect();
    let text = doc(&body.join("\n"));
    let p = clean(&text);
    let body: Vec<String> = theirs.iter().map(|v| ofn_label(v)).collect();
    same_mapping(&p, &functional(&ofn(&body.join("\n"))));
    (text, p)
}

/// The text at the object of each triple of `p` past the header.
fn objects<'t>(text: &'t str, p: &Parsed) -> Vec<&'t str> {
    p.triples[1..].iter().map(|t| at(text, t.o_span)).collect()
}

// ---- literals: the spec's table, one test per row ----

/// No `datatypeIRI` and no language: `"text"`, an `xsd:string`. The
/// predicate is at the annotation property's value, the object at the
/// literal's element, each reference at its value.
#[test]
fn a_literal_without_a_datatype_or_a_language_is_a_string() {
    let (text, p) = labels_read_as(&["<Literal>text</Literal>"], &["\"text\""]);
    assert_eq!(p.triples.len(), 2);
    let t = &p.triples[1];
    assert_eq!(at(&text, t.p_span), "rdfs:label");
    assert_eq!(at(&text, t.o_span), "<Literal>text</Literal>");
    assert_eq!(
        refs(&text, &p)[1..],
        [
            (
                "rdfs:label",
                "http://www.w3.org/2000/01/rdf-schema#label",
                Predicate,
                pn(4)
            ),
            (":A", "http://example.org/o#A", Subject, pn(0)),
        ]
    );
    assert_eq!(
        statements(&text, &p)[1],
        (&*label("<Literal>text</Literal>"), Some(":A"))
    );
}

/// No `datatypeIRI`, a language in scope: `"text"@tag`.
#[test]
fn a_literal_with_a_language_and_no_datatype_has_the_language() {
    labels_read_as(
        &["<Literal xml:lang=\"en\">text</Literal>"],
        &["\"text\"@en"],
    );
}

/// `rdf:PlainLiteral` with a language in scope: `"text"@tag`, the content
/// taken whole, an `@` in it too: in Functional syntax, the content and
/// `@tag` in the lexical form.
#[test]
fn a_plain_literal_with_a_language_is_its_content_whole_with_the_language() {
    let plain = format!("{RDF}PlainLiteral");
    let (text, p) = labels_read_as(
        &[
            &format!("<Literal xml:lang=\"en\" datatypeIRI=\"{plain}\">Bonn</Literal>"),
            &format!("<Literal xml:lang=\"en\" datatypeIRI=\"{plain}\">Bonn@de</Literal>"),
        ],
        &[
            "\"Bonn@en\"^^rdf:PlainLiteral",
            "\"Bonn@de@en\"^^rdf:PlainLiteral",
        ],
    );
    // The datatype is a reference at its value, with the role `Datatype`.
    let datatypes: Vec<_> = refs(&text, &p)
        .into_iter()
        .filter(|r| r.2 == Datatype)
        .collect();
    assert_eq!(
        datatypes,
        [(plain.as_str(), plain.as_str(), Datatype, IRI); 2]
    );
}

/// `rdf:PlainLiteral` with no language: as in Functional syntax, `text@tag`
/// or `text@`, split at the last `@`.
#[test]
fn a_plain_literal_without_a_language_is_read_as_in_functional_syntax() {
    let plain = format!("{RDF}PlainLiteral");
    labels_read_as(
        &[
            &format!("<Literal datatypeIRI=\"{plain}\">Bonn@DE</Literal>"),
            &format!("<Literal datatypeIRI=\"{plain}\">a@b@</Literal>"),
        ],
        &[
            "\"Bonn@DE\"^^rdf:PlainLiteral",
            "\"a@b@\"^^rdf:PlainLiteral",
        ],
    );
}

/// Any other datatype: `"text"^^datatype`, the language in scope not used.
#[test]
fn a_literal_with_another_datatype_does_not_use_the_language() {
    labels_read_as(
        &[
            &format!("<Literal xml:lang=\"en\" datatypeIRI=\"{XSD}integer\">1</Literal>"),
            &format!("<Literal datatypeIRI=\"{RDF}langString\">x</Literal>"),
        ],
        &["\"1\"^^xsd:integer", "\"x\"^^rdf:langString"],
    );
}

/// The `rdf:PlainLiteral` without an `@` and without a language: the
/// mapper's error at the literal's content, its item gives nothing, and
/// the items around it map.
#[test]
fn a_plain_literal_without_an_at_or_a_language_is_an_error_at_its_content() {
    let plain = format!("{RDF}PlainLiteral");
    let text = doc(&[
        label("<Literal>before</Literal>"),
        label(&format!(
            "<Literal datatypeIRI=\"{plain}\"> Bonn </Literal>"
        )),
        label(&format!("<Literal datatypeIRI=\"{plain}\"/>")),
        label("<Literal>after</Literal>"),
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    let message = "An rdf:PlainLiteral is \"text@tag\" or \"text@\": no '@' here";
    assert_eq!(
        errors(&text, &p),
        [
            (CODE, " Bonn ", message),
            (
                CODE,
                &*format!("<Literal datatypeIRI=\"{plain}\"/>"),
                message
            ),
        ]
    );
    same_mapping(
        &p,
        &functional(&ofn(
            &[ofn_label("\"before\""), ofn_label("\"after\"")].join("\n")
        )),
    );
}

// ---- a literal's content ----

/// References and CDATA sections are read, white space is kept as
/// written, and an empty `Literal` is the empty string.
#[test]
fn a_literal_s_content_is_its_text_its_references_read() {
    let (text, p) = labels_read_as(
        &[
            "<Literal>a &lt; b &amp; &#x41;</Literal>",
            "<Literal><![CDATA[<c> &amp;]]></Literal>",
            "<Literal>t &amp; <![CDATA[<c>]]>!</Literal>",
            "<Literal>  two\n  lines\t</Literal>",
            "<Literal>a<!-- c -->b<?pi x?>c</Literal>",
            "<Literal></Literal>",
            "<Literal/>",
        ],
        &[
            "\"a < b & A\"",
            "\"<c> &amp;\"",
            "\"t & <c>!\"",
            "\"  two\n  lines\t\"",
            "\"abc\"",
            "\"\"",
            "\"\"",
        ],
    );
    assert_eq!(
        objects(&text, &p),
        [
            "<Literal>a &lt; b &amp; &#x41;</Literal>",
            "<Literal><![CDATA[<c> &amp;]]></Literal>",
            "<Literal>t &amp; <![CDATA[<c>]]>!</Literal>",
            "<Literal>  two\n  lines\t</Literal>",
            "<Literal>a<!-- c -->b<?pi x?>c</Literal>",
            "<Literal></Literal>",
            "<Literal/>",
        ]
    );
}

/// A literal through an entity the DOCTYPE declares.
#[test]
fn a_literal_s_content_through_an_entity() {
    let text = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE Ontology [<!ENTITY name \"Bonn &amp; Köln\">]>\n{}",
        &doc(&label("<Literal>&name;!</Literal>"))["<?xml version=\"1.0\"?>\n".len()..]
    );
    let p = clean(&text);
    same_mapping(&p, &functional(&ofn(&ofn_label("\"Bonn & Köln!\""))));
}

/// An element inside a `Literal` is an error at its name, and breaks the
/// item.
#[test]
fn an_element_inside_a_literal_is_an_error() {
    let text = doc(&[
        label("<Literal>a<b/>c</Literal>"),
        label("<Literal>ok</Literal>"),
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [(CODE, "b", "<Literal> holds text and no element, found <b>")]
    );
    same_mapping(&p, &functional(&ofn(&ofn_label("\"ok\""))));
}

// ---- xml:lang ----

/// The language is the nearest `xml:lang`: on the literal, on the axiom,
/// on `Ontology`; `xml:lang=""` switches it off; the tag is lowercased as
/// the mapper does for Functional literals.
#[test]
fn the_language_is_the_nearest_xml_lang_in_scope() {
    let text = format!(
        "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:lang=\"de\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n{TABLE2}{}\n</Ontology>\n",
        [
            label("<Literal>o</Literal>"),
            "<AnnotationAssertion xml:lang=\"fr\"><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:A</AbbreviatedIRI><Literal>a</Literal></AnnotationAssertion>".to_string(),
            label("<Literal xml:lang=\"EN-gb\">l</Literal>"),
            "<AnnotationAssertion xml:lang=\"\"><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:A</AbbreviatedIRI><Literal>none</Literal></AnnotationAssertion>".to_string(),
            "<AnnotationAssertion xml:lang=\"\"><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:A</AbbreviatedIRI><Literal xml:lang=\"it\">again</Literal></AnnotationAssertion>".to_string(),
            label("<Literal>after</Literal>"),
        ]
        .join("\n")
    );
    let p = clean(&text);
    let theirs = functional(&format!(
        "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\n{}\n)",
        [
            ofn_label("\"o\"@de"),
            ofn_label("\"a\"@fr"),
            ofn_label("\"l\"@EN-gb"),
            ofn_label("\"none\""),
            ofn_label("\"again\"@it"),
            ofn_label("\"after\"@de"),
        ]
        .join("\n")
    ));
    same_mapping(&p, &theirs);
    let crate::syntax::Node::Literal { lang, .. } = &p.triples[3].o else {
        panic!("{:?}", p.triples[3])
    };
    assert_eq!(lang.map(|l| l.as_str()), Some("en-gb"));
}

/// An `xml:lang` that is no language tag is an error at its value, in the
/// Functional words; its item gives nothing. On `Ontology`, it breaks the
/// header (as an `xml:base` in error does) and every literal under it.
#[test]
fn an_xml_lang_that_is_no_language_tag_is_an_error_at_its_value() {
    let text = doc(&[
        label("<Literal xml:lang=\"en_GB\">x</Literal>"),
        "<AnnotationAssertion xml:lang=\"1-a\"><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:A</AbbreviatedIRI><IRI>http://example.org/v</IRI></AnnotationAssertion>".to_string(),
        label("<Literal xml:lang=\"en\">ok</Literal>"),
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(e.len(), 2, "{e:?}");
    assert_eq!((e[0].0, e[0].1), (CODE, "en_GB"));
    assert!(
        e[0].2.starts_with("Invalid language tag 'en_GB': "),
        "{e:?}"
    );
    assert_eq!((e[1].0, e[1].1), (CODE, "1-a"));
    assert!(e[1].2.starts_with("Invalid language tag '1-a': "), "{e:?}");
    same_mapping(&p, &functional(&ofn(&ofn_label("\"ok\"@en"))));
    // On `Ontology`.
    let text = format!(
        "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:lang=\"x y\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n{TABLE2}{}\n</Ontology>\n",
        [
            label("<Literal>under</Literal>"),
            label("<Literal xml:lang=\"en\">own</Literal>"),
            "<Declaration><Class abbreviatedIRI=\":A\"/></Declaration>".to_string(),
        ]
        .join("\n")
    );
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!((e[0].0, e[0].1), (CODE, "x y"));
    let theirs = functional(
        "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nAnnotationAssertion(rdfs:label :A \"own\"@en)\nDeclaration(Class(:A))\n)",
    );
    assert_eq!(triples(&p), triples(&theirs)[1..]);
}

// ---- datatypeIRI ----

/// `datatypeIRI` is an `xsd:anyURI`: absolute, relative (resolved against
/// the base in scope), or through an entity; its reference has the role
/// `Datatype` at the value.
#[test]
fn a_datatype_iri_is_resolved_and_is_a_reference_at_its_value() {
    let text = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE Ontology [<!ENTITY xsd \"http://www.w3.org/2001/XMLSchema#\">]>\n{}",
        &doc(&[
            label(&format!("<Literal datatypeIRI=\"{XSD}integer\">1</Literal>")),
            label("<Literal xml:base=\"http://www.w3.org/2001/XMLSchema\" datatypeIRI=\"#decimal\">2</Literal>"),
            label("<Literal datatypeIRI=\"&xsd;boolean\">true</Literal>"),
        ]
        .join("\n"))["<?xml version=\"1.0\"?>\n".len()..]
    );
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&ofn(&[
            ofn_label("\"1\"^^xsd:integer"),
            ofn_label("\"2\"^^xsd:decimal"),
            ofn_label("\"true\"^^xsd:boolean"),
        ]
        .join("\n"))),
    );
    let datatypes: Vec<_> = refs(&text, &p)
        .into_iter()
        .filter(|r| r.2 == Datatype)
        .collect();
    let (integer, decimal, boolean) = (
        format!("{XSD}integer"),
        format!("{XSD}decimal"),
        format!("{XSD}boolean"),
    );
    assert_eq!(
        datatypes,
        [
            (integer.as_str(), integer.as_str(), Datatype, IRI),
            ("#decimal", decimal.as_str(), Datatype, IRI),
            ("&xsd;boolean", boolean.as_str(), Datatype, ANCHOR),
        ]
    );
}

/// A `datatypeIRI` that is no IRI is an error at its value.
#[test]
fn a_datatype_iri_that_is_no_iri_is_an_error() {
    let text = doc(&[
        label("<Literal datatypeIRI=\"http://example.org/a b\">1</Literal>"),
        label("<Literal datatypeIRI=\"\">1</Literal>"),
        label("<Literal>ok</Literal>"),
    ]
    .join("\n"));
    let p = read(&text, None);
    let e = errors(&text, &p);
    assert_eq!(e.len(), 2, "{e:?}");
    assert_eq!((e[0].0, e[0].1), (CODE, "http://example.org/a b"));
    assert!(e[0].2.starts_with("Invalid IRI: "), "{e:?}");
    assert_eq!(
        e[1],
        (
            CODE,
            "",
            "<> is a relative IRI, and no base is in scope to resolve it against"
        )
    );
    same_mapping(&p, &functional(&ofn(&ofn_label("\"ok\""))));
}

// ---- anonymous individuals ----

/// One blank node per `nodeID` in the document: the same node in two
/// axioms, a different one per name.
#[test]
fn an_anonymous_individual_is_one_blank_node_per_node_id() {
    let text = doc(&[
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"x\"/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":B\"/><AnonymousIndividual nodeID=\"y\"/></ClassAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><AnonymousIndividual nodeID=\"&#x78;\"/><AnonymousIndividual nodeID=\"y\"/></AnnotationAssertion>",
    ]
    .join("\n"));
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&ofn(
            "ClassAssertion(:A _:x)\nClassAssertion(:B _:y)\nAnnotationAssertion(rdfs:comment _:x _:y)",
        )),
    );
    // Each a node at its `nodeID` value.
    assert_eq!(
        p.triples[1..]
            .iter()
            .map(|t| at(&text, t.o_span))
            .collect::<Vec<_>>(),
        [":A", ":B", "y"]
    );
    let blanks: Vec<_> = triples(&p)[1..].iter().map(|(s, _, _)| s.clone()).collect();
    assert_eq!(blanks[0], blanks[2]);
    assert_ne!(blanks[0], blanks[1]);
}

/// A missing `nodeID`, and one that is no `NCName` and no `_:` and an
/// `NCName` (as OWL API writes it, [`super::rule_tests`]), are errors.
#[test]
fn an_anonymous_individual_needs_an_nc_name_node_id() {
    let text = doc(&[
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"a:b\"/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"1x\"/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"_:1x\"/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"_:\"/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"_:ok\"/></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"x\"><Class abbreviatedIRI=\":A\"/></AnonymousIndividual></ClassAssertion>",
        "<ClassAssertion><Class abbreviatedIRI=\":A\"/><AnonymousIndividual nodeID=\"ok\"/></ClassAssertion>",
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "AnonymousIndividual",
                "<AnonymousIndividual> needs the attribute nodeID"
            ),
            (
                CODE,
                "a:b",
                "'a:b' is no nodeID: a nodeID is an NCName (a name without ':'), or '_:' and an NCName"
            ),
            (
                CODE,
                "1x",
                "'1x' is no nodeID: a nodeID is an NCName (a name without ':'), or '_:' and an NCName"
            ),
            (
                CODE,
                "_:1x",
                "'_:1x' is no nodeID: a nodeID is an NCName (a name without ':'), or '_:' and an NCName"
            ),
            (
                CODE,
                "_:",
                "'_:' is no nodeID: a nodeID is an NCName (a name without ':'), or '_:' and an NCName"
            ),
            (
                CODE,
                "Class",
                "<AnonymousIndividual> holds no element, found <Class>"
            ),
        ]
    );
    same_mapping(
        &p,
        &functional(&ofn("ClassAssertion(:A _:ok)\nClassAssertion(:A _:ok2)")),
    );
}

// ---- IRI and AbbreviatedIRI elements ----

/// An IRI element is its trimmed content: the reference is there; a
/// relative one resolves against the base in scope.
#[test]
fn an_iri_element_is_its_trimmed_content() {
    let text = doc(&[
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>\n  http://example.org/s\n</IRI><AbbreviatedIRI>  ex:v </AbbreviatedIRI></AnnotationAssertion>",
        "<AnnotationAssertion xml:base=\"http://example.org/b/\"><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>s</IRI><IRI xml:base=\"http://example.org/c/\">../v</IRI></AnnotationAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>#s</IRI><IRI>http://example.org/<!-- c -->v</IRI></AnnotationAssertion>",
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    same_mapping(
        &p,
        &functional(&ofn(
            "AnnotationAssertion(rdfs:seeAlso <http://example.org/s> ex:v)\nAnnotationAssertion(rdfs:seeAlso <http://example.org/b/s> <http://example.org/v>)\nAnnotationAssertion(rdfs:seeAlso <file:///w/o.owx#s> <http://example.org/v>)",
        )),
    );
    let see_also = format!("{RDFS}seeAlso");
    assert_eq!(
        refs(&text, &p)[1..],
        [
            ("rdfs:seeAlso", see_also.as_str(), Predicate, pn(4)),
            ("http://example.org/s", "http://example.org/s", Subject, IRI),
            ("ex:v", "http://example.org/v", Object, pn(2)),
            ("rdfs:seeAlso", see_also.as_str(), Predicate, pn(4)),
            ("s", "http://example.org/b/s", Subject, IRI),
            ("../v", "http://example.org/v", Object, IRI),
            ("rdfs:seeAlso", see_also.as_str(), Predicate, pn(4)),
            ("#s", "file:///w/o.owx#s", Subject, IRI),
            (
                "http://example.org/<!-- c -->v",
                "http://example.org/v",
                Object,
                ANCHOR
            ),
        ]
    );
    let t = &p.triples[1];
    assert_eq!(at(&text, t.o_span), "ex:v");
}

/// An undeclared prefix, an empty `AbbreviatedIRI` and an element inside
/// are errors; an empty `IRI` is the base (here the document's URL).
#[test]
fn iri_elements_in_error() {
    let text = doc(&[
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><AbbreviatedIRI>nope:s</AbbreviatedIRI><IRI>http://example.org/v</IRI></AnnotationAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>  </IRI><IRI>http://example.org/v</IRI></AnnotationAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><AbbreviatedIRI/><IRI>http://example.org/v</IRI></AnnotationAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>http://example.org/<Class/></IRI><IRI>http://example.org/v</IRI></AnnotationAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><AbbreviatedIRI>noColon</AbbreviatedIRI><IRI>http://example.org/v</IRI></AnnotationAssertion>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>:A</IRI><IRI>http://example.org/v</IRI></AnnotationAssertion>",
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(
        e[..4],
        [
            (
                CODE,
                "nope:s",
                "Undeclared prefix 'nope:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"nope\" IRI=\"…\"/>"
            ),
            (
                CODE,
                "<AbbreviatedIRI/>",
                "Expected an abbreviated IRI, found </AbbreviatedIRI>"
            ),
            (
                CODE,
                "Class",
                "<IRI> holds text and no element, found <Class>"
            ),
            (
                CODE,
                "noColon",
                "'noColon' is no abbreviated IRI: it has no ':'"
            ),
        ]
    );
    assert_eq!((e[4].0, e[4].1), (CODE, ":A"), "{e:?}");
    assert!(e[4].2.starts_with("Invalid IRI: "), "{e:?}");
    assert_eq!(e.len(), 5);
    assert_eq!(p.triples.len(), 2);
    assert!(
        p.refs
            .iter()
            .any(|r| at(&text, r.span) == "<IRI>  </IRI>" && r.iri.as_str() == URL)
    );
}

// ---- annotations ----

/// `Annotation` on the ontology (after the imports), on an axiom, two on
/// one axiom, and nested in an annotation: the reification of the
/// mapping's sections 2.2 and 2.3.
#[test]
fn annotations_on_the_ontology_on_axioms_and_nested() {
    let ann = |p: &str, v: &str| {
        format!("<Annotation><AnnotationProperty abbreviatedIRI=\"{p}\"/>{v}</Annotation>")
    };
    let nested = format!(
        "<Annotation>{}<AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/><IRI>http://example.org/doc</IRI></Annotation>",
        ann("rdfs:label", "<Literal xml:lang=\"en\">nested</Literal>")
    );
    let text = doc(&format!(
        "<Import>http://example.org/other</Import>\n{}\n{nested}\n<SubClassOf>{}<Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>\n<SubClassOf>{}{}<Class abbreviatedIRI=\":B\"/><Class abbreviatedIRI=\":C\"/></SubClassOf>\n<Declaration>{}<Class abbreviatedIRI=\":D\"/></Declaration>",
        ann("rdfs:comment", "<Literal>an ontology</Literal>"),
        ann("rdfs:comment", "<Literal>one</Literal>"),
        ann("rdfs:comment", "<Literal>two</Literal>"),
        ann("ex:n", "<AnonymousIndividual nodeID=\"n\"/>"),
        ann("rdfs:label", "<AbbreviatedIRI>:L</AbbreviatedIRI>"),
    ));
    let p = clean(&text);
    let theirs = functional(&ofn(
        "Import(<http://example.org/other>)\nAnnotation(rdfs:comment \"an ontology\")\nAnnotation(Annotation(rdfs:label \"nested\"@en) rdfs:seeAlso <http://example.org/doc>)\nSubClassOf(Annotation(rdfs:comment \"one\") :A :B)\nSubClassOf(Annotation(rdfs:comment \"two\") Annotation(ex:n _:n) :B :C)\nDeclaration(Annotation(rdfs:label :L) Class(:D))",
    ));
    same_mapping(&p, &theirs);
    assert_eq!(p.triples.len(), theirs.triples.len());
    // The ontology's annotation: its predicate at the property's value,
    // its object at the literal.
    let t = p
        .triples
        .iter()
        .find(|t| at(&text, t.o_span) == "<Literal>an ontology</Literal>")
        .unwrap();
    assert_eq!(at(&text, t.p_span), "rdfs:comment");
    assert_eq!(
        statements(&text, &p)[2..4],
        [
            (
                "<Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>an ontology</Literal></Annotation>",
                None
            ),
            (nested.as_str(), None),
        ]
    );
}

/// `AnnotationAssertion` with each subject form and each value form; its
/// roles as a Functional assertion gets them.
#[test]
fn annotation_assertions_of_each_subject_and_value_form() {
    let subjects = [
        "<IRI>http://example.org/o#A</IRI>",
        "<AbbreviatedIRI>:A</AbbreviatedIRI>",
        "<AnonymousIndividual nodeID=\"s\"/>",
    ];
    let values = [
        "<IRI>http://example.org/v</IRI>",
        "<AbbreviatedIRI>ex:v</AbbreviatedIRI>",
        "<AnonymousIndividual nodeID=\"v\"/>",
        "<Literal>v</Literal>",
    ];
    let ofn_subjects = ["<http://example.org/o#A>", ":A", "_:s"];
    let ofn_values = ["<http://example.org/v>", "ex:v", "_:v", "\"v\""];
    let mut body = Vec::new();
    let mut theirs = Vec::new();
    for (s, os) in subjects.iter().zip(ofn_subjects) {
        for (v, ov) in values.iter().zip(ofn_values) {
            body.push(format!(
                "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:seeAlso\"/>{s}{v}</AnnotationAssertion>"
            ));
            theirs.push(format!("AnnotationAssertion(rdfs:seeAlso {os} {ov})"));
        }
    }
    let text = doc(&body.join("\n"));
    let p = clean(&text);
    same_mapping(&p, &functional(&ofn(&theirs.join("\n"))));
    assert_eq!(p.triples.len(), 1 + 12);
    // The first: its subject defined there, its value an object.
    assert_eq!(
        refs(&text, &p)[1..4],
        [
            ("rdfs:seeAlso", &*format!("{RDFS}seeAlso"), Predicate, pn(4)),
            (
                "http://example.org/o#A",
                "http://example.org/o#A",
                Subject,
                IRI
            ),
            ("http://example.org/v", "http://example.org/v", Object, IRI),
        ]
    );
}

/// A term's own label is not a use of it: after
/// `a_terms_own_axioms_are_not_uses_of_it` in `ofn`.
#[test]
fn a_term_s_own_annotations_are_not_uses_of_it() {
    let text = doc(&[
        "<Declaration><Class abbreviatedIRI=\":Old\"/></Declaration>",
        &label("<Literal>a</Literal>").replace(":A", ":Old"),
        "<SubClassOf><Class abbreviatedIRI=\":New\"/><Class abbreviatedIRI=\":Old\"/></SubClassOf>",
        &label("<Literal>u</Literal>").replace(":A", ":Undeclared"),
        &label("<Literal>c</Literal>")
            .replace(":A", ":Undeclared")
            .replace("rdfs:label", "rdfs:comment"),
    ]
    .join("\n"));
    let p = clean(&text);
    let roles: Vec<_> = p
        .refs
        .iter()
        .filter(|r| r.iri.as_str().ends_with("#Old") || r.iri.as_str().ends_with("#Undeclared"))
        .map(|r| r.role)
        .collect();
    assert_eq!(roles, [Subject, About, Object, Subject, About]);
}

/// `SubAnnotationPropertyOf`, `AnnotationPropertyDomain`,
/// `AnnotationPropertyRange`.
#[test]
fn the_annotation_property_axioms() {
    let text = doc(&[
        "<SubAnnotationPropertyOf><AnnotationProperty abbreviatedIRI=\":p\"/><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/></SubAnnotationPropertyOf>",
        "<AnnotationPropertyDomain><AnnotationProperty abbreviatedIRI=\":p\"/><AbbreviatedIRI>:A</AbbreviatedIRI></AnnotationPropertyDomain>",
        "<AnnotationPropertyRange><Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>r</Literal></Annotation><AnnotationProperty abbreviatedIRI=\":p\"/><IRI>http://www.w3.org/2001/XMLSchema#string</IRI></AnnotationPropertyRange>",
    ]
    .join("\n"));
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&ofn(
            "SubAnnotationPropertyOf(:p rdfs:label)\nAnnotationPropertyDomain(:p :A)\nAnnotationPropertyRange(Annotation(rdfs:comment \"r\") :p xsd:string)",
        )),
    );
    let sub = &p.triples[1];
    assert_eq!(
        (at(&text, sub.p_span), at(&text, sub.o_span)),
        ("SubAnnotationPropertyOf", "rdfs:label")
    );
    let domain = &p.triples[2];
    assert_eq!(
        (at(&text, domain.p_span), at(&text, domain.o_span)),
        ("AnnotationPropertyDomain", ":A")
    );
    let s = statements(&text, &p);
    assert_eq!(s[1].1, Some(":p"));
}

// ---- checks ----

/// Each check is an `owx-syntax` error at its exact span, in the words of
/// the Functional errors, and its item gives nothing.
#[test]
fn annotations_in_error() {
    let text = doc(&[
        "<SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/><Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>late</Literal></Annotation></SubClassOf>",
        "<SubClassOf><Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/></Annotation><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>",
        "<SubClassOf><Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>a</Literal><Literal>b</Literal></Annotation><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>",
        "<SubClassOf><Annotation><Class abbreviatedIRI=\":C\"/><Literal>a</Literal></Annotation><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>",
        "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><Literal>s</Literal><Literal>v</Literal></AnnotationAssertion>",
        "<Declaration><Class abbreviatedIRI=\":D\"/></Declaration>",
        "<Annotation><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><Literal>after</Literal></Annotation>",
        "<SubClassOf><Class abbreviatedIRI=\":E\"/><Class abbreviatedIRI=\":F\"/></SubClassOf>",
    ]
    .join("\n"));
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(
        e,
        [
            (
                CODE,
                "Annotation",
                "Expected the end of <SubClassOf>, found <Annotation>"
            ),
            (
                CODE,
                "</Annotation>",
                "Expected an IRI, a literal or an anonymous individual, found </Annotation>"
            ),
            (
                CODE,
                "Literal",
                "Expected the end of <Annotation>, found <Literal>"
            ),
            (
                CODE,
                "Class",
                "Expected an annotation property, found <Class>"
            ),
            (
                CODE,
                "Literal",
                "Expected an IRI or an anonymous individual, found <Literal>"
            ),
            (
                CODE,
                "Annotation",
                "An ontology annotation comes before the axioms"
            ),
        ]
    );
    // The spans of the two `Literal`s: the second of each.
    let second = text.find("<Literal>b</Literal>").unwrap() as u32 + 1;
    assert_eq!(p.errors[2].span, Span::new(second, second + 7));
    let subject = text.find("<Literal>s</Literal>").unwrap() as u32 + 1;
    assert_eq!(p.errors[4].span, Span::new(subject, subject + 7));
    same_mapping(
        &p,
        &functional(&ofn("Declaration(Class(:D))\nSubClassOf(:E :F)")),
    );
}

// ---- across features ----

#[cfg(test)]
mod across_features {
    use super::*;

    /// On one small document: hover on a class shows the label from its
    /// `AnnotationAssertion`; a deprecated class is reported at its use and
    /// not at its own axioms; the outline names each term once.
    #[test]
    fn the_features_read_labels_and_deprecation() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::{diagnostics, hover, symbols};
        use crate::knowledge::{Knowledge, Source};
        use lsp_types::HoverContents;
        use std::sync::Arc;
        let text = doc(&[
            "<Declaration><Class abbreviatedIRI=\":Old\"/></Declaration>",
            "<Declaration><Class abbreviatedIRI=\":New\"/></Declaration>",
            "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"owl:deprecated\"/><AbbreviatedIRI>:Old</AbbreviatedIRI><Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#boolean\">true</Literal></AnnotationAssertion>",
            "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:Old</AbbreviatedIRI><Literal xml:lang=\"en\">old thing</Literal></AnnotationAssertion>",
            "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>:New</AbbreviatedIRI><Literal>new thing</Literal></AnnotationAssertion>",
            "<SubClassOf><Class abbreviatedIRI=\":Old\"/><Class abbreviatedIRI=\":Thing\"/></SubClassOf>",
            "<SubClassOf><Class abbreviatedIRI=\":New\"/><Class abbreviatedIRI=\":Old\"/></SubClassOf>",
        ]
        .join("\n"));
        // The tree fills its signatures.
        clean(&text);
        let a = Arc::new(Analysis::with_syntax(
            URL.into(),
            &text,
            Arc::default(),
            true,
            crate::syntax::Syntax::OwlXml,
        ));
        let mut k = Knowledge::default();
        k.set(URL, Source::Open, a.clone());
        let enc = Encoding::Utf8;
        // Hover on `:New` in the last axiom.
        let use_at = text.rfind("\":New\"").unwrap() + 2;
        let h = hover::hover(&a, &k, enc, a.position(enc, use_at as u32)).unwrap();
        let HoverContents::Markup(m) = &h.contents else {
            panic!("{h:?}")
        };
        assert!(m.value.contains("new thing"), "{}", m.value);
        // Deprecated at its use only, in the last axiom.
        let d = diagnostics::diagnostics_with(&a, &k, enc, &Default::default());
        let found: Vec<_> = d
            .iter()
            .map(|d| match &d.code {
                Some(lsp_types::NumberOrString::String(c)) => (c.as_str(), d.range.start),
                _ => ("", d.range.start),
            })
            .collect();
        let old = text.rfind("\":Old\"").unwrap() + 1;
        assert_eq!(
            found,
            [("deprecated-term", a.position(enc, old as u32))],
            "{d:?}"
        );
        // The outline names each term once.
        let outline = symbols::document_symbols(&a, enc);
        let mut names: Vec<String> = Vec::new();
        fn walk(s: &[lsp_types::DocumentSymbol], out: &mut Vec<String>) {
            for s in s {
                out.push(s.name.clone());
                walk(s.children.as_deref().unwrap_or_default(), out);
            }
        }
        walk(&outline, &mut names);
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "{names:?}");
        assert!(names.iter().any(|n| n.contains("Old")), "{names:?}");
        assert!(names.iter().any(|n| n.contains("New")), "{names:?}");
    }
}

/// A literal's object span and the reference at a datatype's value hold
/// when the literal is a data value too: in `DataPropertyAssertion`.
#[test]
fn a_literal_where_an_axiom_takes_one() {
    let text = doc(
        "<DataPropertyAssertion><DataProperty abbreviatedIRI=\":age\"/><NamedIndividual abbreviatedIRI=\":a\"/><Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#integer\">7</Literal></DataPropertyAssertion>",
    );
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&ofn("DataPropertyAssertion(:age :a \"7\"^^xsd:integer)")),
    );
    let t = &p.triples[1];
    assert_eq!(
        at(&text, t.o_span),
        "<Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#integer\">7</Literal>"
    );
}
