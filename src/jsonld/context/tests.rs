use super::*;
use crate::jsonld::NoNetwork;
use crate::jsonld::json;

const BASE: &str = "http://example.com/doc.jsonld";

fn root(text: &str) -> Json {
    let doc = json::parse(text);
    assert!(doc.errors.is_empty(), "{:?}", doc.errors);
    doc.root.unwrap()
}

fn context_of(doc: &Json) -> &Json {
    &doc.as_object().unwrap().get("@context").unwrap().value
}

/// Process the `@context` of `text` with a private cache.
fn process_with(text: &str, loader: &dyn ContextLoader) -> Result<ActiveContext, ContextError> {
    let cache = ContextCache::new();
    let doc = root(text);
    Processor::with_cache(loader, &cache).process(
        &ActiveContext::new(Some(BASE)),
        context_of(&doc),
        Some(BASE),
        Options::default(),
    )
}

fn process(text: &str) -> Result<ActiveContext, ContextError> {
    process_with(text, &NoNetwork::default())
}

fn ok(text: &str) -> ActiveContext {
    process(text).unwrap_or_else(|e| panic!("{e}"))
}

fn iri<'a>(c: &'a ActiveContext, term: &str) -> Option<&'a str> {
    c.term(term).and_then(|d| d.iri.as_deref())
}

fn spanned(text: &str, span: Span) -> String {
    text[span.start as usize..span.end as usize].to_string()
}

/// The error's code and the text its span covers.
fn fails(text: &str) -> (ErrorCode, String) {
    fails_with(text, &NoNetwork::default())
}

fn fails_with(text: &str, loader: &dyn ContextLoader) -> (ErrorCode, String) {
    let e = process_with(text, loader).expect_err("an error");
    (e.code, spanned(text, e.span))
}

#[test]
fn simple_and_expanded_term_definitions() {
    let c = ok(r#"{"@context": {
            "ex": "http://ex.org/",
            "name": {"@id": "http://ex.org/name", "@type": "@id", "@container": "@set"},
            "label": {"@id": "ex:label", "@language": "en", "@direction": "rtl"},
            "nothing": null
        }}"#);
    assert_eq!(iri(&c, "ex"), Some("http://ex.org/"));
    assert!(c.term("ex").unwrap().prefix);
    let name = c.term("name").unwrap();
    assert_eq!(name.type_mapping.as_deref(), Some("@id"));
    assert_eq!(name.container, Container::SET);
    assert!(!name.prefix);
    let label = c.term("label").unwrap();
    assert_eq!(label.iri.as_deref(), Some("http://ex.org/label"));
    assert_eq!(label.language, Some(Some(Arc::from("en"))));
    assert_eq!(label.direction, Some(Some(Direction::Rtl)));
    assert_eq!(c.term("nothing").unwrap().iri, None);
    assert_eq!(expand_iri(&c, "nothing", false, true), None);
}

#[test]
fn term_spans_point_at_their_keys() {
    let text = r#"{"@context": {"ex": "http://ex.org/"}}"#;
    let c = ok(text);
    assert_eq!(spanned(text, c.term("ex").unwrap().span.unwrap()), "ex");
}

#[test]
fn compact_iris_and_prefix_flags() {
    let c = ok(r#"{"@context": {
            "ex": "http://ex.org/",
            "noprefix": {"@id": "http://np.org/"},
            "forced": {"@id": "http://f.org/", "@prefix": true},
            "nodelim": "http://nd.org/x",
            "ex:thing": {"@type": "@id"},
            "bnode": "_:b"
        }}"#);
    let x = |v: &str| expand_iri(&c, v, false, true);
    assert_eq!(x("ex:a").as_deref(), Some("http://ex.org/a"));
    // Only prefix terms make compact IRIs.
    assert_eq!(x("noprefix:a").as_deref(), Some("noprefix:a"));
    assert_eq!(x("forced:a").as_deref(), Some("http://f.org/a"));
    assert_eq!(x("nodelim:a").as_deref(), Some("nodelim:a"));
    assert!(c.term("bnode").unwrap().prefix);
    // A compact-IRI term takes its IRI from its prefix.
    assert_eq!(iri(&c, "ex:thing"), Some("http://ex.org/thing"));
    // Blank nodes and IRIs with an authority are left alone.
    assert_eq!(x("_:x").as_deref(), Some("_:x"));
    assert_eq!(x("ex://a").as_deref(), Some("ex://a"));
}

#[test]
fn definitions_may_depend_on_later_ones() {
    let c = ok(r#"{"@context": {"name": "foaf:name", "foaf": "http://xmlns.com/foaf/0.1/"}}"#);
    assert_eq!(iri(&c, "name"), Some("http://xmlns.com/foaf/0.1/name"));
}

#[test]
fn vocab_and_base() {
    let c = ok(
        r#"{"@context": {"@vocab": "http://v.org/", "@base": "http://b.org/dir/", "term": {}}}"#,
    );
    assert_eq!(c.vocab(), Some("http://v.org/"));
    assert_eq!(c.base(), Some("http://b.org/dir/"));
    assert_eq!(c.original_base(), Some(BASE));
    assert_eq!(iri(&c, "term"), Some("http://v.org/term"));
    assert_eq!(
        expand_iri(&c, "x", false, true).as_deref(),
        Some("http://v.org/x")
    );
    assert_eq!(
        expand_iri(&c, "../x", true, false).as_deref(),
        Some("http://b.org/x")
    );
    assert_eq!(expand_iri(&c, "x", false, false).as_deref(), Some("x"));
    // A relative @vocab resolves against the base IRI, or concatenates
    // to the current @vocab.
    assert_eq!(
        ok(r#"{"@context": {"@vocab": "/v#"}}"#).vocab(),
        Some("http://example.com/v#")
    );
    let c = ok(r#"{"@context": [{"@vocab": "http://v.org/"}, {"@vocab": "sub/"}]}"#);
    assert_eq!(c.vocab(), Some("http://v.org/sub/"));
    assert_eq!(ok(r#"{"@context": {"@vocab": "_:"}}"#).vocab(), Some("_:"));
    // A relative @base resolves; null removes it.
    let c = ok(r#"{"@context": {"@base": "other/"}}"#);
    assert_eq!(c.base(), Some("http://example.com/other/"));
    assert_eq!(ok(r#"{"@context": {"@base": null}}"#).base(), None);
}

#[test]
fn language_and_direction_defaults() {
    let c = ok(r#"{"@context": {"@language": "de", "@direction": "ltr"}}"#);
    assert_eq!(c.default_language(), Some("de"));
    assert_eq!(c.default_direction(), Some(Direction::Ltr));
    let c = ok(r#"{"@context": [{"@language": "de"}, {"@language": null}]}"#);
    assert_eq!(c.default_language(), None);
}

#[test]
fn keyword_aliases() {
    let c = ok(r#"{"@context": {"id": "@id", "type": "@type", "@type": {"@container": "@set"}}}"#);
    assert_eq!(expand_iri(&c, "id", false, true).as_deref(), Some("@id"));
    assert_eq!(
        expand_iri(&c, "type", false, false).as_deref(),
        Some("@type")
    );
    assert_eq!(
        expand_iri(&c, "@type", false, true).as_deref(),
        Some("@type")
    );
    assert_eq!(c.term("@type").unwrap().container, Container::SET);
}

#[test]
fn keyword_like_terms_and_ids_are_ignored_with_a_warning() {
    let cache = ContextCache::new();
    let text = r#"{"@context": {"@ignoreMe": "http://x.org/", "t": {"@id": "@ignored"}, "r": {"@reverse": "@nope"}}}"#;
    let doc = root(text);
    let loader = NoNetwork::default();
    let mut p = Processor::with_cache(&loader, &cache);
    let c = p
        .process(
            &ActiveContext::new(None),
            context_of(&doc),
            None,
            Options::default(),
        )
        .unwrap();
    assert!(c.term("@ignoreMe").is_none() && c.term("t").is_none() && c.term("r").is_none());
    let w: Vec<String> = p
        .take_warnings()
        .iter()
        .inspect(|w| assert_eq!((w.code, w.severity), (DIAGNOSTIC_CODE, Severity::Warning)))
        .map(|w| spanned(text, w.span))
        .collect();
    assert_eq!(w, ["@ignoreMe", "t", "r"]);
    assert_eq!(expand_iri(&c, "@ignoreMe", false, true), None);
}

#[test]
fn null_resets_and_keeps_the_original_base() {
    let c = ok(
        r#"{"@context": [{"@base": "http://b.org/", "@vocab": "http://v.org/", "a": "http://a.org/"}, null]}"#,
    );
    assert!(c.term("a").is_none() && c.vocab().is_none());
    assert_eq!(c.base(), Some(BASE));
}

#[test]
fn protected_terms() {
    // The same definition again is fine; a different one is an error at
    // the key.
    ok(r#"{"@context": [{"@protected": true, "a": "http://a.org/"}, {"a": "http://a.org/"}]}"#);
    let text =
        r#"{"@context": [{"@protected": true, "a": "http://a.org/"}, {"a": "http://b.org/"}]}"#;
    assert_eq!(
        fails(text),
        (ErrorCode::ProtectedTermRedefinition, "a".into())
    );
    // A term's @protected overrides the context's.
    ok(
        r#"{"@context": [{"@protected": true, "a": {"@id": "http://a.org/", "@protected": false}}, {"a": "http://b.org/"}]}"#,
    );
    let c = ok(r#"{"@context": {"a": {"@id": "http://a.org/", "@protected": true}}}"#);
    assert!(c.term("a").unwrap().protected && c.has_protected());
    // Null cannot clear protected terms, unless overriding.
    let text = r#"{"@context": [{"@protected": true, "a": "http://a.org/"}, null]}"#;
    assert_eq!(
        fails(text),
        (ErrorCode::InvalidContextNullification, "null".into())
    );
    let doc = root(r#"{"@context": [{"@protected": true, "a": "http://a.org/"}]}"#);
    let cache = ContextCache::new();
    let loader = NoNetwork::default();
    let mut p = Processor::with_cache(&loader, &cache);
    let c = p
        .process(
            &ActiveContext::new(None),
            context_of(&doc),
            None,
            Options::default(),
        )
        .unwrap();
    let over = Options {
        override_protected: true,
        ..Options::default()
    };
    assert!(
        p.process(&c, &root("null"), None, over)
            .unwrap()
            .term("a")
            .is_none()
    );
    let redefine = root(r#"{"a": "http://b.org/"}"#);
    let c2 = p.process(&c, &redefine, None, over).unwrap();
    assert_eq!(iri(&c2, "a"), Some("http://b.org/"));
}

#[test]
fn propagate_false_keeps_the_previous_context() {
    let cache = ContextCache::new();
    let loader = NoNetwork::default();
    let mut p = Processor::with_cache(&loader, &cache);
    let start = ActiveContext::new(None);
    let doc = root(r#"{"@propagate": false, "a": "http://a.org/"}"#);
    let c = p.process(&start, &doc, None, Options::default()).unwrap();
    assert_eq!(iri(&c, "a"), Some("http://a.org/"));
    assert!(c.previous().unwrap().term("a").is_none());
    // `propagate = false` (a type-scoped context) does the same, and an
    // existing previous context stays.
    let doc = root(r#"{"b": "http://b.org/"}"#);
    let opts = Options {
        propagate: false,
        ..Options::default()
    };
    let c2 = p.process(&c, &doc, None, opts).unwrap();
    assert!(c2.previous().unwrap().term("a").is_none());
    let c3 = p.process(&start, &doc, None, opts).unwrap();
    assert!(c3.term("b").is_some() && c3.previous().unwrap().term("b").is_none());
}

#[test]
fn scoped_contexts_are_validated_and_kept_unprocessed() {
    let text = r#"{"@context": {
            "@vocab": "http://v.org/",
            "Person": {"@context": {"name": "http://schema.org/name"}},
            "knows": {"@id": "http://v.org/knows", "@context": [null, {"x": "http://x.org/"}]}
        }}"#;
    let c = ok(text);
    let scoped = c.term("Person").unwrap().context.clone().unwrap();
    assert!(scoped.source.is_none());
    assert_eq!(scoped.base_url.as_deref(), Some(BASE));
    assert!(c.term("name").is_none());
    let cache = ContextCache::new();
    let loader = NoNetwork::default();
    let mut p = Processor::with_cache(&loader, &cache);
    let applied = p
        .process_scoped(&c, &scoped, Span::default(), false, true)
        .unwrap();
    assert_eq!(iri(&applied, "name"), Some("http://schema.org/name"));
    assert!(applied.previous().unwrap().term("name").is_none());
    // Applying it again is memoized.
    let again = p
        .process_scoped(&c, &scoped, Span::default(), false, true)
        .unwrap();
    assert_eq!(again.id, applied.id);
    // An invalid scoped context is an error at the definition inside it.
    let text = r#"{"@context": {"Person": {"@id": "http://v.org/P", "@context": {"bad": 5}}}}"#;
    let e = process(text).unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidScopedContext);
    assert!(
        e.message.starts_with("invalid term definition: "),
        "{}",
        e.message
    );
    assert_eq!(spanned(text, e.span), "bad");
    assert_eq!(
        e.to_syntax_error().message(),
        format!("invalid scoped context: {}", e.message)
    );
    // Scoped contexts may redefine protected terms and reset the context.
    ok(
        r#"{"@context": {"@protected": true, "a": "http://a.org/", "T": {"@id": "http://t.org/", "@context": [null, {"a": "http://b.org/"}]}}}"#,
    );
}

#[test]
fn a_remote_scoped_context_reports_at_the_reference() {
    let loader = NoNetwork::default().with(
        "http://example.com/remote",
        r#"{"@context": {"T": {"@id": "http://t.org/", "@context": {"x": "http://x.org/"}}}}"#,
    );
    let c = process_with(r#"{"@context": "remote"}"#, &loader).unwrap();
    let scoped = c.term("T").unwrap().context.clone().unwrap();
    assert_eq!(scoped.source.as_deref(), Some("http://example.com/remote"));
    assert_eq!(
        scoped.base_url.as_deref(),
        Some("http://example.com/remote")
    );
    let cache = ContextCache::new();
    let mut p = Processor::with_cache(&loader, &cache);
    let applied = p
        .process_scoped(&c, &scoped, Span::new(3, 4), true, true)
        .unwrap();
    assert_eq!(iri(&applied, "x"), Some("http://x.org/"));
}

#[test]
fn import_merges_a_remote_context_under_the_local_one() {
    let loader = NoNetwork::default().with(
            "http://example.com/imported.jsonld",
            r#"{"@context": {"a": "http://imported.org/a", "b": "http://imported.org/b", "@vocab": "http://iv.org/"}}"#,
        );
    let c = process_with(
        r#"{"@context": {"@import": "imported.jsonld", "b": "http://local.org/b"}}"#,
        &loader,
    )
    .unwrap();
    assert_eq!(iri(&c, "a"), Some("http://imported.org/a"));
    assert_eq!(iri(&c, "b"), Some("http://local.org/b"));
    assert_eq!(c.vocab(), Some("http://iv.org/"));
    // Imported definitions are written in the imported document.
    let (a, b) = (c.term("a").unwrap(), c.term("b").unwrap());
    assert_eq!(
        a.source.as_deref(),
        Some("http://example.com/imported.jsonld")
    );
    assert!(a.span.is_some() && b.is_local() && b.span.is_some());
    // An error inside the imported context is reported at the @import.
    let loader =
        NoNetwork::default().with("http://example.com/bad.jsonld", r#"{"@context": {"a": 5}}"#);
    let text = r#"{"@context": {"@import": "bad.jsonld"}}"#;
    assert_eq!(
        fails_with(text, &loader),
        (ErrorCode::InvalidTermDefinition, "bad.jsonld".into())
    );
    let loader = NoNetwork::default().with(
        "http://example.com/nested.jsonld",
        r#"{"@context": {"@import": "x.jsonld"}}"#,
    );
    let text = r#"{"@context": {"@import": "nested.jsonld"}}"#;
    assert_eq!(
        fails_with(text, &loader),
        (ErrorCode::InvalidContextEntry, "nested.jsonld".into())
    );
    let loader = NoNetwork::default().with("http://example.com/arr.jsonld", r#"{"@context": []}"#);
    let text = r#"{"@context": {"@import": "arr.jsonld"}}"#;
    assert_eq!(
        fails_with(text, &loader),
        (ErrorCode::InvalidRemoteContext, "arr.jsonld".into())
    );
    assert_eq!(
        fails(r#"{"@context": {"@import": 5}}"#),
        (ErrorCode::InvalidImportValue, "5".into())
    );
}

#[test]
fn remote_contexts_resolve_against_their_own_url() {
    let loader = NoNetwork::default()
        .with(
            "http://example.com/ctx/a.jsonld",
            r#"{"@context": ["b.jsonld", {"a": "http://a.org/", "@base": "http://ignored.org/"}]}"#,
        )
        .with(
            "http://example.com/ctx/b.jsonld",
            r#"{"@context": {"b": "http://b.org/"}}"#,
        );
    let c = process_with(r#"{"@context": "ctx/a.jsonld"}"#, &loader).unwrap();
    assert_eq!(iri(&c, "a"), Some("http://a.org/"));
    assert_eq!(iri(&c, "b"), Some("http://b.org/"));
    // @base in a remote context is ignored.
    assert_eq!(c.base(), Some(BASE));
    let a = c.term("a").unwrap();
    assert_eq!(a.source.as_deref(), Some("http://example.com/ctx/a.jsonld"));
    assert_eq!(a.span, Some(Span::new(28, 29)));
}

#[test]
fn remote_context_failures_are_errors_at_the_reference() {
    let text = r#"{"@context": "https://example.org/missing.jsonld"}"#;
    assert_eq!(
        fails(text),
        (
            ErrorCode::LoadingRemoteContextFailed,
            "https://example.org/missing.jsonld".into()
        )
    );
    let e = process(text).unwrap_err();
    assert!(
        e.message.contains("imports.network") && e.message.contains("--network"),
        "{e}"
    );
    assert_eq!(
        fails(r#"{"@context": "urn:x:nothing"}"#),
        (
            ErrorCode::LoadingRemoteContextFailed,
            "urn:x:nothing".into()
        )
    );
    let loader = NoNetwork::default()
        .with("http://example.com/broken", "{")
        .with("http://example.com/plain", r#"{"a": 1}"#)
        .with(
            "http://example.com/bad",
            r#"{"@context": {"a": {"@id": 5}}}"#,
        );
    assert_eq!(
        fails_with(r#"{"@context": "broken"}"#, &loader),
        (ErrorCode::LoadingRemoteContextFailed, "broken".into())
    );
    assert_eq!(
        fails_with(r#"{"@context": "plain"}"#, &loader),
        (ErrorCode::InvalidRemoteContext, "plain".into())
    );
    let text = r#"{"@context": [{}, "bad"]}"#;
    let e = process_with(text, &loader).unwrap_err();
    assert_eq!(
        (e.code, spanned(text, e.span).as_str()),
        (ErrorCode::InvalidIriMapping, "bad")
    );
    assert!(e.message.ends_with("(in http://example.com/bad)"), "{e}");
}

#[test]
fn a_context_including_itself_overflows() {
    let loader = NoNetwork::default().with("http://example.com/self", r#"{"@context": "self"}"#);
    assert_eq!(
        fails_with(r#"{"@context": "self"}"#, &loader),
        (ErrorCode::ContextOverflow, "self".into())
    );
    // A scoped context naming its own document is not followed again.
    let loader = NoNetwork::default().with(
        "http://example.com/scoped",
        r#"{"@context": {"t": {"@id": "http://t.org/", "@context": "scoped"}}}"#,
    );
    process_with(r#"{"@context": "scoped"}"#, &loader).unwrap();
}

#[test]
fn cyclic_definitions() {
    assert_eq!(
        fails(r#"{"@context": {"a": "b:x", "b": "a:y"}}"#),
        (ErrorCode::CyclicIriMapping, "a".into())
    );
    assert_eq!(
        fails(r#"{"@context": {"a": {"@id": "a:x"}}}"#),
        (ErrorCode::CyclicIriMapping, "a".into())
    );
}

/// Each error code: in a term definition on the term's key, elsewhere on
/// the offending value (a string's content without the quotes).
#[test]
fn every_error_code_at_its_span() {
    use ErrorCode::*;
    let cases: &[(&str, ErrorCode, &str)] = &[
        (r#"{"@context": 5}"#, InvalidLocalContext, "5"),
        (r#"{"@context": [true]}"#, InvalidLocalContext, "true"),
        (
            r#"{"@context": {"@version": 1.0}}"#,
            InvalidVersionValue,
            "1.0",
        ),
        (
            r#"{"@context": {"@version": "1.1"}}"#,
            InvalidVersionValue,
            "\"1.1\"",
        ),
        (r#"{"@context": {"@base": 5}}"#, InvalidBaseIri, "5"),
        (
            r#"{"@context": [{"@base": null}, {"@base": "rel"}]}"#,
            InvalidBaseIri,
            "rel",
        ),
        (r#"{"@context": {"@vocab": 5}}"#, InvalidVocabMapping, "5"),
        (
            r#"{"@context": [{"@base": null}, {"@vocab": "rel"}]}"#,
            InvalidVocabMapping,
            "rel",
        ),
        (
            r#"{"@context": {"@language": 5}}"#,
            InvalidDefaultLanguage,
            "5",
        ),
        (
            r#"{"@context": {"@direction": "up"}}"#,
            InvalidBaseDirection,
            "\"up\"",
        ),
        (
            r#"{"@context": {"@propagate": "no"}}"#,
            InvalidPropagateValue,
            "\"no\"",
        ),
        (
            r#"{"@context": {"@protected": 1}}"#,
            InvalidProtectedValue,
            "1",
        ),
        (
            r#"{"@context": {"": "http://x.org/"}}"#,
            InvalidTermDefinition,
            "",
        ),
        (r#"{"@context": {"a": 5}}"#, InvalidTermDefinition, "a"),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@foo": 1}}}"#,
            InvalidTermDefinition,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@index": "http://p/"}}}"#,
            InvalidTermDefinition,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@container": "@index", "@index": "@type"}}}"#,
            InvalidTermDefinition,
            "a",
        ),
        (
            r#"{"@context": {"@vocab": "http://x/", "a/b": {"@prefix": true}}}"#,
            InvalidTermDefinition,
            "a/b",
        ),
        (
            r#"{"@context": {"a": {"@id": "@type", "@prefix": true}}}"#,
            InvalidTermDefinition,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@prefix": "yes"}}}"#,
            InvalidPrefixValue,
            "a",
        ),
        (
            r#"{"@context": {"@id": "http://x/"}}"#,
            KeywordRedefinition,
            "@id",
        ),
        (
            r#"{"@context": {"@type": {"@id": "http://x/"}}}"#,
            KeywordRedefinition,
            "@type",
        ),
        (r#"{"@context": {"a": {"@id": 5}}}"#, InvalidIriMapping, "a"),
        (
            r#"{"@context": [{"@base": null}, {"a": "relative"}]}"#,
            InvalidIriMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@type": "@id"}}}"#,
            InvalidIriMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@reverse": 5}}}"#,
            InvalidIriMapping,
            "a",
        ),
        (
            r#"{"@context": {"a:b": {"@id": "http://x/other"}}}"#,
            InvalidIriMapping,
            "a:b",
        ),
        (
            r#"{"@context": {"a": "@context"}}"#,
            InvalidKeywordAlias,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@type": 5}}}"#,
            InvalidTypeMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@type": "_:b"}}}"#,
            InvalidTypeMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@type": "http://t/", "@container": "@type"}}}"#,
            InvalidTypeMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@container": "@foo"}}}"#,
            InvalidContainerMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@container": ["@list", "@set"]}}}"#,
            InvalidContainerMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@container": ["@graph", "@id", "@index"]}}}"#,
            InvalidContainerMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@container": ["@index", "@language"]}}}"#,
            InvalidContainerMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@reverse": "http://x/", "@id": "http://y/"}}}"#,
            InvalidReverseProperty,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@reverse": "http://x/", "@container": "@list"}}}"#,
            InvalidReverseProperty,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@language": 5}}}"#,
            InvalidLanguageMapping,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@direction": "up"}}}"#,
            InvalidBaseDirection,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@nest": "@id"}}}"#,
            InvalidNestValue,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@nest": 5}}}"#,
            InvalidNestValue,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@protected": "yes"}}}"#,
            InvalidProtectedValue,
            "a",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@context": {"b": 5}}}}"#,
            InvalidScopedContext,
            "b",
        ),
        (
            r#"{"@context": {"a": {"@id": "http://x/", "@context": 5}}}"#,
            InvalidScopedContext,
            "a",
        ),
        (
            r#"{"@context": {"@import": "https://example.org/x"}}"#,
            LoadingRemoteContextFailed,
            "https://example.org/x",
        ),
    ];
    for (text, code, span) in cases {
        let got = fails(text);
        assert_eq!((got.0, got.1.as_str()), (*code, *span), "{text}");
    }
    let e = process(r#"{"@context": {"a": 5}}"#)
        .unwrap_err()
        .to_syntax_error();
    assert!(e.message().starts_with("invalid term definition: "));
    assert_eq!((e.code, e.severity), (DIAGNOSTIC_CODE, Severity::Error));
}

#[test]
fn valid_containers() {
    let c = ok(r#"{"@context": {
            "a": {"@id": "http://x/a", "@container": ["@graph", "@id", "@set"]},
            "b": {"@id": "http://x/b", "@container": ["@set", "@type"]},
            "c": {"@id": "http://x/c", "@container": "@list"},
            "d": {"@id": "http://x/d", "@container": "@index", "@index": "http://x/p"},
            "e": {"@id": "http://x/e", "@container": null}
        }}"#);
    assert!(
        c.term("a")
            .unwrap()
            .container
            .contains(Container::GRAPH.with(Container::ID))
    );
    assert_eq!(c.term("b").unwrap().type_mapping.as_deref(), Some("@id"));
    assert_eq!(c.term("c").unwrap().container, Container::LIST);
    assert_eq!(c.term("d").unwrap().index.as_deref(), Some("http://x/p"));
    assert!(c.term("e").unwrap().container.is_empty());
}

#[test]
fn iri_and_relative_terms() {
    let c = ok(
        r#"{"@context": {"@vocab": "http://v.org/", "http://x.org/p": {"@type": "@id"}, "rel/p": {}}}"#,
    );
    assert_eq!(iri(&c, "http://x.org/p"), Some("http://x.org/p"));
    assert_eq!(iri(&c, "rel/p"), Some("http://v.org/rel/p"));
    // An IRI-shaped term may map only to itself.
    ok(r#"{"@context": {"http://x.org/p": {"@id": "http://x.org/p"}}}"#);
    assert_eq!(
        fails(r#"{"@context": {"http://x.org/p": {"@id": "http://y.org/p"}}}"#),
        (ErrorCode::InvalidIriMapping, "http://x.org/p".into())
    );
}

#[test]
fn rfc3986_resolution() {
    let base = "http://a/b/c/d;p?q";
    for (r, want) in [
        ("g", "http://a/b/c/g"),
        ("./g", "http://a/b/c/g"),
        ("g/", "http://a/b/c/g/"),
        ("/g", "http://a/g"),
        ("//g", "http://g"),
        ("?y", "http://a/b/c/d;p?y"),
        ("g?y", "http://a/b/c/g?y"),
        ("#s", "http://a/b/c/d;p?q#s"),
        ("", "http://a/b/c/d;p?q"),
        (".", "http://a/b/c/"),
        ("..", "http://a/b/"),
        ("../g", "http://a/b/g"),
        ("../../../g", "http://a/g"),
        ("/./g", "http://a/g"),
        ("g;x=1/../y", "http://a/b/c/y"),
        ("g:h", "g:h"),
    ] {
        assert_eq!(resolve_iri(base, r), want, "{r}");
    }
    assert_eq!(resolve_iri("http://a", "b"), "http://a/b");
}

#[test]
fn processed_remote_contexts_come_from_the_cache() {
    let loader = NoNetwork::default().with(
        "http://example.com/ctx",
        r#"{"@context": {"@vocab": "http://v.org/", "n": "http://n.org/"}}"#,
    );
    let cache = ContextCache::new();
    let doc = root(r#"{"@context": ["ctx", {"x": "http://x.org/"}]}"#);
    let run = |loader: &NoNetwork, base: &str| {
        Processor::with_cache(loader, &cache)
            .process(
                &ActiveContext::new(Some(base)),
                context_of(&doc),
                Some(BASE),
                Options::default(),
            )
            .unwrap()
    };
    let a = run(&loader, BASE);
    assert_eq!(cache.stats(), CacheStats { hits: 0, misses: 1 });
    let b = run(&loader, BASE);
    assert_eq!(cache.stats(), CacheStats { hits: 1, misses: 1 });
    assert_eq!(iri(&b, "n"), iri(&a, "n"));
    assert_eq!(iri(&b, "x"), Some("http://x.org/"));
    // Another document reuses it too: nothing in it used the base.
    let c = run(&loader, "http://other.org/doc");
    assert_eq!(cache.stats().hits, 2);
    assert_eq!(c.base(), Some("http://other.org/doc"));
    // New content is processed again.
    let changed = NoNetwork::default().with(
        "http://example.com/ctx",
        r#"{"@context": {"n": "http://n2.org/"}}"#,
    );
    assert_eq!(iri(&run(&changed, BASE), "n"), Some("http://n2.org/"));
    assert_eq!(cache.stats().misses, 2);
}

#[test]
fn a_cached_context_is_reprocessed_when_a_nested_one_changed() {
    let cache = ContextCache::new();
    let text = r#"{"@context": "outer"}"#;
    let doc = root(text);
    let run = |inner: &str| {
        let loader = NoNetwork::default()
            .with("http://example.com/outer", r#"{"@context": "inner"}"#)
            .with("http://example.com/inner", inner.to_string());
        Processor::with_cache(&loader, &cache)
            .process(
                &ActiveContext::new(Some(BASE)),
                context_of(&doc),
                Some(BASE),
                Options::default(),
            )
            .map(|c| iri(&c, "a").map(str::to_string))
    };
    let one = r#"{"@context": {"a": "http://1/"}}"#;
    let two = r#"{"@context": {"a": "http://2/"}}"#;
    assert_eq!(run(one).unwrap().as_deref(), Some("http://1/"));
    assert_eq!(run(two).unwrap().as_deref(), Some("http://2/"));
    assert_eq!(run(two).unwrap().as_deref(), Some("http://2/"));
    assert_eq!(cache.stats().hits, 1);
    // A cached error is reported again, at the reference.
    let bad = r#"{"@context": {"a": 1}}"#;
    assert!(run(bad).is_err());
    let hits = cache.stats().hits;
    let e = run(bad).unwrap_err();
    assert_eq!(cache.stats().hits, hits + 1);
    assert_eq!(
        (e.code, spanned(text, e.span).as_str()),
        (ErrorCode::InvalidTermDefinition, "outer")
    );
    assert!(e.message.ends_with("(in http://example.com/inner)"), "{e}");
}

/// Processing schema.org's context a second time costs nearly nothing.
#[test]
fn timing_the_bundled_schema_org_context_is_processed_once() {
    let cache = ContextCache::new();
    let doc = root(r#"{"@context": "https://schema.org/"}"#);
    let run = || {
        let t = std::time::Instant::now();
        let c = Processor::with_cache(&crate::jsonld::loader::Bundled, &cache)
            .process(
                &ActiveContext::new(Some(BASE)),
                context_of(&doc),
                Some(BASE),
                Options::default(),
            )
            .unwrap();
        (c, t.elapsed())
    };
    let (first, cold) = run();
    let (second, warm) = run();
    assert_eq!(cache.stats(), CacheStats { hits: 1, misses: 1 });
    assert_eq!(iri(&second, "Person"), Some("http://schema.org/Person"));
    assert!(first.terms().count() > 1000);
    eprintln!("schema.org context: cold {cold:?}, warm {warm:?}");
    if cfg!(not(debug_assertions)) {
        assert!(warm * 20 < cold, "cold {cold:?}, warm {warm:?}");
    }
}

/// Process `@context` of `text` over a context that already defines
/// `base` (so it is not the pristine one), with `cache` shared with
/// other runs.
fn run_shared(
    cache: &ContextCache,
    loader: &NoNetwork,
    text: &str,
) -> Result<ActiveContext, ContextError> {
    static OUTER: OnceLock<ActiveContext> = OnceLock::new();
    let outer = OUTER.get_or_init(|| {
        let doc = root(r#"{"@context": {"outer": "http://outer.org/"}}"#);
        Processor::with_cache(&NoNetwork::default(), &ContextCache::new())
            .process(
                &ActiveContext::new(Some(BASE)),
                context_of(&doc),
                Some(BASE),
                Options::default(),
            )
            .unwrap()
    });
    let doc = root(text);
    Processor::with_cache(loader, cache).process(
        outer,
        context_of(&doc),
        Some(BASE),
        Options::default(),
    )
}

fn credentials() -> NoNetwork {
    NoNetwork::default().with(
        "http://example.com/credentials",
        r#"{"@context": {"@version": 1.1, "cred": "http://cred.org/"}}"#,
    )
}

/// A remote context applied after an inline object is cached for that
/// result, not for the context the inline object started from.
#[test]
fn a_remote_context_after_an_inline_object_is_not_cached_for_the_original() {
    let loader = credentials();
    let inline = r#"{"@context": [{"zz": "http://evil/zz"}, "credentials"]}"#;
    let plain = r#"{"@context": "credentials"}"#;
    for order in [[inline, plain], [plain, inline]] {
        let cache = ContextCache::new();
        for text in order {
            let c = run_shared(&cache, &loader, text).unwrap();
            assert_eq!(iri(&c, "cred"), Some("http://cred.org/"), "{text}");
            let wants_zz = text == inline;
            assert_eq!(
                iri(&c, "zz"),
                wants_zz.then_some("http://evil/zz"),
                "{text} after {order:?}"
            );
        }
    }
}

/// Two documents sharing one cache and one remote context each get
/// their own inline terms.
#[test]
fn documents_sharing_a_cache_keep_their_own_inline_terms() {
    let loader = credentials();
    let cache = ContextCache::new();
    let a = run_shared(
        &cache,
        &loader,
        r#"{"@context": [{"a": "http://a.org/a"}, "credentials"]}"#,
    )
    .unwrap();
    let b = run_shared(
        &cache,
        &loader,
        r#"{"@context": [{"b": "http://b.org/b"}, "credentials"]}"#,
    )
    .unwrap();
    let c = run_shared(&cache, &loader, r#"{"@context": "credentials"}"#).unwrap();
    assert_eq!((iri(&a, "a"), iri(&a, "b")), (Some("http://a.org/a"), None));
    assert_eq!((iri(&b, "a"), iri(&b, "b")), (None, Some("http://b.org/b")));
    assert_eq!((iri(&c, "a"), iri(&c, "b")), (None, None));
    assert_ne!(a.content_id(), b.content_id());
}

/// A term's scoped context is checked against the context as it is at
/// that term, not as another document left it in the cache.
#[test]
fn scoped_context_validation_sees_its_own_context() {
    let loader = NoNetwork::default()
            .with(
                "http://example.com/scoped",
                r#"{"@context": {"t": {"@id": "http://t.org/t", "@context": {"@vocab": "http://v.org/"}}}}"#,
            )
            .with(
                "http://example.com/other",
                r#"{"@context": {"o": "http://o.org/"}}"#,
            );
    let cache = ContextCache::new();
    let texts = [
        r#"{"@context": [{"p": {"@id": "http://p.org/p", "@protected": true}}, "other", "scoped"]}"#,
        r#"{"@context": ["other", "scoped"]}"#,
        r#"{"@context": [{"zz": "http://z.org/"}, "scoped"]}"#,
        r#"{"@context": "scoped"}"#,
    ];
    for text in texts.iter().chain(texts.iter().rev()) {
        let c = run_shared(&cache, &loader, text).unwrap();
        assert_eq!(iri(&c, "t"), Some("http://t.org/t"), "{text}");
        assert_eq!(iri(&c, "zz").is_some(), text.contains("zz"), "{text}");
        assert_eq!(iri(&c, "o").is_some(), text.contains("other"), "{text}");
        assert_eq!(iri(&c, "p").is_some(), text.contains("\"p\""), "{text}");
    }
}

/// Nested contexts that include the one below them twice would take
/// 2^depth steps: the work is capped, and the error is one.
#[test]
fn timing_doubling_nested_contexts_overflow_quickly() {
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
    let cache = ContextCache::new();
    let start = std::time::Instant::now();
    let e = run_shared(&cache, &loader, r#"{"@context": "c20"}"#).unwrap_err();
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(e.code, ErrorCode::ContextOverflow);
    assert!(e.message.contains("too many"), "{e}");
    // The overflow is not remembered: a small context still works,
    // and so does the same run after a shallow one.
    let c = run_shared(&cache, &loader, r#"{"@context": "c3"}"#).unwrap();
    assert_eq!(iri(&c, "a"), Some("http://a.org/"));
    // A realistic number of remote contexts stays well below the cap.
    let many = format!(r#"{{"@context": [{}]}}"#, vec![r#""c1""#; 200].join(","));
    run_shared(&cache, &loader, &many).unwrap();
}
