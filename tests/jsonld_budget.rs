//! The JSON-LD performance budgets (docs/specs/jsonld.md), on a generated
//! 30,000-line document like `bench/jsonld-gen.mjs`'s:
//! - open to diagnostics within 3x a Turtle document of the same length;
//! - keystroke to diagnostics under 10 ms, with the contexts cached.
//!
//! Each step is what the server does for an open document: parse into an
//! [`Analysis`], put it in the [`Knowledge`], compute its diagnostics. The
//! times leave out the LSP transport; `npm run bench` measures end to end.
//! Timings mean nothing in a debug build, so the test runs in release only:
//!
//!     cargo test --release --test jsonld_budget -- --nocapture
#![cfg(not(debug_assertions))]

use std::sync::Arc;
use std::time::{Duration, Instant};
use turtle_lsp::analysis::{Analysis, Encoding};
use turtle_lsp::features::diagnostics::diagnostics;
use turtle_lsp::knowledge::{Knowledge, Source};
use turtle_lsp::syntax::{PrefixMap, Syntax};

const LINES: usize = 30_000;
const RUNS: usize = 7;

/// `bench/perf.mjs`'s Turtle document: 6 lines per person.
fn turtle(lines: usize, tag: &str) -> String {
    let n = lines / 6;
    let mut out = String::from(
        "@prefix ex: <http://example.org/> .\n\
         @prefix foaf: <http://xmlns.com/foaf/0.1/> .\n\
         @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
         @prefix schema: <https://schema.org/> .\n",
    );
    for i in 0..n {
        out += &format!(
            "\nex:p{tag}{i} a foaf:Person ;\n    foaf:name \"Person {i}\" ;\n    \
             rdfs:comment \"\"\"A longer\nmulti-line description {i}\"\"\" ;\n    \
             foaf:knows ex:p{tag}{} .\n",
            (i + 1) % n
        );
    }
    out
}

/// `bench/jsonld-gen.mjs`'s document, pretty-printed the same way: one
/// `@graph` of people under an inline `@context`, 20 lines per person.
fn jsonld(lines: usize, tag: &str) -> String {
    let n = ((lines - 27) as f64 / 20.0).round() as usize;
    let mut out = String::from(
        r#"{
  "@context": {
    "ex": "http://example.org/",
    "foaf": "http://xmlns.com/foaf/0.1/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "schema": "https://schema.org/",
    "xsd": "http://www.w3.org/2001/XMLSchema#",
    "name": "foaf:name",
    "knows": {
      "@id": "foaf:knows",
      "@type": "@id"
    },
    "born": {
      "@id": "schema:birthDate",
      "@type": "xsd:date"
    },
    "comment": {
      "@id": "rdfs:comment",
      "@language": "en"
    },
    "address": "schema:address",
    "skills": {
      "@id": "ex:skills",
      "@container": "@list"
    }
  },
  "@graph": ["#,
    );
    for i in 0..n {
        let sep = if i + 1 < n { "," } else { "" };
        out += &format!(
            r#"
    {{
      "@id": "ex:p{tag}{i}",
      "@type": "foaf:Person",
      "name": "Person {i}",
      "born": "19{:02}-01-01",
      "comment": "A longer description of person {i}",
      "knows": [
        "ex:p{tag}{}",
        "ex:p{tag}{}"
      ],
      "address": {{
        "@type": "schema:PostalAddress",
        "schema:addressLocality": "Town {}"
      }},
      "skills": [
        "rdf",
        "sparql",
        "skill-{}"
      ]
    }}{sep}"#,
            50 + i % 50,
            (i + 1) % n,
            (i + 7) % n,
            i % 100,
            i % 13,
        );
    }
    out += "\n  ]\n}\n";
    out
}

/// Parse, index and diagnose `text` as the open document `uri`; the number
/// of diagnostics.
fn check(k: &mut Knowledge, uri: &str, text: &str, syntax: Syntax) -> usize {
    let a = Arc::new(Analysis::with_syntax(
        uri.to_string(),
        text,
        Arc::new(PrefixMap::default()),
        true,
        syntax,
    ));
    k.set(uri, Source::Open, a.clone());
    diagnostics(&a, k, Encoding::Utf16).len()
}

fn median(mut xs: Vec<Duration>) -> Duration {
    xs.sort();
    xs[xs.len() / 2]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// The median time to open a new document of `syntax`, each run with new
/// IRIs in a fresh workspace.
fn open(syntax: Syntax, make: fn(usize, &str) -> String, ext: &str) -> Duration {
    let times = (0..RUNS)
        .map(|run| {
            let text = make(LINES, &format!("o{run}_"));
            let mut k = Knowledge::default();
            let t = Instant::now();
            check(
                &mut k,
                &format!("file:///bench/open{run}.{ext}"),
                &text,
                syntax,
            );
            t.elapsed()
        })
        .collect();
    median(times)
}

#[test]
fn json_ld_meets_its_budgets_on_a_30000_line_document() {
    let doc = jsonld(LINES, "");
    assert!(
        doc.lines().count() >= LINES,
        "{} lines",
        doc.lines().count()
    );
    assert!(turtle(LINES, "").lines().count() >= LINES);

    // Warm up: the bundled vocabularies load on first use.
    check(
        &mut Knowledge::default(),
        "file:///bench/warm.ttl",
        &turtle(600, "w"),
        Syntax::Turtle,
    );
    check(
        &mut Knowledge::default(),
        "file:///bench/warm.jsonld",
        &jsonld(600, "w"),
        Syntax::JsonLd,
    );

    let turtle_open = open(Syntax::Turtle, turtle, "ttl");
    let jsonld_open = open(Syntax::JsonLd, jsonld, "jsonld");

    // Typing in a name, one character per keystroke.
    let uri = "file:///bench/big.jsonld";
    let mut k = Knowledge::default();
    let mut text = doc;
    check(&mut k, uri, &text, Syntax::JsonLd);
    let keystrokes = (0..15)
        .map(|i| {
            text = text.replacen(&format!("\"Person {i}\""), &format!("\"Person {i}x\""), 1);
            let t = Instant::now();
            check(&mut k, uri, &text, Syntax::JsonLd);
            t.elapsed()
        })
        .collect();
    let keystroke = median(keystrokes);

    eprintln!(
        "open: Turtle {:.1} ms, JSON-LD {:.1} ms ({:.2}x, budget 3x); \
         JSON-LD keystroke {:.1} ms (budget 10 ms)",
        ms(turtle_open),
        ms(jsonld_open),
        ms(jsonld_open) / ms(turtle_open),
        ms(keystroke)
    );
    assert!(
        jsonld_open <= turtle_open * 3,
        "opening JSON-LD took {:.1} ms, over 3x Turtle's {:.1} ms",
        ms(jsonld_open),
        ms(turtle_open)
    );
    assert!(
        keystroke < Duration::from_millis(10),
        "a JSON-LD keystroke took {:.1} ms (budget 10 ms)",
        ms(keystroke)
    );
}
