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
//! Both graphs are compared as canonical datasets (tests/support/oracle.rs).
//! Every file gets a line on stderr (`--nocapture` shows them): `ok`, `DIFF`
//! with both quad counts, the fingerprint of the difference and the quads
//! only one side has, `known` for a difference listed in [`KNOWN`], or
//! `skip` when oxjsonld itself rejects the document. A listed difference
//! must have the listed counts and the listed fingerprint of the quads only
//! one side has ([`oracle::fingerprint`]), so a different difference in a
//! listed file is not taken for the known one. The test fails on any
//! unlisted difference, and on a listed one that changed or went away
//! (update [`KNOWN`] then); `JSONLD_ORACLE_STRICT=0` only reports.
//!
//!     cargo test --release --test jsonld_oracle -- --nocapture

use oxrdf::Dataset;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use turtle_lsp::jsonld::{self, NoNetwork};

#[path = "support/oracle.rs"]
mod oracle;

include!("support/corpus.rs");

/// Files where oxjsonld (0.2) departs from JSON-LD 1.1 and this engine
/// follows it: (path, our quads, oxjsonld's quads, the fingerprint of the
/// difference, why). Each was checked by hand against the spec and the W3C
/// suite.
const KNOWN: &[(&str, usize, usize, u64, &str)] = &[
    // oxjsonld keeps the value of an @graph-container term that is written
    // as an @id string as the object; JSON-LD 1.1 Expansion step 13.12 wraps
    // every value in a graph object, so the object is a blank node naming a
    // graph that holds the reference.
    (
        "vc-1.0/example-016-jwt-presentation.jsonld",
        4,
        4,
        0x0ff2d02108b966c8,
        GRAPH_CONTAINER,
    ),
    (
        "vc-1.0/example-016-jwt-presentation-no-iss.jsonld",
        3,
        3,
        0x0ff2d02108b966c8,
        GRAPH_CONTAINER,
    ),
    (
        "vc-1.0/example-016-jwt-presentation-no-jti.jsonld",
        4,
        4,
        0x1b8fb9dea7b69c9e,
        GRAPH_CONTAINER,
    ),
    // oxjsonld (rdf-12 feature) keeps @direction as an RDF 1.2 directional
    // language-tagged string; JSON-LD 1.1 toRdf without rdfDirection drops
    // the direction (W3C toRdf#tdi01 to #tdi07).
    (
        "vc-2.0/credential-name-language-direction-en-ok.jsonld",
        3,
        3,
        0xd5a16a307d11ef1f,
        DIRECTION,
    ),
    // The IIIF cookbook manifests have template @ids ("{{ id.url }}"), not
    // IRIs, so the nodes are dropped with their triples; oxjsonld still
    // emits the rdf:first/rdf:rest triples of the @list values of those
    // nodes (the spec converts a list only for a triple it keeps). 0006 and
    // 0007 also alias @none ("none") in the scoped context of "value";
    // oxjsonld tags those strings "@none" (both read the same once
    // normalized; checked by hand).
    (
        "iiif/0001-mvm-image-manifest.jsonld",
        4,
        7,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0002-mvm-audio-manifest.jsonld",
        3,
        6,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0003-mvm-video-manifest.jsonld",
        5,
        8,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0004-canvas-size-manifest.jsonld",
        4,
        7,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0005-image-service-manifest.jsonld",
        7,
        10,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0006-text-language-manifest.jsonld",
        17,
        24,
        0xd8c110713d306697,
        DROPPED_LIST,
    ),
    (
        "iiif/0007-string-formats-manifest.jsonld",
        11,
        16,
        0xaf5043ff8ffe8b61,
        DROPPED_LIST,
    ),
    (
        "iiif/0008-rights-manifest.jsonld",
        9,
        12,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0009-book-1-manifest.jsonld",
        35,
        50,
        0x63817ce167b8f0ad,
        DROPPED_LIST,
    ),
    (
        "iiif/0010-book-2-viewing-direction-manifest-rtl.jsonld",
        35,
        50,
        0x63817ce167b8f0ad,
        DROPPED_LIST,
    ),
    (
        "iiif/0010-book-2-viewing-direction-manifest-ttb.jsonld",
        28,
        40,
        0x9f2bafeaae2d870f,
        DROPPED_LIST,
    ),
    (
        "iiif/0011-book-3-behavior-manifest-continuous.jsonld",
        28,
        40,
        0x9f2bafeaae2d870f,
        DROPPED_LIST,
    ),
    (
        "iiif/0011-book-3-behavior-manifest-individuals.jsonld",
        28,
        40,
        0x9f2bafeaae2d870f,
        DROPPED_LIST,
    ),
    (
        "iiif/0013-placeholderCanvas-manifest.jsonld",
        9,
        14,
        0x93cae7840cb321ad,
        DROPPED_LIST,
    ),
    (
        "iiif/0014-accompanyingcanvas-manifest.jsonld",
        10,
        15,
        0x93cae7840cb321ad,
        DROPPED_LIST,
    ),
    (
        "iiif/0015-start-manifest.jsonld",
        7,
        10,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0017-transcription-av-manifest.jsonld",
        8,
        11,
        0xdc965ec9932097ed,
        DROPPED_LIST,
    ),
    (
        "iiif/0019-html-in-annotations-manifest.jsonld",
        11,
        16,
        0x93cae7840cb321ad,
        DROPPED_LIST,
    ),
    (
        "iiif/0021-tagging-manifest.jsonld",
        11,
        16,
        0x93cae7840cb321ad,
        DROPPED_LIST,
    ),
    (
        "iiif/0022-linking-with-a-hotspot-manifest.jsonld",
        19,
        27,
        0x2385e57cad1b31af,
        DROPPED_LIST,
    ),
    // "geographicBoundingBox" is not defined for a dcat:DatasetSeries, so
    // the key is dropped with its value (Expansion step 13.3); oxjsonld
    // still emits the rdf:type triple of the node object in the value.
    (
        "dcat-us/dataset-in_series.jsonld",
        22,
        23,
        0x2d22068c8d9b399e,
        UNDEFINED_KEY,
    ),
];

const GRAPH_CONTAINER: &str = "oxjsonld does not wrap an @id-coerced value of an @graph container";
const DIRECTION: &str = "oxjsonld keeps @direction (RDF 1.2); JSON-LD 1.1 drops it";
const DROPPED_LIST: &str = "oxjsonld emits the list triples of a node with an invalid @id";
const UNDEFINED_KEY: &str = "oxjsonld emits a triple from the value of an undefined key";

/// Our parse of `text`, as a canonical dataset.
fn ours(text: &str, base: &str, loader: &NoNetwork) -> Dataset {
    oracle::ours(&jsonld::parse(text, Some(base), loader).triples)
}

/// oxjsonld's parse of `text`, or its first error.
fn oxjsonld_quads(
    text: &str,
    base: &str,
    contexts: &Arc<HashMap<String, Vec<u8>>>,
) -> Result<Vec<oxrdf::Quad>, String> {
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
        let want = match oxjsonld_quads(&text, base, &contexts) {
            Ok(d) => oracle::canonical(d),
            Err(e) => {
                eprintln!("skip  {path}: oxjsonld rejects it: {e}");
                skipped += 1;
                continue;
            }
        };
        let got = ours(&text, base, &loader);
        let fingerprint = oracle::fingerprint(&got, &want);
        let listed = KNOWN.iter().find(|k| k.0 == path);
        if got == want {
            eprintln!("ok    {path} ({} quads)", want.len());
            same += 1;
            if listed.is_some() {
                problems.push(format!(
                    "{path}: now matches oxjsonld; remove it from KNOWN"
                ));
            }
        } else if let Some(&(_, ours, theirs, print, why)) = listed
            && (got.len(), want.len(), fingerprint) == (ours, theirs, print)
        {
            eprintln!("known {path}: ours {ours} quads, oxjsonld {theirs}: {why}");
            known += 1;
        } else {
            let diff = oracle::difference(&got, "ours", &want, "oxjsonld", 3).unwrap_or_default();
            let line = format!("{path}: fingerprint {fingerprint:#018x}, {diff}");
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
