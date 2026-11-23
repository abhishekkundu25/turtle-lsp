//! The reader, through what the mapping makes of its trees: the document
//! level, IRIs, prefixes and bases, declarations, a few axioms (the rest
//! are in `axiom_tests`), the features over one document, and recovery and
//! limits. Each test gives a text and asserts the triples
//! (against the mapping of the same ontology in Functional syntax), the
//! references with their spans, roles and forms, the statements, and the
//! errors with their codes, spans and messages. Every tree the reader
//! builds here fills its signatures ([`signature::check`]).

use super::source::OwlXml;
use super::{CODE, parse, reader};
use crate::ofn::signature;
use crate::ofn::tree::{Document, NodeKind};
use crate::syntax::{Form, Iri, Node, Parsed, Role, Span};

pub(super) const URL: &str = "file:///w/o.owx";

/// The start of the documents below: the ontology `<http://example.org/o>`
/// and the prefixes `:` and `ex:`.
pub(super) const HEAD: &str = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n<Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n";

/// The same in Functional syntax.
pub(super) const OFN_HEAD: &str = "Prefix(:=<http://example.org/o#>)\nPrefix(ex:=<http://example.org/>)\nOntology(<http://example.org/o>\n";

pub(super) fn owx(body: &str) -> String {
    format!("{HEAD}{body}\n</Ontology>\n")
}

pub(super) fn ofn(body: &str) -> String {
    format!("{OFN_HEAD}{body}\n)\n")
}

/// The parse of `text`, whose tree fills its signatures.
#[track_caller]
pub(super) fn read(text: &str, base: Option<&str>) -> Parsed {
    let (doc, terms) = reader::read(text, base);
    if let Err(m) = signature::check(&doc) {
        panic!("{m:?}\n{:?}\n{text}", doc.nodes);
    }
    let p = crate::ofn::rdf::map_with(&OwlXml::new(text, &terms), doc, base);
    assert_eq!(p, parse(text, base));
    p
}

/// The parse of `text`, which has no error.
#[track_caller]
pub(super) fn clean(text: &str) -> Parsed {
    let p = read(text, Some(URL));
    assert!(p.errors.is_empty(), "{:#?}\n{text}", p.errors);
    p
}

/// The parse of a Functional text, which has no error.
#[track_caller]
pub(super) fn functional(text: &str) -> Parsed {
    let p = crate::ofn::parse(text, None);
    assert!(p.errors.is_empty(), "{:#?}\n{text}", p.errors);
    p
}

pub(super) fn at(text: &str, span: Span) -> &str {
    &text[span.start as usize..span.end as usize]
}

pub(super) fn triples(p: &Parsed) -> Vec<(Node, Iri, Node)> {
    p.triples
        .iter()
        .map(|t| (t.s.clone(), t.p, t.o.clone()))
        .collect()
}

/// The references: the text at each, its IRI, role and form.
pub(super) fn refs<'t>(text: &'t str, p: &Parsed) -> Vec<(&'t str, &'static str, Role, Form)> {
    p.refs
        .iter()
        .map(|r| (at(text, r.span), r.iri.as_str(), r.role, r.form))
        .collect()
}

/// The text of each statement, and of its subject's span.
pub(super) fn statements<'t>(text: &'t str, p: &Parsed) -> Vec<(&'t str, Option<&'t str>)> {
    p.statements
        .iter()
        .map(|s| (at(text, s.span), s.subject.map(|(_, span)| at(text, span))))
        .collect()
}

/// The errors: code, the text at each, message.
pub(super) fn errors<'t>(text: &'t str, p: &'t Parsed) -> Vec<(&'static str, &'t str, &'t str)> {
    p.errors
        .iter()
        .map(|e| (e.code, at(text, e.span), e.message()))
        .collect()
}

/// The IRIs and roles of the references, the subjects of the statements,
/// and the triples of `ours` are those of `theirs`.
#[track_caller]
pub(super) fn same_mapping(ours: &Parsed, theirs: &Parsed) {
    assert_eq!(triples(ours), triples(theirs));
    let roles =
        |p: &Parsed| -> Vec<(Iri, Role)> { p.refs.iter().map(|r| (r.iri, r.role)).collect() };
    assert_eq!(roles(ours), roles(theirs));
    let subjects = |p: &Parsed| -> Vec<Option<Iri>> {
        p.statements
            .iter()
            .map(|s| s.subject.map(|(iri, _)| iri))
            .collect()
    };
    assert_eq!(subjects(ours), subjects(theirs));
}

/// A node's kind without what the syntax spells differently (where a
/// literal's string ends, how an IRI is written), and the index past its
/// subtree: two trees of one shape map alike.
pub(super) fn shape(doc: &Document) -> Vec<(String, u32)> {
    doc.nodes
        .iter()
        .map(|n| {
            let kind = match n.kind {
                NodeKind::Construct(k) => k.as_str().to_string(),
                NodeKind::Iri { .. } => "IRI".into(),
                NodeKind::Literal { .. } => "Literal".into(),
                kind => format!("{kind:?}"),
            };
            (kind, n.next)
        })
        .collect()
}

use Role::*;
pub(super) const IRI: Form = Form::Iri;
pub(super) const ANCHOR: Form = Form::Anchor;
pub(super) fn pn(prefix_len: u32) -> Form {
    Form::PName { prefix_len }
}

/// The example of section 2 of the Recommendation, its names neutral.
const EXAMPLE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<Ontology xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"
    xsi:schemaLocation="http://www.w3.org/2002/07/owl# http://www.w3.org/2009/09/owl2-xml.xsd"
    xmlns="http://www.w3.org/2002/07/owl#"
    xml:base="http://example.org/myOntology"
    ontologyIRI="http://example.org/myOntology">

    <Prefix name="myOnt" IRI="http://example.org/myOntology#"/>
    <Import>http://example.org/someOtherOntology</Import>

    <Declaration>
        <Class IRI="#Animal"/>
    </Declaration>
    <Declaration>
        <Class abbreviatedIRI="myOnt:Tabloid"/>
    </Declaration>
    <Declaration>
        <ObjectProperty IRI="#eats"/>
    </Declaration>
    <Declaration>
        <ObjectProperty IRI="#reads"/>
    </Declaration>

    <SubClassOf>
        <Class abbreviatedIRI="myOnt:Animal"/>
        <ObjectAllValuesFrom>
            <ObjectProperty IRI="#reads"/>
            <Class IRI="#Tabloid"/>
        </ObjectAllValuesFrom>
    </SubClassOf>

</Ontology>
"##;

#[test]
fn the_recommendation_s_example_reads() {
    let p = clean(EXAMPLE);
    let theirs = functional(
        "Prefix(myOnt:=<http://example.org/myOntology#>)\nOntology(<http://example.org/myOntology>\nImport(<http://example.org/someOtherOntology>)\nDeclaration(Class(<http://example.org/myOntology#Animal>))\nDeclaration(Class(myOnt:Tabloid))\nDeclaration(ObjectProperty(<http://example.org/myOntology#eats>))\nDeclaration(ObjectProperty(<http://example.org/myOntology#reads>))\nSubClassOf(myOnt:Animal ObjectAllValuesFrom(<http://example.org/myOntology#reads> <http://example.org/myOntology#Tabloid>))\n)\n",
    );
    same_mapping(&p, &theirs);
    assert_eq!(p.triples.len(), 10);
    let o = "http://example.org/myOntology";
    assert_eq!(
        refs(EXAMPLE, &p),
        [
            (o, o, Subject, IRI),
            (
                "http://example.org/someOtherOntology",
                "http://example.org/someOtherOntology",
                Object,
                IRI
            ),
            (
                "#Animal",
                "http://example.org/myOntology#Animal",
                Subject,
                IRI
            ),
            (
                "myOnt:Tabloid",
                "http://example.org/myOntology#Tabloid",
                Subject,
                pn(5)
            ),
            ("#eats", "http://example.org/myOntology#eats", Subject, IRI),
            (
                "#reads",
                "http://example.org/myOntology#reads",
                Subject,
                IRI
            ),
            (
                "myOnt:Animal",
                "http://example.org/myOntology#Animal",
                About,
                pn(5)
            ),
            ("#reads", "http://example.org/myOntology#reads", Object, IRI),
            (
                "#Tabloid",
                "http://example.org/myOntology#Tabloid",
                Object,
                IRI
            ),
        ]
    );
    let s = statements(EXAMPLE, &p);
    assert!(s[0].0.starts_with("<Ontology xmlns:xsi"), "{:?}", s[0]);
    assert!(
        s[0].0
            .ends_with("ontologyIRI=\"http://example.org/myOntology")
    );
    assert_eq!(s[0].1, Some(o));
    assert_eq!(
        s[1..],
        [
            (
                "<Import>http://example.org/someOtherOntology</Import>",
                None
            ),
            (
                "<Declaration>\n        <Class IRI=\"#Animal\"/>\n    </Declaration>",
                Some("#Animal")
            ),
            (
                "<Declaration>\n        <Class abbreviatedIRI=\"myOnt:Tabloid\"/>\n    </Declaration>",
                Some("myOnt:Tabloid")
            ),
            (
                "<Declaration>\n        <ObjectProperty IRI=\"#eats\"/>\n    </Declaration>",
                Some("#eats")
            ),
            (
                "<Declaration>\n        <ObjectProperty IRI=\"#reads\"/>\n    </Declaration>",
                Some("#reads")
            ),
            (
                "<SubClassOf>\n        <Class abbreviatedIRI=\"myOnt:Animal\"/>\n        <ObjectAllValuesFrom>\n            <ObjectProperty IRI=\"#reads\"/>\n            <Class IRI=\"#Tabloid\"/>\n        </ObjectAllValuesFrom>\n    </SubClassOf>",
                None
            ),
        ]
    );
    // A triple a rule makes is at the element's name.
    let ty = p
        .triples
        .iter()
        .find(|t| t.p.as_str().ends_with("subClassOf"))
        .unwrap();
    assert_eq!(at(EXAMPLE, ty.p_span), "SubClassOf");
    assert!(at(EXAMPLE, ty.o_span).starts_with("<ObjectAllValuesFrom>"));
    let all = p
        .triples
        .iter()
        .find(|t| t.p.as_str().ends_with("#allValuesFrom"))
        .unwrap();
    assert_eq!(
        (at(EXAMPLE, all.p_span), at(EXAMPLE, all.o_span)),
        ("ObjectAllValuesFrom", "#Tabloid")
    );
    let header = &p.triples[0];
    assert_eq!(at(EXAMPLE, header.p_span), "Ontology");
    let declared = &p.triples[2];
    assert_eq!(at(EXAMPLE, declared.p_span), "Class");
}

// ---- the root ----

#[test]
fn the_root_is_ontology_in_the_owl_namespace_under_any_prefix() {
    let text = "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<owl:Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n<owl:Declaration><owl:Class abbreviatedIRI=\":A\"/></owl:Declaration>\n</owl:Ontology>";
    let p = clean(text);
    same_mapping(&p, &functional(&ofn("Declaration(Class(:A))")));
    assert_eq!(at(text, p.triples[0].p_span), "owl:Ontology");
    assert_eq!(at(text, p.triples[1].p_span), "owl:Class");
    assert_eq!(
        statements(text, &p)[1].0,
        "<owl:Declaration><owl:Class abbreviatedIRI=\":A\"/></owl:Declaration>"
    );
}

#[test]
fn a_root_that_is_not_ontology_is_one_error_that_says_what_the_text_is() {
    let rdf = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"http://example.org/a\"/></rdf:RDF>";
    let html = "<html><body>x</body></html>";
    let bare = "<Ontology ontologyIRI=\"http://example.org/o\"><Declaration><Class IRI=\"http://example.org/A\"/></Declaration></Ontology>";
    let owl = "http://www.w3.org/2002/07/owl#";
    for (text, name, message) in [
        (
            rdf,
            "rdf:RDF",
            format!(
                "The root element is <rdf:RDF>: the text is RDF/XML, not OWL/XML, whose root is <Ontology> in {owl}"
            ),
        ),
        (
            html,
            "html",
            format!(
                "The root element is <html>: the text is XML but not OWL/XML, whose root is <Ontology> in {owl}"
            ),
        ),
        (
            bare,
            "Ontology",
            format!(
                "The root element <Ontology> is not in OWL's namespace: OWL/XML's root is <Ontology> in {owl}"
            ),
        ),
    ] {
        let p = read(text, Some(URL));
        assert_eq!(errors(text, &p), [(CODE, name, message.as_str())], "{text}");
        assert!(p.triples.is_empty() && p.refs.is_empty() && p.statements.is_empty());
    }
}

#[test]
fn an_element_in_another_namespace_or_unknown_is_an_error_at_its_name() {
    let text = owx(
        "<ex:Note xmlns:ex=\"http://example.org/\"><Declaration/></ex:Note>\n<Klass/>\n<Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration>\n<SubClassOf><Klass IRI=\"http://example.org/o#A\"/><Class IRI=\"http://example.org/o#B\"/></SubClassOf>\n<SubClassOf><x:C xmlns:x=\"http://example.org/x\"/><Class IRI=\"http://example.org/o#B\"/></SubClassOf>\n<SubClassOf><Class abbreviatedIRI=\":B\"/><Class abbreviatedIRI=\":C\"/></SubClassOf>",
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "ex:Note",
                "<ex:Note> is in the namespace http://example.org/, not in OWL's (http://www.w3.org/2002/07/owl#)"
            ),
            (CODE, "Klass", "<Klass> is no element of OWL/XML"),
            (CODE, "Klass", "<Klass> is no element of OWL/XML"),
            (
                CODE,
                "x:C",
                "<x:C> is in the namespace http://example.org/x, not in OWL's (http://www.w3.org/2002/07/owl#)"
            ),
        ]
    );
    same_mapping(
        &p,
        &functional(&ofn("Declaration(Class(:A))\nSubClassOf(:B :C)")),
    );
}

// ---- the header ----

#[test]
fn the_header_its_iris_and_its_statement() {
    let tail = "><Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration></Ontology>";
    let start = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"";
    let o = "http://example.org/o";
    let v = "http://example.org/o/1";
    // The ontology IRI alone.
    let text = format!("{start} ontologyIRI=\"{o}\"{tail}");
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&format!("Ontology(<{o}> Declaration(Class(<{o}#A>)))")),
    );
    assert_eq!(
        statements(&text, &p)[0],
        (&text[..text.find(tail).unwrap() - 1], Some(o))
    );
    assert_eq!(refs(&text, &p)[0], (o, o, Subject, IRI));
    // With a version IRI.
    let text = format!("{start} ontologyIRI=\"{o}\" versionIRI=\"{v}\"{tail}");
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&format!(
            "Ontology(<{o}> <{v}> Declaration(Class(<{o}#A>)))"
        )),
    );
    assert!(
        statements(&text, &p)[0]
            .0
            .ends_with(&format!("versionIRI=\"{v}"))
    );
    assert_eq!(
        refs(&text, &p)[..2],
        [(o, o, Subject, IRI), (v, v, Object, IRI)]
    );
    // Neither: a blank node, the statement to the element's name.
    let text = format!("{start}{tail}");
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&format!("Ontology(Declaration(Class(<{o}#A>)))")),
    );
    assert_eq!(statements(&text, &p)[0], ("<Ontology", None));
    // The version IRI first in the tag: the references in text order, the
    // statement to the end of the later IRI, so it holds its subject (found
    // by the OWL/XML chaos shards).
    let text = format!("{start} versionIRI=\"{v}\" ontologyIRI=\"{o}\"{tail}");
    let p = clean(&text);
    let theirs = functional(&format!(
        "Ontology(<{o}> <{v}> Declaration(Class(<{o}#A>)))"
    ));
    assert_eq!(triples(&p), triples(&theirs));
    assert_eq!(
        refs(&text, &p)[..2],
        [(v, v, Object, IRI), (o, o, Subject, IRI)]
    );
    let (header, subject) = statements(&text, &p)[0];
    assert!(header.ends_with(&format!("ontologyIRI=\"{o}")), "{header}");
    assert!(header.contains(&format!("versionIRI=\"{v}\"")), "{header}");
    assert_eq!(subject, Some(o));
}

#[test]
fn a_header_in_error_gives_no_header_and_no_import_and_the_axioms_map() {
    let body = "<Import>http://example.org/i</Import><Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration></Ontology>";
    let start = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"";
    for (attrs, at_text, message) in [
        (
            " versionIRI=\"http://example.org/o/1\"",
            "versionIRI",
            "A versionIRI needs an ontologyIRI (structural specification, section 3.1)",
        ),
        (
            " ontologyIRI=\"http://example.org/o o\"",
            "http://example.org/o o",
            "Invalid IRI: Invalid IRI code point ' '",
        ),
    ] {
        let text = format!("{start}{attrs}>{body}");
        let p = read(&text, Some(URL));
        let errs = errors(&text, &p);
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0], (CODE, at_text, message));
        // No header triple: the declaration's alone.
        let theirs = functional("Ontology(Declaration(Class(<http://example.org/o#A>)))");
        assert_eq!(triples(&p), triples(&theirs)[1..]);
        assert_eq!(
            statements(&text, &p),
            [(
                "<Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration>",
                Some("http://example.org/o#A")
            )]
        );
    }
}

// ---- the order of the children of `Ontology` ----

#[test]
fn a_prefix_after_an_import_or_an_import_after_an_axiom_is_an_item_with_an_error() {
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n<Import>http://example.org/i</Import>\n<Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n<Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n<Import>http://example.org/j</Import>\n<Declaration><Class abbreviatedIRI=\":B\"/></Declaration>\n</Ontology>";
    let p = read(text, Some(URL));
    assert_eq!(
        errors(text, &p),
        [
            (
                CODE,
                "Prefix",
                "A <Prefix> comes before the imports, annotations and axioms"
            ),
            (
                CODE,
                "Import",
                "An <Import> comes before the ontology's annotations and axioms"
            ),
        ]
    );
    // The misplaced prefix is not declared; the misplaced import gives nothing.
    assert_eq!(p.prefixes.len(), 1);
    same_mapping(
        &p,
        &functional(
            "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nImport(<http://example.org/i>)\nDeclaration(Class(:A))\nDeclaration(Class(:B)))",
        ),
    );
}

// ---- prefixes ----

#[test]
fn a_prefix_declaration_and_its_spans() {
    let text = owx(
        "<Prefix name=\"x\" IRI=\"x/\"/>\n<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\":B\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"x:C\"/></Declaration>",
    );
    let p = clean(&text);
    let decls: Vec<_> = p
        .prefixes
        .iter()
        .map(|d| {
            (
                d.name.as_str(),
                d.iri.as_str(),
                at(&text, d.span),
                at(&text, d.name_span),
                at(&text, d.iri_span),
            )
        })
        .collect();
    assert_eq!(
        decls,
        [
            (
                "",
                "http://example.org/o#",
                "<Prefix name=\"\" IRI=\"http://example.org/o#\"/>",
                "",
                "http://example.org/o#"
            ),
            (
                "ex",
                "http://example.org/",
                "<Prefix name=\"ex\" IRI=\"http://example.org/\"/>",
                "ex",
                "http://example.org/"
            ),
            // `IRI` is an xsd:anyURI: it resolves against the document's URL.
            (
                "x",
                "file:///w/x/",
                "<Prefix name=\"x\" IRI=\"x/\"/>",
                "x",
                "x/"
            ),
        ]
    );
    // The empty name's span is where its value is.
    let empty = p.prefixes[0].name_span;
    assert_eq!(empty.start, empty.end);
    assert_eq!(
        &text[..empty.start as usize],
        &HEAD[..HEAD.find("name=\"\"").unwrap() + 6]
    );
    assert_eq!(
        refs(&text, &p)[1..],
        [
            ("ex:A", "http://example.org/A", Subject, pn(2)),
            (":B", "http://example.org/o#B", Subject, pn(0)),
            ("x:C", "file:///w/x/C", Subject, pn(1)),
        ]
    );
}

#[cfg(test)]
mod prefix_map {
    use super::*;

    /// A prefix declared as in Functional syntax holds what the Functional
    /// declaration holds: the document's prefix map is the same.
    #[test]
    fn a_prefix_declaration_holds_what_a_functional_one_holds() {
        let text = owx("<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>");
        let p = clean(&text);
        let theirs = functional(&ofn("Declaration(Class(ex:A))"));
        let decls = |p: &Parsed| -> Vec<(String, Iri)> {
            p.prefixes.iter().map(|d| (d.name.clone(), d.iri)).collect()
        };
        assert_eq!(decls(&p), decls(&theirs));
        use crate::analysis::Analysis;
        use std::sync::Arc;
        let ours = Analysis::with_syntax(
            URL.into(),
            &text,
            Arc::default(),
            true,
            crate::syntax::Syntax::OwlXml,
        );
        let theirs = Analysis::new(
            "file:///w/o.ofn".into(),
            &ofn("Declaration(Class(ex:A))"),
            Arc::default(),
            true,
        );
        assert_eq!(ours.prefix_map(), theirs.prefix_map());
    }
}

#[test]
fn prefix_declarations_in_error() {
    let text = owx(
        "<Prefix IRI=\"http://example.org/a#\"/>\n<Prefix name=\"b\"/>\n<Prefix name=\"ex\" IRI=\"http://example.org/other/\"/>\n<Prefix name=\"1x\" IRI=\"http://example.org/c#\"/>\n<Prefix name=\"d\" IRI=\"http://example.org/d#\" id=\"1\"/>\n<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"owl:Thing\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"nope:B\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"d:E\"/></Declaration>",
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (CODE, "Prefix", "<Prefix> needs the attribute name"),
            (CODE, "Prefix", "<Prefix> needs the attribute IRI"),
            (
                CODE,
                "ex",
                "The prefix 'ex:' is declared twice: the first declaration holds"
            ),
            (CODE, "1x", "'1x' is no prefix name (PN_PREFIX)"),
            (CODE, "id", "<Prefix> has no attribute id"),
            (
                CODE,
                "owl:Thing",
                "Undeclared prefix 'owl:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"owl\" IRI=\"http://www.w3.org/2002/07/owl#\"/>"
            ),
            (
                CODE,
                "nope:B",
                "Undeclared prefix 'nope:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"nope\" IRI=\"…\"/>"
            ),
            (
                CODE,
                "d:E",
                "Undeclared prefix 'd:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"d\" IRI=\"…\"/>"
            ),
        ]
    );
    // The first `ex:` holds.
    assert_eq!(p.prefixes.len(), 2);
    same_mapping(&p, &functional(&ofn("Declaration(Class(ex:A))")));
}

// ---- IRIs ----

#[test]
fn iris_resolve_against_the_base_in_scope() {
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"http://example.org/one/\" ontologyIRI=\"o\">\n<Declaration><Class IRI=\"http://example.org/A\"/></Declaration>\n<Declaration><Class IRI=\"B\"/></Declaration>\n<Declaration xml:base=\"two/\"><Class IRI=\"C\"/></Declaration>\n<Declaration xml:base=\"two/\"><Class xml:base=\"../three/x\" IRI=\"#D\"/></Declaration>\n<Declaration><Class IRI=\"http://example.org/a/../E\"/></Declaration>\n</Ontology>";
    let p = clean(text);
    let iri = "http://example.org/";
    assert_eq!(
        refs(text, &p),
        [
            ("o", "http://example.org/one/o", Subject, IRI),
            ("http://example.org/A", "http://example.org/A", Subject, IRI),
            ("B", "http://example.org/one/B", Subject, IRI),
            ("C", "http://example.org/one/two/C", Subject, IRI),
            ("#D", "http://example.org/one/three/x#D", Subject, IRI),
            (
                "http://example.org/a/../E",
                "http://example.org/E",
                Subject,
                IRI
            ),
        ]
    );
    same_mapping(
        &p,
        &functional(&format!(
            "Ontology(<{iri}one/o>\nDeclaration(Class(<{iri}A>))\nDeclaration(Class(<{iri}one/B>))\nDeclaration(Class(<{iri}one/two/C>))\nDeclaration(Class(<{iri}one/three/x#D>))\nDeclaration(Class(<{iri}E>)))"
        )),
    );
    // Without an `xml:base`, against the document's URL.
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"><Declaration><Class IRI=\"#A\"/></Declaration></Ontology>";
    let p = clean(text);
    assert_eq!(refs(text, &p), [("#A", "file:///w/o.owx#A", Subject, IRI)]);
    // Without either, a relative IRI is an error at its value.
    let p = read(text, None);
    assert_eq!(
        errors(text, &p),
        [(
            CODE,
            "#A",
            "<#A> is a relative IRI, and no base is in scope to resolve it against"
        )]
    );
    assert!(p.triples.len() == 1 && p.refs.is_empty());
}

#[test]
fn an_iri_written_through_a_reference_is_an_anchor() {
    let text = "<!DOCTYPE Ontology [\n<!ENTITY o \"http://example.org/o#\">\n<!ENTITY rdfs \"http://www.w3.org/2000/01/rdf-schema#\">\n]>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"&o;\"/>\n<Declaration><AnnotationProperty IRI=\"&rdfs;comment\"/></Declaration>\n<Declaration><Class IRI=\"http://example.org/o#A&amp;B\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"&#58;C\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\":D\"/></Declaration>\n</Ontology>";
    let p = clean(text);
    assert_eq!(
        refs(text, &p)[1..],
        [
            (
                "&rdfs;comment",
                "http://www.w3.org/2000/01/rdf-schema#comment",
                Subject,
                ANCHOR
            ),
            (
                "http://example.org/o#A&amp;B",
                "http://example.org/o#A&B",
                Subject,
                ANCHOR
            ),
            ("&#58;C", "http://example.org/o#C", Subject, ANCHOR),
            (":D", "http://example.org/o#D", Subject, pn(0)),
        ]
    );
    assert_eq!(at(text, p.prefixes[0].iri_span), "&o;");
    assert_eq!(p.prefixes[0].iri.as_str(), "http://example.org/o#");
}

#[test]
fn an_entity_has_one_iri_attribute_and_each_iri_is_one() {
    let text = owx(
        "<Declaration><Class IRI=\"http://example.org/o#A\" abbreviatedIRI=\":A\"/></Declaration>\n<Declaration><Class/></Declaration>\n<Declaration><Class IRI=\"http://example.org/o o\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"A\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"ex:a b\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\":B\"/></Declaration>",
    );
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(
        e[..2],
        [
            (
                CODE,
                "abbreviatedIRI",
                "<Class> has an IRI and an abbreviatedIRI: one of them, not both"
            ),
            (
                CODE,
                "Class",
                "<Class> needs the attribute IRI or abbreviatedIRI"
            ),
        ]
    );
    assert_eq!((e[2].0, e[2].1), (CODE, "http://example.org/o o"));
    assert!(e[2].2.starts_with("Invalid IRI: "), "{e:?}");
    assert_eq!(
        e[3..],
        [
            (CODE, "A", "'A' is no abbreviated IRI: it has no ':'"),
            (
                CODE,
                "ex:a b",
                "'ex:a b' is no abbreviated IRI: a prefix name, ':' and a local name"
            ),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:B))")));
}

// ---- references to prefixes, through the features ----

#[cfg(test)]
mod prefix_features {
    use super::*;

    /// The readers of a prefix declaration's spans, on an OWL/XML document:
    /// hover on the `name` value describes the prefix there; the outline
    /// selects the `name` value. On the prefix of an abbreviated IRI, hover and
    /// go-to-definition describe and reach the term, as in Functional syntax.
    #[test]
    fn the_features_read_a_prefix_declaration_and_an_abbreviated_iri() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::{hover, navigation, symbols};
        use crate::knowledge::{Knowledge, Source};
        use lsp_types::HoverContents;
        use std::sync::Arc;
        let text = owx(
            "<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n<SubClassOf><Class abbreviatedIRI=\"ex:A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>",
        );
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
        let pos = |offset: usize| a.position(enc, offset as u32);
        // Hover on the name value of `<Prefix name="ex" …>`.
        let name = text.find("name=\"ex\"").unwrap() + 6;
        let h = hover::hover(&a, &k, enc, pos(name + 1)).unwrap();
        let HoverContents::Markup(m) = &h.contents else {
            panic!("{h:?}")
        };
        assert_eq!(m.value, "**ex:** → `<http://example.org/>`");
        assert_eq!(
            h.range,
            Some(a.range(enc, Span::new(name as u32, name as u32 + 2)))
        );
        // The outline lists the prefixes, each selecting its name value.
        let outline = symbols::document_symbols(&a, enc);
        let prefixes = outline[0].children.as_ref().unwrap();
        let names: Vec<_> = prefixes
            .iter()
            .map(|s| (s.name.as_str(), s.selection_range))
            .collect();
        assert_eq!(
            names,
            [
                (":", a.range(enc, p_name(&a.parsed, 0))),
                ("ex:", a.range(enc, Span::new(name as u32, name as u32 + 2))),
            ]
        );
        // The empty name's span is empty, where its value is: hover there
        // describes `:`, and only there.
        let empty = a.parsed.prefixes[0].name_span;
        let h = hover::hover(&a, &k, enc, pos(empty.start as usize)).unwrap();
        let HoverContents::Markup(m) = &h.contents else {
            panic!("{h:?}")
        };
        assert_eq!(m.value, "**:** → `<http://example.org/o#>`");
        assert_eq!(h.range, Some(a.range(enc, empty)));
        assert!(hover::hover(&a, &k, enc, pos(empty.start as usize - 1)).is_none());
        // Go-to-definition reads references only in a read-only document: on
        // a name value, nothing.
        assert_eq!(navigation::definition(&a, &k, enc, pos(name + 1), None), []);
        // On the prefix of `ex:A` in the axiom: the term, defined at its
        // declaration.
        let use_at = text.rfind("\"ex:A\"").unwrap() + 1;
        let h = hover::hover(&a, &k, enc, pos(use_at + 1)).unwrap();
        assert_eq!(
            h.range,
            Some(a.range(enc, Span::new(use_at as u32, use_at as u32 + 4)))
        );
        let declared = text.find("\"ex:A\"").unwrap() + 1;
        let found = navigation::definition(&a, &k, enc, pos(use_at + 1), None);
        let spans: Vec<_> = found.iter().map(|l| l.range).collect();
        assert_eq!(
            spans,
            [a.range(enc, Span::new(declared as u32, declared as u32 + 4))]
        );
    }
}

fn p_name(p: &Parsed, i: usize) -> Span {
    p.prefixes[i].name_span
}

// ---- imports, declarations, the first axioms ----

#[test]
fn an_import_is_its_trimmed_content_resolved_against_the_base() {
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"http://example.org/o/\" ontologyIRI=\"http://example.org/o\">\n<Import>\n  other\n</Import>\n<Import>http://example.org/<!-- c -->two</Import>\n<Import><Class IRI=\"x\"/></Import>\n<Import xml:base=\"http://example.org/b/\"><![CDATA[three]]></Import>\n</Ontology>";
    let p = read(text, Some(URL));
    assert_eq!(
        errors(text, &p),
        [(
            CODE,
            "Class",
            "<Import> holds text and no element, found <Class>"
        )]
    );
    assert_eq!(
        refs(text, &p)[1..],
        [
            ("other", "http://example.org/o/other", Object, IRI),
            (
                "http://example.org/<!-- c -->two",
                "http://example.org/two",
                Object,
                ANCHOR
            ),
            ("three", "http://example.org/b/three", Object, IRI),
        ]
    );
    same_mapping(
        &p,
        &functional(
            "Ontology(<http://example.org/o>\nImport(<http://example.org/o/other>)\nImport(<http://example.org/two>)\nImport(<http://example.org/b/three>))",
        ),
    );
    assert_eq!(
        statements(text, &p)[1..],
        [
            ("<Import>\n  other\n</Import>", None),
            ("<Import>http://example.org/<!-- c -->two</Import>", None),
            (
                "<Import xml:base=\"http://example.org/b/\"><![CDATA[three]]></Import>",
                None
            ),
        ]
    );
}

/// An empty `xsd:anyURI` is the empty relative reference: an `IRI` or an
/// `Import` with no content is the base in scope (XML Base, RFC 3986),
/// as ROBOT writes the ontology's own IRI. Its node is the whole element,
/// which does not spell it. With no base, it is the relative-IRI error.
#[test]
fn an_empty_iri_or_import_is_the_base() {
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"http://example.org/lib\" ontologyIRI=\"http://example.org/lib\">\n<Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n<Import xml:base=\"http://example.org/other\"></Import>\n<Declaration><Class IRI=\"\"/></Declaration>\n<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><IRI></IRI><Literal>the library</Literal></AnnotationAssertion>\n<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><IRI/><Literal>empty</Literal></AnnotationAssertion>\n</Ontology>";
    let p = clean(text);
    let lib = "http://example.org/lib";
    assert_eq!(
        refs(text, &p),
        [
            (lib, lib, Subject, IRI),
            (
                "<Import xml:base=\"http://example.org/other\"></Import>",
                "http://example.org/other",
                Object,
                ANCHOR
            ),
            ("", lib, Subject, IRI),
            (
                "rdfs:label",
                "http://www.w3.org/2000/01/rdf-schema#label",
                Predicate,
                pn(4)
            ),
            ("<IRI></IRI>", lib, About, ANCHOR),
            (
                "rdfs:comment",
                "http://www.w3.org/2000/01/rdf-schema#comment",
                Predicate,
                pn(4)
            ),
            ("<IRI/>", lib, About, ANCHOR),
        ]
    );
    same_mapping(
        &p,
        &functional(&format!(
            "Prefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)\nOntology(<{lib}>\nImport(<http://example.org/other>)\nDeclaration(Class(<{lib}>))\nAnnotationAssertion(rdfs:label <{lib}> \"the library\")\nAnnotationAssertion(rdfs:comment <{lib}> \"empty\"))"
        )),
    );
    // Without a base, the relative-IRI error at the element.
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n<Import></Import>\n<AnnotationAssertion><AnnotationProperty IRI=\"http://example.org/p\"/><IRI/><Literal>x</Literal></AnnotationAssertion>\n</Ontology>";
    let p = read(text, None);
    let relative = "<> is a relative IRI, and no base is in scope to resolve it against";
    assert_eq!(
        errors(text, &p),
        [
            (CODE, "<Import></Import>", relative),
            (CODE, "<IRI/>", relative)
        ]
    );
    assert!(p.refs.is_empty());
}

#[test]
fn declarations_of_the_six_kinds() {
    let kinds = [
        "Class",
        "Datatype",
        "ObjectProperty",
        "DataProperty",
        "AnnotationProperty",
        "NamedIndividual",
    ];
    let body: Vec<String> = kinds
        .iter()
        .map(|k| format!("<Declaration><{k} abbreviatedIRI=\":{k}\"/></Declaration>"))
        .collect();
    let text = owx(&body.join("\n"));
    let p = clean(&text);
    let theirs: Vec<String> = kinds
        .iter()
        .map(|k| format!("Declaration({k}(:{k}))"))
        .collect();
    same_mapping(&p, &functional(&ofn(&theirs.join("\n"))));
    for (t, k) in p.triples[1..].iter().zip(kinds) {
        assert_eq!(at(&text, t.p_span), k);
        assert_eq!(at(&text, t.o_span), k);
    }
    let s = statements(&text, &p);
    assert_eq!(
        s[1],
        (
            "<Declaration><Class abbreviatedIRI=\":Class\"/></Declaration>",
            Some(":Class")
        )
    );
    assert!(s[1..].iter().all(|s| s.1.is_some()));
}

#[test]
fn an_entity_of_a_kind_the_slot_does_not_take_is_an_error() {
    let text = owx(
        "<SubClassOf><ObjectProperty abbreviatedIRI=\":p\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>\n<DisjointUnion><ObjectProperty abbreviatedIRI=\":p\"/><Class abbreviatedIRI=\":B\"/><Class abbreviatedIRI=\":C\"/></DisjointUnion>\n<Declaration><Datatype abbreviatedIRI=\":d\"/><Class abbreviatedIRI=\":B\"/></Declaration>\n<Declaration/>\n<SubClassOf><Class abbreviatedIRI=\":B\"/></SubClassOf>\n<ClassAssertion><Class abbreviatedIRI=\":B\"/><Class abbreviatedIRI=\":C\"/></ClassAssertion>\n<EquivalentClasses><Class abbreviatedIRI=\":B\"/></EquivalentClasses>\n<Declaration><Class abbreviatedIRI=\":E\"/></Declaration>",
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "ObjectProperty",
                "Expected a class expression, found <ObjectProperty>"
            ),
            (
                CODE,
                "ObjectProperty",
                "Expected a class, found <ObjectProperty>"
            ),
            (
                CODE,
                "Class",
                "Expected the end of <Declaration>, found <Class>"
            ),
            (
                CODE,
                "<Declaration/>",
                "Expected an entity (Class, Datatype, ObjectProperty, DataProperty, AnnotationProperty or NamedIndividual), found </Declaration>"
            ),
            (
                CODE,
                "</SubClassOf>",
                "Expected a class expression, found </SubClassOf>"
            ),
            (CODE, "Class", "Expected an individual, found <Class>"),
            (
                CODE,
                "EquivalentClasses",
                "<EquivalentClasses> needs at least 2 class expressions, found 1"
            ),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:E))")));
}

/// `SubClassOf` and `ClassAssertion` over named terms: an IRI is defined
/// at its declaration, else where an item first has it as subject.
#[test]
fn subclass_and_class_assertion_axioms() {
    let body = "<Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n<SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>\n<SubClassOf><Class abbreviatedIRI=\":C\"/><Class abbreviatedIRI=\":A\"/></SubClassOf>\n<ClassAssertion><Class abbreviatedIRI=\":A\"/><NamedIndividual abbreviatedIRI=\":x\"/></ClassAssertion>\n<ClassAssertion><Class abbreviatedIRI=\":C\"/><NamedIndividual abbreviatedIRI=\":x\"/></ClassAssertion>";
    let text = owx(body);
    let p = clean(&text);
    same_mapping(
        &p,
        &functional(&ofn(
            "Declaration(Class(:A))\nSubClassOf(:A :B)\nSubClassOf(:C :A)\nClassAssertion(:A :x)\nClassAssertion(:C :x)",
        )),
    );
    assert_eq!(
        refs(&text, &p)[1..],
        [
            (":A", "http://example.org/o#A", Subject, pn(0)),
            (":A", "http://example.org/o#A", About, pn(0)),
            (":B", "http://example.org/o#B", Object, pn(0)),
            (":C", "http://example.org/o#C", Subject, pn(0)),
            (":A", "http://example.org/o#A", Object, pn(0)),
            (":A", "http://example.org/o#A", Object, pn(0)),
            (":x", "http://example.org/o#x", Subject, pn(0)),
            (":C", "http://example.org/o#C", Object, pn(0)),
            (":x", "http://example.org/o#x", About, pn(0)),
        ]
    );
    let s = statements(&text, &p);
    assert_eq!(
        s[2..],
        [
            (
                "<SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\":B\"/></SubClassOf>",
                None
            ),
            (
                "<SubClassOf><Class abbreviatedIRI=\":C\"/><Class abbreviatedIRI=\":A\"/></SubClassOf>",
                Some(":C")
            ),
            (
                "<ClassAssertion><Class abbreviatedIRI=\":A\"/><NamedIndividual abbreviatedIRI=\":x\"/></ClassAssertion>",
                Some(":x")
            ),
            (
                "<ClassAssertion><Class abbreviatedIRI=\":C\"/><NamedIndividual abbreviatedIRI=\":x\"/></ClassAssertion>",
                None
            ),
        ]
    );
    let sub = p
        .triples
        .iter()
        .find(|t| t.p.as_str().ends_with("#subClassOf"))
        .unwrap();
    assert_eq!(at(&text, sub.p_span), "SubClassOf");
    assert_eq!(at(&text, sub.o_span), ":B");
}

// ---- attributes and content ----

#[test]
fn attributes_in_no_namespace_are_checked_and_others_passed_over() {
    let text = owx(
        "<Declaration xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xsi:schemaLocation=\"x y\" xml:space=\"preserve\" xml:lang=\"en\"><Class abbreviatedIRI=\":A\" ex:note=\"n\" xmlns:ex=\"http://example.org/\"/></Declaration>\n<Declaration kind=\"c\"><Class abbreviatedIRI=\":B\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\":C\" nope:x=\"1\"/></Declaration>\n<Prefix/>",
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (CODE, "kind", "<Declaration> has no attribute kind"),
            (
                CODE,
                "nope:x",
                "The prefix 'nope:' of the attribute nope:x is not declared"
            ),
            (
                CODE,
                "Prefix",
                "A <Prefix> comes before the imports, annotations and axioms"
            ),
            (CODE, "Prefix", "<Prefix> needs the attribute name"),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:A))")));
}

#[test]
fn white_space_comments_and_instructions_are_passed_over_and_text_is_an_error() {
    let text = owx(
        "<!-- c --><?pi x?>\n<Declaration> <!-- c --> <?pi?>\n<Class abbreviatedIRI=\":A\"> </Class> </Declaration>\ntext\n<Declaration>x<Class abbreviatedIRI=\":B\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\":C\">y</Class></Declaration>",
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (
                CODE,
                "text",
                "Text in <Ontology>, which holds elements only"
            ),
            (
                CODE,
                "x",
                "Text in <Declaration>, which holds elements only"
            ),
            (CODE, "y", "Text in <Class>, which holds elements only"),
        ]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:A))")));
}

// ---- recovery and limits ----

/// An axiom with an error gives no triple, reference or statement, and the
/// axioms before and after it map whole: a structure error, an XML error
/// inside it, an end that is implied, an element skipped for its depth, an
/// XML error past the scanner's cap.
#[test]
fn an_axiom_with_an_error_gives_nothing_and_the_others_map() {
    let a = "<Declaration><Class abbreviatedIRI=\":A\"/></Declaration>";
    let b = "<Declaration><Class abbreviatedIRI=\":B\"/></Declaration>";
    let deep = format!(
        "<SubClassOf><Class abbreviatedIRI=\":X\"/>{}<Class abbreviatedIRI=\":Y\"/>{}</SubClassOf>",
        "<ObjectComplementOf>".repeat(300),
        "</ObjectComplementOf>".repeat(300)
    );
    let past_cap = format!(
        "{}<Declaration><Class IRI=\"http://example.org/o#X&\"/></Declaration>",
        "<Declaration><Class IRI=\"http://example.org/o#Z&\"/></Declaration>\n".repeat(110)
    );
    for (broken, xml) in [
        ("<SubClassOf><Class abbreviatedIRI=\":X\"/><Literal>1</Literal></SubClassOf>".to_string(), false),
        ("<SubClassOf><Class abbreviatedIRI=\":X\"/><Class IRI=\"http://example.org/o#Y&\"/></SubClassOf>".to_string(), true),
        ("<SubClassOf><Class abbreviatedIRI=\":X\"/><Class abbreviatedIRI=\":Y\" a=\"1\" a=\"2\"/></SubClassOf>".to_string(), true),
        ("<SubClassOf><Class abbreviatedIRI=\":X\"><Class abbreviatedIRI=\":Y\"/></SubClassOf>".to_string(), true),
        (deep, true),
    ] {
        let text = owx(&format!("{a}\n{broken}\n{b}"));
        let p = read(&text, Some(URL));
        assert!(!p.errors.is_empty(), "{broken}");
        assert_eq!(
            p.errors.iter().any(|e| e.code == "xml-syntax"),
            xml,
            "{broken}: {:?}",
            p.errors
        );
        same_mapping(&p, &functional(&ofn("Declaration(Class(:A))\nDeclaration(Class(:B))")));
        assert_eq!(statements(&text, &p)[1..], [(a, Some(":A")), (b, Some(":B"))], "{broken}");
    }
    // An end implied by the ontology's: the axioms before map.
    let text = format!(
        "{HEAD}{a}\n<SubClassOf><Class abbreviatedIRI=\":X\"/><Class abbreviatedIRI=\":Y\"/>\n</Ontology>"
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [(
            "xml-syntax",
            "SubClassOf",
            "<SubClassOf> is not closed before </Ontology>"
        )]
    );
    same_mapping(&p, &functional(&ofn("Declaration(Class(:A))")));
    // Past the scanner's cap of 100 reported errors, an XML error still
    // breaks its axiom.
    let text = owx(&format!("{past_cap}\n{b}"));
    let p = read(&text, Some(URL));
    assert!(p.errors.iter().all(|e| e.code == "xml-syntax"));
    assert_eq!(p.errors.len(), crate::rdfxml::xml::MAX_ERRORS);
    assert!(!p.refs.iter().any(|r| r.iri.as_str().ends_with("#X")));
    assert!(!p.refs.iter().any(|r| r.iri.as_str().ends_with("#Z")));
    assert_eq!(statements(&text, &p)[1..], [(b, Some(":B"))]);
}

/// Once the scanner stops expanding references, an item that holds one it
/// left as written gives nothing.
#[test]
fn references_past_the_expansion_limit_break_their_items() {
    let long = format!("http://example.org/{}#", "v".repeat(60_000));
    let items: Vec<String> = (0..40)
        .map(|i| format!("<Declaration><Class IRI=\"&e;C{i}\"/></Declaration>"))
        .collect();
    let text = format!(
        "<!DOCTYPE Ontology [<!ENTITY e \"{long}\">]>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n{}\n<Declaration><Class IRI=\"http://example.org/last\"/></Declaration>\n</Ontology>",
        items.join("\n")
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [(
            "xml-syntax",
            "&e;",
            "entity expansion is over its limit: the rest of the references are not expanded"
        )]
    );
    let mapped: Vec<&str> = p.refs.iter().map(|r| r.iri.as_str()).collect();
    let expanded = mapped.len() - 1;
    assert!((10..40).contains(&expanded), "{expanded}");
    for (i, iri) in mapped[..expanded].iter().enumerate() {
        assert_eq!(*iri, format!("{long}C{i}"));
    }
    assert_eq!(mapped[expanded], "http://example.org/last");
    assert_eq!(p.statements.len(), 1 + expanded + 1);
}

/// The errors are one list in text order, past the cap the last one saying
/// so under `owx-syntax`.
#[test]
fn the_errors_are_one_list_in_text_order_and_capped() {
    let text = owx(
        "<Declaration><Klass/></Declaration>\n<Declaration><Class abbreviatedIRI=\":A\" b=\"&\"/></Declaration>\n<Declaration kind=\"1\"><Class abbreviatedIRI=\"&#0;\"/></Declaration>",
    );
    let p = read(&text, Some(URL));
    assert_eq!(
        errors(&text, &p),
        [
            (CODE, "Klass", "<Klass> is no element of OWL/XML"),
            (CODE, "b", "<Class> has no attribute b"),
            ("xml-syntax", "&", "`&` starts no reference (write `&amp;`)"),
            (CODE, "kind", "<Declaration> has no attribute kind"),
            (
                "xml-syntax",
                "&#0;",
                "not a character reference to an XML character"
            ),
        ]
    );
    let max = crate::ofn::MAX_ERRORS;
    let text = owx(&"<Declaration><Klass/></Declaration>\n".repeat(max + 5));
    let p = read(&text, Some(URL));
    assert_eq!(p.errors.len(), max + 1);
    let last = p.errors.last().unwrap();
    assert_eq!(last.code, CODE);
    assert_eq!(
        last.message(),
        format!("More than {max} errors; the rest are not reported")
    );
    assert_eq!(at(&text, last.span), "Klass");
    assert!(
        p.errors
            .windows(2)
            .all(|w| w[0].span.start < w[1].span.start)
    );
}

/// One text in an `IRI` and in an `abbreviatedIRI` is two IRIs.
#[test]
fn an_iri_and_an_abbreviated_iri_written_alike_are_two() {
    let text = owx(
        "<Declaration><Class IRI=\"ex:a\"/></Declaration>\n<Declaration><Class abbreviatedIRI=\"ex:a\"/></Declaration>\n<SubClassOf><Class IRI=\"ex:a\"/><Class abbreviatedIRI=\"ex:a\"/></SubClassOf>",
    );
    let p = clean(&text);
    assert_eq!(
        refs(&text, &p)[1..],
        [
            ("ex:a", "ex:a", Subject, IRI),
            ("ex:a", "http://example.org/a", Subject, pn(2)),
            ("ex:a", "ex:a", About, IRI),
            ("ex:a", "http://example.org/a", Object, pn(2)),
        ]
    );
}

/// Elements read from an entity's replacement text are read as any other,
/// at the reference's span; their IRIs are anchors.
#[test]
fn an_element_from_an_entity_is_read_at_the_reference() {
    let text = "<!DOCTYPE Ontology [<!ENTITY a \"<Class abbreviatedIRI=':A'/>\">]>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n<Declaration>&a;</Declaration>\n<SubClassOf><Class abbreviatedIRI=\":B\"/>&a;</SubClassOf>\n</Ontology>";
    let p = clean(text);
    same_mapping(
        &p,
        &functional(
            "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nDeclaration(Class(:A))\nSubClassOf(:B :A))",
        ),
    );
    assert_eq!(
        refs(text, &p)[1..],
        [
            ("&a;", "http://example.org/o#A", Subject, ANCHOR),
            (":B", "http://example.org/o#B", Subject, pn(0)),
            ("&a;", "http://example.org/o#A", Object, ANCHOR),
        ]
    );
    // The declaration's `rdf:type` is at its entity's name: the reference.
    assert_eq!(at(text, p.triples[1].p_span), "&a;");
}

/// An `xml:base` in error is reported at its value and breaks its element's
/// item; under it, a relative IRI is not known (no second error) and an
/// absolute one is.
#[test]
fn an_xml_base_in_error() {
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"o o\" ontologyIRI=\"http://example.org/o\">\n<Declaration><Class IRI=\"A\"/></Declaration>\n<Declaration><Class IRI=\"http://example.org/B\"/></Declaration>\n<Declaration xml:base=\"http://example.org/c/\"><Class IRI=\"C\"/></Declaration>\n</Ontology>";
    let p = read(text, None);
    assert_eq!(errors(text, &p).len(), 1);
    assert_eq!(
        (errors(text, &p)[0].0, errors(text, &p)[0].1),
        (CODE, "o o")
    );
    assert!(
        errors(text, &p)[0]
            .2
            .starts_with("xml:base is not an absolute IRI: ")
    );
    // The root's error breaks the header.
    same_mapping(
        &p,
        &functional("Ontology(Declaration(Class(<http://example.org/B>))\nDeclaration(Class(<http://example.org/c/C>)))")
            .without_header(),
    );
    let text = owx(
        "<Declaration xml:base=\"o o\"><Class IRI=\"http://example.org/A\"/></Declaration>\n<Declaration><Class IRI=\"http://example.org/B\"/></Declaration>",
    );
    let p = read(&text, None);
    assert_eq!(errors(&text, &p)[0].1, "o o");
    same_mapping(
        &p,
        &functional(&ofn("Declaration(Class(<http://example.org/B>))")),
    );
}

trait WithoutHeader {
    /// The parse without its header's triple and statement.
    fn without_header(self) -> Parsed;
}

impl WithoutHeader for Parsed {
    fn without_header(self) -> Parsed {
        Parsed {
            triples: self.triples[1..].to_vec(),
            statements: self.statements[1..].to_vec(),
            ..self
        }
    }
}

/// An `Import` whose text the scanner normalizes around its IRI (line ends)
/// is still spelled at its trimmed span.
#[test]
fn an_import_between_crlf_line_ends_is_spelled() {
    let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\r\n<Import>\r\n  http://example.org/i\r\n</Import>\r\n</Ontology>";
    let p = clean(text);
    assert_eq!(
        refs(text, &p)[1..],
        [("http://example.org/i", "http://example.org/i", Object, IRI)]
    );
}

/// The bases the reader resolves count against a budget of the mapper's
/// size: past it, one error in the mapper's words, and the items from
/// there on give nothing.
#[test]
fn the_bases_the_reader_resolves_are_charged_to_a_budget() {
    let long = format!("http://example.org/{}/", "x".repeat(100_000));
    let items: Vec<String> = (0..40)
        .map(|i| format!("<Declaration xml:base=\"d{i}/\"><Class IRI=\"http://example.org/A{i}\"/></Declaration>"))
        .collect();
    let text = format!(
        "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"{long}\" ontologyIRI=\"http://example.org/o\">\n{}\n</Ontology>",
        items.join("\n")
    );
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(e.len(), 1, "{e:?}");
    let message =
        "The IRIs are over 16 bytes per byte of text: the axioms from here on are not read";
    assert_eq!((e[0].0, e[0].2), (CODE, message));
    let at: usize = e[0]
        .1
        .trim_start_matches('d')
        .trim_end_matches('/')
        .parse()
        .unwrap();
    assert!((10..40).contains(&at), "{at}");
    let mapped: Vec<_> = p
        .refs
        .iter()
        .skip(1)
        .map(|r| r.iri.as_str().to_string())
        .collect();
    let expected: Vec<String> = (0..at)
        .map(|i| format!("http://example.org/A{i}"))
        .collect();
    assert_eq!(mapped, expected);
    assert_eq!(p.statements.len(), 1 + at);
}

/// The mapper's limits hold for OWL/XML too, under `owx-syntax`: a long base
/// resolved for many references.
#[test]
fn the_mapper_s_iri_budget_holds_under_owx_syntax() {
    let long = format!("http://example.org/{}/o", "x".repeat(60_000));
    let items: Vec<String> = (0..40)
        .map(|i| format!("<Declaration><Class IRI=\"#C{i}\"/></Declaration>"))
        .collect();
    let text = format!(
        "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"{long}\">\n{}\n</Ontology>",
        items.join("\n")
    );
    let p = read(&text, Some(URL));
    let e = errors(&text, &p);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(
        (e[0].0, e[0].2),
        (
            CODE,
            "The IRIs are over 16 bytes per byte of text: the axioms from here on are not read"
        )
    );
    let at: usize = e[0].1.trim_start_matches("#C").parse().unwrap();
    assert!((5..40).contains(&at), "{at}");
    assert_eq!(p.statements.len(), 1 + at);
    assert_eq!(p.refs.len(), at);
    assert_eq!(p.refs[0].iri.as_str(), format!("{long}#C0"));
}

/// An error quotes at most the first characters of an element's name, so
/// one long name in many errors costs bytes linear in the text: unknown
/// attributes of one element, and text runs in a long-named root.
#[test]
fn an_error_quotes_a_long_name_cut() {
    let long = "p".repeat(20_000);
    let attrs: String = (0..2_000).map(|i| format!(" a{i}=\"\"")).collect();
    let runs = "t<!---->".repeat(2_000);
    let text = format!(
        "<{long}:Ontology xmlns:{long}=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n<{long}:Declaration{attrs}><{long}:Class IRI=\"http://example.org/A\"/></{long}:Declaration>\n{runs}\n</{long}:Ontology>\n"
    );
    let p = read(&text, Some(URL));
    assert_eq!(p.errors.len(), 4_000);
    assert!(p.errors.iter().all(|e| e.message().len() < 200));
    let bytes: usize = p.errors.iter().map(|e| e.message().len()).sum();
    assert!(bytes <= 4 * text.len(), "{bytes} bytes of messages");
    let first = p.errors[0].message();
    assert!(
        first.starts_with("<ppp") && first.ends_with("…> has no attribute a0"),
        "{first}"
    );
    let run = p.errors.iter().find(|e| at(&text, e.span) == "t").unwrap();
    assert!(
        run.message().starts_with("Text in <ppp"),
        "{}",
        run.message()
    );
    assert!(run.message().len() < 200);
}
