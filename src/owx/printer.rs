//! A test-only printer: a [`Document`] of the Functional grammar written as
//! OWL/XML, so that every Functional text the project has is an OWL/XML
//! test input too. Every IRI is written full, so no `Prefix` is needed; an
//! anonymous individual is `n` and its number in order of first use (a
//! Functional label need not be an `NCName`). Each construct is the element
//! of its keyword, and each IRI the element its slot's category names
//! ([`Cat::entity`]): an entity element, or `IRI` where the category names
//! none. A cardinality is the `cardinality` attribute, a facet and its
//! value a `FacetRestriction`, and a key list its properties, flat. A SWRL
//! rule is written as OWL API writes it: a variable `<Variable IRI="…"/>`,
//! a built-in's IRI the `IRI` attribute of its `BuiltInAtom`.
//!
//! It is no feature and no oracle: one hand wrote it and the reader. The
//! tests below read the printed text back to the same tree shape and the
//! same graph, over the 78 W3C Functional texts and one text with every
//! construct of the schema.

use crate::ofn::signature::{Cat, Entity, Rep, signature};
use crate::ofn::tree::{Document, Keyword, NodeKind};
use crate::syntax::Span;
use rustc_hash::FxHashMap;
use std::fmt::Write as _;

const STANDARD: [(&str, &str); 4] = [
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
];

/// `doc`, the grammar's tree of the Functional text `text`, as OWL/XML.
pub(super) fn print(text: &str, doc: &Document) -> String {
    let mut p = Printer {
        text,
        doc,
        out: String::new(),
        labels: FxHashMap::default(),
    };
    p.document();
    p.out
}

struct Printer<'a> {
    text: &'a str,
    doc: &'a Document,
    out: String,
    labels: FxHashMap<&'a str, usize>,
}

/// Where a node is printed: the category of its slot.
#[derive(Clone, Copy)]
enum Place {
    In(Cat),
    /// The last argument of `DataSomeValuesFrom` or `DataAllValuesFrom`.
    Range,
}

impl<'a> Printer<'a> {
    fn at(&self, span: Span) -> &'a str {
        &self.text[span.start as usize..span.end as usize]
    }

    fn document(&mut self) {
        let doc = self.doc;
        self.out.push_str(
            "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"",
        );
        for (name, node) in [
            ("ontologyIRI", doc.ontology_iri),
            ("versionIRI", doc.version_iri),
        ] {
            if let Some(i) = node {
                let iri = self.iri(i);
                let _ = write!(self.out, " {name}=\"{}\"", attr(&iri));
            }
        }
        self.out.push_str(">\n");
        for i in doc.top() {
            if Some(i) == doc.ontology_iri || Some(i) == doc.version_iri {
                continue;
            }
            match doc.nodes[i].kind {
                NodeKind::Construct(Keyword::Import) => {
                    let c = doc.children(i).next().expect("an import's IRI");
                    let iri = self.iri(c);
                    let _ = writeln!(self.out, "<Import>{}</Import>", content(&iri));
                }
                NodeKind::Construct(k) => {
                    self.construct(k, i);
                    self.out.push('\n');
                }
                kind => panic!("a top-level {kind:?}"),
            }
        }
        self.out.push_str("</Ontology>\n");
    }

    /// The full IRI of IRI node `i`.
    fn iri(&self, i: usize) -> String {
        let n = self.doc.nodes[i];
        let NodeKind::Iri { prefix_len } = n.kind else {
            panic!("node {i} is no IRI: {:?}", n.kind)
        };
        let written = self.at(n.span);
        let Some(len) = prefix_len else {
            return written[1..written.len() - 1].to_string();
        };
        let (name, local) = (&written[..len as usize], &written[len as usize + 1..]);
        // As the mapping binds it: the first declaration, from where it is.
        let declared = self.doc.prefixes.iter().find(|d| d.name == name);
        let prefix = match declared {
            Some(d) if d.span.start < n.span.start => d.iri.as_str(),
            _ => STANDARD
                .iter()
                .find(|s| s.0 == name)
                .map(|s| s.1)
                .expect("a bound prefix"),
        };
        format!("{prefix}{local}")
    }

    /// Construct `k` at node `i`.
    fn construct(&mut self, k: Keyword, i: usize) {
        let doc = self.doc;
        let kids: Vec<usize> = doc.children(i).collect();
        if let Some(e) = Entity::of(k) {
            // Inside `Declaration`.
            let iri = self.iri(kids[0]);
            self.entity(e, &iri);
            return;
        }
        if k == Keyword::Variable {
            let iri = self.iri(kids[0]);
            let _ = write!(self.out, "<Variable IRI=\"{}\"/>", attr(&iri));
            return;
        }
        let _ = write!(self.out, "<{}", k.as_str());
        let mut rest = &kids[..];
        let mut slots = signature(k);
        if let Some((Cat::Integer, _)) = slots.first() {
            let _ = write!(
                self.out,
                " cardinality=\"{}\"",
                self.at(doc.nodes[rest[0]].span)
            );
            rest = &rest[1..];
        } else if k == Keyword::BuiltInAtom {
            let _ = write!(self.out, " IRI=\"{}\"", attr(&self.iri(rest[0])));
            (rest, slots) = (&rest[1..], &slots[1..]);
        }
        self.out.push('>');
        for &(cat, rep) in slots {
            let n = match (cat, rep) {
                (Cat::Integer, _) => 0,
                (Cat::Annotation, _) => rest
                    .iter()
                    .take_while(|&&c| doc.nodes[c].kind == NodeKind::Construct(Keyword::Annotation))
                    .count(),
                (Cat::DataPropertiesThenRange | Cat::Facets, _) | (_, Rep::AtLeast(_)) => {
                    rest.len()
                }
                (_, Rep::Optional) => rest.len().min(1),
                (_, Rep::One) => 1,
            };
            let (these, after) = rest.split_at(n);
            rest = after;
            match cat {
                Cat::Facets => {
                    for pair in these.chunks(2) {
                        let facet = self.iri(pair[0]);
                        let _ = write!(self.out, "<FacetRestriction facet=\"{}\">", attr(&facet));
                        self.node(pair[1], Place::In(Cat::Literal));
                        self.out.push_str("</FacetRestriction>");
                    }
                }
                Cat::DataPropertiesThenRange => {
                    let (&range, properties) = these.split_last().expect("a data range");
                    for &c in properties {
                        self.node(c, Place::In(Cat::DataProperty));
                    }
                    self.node(range, Place::Range);
                }
                Cat::ObjectKeys | Cat::DataKeys => {
                    let each = if cat == Cat::ObjectKeys {
                        Cat::ObjectProperty
                    } else {
                        Cat::DataProperty
                    };
                    for c in doc.children(these[0]) {
                        self.node(c, Place::In(each));
                    }
                }
                _ => {
                    for &c in these {
                        self.node(c, Place::In(cat));
                    }
                }
            }
        }
        assert!(rest.is_empty(), "{k:?} has more than its signature");
        let _ = write!(self.out, "</{}>", k.as_str());
    }

    fn entity(&mut self, e: Entity, iri: &str) {
        let name = match e {
            Entity::Class => "Class",
            Entity::Datatype => "Datatype",
            Entity::ObjectProperty => "ObjectProperty",
            Entity::DataProperty => "DataProperty",
            Entity::AnnotationProperty => "AnnotationProperty",
            Entity::NamedIndividual => "NamedIndividual",
        };
        let _ = write!(self.out, "<{name} IRI=\"{}\"/>", attr(iri));
    }

    /// Node `i`, in `place`.
    fn node(&mut self, i: usize, place: Place) {
        let doc = self.doc;
        let n = doc.nodes[i];
        match n.kind {
            NodeKind::Construct(k) => self.construct(k, i),
            NodeKind::Iri { .. } => {
                let iri = self.iri(i);
                let entity = match place {
                    Place::Range => Some(Entity::Datatype),
                    Place::In(cat) => cat.entity(),
                };
                match entity {
                    Some(e) => self.entity(e, &iri),
                    None => {
                        let _ = write!(self.out, "<IRI>{}</IRI>", content(&iri));
                    }
                }
            }
            NodeKind::NodeId => {
                let next = self.labels.len();
                let label = *self.labels.entry(self.at(n.span)).or_insert(next);
                let _ = write!(self.out, "<AnonymousIndividual nodeID=\"n{label}\"/>");
            }
            NodeKind::Literal { string_end } => {
                let raw = &self.text[n.span.start as usize + 1..string_end as usize - 1];
                let mut lexical = String::with_capacity(raw.len());
                let mut chars = raw.chars();
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => lexical.extend(chars.next()),
                        c => lexical.push(c),
                    }
                }
                self.out.push_str("<Literal");
                match doc.children(i).next() {
                    Some(d) => {
                        let iri = self.iri(d);
                        let _ = write!(self.out, " datatypeIRI=\"{}\"", attr(&iri));
                    }
                    None => {
                        let after = &self.text[string_end as usize..n.span.end as usize];
                        if let Some(at) = after.rfind('@') {
                            let _ = write!(self.out, " xml:lang=\"{}\"", attr(&after[at + 1..]));
                        }
                    }
                }
                let _ = write!(self.out, ">{}</Literal>", content(&lexical));
            }
            kind => panic!("node {i} is a {kind:?} in a slot"),
        }
    }
}

/// `s` in an attribute value: what the attribute-value normalisation of
/// XML would change written as a character reference.
fn attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

/// `s` as element content: a carriage return as a reference, which the
/// end-of-line handling of XML would change.
fn content(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::print;
    use crate::ofn::signature;
    use crate::ofn::tree::Document;
    use crate::owx::reader;
    use crate::owx::tests::{functional, same_mapping, shape};

    /// The Functional text `fs`, printed as OWL/XML, reads with no error
    /// to a tree of the same shape that fills its signatures, and to the
    /// same graph: `Err` says how it differs.
    fn round_trip(fs: &str) -> Result<String, String> {
        let tree = crate::ofn::tree(fs);
        if !tree.errors.is_empty() {
            return Err(format!("the Functional text has errors: {:?}", tree.errors));
        }
        let xml = print(fs, &tree);
        let (doc, _) = reader::read(&xml, None);
        if !doc.errors.is_empty() {
            return Err(format!("errors: {:?}", doc.errors));
        }
        signature::check(&doc).map_err(|m| format!("{m:?}"))?;
        let header = |d: &Document| (d.ontology_iri, d.version_iri, d.header_broken);
        if shape(&doc) != shape(&tree) || header(&doc) != header(&tree) {
            return Err("another tree shape".into());
        }
        if !doc.broken.is_empty() {
            return Err(format!("broken items: {:?}", doc.broken));
        }
        let ours = crate::owx::tests::read(&xml, None);
        let theirs = functional(fs);
        let same = std::panic::catch_unwind(|| same_mapping(&ours, &theirs));
        same.map_err(|_| "another graph".to_string())?;
        Ok(xml)
    }

    /// Each of the 78 approved W3C Functional texts, printed as OWL/XML,
    /// reads back to its tree's shape and its graph. None has a SWRL rule.
    #[test]
    fn the_w3c_functional_texts_read_the_same_as_owl_xml() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/owl2/fs");
        let mut names: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        names.sort();
        let mut faults = Vec::new();
        let mut same = 0;
        for path in &names {
            let fs = std::fs::read_to_string(path).unwrap();
            assert!(!fs.contains("DLSafeRule"), "{}", path.display());
            match round_trip(&fs) {
                Ok(_) => same += 1,
                Err(why) => faults.push(format!("{}: {why}", path.display())),
            }
        }
        assert!(faults.is_empty(), "{}", faults.join("\n"));
        assert_eq!((same, names.len()), (78, 78));
    }

    /// A Functional text with a SWRL rule of each atom kind, printed as
    /// OWL API writes rules, reads back to its tree's shape and its graph.
    #[test]
    fn a_text_with_rules_reads_the_same_as_owl_xml() {
        let fs = include_str!("../../tests/owx/forms.ofn");
        let xml = round_trip(fs).unwrap_or_else(|why| panic!("{why}"));
        assert!(xml.contains("<BuiltInAtom IRI=\"http://www.w3.org/2003/11/swrlb#greaterThan\">"));
        assert!(xml.contains("<Variable IRI=\"http://example.org/var#x\"/>"));
    }

    /// The global elements of the schema (the Recommendation, section 3.4),
    /// and the two it declares locally, `FacetRestriction` and
    /// `ObjectPropertyChain`.
    const SCHEMA: [&str; 76] = [
        "Prefix",
        "Import",
        "Ontology",
        "Class",
        "Datatype",
        "ObjectProperty",
        "DataProperty",
        "AnnotationProperty",
        "NamedIndividual",
        "AnonymousIndividual",
        "Literal",
        "Declaration",
        "ObjectInverseOf",
        "DataIntersectionOf",
        "DataUnionOf",
        "DataComplementOf",
        "DataOneOf",
        "DatatypeRestriction",
        "ObjectIntersectionOf",
        "ObjectUnionOf",
        "ObjectComplementOf",
        "ObjectOneOf",
        "ObjectSomeValuesFrom",
        "ObjectAllValuesFrom",
        "ObjectHasValue",
        "ObjectHasSelf",
        "ObjectMinCardinality",
        "ObjectMaxCardinality",
        "ObjectExactCardinality",
        "DataSomeValuesFrom",
        "DataAllValuesFrom",
        "DataHasValue",
        "DataMinCardinality",
        "DataMaxCardinality",
        "DataExactCardinality",
        "SubClassOf",
        "EquivalentClasses",
        "DisjointClasses",
        "DisjointUnion",
        "SubObjectPropertyOf",
        "EquivalentObjectProperties",
        "DisjointObjectProperties",
        "ObjectPropertyDomain",
        "ObjectPropertyRange",
        "InverseObjectProperties",
        "FunctionalObjectProperty",
        "InverseFunctionalObjectProperty",
        "ReflexiveObjectProperty",
        "IrreflexiveObjectProperty",
        "SymmetricObjectProperty",
        "AsymmetricObjectProperty",
        "TransitiveObjectProperty",
        "SubDataPropertyOf",
        "EquivalentDataProperties",
        "DisjointDataProperties",
        "DataPropertyDomain",
        "DataPropertyRange",
        "FunctionalDataProperty",
        "DatatypeDefinition",
        "HasKey",
        "SameIndividual",
        "DifferentIndividuals",
        "ClassAssertion",
        "ObjectPropertyAssertion",
        "NegativeObjectPropertyAssertion",
        "DataPropertyAssertion",
        "NegativeDataPropertyAssertion",
        "IRI",
        "AbbreviatedIRI",
        "Annotation",
        "AnnotationAssertion",
        "SubAnnotationPropertyOf",
        "AnnotationPropertyDomain",
        "AnnotationPropertyRange",
        "FacetRestriction",
        "ObjectPropertyChain",
    ];

    /// One Functional text with every construct of the schema, each with an
    /// annotation where it is an axiom, and one annotation annotated.
    const EVERY: &str = r#"Prefix(:=<http://example.org/o#>)
Ontology(<http://example.org/o> <http://example.org/o/1>
Import(<http://example.org/other>)
Annotation(rdfs:comment "an ontology")
Declaration(Annotation(rdfs:comment "c") Class(:C))
Declaration(Datatype(:dt))
Declaration(ObjectProperty(:p))
Declaration(DataProperty(:d))
Declaration(AnnotationProperty(:a))
Declaration(NamedIndividual(:x))
SubClassOf(Annotation(Annotation(:a "on an annotation") rdfs:comment "c") ObjectIntersectionOf(:C :D) ObjectUnionOf(:C ObjectComplementOf(:D)))
SubClassOf(ObjectOneOf(:x _:b) ObjectSomeValuesFrom(ObjectInverseOf(:p) :C))
SubClassOf(ObjectAllValuesFrom(:p :C) ObjectHasValue(:p _:b))
SubClassOf(ObjectHasSelf(:p) ObjectMinCardinality(1 :p))
SubClassOf(ObjectMaxCardinality(2 :p :C) ObjectExactCardinality(3 :p))
SubClassOf(DataSomeValuesFrom(:d :e DataIntersectionOf(xsd:integer DataComplementOf(xsd:string))) DataAllValuesFrom(:d DataUnionOf(xsd:integer DataOneOf("a" "b"@en))))
SubClassOf(DataHasValue(:d "1"^^xsd:integer) DataMinCardinality(0 :d))
SubClassOf(DataMaxCardinality(1 :d xsd:integer) DataExactCardinality(2 :d DatatypeRestriction(xsd:integer xsd:minInclusive "0"^^xsd:integer xsd:maxExclusive "9"^^xsd:integer)))
EquivalentClasses(Annotation(rdfs:comment "c") :C :D :E)
DisjointClasses(Annotation(rdfs:comment "c") :C :D)
DisjointUnion(Annotation(rdfs:comment "c") :C :D :E)
SubObjectPropertyOf(Annotation(rdfs:comment "c") ObjectPropertyChain(:p ObjectInverseOf(:q)) :r)
SubObjectPropertyOf(:p :q)
EquivalentObjectProperties(Annotation(rdfs:comment "c") :p :q)
DisjointObjectProperties(Annotation(rdfs:comment "c") :p :q :r)
InverseObjectProperties(Annotation(rdfs:comment "c") :p ObjectInverseOf(:q))
ObjectPropertyDomain(Annotation(rdfs:comment "c") :p :C)
ObjectPropertyRange(Annotation(rdfs:comment "c") :p :C)
FunctionalObjectProperty(Annotation(rdfs:comment "c") :p)
InverseFunctionalObjectProperty(Annotation(rdfs:comment "c") :p)
ReflexiveObjectProperty(Annotation(rdfs:comment "c") :p)
IrreflexiveObjectProperty(Annotation(rdfs:comment "c") :p)
SymmetricObjectProperty(Annotation(rdfs:comment "c") :p)
AsymmetricObjectProperty(Annotation(rdfs:comment "c") :p)
TransitiveObjectProperty(Annotation(rdfs:comment "c") :p)
SubDataPropertyOf(Annotation(rdfs:comment "c") :d :e)
EquivalentDataProperties(Annotation(rdfs:comment "c") :d :e)
DisjointDataProperties(Annotation(rdfs:comment "c") :d :e :f)
DataPropertyDomain(Annotation(rdfs:comment "c") :d :C)
DataPropertyRange(Annotation(rdfs:comment "c") :d xsd:integer)
FunctionalDataProperty(Annotation(rdfs:comment "c") :d)
DatatypeDefinition(Annotation(rdfs:comment "c") :dt DataUnionOf(xsd:integer xsd:string))
HasKey(Annotation(rdfs:comment "c") :C (:p ObjectInverseOf(:q)) (:d))
HasKey(:C () (:d))
HasKey(:C (:p) ())
SameIndividual(Annotation(rdfs:comment "c") :x :y _:b)
DifferentIndividuals(Annotation(rdfs:comment "c") :x :y)
ClassAssertion(Annotation(rdfs:comment "c") ObjectComplementOf(:C) _:b)
ObjectPropertyAssertion(Annotation(rdfs:comment "c") ObjectInverseOf(:p) :x _:b)
NegativeObjectPropertyAssertion(Annotation(rdfs:comment "c") :p _:b :y)
DataPropertyAssertion(Annotation(rdfs:comment "c") :d _:b "v")
NegativeDataPropertyAssertion(Annotation(rdfs:comment "c") :d :x "w"@en)
AnnotationAssertion(Annotation(rdfs:comment "c") :a _:b <http://example.org/o#v>)
SubAnnotationPropertyOf(Annotation(rdfs:comment "c") :a rdfs:label)
AnnotationPropertyDomain(Annotation(rdfs:comment "c") :a :C)
AnnotationPropertyRange(Annotation(rdfs:comment "c") :a :C)
)
"#;

    /// Every element of the schema is read by a test: the printed text of
    /// [`EVERY`], and a text with the elements the printer writes no other
    /// way (`Prefix`, `AbbreviatedIRI`), each start tag found and the
    /// whole read with no error to the Functional graph.
    #[test]
    fn every_keyword_of_the_schema_is_read() {
        let printed = round_trip(EVERY).unwrap_or_else(|why| panic!("{why}"));
        let abbreviated = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n<Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"ex:a\"/><AbbreviatedIRI>ex:s</AbbreviatedIRI><Literal>v</Literal></AnnotationAssertion>\n</Ontology>\n";
        let p = crate::owx::tests::read(abbreviated, None);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        same_mapping(
            &p,
            &functional(
                "Prefix(ex:=<http://example.org/>)\nOntology(\nAnnotationAssertion(ex:a ex:s \"v\")\n)\n",
            ),
        );
        let missing: Vec<&str> = SCHEMA
            .iter()
            .copied()
            .filter(|name| {
                let tag = |end: &str| format!("<{name}{end}");
                ![&printed, abbreviated]
                    .iter()
                    .any(|t| [" ", ">", "/"].iter().any(|e| t.contains(&tag(e))))
            })
            .collect();
        assert_eq!(missing, Vec::<&str>::new());
        // A name the reader does not know fails the read.
        let unknown = printed
            .replace("<HasKey>", "<HasKeys>")
            .replace("</HasKey>", "</HasKeys>");
        let (doc, _) = reader::read(&unknown, None);
        assert!(
            doc.errors
                .iter()
                .any(|e| e.message() == "<HasKeys> is no element of OWL/XML"),
            "{:?}",
            doc.errors
        );
        assert_eq!(SCHEMA.len(), 74 + 2);
    }
}
