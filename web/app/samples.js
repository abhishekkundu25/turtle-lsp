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

// JSON-LD: a person described with a context of short keys. JSON has no
// comments, so the guided workspace explains it in start-here.ttl.
const CAROL = `{
  "@context": {
    "ex": "https://toolbox.example.org/def/",
    "id": "https://toolbox.example.org/id/",
    "name": "ex:name"
  },
  "@id": "id:carol",
  "@type": "ex:Person",
  "name": "Carol"
}
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
    label: "Datasets (TriG, N-Quads and JSON-LD)",
    files: [
      { name: "ontology.ttl", text: ONTOLOGY },
      { name: "graphs.trig", text: GRAPHS },
      { name: "hr.nq", text: QUADS },
      { name: "carol.jsonld", text: CAROL },
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

const LABELS = `@prefix ex: <https://toolbox.example.org/def/> .
@prefix id: <https://toolbox.example.org/id/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

id:alice rdfs:label "Alice Smith"@en , "Alice Schmidt"@de .
id:acme rdfs:label "ACME Corporation"@en .
`

// The editor's starting workspace: the same files, in folders, each saying
// what to try in it, with a start file first. Only core.ttl and people.ttl
// keep deliberate problems.
const head = (lines) => lines.map((l) => `# ${l}`.trimEnd()).join("\n") + "\n\n"
const START = `${head([
  "Welcome to the RDF Editor. This is a small project: an ontology, SHACL",
  "shapes, data and SPARQL queries, checked as you type. Try these:",
  "",
  "1. Hover ex:Person below: its label, and the file that defines it.",
  "2. Ctrl+click (Cmd+click on a Mac) ex:Person to jump to its definition.",
  "3. Put the cursor on ex:worksFor and press F2 to rename it in every file.",
  "4. The problems in ontology/core.ttl and data/people.ttl are deliberate:",
  "   hover a squiggle, then press Ctrl+. (Cmd+.) for the fix.",
  "5. data/carol.jsonld is JSON-LD: hover its \"name\" key for the term it stands for.",
  "",
  "ex: and id: need no @prefix here: prefixes.ttl declares them for the",
  "whole workspace, as it does in VS Code.",
  "",
  "Your own files: import or drop them on the explorer, or open a folder.",
])}id:you a ex:Person ;
    ex:name "You" ;
    ex:worksFor id:acme .
`
const CORE = head(["The ontology. One deliberate mistake: a misspelt rdfs: term. Hover it, then Ctrl+. to fix it."]) + ONTOLOGY.replace("@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n", "")
const PEOPLE = head([
  "Data, checked against shapes/person.ttl. Deliberate mistakes: rdfs: is used",
  "without its prefix (Ctrl+. adds it), and Bob breaks two SHACL rules.",
]) + DATA
const PEOPLE_RQ = head(["A query. Completion (Ctrl+Space) knows the ontology's terms."]) +
  QUERY.replace("PREFIX id: <https://toolbox.example.org/id/>\n", "").replace("?name ?org", "?name").replace("\n  ?p foaf:knows ?friend }", " }")
const MOVE = `${head(["A SPARQL update: whoever works for a person works for ACME instead."])}PREFIX ex: <https://toolbox.example.org/def/>
PREFIX id: <https://toolbox.example.org/id/>

DELETE { ?p ex:worksFor id:alice }
INSERT { ?p ex:worksFor id:acme }
WHERE { ?p ex:worksFor id:alice }
`

const PREFIXES = `${head(["Prefixes for every file in this folder and below: no @prefix needed for them."])}@prefix ex: <https://toolbox.example.org/def/> .
@prefix id: <https://toolbox.example.org/id/> .
`

export const WORKSPACE = [
  { path: "start-here.ttl", text: START },
  { path: "prefixes.ttl", text: PREFIXES },
  { path: "ontology/core.ttl", text: CORE },
  { path: "ontology/labels.ttl", text: head(["Labels, shown next to terms with the Labels button (Alt+L)."]) + LABELS.replace("@prefix ex: <https://toolbox.example.org/def/> .\n", "") },
  { path: "shapes/person.ttl", text: head(["SHACL shapes: data/people.ttl is validated against them as you type."]) + SHAPES },
  { path: "data/people.ttl", text: PEOPLE },
  { path: "data/graphs.trig", text: head(["A dataset in TriG: one graph per year."]) + GRAPHS },
  { path: "data/hr.nq", text: head(["The same kind of data as N-Quads."]) + QUADS },
  { path: "data/carol.jsonld", text: CAROL },
  { path: "queries/people.rq", text: PEOPLE_RQ },
  { path: "queries/move-to-acme.ru", text: MOVE },
]
