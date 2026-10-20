//! The JSON-LD lints on the public corpus: no document gets one, except
//! those listed in [`EXPECTED`], each checked by hand.
//!
//! The corpus is downloaded by `tools/corpus/fetch.sh` (or
//! `tools/corpus/fetch-jsonld.sh`) into `test/corpus/jsonld/`, which is not
//! tracked. Documents are checked as `check` checks them, with their URL as
//! the base and the corpus's remote contexts offline. Without the corpus the
//! test skips.
//!
//!     cargo test --release --test jsonld_lints -- --nocapture

use std::fs;
use std::path::Path;
use std::sync::Arc;
use turtle_lsp::analysis::{Analysis, Encoding};
use turtle_lsp::features::diagnostics::diagnostics;
use turtle_lsp::jsonld::NoNetwork;
use turtle_lsp::knowledge::{Knowledge, Source};
use turtle_lsp::syntax::{PrefixMap, Syntax};

/// The codes this test watches: the JSON-LD lints and the vocabulary lints
/// they reach through the context.
const CODES: &[&str] = &[
    "jsonld-undefined-term",
    "jsonld-undefined-prefix",
    "invalid-iri",
    "invalid-language-tag",
    "unknown-term",
    "deprecated-term",
];

/// What the corpus is expected to get: (path, code, what the diagnostic
/// covers, why). A path or covered text ending in `*` matches by prefix.
/// Every entry must match at least one diagnostic, and every diagnostic an
/// entry. Each was checked by hand; the graphs of all these files are the
/// same as oxjsonld's (tests/jsonld_oracle.rs), so oxjsonld drops the same
/// keys and IRIs.
const EXPECTED: &[(&str, &str, &str, &str)] = &[
    // Keys no context defines: JSON-LD drops them (true positives).
    (
        "vc-1.0/example-015-zkp-vp.jsonld",
        UNDEFINED,
        "proofValue",
        "the presentation's proof type ex:AnonCredPresentationProofv1 has no scoped context, and credentials/v1 defines proofValue only in those of its proof types",
    ),
    (
        "iiif/0015-start-manifest.jsonld",
        UNDEFINED,
        "source",
        "the IIIF Presentation 3 context does not define source (the Web Annotation context does) outside annotations",
    ),
    (
        "iiif/0022-linking-with-a-hotspot-manifest.jsonld",
        UNDEFINED,
        "source",
        "as in 0015",
    ),
    ("dcat-us/catalog.jsonld", UNDEFINED, "name", DCAT_US_SCOPED),
    (
        "dcat-us/catalog-catalog.jsonld",
        UNDEFINED,
        "name",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/catalog-catalog.jsonld",
        UNDEFINED,
        "prefLabel",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/dataset-attributions.jsonld",
        UNDEFINED,
        "name",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/dataset-attributions.jsonld",
        UNDEFINED,
        "prefLabel",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/dataset-dataset.jsonld",
        UNDEFINED,
        "mediaType",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/dataset-dataset.jsonld",
        UNDEFINED,
        "downloadURL",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/dataset-in_series.jsonld",
        UNDEFINED,
        "geographicBoundingBox",
        "dcat:DatasetSeries has no scoped context; dcat:Dataset's defines it",
    ),
    (
        "dcat-us/dataset-in_series.jsonld",
        UNDEFINED,
        "start",
        "dcterms:PeriodOfTime's scoped context defines startDate and endDate",
    ),
    (
        "dcat-us/dataset-in_series.jsonld",
        UNDEFINED,
        "end",
        "as start",
    ),
    (
        "dcat-us/dataset-series.jsonld",
        UNDEFINED,
        "temporalResolution",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/license-document.jsonld",
        UNDEFINED,
        "license",
        DCAT_US_SCOPED,
    ),
    (
        "dcat-us/service-service.jsonld",
        UNDEFINED,
        "name",
        DCAT_US_SCOPED,
    ),
    // IIIF cookbook templates: "{{ id.path }}/canvas/p1" is no IRI, and
    // JSON-LD drops the nodes it names (true positives).
    (
        "iiif/*",
        INVALID_IRI,
        "{{ id.*",
        "a template placeholder, not an IRI",
    ),
    // Terms the vocabularies do not define (true positives).
    (
        "activitystreams/core-ex27.jsonld",
        UNKNOWN,
        "oa:Annotate",
        "no such class in Web Annotation (the motivations are oa:editing, ...)",
    ),
    (
        "activitystreams/core-ex27.jsonld",
        UNKNOWN,
        "oa:created",
        "dcterms:created in Web Annotation",
    ),
    (
        "activitystreams/core-ex27.jsonld",
        UNKNOWN,
        "oa:creator",
        "dcterms:creator in Web Annotation",
    ),
    (
        "croissant/huggingface-squad_v2.jsonld",
        UNKNOWN,
        "prov:locatedAt",
        "PROV-O has prov:atLocation",
    ),
    (
        "croissant/huggingface-squad_v2.jsonld",
        UNKNOWN,
        "prov:isAssociatedWith",
        "PROV-O has prov:wasAssociatedWith (suggested)",
    ),
    (
        "croissant/huggingface-squad_v2.jsonld",
        UNKNOWN,
        "prov:label",
        PROV_LABEL,
    ),
    (
        "croissant/huggingface-wildchat.jsonld",
        UNKNOWN,
        "prov:label",
        PROV_LABEL,
    ),
    (
        "croissant/huggingface-wildchat.jsonld",
        UNKNOWN,
        "prov:description",
        PROV_LABEL,
    ),
    (
        "dcat-us/document.jsonld",
        UNKNOWN,
        "dcterms:mediaType",
        "dcat:mediaType; dcterms has the class MediaType",
    ),
    (
        "dcat-us/nps-organization.jsonld",
        UNKNOWN,
        "dcat:description",
        DCTERMS,
    ),
    (
        "dcat-us/nps-organization.jsonld",
        UNKNOWN,
        "dcat:publisher",
        DCTERMS,
    ),
    (
        "dcat-us/nps-organization.jsonld",
        UNKNOWN,
        "dcat:title",
        DCTERMS,
    ),
    (
        "gs1/gs1Voc-1.16.jsonld",
        UNKNOWN,
        "dc:contributors",
        "dc:contributor (suggested)",
    ),
    (
        "gs1/gs1Voc-1.16.jsonld",
        UNKNOWN,
        "dc:lastModified",
        "not a Dublin Core term",
    ),
    (
        "gs1/gs1Voc-1.16.jsonld",
        UNKNOWN,
        "rdfs:comemnt",
        "rdfs:comment (suggested)",
    ),
    (
        "gs1/gs1Voc-1.16.jsonld",
        UNKNOWN,
        "rdfs:coment",
        "rdfs:comment (suggested)",
    ),
    (
        "gs1/pact-extension.jsonld",
        UNKNOWN,
        "dcterms:contributors",
        "dcterms:contributor (suggested)",
    ),
    (
        "gs1/pact-extension.jsonld",
        UNKNOWN,
        "dcterms:lastModified",
        "not a Dublin Core term",
    ),
    // Terms newer than the bundled vocabulary snapshot (data/vocab.tsv): not
    // a JSON-LD matter (Turtle gets the same), but false positives until the
    // snapshot is refreshed.
    (
        "gs1/gs1Voc-1.16.jsonld",
        UNKNOWN,
        "https://gs1.org/voc/*",
        "GS1 Web Vocabulary 1.16 maps each of its terms to https://gs1.org/voc/ with owl:equivalentClass or owl:equivalentProperty; the snapshot has an older GS1 vocabulary",
    ),
    (
        "dcat-us/dataset-series.jsonld",
        UNKNOWN,
        "dcat:next",
        "a DCAT 3 term; the snapshot has DCAT 2",
    ),
    // Compact IRIs whose prefix no context defines: JSON-LD reads them as
    // IRIs with the prefix as scheme (true positives).
    (
        "gs1/pact-extension.jsonld",
        UNDEFINED_PREFIX,
        "owl:*",
        "its context defines no owl prefix, so owl:Class is the IRI <owl:Class>",
    ),
    (
        "dcat-us/dataset-dataset.jsonld",
        UNDEFINED_PREFIX,
        "xsd:*",
        "the DCAT-US 3.0 context defines no xsd prefix, so the literals are typed <xsd:date> and <xsd:dateTime>",
    ),
    (
        "gs1/gs1Voc-1.16.jsonld",
        UNDEFINED_PREFIX,
        "dc11:title",
        "the context defines dc and dcterms (both as Dublin Core elements), not dc11",
    ),
];

const UNDEFINED: &str = "jsonld-undefined-term";
const UNDEFINED_PREFIX: &str = "jsonld-undefined-prefix";
const INVALID_IRI: &str = "invalid-iri";
const UNKNOWN: &str = "unknown-term";
const DCAT_US_SCOPED: &str = "the DCAT-US 3.0 context defines it only in the type-scoped contexts of other types than the node's";
const PROV_LABEL: &str = "not a PROV-O term (rdfs:label, dcterms:description)";
const DCTERMS: &str = "a dcterms term, not a DCAT one";

/// `text` matches `pattern`, which may end in `*`.
fn matches(pattern: &str, text: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => text.starts_with(prefix),
        None => text == pattern,
    }
}

#[test]
fn the_corpus_gets_only_the_listed_lints() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/corpus/jsonld");
    let Ok(index) = fs::read_to_string(dir.join("index.tsv")) else {
        eprintln!("skipping: test/corpus/jsonld not present (run tools/corpus/fetch-jsonld.sh)");
        return;
    };
    let mut docs = Vec::new();
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
            }
            _ => {}
        }
    }
    // The corpus is one workspace, as `check` sees a folder: what one
    // document describes is known to all.
    let external = Arc::new(PrefixMap::default());
    let mut k = Knowledge::default();
    let mut analyses = Vec::new();
    for (path, base) in &docs {
        let text = fs::read_to_string(dir.join(path)).unwrap();
        let a = Arc::new(Analysis::with_loader(
            base.clone(),
            &text,
            external.clone(),
            false,
            Syntax::JsonLd,
            &loader,
        ));
        k.set(base, Source::Disk, a.clone());
        analyses.push((path, text, a));
    }
    let mut found = Vec::new();
    for (path, text, a) in &analyses {
        for d in diagnostics(a, &k, Encoding::Utf8) {
            let Some(lsp_types::NumberOrString::String(code)) = &d.code else {
                continue;
            };
            let Some(code) = CODES.iter().find(|c| *c == code) else {
                continue;
            };
            let start = a.offset(Encoding::Utf8, d.range.start).unwrap() as usize;
            let end = a.offset(Encoding::Utf8, d.range.end).unwrap() as usize;
            let written = text[start..end].to_string();
            eprintln!(
                "{path}:{}:{}: {code} {written:?}: {}",
                d.range.start.line + 1,
                d.range.start.character + 1,
                d.message
            );
            found.push((path.to_string(), *code, written));
        }
    }
    let is = |e: &(&str, &str, &str, &str), (p, c, w): &(String, &str, String)| {
        matches(e.0, p) && e.1 == *c && matches(e.2, w)
    };
    let unexpected: Vec<_> = found
        .iter()
        .filter(|f| !EXPECTED.iter().any(|e| is(e, f)))
        .collect();
    let missing: Vec<_> = EXPECTED
        .iter()
        .filter(|e| !found.iter().any(|f| is(e, f)))
        .map(|e| (e.0, e.1, e.2))
        .collect();
    assert!(
        unexpected.is_empty() && missing.is_empty(),
        "unexpected lints: {unexpected:#?}\nexpected, not found: {missing:#?}"
    );
}
