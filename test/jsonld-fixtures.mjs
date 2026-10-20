// Fixtures and position helpers for the pending JSON-LD tests
// (test/jsonld.test.mjs, web/test/jsonld-engine.test.mjs).
//
// Every expected range is written out as numbers (0-based line and UTF-16
// character, end exclusive) and checked against the fixture text when this
// module loads, so a fixture edit that moves a range fails loudly here, not
// as a confusing pending test.
//
// Where a range sits, as these tests pin it:
// - an IRI or term written in a JSON string (a key, an `@id`, an `@type`
//   value, a context entry, a language tag) covers the string's content,
//   without the quotes, as a Turtle IRI token would;
// - a literal value covers the whole JSON token, quotes included, as a
//   Turtle literal does;
// - a JSON syntax error covers the offending token: the value after which a
//   comma is missing (as Turtle marks the token before a missing '.'), a
//   trailing comma, or a bad literal.
// Every file here is ASCII, so UTF-16 characters, code points and bytes agree.

/** The range from `[line, character]` to `[line, character]`, after checking it covers `written`. */
export function span(text, [l1, c1], [l2, c2], written) {
  const range = { start: { line: l1, character: c1 }, end: { line: l2, character: c2 } }
  const got = covered(text, range)
  if (got !== written) throw new Error(`fixture range ${l1}:${c1}-${l2}:${c2} covers ${JSON.stringify(got)}, not ${JSON.stringify(written)}`)
  return range
}

export function offsetOf(text, p) {
  const lines = text.split("\n")
  return lines.slice(0, p.line).reduce((n, l) => n + l.length + 1, 0) + p.character
}

/** The text a range covers. */
export function covered(text, range) {
  return text.slice(offsetOf(text, range.start), offsetOf(text, range.end))
}

export function applyEdits(text, edits) {
  const sorted = [...edits].sort((a, b) => offsetOf(text, b.range.start) - offsetOf(text, a.range.start))
  for (const e of sorted) text = text.slice(0, offsetOf(text, e.range.start)) + e.newText + text.slice(offsetOf(text, e.range.end))
  return text
}

/** An LSP range as `turtle-lsp check --format json` writes it (1-based line and column). */
export const toCli = (r) => ({
  start: { line: r.start.line + 1, column: r.start.character + 1 },
  end: { line: r.end.line + 1, column: r.end.character + 1 },
})

/** A position inside a range, `n` characters after its start. */
export const inside = (r, n = 1) => ({ line: r.start.line, character: r.start.character + n })

export const rangeKey = (r) => `${r.start.line}:${r.start.character}-${r.end.line}:${r.end.character}`

export const NS = "https://jsonld.example.org/def/"

// ---- a vocabulary written in JSON-LD -----------------------------------------
// Describes seven terms of NS (enough to make NS an own namespace) and the
// ontology <https://jsonld.example.org/def>.
export const VOCAB = `{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "owl": "http://www.w3.org/2002/07/owl#",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#"
  },
  "@graph": [
    { "@id": "https://jsonld.example.org/def", "@type": "owl:Ontology" },
    { "@id": "ex:Widget", "@type": "owl:Class", "rdfs:label": "Widget from JSON-LD" },
    { "@id": "ex:Gadget", "@type": "owl:Class", "rdfs:label": "Gadget from JSON-LD" },
    { "@id": "ex:Part", "@type": "owl:Class" },
    { "@id": "ex:weight", "@type": "owl:DatatypeProperty" },
    { "@id": "ex:colour", "@type": "owl:DatatypeProperty" },
    { "@id": "ex:partOf", "@type": "owl:ObjectProperty" }
  ]
}
`
export const VOCAB_WIDGET = span(VOCAB, [8, 14], [8, 23], "ex:Widget")
export const VOCAB_WEIGHT = span(VOCAB, [11, 14], [11, 23], "ex:weight")

// ---- Turtle that uses the JSON-LD vocabulary ---------------------------------
export const USES = `@prefix ex: <https://jsonld.example.org/def/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .

<https://jsonld.example.org/data> a owl:Ontology ;
    owl:imports <https://jsonld.example.org/def> .

ex:Sprocket a owl:Class .

ex:w1 a ex:Widget ;
    ex:weight 3 ;
    foaf:name "w1" ;
    ex:partOf ex:g1 .
ex:g1 a ex:Gadgt .
`
export const USES_IMPORT = span(USES, [5, 16], [5, 48], "<https://jsonld.example.org/def>")
export const USES_WIDGET = span(USES, [9, 8], [9, 17], "ex:Widget")
export const USES_FOAF_NAME = span(USES, [11, 4], [11, 13], "foaf:name")
export const USES_GADGT = span(USES, [13, 8], [13, 16], "ex:Gadgt")

// JSON-LD that uses a term the Turtle describes (ex:Sprocket) and a typo of it.
export const BACK = `{
  "@context": { "ex": "https://jsonld.example.org/def/" },
  "@graph": [
    { "@id": "ex:s1", "@type": "ex:Sprocket" },
    { "@id": "ex:s2", "@type": "ex:Sprockett" }
  ]
}
`
export const BACK_TYPO = span(BACK, [4, 32], [4, 44], "ex:Sprockett")

// Keys expanded through @vocab: "colur" is a typo of the described ex:colour.
export const VOCAB_KEYS = `{
  "@context": { "@vocab": "https://jsonld.example.org/def/" },
  "@id": "https://jsonld.example.org/def/w12",
  "colour": "red",
  "colur": "blue",
  "http://www.w3.org/2000/01/rdf-schema#label": "w12"
}
`
export const VOCAB_KEYS_TYPO = span(VOCAB_KEYS, [4, 3], [4, 8], "colur")

// ---- JSON syntax ---------------------------------------------------------------
// A comma missing after `3`, and a bad literal `tru`: two errors, the second
// one found after recovering from the first.
export const SYNTAX = `{
  "@context": { "ex": "https://jsonld.example.org/def/" },
  "@id": "ex:w9",
  "ex:weight": 3
  "ex:colour": "red",
  "ex:partOf": { "@id": "ex:g1", "ex:weight": tru }
}
`
export const SYNTAX_MISSING_COMMA = span(SYNTAX, [3, 15], [3, 16], "3")
export const SYNTAX_BAD_LITERAL = span(SYNTAX, [5, 46], [5, 49], "tru")
export const SYNTAX_FIXED_COMMA = SYNTAX.replace('"ex:weight": 3\n', '"ex:weight": 3,\n')

export const TRAILING_COMMA = `{
  "@id": "ex:w17",
}
`
export const TRAILING_COMMA_AT = span(TRAILING_COMMA, [1, 17], [1, 18], ",")

// ---- @context errors -------------------------------------------------------------
export const CTX_INVALID = `{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "weight": 5
  },
  "@id": "ex:w10",
  "ex:colour": "red"
}
`
export const CTX_INVALID_AT = span(CTX_INVALID, [3, 5], [3, 11], "weight")

export const CTX_CYCLE = `{
  "@context": {
    "a": "b:x",
    "b": "a:y"
  },
  "@id": "https://jsonld.example.org/def/w15",
  "a": 1
}
`
export const CTX_CYCLE_A = span(CTX_CYCLE, [2, 5], [2, 6], "a")
export const CTX_CYCLE_B = span(CTX_CYCLE, [3, 5], [3, 6], "b")

export const CTX_PROTECTED = `{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "label": { "@id": "http://www.w3.org/2000/01/rdf-schema#label", "@protected": true }
  },
  "@id": "ex:w16",
  "ex:partOf": {
    "@context": { "label": "ex:name" },
    "@id": "ex:g16"
  }
}
`
export const CTX_PROTECTED_AT = span(CTX_PROTECTED, [7, 19], [7, 24], "label")

// ---- lints -------------------------------------------------------------------------
export const LINTS = `{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "vcard": "http://www.w3.org/2006/vcard/ns#",
    "weight": "ex:weight",
    "title": "rdfs:lable",
    "seeAlso": { "@id": "rdfs:seeAlso", "@type": "@id" }
  },
  "@id": "ex:w11",
  "@type": "vcard:Email",
  "weight": 3,
  "wieght": 4,
  "rdfs:labl": "Widget eleven",
  "title": "Eleven",
  "seeAlso": "http://exa mple.org/x",
  "rdfs:comment": { "@value": "Hallo", "@language": "de_DE" }
}
`
export const LINTS_DEF_TYPO = span(LINTS, [6, 14], [6, 24], "rdfs:lable")
export const LINTS_DEPRECATED = span(LINTS, [10, 12], [10, 23], "vcard:Email")
export const LINTS_UNDEFINED = span(LINTS, [12, 3], [12, 9], "wieght")
export const LINTS_KEY_TYPO = span(LINTS, [13, 3], [13, 12], "rdfs:labl")
export const LINTS_BAD_IRI = span(LINTS, [15, 14], [15, 35], "http://exa mple.org/x")
export const LINTS_BAD_LANG = span(LINTS, [16, 53], [16, 58], "de_DE")

// Every key here means something: keywords, a compact IRI with a defined
// prefix, an absolute IRI, a defined term and @vocab.
export const LINTS_CLEAN = `{
  "@context": { "ex": "https://jsonld.example.org/def/", "weight": "ex:weight" },
  "@id": "ex:w18",
  "@type": "ex:Widget",
  "weight": 18,
  "ex:colour": "green",
  "http://www.w3.org/2000/01/rdf-schema#label": "w18"
}
`

// ---- remote contexts ---------------------------------------------------------------
export const UNKNOWN_CONTEXT_URL = "https://contexts.example.org/unknown.jsonld"
/** A document whose only context is `url`, defining "colour" (or not), and using an undefined "flavour". */
export const remoteDoc = (url) => `{
  "@context": "${url}",
  "@id": "https://jsonld.example.org/def/w13",
  "colour": "red",
  "flavour": "sweet"
}
`
/** The context URL's range in remoteDoc(url). */
export const remoteDocUrl = (url) => span(remoteDoc(url), [1, 15], [1, 15 + url.length], url)
export const REMOTE_DOC_FLAVOUR = span(remoteDoc("x"), [4, 3], [4, 10], "flavour")
export const REMOTE_CONTEXT = `{ "@context": { "ex": "https://jsonld.example.org/def/", "colour": "ex:colour" } }\n`

export const BUNDLED_CONTEXTS = [
  "https://schema.org/",
  "https://www.w3.org/ns/activitystreams",
  "https://www.w3.org/2018/credentials/v1",
]

// schema.org's context sets @vocab to schema:, so "Persn" expands to schema:Persn.
export const SCHEMA_DOC = `{
  "@context": "https://schema.org/",
  "@type": "Persn",
  "name": "Ann"
}
`
export const SCHEMA_DOC_TYPO = span(SCHEMA_DOC, [2, 12], [2, 17], "Persn")

// A context in a file next to the document that uses it.
export const CTX_FILE = `{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "nickname": "ex:nickname"
  }
}
`
export const CTX_FILE_NICKNAME = span(CTX_FILE, [3, 5], [3, 13], "nickname")
export const USES_CTX_FILE = `{
  "@context": "people.context.jsonld",
  "@id": "https://jsonld.example.org/def/w14",
  "nickname": "Fourteen",
  "nickame": "Typo"
}
`
export const USES_CTX_FILE_NICKNAME = span(USES_CTX_FILE, [3, 3], [3, 11], "nickname")
export const USES_CTX_FILE_TYPO = span(USES_CTX_FILE, [4, 3], [4, 10], "nickame")

// ---- SHACL ------------------------------------------------------------------------------
export const PERSON_SHAPES = `@prefix ex: <https://jsonld.example.org/def/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] ;
    sh:property [ sh:path ex:age ; sh:datatype xsd:integer ] ;
    sh:property [ sh:path ex:worksFor ; sh:class ex:Organisation ] ;
    sh:property [ sh:path ex:nick ; sh:maxCount 1 ] .
`
// ann conforms; every other node breaks one constraint.
export const PEOPLE = `{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "name": "ex:name",
    "age": "ex:age",
    "nick": "ex:nick",
    "worksFor": { "@id": "ex:worksFor", "@type": "@id" }
  },
  "@graph": [
    { "@id": "ex:ann", "@type": "ex:Person", "name": "Ann", "age": 41 },
    { "@id": "ex:bob", "@type": "ex:Person", "age": 7 },
    { "@id": "ex:cy", "@type": "ex:Person", "name": "Cy", "age": "old" },
    { "@id": "ex:di", "@type": "ex:Person", "name": "Di", "worksFor": "ex:cy" },
    { "@type": "ex:Person", "name": "Ed", "nick": ["E", "Eddie"] },
    { "@type": "ex:Person", "age": 3 }
  ]
}
`
// The results, each on what breaks the shape.
export const PEOPLE_RESULTS = [
  // bob has no name: on his @id.
  span(PEOPLE, [10, 14], [10, 20], "ex:bob"),
  // cy's age is not an integer: on the value.
  span(PEOPLE, [11, 65], [11, 70], '"old"'),
  // di works for a person: on the value.
  span(PEOPLE, [12, 71], [12, 76], "ex:cy"),
  // Ed has two nicknames: on the key.
  span(PEOPLE, [13, 43], [13, 47], "nick"),
  // A node without an @id and without a name: on its opening brace.
  span(PEOPLE, [14, 4], [14, 5], "{"),
]

export const ASSET_SHAPES = `@prefix ex: <https://jsonld.example.org/def/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:AssetShape a sh:NodeShape ;
    sh:targetClass ex:Asset ;
    sh:property [ sh:path ex:designation ; sh:minCount 1 ; sh:maxCount 1 ] ;
    sh:sparql [ sh:message "no ex:bad" ; sh:select "SELECT $this ?value WHERE { $this <https://jsonld.example.org/def/bad> ?value }" ] .
`
// Shared data: ex:p7 has no designation (one result, here), ex:p9 conforms.
export const SHARED_ASSETS = `@prefix ex: <https://jsonld.example.org/def/> .

ex:p7 a ex:Asset .
ex:p9 a ex:Asset ;
    ex:designation "A" .
`
export const SHARED_P7 = span(SHARED_ASSETS, [2, 0], [2, 5], "ex:p7")

/** A JSON-LD named graph `ex:<graph>` holding the asset ex:p8 with `designation` (JSON). */
export const assetRun = (graph, designation) => `{
  "@context": { "ex": "https://jsonld.example.org/def/" },
  "@id": "ex:${graph}",
  "@graph": [
    { "@id": "ex:p8", "@type": "ex:Asset", "ex:designation": ${designation} }
  ]
}
`
export const ASSET_RUN_DESIGNATION = span(assetRun("run3", '["A", "B"]'), [4, 44], [4, 58], "ex:designation")

// A named graph that only mentions the shared, violating ex:p7.
export const MENTIONS = `{
  "@context": { "ex": "https://jsonld.example.org/def/" },
  "@id": "ex:run5",
  "@graph": [
    { "@id": "ex:p7", "ex:label": "P7, run 5" }
  ]
}
`
// A named graph that gives the shared ex:p9 a second designation.
export const SECOND_DESIGNATION = `{
  "@context": { "ex": "https://jsonld.example.org/def/" },
  "@id": "ex:run4",
  "@graph": [
    { "@id": "ex:p9", "ex:designation": "B" }
  ]
}
`
export const SECOND_DESIGNATION_KEY = span(SECOND_DESIGNATION, [4, 23], [4, 37], "ex:designation")

// A focus node the SPARQL constraint rejects.
export const BAD_ASSET = `{
  "@context": { "ex": "https://jsonld.example.org/def/" },
  "@id": "ex:p10",
  "@type": "ex:Asset",
  "ex:designation": "X",
  "ex:bad": 1
}
`
export const BAD_ASSET_ID = span(BAD_ASSET, [2, 10], [2, 16], "ex:p10")

// Shapes written in JSON-LD, and Turtle data they reject.
export const JSONLD_SHAPES = `{
  "@context": { "sh": "http://www.w3.org/ns/shacl#", "ex": "https://jsonld.example.org/def/" },
  "@id": "ex:ThingShape",
  "@type": "sh:NodeShape",
  "sh:targetClass": { "@id": "ex:Thing" },
  "sh:property": { "sh:path": { "@id": "ex:label" }, "sh:minCount": 1 }
}
`
export const THING_DATA = `@prefix ex: <https://jsonld.example.org/def/> .

ex:t1 a ex:Thing .
`
export const THING_T1 = span(THING_DATA, [2, 0], [2, 5], "ex:t1")

// ---- phase 2: editing inside JSON-LD ----------------------------------------------------
export const EDIT = `{
  "@context": {
    "@vocab": "https://jsonld.example.org/def/",
    "@base": "https://jsonld.example.org/id/",
    "foaf": "http://xmlns.com/foaf/0.1/",
    "name": "foaf:name",
    "knows": { "@id": "foaf:knows", "@type": "@id" },
    "id": "@id"
  },
  "@graph": [
    { "id": "ann", "@type": "Widget", "name": "Ann", "knows": "bob" },
    { "id": "bob", "@type": "Widget", "name": "Bob", "weight": 4 }
  ]
}
`
export const EDIT_CONTEXT_KEYWORD = span(EDIT, [1, 3], [1, 11], "@context")
export const EDIT_NAME_DEF = span(EDIT, [5, 5], [5, 9], "name")
export const EDIT_ID_DEF = span(EDIT, [7, 5], [7, 7], "id")
export const EDIT_ANN_OBJECT = span(EDIT, [10, 4], [10, 69], '{ "id": "ann", "@type": "Widget", "name": "Ann", "knows": "bob" }')
export const EDIT_ANN_ID_KEY = span(EDIT, [10, 7], [10, 9], "id")
export const EDIT_ANN_ID = span(EDIT, [10, 13], [10, 16], "ann")
export const EDIT_ANN_TYPE = span(EDIT, [10, 29], [10, 35], "Widget")
export const EDIT_ANN_NAME = span(EDIT, [10, 39], [10, 43], "name")
export const EDIT_ANN_KNOWS = span(EDIT, [10, 54], [10, 59], "knows")
export const EDIT_BOB_OBJECT = span(EDIT, [11, 4], [11, 66], '{ "id": "bob", "@type": "Widget", "name": "Bob", "weight": 4 }')
export const EDIT_BOB_ID = span(EDIT, [11, 13], [11, 16], "bob")
export const EDIT_BOB_TYPE = span(EDIT, [11, 29], [11, 35], "Widget")
export const EDIT_BOB_NAME = span(EDIT, [11, 39], [11, 43], "name")
export const EDIT_BOB_WEIGHT = span(EDIT, [11, 54], [11, 60], "weight")

// Valid, compact, unformatted JSON-LD.
export const MESSY = `{"@context":{"ex":"https://jsonld.example.org/def/"},"@id":"ex:w20","@type":"ex:Widget","ex:weight":20,"ex:colour":["red","blue"],"ex:partOf":{"@id":"ex:g20","@type":"ex:Gadget"}}\n`
