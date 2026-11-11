//! The bundled vocabularies (`data/vocab.tsv`, made by
//! `tools/build-vocab.mjs`) against the specifications: every term of the
//! core W3C vocabularies is known, and the W3C test suites under `tests/w3c`
//! get no `unknown-term` but the listed ones.
//!
//!     cargo test --release --test vocabulary -- --nocapture

use lsp_types::NumberOrString;
use std::path::Path;
use turtle_lsp::batch::{self, BatchFile, Options};
use turtle_lsp::settings::Settings;

/// RDF 1.2 Schema, A.1: the `rdf:` registry, less the four names reserved
/// for RDF-INTEROP (`rdf:PropositionForm` and its three properties), and one
/// container membership property (5.1.6 of RDF Schema 1.1, 7.1.5 of 1.2).
const RDF: &[&str] = &[
    "Alt",
    "Bag",
    "Seq",
    "HTML",
    "JSON",
    "XMLLiteral",
    "List",
    "Property",
    "Statement",
    "nil",
    "rest",
    "dirLangString",
    "first",
    "langString",
    "object",
    "predicate",
    "subject",
    "reifies",
    "type",
    "value",
    "CompoundLiteral",
    "direction",
    "language",
    "PlainLiteral",
    "langRange",
    "_1",
    "_2",
    "_12",
];

/// RDF 1.2 Schema, A.2: the `rdfs:` registry.
const RDFS: &[&str] = &[
    "Class",
    "Container",
    "ContainerMembershipProperty",
    "Datatype",
    "Literal",
    "Proposition",
    "Resource",
    "comment",
    "domain",
    "isDefinedBy",
    "label",
    "member",
    "range",
    "seeAlso",
    "subClassOf",
    "subPropertyOf",
];

/// The `owl:` names of OWL 2 Mapping to RDF Graphs (its tables, with the
/// OWL 1 vocabulary that 3.1 and 3.2 still parse) and of the OWL 2
/// Structural Specification (the reserved vocabulary of 2.4 and the datatype
/// map of 4), less the four that only their change logs name
/// (`owl:dateTime`, `owl:subject`, `owl:predicate`, `owl:object`).
const OWL: &[&str] = &[
    "AllDifferent",
    "AllDisjointClasses",
    "AllDisjointProperties",
    "Annotation",
    "AnnotationProperty",
    "AsymmetricProperty",
    "Axiom",
    "Class",
    "DataRange",
    "DatatypeProperty",
    "DeprecatedClass",
    "DeprecatedProperty",
    "FunctionalProperty",
    "InverseFunctionalProperty",
    "IrreflexiveProperty",
    "NamedIndividual",
    "NegativePropertyAssertion",
    "Nothing",
    "ObjectProperty",
    "Ontology",
    "OntologyProperty",
    "ReflexiveProperty",
    "Restriction",
    "SymmetricProperty",
    "Thing",
    "TransitiveProperty",
    "allValuesFrom",
    "annotatedProperty",
    "annotatedSource",
    "annotatedTarget",
    "assertionProperty",
    "backwardCompatibleWith",
    "bottomDataProperty",
    "bottomObjectProperty",
    "cardinality",
    "complementOf",
    "datatypeComplementOf",
    "deprecated",
    "differentFrom",
    "disjointUnionOf",
    "disjointWith",
    "distinctMembers",
    "equivalentClass",
    "equivalentProperty",
    "hasKey",
    "hasSelf",
    "hasValue",
    "imports",
    "incompatibleWith",
    "intersectionOf",
    "inverseOf",
    "maxCardinality",
    "maxQualifiedCardinality",
    "members",
    "minCardinality",
    "minQualifiedCardinality",
    "onClass",
    "onDataRange",
    "onDatatype",
    "onProperties",
    "onProperty",
    "oneOf",
    "priorVersion",
    "propertyChainAxiom",
    "propertyDisjointWith",
    "qualifiedCardinality",
    "rational",
    "real",
    "sameAs",
    "someValuesFrom",
    "sourceIndividual",
    "targetIndividual",
    "targetValue",
    "topDataProperty",
    "topObjectProperty",
    "unionOf",
    "versionIRI",
    "versionInfo",
    "withRestrictions",
];

/// XML Schema 1.1 Part 2: the built-in datatypes of section 3 and the 14
/// constraining facets of 4.3, whose IRIs section 3 gives.
const XSD: &[&str] = &[
    "anySimpleType",
    "anyAtomicType",
    "string",
    "boolean",
    "decimal",
    "float",
    "double",
    "duration",
    "dateTime",
    "time",
    "date",
    "gYearMonth",
    "gYear",
    "gMonthDay",
    "gDay",
    "gMonth",
    "hexBinary",
    "base64Binary",
    "anyURI",
    "QName",
    "NOTATION",
    "normalizedString",
    "token",
    "language",
    "NMTOKEN",
    "NMTOKENS",
    "Name",
    "NCName",
    "ID",
    "IDREF",
    "IDREFS",
    "ENTITY",
    "ENTITIES",
    "integer",
    "nonPositiveInteger",
    "negativeInteger",
    "long",
    "int",
    "short",
    "byte",
    "nonNegativeInteger",
    "unsignedLong",
    "unsignedInt",
    "unsignedShort",
    "unsignedByte",
    "positiveInteger",
    "yearMonthDuration",
    "dayTimeDuration",
    "dateTimeStamp",
    "length",
    "minLength",
    "maxLength",
    "pattern",
    "enumeration",
    "whiteSpace",
    "maxInclusive",
    "maxExclusive",
    "minExclusive",
    "minInclusive",
    "totalDigits",
    "fractionDigits",
    "assertion",
    "explicitTimezone",
];

/// SHACL 1.0 (W3C Recommendation, 2017): the terms the Core and
/// SHACL-SPARQL sections name, and the SHACL JavaScript Extensions note.
/// Three names in the Recommendation's text are slips its own vocabulary
/// does not have, and are left out: `sh:PropertyShapeComponent` in 4.7.2
/// (the component is `sh:PropertyConstraintComponent`, as 4.7.2's table
/// says), and `sh:RegexConstraintComponent` and `sh:regex` in the example
/// of 1.4 (`sh:PatternConstraintComponent`, `sh:pattern`).
const SH: &[&str] = &[
    "AbstractResult",
    "AndConstraintComponent",
    "BlankNode",
    "BlankNodeOrIRI",
    "BlankNodeOrLiteral",
    "ClassConstraintComponent",
    "ClosedConstraintComponent",
    "ConstraintComponent",
    "DatatypeConstraintComponent",
    "DisjointConstraintComponent",
    "EqualsConstraintComponent",
    "HasValueConstraintComponent",
    "IRI",
    "IRIOrLiteral",
    "InConstraintComponent",
    "Info",
    "JSConstraint",
    "JSConstraintComponent",
    "JSExecutable",
    "JSFunction",
    "JSLibrary",
    "JSRule",
    "JSTarget",
    "JSTargetType",
    "JSValidator",
    "LanguageInConstraintComponent",
    "LessThanConstraintComponent",
    "LessThanOrEqualsConstraintComponent",
    "Literal",
    "MaxCountConstraintComponent",
    "MaxExclusiveConstraintComponent",
    "MaxInclusiveConstraintComponent",
    "MaxLengthConstraintComponent",
    "MinCountConstraintComponent",
    "MinExclusiveConstraintComponent",
    "MinInclusiveConstraintComponent",
    "MinLengthConstraintComponent",
    "NodeConstraintComponent",
    "NodeKind",
    "NodeKindConstraintComponent",
    "NodeShape",
    "NotConstraintComponent",
    "OrConstraintComponent",
    "Parameter",
    "PatternConstraintComponent",
    "PrefixDeclaration",
    "PropertyConstraintComponent",
    "PropertyGroup",
    "PropertyShape",
    "QualifiedMaxCountConstraintComponent",
    "QualifiedMinCountConstraintComponent",
    "SPARQLAskValidator",
    "SPARQLConstraint",
    "SPARQLConstraintComponent",
    "SPARQLFunction",
    "SPARQLSelectValidator",
    "Severity",
    "Shape",
    "TargetType",
    "UniqueLangConstraintComponent",
    "ValidationReport",
    "ValidationResult",
    "Violation",
    "Warning",
    "XoneConstraintComponent",
    "alternativePath",
    "and",
    "ask",
    "class",
    "closed",
    "conforms",
    "datatype",
    "deactivated",
    "declare",
    "defaultValue",
    "description",
    "detail",
    "disjoint",
    "entailment",
    "equals",
    "flags",
    "focusNode",
    "group",
    "hasValue",
    "ignoredProperties",
    "in",
    "inversePath",
    "js",
    "jsFunctionName",
    "jsLibrary",
    "jsLibraryURL",
    "labelTemplate",
    "languageIn",
    "lessThan",
    "lessThanOrEquals",
    "maxCount",
    "maxExclusive",
    "maxInclusive",
    "maxLength",
    "message",
    "minCount",
    "minExclusive",
    "minInclusive",
    "minLength",
    "name",
    "namespace",
    "node",
    "nodeKind",
    "nodeValidator",
    "not",
    "oneOrMorePath",
    "optional",
    "or",
    "order",
    "parameter",
    "path",
    "pattern",
    "prefix",
    "prefixes",
    "property",
    "propertyValidator",
    "qualifiedMaxCount",
    "qualifiedMinCount",
    "qualifiedValueShape",
    "qualifiedValueShapesDisjoint",
    "result",
    "resultMessage",
    "resultPath",
    "resultSeverity",
    "returnType",
    "rule",
    "select",
    "severity",
    "shapesGraph",
    "shapesGraphWellFormed",
    "sourceConstraint",
    "sourceConstraintComponent",
    "sourceShape",
    "sparql",
    "targetClass",
    "targetNode",
    "targetObjectsOf",
    "targetSubjectsOf",
    "uniqueLang",
    "validator",
    "value",
    "xone",
    "zeroOrMorePath",
    "zeroOrOnePath",
];

/// SKOS Reference, appendix B (the SKOS overview).
const SKOS: &[&str] = &[
    "Concept",
    "ConceptScheme",
    "Collection",
    "OrderedCollection",
    "inScheme",
    "hasTopConcept",
    "topConceptOf",
    "prefLabel",
    "altLabel",
    "hiddenLabel",
    "notation",
    "note",
    "changeNote",
    "definition",
    "editorialNote",
    "example",
    "historyNote",
    "scopeNote",
    "semanticRelation",
    "broader",
    "narrower",
    "related",
    "broaderTransitive",
    "narrowerTransitive",
    "member",
    "memberList",
    "mappingRelation",
    "closeMatch",
    "exactMatch",
    "broadMatch",
    "narrowMatch",
    "relatedMatch",
];

/// Each `unknown-term` that `batch::analyse` reports for `files`, analysed
/// as one workspace: (file, the term as `prefix:local`). The term is read
/// from the message, which names the vocabulary and the local name.
fn unknown_terms(files: &[BatchFile]) -> Vec<(String, String)> {
    let opts = Options {
        shacl: false,
        fixes: false,
        verdict: false,
        ..Options::default()
    };
    let report = batch::analyse(files, &Settings::default(), &opts);
    let mut out = Vec::new();
    for f in &report.files {
        for p in &f.problems {
            let d = &p.diagnostic;
            if d.code != Some(NumberOrString::String("unknown-term".into())) {
                continue;
            }
            let (local, rest) = d.message[1..]
                .split_once("' is not defined in the ")
                .unwrap_or_else(|| panic!("{}: {}", f.name, d.message));
            let prefix = rest.split_once(' ').unwrap().0;
            out.push((f.name.clone(), format!("{prefix}:{local}")));
        }
    }
    out
}

#[test]
fn every_core_w3c_term_is_known() {
    let vocabularies = [
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#", RDF),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#", RDFS),
        ("owl", "http://www.w3.org/2002/07/owl#", OWL),
        ("xsd", "http://www.w3.org/2001/XMLSchema#", XSD),
        ("sh", "http://www.w3.org/ns/shacl#", SH),
        ("skos", "http://www.w3.org/2004/02/skos/core#", SKOS),
    ];
    let mut text = String::from("@prefix ex: <http://example.org/> .\n");
    for (prefix, namespace, _) in vocabularies {
        text.push_str(&format!("@prefix {prefix}: <{namespace}> .\n"));
    }
    for (prefix, _, terms) in vocabularies {
        for local in terms {
            text.push_str(&format!("ex:s ex:uses {prefix}:{local} .\n"));
        }
    }
    let files = [BatchFile::named("core.ttl", text, false)];
    assert_eq!(unknown_terms(&files), [] as [(String, String); 0]);
}

/// What the W3C suites are expected to get: (the term, how many uses, why).
/// Every one is a mistake or an invention of the suite's own files.
const W3C_EXPECTED: &[(&str, usize, &str)] = &[
    (
        "rdfs:XMLLiteral",
        18,
        "nt-syntax-subm-01 (RDF 1.1 N-Triples and N-Quads) types literals with the rdfs spelling; the datatype is rdf:XMLLiteral",
    ),
    (
        "dcterms:licence",
        11,
        "the RDF 1.2 and SPARQL 1.2 manifests spell dcterms:license the British way",
    ),
    (
        "rdf:Graph",
        6,
        "JSON-LD toRdf 0027 and 0028 type named graphs rdf:Graph, which RDF does not define",
    ),
    (
        "schema:term",
        4,
        "JSON-LD expand 0118 and toRdf e118 expand a made-up key with @vocab http://schema.org/",
    ),
    ("schema::fish", 4, "as schema:term: the key ':fish'"),
    (
        "dcterms:published",
        4,
        "JSON-LD c038 (expand and toRdf): Dublin Core has issued and publisher, not published",
    ),
    (
        "rdfs:definedBy",
        2,
        "JSON-LD toRdf 0031: the property is rdfs:isDefinedBy",
    ),
    (
        "sh:nodeShape",
        1,
        "SHACL core/property/qualifiedValueShape-001 uses sh:nodeShape, which SHACL 1.0 does not define",
    ),
    (
        "og:audio%3Atitle",
        1,
        "SPARQL 1.1 qname-escape-02: a percent escape in a local name stays in the IRI, so the term is og:audio%3Atitle, not og:audio:title",
    ),
];

/// The files `check --root tests/w3c` reads, as it finds them: RDF in any
/// letter case, SPARQL only in lower case, no hidden folder.
fn w3c_files(dir: &Path, root: &Path, out: &mut Vec<BatchFile>) {
    const RDF_EXTENSIONS: &[&str] = &["ttl", "turtle", "trig", "nt", "nq", "jsonld"];
    const SPARQL_EXTENSIONS: &[&str] = &["rq", "sparql", "ru"];
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') {
                w3c_files(&path, root, out);
            }
            continue;
        }
        let ext = path.extension().map(|e| e.to_string_lossy().to_string());
        let wanted = ext.is_some_and(|e| {
            RDF_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                || SPARQL_EXTENSIONS.contains(&e.as_str())
        });
        // A file that is not UTF-8 is an `encoding` error to `check`.
        if let (true, Ok(text)) = (wanted, std::fs::read_to_string(&path)) {
            let relative = path.strip_prefix(root).unwrap().to_string_lossy();
            out.push(BatchFile::named(&relative.replace('\\', "/"), text, false));
        }
    }
}

#[test]
fn the_w3c_suites_get_only_the_listed_unknown_terms() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c");
    let mut files = Vec::new();
    w3c_files(&root, &root, &mut files);
    let found = unknown_terms(&files);
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for (file, term) in &found {
        eprintln!("{file}: {term}");
        *counts.entry(term).or_default() += 1;
    }
    let unexpected: Vec<_> = counts
        .iter()
        .filter(|(term, n)| !W3C_EXPECTED.iter().any(|e| e.0 == **term && e.1 == **n))
        .collect();
    let missing: Vec<_> = W3C_EXPECTED
        .iter()
        .filter(|e| counts.get(e.0) != Some(&e.1))
        .map(|e| (e.0, e.1))
        .collect();
    assert!(
        unexpected.is_empty() && missing.is_empty(),
        "found, not as listed: {unexpected:?}\nlisted, not found: {missing:?}"
    );
}
