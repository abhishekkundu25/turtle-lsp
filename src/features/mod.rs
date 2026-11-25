pub mod completion;
pub(crate) mod curation;
pub mod diagnostics;
pub(crate) mod embedded;
pub mod format;
pub mod hierarchy;
pub mod hover;
pub mod inlay;
pub(crate) mod kept;
pub(crate) mod lens;
pub(crate) mod lint;
pub(crate) mod literals;
pub mod navigation;
pub(crate) mod prefixes;
pub mod quickfix;
pub mod refactor;
pub mod semantic;
pub(crate) mod shacl_lint;
pub mod shacl_validate;
pub mod symbols;

use crate::syntax::{Family, Syntax};

/// Whether the features that write text (formatting, quick fixes,
/// refactorings, `check --fix`, the Turtle-family rename) may edit a
/// document of `syntax`. RDF/XML is read-only for all of them, whatever form
/// its references have; rename is its one writer, and not through this gate
/// ([`renames`]). OWL Functional syntax and OWL/XML are read-only for all of
/// them and for rename too: turtle-lsp reads them and writes none of them.
pub(crate) fn writes(syntax: Syntax) -> bool {
    match syntax.family() {
        Family::Turtle | Family::JsonLd => true,
        Family::ReadOnly => false,
    }
}

/// Whether completion offers anything in a document of `syntax`. Everything
/// it offers is a keyword or a prefixed name: N-Triples and N-Quads allow
/// neither. RDF/XML and OWL/XML get none. OWL Functional syntax gets its
/// keywords and terms, and completion is the one feature that edits it: an
/// accepted term may add its `Prefix( … )` declaration.
pub(crate) fn completes(syntax: Syntax) -> bool {
    match syntax {
        Syntax::Turtle | Syntax::TriG | Syntax::JsonLd | Syntax::OwlFunctional => true,
        Syntax::NTriples | Syntax::NQuads | Syntax::RdfXml | Syntax::OwlXml => false,
    }
}

/// Whether a rename may edit a document of `syntax`, or start in one: every
/// syntax that [`writes`], and RDF/XML too, which [`Family`] groups with the
/// read-only syntaxes. `navigation::rename_term` edits an RDF/XML document
/// only through `rdfxml::rename::plan`, which places each edit at the spans
/// the RDF/XML grammar reads and refuses the rename when it cannot
/// (`rename.rdfXml` turns it off). OWL Functional syntax and OWL/XML are
/// not renamed in: a rename that would edit one is refused.
pub(crate) fn renames(syntax: Syntax) -> bool {
    match syntax {
        Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads | Syntax::JsonLd => true,
        Syntax::RdfXml => true,
        Syntax::OwlFunctional | Syntax::OwlXml => false,
    }
}

/// Analyses of RDF/XML documents for the tests of the gates.
#[cfg(test)]
pub(crate) mod rdfxml_fixture {
    use crate::analysis::{Analysis, LineIndex};
    use crate::syntax::{self, PrefixMap, Syntax};
    use std::sync::Arc;

    /// A small RDF/XML document of `http://example.org/` terms: a class
    /// `ex:A` with a label that ends in a space, `ex:a` of type `ex:A` with
    /// a link `ex:p` to `ex:b`, a misspelt `rdfs:labl` and an `ex:n` that is
    /// no integer. Every lint would find something in it.
    pub(crate) const TEXT: &str = "<?xml version=\"1.0\"?>\n\
        <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
        \x20        xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n\
        \x20        xmlns:owl=\"http://www.w3.org/2002/07/owl#\"\n\
        \x20        xmlns:ex=\"http://example.org/\">\n\
        \x20 <owl:Class rdf:about=\"http://example.org/A\">\n\
        \x20   <rdfs:label>A thing </rdfs:label>\n\
        \x20 </owl:Class>\n\
        \x20 <ex:A rdf:about=\"http://example.org/a\">\n\
        \x20   <ex:p rdf:resource=\"http://example.org/b\"/>\n\
        \x20   <rdfs:labl>a</rdfs:labl>\n\
        \x20   <ex:n rdf:datatype=\"http://www.w3.org/2001/XMLSchema#integer\">x</ex:n>\n\
        \x20 </ex:A>\n\
        </rdf:RDF>\n";

    /// [`TEXT`] read by the RDF/XML frontend: references at the exact
    /// text of their terms (attribute values and qualified names).
    pub(crate) fn read(uri: &str) -> Analysis {
        let a = Analysis::with_syntax(uri.into(), TEXT, Arc::default(), true, Syntax::RdfXml);
        assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
        assert!(!a.parsed.triples.is_empty());
        a
    }

    /// What [`TEXT`] says, in Turtle, with an unused prefix too.
    pub(crate) const TURTLE: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
        @prefix unused: <http://example.org/unused/> .\n\
        ex:A a owl:Class ; rdfs:label \"A thing \" .\n\
        ex:a a ex:A ; ex:p <http://example.org/b> ; rdfs:labl \"a\" ; ex:n \"x\"^^xsd:integer .\n";

    /// `text` read as Turtle (with the project prefixes `external`), then
    /// called RDF/XML: references written as IRIs and prefixed names,
    /// prefix declarations and uses, in a document of syntax
    /// [`Syntax::RdfXml`]: Turtle's text and forms, so a gate is shown to
    /// hold by syntax, not by what the text at a reference is.
    pub(crate) fn written(uri: &str, text: &str) -> Analysis {
        written_with(uri, text, Arc::default())
    }

    /// [`written`], with the project prefixes `external`.
    pub(crate) fn written_with(uri: &str, text: &str, external: Arc<PrefixMap>) -> Analysis {
        let mut parsed = syntax::parse_with_base(text, &external, Syntax::Turtle, Some(uri));
        parsed.syntax = Syntax::RdfXml;
        parsed.tokens = Syntax::RdfXml.tokens(text);
        let lines = LineIndex::new(text);
        Analysis::with_parse(uri.into(), text, external, true, parsed, lines)
    }

    /// [`TURTLE`] as Turtle.
    pub(crate) fn turtle(uri: &str) -> Analysis {
        Analysis::with_syntax(uri.into(), TURTLE, Arc::default(), true, Syntax::Turtle)
    }
}

/// The answer of every per-syntax predicate, for each syntax, in one table:
/// a regrouping of the syntaxes cannot move one answer unnoticed.
#[cfg(test)]
mod syntax_table {
    use crate::analysis::Analysis;
    use crate::features::diagnostics::Check;
    use crate::syntax::splice::{Change, Edit, applies};
    use crate::syntax::{PrefixMap, Syntax};
    use std::sync::Arc;

    /// Every syntax, in the order of the declaration.
    const ALL: [Syntax; 8] = [
        Syntax::Turtle,
        Syntax::TriG,
        Syntax::NTriples,
        Syntax::NQuads,
        Syntax::JsonLd,
        Syntax::RdfXml,
        Syntax::OwlFunctional,
        Syntax::OwlXml,
    ];

    /// The place of `syntax` in [`ALL`] and its label in the table. The
    /// match has no wildcard: a new variant does not compile until it is
    /// listed here, beside [`ALL`], where it goes too.
    /// [`all_is_every_syntax`] checks that [`ALL`] is in this order.
    fn place(syntax: Syntax) -> (usize, &'static str) {
        match syntax {
            Syntax::Turtle => (0, "turtle"),
            Syntax::TriG => (1, "trig"),
            Syntax::NTriples => (2, "ntriples"),
            Syntax::NQuads => (3, "nquads"),
            Syntax::JsonLd => (4, "jsonld"),
            Syntax::RdfXml => (5, "rdfxml"),
            Syntax::OwlFunctional => (6, "ofn"),
            Syntax::OwlXml => (7, "owx"),
        }
    }

    /// A text of `syntax` with one statement and no errors.
    fn text(syntax: Syntax) -> &'static str {
        match syntax {
            Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads => {
                "<http://example.org/a> <http://example.org/p> <http://example.org/b> .\n"
            }
            Syntax::JsonLd => {
                "{\"@id\": \"http://example.org/a\", \"http://example.org/p\": {\"@id\": \"http://example.org/b\"}}\n"
            }
            Syntax::RdfXml => super::rdfxml_fixture::TEXT,
            Syntax::OwlFunctional => {
                "Prefix(:=<http://example.org/>)\nOntology(<http://example.org/o>\nDeclaration(Class(:A))\n)\n"
            }
            Syntax::OwlXml => {
                "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Declaration><Class IRI=\"http://example.org/A\"/></Declaration>\n</Ontology>\n"
            }
        }
    }

    fn yes(b: bool) -> &'static str {
        if b { "yes" } else { "no" }
    }

    /// A row of the table: the first of `cells` in 20 columns, the others in
    /// 10 each, with no trailing space.
    fn line(cells: &[&str]) -> String {
        let (first, rest) = cells.split_first().expect("a row has cells");
        let mut row = format!("{first:<20}");
        for c in rest {
            row += &format!("{c:<10}");
        }
        format!("{}\n", row.trim_end())
    }

    /// Whether [`applies`] lets a splice go on in a parse of `syntax`
    /// (`ok`), or the fallback it gives. The edit is empty and fits, the
    /// prefixes and the base are the parse's own, and the text has a
    /// statement: the syntax is the only reason it can refuse.
    fn splice(syntax: Syntax) -> String {
        let text = text(syntax);
        let a = Analysis::with_syntax(
            "http://example.org/doc".into(),
            text,
            Arc::default(),
            true,
            syntax,
        );
        assert_eq!(a.parsed.syntax, syntax);
        assert!(
            a.parsed.errors.is_empty(),
            "{syntax:?}: {:?}",
            a.parsed.errors
        );
        let change = Change {
            old_text: text,
            new_text: text,
            edit: Edit::new(0..0, 0),
        };
        let none = PrefixMap::default();
        let base = a.parsed.document_base.as_deref();
        match applies(&a.parsed, change, &none, &none, base) {
            Ok(()) => "ok".into(),
            Err(fallback) => format!("{fallback:?}"),
        }
    }

    #[test]
    fn all_is_every_syntax() {
        for (i, s) in ALL.into_iter().enumerate() {
            assert_eq!(place(s).0, i, "{s:?}");
        }
    }

    #[test]
    fn every_syntax_answers_as_today() {
        let mut got = line(&[
            "syntax",
            "prefixes",
            "line",
            "keyword",
            "writes",
            "completes",
            "renames",
            "follows",
            "hinted",
            "tokens",
            "splice",
        ]);
        for s in ALL {
            let splice = splice(s);
            got += &line(&[
                place(s).1,
                yes(s.has_prefixes()),
                yes(s.is_line_based()),
                yes(s.has_keyword_header()),
                yes(super::writes(s)),
                yes(super::completes(s)),
                yes(super::renames(s)),
                yes(super::kept::follows(s)),
                yes(super::inlay::hinted(s)),
                yes(!s.tokens(text(s)).is_empty()),
                &splice,
            ]);
        }
        // `Check::runs_on`: one row per check, one column per syntax.
        let mut header = vec!["runs_on"];
        header.extend(ALL.map(|s| place(s).1));
        got += &line(&header);
        for check in Check::ALL {
            let name = format!("{check:?}");
            let mut row = vec![name.as_str()];
            row.extend(ALL.map(|s| yes(check.runs_on(s))));
            got += &line(&row);
        }
        let want = "\
            syntax              prefixes  line      keyword   writes    completes renames   follows   hinted    tokens    splice\n\
            turtle              yes       no        no        yes       yes       yes       yes       yes       yes       ok\n\
            trig                yes       no        no        yes       yes       yes       yes       yes       yes       ok\n\
            ntriples            no        yes       no        yes       no        yes       yes       yes       yes       ok\n\
            nquads              no        yes       no        yes       no        yes       yes       yes       yes       ok\n\
            jsonld              no        no        no        yes       yes       yes       no        yes       no        Syntax\n\
            rdfxml              no        no        no        no        no        yes       no        no        no        Syntax\n\
            ofn                 no        no        yes       no        yes       no        no        no        no        Syntax\n\
            owx                 no        no        yes       no        no        no        no        no        no        Syntax\n\
            runs_on             turtle    trig      ntriples  nquads    jsonld    rdfxml    ofn       owx\n\
            Syntax              yes       yes       yes       yes       yes       yes       yes       yes\n\
            UndeclaredPrefix    yes       yes       yes       yes       yes       no        no        no\n\
            PrefixDeclarations  yes       yes       yes       yes       yes       no        no        no\n\
            UnusedPrefix        yes       yes       yes       yes       yes       no        no        no\n\
            JsonLd              yes       yes       yes       yes       yes       no        no        no\n\
            TermUses            yes       yes       yes       yes       yes       yes       yes       yes\n\
            Literals            yes       yes       yes       yes       yes       yes       yes       yes\n\
            ShaclLint           yes       yes       yes       yes       yes       no        no        no\n\
            UndescribedTerms    yes       yes       yes       yes       yes       yes       yes       yes\n\
            LabelWhitespace     yes       yes       yes       yes       yes       no        no        no\n\
            DuplicateLabel      yes       yes       yes       yes       yes       no        no        no\n\
            MultipleLabels      yes       yes       yes       yes       yes       no        no        no\n\
            HouseStyle          yes       yes       yes       yes       yes       no        no        no\n\
            ";
        assert_eq!(got, want, "\n{got}");
    }
}
