//! Cross-checks the JSON-LD frontend against oxjsonld (Oxigraph) on the
//! public corpus: for every document, both parsers' graphs must be
//! isomorphic (blank nodes renamed, named graphs included).
//!
//! The corpus is downloaded by `tools/corpus/fetch.sh` (or
//! `tools/corpus/fetch-jsonld.sh`) into `test/corpus/jsonld/`, which is not
//! tracked; `jsonld-sources.tsv` there lists every file and its licence. Its
//! `index.tsv` names each document with its base IRI and each remote
//! `@context` file with the IRI it answers. Both parsers get those contexts
//! offline: oxjsonld through its document loader callback, ours through
//! [`NoNetwork`]. Without the corpus the test skips.
//!
//! Every file gets a line on stderr (`--nocapture` shows them): `ok`, `DIFF`
//! with both quad counts and an example quad only one side has, or `skip`
//! when oxjsonld itself rejects the document. The test only reports for now:
//! it fails on a difference only with `JSONLD_ORACLE_STRICT=1`, which the
//! engine work (ticket 08) makes the default.
//!
//!     cargo test --release --test jsonld_oracle -- --nocapture
//!     JSONLD_ORACLE_STRICT=1 cargo test --release --test jsonld_oracle

use oxrdf::dataset::CanonicalizationAlgorithm;
use oxrdf::{
    BaseDirection, BlankNode, Dataset, GraphName as OxGraph, Literal, NamedNode, NamedOrBlankNode,
    Quad, Term,
};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use turtle_lsp::jsonld::{self, NoNetwork};
use turtle_lsp::syntax::{GraphName, Node};

fn blank(id: u32) -> BlankNode {
    BlankNode::new_unchecked(format!("b{id}"))
}

fn term(n: &Node) -> Term {
    match n {
        Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
        Node::Blank(b) => blank(b.0).into(),
        Node::Literal {
            value,
            datatype,
            lang,
        } => match lang {
            // The base direction stays in the tag, as in Turtle: `ar--rtl`.
            Some(l) => match l.split_once("--") {
                Some((l, dir)) => Literal::new_directional_language_tagged_literal_unchecked(
                    &**value,
                    l,
                    if dir == "rtl" {
                        BaseDirection::Rtl
                    } else {
                        BaseDirection::Ltr
                    },
                ),
                None => Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()),
            }
            .into(),
            None => {
                Literal::new_typed_literal(&**value, NamedNode::new_unchecked(datatype.as_str()))
                    .into()
            }
        },
        Node::Triple(t) => oxrdf::Triple::new(
            subject(&t.0),
            NamedNode::new_unchecked(t.1.as_str()),
            term(&t.2),
        )
        .into(),
    }
}

fn subject(n: &Node) -> NamedOrBlankNode {
    match term(n) {
        Term::NamedNode(n) => n.into(),
        Term::BlankNode(b) => b.into(),
        t => panic!("not a subject: {t}"),
    }
}

/// Our parse of `text`, as a dataset.
fn ours(text: &str, base: &str, loader: &NoNetwork) -> Dataset {
    let parsed = jsonld::parse(text, Some(base), loader);
    parsed
        .triples
        .iter()
        .map(|t| {
            let graph = match t.graph {
                None => OxGraph::DefaultGraph,
                Some(GraphName::Iri(i)) => NamedNode::new_unchecked(i.as_str()).into(),
                Some(GraphName::Blank(b)) => blank(b.0).into(),
            };
            Quad::new(
                subject(&t.s),
                NamedNode::new_unchecked(t.p.as_str()),
                term(&t.o),
                graph,
            )
        })
        .collect()
}

/// oxjsonld's parse of `text`, or its first error.
fn oracle(
    text: &str,
    base: &str,
    contexts: &Arc<HashMap<String, Vec<u8>>>,
) -> Result<Dataset, String> {
    let contexts = Arc::clone(contexts);
    oxjsonld::JsonLdParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?
        .for_slice(text)
        .with_load_document_callback(move |url, _| match contexts.get(url) {
            Some(doc) => Ok(oxjsonld::JsonLdRemoteDocument {
                document: doc.clone(),
                document_url: url.into(),
            }),
            None => Err(format!("not in the corpus: {url}").into()),
        })
        .map(|q| q.map_err(|e| e.to_string()))
        .collect()
}

#[test]
fn corpus_graphs_match_oxjsonld() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus/jsonld");
    let Ok(index) = fs::read_to_string(dir.join("index.tsv")) else {
        eprintln!("skipping: test/corpus/jsonld not present (run tools/corpus/fetch-jsonld.sh)");
        return;
    };
    let mut docs = Vec::new();
    let mut contexts = HashMap::new();
    let mut loader = NoNetwork::default();
    for line in index.lines() {
        let mut cols = line.split('\t');
        let (Some(kind), Some(path), Some(iri)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        match kind {
            "doc" => docs.push((path.to_string(), iri.to_string())),
            "context" => {
                let text = fs::read_to_string(dir.join(path)).unwrap();
                loader = loader.with(iri, text.as_str());
                contexts.insert(iri.to_string(), text.into_bytes());
            }
            _ => {}
        }
    }
    let contexts = Arc::new(contexts);
    let (mut same, mut differ, mut skipped) = (0, 0, 0);
    for (path, base) in &docs {
        let text = fs::read_to_string(dir.join(path)).unwrap();
        let mut want = match oracle(&text, base, &contexts) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("skip  {path}: oxjsonld rejects it: {e}");
                skipped += 1;
                continue;
            }
        };
        let mut got = ours(&text, base, &loader);
        want.canonicalize(CanonicalizationAlgorithm::Unstable);
        got.canonicalize(CanonicalizationAlgorithm::Unstable);
        if got == want {
            eprintln!("ok    {path} ({} quads)", want.len());
            same += 1;
        } else {
            let example = want
                .iter()
                .find(|q| !got.contains(*q))
                .map(|q| format!("oxjsonld only: {q}"))
                .or_else(|| {
                    got.iter()
                        .find(|q| !want.contains(*q))
                        .map(|q| format!("ours only: {q}"))
                })
                .unwrap_or_default();
            eprintln!(
                "DIFF  {path}: ours {} quads, oxjsonld {}; {example}",
                got.len(),
                want.len()
            );
            differ += 1;
        }
    }
    eprintln!(
        "jsonld oracle: {} documents, {same} isomorphic, {differ} differ, {skipped} rejected by oxjsonld",
        docs.len()
    );
    assert!(
        !docs.is_empty(),
        "test/corpus/jsonld/index.tsv lists no documents"
    );
    if std::env::var("JSONLD_ORACLE_STRICT").is_ok_and(|v| v == "1") {
        assert_eq!(differ, 0, "{differ} documents differ from oxjsonld");
    }
}
