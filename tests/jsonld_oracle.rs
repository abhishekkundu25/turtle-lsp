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
//! [`NoNetwork`]. Without the corpus the test prints `skipped`, and fails when
//! `CI=true`.
//!
//! Every file gets a line on stderr (`--nocapture` shows them): `ok`, `DIFF`
//! with both quad counts and an example quad only one side has, `known`
//! for a difference listed in [`KNOWN`], or `skip` when oxjsonld itself
//! rejects the document. The test fails on any unlisted difference, and on
//! a listed one that changed or went away (update [`KNOWN`] then);
//! `JSONLD_ORACLE_STRICT=0` only reports.
//!
//!     cargo test --release --test jsonld_oracle -- --nocapture

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

include!("support/corpus.rs");

/// Files where oxjsonld (0.2) departs from JSON-LD 1.1 and this engine
/// follows it: (path, our quads, oxjsonld's quads, why). Each was checked by
/// hand against the spec and the W3C suite.
const KNOWN: &[(&str, usize, usize, &str)] = &[
    // oxjsonld keeps the value of an @graph-container term that is written
    // as an @id string as the object; JSON-LD 1.1 Expansion step 13.12 wraps
    // every value in a graph object, so the object is a blank node naming a
    // graph that holds the reference.
    (
        "vc-1.0/example-016-jwt-presentation.jsonld",
        4,
        4,
        GRAPH_CONTAINER,
    ),
    (
        "vc-1.0/example-016-jwt-presentation-no-iss.jsonld",
        3,
        3,
        GRAPH_CONTAINER,
    ),
    (
        "vc-1.0/example-016-jwt-presentation-no-jti.jsonld",
        4,
        4,
        GRAPH_CONTAINER,
    ),
    // oxjsonld (rdf-12 feature) keeps @direction as an RDF 1.2 directional
    // language-tagged string; JSON-LD 1.1 toRdf without rdfDirection drops
    // the direction (W3C toRdf#tdi01 to #tdi07).
    (
        "vc-2.0/credential-name-language-direction-en-ok.jsonld",
        3,
        3,
        DIRECTION,
    ),
    // The IIIF cookbook manifests have template @ids ("{{ id.url }}"), not
    // IRIs, so the nodes are dropped with their triples; oxjsonld still
    // emits the rdf:first/rdf:rest triples of the @list values of those
    // nodes (the spec converts a list only for a triple it keeps). 0006 and
    // 0007 also alias @none ("none") in the scoped context of "value";
    // oxjsonld tags those strings "@none" (both read the same once
    // normalized; checked by hand).
    ("iiif/0001-mvm-image-manifest.jsonld", 4, 7, DROPPED_LIST),
    ("iiif/0002-mvm-audio-manifest.jsonld", 3, 6, DROPPED_LIST),
    ("iiif/0003-mvm-video-manifest.jsonld", 5, 8, DROPPED_LIST),
    ("iiif/0004-canvas-size-manifest.jsonld", 4, 7, DROPPED_LIST),
    (
        "iiif/0005-image-service-manifest.jsonld",
        7,
        10,
        DROPPED_LIST,
    ),
    (
        "iiif/0006-text-language-manifest.jsonld",
        17,
        24,
        DROPPED_LIST,
    ),
    (
        "iiif/0007-string-formats-manifest.jsonld",
        11,
        16,
        DROPPED_LIST,
    ),
    ("iiif/0008-rights-manifest.jsonld", 9, 12, DROPPED_LIST),
    ("iiif/0009-book-1-manifest.jsonld", 35, 50, DROPPED_LIST),
    (
        "iiif/0010-book-2-viewing-direction-manifest-rtl.jsonld",
        35,
        50,
        DROPPED_LIST,
    ),
    (
        "iiif/0010-book-2-viewing-direction-manifest-ttb.jsonld",
        28,
        40,
        DROPPED_LIST,
    ),
    (
        "iiif/0011-book-3-behavior-manifest-continuous.jsonld",
        28,
        40,
        DROPPED_LIST,
    ),
    (
        "iiif/0011-book-3-behavior-manifest-individuals.jsonld",
        28,
        40,
        DROPPED_LIST,
    ),
    (
        "iiif/0013-placeholderCanvas-manifest.jsonld",
        9,
        14,
        DROPPED_LIST,
    ),
    (
        "iiif/0014-accompanyingcanvas-manifest.jsonld",
        10,
        15,
        DROPPED_LIST,
    ),
    ("iiif/0015-start-manifest.jsonld", 7, 10, DROPPED_LIST),
    (
        "iiif/0017-transcription-av-manifest.jsonld",
        8,
        11,
        DROPPED_LIST,
    ),
    (
        "iiif/0019-html-in-annotations-manifest.jsonld",
        11,
        16,
        DROPPED_LIST,
    ),
    ("iiif/0021-tagging-manifest.jsonld", 11, 16, DROPPED_LIST),
    (
        "iiif/0022-linking-with-a-hotspot-manifest.jsonld",
        19,
        27,
        DROPPED_LIST,
    ),
    // "geographicBoundingBox" is not defined for a dcat:DatasetSeries, so
    // the key is dropped with its value (Expansion step 13.3); oxjsonld
    // still emits the rdf:type triple of the node object in the value.
    ("dcat-us/dataset-in_series.jsonld", 22, 23, UNDEFINED_KEY),
];

const GRAPH_CONTAINER: &str = "oxjsonld does not wrap an @id-coerced value of an @graph container";
const DIRECTION: &str = "oxjsonld keeps @direction (RDF 1.2); JSON-LD 1.1 drops it";
const DROPPED_LIST: &str = "oxjsonld emits the list triples of a node with an invalid @id";
const UNDEFINED_KEY: &str = "oxjsonld emits a triple from the value of an undefined key";

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
            dir,
        } => match (lang, dir) {
            (Some(l), Some(d)) => Literal::new_directional_language_tagged_literal_unchecked(
                &**value,
                l.as_str(),
                match d {
                    turtle_lsp::syntax::Direction::Rtl => BaseDirection::Rtl,
                    turtle_lsp::syntax::Direction::Ltr => BaseDirection::Ltr,
                },
            )
            .into(),
            (Some(l), None) => {
                Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()).into()
            }
            _ => Literal::new_typed_literal(&**value, NamedNode::new_unchecked(datatype.as_str()))
                .into(),
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
    let Some(index_path) = corpus_path("jsonld/index.tsv") else {
        return;
    };
    let dir = index_path.parent().unwrap().to_path_buf();
    let index = fs::read_to_string(index_path).unwrap();
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
    let (mut same, mut differ, mut known, mut skipped) = (0, 0, 0, 0);
    let mut problems = Vec::new();
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
        let listed = KNOWN.iter().find(|k| k.0 == path);
        if got == want {
            eprintln!("ok    {path} ({} quads)", want.len());
            same += 1;
            if listed.is_some() {
                problems.push(format!(
                    "{path}: now matches oxjsonld; remove it from KNOWN"
                ));
            }
        } else if let Some(&(_, ours, theirs, why)) = listed
            && (got.len(), want.len()) == (ours, theirs)
        {
            eprintln!("known {path}: ours {ours} quads, oxjsonld {theirs}: {why}");
            known += 1;
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
            let line = format!(
                "{path}: ours {} quads, oxjsonld {}; {example}",
                got.len(),
                want.len()
            );
            eprintln!("DIFF  {line}");
            problems.push(line);
            differ += 1;
        }
    }
    eprintln!(
        "jsonld oracle: {} documents, {same} isomorphic, {known} known differences, {differ} differ, {skipped} rejected by oxjsonld",
        docs.len()
    );
    assert!(
        !docs.is_empty(),
        "test/corpus/jsonld/index.tsv lists no documents"
    );
    if std::env::var("JSONLD_ORACLE_STRICT").is_ok_and(|v| v == "0") {
        return;
    }
    assert!(
        problems.is_empty(),
        "{} documents differ from oxjsonld beyond the KNOWN list:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// A missing corpus is skipped on a developer's machine and an error in CI.
#[test]
fn a_missing_corpus_fails_in_ci_and_skips_elsewhere() {
    let absent = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/no-such-corpus");
    assert_eq!(corpus_in(&absent, "jsonld/index.tsv", false), None);
    let in_ci = std::panic::catch_unwind(|| corpus_in(&absent, "jsonld/index.tsv", true));
    let message = *in_ci.unwrap_err().downcast::<String>().unwrap();
    assert!(message.contains("CI=true does not skip"), "{message}");
    // A corpus that is there is used in both.
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        corpus_in(here, "Cargo.toml", true),
        Some(here.join("Cargo.toml"))
    );
}
