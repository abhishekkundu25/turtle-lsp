//! What the grammar builds: the parse of one document, its constructs as a
//! flat tree in text order, each with its exact span.
//!
//! [`Document::nodes`] is the tree in preorder: a node's children follow
//! it, and [`Node::next`] is the index past its last descendant, so the
//! next sibling. The top-level nodes are, in text order, the ontology IRI
//! and version IRI ([`Document::ontology_iri`], [`Document::version_iri`]),
//! then each `Import`, ontology annotation and axiom. An IRI is kept as
//! written (its span); an abbreviated one is expanded with
//! [`Document::prefixes`]. Nothing but the prefix IRIs is interned or
//! copied out of the text.

use crate::syntax::{PrefixDecl, Span, SyntaxError};

/// The parse of a document.
#[derive(Debug, Default)]
pub(crate) struct Document {
    /// The `Prefix(…)` declarations, in text order (a repeated or reserved
    /// name too).
    pub(crate) prefixes: Vec<PrefixDecl>,
    /// `Ontology( … )`, from the keyword to its `)` (or to the last token
    /// when it is not closed).
    pub(crate) ontology: Option<Span>,
    /// The node of the ontology IRI, if written.
    pub(crate) ontology_iri: Option<usize>,
    /// The node of the version IRI, if written.
    pub(crate) version_iri: Option<usize>,
    pub(crate) nodes: Vec<Node>,
    /// Every comment (`#` to the end of the line).
    pub(crate) comments: Vec<Span>,
    /// Every error, in text order.
    pub(crate) errors: Vec<SyntaxError>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    /// A construct from its keyword to its `)` (to the last token read
    /// when it is broken off); a term as written.
    pub(crate) span: Span,
    /// The index past this node's last descendant.
    pub(crate) next: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeKind {
    /// `Keyword( … )`; its arguments are its children.
    Construct(Keyword),
    /// The bare `( … )` of a `HasKey` list.
    Group,
    /// A full IRI `<…>` (`prefix_len` is `None`) or an abbreviated one,
    /// whose prefix name is the first `prefix_len` bytes (before the `:`).
    Iri { prefix_len: Option<u32> },
    /// An anonymous individual, `_:label`.
    NodeId,
    /// `"…"`, `"…"@tag` or `"…"^^datatype`; the quoted string ends at
    /// `string_end`, and a datatype is the one child.
    Literal { string_end: u32 },
    /// A non-negative integer (a cardinality).
    Integer,
}

#[cfg_attr(not(test), expect(dead_code, reason = "only the tests read the tree"))]
impl Document {
    /// The indexes of the children of node `i`.
    pub(crate) fn children(&self, i: usize) -> impl Iterator<Item = usize> + '_ {
        let end = self.nodes[i].next as usize;
        let mut at = i + 1;
        std::iter::from_fn(move || {
            (at < end).then(|| {
                let here = at;
                at = self.nodes[here].next as usize;
                here
            })
        })
    }

    /// The indexes of the top-level nodes.
    pub(crate) fn top(&self) -> impl Iterator<Item = usize> + '_ {
        let mut at = 0;
        std::iter::from_fn(move || {
            (at < self.nodes.len()).then(|| {
                let here = at;
                at = self.nodes[here].next as usize;
                here
            })
        })
    }
}

macro_rules! keywords {
    ($($name:ident),* $(,)?) => {
        /// The keywords of the grammar (section 13), and those of the SWRL
        /// rules OWL API writes ([`super`] lists them as departures).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub(crate) enum Keyword {
            $($name),*
        }

        impl Keyword {
            pub(crate) fn of(word: &str) -> Option<Keyword> {
                match word {
                    $(stringify!($name) => Some(Keyword::$name),)*
                    _ => None,
                }
            }

            pub(crate) fn as_str(self) -> &'static str {
                match self {
                    $(Keyword::$name => stringify!($name)),*
                }
            }
        }
    };
}

keywords!(
    // The document.
    Prefix,
    Ontology,
    Import,
    Annotation,
    // Entities.
    Class,
    Datatype,
    ObjectProperty,
    DataProperty,
    AnnotationProperty,
    NamedIndividual,
    // Property expressions.
    ObjectInverseOf,
    ObjectPropertyChain,
    // Data ranges.
    DataIntersectionOf,
    DataUnionOf,
    DataComplementOf,
    DataOneOf,
    DatatypeRestriction,
    // Class expressions.
    ObjectIntersectionOf,
    ObjectUnionOf,
    ObjectComplementOf,
    ObjectOneOf,
    ObjectSomeValuesFrom,
    ObjectAllValuesFrom,
    ObjectHasValue,
    ObjectHasSelf,
    ObjectMinCardinality,
    ObjectMaxCardinality,
    ObjectExactCardinality,
    DataSomeValuesFrom,
    DataAllValuesFrom,
    DataHasValue,
    DataMinCardinality,
    DataMaxCardinality,
    DataExactCardinality,
    // Axioms.
    Declaration,
    SubClassOf,
    EquivalentClasses,
    DisjointClasses,
    DisjointUnion,
    SubObjectPropertyOf,
    EquivalentObjectProperties,
    DisjointObjectProperties,
    InverseObjectProperties,
    ObjectPropertyDomain,
    ObjectPropertyRange,
    FunctionalObjectProperty,
    InverseFunctionalObjectProperty,
    ReflexiveObjectProperty,
    IrreflexiveObjectProperty,
    SymmetricObjectProperty,
    AsymmetricObjectProperty,
    TransitiveObjectProperty,
    SubDataPropertyOf,
    EquivalentDataProperties,
    DisjointDataProperties,
    DataPropertyDomain,
    DataPropertyRange,
    FunctionalDataProperty,
    DatatypeDefinition,
    HasKey,
    SameIndividual,
    DifferentIndividuals,
    ClassAssertion,
    ObjectPropertyAssertion,
    NegativeObjectPropertyAssertion,
    DataPropertyAssertion,
    NegativeDataPropertyAssertion,
    AnnotationAssertion,
    SubAnnotationPropertyOf,
    AnnotationPropertyDomain,
    AnnotationPropertyRange,
    // SWRL rules, as OWL API writes them.
    DLSafeRule,
    Body,
    Head,
    ClassAtom,
    DataRangeAtom,
    ObjectPropertyAtom,
    DataPropertyAtom,
    BuiltInAtom,
    SameIndividualAtom,
    DifferentIndividualsAtom,
    Variable,
);
