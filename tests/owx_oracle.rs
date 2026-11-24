//! The OWL/XML frontend against texts it did not write: the examples of
//! the W3C OWL 2 Primer (`tests/w3c/owl2-primer/`), each given there in
//! OWL/XML and in Functional syntax, and its appendix ontology also in
//! RDF/XML and Turtle. Graphs are compared with blank nodes renamed
//! (`tests/support/oracle.rs`).
//!
//! The texts are vendored verbatim. Two things are done to them here, both
//! listed and checked:
//! - the Primer's HTML writes a space before `:` or `;` as `&nbsp;`, so its
//!   Functional and Turtle texts hold U+00A0, which neither grammar takes as
//!   white space; each is read as a space ([`as_read`]);
//! - where two texts are not the same ontology, the edit that makes them so
//!   ([`KNOWN`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use turtle_lsp::analysis::Analysis;
use turtle_lsp::syntax::Syntax;

#[path = "support/oracle.rs"]
mod oracle;

/// The vendored text `file`.
fn verbatim(file: &str) -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/owl2-primer");
    std::fs::read_to_string(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
}

/// `file` as the tests read it, each U+00A0 a space, and how many there
/// were. Each is followed by `:` or `;` (the HTML's `&nbsp;` before them),
/// and the OWL/XML and RDF/XML texts have none.
fn as_read(file: &str) -> (String, usize) {
    let text = verbatim(file);
    let n = text.matches('\u{a0}').count();
    if file.ends_with(".owx") || file.ends_with(".rdf") {
        assert_eq!(n, 0, "{file} holds U+00A0");
    }
    for (i, nbsp) in text.match_indices('\u{a0}') {
        let next = text[i + nbsp.len()..].chars().next();
        assert!(
            matches!(next, Some(':' | ';')),
            "{file}: U+00A0 at byte {i} is followed by {next:?}"
        );
    }
    (text.replace('\u{a0}', " "), n)
}

/// Where two texts of the Primer do not give one graph: the file, the edit
/// to its text (as [`as_read`] gives it) that makes them agree, and why,
/// from the Primer or the specifications. The text edited is the one the
/// other syntaxes of the example disagree with. Each edit is checked to
/// apply once and to be needed: with any one of a file's edits left out,
/// the texts do not agree.
///
/// All but three are faults of the text: it is not the same ontology, or
/// not the graph Table 1 gives for it. The three rows of [`TWICE`] are
/// not: a text writes one axiom twice, which is one axiom of the ontology,
/// and turtle-lsp's graph has two blank-node copies of it, in Functional
/// syntax as in OWL/XML. Whether the mapping should merge them is open.
const KNOWN: [(&str, &str, &str, &str); 35] = [
    (
        "22.owx",
        "<Class IRI=\"Happy\"/>",
        "<Class IRI=\"HappyPerson\"/>",
        "the Primer: \"somebody is a happy person exactly if all their children are happy persons\"; the Functional, RDF/XML and Manchester texts have :HappyPerson",
    ),
    (
        "49.owx",
        ">\"States that every man is a person.\"</Literal>",
        ">States that every man is a person.</Literal>",
        QUOTES,
    ),
    ("50.ofn", "   ...\n", "", ELLIPSIS_OFN),
    (
        "50.owx",
        " <Ontology ...\n   ontologyIRI=\"http://example.com/owl/families\">\n   ...\n",
        " <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"\n   ontologyIRI=\"http://example.com/owl/families\">\n",
        "\"...\" stands for attributes and axioms: it is no attribute (XML [41] Attribute ::= Name Eq AttValue, and '.' is no NameStartChar), and without the elided xmlns the Ontology element is not in OWL's namespace",
    ),
    ("51.ofn", "    ...\n", "", ELLIPSIS_OFN),
    (
        "51.owx",
        "  ...\n",
        "",
        "\"...\" stands for the axioms; the schema's Ontology type is an xsd:sequence of elements (Prefix, Import, annotations, axioms), with no text",
    ),
    ("56.owx", FATHER_MOTHER, MOTHER_FATHER, DISJOINT_ORDER),
    ("57.owx", FATHER_MOTHER, MOTHER_FATHER, DISJOINT_ORDER),
    ("58.owx", FATHER_MOTHER, MOTHER_FATHER, DISJOINT_ORDER),
    ("59.owx", FATHER_MOTHER, MOTHER_FATHER, DISJOINT_ORDER),
    (
        "59.owx",
        ">\"States that every man is a person.\"</Literal>",
        ">States that every man is a person.</Literal>",
        QUOTES,
    ),
    (
        "59.owx",
        "   <Declaration>\n     <NamedIndividual IRI=\"Meg\"/>\n   </Declaration>\n",
        "",
        "the OWL/XML declares :Meg, the Functional, RDF/XML and Turtle texts do not; Table 1: Declaration( NamedIndividual( a ) ) is T(a) rdf:type owl:NamedIndividual",
    ),
    (
        "59.owx",
        concat!(
            "   <SubObjectPropertyOf>\n     <ObjectPropertyChain>\n       <ObjectProperty IRI=\"hasFather\"/>\n       <ObjectProperty IRI=\"hasBrother\"/>\n     </ObjectPropertyChain>\n     <ObjectProperty IRI=\"hasUncle\"/>\n   </SubObjectPropertyOf>\n",
            "   <SubObjectPropertyOf>\n     <ObjectPropertyChain>\n       <ObjectProperty IRI=\"hasFather\"/>\n       <ObjectProperty IRI=\"hasBrother\"/>\n     </ObjectPropertyChain>\n     <ObjectProperty IRI=\"hasUncle\"/>\n   </SubObjectPropertyOf>\n",
        ),
        "   <SubObjectPropertyOf>\n     <ObjectPropertyChain>\n       <ObjectProperty IRI=\"hasFather\"/>\n       <ObjectProperty IRI=\"hasBrother\"/>\n     </ObjectPropertyChain>\n     <ObjectProperty IRI=\"hasUncle\"/>\n   </SubObjectPropertyOf>\n",
        TWICE,
    ),
    (
        "59.owx",
        concat!(
            "   <EquivalentClasses>\n     <Class IRI=\"Parent\"/>\n     <ObjectSomeValuesFrom>\n       <ObjectProperty IRI=\"hasChild\"/>\n       <Class IRI=\"Person\"/>\n     </ObjectSomeValuesFrom>\n   </EquivalentClasses>\n",
            "   <EquivalentClasses>\n     <Class IRI=\"Parent\"/>\n     <ObjectSomeValuesFrom>\n       <ObjectProperty IRI=\"hasChild\"/>\n       <Class IRI=\"Person\"/>\n     </ObjectSomeValuesFrom>\n   </EquivalentClasses>\n",
        ),
        "   <EquivalentClasses>\n     <Class IRI=\"Parent\"/>\n     <ObjectSomeValuesFrom>\n       <ObjectProperty IRI=\"hasChild\"/>\n       <Class IRI=\"Person\"/>\n     </ObjectSomeValuesFrom>\n   </EquivalentClasses>\n",
        TWICE,
    ),
    (
        "59.ofn",
        "   EquivalentClasses(\n     :Parent\n     ObjectSomeValuesFrom(\n       :hasChild\n       :Person\n     )\n   )\n",
        "",
        TWICE,
    ),
    (
        "59.rdf",
        " </rdf:RDF>",
        concat!(
            "   <owl:NamedIndividual rdf:about=\"Bill\"/>\n",
            "   <owl:NamedIndividual rdf:about=\"Jack\"/>\n",
            "   <owl:NamedIndividual rdf:about=\"Jim\"/>\n",
            "   <owl:NamedIndividual rdf:about=\"Mary\"/>\n",
            "   <owl:NamedIndividual rdf:about=\"Susan\"/>\n",
            "   <owl:Class rdf:about=\"Dead\"/>\n",
            "   <owl:Class rdf:about=\"Human\"/>\n",
            "   <owl:Class rdf:about=\"SocialRole\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"hasAncestor\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"hasDaughter\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"hasGrandparent\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"hasHusband\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"hasSpouse\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"hasUncle\"/>\n",
            "   <owl:ObjectProperty rdf:about=\"loves\"/>\n",
            "   <rdfs:Datatype rdf:about=\"majorAge\"/>\n",
            "   <rdfs:Datatype rdf:about=\"minorAge\"/>\n",
            "   <rdfs:Datatype rdf:about=\"personAge\"/>\n",
            "   <rdfs:Datatype rdf:about=\"toddlerAge\"/>\n",
            " </rdf:RDF>",
        ),
        "the Functional and OWL/XML texts declare these 19 entities, the RDF/XML does not; Table 1: a Declaration is T(x) rdf:type the entity's class",
    ),
    (
        "59.rdf",
        "<owl:DataProperty rdf:about=\"hasSSN\"/>",
        "<owl:DatatypeProperty rdf:about=\"hasSSN\"/>",
        DATA_PROPERTY,
    ),
    (
        "59.rdf",
        "<owl:ObjectProperty rdf:about=\"hasFather\">\n     <rdfs:subPropertyOf rdf:resource=\"hasParent\"/>\n   </owl:ObjectProperty>",
        "<rdf:Description rdf:about=\"hasFather\">\n     <rdfs:subPropertyOf rdf:resource=\"hasParent\"/>\n   </rdf:Description>",
        HAS_FATHER,
    ),
    (
        "59.rdf",
        "<owl:ObjectProperty rdf:about=\"hasFather\"/>",
        "<rdf:Description rdf:about=\"hasFather\"/>",
        HAS_FATHER,
    ),
    (
        "59.rdf",
        "<owl:Class rdf:about=\"Grandfather\">\n     <rdfs:subClassOf>\n       <owl:Class>\n         <owl:intersectionOf rdf:parseType=\"Collection\">\n           <owl:Class rdf:about=\"Man\"/>\n           <owl:Class rdf:about=\"Parent\"/>\n         </owl:intersectionOf>\n       </owl:Class>\n     </rdfs:subClassOf>\n   </owl:Class>",
        "<rdf:Description rdf:about=\"Grandfather\">\n     <rdfs:subClassOf>\n       <owl:Class>\n         <owl:intersectionOf rdf:parseType=\"Collection\">\n           <owl:Class rdf:about=\"Man\"/>\n           <owl:Class rdf:about=\"Parent\"/>\n         </owl:intersectionOf>\n       </owl:Class>\n     </rdfs:subClassOf>\n   </rdf:Description>",
        "the RDF/XML types :Grandfather owl:Class, the Functional, OWL/XML and Turtle texts do not declare it",
    ),
    (
        "59.rdf",
        "<owl:ObjectProperty>\n             <owl:inverseOf rdf:resource=\"hasChild\"/>\n           </owl:ObjectProperty>",
        "<rdf:Description>\n             <owl:inverseOf rdf:resource=\"hasChild\"/>\n           </rdf:Description>",
        INVERSE,
    ),
    (
        "59.rdf",
        "<owl:ObjectProperty>\n                     <owl:inverseOf rdf:resource=\"hasParent\"/>\n                   </owl:ObjectProperty>",
        "<rdf:Description>\n                     <owl:inverseOf rdf:resource=\"hasParent\"/>\n                   </rdf:Description>",
        INVERSE,
    ),
    (
        "59.rdf",
        ">\n           true\n         </owl:hasSelf>",
        ">true</owl:hasSelf>",
        WHITE_SPACE,
    ),
    (
        "59.rdf",
        ">\n               1\n             </owl:maxCardinality>",
        ">1</owl:maxCardinality>",
        WHITE_SPACE,
    ),
    (
        "59.rdf",
        ">\n           4\n         </owl:maxQualifiedCardinality>",
        ">4</owl:maxQualifiedCardinality>",
        WHITE_SPACE,
    ),
    (
        "59.rdf",
        ">\n           2\n         </owl:minQualifiedCardinality>",
        ">2</owl:minQualifiedCardinality>",
        WHITE_SPACE,
    ),
    (
        "59.rdf",
        ">\n           3\n         </owl:qualifiedCardinality>",
        ">3</owl:qualifiedCardinality>",
        WHITE_SPACE,
    ),
    (
        "59.rdf",
        ">\n           5\n         </owl:cardinality>",
        ">5</owl:cardinality>",
        WHITE_SPACE,
    ),
    (
        "59.rdf",
        "<owl:AllDisjointClasses>\n     <owl:members rdf:parseType=\"Collection\">\n       <owl:Class rdf:about=\"Woman\"/>\n       <owl:Class rdf:about=\"Man\"/>\n     </owl:members>\n   </owl:AllDisjointClasses>",
        "<rdf:Description rdf:about=\"Woman\">\n     <owl:disjointWith rdf:resource=\"Man\"/>\n   </rdf:Description>",
        DISJOINT_TWO,
    ),
    (
        "59.ttl",
        "\"true\"^^xsd:boolean .\n] .",
        "\"true\"^^xsd:boolean\n] .",
        "Turtle [14]: blankNodePropertyList ::= '[' predicateObjectList ']'; a '.' inside it is an error, so the text does not parse",
    ),
    (
        "59.ttl",
        ":ChildlessPerson      owl:subClassOf  [",
        ":ChildlessPerson      rdfs:subClassOf  [",
        "owl:subClassOf is no OWL term; Table 1: SubClassOf( CE1 CE2 ) is T(CE1) rdfs:subClassOf T(CE2)",
    ),
    (
        "59.ttl",
        "[ owl:complementOf  :Parent ]",
        "[ rdf:type owl:Class ; owl:complementOf  :Parent ]",
        COMPLEMENT,
    ),
    (
        "59.ttl",
        "[ owl:complementOf  [",
        "[ rdf:type owl:Class ; owl:complementOf  [",
        COMPLEMENT,
    ),
    (
        "59.ttl",
        "owl:DataProperty .",
        "owl:DatatypeProperty .",
        DATA_PROPERTY,
    ),
    (
        "59.ttl",
        "[] rdf:type     owl:AllDisjointClasses ;\n   owl:members  ( :Woman  :Man ) .",
        ":Woman owl:disjointWith :Man .",
        DISJOINT_TWO,
    ),
];

const TWICE: &str = "not a fault of the text: it has this axiom twice (the RDF/XML and Turtle once). The axioms of an ontology are a set (Structural Specification, section 2.1: \"By default, all associations are sets; that is, the objects in them are unordered and repetitions are disallowed\"), so as an ontology it is one; but turtle-lsp maps each axiom as written, and a blank node \"is fresh in each application of a mapping rule\" (Mapping, section 2.1), so its graph has the expression twice";
const FATHER_MOTHER: &str = "<Class IRI=\"Father\"/>\n     <Class IRI=\"Mother\"/>";
const MOTHER_FATHER: &str = "<Class IRI=\"Mother\"/>\n     <Class IRI=\"Father\"/>";
const QUOTES: &str = "a Literal element's content is the literal's lexical form (the schema's Literal type is <xsd:extension base=\"xsd:string\">; section 3.1: literals are \"fully defined by their lexical representation\"), so the quotes are part of the string; the Functional, RDF/XML, Turtle and Manchester texts have it without them";
const ELLIPSIS_OFN: &str = "\"...\" stands for the axioms; Functional syntax has no such item (section 3.7: Ontology := 'Ontology' '(' [ ontologyIRI [ versionIRI ] ] directlyImportsDocuments ontologyAnnotations axioms ')')";
const DISJOINT_ORDER: &str = "the OWL/XML has Father before Mother, the Functional, RDF/XML, Turtle and Manchester texts Mother before Father. As axioms they are one (Structural Specification, section 2.1: \"By default, all associations are sets; that is, the objects in them are unordered and repetitions are disallowed\"), but Table 1 maps DisjointClasses( CE1 ... CEn ), n > 2, to owl:members T(SEQ CE1 ... CEn), a list in text order";
const DATA_PROPERTY: &str = "owl:DataProperty is no OWL term; Table 1: Declaration( DataProperty( DP ) ) is T(DP) rdf:type owl:DatatypeProperty";
const HAS_FATHER: &str = "the RDF/XML types :hasFather owl:ObjectProperty, in two places; the Functional, OWL/XML and Turtle texts do not declare it";
const INVERSE: &str = "the RDF/XML types the inverse property's blank node owl:ObjectProperty; Table 1: ObjectInverseOf( OP ) is _:x owl:inverseOf T(OP), and nothing more";
const WHITE_SPACE: &str = "RDF/XML takes the element's content, white space and all, as the lexical form; the other texts write the value alone, and two literals are one only if \"the two lexical forms [...] compare equal, character by character\" (RDF 1.1 Concepts, section 3.3)";
const DISJOINT_TWO: &str = "Table 1 maps DisjointClasses( CE1 CE2 ) to T(CE1) owl:disjointWith T(CE2); the text has owl:AllDisjointClasses with two members, which Table 16 parses to the same axiom (its row for owl:AllDisjointClasses asks n >= 2), but it is another graph";
const COMPLEMENT: &str = "Table 1: ObjectComplementOf( CE ) is _:x rdf:type owl:Class . _:x owl:complementOf T(CE); the Turtle has no type";

/// `file` as read, with its edits of [`KNOWN`] but the row `skip`. Each
/// edit applies once, in the text as read and where it is made.
fn edited(file: &str, skip: Option<usize>) -> String {
    let (read, _) = as_read(file);
    let mut text = read.clone();
    for (i, &(f, from, to, _)) in KNOWN.iter().enumerate() {
        if f != file {
            continue;
        }
        assert_eq!(read.matches(from).count(), 1, "{file}: {from:?}");
        if Some(i) != skip {
            assert_eq!(text.matches(from).count(), 1, "{file}: {from:?}");
            text = text.replacen(from, to, 1);
        }
    }
    text
}

/// The rows of [`KNOWN`] that edit `file`.
fn rows(file: &str) -> Vec<usize> {
    (0..KNOWN.len()).filter(|&i| KNOWN[i].0 == file).collect()
}

/// The graph of `text` read under the name `file`, whose extension gives
/// its syntax; an error if the text has any.
fn graph(file: &str, text: &str) -> Result<oxrdf::Dataset, String> {
    let a = Analysis::new(
        format!("file:///w3c/owl2-primer/{file}"),
        text,
        Arc::default(),
        false,
    );
    let syntax = match file.rsplit('.').next() {
        Some("owx") => Syntax::OwlXml,
        Some("ofn") => Syntax::OwlFunctional,
        Some("rdf") => Syntax::RdfXml,
        Some("ttl") => Syntax::Turtle,
        _ => unreachable!("{file}"),
    };
    assert_eq!(a.parsed.syntax, syntax, "{file}");
    match a.parsed.errors.first() {
        Some(e) => Err(format!(
            "{file}: {} errors, the first {e:?}",
            a.parsed.errors.len()
        )),
        None => Ok(oracle::ours(&a.parsed.triples)),
    }
}

/// `body` inside the header of the appendix ontology `head` (its text up
/// to the first import), closed by `end`. A prefix the header and the
/// body both declare is left out of the header, since a document declares
/// a prefix once; the flag says whether one was. `prefix` gives a header
/// line's prefix name and `declares` whether `body` declares that name.
fn inside(
    head: &str,
    body: &str,
    end: &str,
    prefix: impl Fn(&str) -> Option<&str>,
    declares: impl Fn(&str, &str) -> bool,
) -> (String, bool) {
    let mut dropped = false;
    let mut out = String::new();
    for line in head.split_inclusive('\n') {
        if prefix(line).is_some_and(|name| declares(body, name)) {
            dropped = true;
        } else {
            out.push_str(line);
        }
    }
    out.push_str(body);
    out.push_str(end);
    (out, dropped)
}

/// The two headers: the appendix ontology's Functional and OWL/XML texts
/// up to their one import.
fn headers() -> (String, String) {
    let (ofn, _) = as_read("59.ofn");
    let (owx, _) = as_read("59.owx");
    let cut = |text: &str, import: &str| {
        assert_eq!(text.matches(import).count(), 1, "{import}");
        text[..text.find(import).unwrap()].to_string()
    };
    (cut(&ofn, "   Import("), cut(&owx, "   <Import>"))
}

/// What example `n` gives: its Functional and OWL/XML texts, with the
/// edits of [`KNOWN`] but the row `skip`, each inside its header unless it
/// is a whole document, parsed with no error to isomorphic graphs (the
/// number of triples), else why not. Also whether it is whole and whether
/// a header prefix was left out.
fn example(
    n: usize,
    skip: Option<usize>,
    heads: &(String, String),
) -> (Result<usize, String>, bool, bool) {
    let (ofn_file, owx_file) = (format!("{n:02}.ofn"), format!("{n:02}.owx"));
    let (ofn, owx) = (edited(&ofn_file, skip), edited(&owx_file, skip));
    let whole = ofn.contains("Ontology(");
    assert_eq!(whole, owx.contains("<Ontology"), "example {n}");
    let (ofn, owx, dropped) = if whole {
        (ofn, owx, false)
    } else {
        let (ofn, a) = inside(
            &heads.0,
            &ofn,
            "\n)\n",
            |l| l.trim_start().strip_prefix("Prefix(")?.split(":=").next(),
            |body, name| body.contains(&format!("Prefix({name}:=")),
        );
        let (owx, b) = inside(
            &heads.1,
            &owx,
            "\n</Ontology>\n",
            |l| {
                l.trim_start()
                    .strip_prefix("<Prefix name=\"")?
                    .split('"')
                    .next()
            },
            |body, name| body.contains(&format!("<Prefix name=\"{name}\"")),
        );
        (ofn, owx, a || b)
    };
    let result = (|| {
        let a = graph(&ofn_file, &ofn)?;
        let b = graph(&owx_file, &owx)?;
        if a == b {
            Ok(a.len())
        } else {
            Err(differ(&a, "Functional", &b, "OWL/XML"))
        }
    })();
    (result, whole, dropped)
}

/// Each of the 59 examples of the Primer in OWL/XML gives the graph of the
/// same example in Functional syntax. An example is put inside the header
/// of the appendix ontology in its syntax (its prefixes, `xml:base` and
/// entities), but for the three that are whole documents (50, 51 and the
/// appendix, 59), read as they are. 51 agree as they are (with U+00A0 read
/// as a space); the other 8 agree with the edits of [`KNOWN`] and not
/// without any one of them.
#[test]
fn the_primer_s_examples_agree_between_owl_xml_and_functional() {
    let heads = headers();
    let (mut as_is, mut edited_ok, mut wholes, mut dropped, mut nbsp) =
        (0, 0, Vec::new(), Vec::new(), 0);
    let mut problems = Vec::new();
    for n in 1..=59 {
        nbsp += as_read(&format!("{n:02}.ofn")).1;
        let (result, whole, drop) = example(n, None, &heads);
        if whole {
            wholes.push(n);
        }
        if drop {
            dropped.push(n);
        }
        let mine: Vec<usize> = [format!("{n:02}.ofn"), format!("{n:02}.owx")]
            .iter()
            .flat_map(|f| rows(f))
            .collect();
        match result {
            Ok(triples) if mine.is_empty() => {
                eprintln!("{n:02}: agree, {triples} triples");
                as_is += 1;
            }
            Ok(triples) => {
                eprintln!("{n:02}: agree with {} edits, {triples} triples", mine.len());
                edited_ok += 1;
            }
            Err(e) => problems.push(format!("example {n}: {e}")),
        }
        for &r in &mine {
            if example(n, Some(r), &heads).0.is_ok() {
                problems.push(format!(
                    "example {n} agrees without the edit {:?} of {}: take it out of KNOWN",
                    KNOWN[r].1, KNOWN[r].0
                ));
            }
        }
    }
    eprintln!(
        "W3C OWL 2 Primer, OWL/XML and Functional: {as_is}/59 agree as they are, {edited_ok} with the edits of KNOWN"
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert_eq!((as_is, edited_ok), (51, 8));
    assert_eq!(wholes, [50, 51, 59]);
    // 52 declares the prefix otherOnt, which the header also declares.
    assert_eq!(dropped, [52]);
    assert_eq!(nbsp, 357);
}

/// `MANIFEST.tsv` names each file of the directory but itself and
/// `LICENSE.md`: an OWL/XML and a Functional text for each of the 59
/// examples, and the RDF/XML and Turtle texts of the appendix ontology.
#[test]
fn the_primer_s_manifest_names_every_file() {
    let manifest = verbatim("MANIFEST.tsv");
    let mut lines = manifest.lines();
    assert_eq!(lines.next(), Some("file\texample\tsyntax\tsection"));
    let mut named = Vec::new();
    for line in lines {
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols.len(), 4, "{line}");
        let n: usize = cols[1].parse().unwrap();
        let syntax = match cols[2] {
            "OWL/XML" => "owx",
            "Functional" => "ofn",
            "RDF/XML" => "rdf",
            "Turtle" => "ttl",
            other => panic!("{other}"),
        };
        assert_eq!(cols[0], format!("{n:02}.{syntax}"));
        assert!(!cols[3].is_empty(), "{line}");
        named.push(cols[0].to_string());
    }
    let mut want: Vec<String> = (1..=59)
        .flat_map(|n| [format!("{n:02}.ofn"), format!("{n:02}.owx")])
        .chain(["59.rdf".to_string(), "59.ttl".to_string()])
        .collect();
    want.sort();
    named.sort();
    assert_eq!(named, want);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/owl2-primer");
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .filter(|f| f != "MANIFEST.tsv" && f != "LICENSE.md")
        .collect();
    files.sort();
    assert_eq!(files, want);
}

/// The statements of a graph whose blank nodes form trees (as the OWL
/// mapping's do, with no anonymous individual): each triple with an IRI
/// subject, its blank object written out as what it says, and each blank
/// node no triple names, written out; each with how many times the graph
/// says it (two blank nodes may say the same).
fn statements(d: &oxrdf::Dataset) -> BTreeMap<String, usize> {
    let mut out: HashMap<String, Vec<(String, oxrdf::Term)>> = HashMap::new();
    let mut named = HashSet::new();
    for q in d.iter() {
        out.entry(q.subject.to_string())
            .or_default()
            .push((q.predicate.to_string(), q.object.into_owned()));
        if let oxrdf::TermRef::BlankNode(b) = q.object {
            named.insert(b.to_string());
        }
    }
    fn says(
        node: &str,
        out: &HashMap<String, Vec<(String, oxrdf::Term)>>,
        open: &mut HashSet<String>,
    ) -> String {
        assert!(open.insert(node.to_string()), "a cycle through {node}");
        let mut parts: Vec<String> = out
            .get(node)
            .into_iter()
            .flatten()
            .map(|(p, o)| format!("{p} {}", term(o, out, open)))
            .collect();
        open.remove(node);
        parts.sort();
        format!("[{}]", parts.join("; "))
    }
    fn term(
        t: &oxrdf::Term,
        out: &HashMap<String, Vec<(String, oxrdf::Term)>>,
        open: &mut HashSet<String>,
    ) -> String {
        match t {
            oxrdf::Term::BlankNode(b) => says(&b.to_string(), out, open),
            t => t.to_string(),
        }
    }
    let mut set = BTreeMap::new();
    let mut open = HashSet::new();
    for (s, pos) in &out {
        if !s.starts_with("_:") {
            for (p, o) in pos {
                *set.entry(format!("{s} {p} {}", term(o, &out, &mut open)))
                    .or_default() += 1;
            }
        } else if !named.contains(s) {
            *set.entry(says(s, &out, &mut open)).or_default() += 1;
        }
    }
    set
}

/// The statements `x` says more often than `y`, with how many more.
fn beyond(x: &BTreeMap<String, usize>, y: &BTreeMap<String, usize>) -> Vec<String> {
    x.iter()
        .filter_map(|(s, &n)| {
            let m = y.get(s).copied().unwrap_or(0);
            (n > m).then(|| match n - m {
                1 => s.clone(),
                k => format!("{s} ({k} times)"),
            })
        })
        .collect()
}

/// How two graphs differ, as the [`statements`] only one has.
fn differ(a: &oxrdf::Dataset, a_name: &str, b: &oxrdf::Dataset, b_name: &str) -> String {
    let (sa, sb) = (statements(a), statements(b));
    let mut out = format!(
        "not isomorphic: {a_name} {} triples, {b_name} {}",
        a.len(),
        b.len()
    );
    for (name, x, y) in [(a_name, &sa, &sb), (b_name, &sb, &sa)] {
        for s in beyond(x, y).iter().take(20) {
            out.push_str(&format!("\n  only {name}: {s}"));
        }
    }
    out
}

/// `text` read by oxrdfxml or oxttl, by the extension of `file`.
fn theirs(file: &str, text: &str) -> oxrdf::Dataset {
    let base = format!("file:///w3c/owl2-primer/{file}");
    let quads: Vec<oxrdf::Quad> = if file.ends_with(".rdf") {
        oxrdfxml::RdfXmlParser::new()
            .with_base_iri(&base)
            .unwrap()
            .for_slice(text.as_bytes())
            .map(|t| t.unwrap().in_graph(oxrdf::GraphName::DefaultGraph))
            .collect()
    } else {
        oxttl::TurtleParser::new()
            .with_base_iri(&base)
            .unwrap()
            .for_slice(text.as_bytes())
            .map(|t| t.unwrap().in_graph(oxrdf::GraphName::DefaultGraph))
            .collect()
    };
    oracle::canonical(quads)
}

/// The classes of IRIs a declaration types them with.
const ENTITY_TYPES: [&str; 6] = [
    "http://www.w3.org/2002/07/owl#Class",
    "http://www.w3.org/2002/07/owl#ObjectProperty",
    "http://www.w3.org/2002/07/owl#DatatypeProperty",
    "http://www.w3.org/2002/07/owl#AnnotationProperty",
    "http://www.w3.org/2002/07/owl#NamedIndividual",
    "http://www.w3.org/2000/01/rdf-schema#Datatype",
];

/// The appendix ontology of the Primer in OWL/XML, Functional syntax,
/// RDF/XML and Turtle, each with its edits of [`KNOWN`], parses with no
/// error. The first three give one graph. The Turtle is a shorter text
/// than the others: with its edits, everything it says is in that graph,
/// which says 61 things more: 40 declarations, and 21 axioms (most of the
/// assertions about individuals, three datatype definitions, the three
/// negative property assertions). Each edit is
/// needed. The RDF/XML and Turtle graphs are also those oxrdfxml and oxttl
/// read.
#[test]
fn the_primer_s_ontology_is_one_graph_in_four_syntaxes() {
    let start = std::time::Instant::now();
    let read = |file: &str, skip| graph(file, &edited(file, skip));
    let ofn = read("59.ofn", None).unwrap();
    let mut problems = Vec::new();
    for file in ["59.owx", "59.rdf"] {
        let g = read(file, None).unwrap();
        if g != ofn {
            problems.push(differ(&ofn, "59.ofn", &g, file));
        }
    }
    for file in ["59.ofn", "59.owx", "59.rdf"] {
        for r in rows(file) {
            if read(file, Some(r)).is_ok_and(|g| g == ofn) {
                problems.push(format!("{file} agrees without the edit {:?}", KNOWN[r].1));
            }
        }
    }
    let ttl = read("59.ttl", None).unwrap();
    let (all, part) = (statements(&ofn), statements(&ttl));
    for s in beyond(&part, &all) {
        problems.push(format!("only 59.ttl: {s}"));
    }
    for r in rows("59.ttl") {
        if read("59.ttl", Some(r)).is_ok_and(|g| beyond(&statements(&g), &all).is_empty()) {
            problems.push(format!(
                "59.ttl is a part without the edit {:?}",
                KNOWN[r].1
            ));
        }
    }
    let omitted = beyond(&all, &part);
    let declarations = omitted
        .iter()
        .filter(|s| {
            let rdf_type = "> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <";
            ENTITY_TYPES
                .iter()
                .any(|c| s.starts_with('<') && s.ends_with(&format!("{rdf_type}{c}>")))
        })
        .count();
    for file in ["59.rdf", "59.ttl"] {
        let text = edited(file, None);
        let ours = graph(file, &text).unwrap();
        if let Some(d) = oracle::difference(&ours, "ours", &theirs(file, &text), "theirs", 10) {
            problems.push(format!("{file}: {d}"));
        }
    }
    eprintln!(
        "W3C OWL 2 Primer, appendix: OWL/XML, Functional and RDF/XML give one graph of {} triples; the Turtle {} triples, {} statements fewer ({declarations} declarations), in {:?}",
        ofn.len(),
        ttl.len(),
        omitted.len(),
        start.elapsed()
    );
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    assert_eq!(ofn.len(), 315);
    assert_eq!(
        (omitted.len(), declarations),
        (61, 40),
        "the Turtle omits:\n{}",
        omitted.join("\n")
    );
    let print = oracle::fingerprint(&ofn, &ttl);
    assert_eq!(
        print, 0x9fd2_8393_a4c2_bf1c,
        "the difference itself: {print:#018x}"
    );
    assert_eq!(as_read("59.ttl").1, 103);
}
