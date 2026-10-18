// SPARQL documents: recognising them, and their diagnostics (syntax, prefixes,
// vocabulary terms, undescribed own terms, unbound projections). The
// workspace is test/fixtures/query: ontology.ttl and data.nq describe the
// ex: terms, shared/ has a prefixes.ttl.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { queryServer, read, fmt, problems, ofCode, positionOf } from "./query-client.mjs"

const EX = "PREFIX ex: <https://query.example.org/def/>\n"
const RDFS = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n"

let q
before(async () => {
  q = await queryServer()
})
after(async () => {
  await q.stop()
})

const diagsOf = (text, ext = "rq", languageId) => q.withScratch(text, (d) => d.diags, ext, languageId)

test("people.rq and update.ru have no problems", async () => {
  for (const f of ["people.rq", "update.ru"]) {
    const diags = await q.withFile(f, (d) => d.diags)
    assert.deepEqual(problems(diags), [], `${f}\n${fmt(problems(diags))}`)
  }
})

// --- recognising documents ---------------------------------------------------

for (const [name, languageId] of [
  ["a.rq", "sparql"],
  ["b.rq", "turtle"],
  ["c.rq", "plaintext"],
  ["d.sparql", "plaintext"],
  ["e.txt", "sparql"],
]) {
  test(`${name} opened as '${languageId}' is read as SPARQL`, async () => {
    const { diags } = await q.open(`__detect-${name}`, `${EX}SELECT ?s WHERE { ?s a ex:Person }\n`, languageId)
    assert.deepEqual(problems(diags), [], fmt(diags))
  })
}

test("Turtle in a .rq file is a SPARQL syntax error", async () => {
  const diags = await diagsOf('@prefix ex: <https://query.example.org/def/> .\nex:a ex:name "A" .\n')
  assert.ok(ofCode(diags, "syntax").length > 0, fmt(diags))
})

test("an update in a .rq file and a query in a .ru file are checked as what they are", async () => {
  const update = await diagsOf(`${EX}INSERT DATA { ex:a ex:name "A" }\n`, "rq")
  assert.deepEqual(problems(update), [], fmt(update))
  const query = await diagsOf(`${EX}ASK { ?s ex:name ?n }\n`, "ru")
  assert.deepEqual(problems(query), [], fmt(query))
})

test("keywords are case-insensitive and comments are not code", async () => {
  const text = `${EX}select * where {\n  # a comment with { and ?x\n  ?s ex:name "a # not a comment" .\n} limit 5\n`
  const diags = await diagsOf(text)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("a sequence of update operations with all the forms has no problems", async () => {
  const text = `${EX}LOAD SILENT <http://e.org/data> INTO GRAPH <http://e.org/g> ;
CLEAR GRAPH <http://e.org/g> ;
CREATE SILENT GRAPH <http://e.org/h> ;
COPY DEFAULT TO <http://e.org/h> ;
MOVE <http://e.org/h> TO <http://e.org/i> ;
ADD <http://e.org/i> TO DEFAULT ;
DROP ALL ;
WITH <http://e.org/g> DELETE { ?s ex:name ?o } INSERT { ?s ex:name "x" } WHERE { ?s ex:name ?o } ;
DELETE WHERE { ?s ex:worksFor ?o } ;
DELETE DATA { ex:acme ex:founded 1999 }
`
  const diags = await diagsOf(text, "ru")
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("SPARQL 1.2 syntax (VERSION, triple terms, reifiers, annotations) has no problems", async () => {
  const text = `VERSION "1.2"
${EX}SELECT * WHERE {
  ?s ex:name ?o ~ ?r {| ex:worksFor ?src |} .
  << ?s ex:name ?o >> ex:worksFor ?x .
  BIND(<<( ?s ex:name ?o )>> AS ?t)
  FILTER(isTRIPLE(?t) && SUBJECT(?t) = ?s)
}
`
  const diags = await diagsOf(text)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

// --- syntax errors ---------------------------------------------------------------

test("a syntax error is reported at the token where the query stops making sense", async () => {
  const text = `${EX}SELECT ?x WHERE { ?x ex:name }\n`
  const diags = await diagsOf(text)
  const e = ofCode(diags, "syntax")
  assert.equal(e.length, 1, fmt(diags))
  assert.equal(e[0].severity, 1)
  assert.deepEqual(e[0].range.start, positionOf(text, "ex:name }", 8), fmt(diags))
  assert.match(e[0].message, /expected/i)
})

test("a misspelled keyword is reported at the start of the word", async () => {
  const text = `${EX}SELECT ?x WHERE {\n  ?x a ex:Person .\n  FILTR(?x != ex:acme)\n}\n`
  const diags = await diagsOf(text)
  const e = ofCode(diags, "syntax")
  assert.equal(e.length, 1, fmt(diags))
  assert.deepEqual(e[0].range.start, positionOf(text, "FILTR", 0), fmt(diags))
})

test("an unclosed group is reported", async () => {
  const diags = await diagsOf(`${EX}SELECT ?x WHERE {\n  ?x a ex:Person .\n`)
  assert.equal(ofCode(diags, "syntax").length, 1, fmt(diags))
})

test("syntax errors are reported as the text changes, on the unsaved text", async () => {
  const good = `${EX}SELECT ?x WHERE { ?x a ex:Person }\n`
  const { uri } = await q.scratch(good)
  try {
    let diags = await q.change(uri, `${EX}SELECT ?x WHERE { ?x a }\n`)
    assert.equal(ofCode(diags, "syntax").length, 1, fmt(diags))
    diags = await q.change(uri, good)
    assert.deepEqual(problems(diags), [], fmt(diags))
  } finally {
    q.close(uri)
  }
})

// --- prefixes ------------------------------------------------------------------

test("every undeclared prefix is reported on its prefixed name, and not as a syntax error", async () => {
  const text = `SELECT * WHERE {\n  ?s foaf:name ?n ;\n    dc:title ?t .\n}\n`
  const diags = await diagsOf(text)
  const u = ofCode(diags, "undeclared-prefix")
  assert.deepEqual(
    u.map((d) => [d.range.start.line, d.range.start.character, d.severity]),
    [
      [1, 5, 1],
      [2, 4, 1],
    ],
    fmt(diags)
  )
  assert.equal(ofCode(diags, "syntax").length, 0, fmt(diags))
  // The well-known namespace is named, as a PREFIX line.
  assert.match(u[0].message, /foaf:/)
  assert.match(u[0].message, /PREFIX foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/>/)
})

test("a syntax error after an undeclared prefix is still found", async () => {
  const text = `SELECT * WHERE {\n  ?s foaf:name ?n .\n  ?s ?p\n}\n`
  const diags = await diagsOf(text)
  assert.equal(ofCode(diags, "undeclared-prefix").length, 1, fmt(diags))
  assert.equal(ofCode(diags, "syntax").length, 1, fmt(diags))
})

test("an unused PREFIX is shown as unnecessary", async () => {
  const text = `PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n${EX}SELECT * WHERE { ?s ex:name ?o }\n`
  const diags = await diagsOf(text)
  const u = ofCode(diags, "unused-prefix")
  assert.equal(u.length, 1, fmt(diags))
  assert.equal(u[0].range.start.line, 0)
  assert.equal(u[0].severity, 4)
  assert.deepEqual(u[0].tags, [1])
  assert.match(u[0].message, /foaf:/)
})

test("a prefix declared twice: a hint when the IRI is the same, a warning when it differs", async () => {
  const same = await diagsOf(`${EX}${EX}SELECT * WHERE { ?s ex:name ?o }\n`)
  const dup = ofCode(same, "duplicate-prefix")
  assert.deepEqual(
    dup.map((d) => [d.range.start.line, d.severity]),
    [[1, 4]],
    fmt(same)
  )
  const differ = await diagsOf(`${EX}PREFIX ex: <https://other.example.org/>\nSELECT * WHERE { ?s ex:name ?o }\n`)
  const re = ofCode(differ, "redefined-prefix")
  assert.deepEqual(
    re.map((d) => [d.range.start.line, d.severity]),
    [[1, 2]],
    fmt(differ)
  )
})

test("prefixes from the folder's prefixes.ttl count as declared", async () => {
  const diags = await q.withFile("shared/shared.rq", (d) => d.diags)
  assert.deepEqual(problems(diags), [], fmt(diags))
  // The same query outside that folder has two undeclared prefixes.
  const outside = await diagsOf(read("shared/shared.rq"))
  assert.equal(ofCode(outside, "undeclared-prefix").length, 2, fmt(outside))
})

test("prefixes from the turtle.prefixes setting count as declared", async () => {
  const other = await queryServer({ initializationOptions: { prefixes: { ex: "https://query.example.org/def/" } } })
  try {
    const diags = await other.withScratch("SELECT * WHERE { ?s ex:name ?o }\n", (d) => d.diags)
    assert.deepEqual(problems(diags), [], fmt(diags))
  } finally {
    await other.stop()
  }
})

test("BASE and relative IRIs are accepted", async () => {
  const diags = await diagsOf(`BASE <https://query.example.org/>\nSELECT * WHERE { ?s <def/name> ?o }\n`)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

// --- vocabulary and workspace terms ------------------------------------------------

test("an unknown term of a well-known vocabulary is a warning with a 'did you mean'", async () => {
  const text = `${RDFS}SELECT * WHERE { ?s rdfs:lable ?l }\n`
  const diags = await diagsOf(text)
  const u = ofCode(diags, "unknown-term")
  assert.equal(u.length, 1, fmt(diags))
  assert.equal(u[0].severity, 2)
  assert.deepEqual(u[0].range.start, positionOf(text, "rdfs:lable", 0))
  assert.match(u[0].message, /Did you mean 'label'/)
})

test("vocabulary terms are matched by namespace IRI, not by prefix name", async () => {
  const diags = await diagsOf(`PREFIX rdfs: <https://query.example.org/def/>\nSELECT * WHERE { ?s rdfs:name ?l }\n`)
  assert.equal(ofCode(diags, "unknown-term").length, 0, fmt(diags))
  const full = await diagsOf(`SELECT * WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#lable> ?l }\n`)
  assert.equal(ofCode(full, "unknown-term").length, 1, fmt(full))
})

test("a deprecated vocabulary term is marked deprecated", async () => {
  const diags = await diagsOf(`PREFIX vcard: <http://www.w3.org/2006/vcard/ns#>\nSELECT * WHERE { ?s a vcard:Email }\n`)
  const d = ofCode(diags, "deprecated-term")
  assert.equal(d.length, 1, fmt(diags))
  assert.deepEqual(d[0].tags, [2])
})

test("an own term that the workspace describes nowhere is reported with a suggestion", async () => {
  const text = `${EX}SELECT * WHERE { ?p ex:worksFr ?o ; ex:nickname ?n ; ex:name ?m }\n`
  const diags = await diagsOf(text)
  const u = ofCode(diags, "undescribed-term")
  // ex:nickname is described in data.nq, ex:name in ontology.ttl.
  assert.equal(u.length, 1, fmt(diags))
  assert.deepEqual(u[0].range.start, positionOf(text, "ex:worksFr", 0))
  assert.match(u[0].message, /worksFor/)
})

test("a query never makes a term described", async () => {
  const update = `${EX}INSERT DATA { ex:ghost a ex:Person ; ex:name "Ghost" }\n`
  await q.withScratch(update, async () => {
    const diags = await q.withScratch(`@prefix ex: <https://query.example.org/def/> .\nex:a ex:knows ex:ghost .\n`, (d) => d.diags, "ttl")
    assert.ok(
      ofCode(diags, "undescribed-term").some((d) => d.range.start.character === 14),
      fmt(diags)
    )
  }, "ru")
})

// --- variables -----------------------------------------------------------------

test("a projected variable that the WHERE clause never binds is a warning", async () => {
  const text = `${EX}SELECT ?person ?missing WHERE { ?person ex:name ?name }\n`
  const diags = await diagsOf(text)
  const u = ofCode(diags, "unbound-projection")
  assert.equal(u.length, 1, fmt(diags))
  assert.equal(u[0].severity, 2)
  assert.deepEqual(u[0].range.start, positionOf(text, "?missing", 0))
  assert.match(u[0].message, /\?missing/)
})

test("variables bound by BIND, VALUES, OPTIONAL, subqueries and expressions are not unbound", async () => {
  const text = `${EX}SELECT ?a ?b ?c ?d ?e ?f ?g WHERE {
  ?p ex:name ?n .
  BIND(STRLEN(?n) AS ?a)
  VALUES ?b { 1 2 }
  OPTIONAL { ?p ex:worksFor ?c }
  { SELECT ?d WHERE { ?d a ex:Person } }
  GRAPH ?g { ?p ex:name ?f }
}
`
  const diags = await diagsOf(text)
  assert.deepEqual(problems(diags), [], fmt(diags))
  const agg = await diagsOf(`${EX}SELECT ?org (COUNT(?p) AS ?n) WHERE { ?p ex:worksFor ?org } GROUP BY ?org\n`)
  assert.deepEqual(problems(agg), [], fmt(agg))
})
