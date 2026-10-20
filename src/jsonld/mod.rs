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
//!   problems;
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

pub mod context;
pub mod context_edit;
pub mod expand;
pub mod format;
pub mod json;
pub mod lint;
pub mod loader;
pub mod rename;
pub mod source;
pub mod to_rdf;

pub use loader::{ContextLoader, LoadError, NoNetwork, RemoteDocument};
pub use source::SourceIndex;

use crate::syntax::{Parsed, Span, Syntax};

/// Something JSON-LD processing leaves out of the graph without an error,
/// and where it is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Dropped {
    /// The key or the string (without quotes) that is dropped.
    pub span: Span,
    pub reason: DropReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DropReason {
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
pub struct UsedTerm {
    /// The term as written, without quotes: the span of its reference.
    pub span: Span,
    /// Where the definition's IRI is written in this document (its content,
    /// without quotes); `None` for a term of a remote context.
    pub definition: Option<Span>,
}

/// A parsed JSON-LD document: the shared parse model, what JSON-LD dropped
/// from it, and the terms it uses, each in text order, and its source
/// index.
#[derive(Debug, Default)]
pub struct Document {
    pub parsed: Parsed,
    pub dropped: Vec<Dropped>,
    pub used_terms: Vec<UsedTerm>,
    pub index: SourceIndex,
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

/// [`parse`], with the [`Dropped`] records.
pub fn parse_document(text: &str, base: Option<&str>, loader: &dyn ContextLoader) -> Document {
    let doc = json::parse(text);
    let mut parsed = Parsed {
        syntax: Syntax::JsonLd,
        document_base: base.map(str::to_string),
        errors: doc.errors,
        ..Parsed::default()
    };
    let mut dropped = Vec::new();
    let mut used_terms = Vec::new();
    let mut index = SourceIndex::default();
    if let Some(root) = &doc.root {
        let mut processor = context::Processor::new(loader);
        let expanded = expand::expand(root, base, &mut processor);
        parsed.errors.extend(expanded.errors);
        parsed.errors.extend(processor.take_warnings());
        dropped = expanded.dropped;
        used_terms = expanded.used_terms;
        index = expanded.index;
        index.context_iris = lint::context_iris(root);
        to_rdf::to_rdf(&expanded.items, &mut parsed, &mut dropped);
    }
    // The same scoped context can fail at each use: report each error once.
    let mut seen = rustc_hash::FxHashSet::default();
    parsed
        .errors
        .retain(|e| seen.insert((e.span, e.message.clone())));
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

/// The content of a JSON string written at `span`, without its quotes.
pub(crate) fn unquoted(span: Span) -> Span {
    if span.end >= span.start + 2 {
        Span::new(span.start + 1, span.end - 1)
    } else {
        span
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{Form, GraphName, Node, Role, Severity::*};

    const BASE: &str = "file:///x/a.jsonld";

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
        assert!(p.errors[0].message.starts_with("invalid term definition: "));
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
                    e.message.split(':').next().unwrap(),
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
                    e.message.split(':').next().unwrap(),
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
        assert!(p.errors[1].message.contains("imports.network"));
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
    fn the_turtle_entry_point_hands_jsonld_to_this_frontend() {
        // Turtle would report the strings and the colon; JSON only the
        // unclosed `{`.
        let p = crate::syntax::parse_with_base(
            "{\"@id\": \"ex:a\"",
            &crate::syntax::PrefixMap::default(),
            Syntax::JsonLd,
            None,
        );
        assert_eq!(p.syntax, Syntax::JsonLd);
        let errors: Vec<_> = p
            .errors
            .iter()
            .map(|e| (e.span, e.message.as_str()))
            .collect();
        assert_eq!(
            errors,
            [(
                crate::syntax::Span::new(0, 1),
                "Unclosed object: expected '}'"
            )]
        );
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
        let context = r#"{
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
  }"#;
        // 20 lines a person, as bench/jsonld-gen.mjs writes them.
        let n = (30_000 - 30) / 20;
        let mut json = format!("{{\n  \"@context\": {context},\n  \"@graph\": [\n");
        let mut ttl = String::from(
            "@prefix ex: <http://example.org/> .\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix schema: <https://schema.org/> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\n",
        );
        for i in 0..n {
            let born = format!("19{:02}-01-01", 50 + i % 50);
            let (k1, k2) = ((i + 1) % n, (i + 7) % n);
            let sep = if i + 1 < n { "," } else { "" };
            let _ = write!(
                json,
                "    {{\n      \"@id\": \"ex:p{i}\",\n      \"@type\": \"foaf:Person\",\n      \"name\": \"Person {i}\",\n      \"born\": \"{born}\",\n      \"comment\": \"A longer description of person {i}\",\n      \"knows\": [\n        \"ex:p{k1}\",\n        \"ex:p{k2}\"\n      ],\n      \"address\": {{\n        \"@type\": \"schema:PostalAddress\",\n        \"schema:addressLocality\": \"Town {}\"\n      }},\n      \"skills\": [\n        \"rdf\",\n        \"sparql\",\n        \"skill-{}\"\n      ]\n    }}{sep}\n",
                i % 100,
                i % 13
            );
            let _ = write!(
                ttl,
                "ex:p{i} a foaf:Person ;\n  foaf:name \"Person {i}\" ;\n  schema:birthDate \"{born}\"^^xsd:date ;\n  rdfs:comment \"A longer description of person {i}\"@en ;\n  foaf:knows ex:p{k1} , ex:p{k2} ;\n  schema:address [ a schema:PostalAddress ; schema:addressLocality \"Town {}\" ] ;\n  ex:skills ( \"rdf\" \"sparql\" \"skill-{}\" ) .\n\n",
                i % 100,
                i % 13
            );
        }
        json.push_str("  ]\n}\n");
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
        // As deep as the JSON parser goes, in objects, @list arrays and
        // @reverse maps, on the 2 MiB stack of a default thread (parsing
        // threads get 8 MiB, the browser engine 32).
        let depth = json::MAX_DEPTH as usize - 2;
        let objects = format!(
            "{{\"@context\": {{\"@vocab\": \"http://ex.org/\"}}, \"@id\": \"http://ex.org/a\", {}\"p\": 1{}}}",
            "\"p\": {\"q\": 0, ".repeat(depth / 2),
            "}".repeat(depth / 2)
        );
        let lists = format!(
            "{{\"@context\": {{\"@vocab\": \"http://ex.org/\", \"l\": {{\"@container\": \"@list\"}}}}, \"l\": {}1{}}}",
            "[".repeat(depth),
            "]".repeat(depth)
        );
        let reverse = format!(
            "{{\"@id\": \"http://ex.org/a\", {}\"@id\": \"http://ex.org/b\"{}}}",
            "\"@reverse\": {\"http://ex.org/p\": {".repeat(depth / 3),
            "}}".repeat(depth / 3)
        );
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                for text in [objects, lists, reverse] {
                    let p = parse(&text, None, &NoNetwork::default());
                    assert!(
                        p.errors.iter().all(|e| e.code != "json-syntax"),
                        "{:?}",
                        p.errors
                    );
                    assert!(p.triples.len() >= depth / 3, "{}", p.triples.len());
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
