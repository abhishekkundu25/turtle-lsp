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
  "6. reasoning/ holds OWL reasoning examples, with reasoning on: hover as each",
  "   file says. Reasoning is in the settings (Ctrl+, then Reasoning).",
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
const REASONING_CHAIN = head(["Reasoning is on here (settings, Ctrl+, > Reasoning > Rules = default): hover the terms named below."]) + `@prefix lab: <http://example.org/lab/> .
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
const REASONING_CLASH = head(["Turn on Reasoning > Diagnostics in the settings (Ctrl+,) to see seven errors, each with its proof."]) + `@prefix hc: <http://example.org/hidden/> .
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
const REASONING_IDENTITY = head(["Set Reasoning > Rules to complete in the settings (Ctrl+,): the identity rules are off in default."]) + `@prefix id: <http://example.org/identity/> .
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

const REASONING_QUERY_PARTS = `${head([
  "A query over what reasoning infers. Run it (the Run button above), then set",
  "Inferred beside Run to Off and run it again: the rows disappear, since no",
  "file says that a part is certified. With Default rules: lab:gearbox3,",
  "lab:screw9 and lab:catalog77 (the same screw, merged by its serial number).",
])}PREFIX lab: <http://example.org/lab/>

SELECT ?part ?standard WHERE {
  ?part a lab:CertifiedPart ;
        lab:certifiedUnder ?standard .
}
ORDER BY ?part
`
const REASONING_QUERY_SAME = `${head([
  "Which names stand for the same thing? No owl:sameAs is written: a serial",
  "number, a badge holder and a MAC address merge them. Inferred Off: no rows.",
  "Default rules: three pairs. Complete rules: the identity file's merges too.",
])}PREFIX owl: <http://www.w3.org/2002/07/owl#>
PREFIX lab: <http://example.org/lab/>
PREFIX hc: <http://example.org/hidden/>
PREFIX id: <http://example.org/identity/>

SELECT ?a ?b WHERE {
  ?a owl:sameAs ?b .
  FILTER (?a != ?b)
}
ORDER BY ?a ?b
`

const REASONING_Q_ORGS = head(["Who is an organisation? Run it, then flip Inferred beside Run.", "Off: id:acme. Default rules: id:alice too: Bob works for her, and", "whatever someone works for is an Organisation (ex:worksFor's range).", "Complete rules: hc:part88 as well: it is a member of owl:Nothing (a", "contradiction), and with every rule on, owl:Nothing is under every class."]) + `SELECT ?org WHERE { ?org a ex:Organisation } ORDER BY ?org
`
const REASONING_Q_SUPPLIED = head(["Where is the widget supplied from? No file says it.", "Off: no rows. Default rules: lab:bavaria, lab:germany, lab:munich, through", "a property chain built on another chain over a transitive locatedIn."]) + `PREFIX lab: <http://example.org/lab/>

SELECT ?place WHERE { lab:widget42 lab:suppliedFrom ?place } ORDER BY ?place
`
const REASONING_Q_PARTS = head(["What are the widget's parts, parts of parts included?", "Off: no rows (only gearbox3 partOf widget42 is written).", "Default rules: lab:catalog77, lab:gearbox3, lab:screw9 (inverse, then transitive)."]) + `PREFIX lab: <http://example.org/lab/>

SELECT ?part WHERE { lab:widget42 lab:hasPart ?part } ORDER BY ?part
`
const REASONING_Q_TYPES = head(["What is the catalogue entry? It is the same screw, merged by its serial number.", "Off: no rows. Default rules: lab:CertifiedPart, lab:Thing2Ship."]) + `PREFIX lab: <http://example.org/lab/>

SELECT ?type WHERE { lab:catalog77 a ?type FILTER isIRI(?type) } ORDER BY ?type
`
const REASONING_Q_REPORTS = head(["Who reports to whom? A contradiction hides here.", "Off: no rows. Default rules: Dana reports to Erin and Erin to Dana, though", "hc:reportsTo is asymmetric: one of the seven contradictions in 02."]) + `PREFIX hc: <http://example.org/hidden/>

SELECT ?a ?b WHERE { ?a hc:reportsTo ?b } ORDER BY ?a
`
const REASONING_Q_IDENTITY = head(["Are the two bank records one account? Only the identity rules say so.", "Off: false. Default rules: false. Complete rules: true (a key, then", "\"at most one owner\", then \"at most one home country\")."]) + `PREFIX ident: <http://example.org/identity/>
PREFIX owl: <http://www.w3.org/2002/07/owl#>

ASK { ident:rec1 owl:sameAs ident:rec2 }
`
const REASONING_Q_COUNT = head(["How many triples does the query see? Run it with Off, Default rules and", "Complete rules: the count grows with each, by what TurbOWL inferred."]) + `SELECT (COUNT(*) AS ?triples) WHERE { ?s ?p ?o }
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
  { path: "reasoning/04-certified-parts.rq", text: REASONING_QUERY_PARTS },
  { path: "reasoning/05-merged-names.rq", text: REASONING_QUERY_SAME },
  { path: "reasoning/06-organisations.rq", text: REASONING_Q_ORGS },
  { path: "reasoning/07-supplied-from.rq", text: REASONING_Q_SUPPLIED },
  { path: "reasoning/08-parts-of-parts.rq", text: REASONING_Q_PARTS },
  { path: "reasoning/09-catalogue-entry.rq", text: REASONING_Q_TYPES },
  { path: "reasoning/10-reports-to.rq", text: REASONING_Q_REPORTS },
  { path: "reasoning/11-one-account.rq", text: REASONING_Q_IDENTITY },
  { path: "reasoning/12-how-much-added.rq", text: REASONING_Q_COUNT },
]
