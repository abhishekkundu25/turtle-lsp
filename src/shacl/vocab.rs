//! The SHACL, RDF, RDFS and OWL IRIs that the SHACL code compares against,
//! interned once: the shape model ([`super::model`]), the shape lint and its
//! SHACL-SPARQL part, the validator ([`super::validate`]) and the completion
//! index's shapes and schema links ([`crate::knowledge`]), with the SHACL
//! Core constraint parameters the validator compiles ([`Param`]).

use crate::syntax::{Iri, RDF, XSD};
use rustc_hash::FxHashMap;
use std::sync::OnceLock;

pub(crate) struct Vocab {
    pub(crate) rdf_type: Iri,
    pub(crate) rdf_first: Iri,
    pub(crate) rdf_rest: Iri,
    pub(crate) rdf_nil: Iri,
    pub(crate) rdfs_class: Iri,
    pub(crate) rdfs_datatype: Iri,
    pub(crate) sub_class_of: Iri,
    pub(crate) sub_property_of: Iri,
    pub(crate) rdfs_domain: Iri,
    pub(crate) rdfs_range: Iri,
    pub(crate) owl_class: Iri,
    pub(crate) owl_imports: Iri,

    pub(crate) node_shape: Iri,
    pub(crate) property_shape: Iri,
    pub(crate) target_class: Iri,
    pub(crate) target_node: Iri,
    pub(crate) target_subjects_of: Iri,
    pub(crate) target_objects_of: Iri,
    pub(crate) path: Iri,
    pub(crate) inverse_path: Iri,
    pub(crate) alternative_path: Iri,
    pub(crate) zero_or_more_path: Iri,
    pub(crate) one_or_more_path: Iri,
    pub(crate) zero_or_one_path: Iri,

    pub(crate) property: Iri,
    pub(crate) node: Iri,
    pub(crate) qualified_value_shape: Iri,
    pub(crate) not: Iri,
    pub(crate) and: Iri,
    pub(crate) or: Iri,
    pub(crate) xone: Iri,
    pub(crate) min_count: Iri,
    pub(crate) max_count: Iri,
    pub(crate) qualified_min_count: Iri,
    pub(crate) qualified_max_count: Iri,
    pub(crate) qualified_disjoint: Iri,
    pub(crate) class: Iri,
    pub(crate) datatype: Iri,
    pub(crate) in_list: Iri,
    pub(crate) unique_lang: Iri,
    pub(crate) less_than: Iri,
    pub(crate) less_than_or_equals: Iri,
    pub(crate) flags: Iri,
    pub(crate) ignored_properties: Iri,

    pub(crate) deactivated: Iri,
    pub(crate) severity: Iri,
    pub(crate) violation: Iri,
    pub(crate) message: Iri,

    pub(crate) target: Iri,
    pub(crate) sparql: Iri,
    pub(crate) select: Iri,
    pub(crate) ask: Iri,
    pub(crate) prefixes: Iri,
    pub(crate) declare: Iri,
    pub(crate) prefix: Iri,
    pub(crate) namespace: Iri,
    pub(crate) constraint_component: Iri,
    pub(crate) parameter: Iri,
    pub(crate) optional: Iri,
    pub(crate) validator: Iri,
    pub(crate) node_validator: Iri,
    pub(crate) property_validator: Iri,
    pub(crate) ask_validator: Iri,
    pub(crate) select_validator: Iri,
    pub(crate) sparql_target: Iri,
    pub(crate) sparql_target_type: Iri,

    pub(crate) xsd_boolean: Iri,

    params: FxHashMap<Iri, Param>,
}

/// A SHACL Core constraint parameter that makes a constraint on its own
/// (`sh:qualifiedValueShape` and its counts go together, and are left out).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Param {
    Class,
    Datatype,
    NodeKind,
    MinCount,
    MaxCount,
    MinExclusive,
    MinInclusive,
    MaxExclusive,
    MaxInclusive,
    MinLength,
    MaxLength,
    Pattern,
    LanguageIn,
    UniqueLang,
    Equals,
    Disjoint,
    LessThan,
    LessThanOrEquals,
    Not,
    And,
    Or,
    Xone,
    Node,
    Property,
    Closed,
    HasValue,
    In,
}

const PARAMS: [(&str, Param); 27] = [
    ("class", Param::Class),
    ("datatype", Param::Datatype),
    ("nodeKind", Param::NodeKind),
    ("minCount", Param::MinCount),
    ("maxCount", Param::MaxCount),
    ("minExclusive", Param::MinExclusive),
    ("minInclusive", Param::MinInclusive),
    ("maxExclusive", Param::MaxExclusive),
    ("maxInclusive", Param::MaxInclusive),
    ("minLength", Param::MinLength),
    ("maxLength", Param::MaxLength),
    ("pattern", Param::Pattern),
    ("languageIn", Param::LanguageIn),
    ("uniqueLang", Param::UniqueLang),
    ("equals", Param::Equals),
    ("disjoint", Param::Disjoint),
    ("lessThan", Param::LessThan),
    ("lessThanOrEquals", Param::LessThanOrEquals),
    ("not", Param::Not),
    ("and", Param::And),
    ("or", Param::Or),
    ("xone", Param::Xone),
    ("node", Param::Node),
    ("property", Param::Property),
    ("closed", Param::Closed),
    ("hasValue", Param::HasValue),
    ("in", Param::In),
];

/// The table, interned on first use.
pub(crate) fn get() -> &'static Vocab {
    static V: OnceLock<Vocab> = OnceLock::new();
    V.get_or_init(|| {
        let sh = |l: &str| Iri::intern(&format!("{}{l}", super::SH));
        let rdf = |l: &str| Iri::intern(&format!("{RDF}{l}"));
        let rdfs = |l: &str| Iri::intern(&format!("http://www.w3.org/2000/01/rdf-schema#{l}"));
        let owl = |l: &str| Iri::intern(&format!("http://www.w3.org/2002/07/owl#{l}"));
        Vocab {
            rdf_type: rdf("type"),
            rdf_first: rdf("first"),
            rdf_rest: rdf("rest"),
            rdf_nil: rdf("nil"),
            rdfs_class: rdfs("Class"),
            rdfs_datatype: rdfs("Datatype"),
            sub_class_of: rdfs("subClassOf"),
            sub_property_of: rdfs("subPropertyOf"),
            rdfs_domain: rdfs("domain"),
            rdfs_range: rdfs("range"),
            owl_class: owl("Class"),
            owl_imports: owl("imports"),

            node_shape: sh("NodeShape"),
            property_shape: sh("PropertyShape"),
            target_class: sh("targetClass"),
            target_node: sh("targetNode"),
            target_subjects_of: sh("targetSubjectsOf"),
            target_objects_of: sh("targetObjectsOf"),
            path: sh("path"),
            inverse_path: sh("inversePath"),
            alternative_path: sh("alternativePath"),
            zero_or_more_path: sh("zeroOrMorePath"),
            one_or_more_path: sh("oneOrMorePath"),
            zero_or_one_path: sh("zeroOrOnePath"),

            property: sh("property"),
            node: sh("node"),
            qualified_value_shape: sh("qualifiedValueShape"),
            not: sh("not"),
            and: sh("and"),
            or: sh("or"),
            xone: sh("xone"),
            min_count: sh("minCount"),
            max_count: sh("maxCount"),
            qualified_min_count: sh("qualifiedMinCount"),
            qualified_max_count: sh("qualifiedMaxCount"),
            qualified_disjoint: sh("qualifiedValueShapesDisjoint"),
            class: sh("class"),
            datatype: sh("datatype"),
            in_list: sh("in"),
            unique_lang: sh("uniqueLang"),
            less_than: sh("lessThan"),
            less_than_or_equals: sh("lessThanOrEquals"),
            flags: sh("flags"),
            ignored_properties: sh("ignoredProperties"),

            deactivated: sh("deactivated"),
            severity: sh("severity"),
            violation: sh("Violation"),
            message: sh("message"),

            target: sh("target"),
            sparql: sh("sparql"),
            select: sh("select"),
            ask: sh("ask"),
            prefixes: sh("prefixes"),
            declare: sh("declare"),
            prefix: sh("prefix"),
            namespace: sh("namespace"),
            constraint_component: sh("ConstraintComponent"),
            parameter: sh("parameter"),
            optional: sh("optional"),
            validator: sh("validator"),
            node_validator: sh("nodeValidator"),
            property_validator: sh("propertyValidator"),
            ask_validator: sh("SPARQLAskValidator"),
            select_validator: sh("SPARQLSelectValidator"),
            sparql_target: sh("SPARQLTarget"),
            sparql_target_type: sh("SPARQLTargetType"),

            xsd_boolean: Iri::intern(&format!("{XSD}boolean")),

            params: PARAMS.iter().map(|&(l, p)| (sh(l), p)).collect(),
        }
    })
}

impl Vocab {
    /// The Core constraint parameter `p` is, if any.
    pub(crate) fn param(&self, p: Iri) -> Option<Param> {
        self.params.get(&p).copied()
    }

    /// `sh:targetClass`, `sh:targetNode`, `sh:targetSubjectsOf` or
    /// `sh:targetObjectsOf`.
    pub(crate) fn is_target(&self, p: Iri) -> bool {
        p == self.target_class
            || p == self.target_node
            || p == self.target_subjects_of
            || p == self.target_objects_of
    }

    /// `sh:validator`, `sh:nodeValidator` or `sh:propertyValidator`.
    pub(crate) fn is_validator(&self, p: Iri) -> bool {
        p == self.validator || p == self.node_validator || p == self.property_validator
    }
}
