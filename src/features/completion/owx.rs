//! Completion in OWL/XML, the one edit turtle-lsp makes in an OWL/XML
//! document.
//!
//! What may come at the cursor is [`crate::owx::expected`]'s, read from the
//! elements open there, so it holds while the document is half typed:
//! - after `<`, the elements that may start there: inside `<Ontology>` its
//!   children in the schema's order, inside a construct those its next
//!   operand may be, where a new element is typed (its tag cut after the
//!   name). They are written in the document's prefix form; with snippets,
//!   an entity's element whole (`Class IRI="…"/>`, the cursor in its
//!   value), any other with its attributes and its end tag
//!   (`SubClassOf>…</SubClassOf>`);
//! - in an `IRI` or `datatypeIRI` value, or an `IRI` element's text, the
//!   terms of the kinds the element names (classes in `<Class>`), as full
//!   IRIs: from what is typed, as a local name, those of the namespaces the
//!   document declares and the workspace describes, and from two characters
//!   the vocabularies'; as an IRI, those of its namespace and of the
//!   namespaces it starts;
//! - in an `abbreviatedIRI` value, or an `AbbreviatedIRI` element's text,
//!   the terms as Functional syntax offers them ([`super::ofn::terms`]),
//!   over the prefixes the `<Prefix>` elements declare (OWL/XML builds none
//!   in). A term of a well-known prefix not declared adds its `<Prefix>`
//!   element on accept, after the others
//!   ([`crate::owx::expected::declare`]); it is not offered where none can
//!   be added: for a name a `<Prefix>` element names already (misplaced or
//!   with an error), or after a root start tag a `<` cut
//!   ([`crate::owx::expected::declarable`]).
//!
//! A value's edit replaces it whole, escaped as XML. The kinds of a term,
//! the ranking, the items and resolve are Functional syntax's and Turtle's.

use super::ofn::{fits, listed, slot_of, terms, written};
use super::{
    Best, Candidate, Slot, finish, fuzzy, items, local_name_candidates_where,
    namespace_terms_where, ranking,
};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::{Knowledge, namespace_of};
use crate::ofn::expected::Iris;
use crate::owx::expected::{self, Element, Expected, Form, Leaf, Names, escaped};
use crate::syntax::{Iri, Parsed, PrefixMap, Span};
use crate::vocab::{TermKind, VOCABS, split_namespace};
use lsp_types::*;
use rustc_hash::FxHashSet;

/// The directive item details name.
const DIRECTIVE: &str = "Prefix";

pub(super) fn complete(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: usize,
    snippets: bool,
) -> CompletionResponse {
    let list = match expected::at(&a.text, offset) {
        Some(Expected::Elements(names)) => named(a, k, enc, offset, &names, snippets),
        Some(Expected::EndTag { span, name, closed }) => {
            Some(end_tag(a, enc, offset, span, &name, closed))
        }
        Some(Expected::Attributes { span, attrs }) => {
            Some(attributes(a, enc, offset, span, &attrs, snippets))
        }
        Some(Expected::Iri { span, iris, form }) => iri(a, k, enc, offset, span, &iris, form),
        None => None,
    };
    match list {
        Some(list) => CompletionResponse::List(list),
        None => CompletionResponse::Array(vec![]),
    }
}

/// The elements of `names`, their name typed up to `offset`.
fn named(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: usize,
    names: &Names,
    snippets: bool,
) -> Option<CompletionList> {
    let Names {
        name,
        prefix,
        elements,
    } = names;
    let (name, prefix) = (*name, prefix.as_str());
    let text: &str = &a.text;
    let typed = &text[name.start as usize..offset];
    let local = typed.split_once(':').map_or(typed, |(_, l)| l);
    let mut cands = Vec::new();
    for &el in elements {
        let n = expected::name(el);
        if fuzzy(local, n) {
            // An entity's element first: what a slot most often takes.
            let rank = if is_entity(el) { 0 } else { 1 };
            let detail = detail(el);
            let label = format!("{prefix}{n}");
            cands.push(Candidate::plain(
                label,
                CompletionItemKind::KEYWORD,
                detail,
                rank,
            ));
        }
    }
    let none = PrefixMap::default();
    finish(
        &mut cands,
        ranking::Ranking::default(),
        Slot::Object,
        typed,
        k,
        &none,
    );
    let replace = a.range(enc, name);
    let mut items = items(cands, replace, &none, DIRECTIVE, |_, _| None::<TextEdit>);
    if snippets {
        for item in &mut items {
            let el = elements
                .iter()
                .find(|&&el| item.label[prefix.len()..] == *expected::name(el));
            if let (Some(&el), Some(CompletionTextEdit::Edit(edit))) = (el, &mut item.text_edit) {
                edit.new_text = snippet(&item.label, el);
                item.insert_text_format = Some(InsertTextFormat::SNIPPET);
            }
        }
    }
    Some(CompletionList {
        is_incomplete: true,
        items,
    })
}

/// Closing-tag completion after `</`: the innermost open element's name,
/// with `>` appended when no `>` already follows.
fn end_tag(
    a: &Analysis,
    enc: Encoding,
    offset: usize,
    span: Span,
    name: &str,
    closed: bool,
) -> CompletionList {
    let typed = &a.text[span.start as usize..offset];
    let range = a.range(enc, span);
    let mut items = Vec::new();
    if fuzzy(typed, name) {
        let new_text = if closed {
            name.to_string()
        } else {
            format!("{name}>")
        };
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("close element".into()),
            sort_text: Some("0".into()),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, new_text))),
            ..Default::default()
        });
    }
    CompletionList {
        is_incomplete: false,
        items,
    }
}

/// Valid OWL/XML attributes for the start tag being written.
fn attributes(
    a: &Analysis,
    enc: Encoding,
    offset: usize,
    span: Span,
    attrs: &[&str],
    snippets: bool,
) -> CompletionList {
    let typed = &a.text[span.start as usize..offset];
    let range = a.range(enc, span);
    let mut items = Vec::new();
    for (idx, &name) in attrs.iter().enumerate() {
        let local = name.split_once(':').map_or(name, |(_, l)| l);
        let matches = if typed.contains(':') {
            fuzzy(typed, name)
        } else {
            fuzzy(typed, name) || fuzzy(typed, local)
        };
        if !matches {
            continue;
        }
        let detail = match name {
            "IRI" => "full or relative IRI",
            "abbreviatedIRI" => "abbreviated IRI, prefix:local",
            "datatypeIRI" => "datatype IRI of this literal",
            "xml:lang" => "language tag for literals",
            "xml:base" => "base IRI for relative references",
            "name" => "prefix name (empty for default prefix)",
            "facet" => "constraining facet IRI",
            "cardinality" => "non-negative integer cardinality",
            "nodeID" => "blank node identifier",
            "ontologyIRI" => "ontology IRI",
            "versionIRI" => "ontology version IRI",
            _ => "attribute",
        };
        let (new_text, format) = if snippets {
            (format!("{name}=\"$0\""), Some(InsertTextFormat::SNIPPET))
        } else {
            (format!("{name}=\"\""), None)
        };
        items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(detail.into()),
            sort_text: Some(format!("{idx:02}")),
            insert_text_format: format,
            text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(range, new_text))),
            ..Default::default()
        });
    }
    CompletionList {
        is_incomplete: false,
        items,
    }
}

fn is_entity(el: Element) -> bool {
    matches!(el, Element::Keyword(k) if crate::ofn::signature::Entity::of(k).is_some())
}

/// What an element's item says: a keyword's operands, as Functional syntax
/// writes them; what a leaf holds.
fn detail(el: Element) -> String {
    match el {
        Element::Keyword(k) => written(k, &|_, name| name.to_string()),
        Element::Leaf(leaf) => match leaf {
            Leaf::Iri => "an IRI",
            Leaf::AbbreviatedIri => "an abbreviated IRI, prefix:local",
            Leaf::AnonymousIndividual => "an anonymous individual",
            Leaf::Literal => "a literal",
            Leaf::FacetRestriction => "a facet and its literal",
        }
        .to_string(),
    }
}

/// The element `el`, its name written `qualified`, as a snippet after its
/// `<`: its attributes' values the placeholders, then `/>`, or `>`, the
/// cursor and its end tag.
fn snippet(qualified: &str, el: Element) -> String {
    let (attributes, empty) = expected::form(el);
    let mut out = qualified.to_string();
    for (i, attr) in attributes.iter().enumerate() {
        out.push_str(&format!(" {attr}=\"${}\"", i + 1));
    }
    if empty {
        out.push_str("/>");
    } else {
        out.push_str(&format!(">$0</{qualified}>"));
    }
    out
}

/// The IRIs `iris` an IRI at `span`, typed up to `offset`, may be, written
/// in `form`.
fn iri(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    offset: usize,
    span: Span,
    iris: &Iris,
    form: Form,
) -> Option<CompletionList> {
    let text: &str = &a.text;
    let typed = &text[span.start as usize..offset];
    let declared = declared(&a.parsed);
    let (cands, incomplete, filters) = match form {
        Form::Abbreviated { root } => {
            let mut extras = Vec::new();
            let cands = terms(k, typed, iris, &declared, &mut extras)?;
            let (mut cands, cut) = listed(cands, extras, typed, iris, k, &declared);
            // A term whose `<Prefix>` cannot be added is not offered.
            let added = |c: &Candidate| {
                c.import
                    .as_ref()
                    .filter(|(p, _)| !declared.contains_key(p))
                    .is_some()
            };
            if cands.iter().any(added) {
                let declarable = expected::declarable(text, root);
                cands.retain(|c| match &c.import {
                    Some((p, _)) if added(c) => declarable
                        .as_ref()
                        .is_some_and(|named| !named.contains(&p.as_str())),
                    _ => true,
                });
            }
            (cands, cut || !typed.contains(':'), None)
        }
        Form::Full => {
            let (cands, filters) = full(k, typed, iris, &declared);
            (cands, true, filters)
        }
    };
    let replace = a.range(enc, span);
    let mut items = items(cands, replace, &declared, DIRECTIVE, |p, ns| {
        let Form::Abbreviated { root } = form else {
            return None;
        };
        let (at, new_text) = expected::declare(text, &a.parsed, root, p, ns);
        let pos = a.position(enc, at);
        Some(TextEdit::new(Range::new(pos, pos), new_text))
    });
    for item in &mut items {
        if let Some(CompletionTextEdit::Edit(edit)) = &mut item.text_edit
            && let std::borrow::Cow::Owned(e) = escaped(&edit.new_text)
        {
            edit.new_text = e;
        }
    }
    if let Some(filters) = filters {
        for (item, filter) in items.iter_mut().zip(filters) {
            item.filter_text = Some(filter);
        }
    }
    Some(CompletionList {
        is_incomplete: incomplete,
        items,
    })
}

/// The prefixes the document's `<Prefix>` elements declare (the first of a
/// name binds it).
fn declared(parsed: &Parsed) -> PrefixMap {
    let mut out = PrefixMap::default();
    for p in &parsed.prefixes {
        if !out.contains_key(&p.name) {
            out.insert(p.name.clone(), p.iri.to_string());
        }
    }
    out
}

/// The terms `iris` takes as full IRIs that match `typed`, ranked by their
/// local names; and, when a local name is typed, each one's local name for
/// the client to filter on (it filters on what the value holds).
fn full(
    k: &Knowledge,
    typed: &str,
    iris: &Iris,
    declared: &PrefixMap,
) -> (Vec<Candidate>, Option<Vec<String>>) {
    let slot = slot_of(iris);
    let fits = |iri: &str, vocab: Option<TermKind>| {
        (vocab.is_some() || oxiri::Iri::parse(iri).is_ok()) && fits(k, iris, iri, vocab)
    };
    let promoted = |_: Iri| false;
    // Labelled by local name while ranked; written in full after.
    let by_local = |l: &str| format!(":{l}");
    let mut known: FxHashSet<&str> = declared.values().map(String::as_str).collect();
    known.extend(k.described_terms().map(|i| namespace_of(i.as_str())));
    let mut cands = Vec::new();
    let (local, spaces): (&str, Vec<(&str, &str)>) = if typed.contains(':') {
        let (ns, local) = split_namespace(typed);
        // The namespaces it starts, from their first term.
        let started = known
            .iter()
            .copied()
            .chain(VOCABS.all.iter().map(|v| v.namespace))
            .filter(|n| n.len() > typed.len() && n.starts_with(typed))
            .map(|n| (n, ""));
        let mut spaces: Vec<_> = std::iter::once((ns, local)).chain(started).collect();
        spaces.sort_unstable();
        spaces.dedup();
        (local, spaces)
    } else {
        let mut spaces: Vec<_> = known.iter().map(|&n| (n, typed)).collect();
        spaces.sort_unstable();
        (typed, spaces)
    };
    let best = Best {
        typed: local,
        promoted: &promoted,
    };
    for (ns, l) in spaces {
        if !ns.is_empty() {
            cands.extend(namespace_terms_where(
                k,
                ns,
                l,
                slot,
                &by_local,
                None,
                Some(best),
                &fits,
                &|_| true,
            ));
        }
    }
    if !typed.contains(':') {
        local_name_candidates_where(&mut cands, typed, slot, declared, &fits);
    }
    let mut seen = FxHashSet::default();
    cands.retain(|c| c.iri.as_ref().is_some_and(|i| seen.insert(i.clone())));
    let (mut cands, _) = listed(cands, Vec::new(), local, iris, k, declared);
    let filters = (!typed.contains(':')).then(|| {
        cands
            .iter()
            .map(|c| {
                c.label
                    .split_once(':')
                    .map_or(&*c.label, |(_, l)| l)
                    .to_string()
            })
            .collect()
    });
    for c in &mut cands {
        if let Some(iri) = &c.iri {
            c.label = iri.clone();
        }
        c.import = None;
    }
    (cands, filters)
}

#[cfg(test)]
mod tests {
    use crate::analysis::{Analysis, Encoding};
    use crate::knowledge::{Knowledge, Source};
    use lsp_types::*;
    use std::sync::Arc;

    const URI: &str = "file:///w/onto.owx";

    /// A document whose `|` is the cursor, and the workspace around it.
    struct At {
        a: Arc<Analysis>,
        k: Knowledge,
        pos: Position,
        text: String,
    }

    fn at_doc(text: &str) -> At {
        at(text, &[])
    }

    fn at(text: &str, others: &[(&str, &str)]) -> At {
        let cursor = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        let a = Arc::new(Analysis::new(URI.into(), &text, Arc::default(), true));
        assert_eq!(a.parsed.syntax, crate::syntax::Syntax::OwlXml);
        let mut k = Knowledge::default();
        k.set(URI, Source::Open, a.clone());
        for (uri, t) in others {
            let o = Arc::new(Analysis::new(uri.to_string(), t, Arc::default(), true));
            k.set(uri, Source::Disk, o);
        }
        let pos = a.position(Encoding::Utf16, cursor as u32);
        At { a, k, pos, text }
    }

    impl At {
        fn list(&self, snippets: bool) -> CompletionList {
            match super::super::complete_in(&self.a, &self.k, Encoding::Utf16, self.pos, snippets) {
                CompletionResponse::List(l) => l,
                CompletionResponse::Array(items) => CompletionList {
                    is_incomplete: false,
                    items,
                },
            }
        }

        fn items(&self) -> Vec<CompletionItem> {
            self.list(false).items
        }

        fn labels(&self) -> Vec<String> {
            self.items().into_iter().map(|i| i.label).collect()
        }

        fn item(&self, label: &str) -> CompletionItem {
            let items = self.items();
            let labels: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
            items
                .into_iter()
                .find(|i| i.label == label)
                .unwrap_or_else(|| panic!("{label} not in {labels:?}"))
        }

        fn range(&self, from: usize, to: usize) -> Range {
            Range::new(
                self.a.position(Encoding::Utf16, from as u32),
                self.a.position(Encoding::Utf16, to as u32),
            )
        }

        /// The text with `item` accepted: its edits applied, a snippet's
        /// placeholders left empty.
        fn accepted(&self, item: &CompletionItem) -> String {
            let mut edits: Vec<TextEdit> = item.additional_text_edits.clone().unwrap_or_default();
            match &item.text_edit {
                Some(CompletionTextEdit::Edit(e)) => edits.push(e.clone()),
                other => panic!("{other:?}"),
            }
            let offset = |p: Position| self.a.offset(Encoding::Utf16, p).unwrap() as usize;
            edits.sort_by_key(|e| std::cmp::Reverse(offset(e.range.start)));
            let mut out = self.text.clone();
            for e in edits {
                let mut new = e.new_text.clone();
                if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
                    for n in 0..4 {
                        new = new.replace(&format!("${n}"), "");
                    }
                }
                out.replace_range(offset(e.range.start)..offset(e.range.end), &new);
            }
            out
        }
    }

    fn text_of(item: &CompletionItem) -> String {
        match &item.text_edit {
            Some(CompletionTextEdit::Edit(e)) => e.new_text.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn has(labels: &[String], want: &[&str], not: &[&str], what: &str) {
        for w in want {
            assert!(
                labels.iter().any(|l| l == w),
                "{what}: {w} not in {labels:?}"
            );
        }
        for n in not {
            assert!(!labels.iter().any(|l| l == n), "{what}: {n} in {labels:?}");
        }
    }

    /// A small ontology: a class, an object property, a data property, an
    /// annotation property and an individual, all of `http://example.org/o#`.
    fn onto(axioms: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n\
             <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n\
             \x20   <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n\
             \x20   <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n\
             \x20   <Declaration><Class IRI=\"http://example.org/o#Pump\"/></Declaration>\n\
             \x20   <Declaration><ObjectProperty abbreviatedIRI=\":feeds\"/></Declaration>\n\
             \x20   <Declaration><DataProperty abbreviatedIRI=\":power\"/></Declaration>\n\
             \x20   <Declaration><AnnotationProperty abbreviatedIRI=\":note\"/></Declaration>\n\
             \x20   <Declaration><NamedIndividual abbreviatedIRI=\":p1\"/></Declaration>\n\
             \x20   {axioms}\n\
             </Ontology>\n"
        )
    }

    /// Stories 1 and 2.
    #[test]
    fn inside_the_ontology_its_children() {
        let labels = at_doc(&onto("<|")).labels();
        has(
            &labels,
            &[
                "SubClassOf",
                "Declaration",
                "EquivalentClasses",
                "AnnotationAssertion",
                "DLSafeRule",
            ],
            &[
                "Prefix",
                "Import",
                "Annotation",
                "Class",
                "ObjectSomeValuesFrom",
                ":Pump",
            ],
            "after the axioms",
        );
        let between_prefixes = onto("").replacen(
            "    <Prefix name=\"rdfs\"",
            "    <|\n    <Prefix name=\"rdfs\"",
            1,
        );
        let labels = at_doc(&between_prefixes).labels();
        has(
            &labels,
            &["Prefix"],
            &["Import", "Annotation", "SubClassOf"],
            "before a prefix",
        );
        let after_prefixes = onto("").replacen("    <Declaration>", "    <|\n    <Declaration>", 1);
        let labels = at_doc(&after_prefixes).labels();
        has(
            &labels,
            &["Prefix", "Import", "Annotation", "SubClassOf"],
            &[],
            "after all prefixes",
        );
    }

    /// Stories 3, 4 and 5.
    #[test]
    fn inside_a_construct_what_its_signature_takes() {
        let labels = at_doc(&onto("<SubClassOf>\n        <|")).labels();
        has(
            &labels,
            &[
                "Class",
                "ObjectIntersectionOf",
                "ObjectSomeValuesFrom",
                "DataHasValue",
                "Annotation",
            ],
            &[
                "ObjectProperty",
                "SubClassOf",
                "Literal",
                "IRI",
                "NamedIndividual",
            ],
            "SubClassOf",
        );
        // The class first.
        assert_eq!(labels[0], "Class");
        let some = "<SubClassOf><Class abbreviatedIRI=\":Pump\"/><ObjectSomeValuesFrom><ObjectProperty abbreviatedIRI=\":feeds\"/><|";
        has(
            &at_doc(&onto(some)).labels(),
            &["Class", "ObjectUnionOf"],
            &["ObjectProperty", "ObjectInverseOf"],
            "second operand",
        );
        // What is typed narrows them, and is what the item replaces.
        let text = onto("<SubClassOf><Class abbreviatedIRI=\":Pump\"/><ObjectSo|");
        let at = at_doc(&text);
        let labels = at.labels();
        assert_eq!(labels[0], "ObjectSomeValuesFrom", "{labels:?}");
        assert!(!labels.iter().any(|l| l == "Class"), "{labels:?}");
        let item = at.item("ObjectSomeValuesFrom");
        let start = text.find("ObjectSo|").unwrap();
        let Some(CompletionTextEdit::Edit(edit)) = item.text_edit else {
            panic!()
        };
        assert_eq!(edit.range, at.range(start, start + 8));
        assert_eq!(
            item.detail.as_deref(),
            Some("ObjectSomeValuesFrom(objectProperty class)")
        );
    }

    /// Stories 6 and 7.
    #[test]
    fn with_snippets_an_element_is_written_whole() {
        let at = at_doc(&onto("<SubClassOf><|"));
        let items = at.list(true).items;
        let item = |label: &str| items.iter().find(|i| i.label == label).unwrap().clone();
        assert_eq!(text_of(&item("Class")), "Class IRI=\"$1\"/>");
        assert_eq!(
            item("Class").insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
        assert_eq!(
            text_of(&item("ObjectSomeValuesFrom")),
            "ObjectSomeValuesFrom>$0</ObjectSomeValuesFrom>"
        );
        assert_eq!(
            text_of(&item("ObjectMinCardinality")),
            "ObjectMinCardinality cardinality=\"$1\">$0</ObjectMinCardinality>"
        );
        // Without snippets, the name.
        let plain = at.item("Class");
        assert_eq!(text_of(&plain), "Class");
        assert_eq!(plain.insert_text_format, None);
        // A name in a whole tag: nothing, as any other name would leave
        // its attributes or its end tag behind.
        for written in [
            "<SubClassOf><Cl| IRI=\"x\"/>",
            "<SubClassOf><Cl|/>",
            "<SubClassOf><Cl|>",
            "<Sub|ClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>",
        ] {
            assert!(at_doc(&onto(written)).labels().is_empty(), "{written}");
        }
        // The document's prefix form, in the end tag too.
        let text = "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\">\n  <owl:SubClassOf>\n    <owl:Obj|\n  </owl:SubClassOf>\n</owl:Ontology>\n";
        let items = at_doc(text).list(true).items;
        let some = items
            .iter()
            .find(|i| i.label == "owl:ObjectSomeValuesFrom")
            .unwrap();
        assert_eq!(
            text_of(some),
            "owl:ObjectSomeValuesFrom>$0</owl:ObjectSomeValuesFrom>"
        );
        let start = text.find("owl:Obj|").unwrap();
        let Some(CompletionTextEdit::Edit(edit)) = &some.text_edit else {
            panic!()
        };
        assert_eq!(edit.range, at_doc(text).range(start, start + 7));
    }

    /// Stories 8 and 9: an IRI value takes the terms of its element's kind,
    /// in full, this file's and the workspace's.
    #[test]
    fn an_iri_value_takes_full_iris_of_its_element_s_kind() {
        let cases: [(&str, &[&str], &[&str]); 6] = [
            (
                "<SubClassOf><Class IRI=\"|\"/>",
                &["http://example.org/o#Pump"],
                &[
                    "http://example.org/o#feeds",
                    "http://example.org/o#p1",
                    ":Pump",
                ],
            ),
            (
                "<ObjectPropertyDomain><ObjectProperty IRI=\"|\"/>",
                &["http://example.org/o#feeds"],
                &["http://example.org/o#Pump", "http://example.org/o#power"],
            ),
            (
                "<DataPropertyDomain><DataProperty IRI=\"|\"/>",
                &["http://example.org/o#power"],
                &["http://example.org/o#feeds"],
            ),
            (
                "<ClassAssertion><Class abbreviatedIRI=\":Pump\"/><NamedIndividual IRI=\"|\"/>",
                &["http://example.org/o#p1"],
                &["http://example.org/o#Pump"],
            ),
            (
                "<AnnotationAssertion><AnnotationProperty IRI=\"|\"/>",
                &[
                    "http://example.org/o#note",
                    "http://www.w3.org/2000/01/rdf-schema#label",
                ],
                &["http://example.org/o#feeds", "http://example.org/o#Pump"],
            ),
            (
                "<DataPropertyRange><DataProperty abbreviatedIRI=\":power\"/><Datatype IRI=\"http://www.w3.org/2001/XMLSchema#|\"/>",
                &[
                    "http://www.w3.org/2001/XMLSchema#integer",
                    "http://www.w3.org/2001/XMLSchema#string",
                ],
                &[
                    "http://www.w3.org/2001/XMLSchema#minInclusive",
                    "http://example.org/o#Pump",
                ],
            ),
        ];
        for (axiom, want, not) in cases {
            has(&at_doc(&onto(axiom)).labels(), want, not, axiom);
        }
        // The value whole is replaced, and the item is the IRI.
        let text = onto("<SubClassOf><Class IRI=\"http://example.org/o#P|ump\"/>");
        let at = at_doc(&text);
        let item = at.item("http://example.org/o#Pump");
        let start = text.find("http://example.org/o#P|").unwrap();
        let Some(CompletionTextEdit::Edit(edit)) = &item.text_edit else {
            panic!()
        };
        assert_eq!(edit.range, at.range(start, start + 25));
        assert_eq!(item.additional_text_edits, None);
        // A literal's datatype.
        let literal = "<DataPropertyAssertion><DataProperty abbreviatedIRI=\":power\"/><NamedIndividual abbreviatedIRI=\":p1\"/><Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#int|\">1</Literal>";
        has(
            &at_doc(&onto(literal)).labels(),
            &["http://www.w3.org/2001/XMLSchema#integer"],
            &[],
            literal,
        );
    }

    /// Story 8 in a document with no `<Prefix>`, the class described only
    /// in another file of the workspace; story 10.
    #[test]
    fn the_workspace_s_terms_by_local_name_or_by_iri() {
        let turtle = "@prefix : <http://example.org/onto#> .\n\
                      @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                      :Person a owl:Class .\n\
                      :Paper a owl:Class .\n\
                      :knows a owl:ObjectProperty .\n";
        let others = [("file:///w/people.ttl", turtle)];
        let bare = |value: &str| {
            format!(
                "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n  <SubClassOf>\n    <Class IRI=\"{value}\"/>\n  </SubClassOf>\n</Ontology>\n"
            )
        };
        let person = "http://example.org/onto#Person";
        let labels = at(&bare("|"), &others).labels();
        has(
            &labels,
            &[person, "http://example.org/onto#Paper"],
            &["http://example.org/onto#knows"],
            "empty",
        );
        for typed in [
            "Per|",
            "http://example.org/onto#Per|",
            "http://example.org/on|",
            "http://example.org/onto#|",
        ] {
            let at = at(&bare(typed), &others);
            let labels = at.labels();
            has(
                &labels,
                &[person],
                &["http://example.org/onto#knows"],
                typed,
            );
            if typed.starts_with("Per") {
                // The one whose local name starts with what is typed first.
                assert_eq!(labels[0], person, "{labels:?}");
                // The client filters on the local name.
                assert_eq!(at.item(person).filter_text.as_deref(), Some("Person"));
            } else {
                assert_eq!(at.item(person).filter_text.as_deref(), Some(person));
            }
        }
        // The vocabularies' by local name, from two characters.
        let labels = at(&bare("Th|"), &others).labels();
        has(&labels, &["http://www.w3.org/2002/07/owl#Thing"], &[], "Th");
        // An IRI with `&` is escaped in the value.
        let turtle = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.org/q?a=1&b=2#C> a owl:Class .\n";
        let at = at(&bare("|"), &[("file:///w/q.ttl", turtle)]);
        let item = at.item("http://example.org/q?a=1&b=2#C");
        assert_eq!(text_of(&item), "http://example.org/q?a=1&amp;b=2#C");
    }

    /// Stories 11, 12 and 13: `prefix:local` over the prefixes the document
    /// declares; one it does not declare adds its `<Prefix>`.
    #[test]
    fn an_abbreviated_iri_takes_the_declared_prefixes() {
        let text =
            onto("<SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\"|\"/>");
        let labels = at_doc(&text).labels();
        has(
            &labels,
            &[":Pump", ":", "rdfs:"],
            &[":feeds", "http://example.org/o#Pump", "skos:", "owl:"],
            "empty",
        );
        // Story 13.
        let text = onto("<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"|\"/>");
        has(
            &at_doc(&text).labels(),
            &[":note", "rdfs:label", "rdfs:comment"],
            &[":Pump", ":feeds"],
            "annotation",
        );
        // Story 12: after the last `<Prefix>`, indented as it is.
        let text = onto(
            "<SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\"skos:Con|\"/>",
        );
        let at = at_doc(&text);
        let concept = at.item("skos:Concept");
        let skos = "\n    <Prefix name=\"skos\" IRI=\"http://www.w3.org/2004/02/skos/core#\"/>";
        let end = text.find("rdf-schema#\"/>").unwrap() + 14;
        let pos = at.range(end, end).start;
        assert_eq!(
            concept.additional_text_edits,
            Some(vec![TextEdit::new(Range::new(pos, pos), skos.into())])
        );
        assert_eq!(
            concept.label_details.and_then(|d| d.description).as_deref(),
            Some("+Prefix")
        );
        let start = text.find("skos:Con|").unwrap();
        assert_eq!(
            at.item("skos:Concept").text_edit,
            Some(CompletionTextEdit::Edit(TextEdit::new(
                at.range(start, start + 8),
                "skos:Concept".into()
            )))
        );
        // None declared: the ontology's first child; `rdfs` is no exception.
        let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n  <AnnotationAssertion>\n    <AnnotationProperty abbreviatedIRI=\"rdfs:lab|\"/>\n</Ontology>\n";
        let at = at_doc(text);
        let label = at.item("rdfs:label");
        let rdfs = "\n  <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>";
        let pos = Position::new(0, 49);
        assert_eq!(
            label.additional_text_edits,
            Some(vec![TextEdit::new(Range::new(pos, pos), rdfs.into())])
        );
        let after = at.accepted(&label);
        let re = Analysis::new(URI.into(), &after, Arc::default(), true);
        assert!(
            re.parsed.prefixes.iter().any(|p| p.name == "rdfs"),
            "{after}"
        );
        // A name the document binds to another namespace is not reused.
        let text = onto("<SubClassOf><Class abbreviatedIRI=\"rdfs:|\"/>").replace(
            "rdf-schema#\"/>",
            "rdf-schema#\"/>\n    <Prefix name=\"skos\" IRI=\"http://example.org/skos#\"/>",
        );
        let text = text.replacen("rdfs:|", "skos:Con|", 1);
        let labels = at_doc(&text).labels();
        assert!(!labels.iter().any(|l| l == "skos:Concept"), "{labels:?}");
    }

    /// Story 14: an `<IRI>` element's text; story 16: nothing in a literal,
    /// a comment, the prolog or a `nodeID`.
    #[test]
    fn an_iri_element_s_text_and_where_nothing_is_offered() {
        let assertion = "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/>";
        let text = onto(&format!("{assertion}<IRI>http://example.org/o#|</IRI>"));
        has(
            &at_doc(&text).labels(),
            &["http://example.org/o#Pump", "http://example.org/o#p1"],
            &[],
            "IRI",
        );
        let text = onto(&format!("{assertion}<AbbreviatedIRI>:|</AbbreviatedIRI>"));
        has(
            &at_doc(&text).labels(),
            &[":Pump", ":feeds"],
            &[],
            "AbbreviatedIRI",
        );
        for nothing in [
            format!("{assertion}<IRI>http://example.org/o#Pump</IRI><Literal>a pu|</Literal>"),
            "<!-- <SubClassOf><Cl| -->".to_string(),
            format!("{assertion}<AnonymousIndividual nodeID=\"_:a|\"/>"),
            "<SubClassOf>\n      |\n    </SubClassOf>".to_string(),
        ] {
            let labels = at_doc(&onto(&nothing)).labels();
            assert!(labels.is_empty(), "{nothing}: {labels:?}");
        }
        let prolog = onto("").replacen("version=\"1.0\"", "version=\"1|.0\"", 1);
        assert!(at_doc(&prolog).labels().is_empty());
    }

    /// Story 15: an unclosed tag above, a value not closed, the next item
    /// already written.
    #[test]
    fn a_document_half_typed_is_read_as_far_as_it_goes() {
        let cases = [
            (
                "<SubClassOf><Class abbreviatedIRI=\":Pump\"/><ObjectSomeValuesFrom><ObjectProperty abbreviatedIRI=\":feeds\"/>\n    <|\n    <SubClassOf>",
                "Class",
                "ObjectProperty",
            ),
            (
                "<SubClassOf><Class abbreviatedIRI=\":Pu|\n    <Declaration><Class abbreviatedIRI=\":B\"/></Declaration>",
                ":Pump",
                ":feeds",
            ),
            (
                "<ClassAssertion><Class abbreviatedIRI=\":Pump\"/><NamedIndividual abbreviatedIRI=\"|",
                ":p1",
                ":Pump",
            ),
        ];
        for (axiom, want, not) in cases {
            has(&at_doc(&onto(axiom)).labels(), &[want], &[not], axiom);
        }
        // The value cut by a line break: nothing on the next line.
        let next =
            onto("<SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\"\n    |");
        assert!(at_doc(&next).labels().is_empty());
    }

    /// Story 36: every item of these positions, accepted, leaves the
    /// document with no more errors than it had; three of them have none.
    /// An element's name is accepted with snippets, as the spec writes it
    /// whole only then: without, its bare name leaves its tag open, as a
    /// Functional keyword waits for its `(`.
    #[test]
    fn an_accepted_item_adds_no_error() {
        let errors = |text: &str| {
            Analysis::new(URI.into(), text, Arc::default(), true)
                .parsed
                .errors
                .len()
        };
        let assertion = |property: &str, subject: &str| {
            format!(
                "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"{property}\"/><IRI>{subject}</IRI><Literal>A pump</Literal></AnnotationAssertion>"
            )
        };
        let pump = "http://example.org/o#Pump";
        let sub = "<SubClassOf><Class abbreviatedIRI=\":Pump\"/><|</SubClassOf>";
        // (axiom, snippets, whether the document has no error before).
        let cases = [
            ("<SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\"|\"/></SubClassOf>".to_string(), false, false),
            ("<ClassAssertion><Class abbreviatedIRI=\":Pump\"/><NamedIndividual IRI=\"|\"/></ClassAssertion>".to_string(), false, true),
            (assertion("sk|", pump), false, false),
            (sub.to_string(), true, false),
            (assertion("rd|fs:label", pump), false, true),
            ("<SubClassOf><Class IRI=\"http://example.org/o#P|ump\"/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>".to_string(), false, true),
            (assertion("rdfs:label", "http://example.org/o#|Pump"), false, true),
        ];
        for (axiom, snippets, clean) in cases {
            let at = at_doc(&onto(&axiom));
            let before = errors(&at.text);
            assert_eq!(before == 0, clean, "{axiom}");
            let items = at.list(snippets).items;
            assert!(!items.is_empty(), "{axiom}");
            for item in &items {
                let after = at.accepted(item);
                assert!(
                    errors(&after) <= before,
                    "{axiom}: {} gives\n{after}",
                    item.label
                );
            }
        }
    }

    /// The errors (severity error) of OWL/XML `text`.
    fn errors(text: &str) -> Vec<String> {
        let a = Analysis::new(URI.into(), text, Arc::default(), true);
        let errors = a.parsed.errors.iter();
        let errors = errors.filter(|e| e.severity == crate::syntax::Severity::Error);
        errors.map(|e| e.message().to_string()).collect()
    }

    /// A `<Prefix>` added on accept is written as the root's name is: in
    /// the prefix form of `<owl:Ontology>`, OWL's namespace.
    #[test]
    fn an_added_prefix_is_written_as_the_root_is() {
        let text = "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <owl:Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n  <owl:Declaration><owl:Class abbreviatedIRI=\"skos:Conc|\"/></owl:Declaration>\n</owl:Ontology>\n";
        let at = at_doc(text);
        let after = at.accepted(&at.item("skos:Concept"));
        assert!(
            after.contains(
                "\n  <owl:Prefix name=\"skos\" IRI=\"http://www.w3.org/2004/02/skos/core#\"/>"
            ),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new(), "{after}");
    }

    /// A prefix a `<Prefix>` element names, misplaced, malformed or with
    /// an IRI the reader cannot read, is not declared again (it would be
    /// declared twice): its terms are not offered.
    #[test]
    fn a_prefix_some_prefix_element_names_is_not_declared_again() {
        let skos = "http://www.w3.org/2004/02/skos/core#";
        for prefix in [
            format!("<Prefix name=\"skos\" IRI=\"{skos}\"/>"),
            format!("<Prefix name=\"skos\"IRI=\"{skos}\"/>"),
            "<Prefix name=\"skos\" IRI=\"&skos;\"/>".to_string(),
        ] {
            let text = onto(&format!(
                "<Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n    {prefix}\n    <SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\"skos:Conc|\"/></SubClassOf>"
            ));
            let at = at_doc(&text);
            let labels = at.labels();
            assert!(
                !labels.iter().any(|l| l == "skos:Concept"),
                "{prefix}: {labels:?}"
            );
            for item in at.items() {
                let after = at.accepted(&item);
                let now = errors(&after);
                assert!(
                    !now.iter().any(|m| m.contains("declared twice")),
                    "{now:?}\n{after}"
                );
            }
        }
    }

    /// A value the scanner ends at a `<` or at the end of the text, with no
    /// closing quote: its edit replaces it whole, as the reader reads it.
    #[test]
    fn a_value_not_closed_is_replaced_whole() {
        let xsd = "http://www.w3.org/2001/XMLSchema#integer";
        for tail in ["</Declaration>\n</Ontology>\n", ""] {
            let text = format!(
                "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Declaration><Datatype IRI=\"http://www.w3.org/2001/|XMLSchema#integer{tail}"
            );
            let at = at_doc(&text);
            let before = errors(&at.text);
            let after = at.accepted(&at.item(xsd));
            assert!(
                after.contains(&format!("IRI=\"{xsd}{}", &tail[..tail.len().min(1)])),
                "{after}"
            );
            assert!(errors(&after).len() <= before.len(), "{before:?}\n{after}");
        }
    }

    /// A value with no closing quote, which the reader reads to the next
    /// `<`: its edit stops at the first character an IRI cannot hold (a
    /// `>`, white space) and before the `/` of a `/>`, so the end of the
    /// tag and what comes after it stay. The line after accepting, whole.
    #[test]
    fn an_unclosed_value_s_edit_keeps_the_end_of_its_tag() {
        let int = "http://www.w3.org/2001/XMLSchema#integer";
        let xsd = "http://www.w3.org/2001/XMLSchema#";
        let pump = "http://example.org/o#Pump";
        let literal = |l: &str| {
            format!(
                "<DataPropertyAssertion><DataProperty abbreviatedIRI=\":power\"/><NamedIndividual abbreviatedIRI=\":p1\"/>{l}</DataPropertyAssertion>"
            )
        };
        // The whole text ends in the line.
        let last = |line: &str| {
            format!(
                "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  {line}"
            )
        };
        let cases = [
            // `>` and text after the value.
            (
                onto(&literal(&format!(
                    "<Literal datatypeIRI=\"{xsd}int|>42</Literal>"
                ))),
                int,
                format!(
                    "    {}",
                    literal(&format!("<Literal datatypeIRI=\"{int}>42</Literal>"))
                ),
            ),
            (
                onto(&literal(&format!(
                    "<Literal datatypeIRI=\"{xsd}in|t>42</Literal>"
                ))),
                int,
                format!(
                    "    {}",
                    literal(&format!("<Literal datatypeIRI=\"{int}>42</Literal>"))
                ),
            ),
            // `/>` after the value.
            (
                onto(
                    "<SubClassOf><Class IRI=\"http://example.org/o#P|/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>",
                ),
                pump,
                format!(
                    "    <SubClassOf><Class IRI=\"{pump}/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"
                ),
            ),
            (
                onto(
                    "<SubClassOf><Class IRI=\"http://example.org/o#P|um/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>",
                ),
                pump,
                format!(
                    "    <SubClassOf><Class IRI=\"{pump}/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"
                ),
            ),
            (
                onto("<SubClassOf><Class IRI=\"|/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"),
                pump,
                format!(
                    "    <SubClassOf><Class IRI=\"{pump}/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"
                ),
            ),
            // `>` and a child element at once.
            (
                onto(
                    "<SubClassOf><Class IRI=\"http://example.org/o#P|><Class abbreviatedIRI=\":Pump\"/></SubClassOf>",
                ),
                pump,
                format!(
                    "    <SubClassOf><Class IRI=\"{pump}><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"
                ),
            ),
            // At the end of the text.
            (
                last(&format!("<Declaration><Datatype IRI=\"{xsd}int|/>")),
                int,
                format!("  <Declaration><Datatype IRI=\"{int}/>"),
            ),
            (
                last(&format!("<Declaration><Datatype IRI=\"{xsd}int|>")),
                int,
                format!("  <Declaration><Datatype IRI=\"{int}>"),
            ),
            (
                last(&format!("<Declaration><Datatype IRI=\"{xsd}int|")),
                int,
                format!("  <Declaration><Datatype IRI=\"{int}"),
            ),
            // White space, then an attribute in the other quotes: to the
            // white space, as before.
            (
                onto(
                    "<SubClassOf><Class IRI=\"http://example.org/o#P| abbreviatedIRI=':Pump'/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>",
                ),
                pump,
                format!(
                    "    <SubClassOf><Class IRI=\"{pump} abbreviatedIRI=':Pump'/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"
                ),
            ),
            // Closed: between the quotes, as before.
            (
                onto(
                    "<SubClassOf><Class IRI=\"http://example.org/o#P|\"/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>",
                ),
                pump,
                format!(
                    "    <SubClassOf><Class IRI=\"{pump}\"/><Class abbreviatedIRI=\":Pump\"/></SubClassOf>"
                ),
            ),
            (
                onto(&literal(&format!(
                    "<Literal datatypeIRI=\"{xsd}int|\">42</Literal>"
                ))),
                int,
                format!(
                    "    {}",
                    literal(&format!("<Literal datatypeIRI=\"{int}\">42</Literal>"))
                ),
            ),
        ];
        let mut wrong = Vec::new();
        for (marked, label, want) in cases {
            let at = at_doc(&marked);
            let line = marked[..marked.find('|').unwrap()].matches('\n').count();
            let Some(item) = at.items().into_iter().find(|i| i.label == label) else {
                wrong.push(format!("{label} not offered in {marked}"));
                continue;
            };
            let after = at.accepted(&item);
            let got = after.lines().nth(line).unwrap_or_default().to_string();
            if got != want {
                wrong.push(format!("want {want}\n got {got}"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
        // A `>` before the offset: no edit covers it.
        let at = at_doc(&onto(&literal(&format!(
            "<Literal datatypeIRI=\"{xsd}int>4|2</Literal>"
        ))));
        for item in at.items() {
            let after = at.accepted(&item);
            assert!(after.contains(">42</Literal>"), "{}: {after}", item.label);
        }
    }

    /// An `IRI` or `AbbreviatedIRI` element's text is offered only in its
    /// first run, after its start tag: past a wrong end tag the element is
    /// still open, and its text is all its runs.
    #[test]
    fn an_iri_element_s_text_only_after_its_start_tag() {
        let text = onto(
            "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/>\n        <AbbreviatedIRI>:Pu</AbbreviatedIRX>mp</Literal>\n|    </AnnotationAssertion>",
        );
        let at = at_doc(&text);
        let before = errors(&at.text);
        for item in at.items() {
            let after = at.accepted(&item);
            assert!(
                errors(&after).len() <= before.len(),
                "{}: {after}",
                item.label
            );
        }
    }

    /// Where the root's start tag is cut by a `<` in a value, no `<Prefix>`
    /// can go after it: a term that needs one is not offered.
    #[test]
    fn nothing_is_declared_in_a_cut_root_tag() {
        let text = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\" xml:lang=\"en<Declaration>\">\n  <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"skos:Conc|\"/></SubClassOf>\n</Ontology>\n";
        let at = at_doc(text);
        let before = errors(&at.text);
        for item in at.items() {
            assert!(item.additional_text_edits.is_none(), "{}", item.label);
            let after = at.accepted(&item);
            assert!(
                errors(&after).len() <= before.len(),
                "{}: {after}",
                item.label
            );
        }
    }

    /// Before a `<Prefix>`, only `<Prefix>` is offered, so accepting an
    /// ontology child never moves a `<Prefix>` after an import, annotation or
    /// axiom.
    #[test]
    fn before_a_prefix_only_prefix_is_offered() {
        let text = "<?xml version=\"1.0\"?>\n\
            <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n\
                <|\n\
                <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n\
                <Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n\
            </Ontology>\n";
        let at = at_doc(text);
        assert_eq!(at.labels(), ["Prefix"]);
        // Typing `<Im` before `<Prefix>` does not offer `Import`.
        let typing_import = text.replacen("<|", "<Im|", 1);
        assert!(at_doc(&typing_import).labels().is_empty());
        // Accepting the `Prefix` snippet and filling it leaves 0 errors.
        let list = at.list(true);
        let prefix = list.items.iter().find(|i| i.label == "Prefix").unwrap();
        let after = at.accepted(prefix).replacen(
            "<Prefix name=\"\" IRI=\"\"/>",
            "<Prefix name=\"other\" IRI=\"http://example.org/other#\"/>",
            1,
        );
        assert_eq!(errors(&after), Vec::<String>::new(), "{after}");
        // After all `<Prefix>` elements and before `<Import>`: only `Prefix` and `Import`.
        let before_import = "<?xml version=\"1.0\"?>\n\
            <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n\
                <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n\
                <|\n\
                <Import>http://example.org/other</Import>\n\
                <Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n\
            </Ontology>\n";
        let labels = at_doc(before_import).labels();
        has(
            &labels,
            &["Prefix", "Import"],
            &["Annotation", "SubClassOf"],
            "before import",
        );
    }

    /// A closed attribute value with leading or trailing white space is
    /// replaced whole between its quotes, never with a zero-width edit.
    #[test]
    fn a_closed_value_with_leading_space_is_replaced_whole() {
        let ttl = "@prefix ex: <http://example.org/> .\n\
                   @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                   ex:Person a owl:Class .\n";
        let text = onto("<Declaration><Class IRI=\"| http://example.org/B \"/></Declaration>");
        let a = at(&text, &[("file:///w/terms.ttl", ttl)]);
        assert_eq!(errors(&a.text), Vec::<String>::new());
        let item = a.item("http://example.org/Person");
        let after = a.accepted(&item);
        assert!(
            after.contains("<Class IRI=\"http://example.org/Person\"/>"),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new(), "{after}");
        // Same for `<IRI>` element text with leading and trailing spaces.
        let text = onto(
            "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><IRI>| http://example.org/B </IRI><Literal>x</Literal></AnnotationAssertion>",
        );
        let a = at(&text, &[("file:///w/terms.ttl", ttl)]);
        assert_eq!(errors(&a.text), Vec::<String>::new());
        let after = a.accepted(&a.item("http://example.org/Person"));
        assert!(
            after.contains("<IRI>http://example.org/Person </IRI>"),
            "{after}"
        );
        assert_eq!(errors(&after), Vec::<String>::new(), "{after}");
    }

    /// Full-IRI completion offers valid workspace IRIs whose local part is
    /// not a `PN_LOCAL` (such as `+`), both by full IRI and by local name,
    /// while `abbreviatedIRI` keeps requiring `PN_LOCAL`.
    #[test]
    fn full_iri_offers_valid_iris_outside_pn_local() {
        let ttl = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                   <http://example.org/A+B> a owl:Class .\n\
                   <http://example.org/Apple> a owl:Class .\n";
        let others = [("file:///w/terms.ttl", ttl)];
        for axiom in [
            "<Declaration><Class IRI=\"http://example.org/A|\"/></Declaration>",
            "<Declaration><Class IRI=\"http://example.org/A+|\"/></Declaration>",
            "<Declaration><Class IRI=\"A|\"/></Declaration>",
            "<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><IRI>http://example.org/A|</IRI><Literal>x</Literal></AnnotationAssertion>",
        ] {
            let at = at(&onto(axiom), &others);
            let labels = at.labels();
            has(&labels, &["http://example.org/A+B"], &[], axiom);
            let after = at.accepted(&at.item("http://example.org/A+B"));
            assert_eq!(errors(&after), Vec::<String>::new(), "{axiom}:\n{after}");
        }
        // `abbreviatedIRI` still omits `A+B`, which is not a `PN_LOCAL`.
        let abbr = onto("<Declaration><Class abbreviatedIRI=\"ex:A|\"/></Declaration>").replacen(
            "    <Prefix name=\"rdfs\"",
            "    <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n    <Prefix name=\"rdfs\"",
            1,
        );
        let labels = at(&abbr, &others).labels();
        has(&labels, &["ex:Apple"], &["ex:A+B"], "abbreviatedIRI");
    }

    #[test]
    fn closing_tag_and_attribute_completion() {
        // Closing tag without `>` appends `>`.
        let a = at_doc(&onto(
            "<SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\":Valve\"/></|",
        ));
        assert_eq!(a.labels(), ["SubClassOf"]);
        let after = a.accepted(&a.item("SubClassOf"));
        assert!(after.contains("</SubClassOf>"), "{after}");
        assert_eq!(errors(&after), Vec::<String>::new(), "{after}");
        // Closing tag with `>` replaces only the name.
        let a = at_doc(&onto(
            "<SubClassOf><Class abbreviatedIRI=\":Pump\"/><Class abbreviatedIRI=\":Valve\"/></SubCl|>",
        ));
        assert_eq!(a.labels(), ["SubClassOf"]);
        let after = a.accepted(&a.item("SubClassOf"));
        assert!(after.contains("</SubClassOf>"), "{after}");
        assert_eq!(errors(&after), Vec::<String>::new(), "{after}");
        // Entity tag attributes: IRI and abbreviatedIRI, mutually exclusive.
        let a = at_doc(&onto("<Declaration><Class |/></Declaration>"));
        assert_eq!(a.labels(), ["IRI", "abbreviatedIRI"]);
        let a_done = at_doc(&onto(
            "<Declaration><Class IRI=\"http://example.org/o#A\" |/></Declaration>",
        ));
        assert!(a_done.labels().is_empty(), "{:?}", a_done.labels());
        // Snippet vs plain edit on attribute names.
        let a = at_doc(&onto("<Declaration><Class ab|/></Declaration>"));
        assert_eq!(a.labels(), ["abbreviatedIRI"]);
        let snip = a.list(true);
        assert_eq!(
            snip.items[0].insert_text_format,
            Some(InsertTextFormat::SNIPPET)
        );
        assert!(
            matches!(&snip.items[0].text_edit, Some(CompletionTextEdit::Edit(e)) if e.new_text == "abbreviatedIRI=\"$0\""),
            "{:?}",
            snip.items[0].text_edit
        );
        // Literal, Prefix, FacetRestriction, AnonymousIndividual, Ontology attributes.
        assert_eq!(
            at_doc(&onto("<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><IRI>http://example.org/o#A</IRI><Literal |>x</Literal></AnnotationAssertion>")).labels(),
            ["datatypeIRI", "xml:lang"]
        );
        assert_eq!(at_doc(&onto("<Prefix name=\"ex\" |/>")).labels(), ["IRI"]);
        assert_eq!(
            at_doc(&onto("<DatatypeDefinition><Datatype abbreviatedIRI=\":D\"/><DatatypeRestriction><Datatype abbreviatedIRI=\"xsd:integer\"/><FacetRestriction |><Literal>1</Literal></FacetRestriction></DatatypeRestriction></DatatypeDefinition>")).labels(),
            ["facet", "xml:lang"]
        );
        assert_eq!(
            at_doc(&onto("<ClassAssertion><Class abbreviatedIRI=\":Pump\"/><AnonymousIndividual |/></ClassAssertion>")).labels(),
            ["nodeID"]
        );
        let root_attr = at_doc(
            "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\" |>\n</Ontology>\n",
        );
        assert_eq!(root_attr.labels(), ["versionIRI", "xml:base", "xml:lang"]);
    }
}
