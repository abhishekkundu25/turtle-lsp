//! The SHACL, RDF, RDFS and OWL IRIs that the SHACL code compares against,
//! interned once: the shape model ([`super::model`]), the shape lint and its
//! SHACL-SPARQL part, the validator ([`super::validate`]) and the completion
//! index's shapes and schema links ([`crate::knowledge`]), with the SHACL
//! Core constraint parameters the validator compiles ([`Param`]).

use crate::syntax::{Iri, RDF, XSD};
use rustc_hash::FxHashMap;
use std::sync::OnceLock;

pub struct Vocab {
    pub rdf_type: Iri,
    pub rdf_first: Iri,
    pub rdf_rest: Iri,
    pub rdf_nil: Iri,
    pub rdfs_class: Iri,
    pub rdfs_datatype: Iri,
    pub sub_class_of: Iri,
    pub sub_property_of: Iri,
    pub rdfs_domain: Iri,
    pub rdfs_range: Iri,
    pub owl_class: Iri,
    pub owl_imports: Iri,

    pub node_shape: Iri,
    pub property_shape: Iri,
    pub target_class: Iri,
    pub target_node: Iri,
    pub target_subjects_of: Iri,
    pub target_objects_of: Iri,
    pub path: Iri,
    pub inverse_path: Iri,
    pub alternative_path: Iri,
    pub zero_or_more_path: Iri,
    pub one_or_more_path: Iri,
    pub zero_or_one_path: Iri,

    pub property: Iri,
    pub node: Iri,
    pub qualified_value_shape: Iri,
    pub not: Iri,
    pub and: Iri,
    pub or: Iri,
    pub xone: Iri,
    pub min_count: Iri,
    pub max_count: Iri,
    pub qualified_min_count: Iri,
    pub qualified_max_count: Iri,
    pub qualified_disjoint: Iri,
    pub class: Iri,
    pub datatype: Iri,
    pub in_list: Iri,
    pub unique_lang: Iri,
    pub less_than: Iri,
    pub less_than_or_equals: Iri,
    pub flags: Iri,
    pub ignored_properties: Iri,

    pub deactivated: Iri,
    pub severity: Iri,
    pub violation: Iri,
    pub message: Iri,

    pub target: Iri,
    pub sparql: Iri,
    pub select: Iri,
    pub ask: Iri,
    pub prefixes: Iri,
    pub declare: Iri,
    pub prefix: Iri,
    pub namespace: Iri,
    pub constraint_component: Iri,
    pub parameter: Iri,
    pub optional: Iri,
    pub validator: Iri,
    pub node_validator: Iri,
    pub property_validator: Iri,
    pub ask_validator: Iri,
    pub select_validator: Iri,
    pub sparql_target: Iri,
    pub sparql_target_type: Iri,

    pub xsd_boolean: Iri,

    params: FxHashMap<Iri, Param>,
}

/// A SHACL Core constraint parameter that makes a constraint on its own
/// (`sh:qualifiedValueShape` and its counts go together, and are left out).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Param {
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
pub fn get() -> &'static Vocab {
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
    pub fn param(&self, p: Iri) -> Option<Param> {
        self.params.get(&p).copied()
    }

    /// `sh:targetClass`, `sh:targetNode`, `sh:targetSubjectsOf` or
    /// `sh:targetObjectsOf`.
    pub fn is_target(&self, p: Iri) -> bool {
        p == self.target_class
            || p == self.target_node
            || p == self.target_subjects_of
            || p == self.target_objects_of
    }

    /// `sh:validator`, `sh:nodeValidator` or `sh:propertyValidator`.
    pub fn is_validator(&self, p: Iri) -> bool {
        p == self.validator || p == self.node_validator || p == self.property_validator
    }
}
