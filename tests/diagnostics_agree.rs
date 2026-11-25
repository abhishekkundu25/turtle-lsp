//! The editor and `check` show the same diagnostics: a document open in the
//! server (`Embedded`, the server a browser hosts) and the same file given to
//! `batch::analyse` come out equal, whatever the document is, because both
//! are composed by `diagnose::document`. (Import statuses are the one input
//! the two get differently, from the server's resolution passes, so the
//! documents here have no imports; `diagnose`'s own tests cover those.)

use lsp_server::{Message, Notification};
use serde_json::{Value, json};
use turtle_lsp::batch::{self, BatchFile, Options};
use turtle_lsp::server::Embedded;
use turtle_lsp::settings::Settings;

/// What the server publishes for `text` opened at `file:///work/{name}`.
fn published(name: &str, text: &str) -> Vec<Value> {
    published_as(name, text, "turtle")
}

/// [`published`], with the client's language id.
fn published_as(name: &str, text: &str, language_id: &str) -> Vec<Value> {
    let (mut server, _) =
        Embedded::start(json!({"processId": null, "rootUri": null, "capabilities": {}})).unwrap();
    let uri = format!("file:///work/{name}");
    let open = Message::Notification(Notification::new(
        "textDocument/didOpen".into(),
        json!({"textDocument": {"uri": uri, "languageId": language_id, "version": 1, "text": text}}),
    ));
    let sent = server.handle(vec![open]);
    sent.into_iter()
        .rev()
        .find_map(|m| match m {
            Message::Notification(n)
                if n.method == "textDocument/publishDiagnostics" && n.params["uri"] == uri =>
            {
                Some(
                    n.params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                )
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("nothing published for {name}"))
}

/// What `batch::analyse` finds for the same file, as the server would send it.
fn batched(name: &str, text: &str) -> Vec<Value> {
    let files = [BatchFile::named(name, text.to_string(), false)];
    let opts = Options {
        fixes: false,
        ..Options::default()
    };
    let report = batch::analyse(&files, &Settings::default(), &opts);
    report.files[0]
        .problems
        .iter()
        .map(|p| serde_json::to_value(&p.diagnostic).unwrap())
        .collect()
}

#[test]
fn an_open_document_has_the_diagnostics_check_reports() {
    let rdf = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix unused: <http://u/> .\n\
        @prefix sh: <http://www.w3.org/ns/shacl#> .\n\
        <http://x/o> a owl:Ontology .\n\
        <http://x/a> rdfs:labl \"a\" ; foaf:name \"b\" .\n\
        <http://x/P> a sh:PropertyShape .\n\
        <http://x/s> <http://x/p> \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> , \"x\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n";
    let sparql = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
        PREFIX unused: <http://u/>\n\
        SELECT ?s WHERE { ?s rdfs:labl ?o . ?s foaf:name ?n }\n";
    for (name, text) in [("a.ttl", rdf), ("q.rq", sparql)] {
        let (server, check) = (published(name, text), batched(name, text));
        assert!(check.len() >= 3, "{name}: {check:?}");
        assert_eq!(server, check, "{name}");
    }
}

/// An `.owl` or `.rdf` file's syntax is its content's, and a `.ofn` or
/// `.owx` file's its name's, the same for the server, whatever language id the client
/// sends (none, VS Code for the Web; `xml`, the web editor; `turtle`), and
/// for `check`.
#[test]
fn an_owl_file_has_the_diagnostics_check_reports_whatever_its_content() {
    let xml = "<?xml version=\"1.0\"?>\n\
        <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
        \x20        xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\">\n\
        \x20 <rdf:Description rdf:about=\"http://example.org/a\">\n\
        \x20   <rdfs:label>a</rdfs:label>\n\
        \x20   <rdfs:seeAlso><rdf:Description><rdfs:label>b</rdfs:label></rdf:Description></rdfs:seeAlso>\n\
        \x20 </rdf:Description>\n\
        </rdf:RDF>\n";
    let broken = xml.replace("</rdf:Description>\n</rdf:RDF>", "</rdf:Descr>\n</rdf:RDF>");
    let functional = "Prefix(:=<http://example.org/>)\nOntology(<http://example.org/o>)\n";
    let broken_functional = "Prefix(:=<http://example.org/>)\nOntology(SubClassOf(:A nope:B))\n";
    let owl_xml = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Prefix name=\"\" IRI=\"http://example.org/\"/>\n  <Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n</Ontology>\n";
    let broken_owx = owl_xml.replace(
        "</Ontology>",
        "  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"nope:B\"/></SubClassOf>\n</Ontology>",
    );
    let bad_xml = owl_xml.replace("</Declaration>", "</Declaratio>");
    // A misspelt `skos:Concept`, as each syntax writes a reference to it.
    let misspelt_xml = xml
        .replace(
            "xmlns:rdfs=",
            "xmlns:skos=\"http://www.w3.org/2004/02/skos/core#\"\n         xmlns:rdfs=",
        )
        .replace("<rdfs:label>a</rdfs:label>", "<rdf:type rdf:resource=\"http://www.w3.org/2004/02/skos/core#Conceptt\"/><skos:prefLabl>a</skos:prefLabl>");
    let misspelt_functional = "Prefix(:=<http://example.org/>)\nPrefix(skos:=<http://www.w3.org/2004/02/skos/core#>)\nOntology(<http://example.org/o>\nSubClassOf(:A skos:Conceptt)\nSubClassOf(:A <http://www.w3.org/2004/02/skos/core#Conceptt>)\n)\n";
    let misspelt_owx = owl_xml
        .replace(
            "  <Declaration>",
            "  <Prefix name=\"skos\" IRI=\"http://www.w3.org/2004/02/skos/core#\"/>\n  <Declaration>",
        )
        .replace(
            "</Ontology>",
            "  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"skos:Conceptt\"/></SubClassOf>\n  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class IRI=\"http://www.w3.org/2004/02/skos/core#Conceptt\"/></SubClassOf>\n</Ontology>",
        );
    let manchester = "Prefix: : <http://example.org/>\nOntology: <http://example.org/o>\n";
    let turtle = "@prefix ex: <http://example.org/> .\nex:a ex:b ex:c ; ex:d \"1\"^^<http://www.w3.org/2001/XMLSchema#integer> , \"x\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n";
    for (name, text, code) in [
        ("onto.owl", xml, None),
        ("broken.rdf", broken.as_str(), Some("xml-syntax")),
        ("functional.owl", functional, None),
        ("broken.owl", broken_functional, Some("ofn-syntax")),
        ("broken.ofn", broken_functional, Some("ofn-syntax")),
        ("owlxml.owl", owl_xml, None),
        ("broken-owlxml.owl", broken_owx.as_str(), Some("owx-syntax")),
        ("onto.owx", owl_xml, None),
        ("Broken.OWX", broken_owx.as_str(), Some("owx-syntax")),
        ("xml-error.owx", bad_xml.as_str(), Some("xml-syntax")),
        ("misspelt.rdf", misspelt_xml.as_str(), Some("unknown-term")),
        ("misspelt.owl", misspelt_functional, Some("unknown-term")),
        ("misspelt.ofn", misspelt_functional, Some("unknown-term")),
        ("misspelt.owx", misspelt_owx.as_str(), Some("unknown-term")),
        ("manchester.owl", manchester, Some("unsupported-syntax")),
        ("turtle.owl", turtle, Some("invalid-literal")),
    ] {
        let check = batched(name, text);
        let codes: Vec<&str> = check.iter().filter_map(|d| d["code"].as_str()).collect();
        assert_eq!(codes.first().copied(), code, "{name}: {check:?}");
        if code == Some("unknown-term") {
            // Two misspelt references; the "did you mean" in the message,
            // with no fix data: nothing writes these syntaxes.
            assert_eq!(codes, ["unknown-term", "unknown-term"], "{name}: {check:?}");
            for d in &check {
                let message = d["message"].as_str().unwrap();
                assert!(message.contains("Did you mean '"), "{name}: {message}");
                assert_eq!(d.get("data"), None, "{name}: {d}");
            }
        }
        for id in ["turtle", "", "xml"] {
            assert_eq!(published_as(name, text, id), check, "{name} as {id:?}");
        }
    }
}
