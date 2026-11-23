//! The mapping of a Functional document to RDF: the operator T of OWL 2
//! Web Ontology Language Mapping to RDF Graphs (Second Edition, W3C
//! Recommendation 11 December 2012, section 2), over the [`Document`] the
//! grammar builds, into the shared [`Parsed`].
//!
//! Each row of Table 1 is one arm of [`Mapper::term`] (class expressions,
//! property expressions, data ranges, literals) or [`Mapper::axiom`] (the
//! axioms, the ontology header in [`Mapper::header`]); Table 2 (TANN) is
//! [`Mapper::annotation`]; section 2.3, the axioms with annotations, is
//! [`Mapper::main`] (2.3.1, a main triple with an `owl:Axiom` node; the
//! side triples of `DisjointUnion`, a property chain and `HasKey` as they
//! are), the pairs of 2.3.2 (each pair reified, the annotations repeated
//! per pair) and the blank node of 2.3.3 (the annotations on it). Every
//! generated blank node is fresh per application of a rule; an anonymous
//! individual `_:x` is one blank node for the whole document. Blank nodes
//! are numbered in the order they are made, so two parses of one text are
//! equal. A sequence becomes an RDF list (`T(SEQ …)`).
//!
//! Where the specification leaves a choice:
//! - in `EquivalentClasses( CE1 … CEn )` and the other pairwise axioms of
//!   2.3.2, each operand is translated once and its main node stands in
//!   every triple that names it (so `T(CE2)` of the first pair and of the
//!   second are one node), as OWL API writes it. The table writes `T(CE2)`
//!   in two triples, and section 2.1 makes each generated blank node fresh
//!   "in each application of a mapping rule": read as one application of
//!   T to the operand. No W3C test decides it (none has a pairwise axiom of
//!   three operands with a class expression in the middle);
//! - a cardinality is the integer as written, `"n"^^xsd:nonNegativeInteger`;
//! - a literal `"abc@en"^^rdf:PlainLiteral` is `"abc"@en` and
//!   `"abc@"^^rdf:PlainLiteral` is `"abc"` (Table 1), the tag split at the
//!   last `@`; a language tag is lowercased, as [`Node::Literal`] keeps it.
//!   Any other `rdf:PlainLiteral` (no `@`, or a tag that is not a BCP 47
//!   language tag) is an error at its quoted string, and its item gives
//!   nothing: Table 1 has no row for it, and the datatype's definition
//!   leaves it out of the lexical space ("An rdf:PlainLiteral lexical form
//!   is a string of the form "abc@langTag" where "abc" is an arbitrary
//!   (possibly empty) string, and "langTag" is either the empty string or
//!   a (not necessarily lowercase) language tag", rdf:PlainLiteral, W3C
//!   Recommendation 11 December 2012, section 3), where Structural
//!   Specification section 5.7 needs it ("The lexical form of each literal
//!   occurring in an OWL 2 DL ontology MUST belong to the lexical space of
//!   the literal's datatype"). Mapped, `"Bonn"` would become a valid
//!   `xsd:string` and `"Bonn@ x"` a literal with the tag ` x`;
//!
//! Departures (constructs the W3C mapping does not cover):
//! - SWRL rules (`DLSafeRule`), which the grammar takes as OWL API writes
//!   them, map as OWL API 4.5.29 (ROBOT 1.9.11) writes them in RDF, to the
//!   SWRL vocabulary (`http://www.w3.org/2003/11/swrl#`): a blank node of
//!   type `swrl:Imp` with `swrl:body` and `swrl:head` lists whose nodes are
//!   also of type `swrl:AtomList` (`rdf:nil` when empty); each atom a blank
//!   node (`swrl:ClassAtom` with `swrl:classPredicate`,
//!   `swrl:DataRangeAtom` with `swrl:dataRange`,
//!   `swrl:IndividualPropertyAtom` and `swrl:DatavaluedPropertyAtom` with
//!   `swrl:propertyPredicate`, `swrl:BuiltinAtom` with `swrl:builtin` and a
//!   `swrl:arguments` list whose nodes are also `rdf:List`,
//!   `swrl:SameIndividualAtom`, `swrl:DifferentIndividualsAtom`) with
//!   `swrl:argument1` and `swrl:argument2`; a variable is its IRI, of type
//!   `swrl:Variable` (once per document); the rule's annotations are on the
//!   `swrl:Imp` node, as for the axioms of 2.3.3.
//!
//! A document with errors maps what it can: every item that was read whole
//! is mapped, and one with an error inside ([`Document::broken`]) gives no
//! triple, no reference and no statement: never a part of an axiom. Only
//! the items inside `Ontology( … )` are mapped (none of a text with no
//! ontology, nor those after its `)`); an `Ontology` whose `)` is missing,
//! or written without its `(`, has its items mapped all the same, as the
//! ontology being typed. An `Import` or an ontology annotation in the wrong
//! place (after the axioms) is an item with an error. When the ontology IRI
//! or the version IRI has an error, the header is not mapped, nor the
//! imports and the ontology annotations, which hang on it.
//!
//! Limits, so the triples stay in proportion to the text: the IRIs one
//! parse makes total at most [`IRI_BYTES_PER_BYTE`] bytes per byte of text
//! (a long prefix with many short local names), the triples number at
//! most [`TRIPLES_PER_BYTE`] per byte, and their literals total at most
//! [`LITERAL_BYTES_PER_BYTE`] bytes per byte (the pairs of 2.3.2 repeat the
//! annotations, so an annotated `SameIndividual` grows with the square of
//! its text, in triples and in the bytes of each triple's own copy of an
//! annotation's literal), each plus a floor. Past a limit, one error says
//! so, and the item it is met in and every item after it give nothing.
//!
//! What the parse holds:
//! - a [`Triple`] per triple: `p_span` at the IRI that gives the predicate
//!   when the text writes it (an annotation property, the property of an
//!   assertion, a facet), else at the keyword of the construct whose rule
//!   makes it; `o_span` at the construct or term the object comes from;
//!   the triples of a list at its item;
//! - a [`Statement`] per mapped item (an import, an ontology annotation,
//!   an axiom), from its keyword to its `)`, and one for the header, from
//!   `Ontology` to the ontology IRI or version IRI written (its keyword
//!   when it has no IRI). A statement's subject is set where the item
//!   defines its IRI (see below);
//! - a [`Ref`] per IRI written in a mapped item, at its exact text:
//!   [`Form::Iri`] for `<…>`, [`Form::PName`] for `pfx:local`. The role:
//!   [`Role::Predicate`] where the IRI is a triple's predicate,
//!   [`Role::Datatype`] for a literal's datatype, [`Role::Subject`] where
//!   it is defined, [`Role::About`] where an item's main triple has it as
//!   subject elsewhere (its label, its superclass: not a use, as a subject
//!   in Turtle or RDF/XML is not), else [`Role::Object`]. An IRI is defined
//!   where the document declares it (each `Declaration` names its entity
//!   as subject); one the document never declares, at the first item whose
//!   main triple has it as subject (`SubClassOf( :A … )`,
//!   `AnnotationAssertion( … :A … )`, `ClassAssertion( … :a )`); the
//!   ontology IRI, at the header. Definitions are where go-to-definition
//!   leads and what makes a term described in the workspace;
//! - the `Prefix( … )` declarations, as
//!   [`PrefixDecl`](crate::syntax::PrefixDecl)s: a Functional
//!   document's prefixes hold for the whole document, as Turtle's do;
//! - no tokens, comments or prefix uses (nothing edits the document, and
//!   an undeclared prefix is an `ofn-syntax` error).

use super::tree::{Document, Keyword as K, NodeKind};
use super::{CODE, MAX_ERRORS};
use crate::syntax::{
    BlankId, Form, Iri, Node, Parsed, PrefixDecl, RDF, Ref, Role, Severity, Span, Statement,
    Syntax, SyntaxError, Triple, XSD,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::sync::OnceLock;

/// The IRIs a parse makes total at most this many bytes per byte of text,
/// plus [`MIN_IRI_BYTES`]: the interner keeps every IRI for good.
pub(crate) const IRI_BYTES_PER_BYTE: usize = 16;
pub(crate) const MIN_IRI_BYTES: usize = 1 << 20;

/// The triples of a parse number at most this many per byte of text, plus
/// [`MIN_TRIPLES`].
pub(crate) const TRIPLES_PER_BYTE: usize = 2;
const MIN_TRIPLES: usize = 1 << 20;

/// The literals of the triples of a parse total at most this many bytes
/// per byte of text, plus [`MIN_LITERAL_BYTES`]: each triple owns its
/// literal, and the pairs of section 2.3.2 repeat the annotations.
pub(crate) const LITERAL_BYTES_PER_BYTE: usize = 16;
pub(super) const MIN_LITERAL_BYTES: usize = 1 << 24;

const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
const SWRL: &str = "http://www.w3.org/2003/11/swrl#";

macro_rules! vocab {
    ($($field:ident = $ns:ident $local:literal),* $(,)?) => {
        /// The IRIs the mapping writes, interned once.
        struct Vocab {
            $($field: Iri),*
        }

        fn vocab() -> &'static Vocab {
            static V: OnceLock<Vocab> = OnceLock::new();
            V.get_or_init(|| Vocab {
                $($field: Iri::intern(&format!("{}{}", $ns, $local))),*
            })
        }
    };
}

vocab!(
    rdf_type = RDF "type",
    first = RDF "first",
    rest = RDF "rest",
    nil = RDF "nil",
    list = RDF "List",
    plain_literal = RDF "PlainLiteral",
    lang_string = RDF "langString",
    datatype = RDFS "Datatype",
    sub_class_of = RDFS "subClassOf",
    sub_property_of = RDFS "subPropertyOf",
    domain = RDFS "domain",
    range = RDFS "range",
    string = XSD "string",
    non_negative_integer = XSD "nonNegativeInteger",
    boolean = XSD "boolean",
    ontology = OWL "Ontology",
    version_iri = OWL "versionIRI",
    imports = OWL "imports",
    class = OWL "Class",
    object_property = OWL "ObjectProperty",
    datatype_property = OWL "DatatypeProperty",
    annotation_property = OWL "AnnotationProperty",
    named_individual = OWL "NamedIndividual",
    inverse_of = OWL "inverseOf",
    intersection_of = OWL "intersectionOf",
    union_of = OWL "unionOf",
    datatype_complement_of = OWL "datatypeComplementOf",
    one_of = OWL "oneOf",
    on_datatype = OWL "onDatatype",
    with_restrictions = OWL "withRestrictions",
    complement_of = OWL "complementOf",
    restriction = OWL "Restriction",
    on_property = OWL "onProperty",
    on_properties = OWL "onProperties",
    some_values_from = OWL "someValuesFrom",
    all_values_from = OWL "allValuesFrom",
    has_value = OWL "hasValue",
    has_self = OWL "hasSelf",
    min_cardinality = OWL "minCardinality",
    max_cardinality = OWL "maxCardinality",
    cardinality = OWL "cardinality",
    min_qualified_cardinality = OWL "minQualifiedCardinality",
    max_qualified_cardinality = OWL "maxQualifiedCardinality",
    qualified_cardinality = OWL "qualifiedCardinality",
    on_class = OWL "onClass",
    on_data_range = OWL "onDataRange",
    equivalent_class = OWL "equivalentClass",
    disjoint_with = OWL "disjointWith",
    all_disjoint_classes = OWL "AllDisjointClasses",
    members = OWL "members",
    disjoint_union_of = OWL "disjointUnionOf",
    property_chain_axiom = OWL "propertyChainAxiom",
    equivalent_property = OWL "equivalentProperty",
    property_disjoint_with = OWL "propertyDisjointWith",
    all_disjoint_properties = OWL "AllDisjointProperties",
    functional_property = OWL "FunctionalProperty",
    inverse_functional_property = OWL "InverseFunctionalProperty",
    reflexive_property = OWL "ReflexiveProperty",
    irreflexive_property = OWL "IrreflexiveProperty",
    symmetric_property = OWL "SymmetricProperty",
    asymmetric_property = OWL "AsymmetricProperty",
    transitive_property = OWL "TransitiveProperty",
    has_key = OWL "hasKey",
    same_as = OWL "sameAs",
    different_from = OWL "differentFrom",
    all_different = OWL "AllDifferent",
    negative_property_assertion = OWL "NegativePropertyAssertion",
    source_individual = OWL "sourceIndividual",
    assertion_property = OWL "assertionProperty",
    target_individual = OWL "targetIndividual",
    target_value = OWL "targetValue",
    axiom = OWL "Axiom",
    annotation = OWL "Annotation",
    annotated_source = OWL "annotatedSource",
    annotated_property = OWL "annotatedProperty",
    annotated_target = OWL "annotatedTarget",
    imp = SWRL "Imp",
    body = SWRL "body",
    head = SWRL "head",
    atom_list = SWRL "AtomList",
    class_atom = SWRL "ClassAtom",
    class_predicate = SWRL "classPredicate",
    data_range_atom = SWRL "DataRangeAtom",
    data_range = SWRL "dataRange",
    individual_property_atom = SWRL "IndividualPropertyAtom",
    datavalued_property_atom = SWRL "DatavaluedPropertyAtom",
    property_predicate = SWRL "propertyPredicate",
    builtin_atom = SWRL "BuiltinAtom",
    builtin = SWRL "builtin",
    arguments = SWRL "arguments",
    same_individual_atom = SWRL "SameIndividualAtom",
    different_individuals_atom = SWRL "DifferentIndividualsAtom",
    argument1 = SWRL "argument1",
    argument2 = SWRL "argument2",
    variable = SWRL "Variable",
);

/// What only the producer of a tree knows, which the mapping asks it: how
/// node `i` of the tree spells its term, where a construct's keyword is,
/// and whose parse it is. Spans are offsets of the producer's text. The
/// answers depend on the tree and the node only, never on what was asked
/// before. A source never interns: the mapper expands, memoises and
/// interns each IRI value, under its budget.
pub(crate) trait Source<'a> {
    /// The syntax of the parse.
    const SYNTAX: Syntax;
    /// The code of the errors the mapping makes (a limit, an
    /// `rdf:PlainLiteral` outside its lexical space).
    const CODE: &'static str;
    /// The length of the text, which the budgets are per byte of.
    fn len(&self) -> usize;
    /// The key under which the value of IRI node `i` is memoised: in one
    /// document, equal keys have equal values (so a key tells apart the
    /// bases a reference resolves against).
    fn key(&self, doc: &Document, i: usize) -> Key<'a>;
    /// The value of IRI node `i`.
    fn value(&self, doc: &Document, i: usize) -> IriValue<'a>;
    /// How the text at IRI node `i` spells its IRI.
    fn form(&self, doc: &Document, i: usize) -> Form;
    /// The label of anonymous individual `i`: in one document, one label is
    /// one blank node.
    fn label(&self, doc: &Document, i: usize) -> &'a str;
    /// Literal `i`. Its datatype, if it has one, is its child.
    fn literal(&self, doc: &Document, i: usize) -> Literal<'a>;
    /// The digits of integer `i`.
    fn integer(&self, doc: &Document, i: usize) -> &'a str;
    /// The keyword of construct `i`, where the triples its rule makes are.
    fn keyword(&self, doc: &Document, i: usize) -> Span;
    /// The keyword of the ontology at `ontology` ([`Document::ontology`]).
    fn ontology_keyword(&self, ontology: Span) -> Span;
}

/// The value of an IRI node, as its producer has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IriValue<'a> {
    /// A full IRI.
    Full(&'a str),
    /// A prefix IRI, and the local part that follows it.
    Prefixed { prefix: &'a str, local: &'a str },
    /// An absolute IRI and a reference to resolve against it (RFC 3986,
    /// section 5.2), each valid: the source checked them, so the result is
    /// an IRI. The resolution is charged to the IRI budget each time, a
    /// new IRI or not, at the length of the two.
    Based { base: &'a str, reference: &'a str },
}

/// The key of an IRI value: its text as written, and the scope it is
/// written in, where one text means another IRI in another scope (a prefix
/// name bound again).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key<'a> {
    pub(crate) scope: u32,
    pub(crate) written: &'a str,
}

/// A literal, as its producer has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Literal<'a> {
    /// Its lexical form, its escapes read.
    pub(crate) lexical: Cow<'a, str>,
    /// Its language tag as written, if it has one and no datatype.
    pub(crate) lang: Option<&'a str>,
    /// Its string, where an error about its lexical form is.
    pub(crate) string: Span,
}

/// The shared parse of `doc`, the grammar's reading of `text`: its
/// triples, references, statements and prefixes, and its errors. `base`
/// is the document's URL.
pub(crate) fn map(text: &str, doc: Document, base: Option<&str>) -> Parsed {
    map_with(&Functional::new(text, &doc.prefixes), doc, base)
}

/// The shared parse of `doc`, its terms as `source` gives them: its
/// triples, references, statements and prefixes, and its errors. `base`
/// is the document's URL.
pub(crate) fn map_with<'a, S: Source<'a>>(source: &S, doc: Document, base: Option<&str>) -> Parsed {
    let mut m = Mapper::new(source, &doc);
    m.declarations();
    m.document();
    let Mapper {
        triples,
        refs,
        statements,
        limit,
        mut invalid,
        ..
    } = m;
    let at =
        invalid.partition_point(|x| limit.as_ref().is_none_or(|e| x.span.start <= e.span.start));
    invalid.splice(at..at, limit);
    invalid.truncate(MAX_ERRORS.saturating_sub(doc.errors.len()));
    let errors = merge(doc.errors, invalid);
    Parsed {
        syntax: S::SYNTAX,
        prefixes: doc.prefixes,
        document_base: base.map(str::to_string),
        statements,
        refs,
        triples,
        errors,
        ..Parsed::default()
    }
}

/// The errors `a` and `b`, each sorted by start, as one sorted list; of
/// two at one start, `a`'s first.
fn merge(a: Vec<SyntaxError>, b: Vec<SyntaxError>) -> Vec<SyntaxError> {
    if b.is_empty() {
        return a;
    }
    let mut out = Vec::with_capacity(a.len() + b.len());
    let mut b = b.into_iter().peekable();
    for e in a {
        while let Some(x) = b.next_if(|x| x.span.start < e.span.start) {
            out.push(x);
        }
        out.push(e);
    }
    out.extend(b);
    out
}

// ---- the Functional source ----

/// The prefix names of Table 2, which every document has.
const STANDARD: [(&str, &str); 4] = [
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
];

/// A Functional text, the source of the tree the grammar reads from it:
/// every term is its text at its node's span.
struct Functional<'a> {
    text: &'a str,
    /// Each prefix name's bindings.
    bound: FxHashMap<&'a str, Bound<'a>>,
    /// The starts of the declarations that bind a name of Table 2 to
    /// another IRI than its own, in text order: past each, the text of an
    /// abbreviated IRI may mean another IRI.
    rebound: Vec<u32>,
}

/// A prefix name's bindings. A declaration binds a name from where it is
/// written, as the grammar read it; a name of Table 2 is bound before its
/// declaration too, and a second declaration of a name has no effect.
#[derive(Default)]
struct Bound<'a> {
    /// Its IRI in Table 2, if it is one of its names.
    standard: Option<&'a str>,
    /// The start of its declaration, and the IRI it binds.
    declared: Option<(u32, &'a str)>,
}

impl<'a> Functional<'a> {
    fn new(text: &'a str, prefixes: &[PrefixDecl]) -> Functional<'a> {
        let mut bound: FxHashMap<&str, Bound> = STANDARD
            .iter()
            .map(|&(name, iri)| {
                let b = Bound {
                    standard: Some(iri),
                    declared: None,
                };
                (name, b)
            })
            .collect();
        let mut rebound = Vec::new();
        let at = |span: Span| &text[span.start as usize..span.end as usize];
        for d in prefixes {
            let name = at(d.name_span);
            let name = &name[..name.len() - 1];
            let iri = at(d.iri_span);
            let iri = &iri[1..iri.len() - 1];
            let b = bound.entry(name).or_default();
            if b.declared.is_none() {
                b.declared = Some((d.span.start, iri));
                if b.standard.is_some_and(|s| s != iri) {
                    rebound.push(d.span.start);
                }
            }
        }
        Functional {
            text,
            bound,
            rebound,
        }
    }

    fn at(&self, span: Span) -> &'a str {
        &self.text[span.start as usize..span.end as usize]
    }

    /// The node of IRI `i`, and its prefix name's length if abbreviated.
    fn iri_node(doc: &Document, i: usize) -> (Span, Option<u32>) {
        let n = doc.nodes[i];
        let NodeKind::Iri { prefix_len } = n.kind else {
            unreachable!("not an IRI: {:?}", n.kind)
        };
        (n.span, prefix_len)
    }
}

impl<'a> Source<'a> for Functional<'a> {
    const SYNTAX: Syntax = Syntax::OwlFunctional;
    const CODE: &'static str = CODE;

    fn len(&self) -> usize {
        self.text.len()
    }

    /// The IRI as written; an abbreviated one in the scope of the
    /// rebindings before it.
    fn key(&self, doc: &Document, i: usize) -> Key<'a> {
        let (span, prefix_len) = Self::iri_node(doc, i);
        let scope = match prefix_len {
            None => 0,
            Some(_) => self.rebound.partition_point(|&at| at < span.start) as u32,
        };
        Key {
            scope,
            written: self.at(span),
        }
    }

    /// `<…>` without its brackets, or `pfx:local` with the IRI `pfx:` is
    /// bound to where it is written.
    fn value(&self, doc: &Document, i: usize) -> IriValue<'a> {
        let (span, prefix_len) = Self::iri_node(doc, i);
        let written = self.at(span);
        let Some(len) = prefix_len else {
            return IriValue::Full(&written[1..written.len() - 1]);
        };
        let len = len as usize;
        // The grammar checked that the prefix is bound.
        let prefix = self
            .bound
            .get(&written[..len])
            .map_or("", |b| match b.declared {
                Some((from, iri)) if from < span.start => iri,
                _ => b.standard.unwrap_or(""),
            });
        IriValue::Prefixed {
            prefix,
            local: &written[len + 1..],
        }
    }

    fn form(&self, doc: &Document, i: usize) -> Form {
        match Self::iri_node(doc, i).1 {
            None => Form::Iri,
            Some(prefix_len) => Form::PName { prefix_len },
        }
    }

    fn label(&self, doc: &Document, i: usize) -> &'a str {
        self.at(doc.nodes[i].span)
    }

    /// `"…"`, `"…"@tag` or `"…"^^datatype`: the lexical form between the
    /// quotes, a tag after the last `@` past them.
    fn literal(&self, doc: &Document, i: usize) -> Literal<'a> {
        let n = doc.nodes[i];
        let NodeKind::Literal { string_end } = n.kind else {
            unreachable!("not a literal: {:?}", n.kind)
        };
        let raw = &self.text[n.span.start as usize + 1..string_end as usize - 1];
        let lexical = if raw.contains('\\') {
            // The lexer let through `\"` and `\\` only.
            let mut out = String::with_capacity(raw.len());
            let mut chars = raw.chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => out.extend(chars.next()),
                    c => out.push(c),
                }
            }
            out.into()
        } else {
            raw.into()
        };
        let lang = if n.next as usize > i + 1 {
            None
        } else {
            let after = &self.text[string_end as usize..n.span.end as usize];
            after.rfind('@').map(|at| &after[at + 1..])
        };
        Literal {
            lexical,
            lang,
            string: Span::new(n.span.start, string_end),
        }
    }

    fn integer(&self, doc: &Document, i: usize) -> &'a str {
        self.at(doc.nodes[i].span)
    }

    /// The construct's start, as long as its keyword.
    fn keyword(&self, doc: &Document, i: usize) -> Span {
        let n = doc.nodes[i];
        let len = match n.kind {
            NodeKind::Construct(k) => k.as_str().len() as u32,
            _ => n.span.end - n.span.start,
        };
        Span::new(n.span.start, (n.span.start + len).min(n.span.end))
    }

    fn ontology_keyword(&self, ontology: Span) -> Span {
        Span::new(ontology.start, ontology.start + "Ontology".len() as u32)
    }
}

/// A triple an axiom says, with where its predicate and object are written.
struct Said {
    s: Node,
    p: Iri,
    o: Node,
    p_span: Span,
    o_span: Span,
}

/// The item being mapped is given up: a limit is reached.
struct Over;

type M<T = ()> = Result<T, Over>;

struct Mapper<'d, 'a, S> {
    source: &'d S,
    doc: &'d Document,
    v: &'static Vocab,
    /// Each IRI made, by its key.
    memo: FxHashMap<Key<'a>, Iri>,
    buf: String,
    /// The IRIs this parse made, and their bytes.
    made: FxHashSet<Iri>,
    iri_bytes: usize,
    iri_budget: usize,
    triple_budget: usize,
    /// The bytes of the literals of the triples so far.
    literal_bytes: usize,
    literal_budget: usize,
    /// The first limit met, reported once.
    limit: Option<SyntaxError>,
    /// The `rdf:PlainLiteral`s outside its lexical space, one per item, in
    /// text order.
    invalid: Vec<SyntaxError>,
    blanks: FxHashMap<&'a str, BlankId>,
    next_blank: u32,
    triples: Vec<Triple>,
    refs: Vec<Ref>,
    statements: Vec<Statement>,
    /// The roles of the IRI nodes of the item being mapped that are not
    /// objects.
    roles: FxHashMap<u32, Role>,
    /// The IRIs some mapped `Declaration` names.
    declared: FxHashSet<Iri>,
    /// The IRIs defined so far that no `Declaration` names.
    defined: FxHashSet<Iri>,
    /// The SWRL variables typed so far.
    variables: FxHashSet<Iri>,
}

impl<'d, 'a, S: Source<'a>> Mapper<'d, 'a, S> {
    fn new(source: &'d S, doc: &'d Document) -> Mapper<'d, 'a, S> {
        let len = source.len();
        Mapper {
            source,
            doc,
            v: vocab(),
            memo: FxHashMap::default(),
            buf: String::new(),
            made: FxHashSet::default(),
            iri_bytes: 0,
            iri_budget: len.saturating_mul(IRI_BYTES_PER_BYTE) + MIN_IRI_BYTES,
            triple_budget: len.saturating_mul(TRIPLES_PER_BYTE) + MIN_TRIPLES,
            literal_bytes: 0,
            literal_budget: len.saturating_mul(LITERAL_BYTES_PER_BYTE) + MIN_LITERAL_BYTES,
            limit: None,
            invalid: Vec::new(),
            blanks: FxHashMap::default(),
            next_blank: 0,
            triples: Vec::new(),
            refs: Vec::new(),
            statements: Vec::new(),
            roles: FxHashMap::default(),
            declared: FxHashSet::default(),
            defined: FxHashSet::default(),
            variables: FxHashSet::default(),
        }
    }

    // ---- the tree ----

    fn children(&self, i: usize) -> Vec<usize> {
        self.doc.children(i).collect()
    }

    fn keyword_of(&self, i: usize) -> Option<K> {
        match self.doc.nodes[i].kind {
            NodeKind::Construct(k) => Some(k),
            _ => None,
        }
    }

    /// The keyword of construct `i`, where the triples its rule makes are.
    fn keyword_span(&self, i: usize) -> Span {
        self.source.keyword(self.doc, i)
    }

    fn span(&self, i: usize) -> Span {
        self.doc.nodes[i].span
    }

    /// The datatype node of literal `i`, if it has one.
    fn datatype_of(&self, i: usize) -> Option<usize> {
        (self.doc.nodes[i].next as usize > i + 1).then_some(i + 1)
    }

    // ---- terms ----

    /// The IRI of node `i`, made from its value once per key.
    fn iri(&mut self, i: usize) -> M<Iri> {
        let key = self.source.key(self.doc, i);
        if let Some(&iri) = self.memo.get(&key) {
            return Ok(iri);
        }
        let span = self.span(i);
        let iri = match self.source.value(self.doc, i) {
            IriValue::Full(iri) => self.intern(iri, span)?,
            IriValue::Prefixed { prefix, local } => {
                let mut buf = std::mem::take(&mut self.buf);
                buf.clear();
                buf.push_str(prefix);
                buf.push_str(local);
                let iri = self.intern(&buf, span);
                self.buf = buf;
                iri?
            }
            IriValue::Based { base, reference } => {
                // The work is the base's length, however many references
                // resolve to one IRI: charged before it is done.
                let cost = base.len() + reference.len();
                if self.iri_bytes + cost > self.iri_budget {
                    return self.over_iris(span);
                }
                self.iri_bytes += cost;
                let mut buf = std::mem::take(&mut self.buf);
                buf.clear();
                oxiri::Iri::parse_unchecked(base)
                    .resolve_into_unchecked(&oxiri::IriRef::parse_unchecked(reference), &mut buf);
                let iri = self.intern(&buf, span);
                self.buf = buf;
                iri?
            }
        };
        self.memo.insert(key, iri);
        Ok(iri)
    }

    /// The interned IRI `s`, unless it is new to this parse and past the
    /// budget ([`IRI_BYTES_PER_BYTE`]).
    fn intern(&mut self, s: &str, span: Span) -> M<Iri> {
        if self.iri_bytes + s.len() <= self.iri_budget {
            let iri = Iri::intern(s);
            if self.made.insert(iri) {
                self.iri_bytes += s.len();
            }
            return Ok(iri);
        }
        if let Some(iri) = Iri::get(s).filter(|i| self.made.contains(i)) {
            return Ok(iri);
        }
        self.over_iris(span)
    }

    /// The IRI budget is spent, at `span`.
    fn over_iris<T>(&mut self, span: Span) -> M<T> {
        self.over(
            span,
            format!(
                "The IRIs are over {IRI_BYTES_PER_BYTE} bytes per byte of text: the axioms from here on are not read"
            ),
        )
    }

    fn over<T>(&mut self, span: Span, message: String) -> M<T> {
        if self.limit.is_none() {
            self.limit = Some(SyntaxError::new(span, message, Severity::Error, S::CODE));
        }
        Err(Over)
    }

    fn blank(&mut self) -> Node {
        let b = BlankId(self.next_blank);
        self.next_blank += 1;
        Node::Blank(b)
    }

    /// The main node of T(node `i`): an IRI, an anonymous individual, a
    /// literal, or the blank node of an expression, whose triples are made.
    fn term(&mut self, i: usize) -> M<Node> {
        let n = self.doc.nodes[i];
        match n.kind {
            NodeKind::Iri { .. } => Ok(Node::Iri(self.iri(i)?)),
            NodeKind::NodeId => {
                let label = self.source.label(self.doc, i);
                if let Some(&b) = self.blanks.get(label) {
                    return Ok(Node::Blank(b));
                }
                let Node::Blank(b) = self.blank() else {
                    unreachable!()
                };
                self.blanks.insert(label, b);
                Ok(Node::Blank(b))
            }
            NodeKind::Literal { .. } => self.literal(i),
            NodeKind::Integer => Ok(Node::Literal {
                value: self.source.integer(self.doc, i).into(),
                datatype: self.v.non_negative_integer,
                lang: None,
                dir: None,
            }),
            NodeKind::Construct(k) => self.expression(i, k),
            NodeKind::Group => unreachable!("a HasKey group is no term"),
        }
    }

    /// `"…"`, `"…"@tag` or `"…"^^datatype` (Table 1: a plain literal is
    /// `"abc"` or `"abc"@tag`, any other its own literal).
    fn literal(&mut self, i: usize) -> M<Node> {
        let Literal { lexical, lang, .. } = self.source.literal(self.doc, i);
        let value: Box<str> = lexical.into();
        let v = self.v;
        if let Some(d) = self.datatype_of(i) {
            let datatype = self.iri(d)?;
            self.roles.insert(d as u32, Role::Datatype);
            if datatype == v.plain_literal {
                // Its `@` was checked before the item was mapped
                // ([`Mapper::invalid_literal`]).
                let (lexical, tag) = value.rsplit_once('@').unwrap_or((&value, ""));
                return Ok(lang_literal(v, lexical.into(), tag));
            }
            return Ok(Node::Literal {
                value,
                datatype,
                lang: None,
                dir: None,
            });
        }
        match lang {
            Some(tag) => Ok(lang_literal(v, value, tag)),
            None => Ok(Node::Literal {
                value,
                datatype: v.string,
                lang: None,
                dir: None,
            }),
        }
    }

    /// The error of the first `rdf:PlainLiteral` in item `item` that is not
    /// of the form `"abc@langTag"`, at its quoted string (see the module
    /// doc). Checked before the item is mapped, so an item with one gives
    /// nothing and changes nothing.
    fn invalid_literal(&mut self, item: usize) -> M<Option<SyntaxError>> {
        let end = self.doc.nodes[item].next as usize;
        for j in item..end {
            if !matches!(self.doc.nodes[j].kind, NodeKind::Literal { .. }) {
                continue;
            }
            let Some(d) = self.datatype_of(j) else {
                continue;
            };
            if self.iri(d)? != self.v.plain_literal {
                continue;
            }
            let Literal {
                lexical, string, ..
            } = self.source.literal(self.doc, j);
            let message = match lexical.rsplit_once('@') {
                None => "An rdf:PlainLiteral is \"text@tag\" or \"text@\": no '@' here".to_string(),
                Some((_, "")) => continue,
                Some((_, tag)) => match oxilangtag::LanguageTag::parse(tag) {
                    Ok(_) => continue,
                    Err(e) => format!(
                        "Invalid language tag '{}' in an rdf:PlainLiteral: {e}",
                        super::shown(tag)
                    ),
                },
            };
            return Ok(Some(SyntaxError::new(
                string,
                message,
                Severity::Error,
                S::CODE,
            )));
        }
        Ok(None)
    }

    /// A class expression, property expression, data range or SWRL
    /// variable `k` at node `i`: the rows of Table 1.
    fn expression(&mut self, i: usize, k: K) -> M<Node> {
        let v = self.v;
        let kw = self.keyword_span(i);
        let cs = self.children(i);
        if k == K::Variable {
            // A variable is its IRI, typed once.
            let iri = self.iri(cs[0])?;
            if self.variables.insert(iri) {
                let o = Node::Iri(v.variable);
                self.emit(Node::Iri(iri), v.rdf_type, o, kw, kw)?;
            }
            return Ok(Node::Iri(iri));
        }
        let x = self.blank();
        let ty = |m: &mut Self, class: Iri| m.emit(x.clone(), v.rdf_type, Node::Iri(class), kw, kw);
        match k {
            K::ObjectInverseOf => {
                self.arc(&x, v.inverse_of, cs[0], kw)?;
            }
            K::DataIntersectionOf | K::DataUnionOf => {
                ty(self, v.datatype)?;
                let p = if k == K::DataIntersectionOf {
                    v.intersection_of
                } else {
                    v.union_of
                };
                let list = self.list(&cs, None)?;
                self.emit(x.clone(), p, list, kw, self.span(i))?;
            }
            K::DataComplementOf => {
                ty(self, v.datatype)?;
                self.arc(&x, v.datatype_complement_of, cs[0], kw)?;
            }
            K::DataOneOf => {
                ty(self, v.datatype)?;
                let list = self.list(&cs, None)?;
                self.emit(x.clone(), v.one_of, list, kw, self.span(i))?;
            }
            K::DatatypeRestriction => {
                ty(self, v.datatype)?;
                self.arc(&x, v.on_datatype, cs[0], kw)?;
                // `_:x owl:withRestrictions T(SEQ _:y1 … _:yn)`, `_:yi Fi lti`.
                let facets: Vec<(usize, usize)> = cs[1..].chunks(2).map(|c| (c[0], c[1])).collect();
                let mut ys = Vec::with_capacity(facets.len());
                for &(f, lt) in &facets {
                    let y = self.blank();
                    let p = self.predicate(f)?;
                    let o = self.term(lt)?;
                    self.emit(y.clone(), p, o, self.span(f), self.span(lt))?;
                    ys.push((y, Span::new(self.span(f).start, self.span(lt).end)));
                }
                let list = self.list_of(ys, None)?;
                self.emit(x.clone(), v.with_restrictions, list, kw, self.span(i))?;
            }
            K::ObjectIntersectionOf | K::ObjectUnionOf => {
                ty(self, v.class)?;
                let p = if k == K::ObjectIntersectionOf {
                    v.intersection_of
                } else {
                    v.union_of
                };
                let list = self.list(&cs, None)?;
                self.emit(x.clone(), p, list, kw, self.span(i))?;
            }
            K::ObjectComplementOf => {
                ty(self, v.class)?;
                self.arc(&x, v.complement_of, cs[0], kw)?;
            }
            K::ObjectOneOf => {
                ty(self, v.class)?;
                let list = self.list(&cs, None)?;
                self.emit(x.clone(), v.one_of, list, kw, self.span(i))?;
            }
            K::ObjectSomeValuesFrom
            | K::ObjectAllValuesFrom
            | K::DataSomeValuesFrom
            | K::DataAllValuesFrom => {
                ty(self, v.restriction)?;
                let (props, range) = cs.split_at(cs.len() - 1);
                if let [p] = props {
                    self.arc(&x, v.on_property, *p, kw)?;
                } else {
                    let list = self.list(props, None)?;
                    let span = Span::new(
                        self.span(props[0]).start,
                        self.span(props[props.len() - 1]).end,
                    );
                    self.emit(x.clone(), v.on_properties, list, kw, span)?;
                }
                let p = if matches!(k, K::ObjectSomeValuesFrom | K::DataSomeValuesFrom) {
                    v.some_values_from
                } else {
                    v.all_values_from
                };
                self.arc(&x, p, range[0], kw)?;
            }
            K::ObjectHasValue | K::DataHasValue => {
                ty(self, v.restriction)?;
                self.arc(&x, v.on_property, cs[0], kw)?;
                self.arc(&x, v.has_value, cs[1], kw)?;
            }
            K::ObjectHasSelf => {
                ty(self, v.restriction)?;
                self.arc(&x, v.on_property, cs[0], kw)?;
                let t = Node::Literal {
                    value: "true".into(),
                    datatype: v.boolean,
                    lang: None,
                    dir: None,
                };
                self.emit(x.clone(), v.has_self, t, kw, kw)?;
            }
            K::ObjectMinCardinality
            | K::ObjectMaxCardinality
            | K::ObjectExactCardinality
            | K::DataMinCardinality
            | K::DataMaxCardinality
            | K::DataExactCardinality => {
                ty(self, v.restriction)?;
                self.arc(&x, v.on_property, cs[1], kw)?;
                let qualified = cs.len() > 2;
                let p = match (k, qualified) {
                    (K::ObjectMinCardinality | K::DataMinCardinality, false) => v.min_cardinality,
                    (K::ObjectMaxCardinality | K::DataMaxCardinality, false) => v.max_cardinality,
                    (_, false) => v.cardinality,
                    (K::ObjectMinCardinality | K::DataMinCardinality, true) => {
                        v.min_qualified_cardinality
                    }
                    (K::ObjectMaxCardinality | K::DataMaxCardinality, true) => {
                        v.max_qualified_cardinality
                    }
                    (_, true) => v.qualified_cardinality,
                };
                self.arc(&x, p, cs[0], kw)?;
                if qualified {
                    let on = if matches!(
                        k,
                        K::ObjectMinCardinality
                            | K::ObjectMaxCardinality
                            | K::ObjectExactCardinality
                    ) {
                        v.on_class
                    } else {
                        v.on_data_range
                    };
                    self.arc(&x, on, cs[2], kw)?;
                }
            }
            other => unreachable!("{other:?} is no expression"),
        }
        Ok(x)
    }

    /// `s p T(node o)`, the predicate made by the rule of keyword `kw`.
    fn arc(&mut self, s: &Node, p: Iri, o: usize, kw: Span) -> M {
        let t = self.term(o)?;
        self.emit(s.clone(), p, t, kw, self.span(o))
    }

    /// The IRI of node `i`, as a triple's predicate.
    fn predicate(&mut self, i: usize) -> M<Iri> {
        self.roles.insert(i as u32, Role::Predicate);
        self.iri(i)
    }

    /// `T(SEQ …)` of the nodes `items`: `rdf:nil`, or the first of a chain
    /// of blank nodes, each also of type `ty` when given.
    fn list(&mut self, items: &[usize], ty: Option<Iri>) -> M<Node> {
        let mut terms = Vec::with_capacity(items.len());
        for &i in items {
            terms.push((self.term(i)?, self.span(i)));
        }
        self.list_of(terms, ty)
    }

    /// An RDF list of `items`, each at its span.
    fn list_of(&mut self, items: Vec<(Node, Span)>, ty: Option<Iri>) -> M<Node> {
        let v = self.v;
        let nodes: Vec<Node> = items.iter().map(|_| self.blank()).collect();
        let spans: Vec<Span> = items.iter().map(|(_, s)| *s).collect();
        for (k, (item, span)) in items.into_iter().enumerate() {
            let x = nodes[k].clone();
            if let Some(ty) = ty {
                self.emit(x.clone(), v.rdf_type, Node::Iri(ty), span, span)?;
            }
            self.emit(x.clone(), v.first, item, span, span)?;
            let (rest, o_span) = match nodes.get(k + 1) {
                Some(next) => (next.clone(), spans[k + 1]),
                None => (Node::Iri(v.nil), span),
            };
            self.emit(x, v.rest, rest, span, o_span)?;
        }
        Ok(nodes.into_iter().next().unwrap_or(Node::Iri(v.nil)))
    }

    fn emit(&mut self, s: Node, p: Iri, o: Node, p_span: Span, o_span: Span) -> M {
        if self.triples.len() >= self.triple_budget {
            return self.over(
                p_span,
                format!(
                    "The axioms make over {TRIPLES_PER_BYTE} triples per byte of text: the axioms from here on are not read"
                ),
            );
        }
        if let Node::Literal { value, .. } = &o {
            self.literal_bytes += value.len();
            if self.literal_bytes > self.literal_budget {
                return self.over(
                    p_span,
                    format!(
                        "The axioms' literals are over {LITERAL_BYTES_PER_BYTE} bytes per byte of text: the axioms from here on are not read"
                    ),
                );
            }
        }
        self.triples.push(Triple {
            s,
            p,
            o,
            p_span,
            o_span,
            statement: self.statements.len() as u32 - 1,
            graph: None,
            unresolved: false,
        });
        Ok(())
    }

    // ---- annotations (section 2.2) and annotated axioms (section 2.3) ----

    /// TANN(annotation `a`, `y`).
    fn annotation(&mut self, a: usize, y: &Node) -> M {
        let v = self.v;
        let cs = self.children(a);
        let (nested, rest) = cs.split_at(cs.len() - 2);
        let (ap, av) = (rest[0], rest[1]);
        let p = self.predicate(ap)?;
        let o = self.term(av)?;
        self.emit(y.clone(), p, o.clone(), self.span(ap), self.span(av))?;
        if !nested.is_empty() {
            let kw = self.keyword_span(a);
            let x = self.blank();
            self.reification(&x, v.annotation, (y, p, &o), kw, self.span(av))?;
            for &n in nested {
                self.annotation(n, &x)?;
            }
        }
        Ok(())
    }

    /// `x` of type `class`, with `s p o` as its annotated source, property
    /// and target.
    fn reification(
        &mut self,
        x: &Node,
        class: Iri,
        (s, p, o): (&Node, Iri, &Node),
        kw: Span,
        o_span: Span,
    ) -> M {
        let v = self.v;
        self.emit(x.clone(), v.rdf_type, Node::Iri(class), kw, kw)?;
        self.emit(x.clone(), v.annotated_source, s.clone(), kw, kw)?;
        self.emit(x.clone(), v.annotated_property, Node::Iri(p), kw, kw)?;
        self.emit(x.clone(), v.annotated_target, o.clone(), kw, o_span)
    }

    /// The main triple of an axiom (section 2.3.1), with its annotations
    /// `anns` on an `owl:Axiom` node when it has any; `kw` is the axiom's
    /// keyword.
    fn main(&mut self, anns: &[usize], t: Said, kw: Span) -> M {
        let Said {
            s,
            p,
            o,
            p_span,
            o_span,
        } = t;
        self.emit(s.clone(), p, o.clone(), p_span, o_span)?;
        if !anns.is_empty() {
            let x = self.blank();
            self.reification(&x, self.v.axiom, (&s, p, &o), kw, o_span)?;
            for &a in anns {
                self.annotation(a, &x)?;
            }
        }
        Ok(())
    }

    /// The annotations `anns` on the blank node `x` of an axiom (section
    /// 2.3.3).
    fn annotations_on(&mut self, anns: &[usize], x: &Node) -> M {
        for &a in anns {
            self.annotation(a, x)?;
        }
        Ok(())
    }

    // ---- the document ----

    /// The top-level nodes to map: inside `Ontology( … )`, read whole.
    fn items(&self) -> Vec<usize> {
        let Some(ontology) = self.doc.ontology else {
            return Vec::new();
        };
        let mut broken = self.doc.broken.iter().peekable();
        self.doc
            .top()
            .filter(|&i| {
                while broken.next_if(|&&b| (b as usize) < i).is_some() {}
                let whole = broken.peek().is_none_or(|&&b| b as usize != i);
                let span = self.span(i);
                whole && span.start >= ontology.start && span.end <= ontology.end
            })
            .collect()
    }

    /// The first pass: the IRIs that mapped `Declaration`s name.
    fn declarations(&mut self) {
        for i in self.items() {
            if self.keyword_of(i) != Some(K::Declaration) {
                continue;
            }
            // An annotation of the declaration may make it give nothing.
            match self.invalid_literal(i) {
                Ok(None) => {}
                Ok(Some(_)) => continue,
                Err(Over) => break,
            }
            let entity = *self.children(i).last().unwrap();
            let iri_node = entity + 1;
            match self.iri(iri_node) {
                Ok(iri) => {
                    self.declared.insert(iri);
                }
                Err(Over) => break,
            }
        }
        // The pass may have met a limit the mapping meets again.
        self.limit = None;
    }

    fn document(&mut self) {
        let items = self.items();
        let ontology_iri = self.doc.ontology_iri;
        let version_iri = self.doc.version_iri;
        let header_whole = !self.doc.header_broken;
        let mut subject = None;
        if let Some(ontology) = self.doc.ontology
            && header_whole
        {
            match self.header(ontology) {
                Ok(s) => subject = Some(s),
                Err(Over) => {
                    self.triples.clear();
                    self.refs.clear();
                    self.statements.clear();
                    return;
                }
            }
        }
        for i in items {
            if Some(i) == ontology_iri || Some(i) == version_iri {
                continue;
            }
            let k = self.keyword_of(i);
            if matches!(k, Some(K::Import | K::Annotation)) && subject.is_none() {
                continue;
            }
            match self.invalid_literal(i) {
                Ok(None) => {}
                Ok(Some(e)) => {
                    self.invalid.push(e);
                    continue;
                }
                Err(Over) => return,
            }
            let (triples, refs, blank) = (self.triples.len(), self.refs.len(), self.next_blank);
            self.statements.push(Statement {
                span: self.span(i),
                subject: None,
                graph: None,
            });
            self.roles.clear();
            let r = match k {
                Some(K::Import) => {
                    let s = subject.clone().unwrap();
                    let kw = self.keyword_span(i);
                    let target = self.children(i)[0];
                    self.arc(&s, self.v.imports, target, kw).map(|()| None)
                }
                Some(K::Annotation) => {
                    let s = subject.clone().unwrap();
                    self.annotation(i, &s).map(|()| None)
                }
                Some(k) => self.axiom(i, k),
                None => Ok(None),
            };
            match r {
                Ok(defines) => {
                    if let Some((node, iri, declaration)) = defines {
                        let defines = if declaration {
                            true
                        } else if self.declared.contains(&iri) {
                            false
                        } else {
                            self.defined.insert(iri)
                        };
                        if defines {
                            self.roles.insert(node as u32, Role::Subject);
                            let span = self.span(node);
                            self.statements.last_mut().unwrap().subject = Some((iri, span));
                        } else {
                            self.roles.insert(node as u32, Role::About);
                        }
                    }
                    self.references(i);
                }
                Err(Over) => {
                    self.triples.truncate(triples);
                    self.refs.truncate(refs);
                    self.next_blank = blank;
                    self.statements.pop();
                    return;
                }
            }
        }
    }

    /// The header: `ontologyIRI rdf:type owl:Ontology`, its version IRI, or
    /// a blank node when it has no IRI (Table 1). Its node, for the imports
    /// and ontology annotations.
    fn header(&mut self, ontology: Span) -> M<Node> {
        let v = self.v;
        let kw = self.source.ontology_keyword(ontology);
        let last = self.doc.version_iri.or(self.doc.ontology_iri);
        let end = last.map_or(kw.end, |i| self.span(i).end);
        self.statements.push(Statement {
            span: Span::new(ontology.start, end),
            subject: None,
            graph: None,
        });
        self.roles.clear();
        let s = match self.doc.ontology_iri {
            Some(i) => {
                let iri = self.iri(i)?;
                self.roles.insert(i as u32, Role::Subject);
                self.statements.last_mut().unwrap().subject = Some((iri, self.span(i)));
                Node::Iri(iri)
            }
            None => self.blank(),
        };
        self.emit(s.clone(), v.rdf_type, Node::Iri(v.ontology), kw, kw)?;
        if let Some(version) = self.doc.version_iri {
            self.arc(&s, v.version_iri, version, kw)?;
        }
        // In text order, whichever is written first.
        let mut written: Vec<usize> = [self.doc.ontology_iri, self.doc.version_iri]
            .into_iter()
            .flatten()
            .collect();
        written.sort_unstable();
        for i in written {
            self.references(i);
        }
        Ok(s)
    }

    /// The references of the IRIs written in node `i` and below, in text
    /// order, with the roles the mapping gave them.
    fn references(&mut self, i: usize) {
        let end = self.doc.nodes[i].next as usize;
        for j in i..end {
            let n = self.doc.nodes[j];
            if !matches!(n.kind, NodeKind::Iri { .. }) {
                continue;
            }
            let Some(&iri) = self.memo.get(&self.source.key(self.doc, j)) else {
                continue;
            };
            let role = self.roles.get(&(j as u32)).copied().unwrap_or(Role::Object);
            let form = self.source.form(self.doc, j);
            self.refs.push(Ref {
                span: n.span,
                iri,
                role,
                form,
            });
        }
    }

    /// Axiom `k` at node `i`: its triples (Table 1 and section 2.3), and
    /// the IRI node it would define: the subject of its main triple when
    /// written as an IRI, and whether the axiom is a declaration.
    fn axiom(&mut self, i: usize, k: K) -> M<Option<(usize, Iri, bool)>> {
        let v = self.v;
        let kw = self.keyword_span(i);
        let cs = self.children(i);
        let split = cs
            .iter()
            .position(|&c| self.keyword_of(c) != Some(K::Annotation))
            .unwrap_or(cs.len());
        let (anns, ops) = cs.split_at(split);
        let is_iri = |m: &Self, n: usize| matches!(m.doc.nodes[n].kind, NodeKind::Iri { .. });
        // The IRI node that is the main triple's subject, if one is.
        let mut subject = ops.first().copied().filter(|&n| is_iri(self, n));
        let mut declaration = false;
        // `s p o` from the operands `s` and `o`.
        let simple = |m: &mut Self, s: usize, p: Iri, o: usize| -> M {
            let (st, ot) = (m.term(s)?, m.term(o)?);
            m.main(
                anns,
                Said {
                    s: st,
                    p,
                    o: ot,
                    p_span: kw,
                    o_span: m.span(o),
                },
                kw,
            )
        };
        match k {
            K::Declaration => {
                let entity = ops[0];
                let class = match self.keyword_of(entity) {
                    Some(K::Class) => v.class,
                    Some(K::Datatype) => v.datatype,
                    Some(K::ObjectProperty) => v.object_property,
                    Some(K::DataProperty) => v.datatype_property,
                    Some(K::AnnotationProperty) => v.annotation_property,
                    Some(K::NamedIndividual) => v.named_individual,
                    other => unreachable!("{other:?} is no entity"),
                };
                let e = entity + 1;
                let s = self.term(e)?;
                let espan = self.keyword_span(entity);
                self.main(
                    anns,
                    Said {
                        s,
                        p: v.rdf_type,
                        o: Node::Iri(class),
                        p_span: espan,
                        o_span: espan,
                    },
                    kw,
                )?;
                subject = Some(e);
                declaration = true;
            }
            K::SubClassOf => simple(self, ops[0], v.sub_class_of, ops[1])?,
            K::EquivalentClasses
            | K::EquivalentObjectProperties
            | K::EquivalentDataProperties
            | K::SameIndividual => {
                let p = match k {
                    K::EquivalentClasses => v.equivalent_class,
                    K::SameIndividual => v.same_as,
                    _ => v.equivalent_property,
                };
                let mut terms = Vec::with_capacity(ops.len());
                for &o in ops {
                    terms.push(self.term(o)?);
                }
                for w in 0..ops.len() - 1 {
                    let (s, o) = (terms[w].clone(), terms[w + 1].clone());
                    self.main(
                        anns,
                        Said {
                            s,
                            p,
                            o,
                            p_span: kw,
                            o_span: self.span(ops[w + 1]),
                        },
                        kw,
                    )?;
                }
            }
            K::DisjointClasses
            | K::DisjointObjectProperties
            | K::DisjointDataProperties
            | K::DifferentIndividuals => {
                let (pair, all) = match k {
                    K::DisjointClasses => (v.disjoint_with, v.all_disjoint_classes),
                    K::DifferentIndividuals => (v.different_from, v.all_different),
                    _ => (v.property_disjoint_with, v.all_disjoint_properties),
                };
                if let [s, o] = ops {
                    simple(self, *s, pair, *o)?;
                } else {
                    subject = None;
                    let x = self.blank();
                    self.emit(x.clone(), v.rdf_type, Node::Iri(all), kw, kw)?;
                    let list = self.list(ops, None)?;
                    let span =
                        Span::new(self.span(ops[0]).start, self.span(ops[ops.len() - 1]).end);
                    self.emit(x.clone(), v.members, list, kw, span)?;
                    self.annotations_on(anns, &x)?;
                }
            }
            K::DisjointUnion => {
                let c = self.term(ops[0])?;
                let list = self.list(&ops[1..], None)?;
                let span = Span::new(self.span(ops[1]).start, self.span(ops[ops.len() - 1]).end);
                self.main(
                    anns,
                    Said {
                        s: c,
                        p: v.disjoint_union_of,
                        o: list,
                        p_span: kw,
                        o_span: span,
                    },
                    kw,
                )?;
            }
            K::SubObjectPropertyOf if self.keyword_of(ops[0]) == Some(K::ObjectPropertyChain) => {
                let chain = self.children(ops[0]);
                let sup = self.term(ops[1])?;
                let list = self.list(&chain, None)?;
                subject = Some(ops[1]).filter(|&n| is_iri(self, n));
                self.main(
                    anns,
                    Said {
                        s: sup,
                        p: v.property_chain_axiom,
                        o: list,
                        p_span: kw,
                        o_span: self.span(ops[0]),
                    },
                    kw,
                )?;
            }
            K::SubObjectPropertyOf | K::SubDataPropertyOf | K::SubAnnotationPropertyOf => {
                simple(self, ops[0], v.sub_property_of, ops[1])?
            }
            K::InverseObjectProperties => simple(self, ops[0], v.inverse_of, ops[1])?,
            K::ObjectPropertyDomain | K::DataPropertyDomain | K::AnnotationPropertyDomain => {
                simple(self, ops[0], v.domain, ops[1])?
            }
            K::ObjectPropertyRange | K::DataPropertyRange | K::AnnotationPropertyRange => {
                simple(self, ops[0], v.range, ops[1])?
            }
            K::FunctionalObjectProperty
            | K::InverseFunctionalObjectProperty
            | K::ReflexiveObjectProperty
            | K::IrreflexiveObjectProperty
            | K::SymmetricObjectProperty
            | K::AsymmetricObjectProperty
            | K::TransitiveObjectProperty
            | K::FunctionalDataProperty => {
                let class = match k {
                    K::FunctionalObjectProperty | K::FunctionalDataProperty => {
                        v.functional_property
                    }
                    K::InverseFunctionalObjectProperty => v.inverse_functional_property,
                    K::ReflexiveObjectProperty => v.reflexive_property,
                    K::IrreflexiveObjectProperty => v.irreflexive_property,
                    K::SymmetricObjectProperty => v.symmetric_property,
                    K::AsymmetricObjectProperty => v.asymmetric_property,
                    _ => v.transitive_property,
                };
                let s = self.term(ops[0])?;
                self.main(
                    anns,
                    Said {
                        s,
                        p: v.rdf_type,
                        o: Node::Iri(class),
                        p_span: kw,
                        o_span: kw,
                    },
                    kw,
                )?;
            }
            K::DatatypeDefinition => simple(self, ops[0], v.equivalent_class, ops[1])?,
            K::HasKey => {
                let ce = self.term(ops[0])?;
                let keys: Vec<usize> = ops[1..].iter().flat_map(|&g| self.children(g)).collect();
                let list = self.list(&keys, None)?;
                let span = Span::new(self.span(ops[1]).start, self.span(ops[2]).end);
                self.main(
                    anns,
                    Said {
                        s: ce,
                        p: v.has_key,
                        o: list,
                        p_span: kw,
                        o_span: span,
                    },
                    kw,
                )?;
            }
            K::ClassAssertion => {
                subject = Some(ops[1]).filter(|&n| is_iri(self, n));
                let (a, ce) = (self.term(ops[1])?, self.term(ops[0])?);
                self.main(
                    anns,
                    Said {
                        s: a,
                        p: v.rdf_type,
                        o: ce,
                        p_span: kw,
                        o_span: self.span(ops[0]),
                    },
                    kw,
                )?;
            }
            K::ObjectPropertyAssertion | K::DataPropertyAssertion | K::AnnotationAssertion => {
                let (mut s, mut o) = (ops[1], ops[2]);
                let mut p = ops[0];
                if self.keyword_of(p) == Some(K::ObjectInverseOf) {
                    // `ObjectPropertyAssertion( ObjectInverseOf( OP ) a1 a2 )`:
                    // `T(a2) T(OP) T(a1)`.
                    p += 1;
                    std::mem::swap(&mut s, &mut o);
                }
                subject = Some(s).filter(|&n| is_iri(self, n));
                let st = self.term(s)?;
                let pi = self.predicate(p)?;
                let ot = self.term(o)?;
                self.main(
                    anns,
                    Said {
                        s: st,
                        p: pi,
                        o: ot,
                        p_span: self.span(p),
                        o_span: self.span(o),
                    },
                    kw,
                )?;
            }
            K::NegativeObjectPropertyAssertion | K::NegativeDataPropertyAssertion => {
                subject = None;
                let x = self.blank();
                let o = Node::Iri(v.negative_property_assertion);
                self.emit(x.clone(), v.rdf_type, o, kw, kw)?;
                self.arc(&x, v.source_individual, ops[1], kw)?;
                self.arc(&x, v.assertion_property, ops[0], kw)?;
                let target = if k == K::NegativeObjectPropertyAssertion {
                    v.target_individual
                } else {
                    v.target_value
                };
                self.arc(&x, target, ops[2], kw)?;
                self.annotations_on(anns, &x)?;
            }
            K::DLSafeRule => {
                subject = None;
                self.rule(i, anns, ops)?;
            }
            other => unreachable!("{other:?} is no axiom"),
        }
        Ok(subject.map(|n| {
            let iri = self.memo[&self.source.key(self.doc, n)];
            (n, iri, declaration)
        }))
    }

    // ---- SWRL rules, as OWL API writes them ----

    /// `DLSafeRule( anns Body( … ) Head( … ) )` at node `i`.
    fn rule(&mut self, i: usize, anns: &[usize], ops: &[usize]) -> M {
        let v = self.v;
        let kw = self.keyword_span(i);
        let x = self.blank();
        self.emit(x.clone(), v.rdf_type, Node::Iri(v.imp), kw, kw)?;
        for (&part, p) in ops.iter().zip([v.body, v.head]) {
            let atoms = self.children(part);
            let mut terms = Vec::with_capacity(atoms.len());
            for &a in &atoms {
                terms.push((self.atom(a)?, self.span(a)));
            }
            let list = self.list_of(terms, Some(v.atom_list))?;
            self.emit(x.clone(), p, list, self.keyword_span(part), self.span(part))?;
        }
        self.annotations_on(anns, &x)
    }

    /// One atom: a blank node of its type with its predicate and arguments.
    fn atom(&mut self, i: usize) -> M<Node> {
        let v = self.v;
        let kw = self.keyword_span(i);
        let cs = self.children(i);
        let k = self.keyword_of(i).expect("an atom");
        let x = self.blank();
        let (class, predicate) = match k {
            K::ClassAtom => (v.class_atom, Some(v.class_predicate)),
            K::DataRangeAtom => (v.data_range_atom, Some(v.data_range)),
            K::ObjectPropertyAtom => (v.individual_property_atom, Some(v.property_predicate)),
            K::DataPropertyAtom => (v.datavalued_property_atom, Some(v.property_predicate)),
            K::BuiltInAtom => (v.builtin_atom, Some(v.builtin)),
            K::SameIndividualAtom => (v.same_individual_atom, None),
            K::DifferentIndividualsAtom => (v.different_individuals_atom, None),
            other => unreachable!("{other:?} is no atom"),
        };
        self.emit(x.clone(), v.rdf_type, Node::Iri(class), kw, kw)?;
        let args = match predicate {
            Some(p) => {
                self.arc(&x, p, cs[0], kw)?;
                &cs[1..]
            }
            None => &cs[..],
        };
        if k == K::BuiltInAtom {
            let list = self.list(args, Some(v.list))?;
            let span = Span::new(
                self.span(args[0]).start,
                self.span(args[args.len() - 1]).end,
            );
            self.emit(x.clone(), v.arguments, list, kw, span)?;
        } else {
            for (&a, p) in args.iter().zip([v.argument1, v.argument2]) {
                self.arc(&x, p, a, kw)?;
            }
        }
        Ok(x)
    }
}

/// `"value"@tag`, lowercased, or `"value"` when `tag` is empty.
fn lang_literal(v: &Vocab, value: Box<str>, tag: &str) -> Node {
    if tag.is_empty() {
        return Node::Literal {
            value,
            datatype: v.string,
            lang: None,
            dir: None,
        };
    }
    Node::Literal {
        value,
        datatype: v.lang_string,
        lang: Some(Iri::intern(&tag.to_ascii_lowercase())),
        dir: None,
    }
}
