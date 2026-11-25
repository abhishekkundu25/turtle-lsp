//! Completion in OWL Functional syntax, the one edit turtle-lsp makes in a
//! Functional document.
//!
//! What may come at the cursor is the grammar's ([`crate::ofn::expected`]),
//! read from the tokens before it, so it holds while the document is half
//! typed: inside `Ontology( … )` the keywords of its items, inside a
//! construct the keywords of the expressions its next operand may be, with
//! the entities of the kinds the slot names (classes in a class
//! expression's slot, object properties, data properties, datatypes,
//! individuals, annotation properties).
//!
//! - `prefix:` and part of a local name: the terms of that namespace, as in
//!   Turtle (the bundled vocabulary's and those the workspace uses), of the
//!   kinds the slot takes;
//! - anything else: the keywords, then the prefixes (declared, and
//!   well-known ones once something is typed), the terms of the declared
//!   namespaces of the right kinds, and, from two characters typed,
//!   vocabulary terms by local name. The keywords and prefixes keep their
//!   places under the cap of 200 when the terms alone would fill it.
//!
//! A term's kinds: an IRI of the reserved vocabulary has those section 2.4
//! of the structural specification allows; else those its types in the
//! workspace give (`Declaration( Class( :A ) )` types it `owl:Class`); else
//! its bundled vocabulary's (a class, any property, an individual); else
//! any property for one used as a predicate, and any kind for the rest.
//!
//! A term whose prefix the document does not declare adds its
//! `Prefix( … )` declaration on accept, after the others
//! ([`crate::ofn::expected::Header`]). With snippets, a keyword is written
//! with its operands as placeholders. A full IRI (`<http://…`) gets
//! nothing. The ranking, the items and resolve are Turtle's.

use super::{
    Best, Candidate, LIMIT, Slot, finish, fuzzy, items, local_name_candidates_where, local_of,
    namespace_terms_best, prefix_candidates, prefixed_terms_where, ranking, starts_with_lowercase,
};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::ofn::expected::{self, Header, Iris};
use crate::ofn::signature::Entity;
use crate::ofn::tree::Keyword;
use crate::syntax::Iri;
use crate::vocab::TermKind;
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
    let text: &str = &a.text;
    let Some(e) = expected::at(text, &a.parsed, offset as u32) else {
        return CompletionResponse::Array(vec![]);
    };
    let typed = &text[e.word.start as usize..offset];
    let replace = a.range(enc, e.word);
    let prefixes = expected::prefixes(&a.parsed);
    let slot = slot_of(&e.iris);
    let fits = |iri: &str, vocab: Option<TermKind>| fits(k, &e.iris, iri, vocab);
    let promoted = |_: Iri| false;
    let best = Best {
        typed,
        promoted: &promoted,
    };
    // The terms, and the keywords and prefixes, which keep their places
    // under the cap: a slot of a large ontology has more than it of terms.
    let mut cands = Vec::new();
    let mut extras = Vec::new();
    let mut incomplete = false;
    if let Some((pfx, local)) = typed.split_once(':') {
        if e.iris == Iris::None {
            return CompletionResponse::Array(vec![]);
        }
        match prefixed_terms_where(k, pfx, local, slot, &prefixes, Some(best), &fits) {
            Some(c) => cands = c,
            None => return CompletionResponse::Array(vec![]),
        }
    } else {
        incomplete = true;
        for &kw in &e.keywords {
            if fuzzy(typed, kw.as_str()) {
                extras.push(Candidate::plain(
                    kw.as_str().to_string(),
                    CompletionItemKind::KEYWORD,
                    written(kw, &|_, name| name.to_string()),
                    1,
                ));
            }
        }
        if e.iris != Iris::None {
            // The well-known prefixes once something is typed, as in Turtle.
            prefix_candidates(&mut extras, typed, &prefixes, DIRECTIVE, false);
            let mut seen = FxHashSet::default();
            for (p, ns) in prefixes.iter() {
                if seen.insert(ns.as_str()) {
                    let label = |l: &str| format!("{p}:{l}");
                    let terms =
                        namespace_terms_best(k, ns, typed, slot, &label, None, Some(best), &fits);
                    cands.extend(terms);
                }
            }
            local_name_candidates_where(&mut cands, typed, slot, &prefixes, &fits);
            // A term of a namespace in scope is found twice: keep the first.
            let mut labels = FxHashSet::default();
            cands.retain(|c| labels.insert(c.label.clone()));
        }
    }
    let ranked = ranking::Ranking::default;
    incomplete |= finish(&mut cands, ranked(), slot, typed, k, &prefixes);
    incomplete |= finish(&mut extras, ranked(), slot, typed, k, &prefixes);
    let room = LIMIT.saturating_sub(extras.len());
    if cands.len() > room {
        cands.truncate(room);
        incomplete = true;
    }
    let cands = merged(cands, extras, typed);
    let header = std::cell::OnceCell::new();
    let mut items = items(cands, replace, &prefixes, DIRECTIVE, |p, ns| {
        let header = header.get_or_init(|| Header::of(text, &a.parsed));
        // A name declared with an IRI in error: a second declaration
        // would be another error.
        if header.names.iter().any(|n| n == p) {
            return None;
        }
        let (at, new_text) = header.declare(text, p, ns);
        let pos = a.position(enc, at);
        Some(TextEdit::new(Range::new(pos, pos), new_text))
    });
    let paren_follows = text[e.word.end as usize..].trim_start().starts_with('(');
    if snippets && !paren_follows {
        for item in &mut items {
            snippet(item);
        }
    }
    debug_assert!(items.len() <= LIMIT);
    CompletionResponse::List(CompletionList {
        is_incomplete: incomplete,
        items,
    })
}

/// Two lists in [`finish`]'s order, merged in that order.
fn merged(a: Vec<Candidate>, b: Vec<Candidate>, typed: &str) -> Vec<Candidate> {
    let lower = typed.rsplit(':').next().unwrap_or("").to_lowercase();
    let key = |c: &Candidate| {
        let typed_first = !starts_with_lowercase(local_of(&c.label), &lower);
        (c.tier, c.rank, typed_first, c.label.len(), c.label.clone())
    };
    let mut out = Vec::with_capacity(a.len() + b.len());
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    loop {
        let next = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) if key(y) < key(x) => b.next(),
            (Some(_), _) => a.next(),
            (None, _) => b.next(),
        };
        match next {
            Some(c) => out.push(c),
            None => return out,
        }
    }
}

/// `kw` written with its operands, each written by `operand(number, name)`.
fn written(kw: Keyword, operand: &dyn Fn(usize, &str) -> String) -> String {
    let ops: Vec<String> = match kw {
        Keyword::Prefix => {
            return format!(
                "Prefix({}:=<{}>)",
                operand(1, "ex"),
                operand(2, "http://example.org/")
            );
        }
        Keyword::Ontology => vec![operand(1, "<http://example.org/ontology>")],
        _ => expected::operands(kw)
            .iter()
            .enumerate()
            .map(|(i, name)| operand(i + 1, name))
            .collect(),
    };
    format!("{}({})", kw.as_str(), ops.join(" "))
}

/// A keyword's item, written as a snippet with its operands.
fn snippet(item: &mut CompletionItem) {
    if item.kind != Some(CompletionItemKind::KEYWORD) {
        return;
    }
    let Some(kw) = Keyword::of(&item.label) else {
        return;
    };
    if let Some(CompletionTextEdit::Edit(edit)) = &mut item.text_edit {
        edit.new_text = written(kw, &|i, name| format!("${{{i}:{name}}}"));
        item.insert_text_format = Some(InsertTextFormat::SNIPPET);
    }
}

/// The Turtle slot whose ranking fits what is expected.
fn slot_of(iris: &Iris) -> Slot {
    match iris {
        Iris::Of(kinds) => match kinds.first() {
            Some(Entity::Class) => Slot::Class,
            Some(Entity::Datatype) => Slot::Datatype,
            Some(Entity::ObjectProperty | Entity::DataProperty | Entity::AnnotationProperty) => {
                Slot::Predicate
            }
            Some(Entity::NamedIndividual) | None => Slot::Object,
        },
        Iris::None | Iris::Any => Slot::Object,
    }
}

/// Whether the term `iri` (of kind `vocab` in the bundled vocabulary) may
/// be offered where `iris` are expected: whether one of its kinds is.
fn fits(k: &Knowledge, iris: &Iris, iri: &str, vocab: Option<TermKind>) -> bool {
    match iris {
        Iris::None => false,
        Iris::Any => true,
        Iris::Of(want) => {
            let have = kinds(k, iri, vocab);
            want.iter().any(|e| have.contains(e))
        }
    }
}

/// The kinds of entity `iri` may name: see the module doc.
fn kinds(k: &Knowledge, iri: &str, vocab: Option<TermKind>) -> Vec<Entity> {
    if let Some(r) = Entity::reserved(iri, vocab == Some(TermKind::Class)) {
        return r.to_vec();
    }
    let types = k.types(iri);
    if !types.is_empty() {
        let mut out = Vec::new();
        for t in &types {
            let of = Entity::typed_by(t).unwrap_or(&[Entity::NamedIndividual]);
            out.extend(
                of.iter()
                    .filter(|e| !out.contains(*e))
                    .copied()
                    .collect::<Vec<_>>(),
            );
        }
        return out;
    }
    let properties = vec![
        Entity::ObjectProperty,
        Entity::DataProperty,
        Entity::AnnotationProperty,
    ];
    match vocab {
        Some(TermKind::Class) => vec![Entity::Class],
        Some(TermKind::Property) => properties,
        Some(TermKind::Individual) => vec![Entity::NamedIndividual],
        None if k.used_as_predicate(iri) => properties,
        None => Entity::ALL.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::{Analysis, Encoding};
    use crate::knowledge::{Knowledge, Source};
    use lsp_types::*;
    use std::sync::Arc;

    const URI: &str = "file:///w/onto.ofn";

    /// A document whose `|` is the cursor, and the workspace around it:
    /// the document and `others` (URI, text).
    struct At {
        a: Arc<Analysis>,
        k: Knowledge,
        pos: Position,
    }

    fn at_doc(text: &str) -> At {
        at(text, &[])
    }

    fn at(text: &str, others: &[(&str, &str)]) -> At {
        let cursor = text.find('|').expect("a cursor");
        let text = text.replacen('|', "", 1);
        let a = Arc::new(Analysis::new(URI.into(), &text, Arc::default(), true));
        let mut k = Knowledge::default();
        k.set(URI, Source::Open, a.clone());
        for (uri, t) in others {
            let o = Arc::new(Analysis::new(uri.to_string(), t, Arc::default(), true));
            k.set(uri, Source::Disk, o);
        }
        let pos = a.position(Encoding::Utf16, cursor as u32);
        At { a, k, pos }
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

        /// The range the item's text replaces.
        fn replaced(&self, label: &str) -> Range {
            match self.item(label).text_edit {
                Some(CompletionTextEdit::Edit(e)) => e.range,
                other => panic!("{other:?}"),
            }
        }

        fn range(&self, from: u32, to: u32) -> Range {
            Range::new(
                self.a.position(Encoding::Utf16, from),
                self.a.position(Encoding::Utf16, to),
            )
        }
    }

    /// A small ontology: a class, an object property, a data property, an
    /// annotation property and an individual, all of `http://example.org/o#`.
    fn onto(axioms: &str) -> String {
        format!(
            "Prefix(:=<http://example.org/o#>)\n\
             Prefix(skos:=<http://www.w3.org/2004/02/skos/core#>)\n\
             Ontology(<http://example.org/o>\n\
             Declaration(Class(:Pump))\n\
             Declaration(ObjectProperty(:feeds))\n\
             Declaration(DataProperty(:power))\n\
             Declaration(AnnotationProperty(:note))\n\
             Declaration(NamedIndividual(:p1))\n\
             {axioms}\n)\n"
        )
    }

    #[test]
    fn a_prefixed_name_offers_the_terms_of_its_namespace() {
        let text = onto("SubClassOf(:Pump skos:Con|)");
        let at = at(&text, &[]);
        let concept = at.item("skos:Concept");
        // The vocabulary's label, as Turtle shows it.
        assert_eq!(concept.detail.as_deref(), Some("Concept"));
        assert_eq!(concept.kind, Some(CompletionItemKind::CLASS));
        let start = text.find("skos:Con").unwrap() as u32;
        assert_eq!(at.replaced("skos:Concept"), at.range(start, start + 8));
        // Declared already: nothing to add.
        assert!(concept.additional_text_edits.is_none());
    }

    #[test]
    fn a_bare_slot_offers_the_terms_of_its_kind_the_expressions_and_the_prefixes() {
        let text = onto("SubClassOf(:Pump |)");
        let labels = at(&text, &[]).labels();
        let has = |l: &str| labels.iter().any(|x| x == l);
        for want in [
            ":Pump",
            "owl:Thing",
            "ObjectSomeValuesFrom",
            "ObjectIntersectionOf",
            "skos:",
            ":",
        ] {
            assert!(has(want), "{want} not in {labels:?}");
        }
        // Not a class; not a class expression; annotations come first.
        for not in [
            ":feeds",
            ":power",
            ":note",
            ":p1",
            "owl:sameAs",
            "rdfs:label",
            "SubClassOf",
            "Annotation",
            "ObjectInverseOf",
        ] {
            assert!(!has(not), "{not} in {labels:?}");
        }
        // The class first.
        assert_eq!(labels[0], ":Pump", "{labels:?}");
    }

    #[test]
    fn each_slot_takes_the_entities_its_signature_names() {
        let cases: [(&str, &[&str], &[&str]); 7] = [
            (
                "ObjectPropertyAssertion(|",
                &[
                    ":feeds",
                    "owl:topObjectProperty",
                    "ObjectInverseOf",
                    "Annotation",
                ],
                &[":Pump", ":power", ":p1", ":note"],
            ),
            (
                "ObjectPropertyAssertion(:feeds |",
                &[":p1"],
                &[":feeds", ":Pump", "Annotation", "ObjectInverseOf"],
            ),
            (
                "DataPropertyAssertion(|",
                &[":power", "owl:topDataProperty"],
                &[":feeds", ":Pump", ":p1"],
            ),
            ("ClassAssertion(:Pump |", &[":p1"], &[":Pump", ":feeds"]),
            (
                "AnnotationAssertion(|",
                &[":note", "rdfs:label", "skos:prefLabel", "Annotation"],
                &[":Pump", ":feeds", ":power", ":p1", "owl:Thing"],
            ),
            (
                "DataPropertyRange(:power |",
                &[
                    "xsd:integer",
                    "rdfs:Literal",
                    "DataOneOf",
                    "DatatypeRestriction",
                ],
                &[":Pump", "xsd:minInclusive", "ObjectSomeValuesFrom"],
            ),
            (
                "Declaration(|",
                &["Class", "ObjectProperty", "NamedIndividual", "Annotation"],
                &[":Pump", "SubClassOf", "ObjectSomeValuesFrom"],
            ),
        ];
        for (axiom, want, not) in cases {
            let labels = at(&onto(axiom), &[]).labels();
            for w in want {
                assert!(
                    labels.iter().any(|l| l == w),
                    "{axiom}: {w} not in {labels:?}"
                );
            }
            for n in not {
                assert!(!labels.iter().any(|l| l == n), "{axiom}: {n} in {labels:?}");
            }
        }
        // A prefixed name is filtered the same way: a literal's datatype.
        let labels = at(
            &onto("DataPropertyAssertion(:power :p1 \"1\"^^xsd:int|)"),
            &[],
        )
        .labels();
        assert!(labels.iter().any(|l| l == "xsd:integer"), "{labels:?}");
        let labels = at(&onto("SubClassOf(:Pump :|)"), &[]).labels();
        assert_eq!(labels, [":Pump"]);
    }

    #[test]
    fn the_keywords_follow_the_grammar() {
        // Before the ontology, its header; inside it, its items.
        let labels = at("Prefix(:=<http://example.org/o#>)\n|", &[]).labels();
        assert_eq!(labels, ["Prefix", "Ontology"]);
        let text = onto("|");
        let labels = at(&text, &[]).labels();
        for want in [
            "Declaration",
            "SubClassOf",
            "EquivalentClasses",
            "Import",
            "Annotation",
            "DLSafeRule",
        ] {
            assert!(labels.iter().any(|l| l == want), "{want} not in {labels:?}");
        }
        for not in ["ObjectSomeValuesFrom", "Class", "Prefix", ":Pump", "skos:"] {
            assert!(!labels.iter().any(|l| l == not), "{not} in {labels:?}");
        }
        // After a declaration among the items (an error), still the items.
        let labels = at(&onto("Prefix(more:=<http://example.org/more/>)\n|"), &[]).labels();
        assert!(labels.iter().any(|l| l == "SubClassOf"), "{labels:?}");
        assert!(!labels.iter().any(|l| l == "Ontology"), "{labels:?}");
        // What is typed filters them, and is what the item replaces.
        let text = onto("SubCl|");
        let at = at(&text, &[]);
        assert_eq!(at.labels()[0], "SubClassOf");
        let start = text.find("SubCl").unwrap() as u32;
        assert_eq!(at.replaced("SubClassOf"), at.range(start, start + 5));
    }

    #[test]
    fn with_snippets_a_keyword_is_written_with_its_operands() {
        let at = at(&onto("SubClassOf(:Pump Obj|"), &[]);
        let item = |snippets: bool| {
            let items = at.list(snippets).items;
            items
                .into_iter()
                .find(|i| i.label == "ObjectSomeValuesFrom")
                .unwrap()
        };
        let text = |i: &CompletionItem| match &i.text_edit {
            Some(CompletionTextEdit::Edit(e)) => e.new_text.clone(),
            other => panic!("{other:?}"),
        };
        let plain = item(false);
        assert_eq!(text(&plain), "ObjectSomeValuesFrom");
        assert_eq!(plain.insert_text_format, None);
        assert_eq!(
            plain.detail.as_deref(),
            Some("ObjectSomeValuesFrom(objectProperty class)")
        );
        let snippet = item(true);
        assert_eq!(
            text(&snippet),
            "ObjectSomeValuesFrom(${1:objectProperty} ${2:class})"
        );
        assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));
        // An axiom: its annotations are left out.
        let at = at_doc(&onto("SubCl|"));
        let items = at.list(true).items;
        let sub = items.iter().find(|i| i.label == "SubClassOf").unwrap();
        assert_eq!(text(sub), "SubClassOf(${1:class} ${2:class})");
        // A `(` after the word, or after a space: the keyword alone.
        for written in ["SubCl|(:Pump owl:Thing)", "SubCl| (:Pump owl:Thing)"] {
            let at = at_doc(&onto(written));
            let items = at.list(true).items;
            let sub = items.iter().find(|i| i.label == "SubClassOf").unwrap();
            assert_eq!(text(sub), "SubClassOf", "{written}");
        }
    }

    /// The declaration an item adds, as (line, character, text).
    fn added(item: &CompletionItem) -> (u32, u32, String) {
        let edits = item.additional_text_edits.as_ref().expect("a declaration");
        assert_eq!(edits.len(), 1, "{edits:?}");
        let e = &edits[0];
        assert_eq!(e.range.start, e.range.end);
        (
            e.range.start.line,
            e.range.start.character,
            e.new_text.clone(),
        )
    }

    #[test]
    fn a_term_of_an_undeclared_prefix_adds_its_declaration_after_the_others() {
        let text = onto("SubClassOf(:Pump foaf:Ag|)");
        let at = at_doc(&text);
        let agent = at.item("foaf:Agent");
        let foaf = "Prefix(foaf:=<http://xmlns.com/foaf/0.1/>)\n";
        // At the start of the line after the last declaration.
        assert_eq!(added(&agent), (2, 0, foaf.into()));
        assert_eq!(
            agent.label_details.and_then(|d| d.description).as_deref(),
            Some("+Prefix")
        );
        // The prefix itself, while it is typed.
        let at = at_doc(&onto("SubClassOf(:Pump foa|)"));
        assert_eq!(added(&at.item("foaf:")).2, foaf);
        // No declaration: before `Ontology(`, after the comments.
        let text = "# An ontology.\nOntology(<http://example.org/o>\nSubClassOf(<http://example.org/A> foaf:Ag|)\n)\n";
        assert_eq!(added(&at_doc(text).item("foaf:Agent")), (1, 0, foaf.into()));
        // A comment after the last declaration stays on its line.
        let text = "Prefix(:=<http://example.org/o#>) # mine\r\nOntology(\r\nSubClassOf(:A foaf:Ag|)\r\n)\r\n";
        let crlf = foaf.replace('\n', "\r\n");
        assert_eq!(added(&at_doc(text).item("foaf:Agent")), (1, 0, crlf));
        // A declaration with something after it on its line: a line of its own.
        let text = "Prefix(:=<http://example.org/o#>) Ontology(\nSubClassOf(:A foaf:Ag|)\n)\n";
        let line = "\nPrefix(foaf:=<http://xmlns.com/foaf/0.1/>)".to_string();
        assert_eq!(added(&at_doc(text).item("foaf:Agent")), (0, 33, line));
        // The names of Table 2 are declared in every document.
        let text = "Ontology(\nAnnotationAssertion(rdfs:lab|\n)\n";
        assert!(
            at_doc(text)
                .item("rdfs:label")
                .additional_text_edits
                .is_none()
        );
        // A name declared with an IRI in error: no second declaration.
        let text =
            "Prefix(foaf:=<rel>)\nOntology(\nSubClassOf(<http://example.org/A> foaf:Ag|)\n)\n";
        assert!(
            at_doc(text)
                .item("foaf:Agent")
                .additional_text_edits
                .is_none()
        );
    }

    /// Two inputs of the chaos tests, reduced: a broken `Prefix(` whose
    /// recovery reads the rest as items, and one whose parentheses close at
    /// the end of the text. The declaration goes where the grammar reads
    /// declarations (here before the first token): applied, it declares the
    /// prefix and adds no error.
    #[test]
    fn a_declaration_goes_where_the_grammar_reads_declarations() {
        for text in [
            "Prefix(:x(<http://example.org/o>\nDeclaration(Class(:A))\n)=<http://example.org/>)\nOntology(<http://example.org/o>\nSubClassOf(:A foaf:Ag|)\n)\n",
            "Prefix( : = <@enhttp://example.org/>  (()\n\nOntology(\n  ClassAssertion( foaf:Ag| :Stewie )\n)))",
            "Prefix(ex:=<http://example.org/>)\nOntology(\nPrefix(more:=<http://example.org/more/>)\nSubClassOf(ex:A foaf:Ag|)\n)\n",
        ] {
            let at = at_doc(text);
            let item = at.item("foaf:Agent");
            let e = &item.additional_text_edits.as_ref().expect("a declaration")[0];
            let offset = at.a.offset(Encoding::Utf16, e.range.start).unwrap() as usize;
            let plain = text.replacen('|', "", 1);
            let after = format!("{}{}{}", &plain[..offset], e.new_text, &plain[offset..]);
            let re = Analysis::new(URI.into(), &after, Arc::default(), true);
            assert!(
                re.parsed.prefixes.iter().any(|p| p.name == "foaf"),
                "{after}"
            );
            let mut before: Vec<&str> = at.a.parsed.errors.iter().map(|e| e.message()).collect();
            for err in &re.parsed.errors {
                let i = before.iter().position(|m| *m == err.message());
                let i = i.unwrap_or_else(|| panic!("a new error {:?} in {after:?}", err.message()));
                before.swap_remove(i);
            }
        }
    }

    #[test]
    fn a_document_half_typed_is_read_from_its_tokens() {
        // Not closed, and the next axiom already written below.
        let cases = [
            (
                "SubClassOf(:Pump ObjectSomeValuesFrom(:feeds |\nDeclaration(Class(:B))",
                ":Pump",
                ":feeds",
            ),
            ("SubClassOf(:Pump ObjectSomeValuesFrom(|", ":feeds", ":Pump"),
            // An axiom broken off before: the next one is read alone.
            ("SubClassOf(:Pump \nClassAssertion(:Pump |", ":p1", ":Pump"),
            (
                "EquivalentClasses(:Pump ObjectIntersectionOf(:Pump ObjectHasValue(:feeds |",
                ":p1",
                ":feeds",
            ),
            // An annotated axiom: the annotation is passed.
            ("SubClassOf(Annotation(:note \"n\"@en) |", ":Pump", ":p1"),
            ("HasKey(:Pump (|", ":feeds", ":power"),
            ("HasKey(:Pump () (|", ":power", ":feeds"),
            ("DataSomeValuesFrom(:power |", ":power", ":Pump"),
            ("ObjectMinCardinality(1 |", ":feeds", ":Pump"),
        ];
        for (axiom, want, not) in cases {
            let labels = at_doc(&onto(axiom)).labels();
            assert!(
                labels.iter().any(|l| l == want),
                "{axiom}: {want} not in {labels:?}"
            );
            assert!(
                !labels.iter().any(|l| l == not),
                "{axiom}: {not} in {labels:?}"
            );
        }
        let labels = at_doc(&onto("DataSomeValuesFrom(:power |")).labels();
        assert!(labels.iter().any(|l| l == "xsd:string"), "{labels:?}");
        // Nothing in a string, a comment or a full IRI, or after a keyword
        // that waits for its `(`.
        for nothing in [
            "AnnotationAssertion(:note :Pump \"a pu|",
            "# Class: :Pu|",
            // The first of two comment lines.
            "# Class: :Pu|\n# more",
            "SubClassOf(:Pump <http://example.org/o#Pu|",
            "SubClassOf(:Pump ObjectSomeValuesFrom |",
            "AnnotationAssertion(:note :Pump \"x\"@e|",
        ] {
            let labels = at_doc(&onto(nothing)).labels();
            assert!(labels.is_empty(), "{nothing}: {labels:?}");
        }
    }

    #[test]
    fn the_terms_of_other_workspace_files_are_offered() {
        let turtle = "@prefix : <http://example.org/o#> .\n\
                      @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                      @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                      :Valve a owl:Class ; rdfs:label \"A valve\" .\n\
                      :drives a owl:ObjectProperty .\n\
                      :t1 a owl:Thing .\n";
        let other = "Prefix(:=<http://example.org/o#>)\nOntology(\nDeclaration(Class(:Tank))\n)\n";
        let others = [
            ("file:///w/parts.ttl", turtle),
            ("file:///w/more.ofn", other),
        ];
        let at = at(&onto("SubClassOf(:Pump :|)"), &others);
        let labels = at.labels();
        for want in [":Valve", ":Tank", ":Pump"] {
            assert!(labels.iter().any(|l| l == want), "{want} not in {labels:?}");
        }
        assert!(!labels.iter().any(|l| l == ":drives"), "{labels:?}");
        // With the label the workspace gives.
        assert_eq!(at.item(":Valve").detail.as_deref(), Some("A valve"));
        // An instance of owl:Thing is an individual.
        let labels = super::tests::at(&onto("ClassAssertion(:Pump |)"), &others).labels();
        assert!(labels.iter().any(|l| l == ":t1"), "{labels:?}");
        assert!(!labels.iter().any(|l| l == ":Valve"), "{labels:?}");
    }

    /// A Functional document of about `bytes` bytes in the shape OWL API
    /// writes (the `large` of `src/ofn/tests.rs`, with neutral names):
    /// classes with labels, definitions, subclass axioms and, for one in
    /// four, an equivalence with a class expression.
    fn large(bytes: usize) -> String {
        let mut out = String::from(
            "Prefix(:=<http://example.org/o#>)\nPrefix(ex:=<http://example.org/>)\nPrefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)\n\nOntology(<http://example.org/o>\nDeclaration(ObjectProperty(ex:partOf))\n\n",
        );
        let mut i = 0u32;
        while out.len() < bytes {
            let c = format!("ex:C_{i:07}");
            let p = format!("ex:C_{:07}", i / 2);
            out.push_str(&format!(
                "# Class: {c} (number {i})\n\n\
                 AnnotationAssertion(Annotation(rdfs:seeAlso \"ref {i}\") rdfs:comment {c} \"A class, number {i}, with a \\\"quoted\\\" word.\")\n\
                 AnnotationAssertion(rdfs:label {c} \"Class number {i}\"@en)\n\
                 SubClassOf({c} {p})\n"
            ));
            if i.is_multiple_of(4) {
                out.push_str(&format!(
                    "EquivalentClasses({c} ObjectIntersectionOf({p} ObjectSomeValuesFrom(ex:partOf {p})))\n"
                ));
            }
            out.push_str(&format!("Declaration(Class({c}))\n\n"));
            i += 1;
        }
        out.push_str(")\n");
        out
    }

    /// A slot with more terms of its kind than the list holds: the keywords
    /// and the declared prefixes keep their places, the terms fill the
    /// rest, and the list says it is incomplete.
    #[test]
    fn in_a_large_ontology_the_keywords_and_prefixes_keep_their_places() {
        let base = large(200_000);
        let at = base.find("\nSubClassOf(").unwrap() + 1;
        let slot = at + base[at..].find(' ').unwrap() + 1;
        let text = format!("{}|{}", &base[..slot], &base[slot..]);
        let at = at_doc(&text);
        let list = at.list(false);
        let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels.len(), super::LIMIT);
        assert!(list.is_incomplete);
        for want in [
            "ObjectSomeValuesFrom",
            "ObjectIntersectionOf",
            "ex:",
            ":",
            "ex:C_0000001",
        ] {
            assert!(labels.contains(&want), "{want} not in {labels:?}");
        }
        // The classes first, the shortest first.
        assert_eq!(labels[..3], ["owl:Thing", "owl:Nothing", "ex:C_0000000"]);
    }

    /// Completion in the middle of a 1 MB Functional document, at the start
    /// of an axiom, in a class slot with nothing typed and after `ex:C_00`,
    /// against the analysis each keystroke in it costs (the parse and the
    /// mapping in full: a Functional document is not spliced). The context
    /// is read from the last axiom mapped whole, not from the start; the
    /// terms are the document's 3,134 classes, cut to the 200 shown. On an
    /// M-series laptop at load 7 (other builds running): the analysis
    /// 7.4 ms; completion 15 µs at the axiom, 0.51 ms in the class slot and
    /// 0.60 ms after `ex:C_00`, a twelfth of it. The checks are ratios, as
    /// in `src/workspace/tests.rs`, with three times that room; the
    /// wall-clock bound only catches a completion gone badly wrong.
    #[cfg(not(debug_assertions))]
    #[test]
    fn timing_completion_in_a_large_functional_document_is_a_fraction_of_its_analysis() {
        let base = large(1 << 20);
        let mid = base.len() / 2;
        let line = mid + base[mid..].find("\nSubClassOf(").unwrap() + 1;
        let slot = line + base[line..].find(' ').unwrap() + 1;
        let cases = [(line, ""), (slot, ""), (slot, "ex:C_00")];
        let median = |v: &mut Vec<std::time::Duration>| {
            v.sort();
            v[v.len() / 2]
        };
        let mut analyses = Vec::new();
        let mut completions = Vec::new();
        for (at, typed) in cases {
            let text = format!("{}{typed}{}", &base[..at], &base[at..]);
            let t = std::time::Instant::now();
            let a = Arc::new(Analysis::new(URI.into(), &text, Arc::default(), true));
            analyses.push(t.elapsed());
            assert!(a.parsed.errors.is_empty() || !typed.is_empty());
            let mut k = Knowledge::default();
            k.set(URI, Source::Open, a.clone());
            let pos = a.position(Encoding::Utf16, (at + typed.len()) as u32);
            let mut times = Vec::new();
            let mut list = None;
            for _ in 0..7 {
                let t = std::time::Instant::now();
                let got = super::super::complete_in(&a, &k, Encoding::Utf16, pos, true);
                times.push(t.elapsed());
                list = Some(got);
            }
            let Some(CompletionResponse::List(list)) = list else {
                panic!("a list")
            };
            let labels: Vec<_> = list.items.iter().map(|i| i.label.as_str()).collect();
            let want = if at == line {
                "SubClassOf"
            } else {
                "ex:C_0000001"
            };
            assert!(labels.contains(&want), "{typed:?}: {labels:?}");
            assert!(list.items.len() <= super::LIMIT);
            completions.push(median(&mut times));
        }
        let analysis = median(&mut analyses);
        let msg = format!(
            "{} bytes: analysis {analysis:?}; completions {completions:?}",
            base.len()
        );
        eprintln!("{msg}");
        for c in &completions {
            assert!(*c * 4 < analysis, "{msg}");
            assert!(c.as_millis() < 200, "{msg}");
        }
    }
}
