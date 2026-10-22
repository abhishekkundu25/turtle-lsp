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
    let (mut server, _) =
        Embedded::start(json!({"processId": null, "rootUri": null, "capabilities": {}})).unwrap();
    let uri = format!("file:///work/{name}");
    let open = Message::Notification(Notification::new(
        "textDocument/didOpen".into(),
        json!({"textDocument": {"uri": uri, "languageId": "turtle", "version": 1, "text": text}}),
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
