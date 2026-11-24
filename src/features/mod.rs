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

use crate::syntax::Syntax;

/// Whether the features that write text (quick fixes, refactorings,
/// rename) may edit a document of `syntax`. RDF/XML is read-only: nothing
/// writes XML, whatever form its references have. OWL Functional syntax and
/// OWL/XML are read-only too: turtle-lsp reads them and writes none of them.
pub(crate) fn writes(syntax: Syntax) -> bool {
    match syntax {
        Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads | Syntax::JsonLd => true,
        Syntax::RdfXml | Syntax::OwlFunctional | Syntax::OwlXml => false,
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
