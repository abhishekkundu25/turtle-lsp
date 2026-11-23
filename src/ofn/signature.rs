//! The signature of each construct of the tree ([`super::tree`]): its
//! operands, as argument slots in order, each a category ([`Cat`]) taken
//! once, at most once, or at least n times ([`Rep`]); which keywords are of
//! which category ([`keyword_in`]); which keywords are axioms
//! ([`is_axiom`]); and the words errors use for a category ([`Cat::name`],
//! [`Cat::counted`]). It is one model for every producer of the tree: the
//! Functional grammar reads its productions from it, and the OWL/XML
//! serialization has the same operands with the same counts.
//!
//! A slot is one node of the tree, but for four categories that are
//! Functional notation more than structure; each gives a fixed shape, which
//! a producer that reads other markup builds the same:
//! - [`Cat::ObjectKeys`] and [`Cat::DataKeys`] (the parenthesised lists of
//!   `HasKey`) are one [`super::tree::NodeKind::Group`] node each, whose children are the
//!   properties (none, or each of [`Cat::ObjectProperty`] or
//!   [`Cat::DataProperty`]);
//! - [`Cat::DataPropertiesThenRange`] (of `DataSomeValuesFrom` and
//!   `DataAllValuesFrom`) is two or more sibling nodes: the data
//!   properties, each an IRI node, then the data range, last;
//! - [`Cat::Facets`] (of `DatatypeRestriction`) is one or more pairs of
//!   sibling nodes: an IRI node (the constraining facet), then a literal
//!   node (its value).
//!
//! The rules the grammar checks beside the signature (that a `HasKey` names
//! a property, the checks of IRIs and literals) are not in it. For tests,
//! `check` says whether a tree's constructs fill their signatures.

use super::tree::Keyword;
#[cfg(test)]
use super::tree::{Document, NodeKind};

/// What an argument slot takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cat {
    /// `Annotation( … )`.
    Annotation,
    /// An IRI: a class, datatype, property or individual name, a facet.
    Iri,
    /// A data property, as the grammar names `DataPropertyExpression`.
    DataProperty,
    ClassExpression,
    ObjectProperty,
    /// `ObjectPropertyChain( … )` or an object property expression.
    SubObjectProperty,
    DataRange,
    Individual,
    Literal,
    /// An IRI or an anonymous individual.
    AnnotationSubject,
    /// An IRI, a literal or an anonymous individual.
    AnnotationValue,
    Integer,
    Entity,
    /// `( {ObjectPropertyExpression} )` of `HasKey`.
    ObjectKeys,
    /// `( {DataPropertyExpression} )` of `HasKey`.
    DataKeys,
    /// `DataPropertyExpression { DataPropertyExpression } DataRange`.
    DataPropertiesThenRange,
    /// `constrainingFacet restrictionValue { … }`.
    Facets,
    Body,
    Head,
    Atom,
    /// A SWRL individual argument: a variable or an individual.
    IArg,
    /// A SWRL data argument: a variable or a literal.
    DArg,
}

impl Cat {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Cat::Annotation => "an annotation",
            Cat::Iri => "an IRI",
            Cat::DataProperty => "a data property",
            Cat::ClassExpression => "a class expression",
            Cat::ObjectProperty => "an object property expression",
            Cat::SubObjectProperty => "an object property expression or 'ObjectPropertyChain'",
            Cat::DataRange => "a data range",
            Cat::Individual => "an individual",
            Cat::Literal => "a literal",
            Cat::AnnotationSubject => "an IRI or an anonymous individual",
            Cat::AnnotationValue => "an IRI, a literal or an anonymous individual",
            Cat::Integer => "a non-negative integer",
            Cat::Entity => {
                "an entity (Class, Datatype, ObjectProperty, DataProperty, AnnotationProperty or NamedIndividual)"
            }
            Cat::ObjectKeys => "'(' and the key's object property expressions",
            Cat::DataKeys => "'(' and the key's data properties",
            Cat::DataPropertiesThenRange => "a data property",
            Cat::Facets => "a facet",
            Cat::Body => "'Body'",
            Cat::Head => "'Head'",
            Cat::Atom => "an atom",
            Cat::IArg => "an individual or a variable",
            Cat::DArg => "a literal or a variable",
        }
    }

    /// What `n` of them are called, after "at least".
    pub(crate) fn counted(self, n: u8) -> String {
        let (one, many) = match self {
            Cat::ClassExpression => ("class expression", "class expressions"),
            Cat::ObjectProperty => ("object property expression", "object property expressions"),
            Cat::DataProperty | Cat::DataPropertiesThenRange => {
                ("data property", "data properties")
            }
            Cat::DataRange => ("data range", "data ranges"),
            Cat::Individual => ("individual", "individuals"),
            Cat::Literal => ("literal", "literals"),
            _ => ("argument", "arguments"),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    }
}

/// How many times a slot is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rep {
    One,
    Optional,
    AtLeast(u8),
}

pub(crate) type Slot = (Cat, Rep);

const ANNOTATIONS: Slot = (Cat::Annotation, Rep::AtLeast(0));
const IRI: Slot = (Cat::Iri, Rep::One);
const DP: Slot = (Cat::DataProperty, Rep::One);
const CE: Slot = (Cat::ClassExpression, Rep::One);
const OPE: Slot = (Cat::ObjectProperty, Rep::One);
const DR: Slot = (Cat::DataRange, Rep::One);
const IND: Slot = (Cat::Individual, Rep::One);
const LIT: Slot = (Cat::Literal, Rep::One);
const INT: Slot = (Cat::Integer, Rep::One);
const IARG: Slot = (Cat::IArg, Rep::One);
const DARG: Slot = (Cat::DArg, Rep::One);

/// The argument slots of `k`, in order.
pub(crate) fn signature(k: Keyword) -> &'static [Slot] {
    use Keyword as K;
    match k {
        // Prefix and Ontology have their own productions.
        K::Prefix | K::Ontology => &[],
        K::Import => &[IRI],
        K::Annotation => &[ANNOTATIONS, IRI, (Cat::AnnotationValue, Rep::One)],
        K::Class
        | K::Datatype
        | K::ObjectProperty
        | K::DataProperty
        | K::AnnotationProperty
        | K::NamedIndividual
        | K::ObjectInverseOf
        | K::Variable => &[IRI],
        K::ObjectPropertyChain => &[(Cat::ObjectProperty, Rep::AtLeast(2))],
        K::DataIntersectionOf | K::DataUnionOf => &[(Cat::DataRange, Rep::AtLeast(2))],
        K::DataComplementOf => &[DR],
        K::DataOneOf => &[(Cat::Literal, Rep::AtLeast(1))],
        K::DatatypeRestriction => &[IRI, (Cat::Facets, Rep::One)],
        K::ObjectIntersectionOf | K::ObjectUnionOf => &[(Cat::ClassExpression, Rep::AtLeast(2))],
        K::ObjectComplementOf => &[CE],
        K::ObjectOneOf => &[(Cat::Individual, Rep::AtLeast(1))],
        K::ObjectSomeValuesFrom | K::ObjectAllValuesFrom => &[OPE, CE],
        K::ObjectHasValue => &[OPE, IND],
        K::ObjectHasSelf => &[OPE],
        K::ObjectMinCardinality | K::ObjectMaxCardinality | K::ObjectExactCardinality => {
            &[INT, OPE, (Cat::ClassExpression, Rep::Optional)]
        }
        K::DataSomeValuesFrom | K::DataAllValuesFrom => &[(Cat::DataPropertiesThenRange, Rep::One)],
        K::DataHasValue => &[DP, LIT],
        K::DataMinCardinality | K::DataMaxCardinality | K::DataExactCardinality => {
            &[INT, DP, (Cat::DataRange, Rep::Optional)]
        }
        K::Declaration => &[ANNOTATIONS, (Cat::Entity, Rep::One)],
        K::SubClassOf => &[ANNOTATIONS, CE, CE],
        K::EquivalentClasses | K::DisjointClasses => {
            &[ANNOTATIONS, (Cat::ClassExpression, Rep::AtLeast(2))]
        }
        K::DisjointUnion => &[ANNOTATIONS, IRI, (Cat::ClassExpression, Rep::AtLeast(2))],
        K::SubObjectPropertyOf => &[ANNOTATIONS, (Cat::SubObjectProperty, Rep::One), OPE],
        K::EquivalentObjectProperties | K::DisjointObjectProperties => {
            &[ANNOTATIONS, (Cat::ObjectProperty, Rep::AtLeast(2))]
        }
        K::InverseObjectProperties => &[ANNOTATIONS, OPE, OPE],
        K::ObjectPropertyDomain | K::ObjectPropertyRange => &[ANNOTATIONS, OPE, CE],
        K::FunctionalObjectProperty
        | K::InverseFunctionalObjectProperty
        | K::ReflexiveObjectProperty
        | K::IrreflexiveObjectProperty
        | K::SymmetricObjectProperty
        | K::AsymmetricObjectProperty
        | K::TransitiveObjectProperty => &[ANNOTATIONS, OPE],
        K::SubDataPropertyOf => &[ANNOTATIONS, DP, DP],
        K::EquivalentDataProperties | K::DisjointDataProperties => {
            &[ANNOTATIONS, (Cat::DataProperty, Rep::AtLeast(2))]
        }
        K::DataPropertyDomain => &[ANNOTATIONS, DP, CE],
        K::DataPropertyRange => &[ANNOTATIONS, DP, DR],
        K::FunctionalDataProperty => &[ANNOTATIONS, DP],
        K::DatatypeDefinition => &[ANNOTATIONS, IRI, DR],
        K::HasKey => &[
            ANNOTATIONS,
            CE,
            (Cat::ObjectKeys, Rep::One),
            (Cat::DataKeys, Rep::One),
        ],
        K::SameIndividual | K::DifferentIndividuals => {
            &[ANNOTATIONS, (Cat::Individual, Rep::AtLeast(2))]
        }
        K::ClassAssertion => &[ANNOTATIONS, CE, IND],
        K::ObjectPropertyAssertion | K::NegativeObjectPropertyAssertion => {
            &[ANNOTATIONS, OPE, IND, IND]
        }
        K::DataPropertyAssertion | K::NegativeDataPropertyAssertion => &[ANNOTATIONS, DP, IND, LIT],
        K::AnnotationAssertion => &[
            ANNOTATIONS,
            IRI,
            (Cat::AnnotationSubject, Rep::One),
            (Cat::AnnotationValue, Rep::One),
        ],
        K::SubAnnotationPropertyOf | K::AnnotationPropertyDomain | K::AnnotationPropertyRange => {
            &[ANNOTATIONS, IRI, IRI]
        }
        K::DLSafeRule => &[ANNOTATIONS, (Cat::Body, Rep::One), (Cat::Head, Rep::One)],
        K::Body | K::Head => &[(Cat::Atom, Rep::AtLeast(0))],
        K::ClassAtom => &[CE, IARG],
        K::DataRangeAtom => &[DR, DARG],
        K::ObjectPropertyAtom => &[OPE, IARG, IARG],
        K::DataPropertyAtom => &[DP, IARG, DARG],
        K::BuiltInAtom => &[IRI, (Cat::DArg, Rep::AtLeast(1))],
        K::SameIndividualAtom | K::DifferentIndividualsAtom => &[IARG, IARG],
    }
}

/// Whether `k` starts an axiom: what the parse resumes at after an error.
pub(crate) fn is_axiom(k: Keyword) -> bool {
    use Keyword as K;
    matches!(
        k,
        K::Declaration
            | K::SubClassOf
            | K::EquivalentClasses
            | K::DisjointClasses
            | K::DisjointUnion
            | K::SubObjectPropertyOf
            | K::EquivalentObjectProperties
            | K::DisjointObjectProperties
            | K::InverseObjectProperties
            | K::ObjectPropertyDomain
            | K::ObjectPropertyRange
            | K::FunctionalObjectProperty
            | K::InverseFunctionalObjectProperty
            | K::ReflexiveObjectProperty
            | K::IrreflexiveObjectProperty
            | K::SymmetricObjectProperty
            | K::AsymmetricObjectProperty
            | K::TransitiveObjectProperty
            | K::SubDataPropertyOf
            | K::EquivalentDataProperties
            | K::DisjointDataProperties
            | K::DataPropertyDomain
            | K::DataPropertyRange
            | K::FunctionalDataProperty
            | K::DatatypeDefinition
            | K::HasKey
            | K::SameIndividual
            | K::DifferentIndividuals
            | K::ClassAssertion
            | K::ObjectPropertyAssertion
            | K::NegativeObjectPropertyAssertion
            | K::DataPropertyAssertion
            | K::NegativeDataPropertyAssertion
            | K::AnnotationAssertion
            | K::SubAnnotationPropertyOf
            | K::AnnotationPropertyDomain
            | K::AnnotationPropertyRange
            | K::DLSafeRule
    )
}

/// Whether the construct `k` is one of `cat`.
pub(crate) fn keyword_in(k: Keyword, cat: Cat) -> bool {
    use Keyword as K;
    match cat {
        Cat::Annotation => k == K::Annotation,
        Cat::ClassExpression => matches!(
            k,
            K::ObjectIntersectionOf
                | K::ObjectUnionOf
                | K::ObjectComplementOf
                | K::ObjectOneOf
                | K::ObjectSomeValuesFrom
                | K::ObjectAllValuesFrom
                | K::ObjectHasValue
                | K::ObjectHasSelf
                | K::ObjectMinCardinality
                | K::ObjectMaxCardinality
                | K::ObjectExactCardinality
                | K::DataSomeValuesFrom
                | K::DataAllValuesFrom
                | K::DataHasValue
                | K::DataMinCardinality
                | K::DataMaxCardinality
                | K::DataExactCardinality
        ),
        Cat::ObjectProperty => k == K::ObjectInverseOf,
        Cat::SubObjectProperty => matches!(k, K::ObjectInverseOf | K::ObjectPropertyChain),
        Cat::DataRange | Cat::DataPropertiesThenRange => matches!(
            k,
            K::DataIntersectionOf
                | K::DataUnionOf
                | K::DataComplementOf
                | K::DataOneOf
                | K::DatatypeRestriction
        ),
        Cat::Entity => matches!(
            k,
            K::Class
                | K::Datatype
                | K::ObjectProperty
                | K::DataProperty
                | K::AnnotationProperty
                | K::NamedIndividual
        ),
        Cat::Body => k == K::Body,
        Cat::Head => k == K::Head,
        Cat::Atom => matches!(
            k,
            K::ClassAtom
                | K::DataRangeAtom
                | K::ObjectPropertyAtom
                | K::DataPropertyAtom
                | K::BuiltInAtom
                | K::SameIndividualAtom
                | K::DifferentIndividualsAtom
        ),
        Cat::IArg | Cat::DArg => k == K::Variable,
        Cat::Iri
        | Cat::DataProperty
        | Cat::Individual
        | Cat::Literal
        | Cat::AnnotationSubject
        | Cat::AnnotationValue
        | Cat::Integer
        | Cat::ObjectKeys
        | Cat::DataKeys
        | Cat::Facets => false,
    }
}

/// The first node of a tree that does not fill its signature.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Misfit {
    /// Its index in [`Document::nodes`].
    pub(crate) node: usize,
    /// What it lacks or holds that it should not.
    pub(crate) why: String,
}

/// Whether every top-level node of `doc` not in [`Document::broken`], and
/// every node below it, fills its signature: at the top level, the nodes of
/// the ontology IRI and the version IRI are IRI nodes and every other is an
/// `Import`, an ontology annotation or an axiom; a construct's children
/// fill its slots in order, of the kinds and counts the table gives (with
/// the shapes of the module doc); a literal has no child or one IRI node;
/// a term has no child; and each node's subtree ends past it and within
/// its parent's. Else the first such node in preorder, that is, the
/// outermost and then the first in text order.
#[cfg(test)]
pub(crate) fn check(doc: &Document) -> Result<(), Misfit> {
    let broken: rustc_hash::FxHashSet<u32> = doc.broken.iter().copied().collect();
    let top = children(doc, None, 0, doc.nodes.len())?;
    for i in top {
        if broken.contains(&(i as u32)) {
            continue;
        }
        let header = doc.ontology_iri == Some(i) || doc.version_iri == Some(i);
        match doc.nodes[i].kind {
            NodeKind::Iri { .. } if header => node(doc, i)?,
            NodeKind::Construct(k)
                if !header
                    && (is_axiom(k) || matches!(k, Keyword::Import | Keyword::Annotation)) =>
            {
                node(doc, i)?
            }
            _ => {
                let what = if header {
                    "the ontology IRI or the version IRI, an IRI"
                } else {
                    "an import, an ontology annotation or an axiom"
                };
                return misfit(i, format!("Expected {what}, found {}", found(doc, i)));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn misfit(node: usize, why: String) -> Result<(), Misfit> {
    Err(Misfit { node, why })
}

/// How node `i` is named in a misfit.
#[cfg(test)]
fn found(doc: &Document, i: usize) -> String {
    match doc.nodes[i].kind {
        NodeKind::Construct(k) => format!("'{}'", k.as_str()),
        NodeKind::Group => "a key list".into(),
        NodeKind::Iri { .. } => "an IRI".into(),
        NodeKind::NodeId => "an anonymous individual".into(),
        NodeKind::Literal { .. } => "a literal".into(),
        NodeKind::Integer => "an integer".into(),
    }
}

/// The nodes from `from` to `end` as siblings, each ending past itself and
/// by `end`; `parent` is named if one does not.
#[cfg(test)]
fn children(
    doc: &Document,
    parent: Option<usize>,
    from: usize,
    end: usize,
) -> Result<Vec<usize>, Misfit> {
    let mut out = Vec::new();
    let mut at = from;
    while at < end {
        let next = doc.nodes[at].next as usize;
        if next <= at || next > end {
            let why = format!("Node {at} ends at {next}, not past itself and by {end}");
            return Err(Misfit {
                node: parent.unwrap_or(at),
                why,
            });
        }
        out.push(at);
        at = next;
    }
    Ok(out)
}

/// Whether node `i` and its subtree fill their signatures.
#[cfg(test)]
fn node(doc: &Document, i: usize) -> Result<(), Misfit> {
    let end = doc.nodes[i].next as usize;
    let kids = children(doc, Some(i), i + 1, end)?;
    match doc.nodes[i].kind {
        NodeKind::Construct(k) => fill(doc, i, k, &kids)?,
        NodeKind::Literal { .. } => match kids[..] {
            [] => {}
            [d] if matches!(doc.nodes[d].kind, NodeKind::Iri { .. }) => {}
            _ => return misfit(i, format!("A literal holds {}", found(doc, kids[0]))),
        },
        // Its children are checked against the key's category by `HasKey`.
        NodeKind::Group => {}
        NodeKind::Iri { .. } | NodeKind::NodeId | NodeKind::Integer => {
            if let Some(&c) = kids.first() {
                return misfit(i, format!("{} holds {}", found(doc, i), found(doc, c)));
            }
        }
    }
    kids.iter().try_for_each(|&c| node(doc, c))
}

/// Whether `kids`, the children of the construct `k` at `i`, fill its
/// slots.
#[cfg(test)]
fn fill(doc: &Document, i: usize, k: Keyword, kids: &[usize]) -> Result<(), Misfit> {
    let name = k.as_str();
    let mut rest = kids;
    for &(cat, rep) in signature(k) {
        let mut taken = 0usize;
        loop {
            if !matches!(rep, Rep::AtLeast(_)) && taken == 1 {
                break;
            }
            match take(doc, cat, rest) {
                Some(after) => {
                    rest = after;
                    taken += 1;
                }
                None => break,
            }
        }
        match rep {
            Rep::One if taken == 0 => {
                let what = rest.first().map_or("nothing".into(), |&c| found(doc, c));
                return misfit(i, format!("'{name}' needs {}, found {what}", cat.name()));
            }
            Rep::AtLeast(n) if taken < n as usize => {
                let message = format!("'{name}' needs at least {}, found {taken}", cat.counted(n));
                return misfit(i, message);
            }
            _ => {}
        }
    }
    match rest.first() {
        None => Ok(()),
        Some(&c) => misfit(
            i,
            format!("'{name}' takes no more, found {}", found(doc, c)),
        ),
    }
}

/// What is left of `nodes` after one of `cat` at its start, if one is.
#[cfg(test)]
fn take<'n>(doc: &Document, cat: Cat, nodes: &'n [usize]) -> Option<&'n [usize]> {
    let is_iri = |c: usize| matches!(doc.nodes[c].kind, NodeKind::Iri { .. });
    match cat {
        Cat::ObjectKeys | Cat::DataKeys => {
            let (&g, rest) = nodes.split_first()?;
            let each = if cat == Cat::ObjectKeys {
                Cat::ObjectProperty
            } else {
                Cat::DataProperty
            };
            if doc.nodes[g].kind != NodeKind::Group {
                return None;
            }
            let keys = children(doc, Some(g), g + 1, doc.nodes[g].next as usize).ok()?;
            keys.iter().all(|&c| one(doc, c, each)).then_some(rest)
        }
        Cat::DataPropertiesThenRange => {
            let n = nodes
                .iter()
                .take_while(|&&c| one(doc, c, Cat::DataRange))
                .count();
            let fits = n >= 2 && nodes[..n - 1].iter().all(|&c| is_iri(c));
            fits.then(|| &nodes[n..])
        }
        Cat::Facets => {
            let mut rest = nodes;
            while let [f, v, after @ ..] = rest
                && is_iri(*f)
                && matches!(doc.nodes[*v].kind, NodeKind::Literal { .. })
            {
                rest = after;
            }
            (rest.len() < nodes.len()).then_some(rest)
        }
        _ => {
            let (&c, rest) = nodes.split_first()?;
            one(doc, c, cat).then_some(rest)
        }
    }
}

/// Whether node `c` alone is one of `cat`.
#[cfg(test)]
fn one(doc: &Document, c: usize, cat: Cat) -> bool {
    let kind = doc.nodes[c].kind;
    let iri = matches!(kind, NodeKind::Iri { .. });
    let literal = matches!(kind, NodeKind::Literal { .. });
    if let NodeKind::Construct(k) = kind {
        return keyword_in(k, cat);
    }
    match cat {
        Cat::Iri
        | Cat::DataProperty
        | Cat::ClassExpression
        | Cat::ObjectProperty
        | Cat::SubObjectProperty
        | Cat::DataRange => iri,
        Cat::Individual | Cat::AnnotationSubject | Cat::IArg => iri || kind == NodeKind::NodeId,
        Cat::AnnotationValue => iri || literal || kind == NodeKind::NodeId,
        Cat::Literal | Cat::DArg => literal,
        Cat::Integer => kind == NodeKind::Integer,
        // A node alone is none of these: `take` reads their shapes.
        Cat::ObjectKeys | Cat::DataKeys | Cat::DataPropertiesThenRange | Cat::Facets => false,
        Cat::Annotation | Cat::Entity | Cat::Body | Cat::Head | Cat::Atom => false,
    }
}
