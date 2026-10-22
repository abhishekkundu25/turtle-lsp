//! JSON-LD 1.1: a frontend that turns a `.jsonld` document into the same
//! [`Parsed`] Turtle and TriG produce, so knowledge, lints, SHACL, the CLI
//! and the browser engine need no JSON-LD-specific code.
//!
//! The pipeline, one module per stage:
//! - [`json`]: a fault-tolerant JSON parser keeping the span of every key,
//!   value and bracket ([`json::Json`]), reporting `json-syntax` errors;
//! - [`loader`]: where remote `@context` documents come from
//!   ([`ContextLoader`]);
//! - [`context`]: context processing (the active context and its term
//!   definitions), with errors on the offending definition;
//! - [`expand`] and [`to_rdf`]: expansion and deserialization to triples and
//!   named graphs, each triple keeping the spans of the key, `@id` or value
//!   that produced it;
//! - [`lint`]: JSON-LD-specific warnings (undefined keys, invalid IRIs and
//!   language tags, compact IRIs whose prefix no context defines).
//!
//! What a JSON-LD [`Parsed`] holds:
//! - `syntax` is [`Syntax::JsonLd`] and `document_base` the base passed in;
//! - `triples`: `p_span` is the key that gave the predicate, `o_span` the
//!   value that gave the object; `statement` indexes `statements`, one per
//!   node object that has triples, whose span is the object and whose
//!   `subject` is its IRI `@id` (statements nest as node objects do);
//! - `graph` is the named graph (`@graph` with an `@id`), as in TriG;
//! - `refs` are the IRIs written in the document (an `@id`, a key, a type)
//!   with their roles; `errors` carry `json-syntax` and `jsonld-context`
//!   problems, and the warnings of [`json`] (`json-duplicate-key`,
//!   `json-lone-surrogate`);
//! - `prefixes`, `prefix_uses`, `graphs` and `comments` stay empty: JSON-LD
//!   prefixes live in `@context`, and JSON has no comments or graph blocks;
//! - `tokens` stays empty: its kinds are Turtle's. Spans for highlighting
//!   come from the [`json::Json`] tree.
//!
//! Spans: an IRI or term written in a JSON string (a key, an `@id`, a
//! `@type` value, a context entry, a language tag) covers the string's
//! content without the quotes; a literal covers its whole JSON token. A
//! context error sits on the key of the term whose definition is wrong.
//!
//! [`parse_document`] also returns what JSON-LD drops silently ([`Dropped`]:
//! undefined keys, relative or invalid IRIs, ill-formed language tags) and
//! which references are terms whose IRI comes from a definition
//! ([`UsedTerm`]), for the JSON-LD lints, and the [`SourceIndex`] of the
//! text ([`source`]: each key, `@type` and `@id` value with how it is
//! written, the term definitions they use, the context inside each object),
//! for the editor features.

pub(crate) mod context;
pub(crate) mod context_edit;
pub(crate) mod expand;
pub mod format;
pub mod json;
pub(crate) mod lint;
pub(crate) mod loader;
pub(crate) mod rename;
pub(crate) mod source;
pub(crate) mod to_rdf;

pub use loader::{ContextLoader, LoadError, NoNetwork, RemoteDocument};
pub(crate) use source::SourceIndex;

use crate::syntax::{Parsed, Span, Syntax};

/// Something JSON-LD processing leaves out of the graph without an error,
/// and where it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Dropped {
    /// The key or the string (without quotes) that is dropped.
    pub span: Span,
    pub(crate) reason: DropReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DropReason {
    /// A key the active context does not define: not a term, a compact IRI,
    /// an absolute IRI or a keyword, and no `@vocab` to map it.
    UndefinedTerm,
    /// A key whose term the context maps to `null`, on purpose.
    NullTerm,
    /// A key, `@id` or `@type` value of the form `@word` that is not a
    /// keyword (reserved, so ignored).
    KeywordLike,
    /// An IRI (an `@id`, a type, a datatype, a key or a graph name) that is
    /// not a well-formed absolute IRI: the triples it is in are dropped.
    InvalidIri,
    /// An IRI that is still relative: the document has no base to resolve it
    /// against. Its triples are dropped.
    RelativeIri,
    /// A language tag that is not well-formed (BCP 47): its literal is
    /// dropped. Only tags written in the document are recorded, not a
    /// context's `@language`.
    InvalidLanguageTag,
    /// A key that maps to a blank node: RDF has no blank node predicates.
    BlankNodePredicate,
}

/// A key, a type or a `@vocab`-coerced value written as a term of the
/// active context (not a keyword alias) whose IRI the term itself does not
/// spell: the IRI comes from its definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct UsedTerm {
    /// The term as written, without quotes: the span of its reference.
    pub(crate) span: Span,
    /// Where the definition's IRI is written in this document (its content,
    /// without quotes); `None` for a term of a remote context.
    pub(crate) definition: Option<Span>,
}

/// A parsed JSON-LD document: the shared parse model, what JSON-LD dropped
/// from it, and the terms it uses, each in text order, and its source
/// index.
#[derive(Debug, Default)]
pub(crate) struct Document {
    pub(crate) parsed: Parsed,
    pub(crate) dropped: Vec<Dropped>,
    pub(crate) used_terms: Vec<UsedTerm>,
    pub(crate) index: SourceIndex,
}

/// Parse a JSON-LD document into the shared parse model.
///
/// `base` is the document's URL: relative IRIs resolve against it until an
/// `@base` replaces it (`None` leaves them unresolved, as for an untitled
/// buffer). Remote contexts come from `loader`; one it cannot supply is an
/// error on the `@context` entry that names it, never a network access the
/// loader did not allow.
pub fn parse(text: &str, base: Option<&str>, loader: &dyn ContextLoader) -> Parsed {
    parse_document(text, base, loader).parsed
}

/// [`parse`], with the [`Dropped`] records. Remote contexts are processed
/// again for each call: use [`parse_document_with`] to share a cache.
pub(crate) fn parse_document(
    text: &str,
    base: Option<&str>,
    loader: &dyn ContextLoader,
) -> Document {
    parse_document_with(text, base, loader, &context::ContextCache::new())
}

/// [`parse_document`], keeping processed remote contexts in `cache`, which
/// the caller owns and may share between documents.
pub(crate) fn parse_document_with(
    text: &str,
    base: Option<&str>,
    loader: &dyn ContextLoader,
    cache: &context::ContextCache,
) -> Document {
    let doc = json::parse(text);
    let mut parsed = Parsed {
        syntax: Syntax::JsonLd,
        document_base: base.map(str::to_string),
        errors: doc.errors,
        ..Parsed::default()
    };
    parsed.errors.extend(doc.warnings);
    let mut dropped = Vec::new();
    let mut used_terms = Vec::new();
    let mut index = SourceIndex::default();
    if let Some(root) = &doc.root {
        let mut processor = context::Processor::with_cache(loader, cache);
        let expanded = expand::expand(root, base, &mut processor);
        parsed.errors.extend(expanded.errors);
        parsed.errors.extend(processor.take_warnings());
        dropped = expanded.dropped;
        used_terms = expanded.used_terms;
        index = expanded.index;
        to_rdf::to_rdf(&expanded.items, &mut parsed, &mut dropped);
    }
    // The same scoped context can fail at each use: report each error once.
    let mut seen = rustc_hash::FxHashSet::default();
    parsed
        .errors
        .retain(|e| seen.insert((e.span, e.message().to_string())));
    parsed.errors.sort_by_key(|e| (e.span.start, e.span.end));
    dropped.sort_unstable_by_key(|d| (d.span.start, d.span.end, d.reason));
    dropped.dedup();
    used_terms.sort_unstable_by_key(|u| (u.span.start, u.span.end));
    used_terms.dedup();
    Document {
        parsed,
        dropped,
        used_terms,
        index,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{Form, GraphName, Node, Role, Severity::*};

    const BASE: &str = "file:///x/a.jsonld";

    include!("../../tests/support/jsonld_people.rs");

    /// FNV-1a of the bytes of `text`.
    fn fnv1a(text: &str) -> u64 {
        text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    }

    /// The Rust and the JavaScript generators of the budget document write
    /// the same bytes: `test/jsonld.test.mjs` asserts the same digest of
    /// `bench/jsonld-gen.mjs`'s 600-line document tagged `w`.
    #[test]
    fn the_budget_document_is_the_one_the_bench_writes() {
        let doc = people_jsonld(600, "w");
        assert_eq!(doc.lines().count(), 609);
        assert_eq!(format!("{:016x}", fnv1a(&doc)), "bbc6c5ea77801ed1");
        // It is JSON-LD: no errors, one node a person.
        let p = parse(&doc, Some(BASE), &NoNetwork::default());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(
            doc.matches("\"@type\": \"foaf:Person\"").count(),
            people_count(600)
        );
        // Lines come in whole people, from the template alone.
        assert_eq!(people_count(30_000), 1499);
        assert_eq!(people_jsonld(30_000, "").lines().count(), 30_009);
    }

    fn at(text: &str, span: Span) -> &str {
        &text[span.start as usize..span.end as usize]
    }

    /// Each triple as (subject, predicate, object) in N-Triples-like text,
    /// with the text its predicate and object spans cover.
    fn triples(text: &str, p: &Parsed) -> Vec<(String, String, String, String, String)> {
        let node = |n: &Node| match n {
            Node::Iri(i) => format!("<{i}>"),
            Node::Blank(b) => format!("_:b{}", b.0),
            Node::Literal {
                value,
                datatype,
                lang,
                ..
            } => match lang {
                Some(l) => format!("{value:?}@{l}"),
                None => format!("{value:?}^^<{datatype}>"),
            },
            Node::Triple(_) => unreachable!(),
        };
        p.triples
            .iter()
            .map(|t| {
                (
                    node(&t.s),
                    t.p.to_string(),
                    node(&t.o),
                    at(text, t.p_span).to_string(),
                    at(text, t.o_span).to_string(),
                )
            })
            .collect()
    }

    fn errors<'t>(text: &'t str, p: &Parsed) -> Vec<(&'static str, &'t str)> {
        p.errors
            .iter()
            .map(|e| (e.code, at(text, e.span)))
            .collect()
    }

    #[test]
    fn triples_point_at_the_key_the_id_and_the_value() {
        let text = r#"{
  "@context": {
    "ex": "http://ex.org/",
    "knows": {"@id": "ex:knows", "@type": "@id"},
    "name": "ex:name"
  },
  "@id": "ex:a",
  "@type": "ex:Person",
  "name": "Ann",
  "ex:age": 41,
  "knows": "ex:b",
  "ex:partOf": {"@id": "ex:g", "ex:size": 2.5},
  "ex:with": {"ex:label": {"@value": "x", "@language": "EN"}}
}"#;
        let p = parse(text, Some(BASE), &NoNetwork::default());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let got = triples(text, &p);
        let row = |p: &str| got.iter().find(|t| t.1 == p).cloned().unwrap();
        let ty = row(crate::syntax::RDF_TYPE);
        assert_eq!(
            (ty.2.as_str(), ty.3.as_str(), ty.4.as_str()),
            ("<http://ex.org/Person>", "@type", "ex:Person")
        );
        let name = row("http://ex.org/name");
        assert_eq!((name.3.as_str(), name.4.as_str()), ("name", "\"Ann\""));
        let age = row("http://ex.org/age");
        assert_eq!(age.2, "\"41\"^^<http://www.w3.org/2001/XMLSchema#integer>");
        assert_eq!((age.3.as_str(), age.4.as_str()), ("ex:age", "41"));
        let knows = row("http://ex.org/knows");
        assert_eq!(
            (knows.2.as_str(), knows.4.as_str()),
            ("<http://ex.org/b>", "ex:b")
        );
        // A nested node object: the object is its braces, its own triples
        // have their own statement, whose subject is its @id.
        let part = row("http://ex.org/partOf");
        assert_eq!(part.4, r#"{"@id": "ex:g", "ex:size": 2.5}"#);
        let size = row("http://ex.org/size");
        assert_eq!(
            (size.0.as_str(), size.2.as_str()),
            (
                "<http://ex.org/g>",
                "\"2.5E0\"^^<http://www.w3.org/2001/XMLSchema#double>"
            )
        );
        let label = row("http://ex.org/label");
        assert_eq!(
            (label.2.as_str(), label.4.as_str()),
            ("\"x\"@en", r#"{"@value": "x", "@language": "EN"}"#)
        );
        // Statements: the document, ex:g, and the blank node of ex:with.
        let subjects: Vec<_> = p
            .statements
            .iter()
            .map(|s| s.subject.map(|(i, span)| (i.to_string(), at(text, span))))
            .collect();
        assert_eq!(
            subjects,
            [
                Some(("http://ex.org/a".to_string(), "ex:a")),
                Some(("http://ex.org/g".to_string(), "ex:g")),
                None
            ]
        );
        assert!(
            at(text, p.statements[0].span).starts_with('{')
                && at(text, p.statements[2].span).starts_with("{\"ex:label\"")
        );
        let stmt_of = |pred: &str| p.triples.iter().find(|t| t.p == pred).unwrap().statement;
        assert_eq!(
            (
                stmt_of("http://ex.org/name"),
                stmt_of("http://ex.org/size"),
                stmt_of("http://ex.org/label")
            ),
            (0, 1, 2)
        );
        // References, in text order, with roles and forms.
        let refs: Vec<_> = p
            .refs
            .iter()
            .map(|r| (at(text, r.span), r.role, r.form))
            .collect();
        let pn = |n| Form::PName { prefix_len: n };
        assert_eq!(
            refs,
            [
                ("ex:a", Role::Subject, pn(2)),
                ("@type", Role::Predicate, Form::A),
                ("ex:Person", Role::Object, pn(2)),
                ("name", Role::Predicate, Form::Iri),
                ("ex:age", Role::Predicate, pn(2)),
                ("knows", Role::Predicate, Form::Iri),
                ("ex:b", Role::Object, pn(2)),
                ("ex:partOf", Role::Predicate, pn(2)),
                ("ex:g", Role::Subject, pn(2)),
                ("ex:size", Role::Predicate, pn(2)),
                ("ex:with", Role::Predicate, pn(2)),
                ("ex:label", Role::Predicate, pn(2)),
            ]
        );
        assert_eq!(
            (p.syntax, p.document_base.as_deref()),
            (Syntax::JsonLd, Some(BASE))
        );
        assert!(p.tokens.is_empty() && p.prefixes.is_empty() && p.graphs.is_empty());
    }

    #[test]
    fn named_graphs_lists_reverse_nest_included_and_json() {
        let text = r#"{
  "@context": {
    "@vocab": "http://ex.org/",
    "tags": {"@container": "@list"},
    "parentOf": {"@reverse": "http://ex.org/childOf"},
    "meta": "@nest",
    "data": {"@type": "@json"}
  },
  "@id": "http://ex.org/g",
  "@graph": [
    {
      "@id": "http://ex.org/a",
      "tags": ["x", 1],
      "parentOf": {"@id": "http://ex.org/c"},
      "meta": {"note": "nested"},
      "data": {"b": 1, "a": [true, null]},
      "@included": [{"@id": "http://ex.org/i", "note": "inc"}],
      "empty": {"@list": []},
      "rel": {"@id": "_:n"},
      "back": {"@id": "_:n"}
    }
  ]
}"#;
        let p = parse(text, Some(BASE), &NoNetwork::default());
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let g = GraphName::Iri(crate::syntax::Iri::intern("http://ex.org/g"));
        assert!(p.triples.iter().all(|t| t.graph == Some(g)));
        let got = triples(text, &p);
        let has =
            |s: &str, pr: &str, o: &str| got.iter().any(|t| t.0 == s && t.1 == pr && t.2 == o);
        let first = "http://www.w3.org/1999/02/22-rdf-syntax-ns#first";
        let rest = "http://www.w3.org/1999/02/22-rdf-syntax-ns#rest";
        assert_eq!(
            got.iter().filter(|t| t.1 == first || t.1 == rest).count(),
            4
        );
        let x = got
            .iter()
            .find(|t| t.1 == first && t.2.starts_with("\"x\""))
            .unwrap();
        assert_eq!((x.3.as_str(), x.4.as_str()), ("\"x\"", "\"x\""));
        assert!(has(
            "<http://ex.org/a>",
            "http://ex.org/empty",
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#nil>"
        ));
        // @reverse: the value is the subject; the predicate span is the key.
        let rev = got.iter().find(|t| t.1 == "http://ex.org/childOf").unwrap();
        assert_eq!(
            (rev.0.as_str(), rev.2.as_str(), rev.3.as_str()),
            ("<http://ex.org/c>", "<http://ex.org/a>", "parentOf")
        );
        assert!(has(
            "<http://ex.org/a>",
            "http://ex.org/note",
            "\"nested\"^^<http://www.w3.org/2001/XMLSchema#string>"
        ));
        assert!(has(
            "<http://ex.org/i>",
            "http://ex.org/note",
            "\"inc\"^^<http://www.w3.org/2001/XMLSchema#string>"
        ));
        assert!(has(
            "<http://ex.org/a>",
            "http://ex.org/data",
            "\"{\\\"a\\\":[true,null],\\\"b\\\":1}\"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#JSON>"
        ));
        // The same blank node label is one node.
        let rel = got.iter().find(|t| t.1 == "http://ex.org/rel").unwrap();
        let back = got.iter().find(|t| t.1 == "http://ex.org/back").unwrap();
        assert!(rel.2.starts_with("_:b") && rel.2 == back.2);
        // The graph name is a Graph reference.
        let r = p
            .refs
            .iter()
            .find(|r| at(text, r.span) == "http://ex.org/g")
            .unwrap();
        assert_eq!(r.role, Role::Graph);
    }

    #[test]
    fn what_json_ld_drops_is_recorded() {
        let text = r#"{
  "@context": {"ex": "http://ex.org/", "gone": null, "see": {"@id": "ex:see", "@type": "@id"}},
  "@id": "ex:a",
  "wieght": 4,
  "gone": 1,
  "@foo": 2,
  "see": "http://exa mple.org/x",
  "ex:rel": {"@id": "relative"},
  "ex:c": {"@value": "Hallo", "@language": "de_DE"},
  "ex:ok": "kept"
}"#;
        let d = parse_document(text, None, &NoNetwork::default());
        let got: Vec<_> = d
            .dropped
            .iter()
            .map(|x| (at(text, x.span), x.reason))
            .collect();
        use DropReason::*;
        assert_eq!(
            got,
            [
                ("wieght", UndefinedTerm),
                ("gone", NullTerm),
                ("@foo", KeywordLike),
                ("http://exa mple.org/x", InvalidIri),
                ("relative", RelativeIri),
                ("de_DE", InvalidLanguageTag),
            ]
        );
        assert_eq!(d.parsed.triples.len(), 1);
        assert!(d.parsed.errors.is_empty());
    }

    #[test]
    fn relative_iris_resolve_against_the_document_and_base() {
        let text = r#"{"@context": {"@vocab": "http://v.org/"}, "@id": "b", "p": {"@id": "../c"}, "q": {"@context": {"@base": "http://other.org/x/"}, "@id": "d"}}"#;
        let p = parse(
            text,
            Some("http://ex.org/dir/a.jsonld"),
            &NoNetwork::default(),
        );
        let got = triples(text, &p);
        assert_eq!(got[0].0, "<http://ex.org/dir/b>");
        assert!(got.iter().any(|t| t.2 == "<http://ex.org/c>"));
        assert!(got.iter().any(|t| t.2 == "<http://other.org/x/d>"));
    }

    #[test]
    fn context_errors_sit_on_the_term_key_once() {
        // The fixtures of test/jsonld-fixtures.mjs.
        let text = "{\n  \"@context\": {\n    \"ex\": \"https://jsonld.example.org/def/\",\n    \"weight\": 5\n  },\n  \"@id\": \"ex:w10\",\n  \"ex:colour\": \"red\"\n}\n";
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(errors(text, &p), [("jsonld-context", "weight")]);
        assert!(
            p.errors[0]
                .message()
                .starts_with("invalid term definition: ")
        );
        let text = "{\n  \"@context\": {\n    \"a\": \"b:x\",\n    \"b\": \"a:y\"\n  },\n  \"@id\": \"https://jsonld.example.org/def/w15\",\n  \"a\": 1\n}\n";
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(p.errors.len(), 1);
        assert!(["a", "b"].contains(&at(text, p.errors[0].span)));
        // A protected term redefined in a nested node's context.
        let text = "{\n  \"@context\": {\n    \"ex\": \"https://jsonld.example.org/def/\",\n    \"label\": { \"@id\": \"http://www.w3.org/2000/01/rdf-schema#label\", \"@protected\": true }\n  },\n  \"@id\": \"ex:w16\",\n  \"ex:partOf\": {\n    \"@context\": { \"label\": \"ex:name\" },\n    \"@id\": \"ex:g16\"\n  }\n}\n";
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(errors(text, &p), [("jsonld-context", "label")]);
        assert_eq!(p.errors[0].span, Span::new(212, 217));
        // The same failing scoped context used twice is one error.
        let text = r#"{"@context": {"T": {"@id": "http://t.org/T", "@context": "http://nowhere.example/c"}}, "@graph": [{"@type": "T"}, {"@type": "T"}]}"#;
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(errors(text, &p), [("jsonld-context", "T")]);
    }

    #[test]
    fn expansion_errors_carry_the_w3c_code() {
        let text = r#"{"@id": 5, "http://ex.org/p": {"@value": "x", "@language": "en", "@type": "http://t/"}}"#;
        let p = parse(text, None, &NoNetwork::default());
        let got: Vec<_> = p
            .errors
            .iter()
            .map(|e| {
                (
                    at(text, e.span),
                    e.message().split(':').next().unwrap(),
                    e.code,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("5", "invalid @id value", "jsonld-context"),
                ("{", "invalid value object", "jsonld-context"),
            ]
        );
    }

    #[test]
    fn top_level_context_errors_are_diagnostics() {
        let text = r#"[{"@context": {"ok": "http://ex.org/"}}, {"@context": {"a": 5, "@b": "http://x/"}}, {"@context": "https://example.org/ctx"}]"#;
        let p = parse(text, Some(BASE), &NoNetwork::default());
        let got: Vec<_> = p
            .errors
            .iter()
            .map(|e| {
                (
                    e.code,
                    e.severity,
                    at(text, e.span),
                    e.message().split(':').next().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("jsonld-context", Error, "a", "invalid term definition"),
                (
                    "jsonld-context",
                    Error,
                    "https://example.org/ctx",
                    "loading remote context failed"
                ),
            ]
        );
        assert!(p.errors[1].message().contains("imports.network"));
        // Processing stopped at `"a": 5` before `"@b"`; alone, `"@b"` is a
        // warning.
        let text = r#"{"@context": {"@b": "http://x/"}}"#;
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(p.errors.len(), 1);
        assert_eq!(
            (p.errors[0].severity, p.errors[0].span),
            (Warning, Span::new(15, 17))
        );
    }

    #[test]
    fn the_turtle_entry_point_leaves_jsonld_to_this_frontend() {
        let text = "{\"@id\": \"ex:a\"";
        let p = crate::syntax::parse_with_base(
            text,
            &crate::syntax::PrefixMap::default(),
            Syntax::JsonLd,
            Some(BASE),
        );
        assert_eq!(p.syntax, Syntax::JsonLd);
        assert_eq!(p.document_base.as_deref(), Some(BASE));
        assert!(p.triples.is_empty() && p.statements.is_empty() && p.tokens.is_empty());
        let errors: Vec<_> = p
            .errors
            .iter()
            .map(|e| (e.code, e.severity, e.span, e.message()))
            .collect();
        assert_eq!(
            errors,
            [(
                "syntax",
                Error,
                Span::new(0, 0),
                "JSON-LD documents are parsed by jsonld::parse"
            )]
        );

        // Turtle would report the strings and the colon; JSON only the
        // unclosed `{`.
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(p.syntax, Syntax::JsonLd);
        let errors: Vec<_> = p.errors.iter().map(|e| (e.span, e.message())).collect();
        assert_eq!(errors, [(Span::new(0, 1), "Unclosed object: expected '}'")]);
    }

    /// The parse alone, run with `cargo test --release jsonld`: the
    /// 30,000-line document of bench/jsonld-gen.mjs (people with compact
    /// IRIs, @type, @id references, typed and language-tagged values, a
    /// nested blank node and a @list) against the same data in Turtle.
    /// Prints both times; fails when JSON-LD is over 4 times as slow. The
    /// parse also builds the source index, about 2.3x Turtle on a fast core
    /// and up to 3.1x on a slow CI runner; the 3x budget is on open to
    /// diagnostics, in tests/jsonld_budget.rs.
    #[cfg(not(debug_assertions))]
    #[test]
    fn parses_30000_lines_within_4x_of_turtle() {
        use std::fmt::Write as _;
        use std::time::{Duration, Instant};
        // The people of bench/jsonld-people.tpl, as the bench writes them
        // (tests/jsonld_budget.rs reads the same template).
        let json = people_jsonld(30_000, "");
        let n = people_count(30_000);
        let mut ttl = String::from(
            "@prefix ex: <http://example.org/> .\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix schema: <https://schema.org/> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n",
        );
        for i in 0..n {
            let (born, k1, k2, town, skill) = person_values(i, n);
            let _ = write!(
                ttl,
                "ex:p{i} a foaf:Person ;\n  foaf:name \"Person {i}\" ;\n  schema:birthDate \"19{born}-01-01\"^^xsd:date ;\n  rdfs:comment \"A longer description of person {i}\"@en ;\n  foaf:knows ex:p{k1} , ex:p{k2} ;\n  schema:address [ a schema:PostalAddress ; schema:addressLocality \"Town {town}\" ] ;\n  ex:skills ( \"rdf\" \"sparql\" \"skill-{skill}\" ) .\n\n",
            );
        }
        assert!(json.lines().count() >= 29_000, "{}", json.lines().count());
        let loader = NoNetwork::default();
        let prefixes = crate::syntax::PrefixMap::default();
        let j = parse(&json, Some(BASE), &loader);
        let t = crate::syntax::parse(&ttl, &prefixes, Syntax::Turtle);
        assert!(
            j.errors.is_empty() && t.errors.is_empty(),
            "{:?}",
            &j.errors[..1]
        );
        assert_eq!(j.triples.len(), t.triples.len());
        let time = |f: &dyn Fn()| {
            let t = Instant::now();
            f();
            t.elapsed()
        };
        // Alternated, so load from other tests running in parallel slows
        // both alike; the best of each is kept.
        let (mut jt, mut tt) = (Duration::MAX, Duration::MAX);
        for _ in 0..7 {
            jt = jt.min(time(&|| drop(parse(&json, Some(BASE), &loader))));
            tt = tt.min(time(&|| {
                drop(crate::syntax::parse(&ttl, &prefixes, Syntax::Turtle))
            }));
        }
        eprintln!(
            "JSON-LD {} lines, {} triples: {jt:?}; Turtle: {tt:?} ({:.2}x)",
            json.lines().count(),
            j.triples.len(),
            jt.as_secs_f64() / tt.as_secs_f64()
        );
        assert!(jt <= tt * 4, "JSON-LD {jt:?} vs Turtle {tt:?}");
    }

    #[test]
    fn the_deepest_json_expands_on_a_small_stack() {
        // As deep as the JSON parser goes (`MAX_DEPTH` levels in all, the
        // top-level object included), in objects, @list arrays and @reverse
        // maps, on the 2 MiB stack of a default thread (parsing threads get
        // 8 MiB, the browser engine 32).
        let depth = json::MAX_DEPTH as usize - 2;
        // `depth` (254) nested objects below the root: 255 levels of 256.
        let objects = format!(
            "{{\"@context\": {{\"@vocab\": \"http://ex.org/\"}}, \"@id\": \"http://ex.org/a\", {}\"p\": 1{}}}",
            "\"p\": {\"q\": 0, ".repeat(depth),
            "}".repeat(depth)
        );
        let lists = format!(
            "{{\"@context\": {{\"@vocab\": \"http://ex.org/\", \"l\": {{\"@container\": \"@list\"}}}}, \"l\": {}1{}}}",
            "[".repeat(depth),
            "]".repeat(depth)
        );
        // Each `@reverse` step is two objects.
        let reverse = format!(
            "{{\"@id\": \"http://ex.org/a\", {}\"@id\": \"http://ex.org/b\"{}}}",
            "\"@reverse\": {\"http://ex.org/p\": {".repeat(depth / 2),
            "}}".repeat(depth / 2)
        );
        let levels = |text: &str| {
            json::tokens(text)
                .iter()
                .fold((0usize, 0usize), |(d, max), t| match t.kind {
                    json::TokenKind::LBrace | json::TokenKind::LBracket => (d + 1, max.max(d + 1)),
                    json::TokenKind::RBrace | json::TokenKind::RBracket => (d - 1, max),
                    _ => (d, max),
                })
                .1
        };
        // The texts really nest as deep as the parser allows.
        assert_eq!(levels(&objects), depth + 1);
        assert_eq!(levels(&lists), depth + 1);
        assert_eq!(levels(&reverse), depth + 1);
        // Unoptimized frames are several times larger: a debug build gets
        // the 8 MiB the parsing threads have.
        let stack = if cfg!(debug_assertions) {
            8 << 20
        } else {
            2 << 20
        };
        std::thread::Builder::new()
            .stack_size(stack)
            .spawn(move || {
                for text in [objects, lists, reverse] {
                    let p = parse(&text, None, &NoNetwork::default());
                    assert!(
                        p.errors.iter().all(|e| e.code != "json-syntax"),
                        "{:?}",
                        p.errors
                    );
                    assert!(p.triples.len() >= depth / 2, "{}", p.triples.len());
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    /// The predicates of the triples whose predicate IRI is one of ours.
    fn evil_triples(p: &Parsed, subject: &str) -> usize {
        p.triples
            .iter()
            .filter(|t| {
                matches!(&t.s, Node::Iri(s) if s.as_str() == subject)
                    && t.p.as_str().starts_with("http://evil/")
            })
            .count()
    }

    /// `@graph` siblings, one with an inline object before a remote context
    /// and one with the remote context alone, share a cache: neither gets
    /// the other's terms, in either order, and neither does a second document.
    #[test]
    fn siblings_sharing_a_cache_keep_their_own_contexts() {
        let loader = NoNetwork::default().with(
            "http://example.com/credentials",
            r#"{"@context": {"cred": "http://cred.org/"}}"#,
        );
        let with_inline = r#"{"@context": [{"zz": "http://evil/zz"}, "http://example.com/credentials"], "@id": "http://ex.org/1", "zz": "v"}"#;
        let plain = r#"{"@context": "http://example.com/credentials", "@id": "http://ex.org/2", "zz": "v"}"#;
        for (first, second) in [(with_inline, plain), (plain, with_inline)] {
            let cache = context::ContextCache::new();
            let text = format!(
                r#"{{"@context": {{"outer": "http://outer.org/"}}, "@graph": [{first}, {second}]}}"#
            );
            let d = parse_document_with(&text, Some(BASE), &loader, &cache);
            assert_eq!(evil_triples(&d.parsed, "http://ex.org/1"), 1);
            assert_eq!(evil_triples(&d.parsed, "http://ex.org/2"), 0);
            let undefined = d
                .dropped
                .iter()
                .filter(|x| x.reason == DropReason::UndefinedTerm)
                .count();
            assert_eq!(undefined, 1, "{:?}", d.dropped);
            // Another document, same cache.
            let other =
                format!(r#"{{"@context": {{"outer": "http://outer.org/"}}, "@graph": [{plain}]}}"#);
            let d = parse_document_with(&other, Some(BASE), &loader, &cache);
            assert_eq!(evil_triples(&d.parsed, "http://ex.org/2"), 0);
            assert_eq!(d.dropped.len(), 1, "{:?}", d.dropped);
        }
    }

    /// Contexts that include the one below twice, 20 levels deep, are one
    /// error, quickly: the work is capped.
    #[test]
    fn doubling_nested_contexts_are_one_error_not_a_hang() {
        let mut loader = NoNetwork::default().with(
            "http://example.com/c0",
            r#"{"@context": {"a": "http://a.org/"}}"#,
        );
        for i in 1..=20 {
            loader = loader.with(
                format!("http://example.com/c{i}"),
                format!(
                    r#"{{"@context": ["http://example.com/c{p}", "http://example.com/c{p}"]}}"#,
                    p = i - 1
                ),
            );
        }
        let text = r#"{"@context": "http://example.com/c20", "a": "x", "@id": "http://ex.org/1"}"#;
        let start = std::time::Instant::now();
        let d = parse_document(text, Some(BASE), &loader);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        let overflow: Vec<_> = d
            .parsed
            .errors
            .iter()
            .filter(|e| e.message().starts_with("context overflow"))
            .collect();
        assert_eq!(overflow.len(), 1, "{:?}", d.parsed.errors);
    }

    /// A `@json` literal with 100k keys is canonicalized in linear time.
    #[test]
    fn a_huge_json_literal_is_parsed_quickly() {
        let mut text = String::from(
            r#"{"@context": {"j": {"@id": "http://ex.org/j", "@type": "@json"}}, "@id": "http://ex.org/s", "j": {"#,
        );
        for i in 0..100_000 {
            if i > 0 {
                text.push(',');
            }
            text.push_str(&format!("\"k{i}\":{i}"));
        }
        text.push_str("}}");
        let start = std::time::Instant::now();
        let p = parse(&text, Some(BASE), &NoNetwork::default());
        let took = start.elapsed();
        assert_eq!(p.triples.len(), 1);
        if cfg!(not(debug_assertions)) {
            assert!(took < std::time::Duration::from_millis(500), "{took:?}");
        }
    }
}
