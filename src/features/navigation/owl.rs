//! Rename in an OWL Functional syntax or OWL/XML document: the edits that
//! make it name one IRI instead of another, at the spans its frontend
//! records for each use ([`Parsed::refs`]), or why they cannot be made.
//!
//! [`plan`] refuses a document with a syntax error whose text may spell
//! the old IRI, as its frontend may not have read every use there. It
//! writes each use in its own form, with the writer of its syntax: in
//! Functional syntax, `<…>` as `<new>`, and `pfx:local` with its own
//! prefix when that still covers the new IRI with a local name the lexer
//! reads, else with another prefix in scope (the longest namespace first),
//! else as `<new>`. The OWL/XML writer (`owl/owx.rs`) keeps a full or
//! relative `IRI` value and an abbreviated IRI's prefix where it can, else
//! writes the full IRI, and refuses an `<AbbreviatedIRI>` element no
//! declared prefix covers. Then it checks its work: the edited text,
//! parsed again by the same frontend from the same base, must have no
//! error and give the same triples with the old IRI read as the new one;
//! a triple that still names the old IRI is one the OWL mapping implies,
//! with no text of its own, and refuses the rename. [`at`] says what a
//! rename started at an offset renames, and the prefixes a new name typed
//! there reads with. It reads only the shared parse and the text.

use crate::analysis::Analysis;
use crate::rdfxml::rename::{Refusal, alone, apply, line, naming, renamed};
use crate::syntax::names::is_pn_local;
use crate::syntax::{Form, Iri, Parsed, PrefixMap, Severity, Span, Syntax};
use std::cmp::Reverse;

mod owx;

/// Why no edit of a use names another IRI: its text is not the IRI.
const ANCHORED: &str =
    "the IRI is written with an entity or character reference, which a rename does not rewrite";

/// Why a rename stops at a prefix declaration.
const PREFIX: &str =
    "can't rename a prefix in an OWL document: rename a term written with it instead";

/// The edits that make `text` (in an OWL syntax, parsed as `parsed`) name
/// `new` wherever it names `old`, sorted, or why the document cannot be
/// edited so: its first syntax error when its text may spell `old`, the
/// first use no edit can change, else what the parse of the edited text
/// says against it.
pub(crate) fn plan(
    text: &str,
    parsed: &Parsed,
    old: Iri,
    new: &str,
) -> Result<Vec<(Span, String)>, Refusal> {
    if *old == *new {
        return Ok(Vec::new());
    }
    if let Some(e) = first_error(parsed)
        && (spells(text, &old) || parsed.syntax == Syntax::OwlXml && owx::spells(text, &old))
    {
        return Err(Refusal {
            line: line(text, e.span.start),
            column: None,
            reason: format!(
                "the document has a syntax error ({}): it is not edited until it is fixed",
                e.message()
            ),
        });
    }
    let mut uses: Vec<(Span, Form)> = parsed
        .refs
        .iter()
        .filter(|r| r.iri == old)
        .map(|r| (r.span, r.form))
        .collect();
    uses.sort_by_key(|(s, _)| (s.start, s.end));
    uses.dedup_by_key(|(s, _)| *s);
    let prefixes = in_scope(parsed);
    let mut edits = Vec::with_capacity(uses.len());
    for (span, form) in uses {
        let edit =
            written(parsed.syntax, text, span, form, new, &prefixes).map_err(|reason| Refusal {
                line: line(text, span.start),
                column: None,
                reason,
            })?;
        edits.push(edit);
    }
    edits.sort_by_key(|(s, _)| (s.start, s.end));
    if edits.is_empty() {
        return match implied(text, parsed, old) {
            Some(r) => Err(r),
            None => Ok(edits),
        };
    }
    let after = apply(text, &edits);
    let reparsed = parse(parsed.syntax, &after, parsed.document_base.as_deref());
    if let Some(e) = first_error(&reparsed) {
        return Err(Refusal {
            line: line(&after, e.span.start),
            column: None,
            reason: format!(
                "the new name would make the document wrong here ({})",
                e.message()
            ),
        });
    }
    if let Some(r) = implied(&after, &reparsed, old) {
        return Err(r);
    }
    let new_iri = Iri::intern(new);
    let changed = parsed
        .triples
        .iter()
        .map(|t| renamed(t, old, new_iri))
        .zip(reparsed.triples.iter().map(|t| renamed(t, old, old)))
        .position(|(want, got)| want != got);
    if changed.is_some() || parsed.triples.len() != reparsed.triples.len() {
        // The first triple that differs, else the first one that only one
        // of the two texts gives; an edit writes no line break, so a line
        // of either text is the same line.
        let n = parsed.triples.len().min(reparsed.triples.len());
        let at = match (changed, reparsed.triples.get(n)) {
            (Some(i), _) => line(&after, reparsed.triples[i].p_span.start),
            (None, Some(t)) => line(&after, t.p_span.start),
            (None, None) => parsed
                .triples
                .get(n)
                .map_or(1, |t| line(text, t.p_span.start)),
        };
        return Err(Refusal {
            line: at,
            column: None,
            reason: format!(
                "with <{new}> written here, the document would say something else than the rename means"
            ),
        });
    }
    Ok(edits)
}

/// What a rename started in an OWL document renames: the IRI written at
/// the offset, the span of its written form, and the prefixes a new name
/// typed there reads with ([`in_scope`]).
#[derive(Debug, PartialEq)]
pub(crate) struct Use {
    pub(crate) span: Span,
    pub(crate) iri: Iri,
    pub(crate) prefixes: PrefixMap,
}

/// What a rename at `offset` of OWL document `a` starts on: `None` where
/// no IRI is written, and at a use whose text is not the IRI (an entity or
/// character reference); `Err` on the name of a prefix declaration, as a
/// prefix is not renamed in an OWL document.
pub(crate) fn at(a: &Analysis, offset: u32) -> Result<Option<Use>, Refusal> {
    if let Some(p) = a
        .parsed
        .prefixes
        .iter()
        .find(|p| p.name_span.contains(offset))
    {
        return Err(Refusal {
            line: line(&a.text, p.name_span.start),
            column: None,
            reason: PREFIX.into(),
        });
    }
    let Some(r) = a.ref_at(offset) else {
        return Ok(None);
    };
    match r.form {
        Form::Iri | Form::PName { .. } => Ok(Some(Use {
            span: r.span,
            iri: r.iri,
            prefixes: in_scope(&a.parsed),
        })),
        Form::A | Form::Anchor => Ok(None),
    }
}

/// The prefixes of an OWL document, by name: the first declaration of each
/// name, and in Functional syntax the four of its Table 2 (`rdf:`, `rdfs:`,
/// `xsd:`, `owl:`) where not declared, as its frontend binds them. An OWL
/// document's prefixes hold for the whole document.
fn in_scope(parsed: &Parsed) -> PrefixMap {
    let mut m = PrefixMap::default();
    for p in &parsed.prefixes {
        m.entry(p.name.clone()).or_insert_with(|| p.iri.to_string());
    }
    let standard: &[(&str, &str)] = match parsed.syntax {
        Syntax::OwlFunctional => &crate::ofn::STANDARD,
        Syntax::OwlXml
        | Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::JsonLd
        | Syntax::RdfXml => &[],
    };
    for (name, iri) in standard {
        m.entry((*name).to_string())
            .or_insert_with(|| (*iri).to_string());
    }
    m
}

/// The edit that writes `new` for the use of an IRI at `span` (in form
/// `form`) of `text` in `syntax`, with `prefixes` in scope; `Err` with why
/// no text names `new` there.
fn written(
    syntax: Syntax,
    text: &str,
    span: Span,
    form: Form,
    new: &str,
    prefixes: &PrefixMap,
) -> Result<(Span, String), String> {
    match syntax {
        Syntax::OwlFunctional => functional(text, span, form, new, prefixes),
        Syntax::OwlXml => owx::written(text, span, form, new, prefixes),
        Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::JsonLd
        | Syntax::RdfXml => Err("the document is in no OWL syntax".into()),
    }
}

/// [`written`] in Functional syntax: `<new>` for a full IRI; for an
/// abbreviated one, its own prefix if it still covers `new` with a local
/// name the lexer reads (`PN_LOCAL` without backslash escapes; not empty,
/// as the grammar reads `pfx:` as a prefix name only), else
/// another prefix in scope that does (the longest namespace first), else
/// `<new>`.
fn functional(
    text: &str,
    span: Span,
    form: Form,
    new: &str,
    prefixes: &PrefixMap,
) -> Result<(Span, String), String> {
    let raw = &text[span.start as usize..span.end as usize];
    let full = (span, format!("<{new}>"));
    match form {
        Form::Iri => Ok(full),
        Form::PName { prefix_len } => {
            let (own, _) = raw.split_at(prefix_len as usize);
            let fits = |p: &str, ns: &str| {
                let local = new
                    .strip_prefix(ns)
                    .filter(|l| !l.is_empty() && is_pn_local(l))?;
                Some((span, format!("{p}:{local}")))
            };
            if let Some(e) = prefixes.get(own).and_then(|ns| fits(own, ns)) {
                return Ok(e);
            }
            let mut others: Vec<(&String, &String)> =
                prefixes.iter().filter(|(p, _)| *p != own).collect();
            others.sort_by_key(|(p, ns)| (Reverse(ns.len()), p.as_str()));
            Ok(others
                .into_iter()
                .find_map(|(p, ns)| fits(p, ns))
                .unwrap_or(full))
        }
        Form::A | Form::Anchor => Err(ANCHORED.into()),
    }
}

/// `text`, parsed as `syntax` from `base`, by its OWL frontend.
fn parse(syntax: Syntax, text: &str, base: Option<&str>) -> Parsed {
    match syntax {
        Syntax::OwlFunctional => crate::ofn::parse(text, base),
        Syntax::OwlXml => crate::owx::parse(text, base),
        // [`written`] refuses every use of a document in another syntax.
        Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::JsonLd
        | Syntax::RdfXml => unreachable!("no use of a {syntax:?} document is written"),
    }
}

/// The first error of `parsed` in the text, warnings left out.
fn first_error(parsed: &Parsed) -> Option<&crate::syntax::SyntaxError> {
    parsed
        .errors
        .iter()
        .filter(|e| e.severity == Severity::Error)
        .min_by_key(|e| e.span.start)
}

/// Why `text` (parsed as `parsed`) still names `old`: a triple the OWL
/// mapping gives names it with no text of its own, at the line of the
/// triple's predicate or object.
fn implied(text: &str, parsed: &Parsed, old: Iri) -> Option<Refusal> {
    let at = parsed
        .triples
        .iter()
        .filter_map(|t| naming(t, old))
        .min_by_key(|s| s.start)?;
    Some(Refusal {
        line: line(text, at.start),
        column: None,
        reason: format!("the OWL mapping implies <{old}> here, with no text of its own"),
    })
}

/// Whether `text` may spell `old`, over-counting: the IRI itself, its
/// local name (after the last `#`, `/` or `:`) standing alone, or a
/// bracketed start of it (`<…>`, as a prefix declaration writes it) with
/// the rest standing alone. An IRI with no local name (a namespace) may be
/// spelled by any text.
fn spells(text: &str, old: &str) -> bool {
    let cut = old.rfind(['#', '/', ':']).map_or(0, |i| i + 1);
    if cut == old.len() || text.contains(old) || alone(text, &old[cut..]) {
        return true;
    }
    let bytes = text.as_bytes();
    let mut tails: Vec<usize> = text
        .match_indices('<')
        .filter_map(|(i, _)| {
            let rest = &bytes[i + 1..];
            let n = rest
                .iter()
                .zip(old.as_bytes())
                .take_while(|(a, b)| a == b)
                .count();
            (n > 0 && rest.get(n) == Some(&b'>') && old.is_char_boundary(n)).then_some(n)
        })
        .collect();
    tails.sort_unstable();
    tails.dedup();
    tails.into_iter().any(|n| alone(text, &old[n..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const URI: &str = "file:///w/onto.ofn";
    const EX: &str = "http://example.org/";

    /// `body` in an ontology with `ex:` (http://example.org/) declared.
    fn doc(body: &str) -> String {
        format!("Prefix(ex:=<{EX}>)\nOntology(<{EX}o>\n{body}\n)\n")
    }

    fn errors(p: &Parsed) -> Vec<String> {
        p.errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .map(|e| e.message().to_string())
            .collect()
    }

    /// The triples of `p` with `from` read as `to`, in order.
    fn graph(p: &Parsed, from: &str, to: &str) -> Vec<String> {
        let (from, to) = (Iri::intern(from), Iri::intern(to));
        p.triples
            .iter()
            .map(|t| format!("{:?}", renamed(t, from, to)))
            .collect()
    }

    /// `text` after renaming `old` to `new`, checked: the text parses again
    /// with no error to the graph before with `old` read as `new`.
    fn renamed_in(text: &str, old: &str, new: &str) -> Result<String, Refusal> {
        let before = crate::ofn::parse(text, Some(URI));
        let edits = plan(text, &before, Iri::intern(old), new)?;
        let out = apply(text, &edits);
        let after = crate::ofn::parse(&out, Some(URI));
        assert_eq!(errors(&after), Vec::<String>::new(), "{out}");
        assert_eq!(graph(&after, old, new), graph(&before, old, new), "{out}");
        Ok(out)
    }

    fn renamed_ok(text: &str, old: &str, new: &str) -> String {
        renamed_in(text, old, new).unwrap_or_else(|r| panic!("{r:?}\n{text}"))
    }

    fn refused(text: &str, old: &str, new: &str) -> Refusal {
        match renamed_in(text, old, new) {
            Err(r) => r,
            Ok(out) => panic!("renamed:\n{out}"),
        }
    }

    fn ex(local: &str) -> String {
        format!("{EX}{local}")
    }

    #[test]
    fn a_prefixed_name_keeps_its_prefix() {
        let text = doc("Declaration(Class(ex:A))\nSubClassOf(ex:B ex:A)");
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(out, doc("Declaration(Class(ex:Z))\nSubClassOf(ex:B ex:Z)"));
    }

    #[test]
    fn a_full_iri_stays_a_full_iri() {
        let text = doc(&format!(
            "Declaration(Class(<{EX}A>))\nSubClassOf(ex:B <{EX}A>)"
        ));
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(
            out,
            doc(&format!(
                "Declaration(Class(<{EX}Z>))\nSubClassOf(ex:B <{EX}Z>)"
            ))
        );
    }

    #[test]
    fn a_name_no_declared_prefix_covers_is_written_in_full() {
        let text = doc("Declaration(Class(ex:A))\nSubClassOf(ex:B ex:A)");
        let out = renamed_ok(&text, &ex("A"), "http://other.example/Z");
        assert_eq!(
            out,
            doc(
                "Declaration(Class(<http://other.example/Z>))\nSubClassOf(ex:B <http://other.example/Z>)"
            )
        );
    }

    #[test]
    fn another_prefix_that_covers_the_new_name_is_taken_the_longest_first() {
        let text = format!(
            "Prefix(ex:=<{EX}>)\nPrefix(o:=<http://other.example/>)\nPrefix(ov:=<http://other.example/v/>)\n\
             Ontology(<{EX}o>\nDeclaration(Class(ex:A))\n)\n"
        );
        let out = renamed_ok(&text, &ex("A"), "http://other.example/v/Z");
        assert_eq!(out, text.replace("Class(ex:A)", "Class(ov:Z)"));
    }

    #[test]
    fn the_standard_prefixes_are_bound_without_a_declaration() {
        let text =
            doc("Declaration(AnnotationProperty(ex:p))\nAnnotationAssertion(ex:p ex:a \"x\")");
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let out = renamed_ok(&text, &ex("p"), &format!("{rdfs}comment"));
        assert_eq!(
            out,
            doc(
                "Declaration(AnnotationProperty(rdfs:comment))\nAnnotationAssertion(rdfs:comment ex:a \"x\")"
            )
        );
    }

    #[test]
    fn a_standard_prefix_declared_again_binds_its_declared_iri() {
        let text =
            format!("Prefix(rdfs:=<{EX}r/>)\nOntology(<{EX}o>\nDeclaration(Class(rdfs:A))\n)\n");
        let out = renamed_ok(&text, &ex("r/A"), &ex("r/Z"));
        assert_eq!(out, text.replace("rdfs:A", "rdfs:Z"));
        // The standard IRI of `rdfs:` is not in scope there.
        let out = renamed_ok(&text, &ex("r/A"), "http://www.w3.org/2000/01/rdf-schema#Z");
        assert_eq!(
            out,
            text.replace("rdfs:A", "<http://www.w3.org/2000/01/rdf-schema#Z>")
        );
    }

    #[test]
    fn a_new_name_that_ends_at_a_namespace_is_written_in_full() {
        // `n:` alone is a prefix name, never an abbreviated IRI.
        let text = format!(
            "Prefix(ex:=<{EX}>)\nPrefix(n:=<{EX}n/>)\nOntology(<{EX}o>\nDeclaration(Class(ex:A))\n)\n"
        );
        let out = renamed_ok(&text, &ex("A"), &ex("n/"));
        assert_eq!(
            out,
            text.replace("Class(ex:A)", &format!("Class(<{EX}n/>)"))
        );
    }

    #[test]
    fn a_local_name_the_lexer_cannot_read_is_written_in_full() {
        let text = doc("Declaration(Class(ex:A))\nSubClassOf(ex:B ex:A)");
        // `~` needs a backslash escape in PN_LOCAL, which OWL Functional
        // syntax does not take; a name may not end in `.`.
        for local in ["Z~1", "Z.", "Z,1"] {
            let out = renamed_ok(&text, &ex("A"), &ex(local));
            let full = format!("<{EX}{local}>");
            assert_eq!(
                out,
                doc(&format!(
                    "Declaration(Class({full}))\nSubClassOf(ex:B {full})"
                )),
                "{local}"
            );
        }
    }

    #[test]
    fn a_broken_document_that_may_spell_the_iri_is_not_edited() {
        let text = doc("Declaration(Class(ex:A))\nSubClassOf(ex:B ex:A))\nSubClassOf(");
        let before = crate::ofn::parse(&text, Some(URI));
        assert!(!errors(&before).is_empty());
        let r = plan(&text, &before, Iri::intern(&ex("A")), &ex("Z")).unwrap_err();
        assert!(
            r.reason.starts_with("the document has a syntax error ("),
            "{r:?}"
        );
        assert!(
            r.reason.ends_with("): it is not edited until it is fixed"),
            "{r:?}"
        );
        let first = before.errors.iter().map(|e| e.span.start).min().unwrap();
        assert_eq!(r.line, line(&text, first));
        // A declared namespace and the rest of the IRI after it spell it.
        let text = format!("Prefix(p:=<{EX}A>)\nOntology(\np:B\n");
        let before = crate::ofn::parse(&text, Some(URI));
        let r = plan(&text, &before, Iri::intern(&ex("AB")), &ex("Z")).unwrap_err();
        assert!(
            r.reason.starts_with("the document has a syntax error ("),
            "{r:?}"
        );
        assert_eq!(r.line, 3);
        // An IRI the text cannot spell is renamed nowhere there.
        let edits = plan(&text, &before, Iri::intern(&ex("Q")), &ex("Z")).unwrap();
        assert!(edits.is_empty());
    }

    #[test]
    fn a_rename_that_changes_the_graph_is_refused() {
        // A literal of type rdf:PlainLiteral maps to a string with a
        // language tag: renaming its datatype there changes the literal.
        let text =
            doc("Declaration(Datatype(ex:dt))\nDataPropertyAssertion(ex:d ex:a \"x@en\"^^ex:dt)");
        let before = crate::ofn::parse(&text, Some(URI));
        let plain = "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral";
        let r = plan(&text, &before, Iri::intern(&ex("dt")), plain).unwrap_err();
        assert!(
            r.reason
                .contains("the document would say something else than the rename means"),
            "{r:?}"
        );
        assert_eq!(r.line, 4);
    }

    #[test]
    fn a_new_name_that_breaks_the_document_is_refused_with_its_line() {
        let text = doc("Declaration(Class(ex:A))\nSubClassOf(ex:B ex:A)");
        let before = crate::ofn::parse(&text, Some(URI));
        let r = plan(&text, &before, Iri::intern(&ex("A")), &ex("Z Y")).unwrap_err();
        assert!(
            r.reason
                .starts_with("the new name would make the document wrong here ("),
            "{r:?}"
        );
        assert_eq!(r.line, 3);
    }

    #[test]
    fn an_iri_the_mapping_implies_is_not_renamed() {
        let text = doc("Declaration(Class(ex:A))");
        let r = refused(&text, "http://www.w3.org/2002/07/owl#Class", &ex("Kind"));
        assert_eq!(
            r.reason,
            "the OWL mapping implies <http://www.w3.org/2002/07/owl#Class> here, with no text of its own"
        );
        assert_eq!(r.line, 3);
    }

    #[test]
    fn the_ontology_and_version_iris_are_renamed_as_any_use() {
        let text =
            format!("Prefix(ex:=<{EX}>)\nOntology(<{EX}o> ex:v1\nDeclaration(Class(ex:A))\n)\n");
        let out = renamed_ok(&text, &ex("o"), &ex("p"));
        assert_eq!(out, text.replace(&format!("<{EX}o>"), &format!("<{EX}p>")));
        let out = renamed_ok(&text, &ex("v1"), &ex("v2"));
        assert_eq!(out, text.replace("ex:v1", "ex:v2"));
    }

    #[test]
    fn a_declaration_is_renamed_with_the_other_uses() {
        let text = doc(
            "Declaration(ObjectProperty(ex:p))\nObjectPropertyAssertion(ex:p ex:a ex:b)\n\
             AnnotationAssertion(rdfs:label ex:p \"p\")",
        );
        let out = renamed_ok(&text, &ex("p"), &ex("q"));
        assert_eq!(out, text.replace("ex:p", "ex:q"));
    }

    #[test]
    fn no_use_or_the_same_name_gives_no_edits() {
        let text = doc("Declaration(Class(ex:A))");
        let before = crate::ofn::parse(&text, Some(URI));
        let none = plan(&text, &before, Iri::intern(&ex("Q")), &ex("Z")).unwrap();
        assert!(none.is_empty());
        let same = plan(&text, &before, Iri::intern(&ex("A")), &ex("A")).unwrap();
        assert!(same.is_empty());
    }

    #[test]
    fn a_warning_does_not_stop_a_rename() {
        let text = doc(
            "Declaration(Class(ex:A))\nAnnotationAssertion(rdfs:label ex:A \"a\"^^rdf:PlainLiteral)",
        );
        let before = crate::ofn::parse(&text, Some(URI));
        assert!(
            before
                .errors
                .iter()
                .any(|e| e.severity == Severity::Warning),
            "{:?}",
            before.errors
        );
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(out, text.replace("ex:A", "ex:Z"));
    }

    fn analysis(uri: &str, text: &str) -> Analysis {
        Analysis::new(uri.into(), text, std::sync::Arc::default(), true)
    }

    #[test]
    fn a_rename_started_in_the_file_reads_the_new_name_with_its_prefixes() {
        let text = format!(
            "Prefix(ex:=<{EX}>)\nPrefix(o:=<http://other.example/>)\nOntology(<{EX}o>\nDeclaration(Class(ex:A))\n)\n"
        );
        let a = analysis(URI, &text);
        let offset = text.find("ex:A").unwrap() as u32 + 1;
        let u = at(&a, offset).unwrap().unwrap();
        assert_eq!(&text[u.span.start as usize..u.span.end as usize], "ex:A");
        assert_eq!(*u.iri, ex("A"));
        let read = |name: &str| super::super::renamed_iri(&u.iri, name, &u.prefixes);
        assert_eq!(read("o:B").as_deref(), Ok("http://other.example/B"));
        assert_eq!(
            read("rdfs:B").as_deref(),
            Ok("http://www.w3.org/2000/01/rdf-schema#B")
        );
        assert_eq!(read("B").as_deref(), Ok(ex("B").as_str()));
        // A full IRI: its whole text, brackets included.
        let offset = text.find("<http://example.org/o>").unwrap() as u32 + 3;
        let u = at(&a, offset).unwrap().unwrap();
        assert_eq!(
            &text[u.span.start as usize..u.span.end as usize],
            format!("<{EX}o>")
        );
        // Nothing is written between the axioms.
        let offset = text.find("Declaration").unwrap() as u32 + 2;
        assert_eq!(at(&a, offset).unwrap(), None);
    }

    #[test]
    fn a_rename_started_on_a_prefix_declaration_is_refused() {
        let text = doc("Declaration(Class(ex:A))");
        let a = analysis(URI, &text);
        let r = at(&a, 8).unwrap_err();
        assert_eq!(r.line, 1);
        assert!(
            r.reason
                .contains("can't rename a prefix in an OWL document"),
            "{r:?}"
        );
    }

    #[test]
    fn a_rename_starts_on_owl_xml_values_and_iri_text() {
        let text = format!(
            "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"{EX}o\">\n\
             <Prefix name=\"ex\" IRI=\"{EX}\"/>\n\
             <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n\
             <Declaration><Class IRI=\"{EX}A\"/></Declaration>\n\
             <Declaration><Class abbreviatedIRI=\"ex:B\"/></Declaration>\n\
             <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><IRI>{EX}C</IRI><AbbreviatedIRI>ex:D</AbbreviatedIRI></AnnotationAssertion>\n\
             </Ontology>\n"
        );
        let uri = "file:///w/onto.owx";
        let a = analysis(uri, &text);
        assert_eq!(errors(&a.parsed), Vec::<String>::new());
        // A rename starts on a value or an IRI element's text, read with
        // the declared prefixes only.
        let span_at = |needle: &str| {
            let u = at(&a, text.find(needle).unwrap() as u32 + 1)
                .unwrap()
                .unwrap();
            (
                text[u.span.start as usize..u.span.end as usize].to_string(),
                u,
            )
        };
        let (written, u) = span_at(&format!("{EX}A"));
        assert_eq!(written, ex("A"));
        let read = |name: &str| super::super::renamed_iri(&u.iri, name, &u.prefixes);
        assert_eq!(read("ex:Q").as_deref(), Ok(ex("Q").as_str()));
        // No standard prefix is bound: `owl:` reads as a scheme.
        assert_eq!(read("owl:Q").as_deref(), Ok("owl:Q"));
        assert_eq!(span_at("ex:B").0, "ex:B");
        assert_eq!(span_at(&format!("{EX}C")).0, ex("C"));
        let (written, u) = span_at("ex:D");
        assert_eq!((written.as_str(), &*u.iri), ("ex:D", ex("D").as_str()));
        let r = at(&a, text.find("\"ex\"").unwrap() as u32 + 1).unwrap_err();
        assert_eq!(r.line, 3);
        assert!(
            r.reason
                .contains("can't rename a prefix in an OWL document"),
            "{r:?}"
        );
        // The OWL/XML writer plans the edits.
        let edits = plan(&text, &a.parsed, Iri::intern(&ex("B")), &ex("Z")).unwrap();
        assert_eq!(apply(&text, &edits), text.replace("ex:B", "ex:Z"));
        let none = plan(&text, &a.parsed, Iri::intern(&ex("Q")), &ex("Z")).unwrap();
        assert!(none.is_empty());
    }
}
