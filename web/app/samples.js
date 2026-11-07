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
  "6. reasoning/ holds OWL reasoning examples: turn reasoning on in the settings",
  "   (Ctrl+, then Reasoning) and hover, as each file says.",
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

// Reasoning through TurbOWL (OWL 2 RL): long proofs and hidden
// contradictions. Off until the settings turn it on (Ctrl+, then Reasoning).
const REASONING_CHAIN = head(["Reasoning: settings (Ctrl+,) > Reasoning > Rules = default, then hover the terms named below."]) + `@prefix lab: <http://example.org/lab/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .

# One long chain of inferences, about 20 steps deep, with rules: default.
#
# Hover, in this order, to watch the chain grow:
#   lab:germany     -> lab:EURegion            (hasValue, equivalentClass)
#   lab:widget42    -> lab:EUProduct, lab:RegulatedGoods
#                      (domain, subclass, a chain built on another chain,
#                       someValuesFrom, intersection, union)
#   lab:screw9      -> lab:CertifiedPart       (allValuesFrom, over a part of
#                      a part: transitive hasPart)
#   lab:catalog77   -> lab:CertifiedPart, lab:Thing2Ship  (the deepest one:
#                      owl:sameAs found through an inverse-functional serial
#                      number, then everything above carried across it)

# ---- Where things are

lab:locatedIn a owl:TransitiveProperty .
lab:munich  lab:locatedIn lab:bavaria .
lab:bavaria lab:locatedIn lab:germany .
lab:germany lab:memberOf  lab:eu .

# A region is an EU region exactly when it is a member of the EU (cls-hv2,
# scm-eqc1 and cax-sco).
lab:EURegion owl:equivalentClass [
    a owl:Restriction ;
    owl:onProperty lab:memberOf ;
    owl:hasValue lab:eu
] .

# ---- Who makes what

lab:acme  lab:locatedIn lab:munich .
lab:alice lab:worksFor  lab:acme .

# Made by someone who works for an organisation (prp-spo2).
lab:builtByEmployeeOf owl:propertyChainAxiom ( lab:madeBy lab:worksFor ) .
# A chain over that chain: where the organisation is (prp-spo2 again, then
# prp-trp underneath it).
lab:suppliedFrom owl:propertyChainAxiom ( lab:builtByEmployeeOf lab:locatedIn ) .

# ---- What things are

lab:hasPart a owl:TransitiveProperty ;
    rdfs:domain lab:Assembly .
lab:partOf owl:inverseOf lab:hasPart .
lab:Assembly rdfs:subClassOf lab:Product .

# An EU product: a product supplied from some EU region (cls-svf1, then
# cls-int1, then scm-eqc1, cax-sco).
lab:EUProduct owl:equivalentClass [
    owl:intersectionOf (
        lab:Product
        [ a owl:Restriction ;
          owl:onProperty lab:suppliedFrom ;
          owl:someValuesFrom lab:EURegion ]
    )
] .

# Regulated goods: EU products or medical ones (cls-uni).
lab:RegulatedGoods owl:equivalentClass [ owl:unionOf ( lab:EUProduct lab:Medical ) ] .

# Every part of regulated goods is certified (cls-avf), and a certified part
# is certified under ISO (cls-hv1), a standard (prp-rng).
lab:RegulatedGoods rdfs:subClassOf [
    a owl:Restriction ;
    owl:onProperty lab:hasPart ;
    owl:allValuesFrom lab:CertifiedPart
] .
lab:CertifiedPart rdfs:subClassOf [
    a owl:Restriction ;
    owl:onProperty lab:certifiedUnder ;
    owl:hasValue lab:iso
] , lab:Thing2Ship .
lab:certifiedUnder rdfs:range lab:Standard .

# ---- The data

lab:widget42 lab:madeBy lab:alice .
lab:gearbox3 lab:partOf lab:widget42 .   # inverse: widget42 hasPart gearbox3
lab:gearbox3 lab:hasPart lab:screw9 .    # transitive: widget42 hasPart screw9

# A serial number names one thing (prp-ifp): the catalogue entry with the
# same serial is the same screw (owl:sameAs), and takes all its types.
lab:serialNumber a owl:InverseFunctionalProperty .
lab:screw9    lab:serialNumber "S-0009" .
lab:catalog77 lab:serialNumber "S-0009" .
`
const REASONING_CLASH = head(["Reasoning: Rules = default, and Reasoning > Diagnostics on: seven errors, each with its proof."]) + `@prefix hc: <http://example.org/hidden/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

# Contradictions that no single line states: each one appears only after
# several inferences. Needs Turtle › Reasoning: Diagnostics on. Each finding
# carries its proof in hover.

# 1. Asymmetric, through an inverse and a subproperty (prp-inv1, prp-spo1,
#    then prp-asyp). dana has erin as deputy, so erin reports to dana; dana
#    acts for erin, so dana reports to erin.
hc:reportsTo a owl:AsymmetricProperty .
hc:hasDeputy owl:inverseOf hc:reportsTo .
hc:actsFor rdfs:subPropertyOf hc:reportsTo .
hc:dana hc:hasDeputy hc:erin .
hc:dana hc:actsFor hc:erin .

# 2. A class and its complement, through a merge (prp-fp, eq-rep,
#    cax-sco, then cls-com). A badge has one holder, so frank and fritz are
#    the same person: an employee (internal) and a vendor (external).
hc:External owl:complementOf hc:Internal .
hc:Employee rdfs:subClassOf hc:Internal .
hc:Vendor rdfs:subClassOf hc:External .
hc:holder a owl:FunctionalProperty .
hc:badge7 hc:holder hc:frank , hc:fritz .
hc:frank a hc:Employee .
hc:fritz a hc:Vendor .

# 3. Disjoint properties, both reached through chains (prp-spo2, prp-spo1,
#    then prp-pdw). Whoever requests a purchase must not approve it.
hc:approvedBy owl:propertyDisjointWith hc:requestedBy .
hc:requestedBy owl:propertyChainAxiom ( hc:partOfOrder hc:orderedBy ) .
hc:signedOffBy rdfs:subPropertyOf hc:approvedBy .
hc:line12 hc:partOfOrder hc:order5 .
hc:order5 hc:orderedBy hc:gina .
hc:line12 hc:signedOffBy hc:gina .

# 4. Nothing allowed, something found (cls-hv2, scm-eqc1 and cax-sco, prp-inv1, then
#    cls-maxc1). The retired work on no project; hank's status makes him
#    retired, and a project lists him as its worker.
hc:Retired owl:equivalentClass [
    a owl:Restriction ; owl:onProperty hc:status ; owl:hasValue hc:retired
] .
hc:Retired rdfs:subClassOf [
    a owl:Restriction ; owl:onProperty hc:worksOn ; owl:maxCardinality "0"^^xsd:nonNegativeInteger
] .
hc:hasWorker owl:inverseOf hc:worksOn .
hc:hank hc:status hc:retired .
hc:apollo hc:hasWorker hc:hank .

# 5. A negative property assertion, broken by a chain (prp-spo2, then
#    prp-npa1). ivy must not audit the vault; she audits the building it is
#    in, and auditing a building audits what is in it.
hc:audits owl:propertyChainAxiom ( hc:audits hc:contains ) .
hc:hq hc:contains hc:vault .
hc:ivy hc:audits hc:hq .
[] a owl:NegativePropertyAssertion ;
    owl:sourceIndividual hc:ivy ;
    owl:assertionProperty hc:audits ;
    owl:targetIndividual hc:vault .

# 6. All different, then made the same (prp-ifp, then eq-diff2). Three
#    sensors declared pairwise different; two share a MAC address, which
#    names one device.
hc:mac a owl:InverseFunctionalProperty .
[] a owl:AllDifferent ; owl:members ( hc:sensorA hc:sensorB hc:sensorC ) .
hc:sensorA hc:mac "00:1B:44:11:3A:B7" .
hc:sensorC hc:mac "00:1B:44:11:3A:B7" .

# 7. Classified into owl:Nothing (cls-int1, cax-sco, then cls-nothing2).
#    A part that is both a prototype and in production cannot exist.
hc:Impossible owl:intersectionOf ( hc:Prototype hc:InProduction ) .
hc:Impossible rdfs:subClassOf owl:Nothing .
hc:InProduction owl:equivalentClass [
    a owl:Restriction ; owl:onProperty hc:line ; owl:someValuesFrom hc:AssemblyLine
] .
hc:part88 a hc:Prototype ; hc:line hc:line3 .
hc:line3 a hc:AssemblyLine .
`
const REASONING_IDENTITY = head(["Reasoning: Rules = complete (the identity rules are off in default)."]) + `@prefix id: <http://example.org/identity/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

# Identity: keys and "at most one" make individuals the same. These rules
# are off in default; set Turtle › Reasoning: Rules to complete to see
# this file work. With default it shows nothing.
#
# The cascade: a key merges two account records (prp-key), each account has
# at most one owner, so its two owners merge (cls-maxc2), each person has at
# most one home country, so their two countries merge (cls-maxqc3), and the
# merged country is both a monarchy and a republic (cax-dw): an error.
# Hover on id:rec2: it is an id:Account and the same as id:rec1.

id:Account owl:hasKey ( id:bank id:number ) .
id:rec1 a id:Account ; id:bank id:northBank ; id:number "4410-22" .
id:rec2 a id:Account ; id:bank id:northBank ; id:number "4410-22" .

id:Account rdfs:subClassOf [
    a owl:Restriction ; owl:onProperty id:owner ; owl:maxCardinality "1"^^xsd:nonNegativeInteger
] .
id:rec1 id:owner id:jo .
id:rec2 id:owner id:joanna .
id:jo a id:Person .

id:Person rdfs:subClassOf [
    a owl:Restriction ;
    owl:onProperty id:homeCountry ;
    owl:maxQualifiedCardinality "1"^^xsd:nonNegativeInteger ;
    owl:onClass id:Country
] .
id:jo id:homeCountry id:ruritania .
id:joanna id:homeCountry id:freedonia .
id:ruritania a id:Country , id:Monarchy .
id:freedonia a id:Country , id:Republic .
id:Monarchy owl:disjointWith id:Republic .
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
  { path: "reasoning/01-long-chain.ttl", text: REASONING_CHAIN },
  { path: "reasoning/02-hidden-contradictions.ttl", text: REASONING_CLASH },
  { path: "reasoning/03-identity-needs-complete.ttl", text: REASONING_IDENTITY },
]
