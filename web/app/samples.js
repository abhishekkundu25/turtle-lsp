// Sample inputs. Each set shows the tools on data that suits them; the
// problems in them are deliberate.

const ONTOLOGY = `@prefix ex: <https://toolbox.example.org/def/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix skos: <http://www.w3.org/2004/02/skos/core#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:Person a owl:Class ;
    rdfs:labl "Person" .

ex:Organisation a owl:Class ;
    rdfs:label "Organisation" .

ex:name a owl:DatatypeProperty ;
    rdfs:label "name" ;
    rdfs:domain ex:Person ;
    rdfs:range xsd:string .

ex:worksFor a owl:ObjectProperty ;
    rdfs:label "works for" ;
    rdfs:domain ex:Person ;
    rdfs:range ex:Organisation .
`

const SHAPES = `@prefix ex: <https://toolbox.example.org/def/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [
        sh:path ex:name ;
        sh:minCount 1 ;
        sh:datatype xsd:string ;
        sh:message "Every person needs a name"
    ] ;
    sh:property [
        sh:path ex:worksFor ;
        sh:class ex:Organisation ;
        sh:message "A person works for an organisation"
    ] .
`

const DATA = `@prefix ex: <https://toolbox.example.org/def/> .
@prefix id: <https://toolbox.example.org/id/> .

id:alice a ex:Person ; ex:name "Alice" ;
  ex:worksFor id:acme .
id:acme a ex:Organisation ;   rdfs:label "ACME" .

id:bob a ex:Person ;
    ex:worksFor id:alice .
`

const QUERY = `PREFIX ex: <https://toolbox.example.org/def/>
PREFIX id: <https://toolbox.example.org/id/>

select ?name ?org where { ?p a ex:Person ; ex:name ?name .
  ?p foaf:knows ?friend }
`

const UPDATE = `PREFIX ex: <https://toolbox.example.org/def/>
PREFIX id: <https://toolbox.example.org/id/>
insert data { id:carol a ex:Person ; ex:name "Carol" ; ex:worksFor id:acme }
`

const GRAPHS = `@prefix ex: <https://toolbox.example.org/def/> .
@prefix id: <https://toolbox.example.org/id/> .
@prefix g: <https://toolbox.example.org/graph/> .

g:2025 { id:dave a ex:Person ; ex:name "Dave" ; ex:worksFor id:acme . }
g:2026 {
id:dave a ex:Person ;ex:name "Dave" .
}
`

const QUADS = `<https://toolbox.example.org/id/erin> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <https://toolbox.example.org/def/Person>   <https://toolbox.example.org/graph/hr> .
<https://toolbox.example.org/id/erin>   <https://toolbox.example.org/def/name> "Erin" <https://toolbox.example.org/graph/hr> .
`

export const SETS = {
  starter: {
    label: "Ontology, shapes, data and a query",
    files: [
      { name: "ontology.ttl", text: ONTOLOGY },
      { name: "shapes.ttl", text: SHAPES },
      { name: "data.ttl", text: DATA },
      { name: "people.rq", text: QUERY },
    ],
  },
  datasets: {
    label: "Datasets (TriG and N-Quads)",
    files: [
      { name: "ontology.ttl", text: ONTOLOGY },
      { name: "graphs.trig", text: GRAPHS },
      { name: "hr.nq", text: QUADS },
    ],
  },
  queries: {
    label: "Queries only",
    files: [
      { name: "people.rq", text: QUERY },
      { name: "add-carol.ru", text: UPDATE },
    ],
  },
}
