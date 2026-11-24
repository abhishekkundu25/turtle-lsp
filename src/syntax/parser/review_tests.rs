use crate::syntax::{PrefixMap, Syntax, parse};

fn flags(text: &str) -> Vec<bool> {
    parse(text, &PrefixMap::default(), Syntax::Turtle)
        .triples
        .iter()
        .map(|t| t.unresolved)
        .collect()
}

/// `urn:isbn:1` with no `urn:` prefix declared is a mistake; `<urn:isbn:1>`
/// is an absolute IRI with the same text. Only the first is unresolved,
/// in either order, in every position.
#[test]
fn an_absolute_iri_spelled_like_an_undeclared_name_is_resolved() {
    let p = "<http://e.org/p>";
    assert_eq!(
        flags(&format!("<urn:isbn:1> {p} 1 .\nurn:isbn:1 {p} 2 .\n")),
        [false, true]
    );
    assert_eq!(
        flags(&format!("urn:isbn:1 {p} 2 .\n<urn:isbn:1> {p} 1 .\n")),
        [true, false]
    );
    assert_eq!(
        flags(
            "<http://e.org/s> urn:x:p <urn:x:o> .\n<http://e.org/s> <urn:x:p> urn:x:o .\n<http://e.org/s> <urn:x:p> <urn:x:o> .\n"
        ),
        [true, true, false]
    );
    // Inside a triple term, and as a reifier.
    assert_eq!(
        flags(&format!(
            "<http://e.org/s> {p} <<( <urn:x:a> {p} <urn:x:b> )>> .\n<http://e.org/s> {p} <<( urn:x:a {p} <urn:x:b> )>> .\n"
        )),
        [false, true]
    );
    assert_eq!(
        flags(&format!(
            "<http://e.org/s> {p} <urn:x:o> ~ <urn:x:r> .\n<http://e.org/s> {p} <urn:x:o> ~ urn:x:r .\n"
        )),
        [false, false, false, true]
    );
}

/// With no document base, a relative `@base` is an error; under a base
/// it resolves.
#[test]
fn a_relative_base_needs_a_document_base() {
    use crate::syntax::{Severity, parse_with_base};
    let text = "@base <hbase/> .\n<s> <p> <o> .\n";
    let errors = |base| {
        parse_with_base(text, &PrefixMap::default(), Syntax::Turtle, base)
            .errors
            .into_iter()
            .filter(|e| e.severity == Severity::Error)
            .map(|e| e.message().to_string())
            .collect::<Vec<_>>()
    };
    let without = errors(None);
    assert!(
        without.len() == 1 && without[0].starts_with("Invalid base IRI"),
        "{without:?}"
    );
    assert_eq!(errors(Some("file:///w/a.ttl")), Vec::<String>::new());
}
