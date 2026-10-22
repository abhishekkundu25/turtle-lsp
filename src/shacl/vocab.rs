//! The SHACL, RDF, RDFS and OWL IRIs that the SHACL code compares against,
//! interned once: the shape model ([`super::model`]), the shape lint and its
//! SHACL-SPARQL part, and the validator.

use crate::syntax::{Iri, RDF};
use std::sync::OnceLock;

pub struct Vocab {
    pub rdf_type: Iri,
    pub rdf_first: Iri,
    pub rdf_rest: Iri,
    pub rdf_nil: Iri,
    pub rdfs_class: Iri,
    pub rdfs_datatype: Iri,
    pub sub_class_of: Iri,
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
}

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
        }
    })
}

impl Vocab {
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
