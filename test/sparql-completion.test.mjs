// Completion in SPARQL documents: keywords by context, variables, prefixes
// (adding the PREFIX line), workspace and vocabulary terms ranked by
// position and by the ontology, and built-in functions.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { queryServer, labelsOf, itemsOf, apply } from "./query-client.mjs"

const EX = "PREFIX ex: <https://query.example.org/def/>\n"

let q
before(async () => {
  q = await queryServer()
})
after(async () => {
  await q.stop()
})

const show = (res) => labelsOf(res).slice(0, 15).join(", ")
const has = (res, ...want) => {
  const l = labelsOf(res)
  for (const w of want) assert.ok(l.includes(w), `${w} not in ${show(res)}`)
}
const hasNot = (res, ...unwanted) => {
  const l = labelsOf(res)
  for (const w of unwanted) assert.ok(!l.includes(w), `${w} offered: ${show(res)}`)
}

// --- keywords -----------------------------------------------------------------

test("query forms and update operations are offered at the top level", async () => {
  const text = `${EX}\n`
  const res = await q.complete(text)
  has(res, "SELECT", "CONSTRUCT", "ASK", "DESCRIBE", "INSERT", "DELETE", "PREFIX", "BASE")
  hasNot(res, "OPTIONAL", "LIMIT")
})

test("a keyword being typed is completed, whatever its case", async () => {
  has(await q.complete(`${EX}SEL`), "SELECT")
  has(await q.complete(`${EX}sel`), "SELECT")
})

test("graph pattern keywords are offered inside a group, solution modifiers are not", async () => {
  const text = `${EX}SELECT * WHERE {\n  ?s ex:name ?o .\n  \n}\n`
  const res = await q.complete(text, "?o .\n  ")
  has(res, "OPTIONAL", "FILTER", "BIND", "VALUES", "MINUS", "GRAPH", "SERVICE")
  hasNot(res, "LIMIT", "ORDER BY", "CONSTRUCT")
})

test("solution modifiers are offered after the WHERE group", async () => {
  const text = `${EX}SELECT * WHERE { ?s ex:name ?o }\n`
  const res = await q.complete(text)
  has(res, "ORDER BY", "GROUP BY", "LIMIT", "OFFSET", "HAVING")
  hasNot(res, "OPTIONAL")
})

test("WHERE is offered after the projection", async () => {
  has(await q.complete(`${EX}SELECT ?s ?o `), "WHERE")
})

// --- variables ------------------------------------------------------------------

test("'?' offers the variables of the query", async () => {
  assert.ok(q.s.init.capabilities.completionProvider.triggerCharacters.includes("?"))
  const text = `${EX}SELECT ?name WHERE {\n  ?person ex:name ?name .\n  FILTER(STRLEN(?\n}\n`
  const res = await q.complete(text, "STRLEN(?")
  has(res, "?name", "?person")
})

test("a variable being typed is completed, and '$' keeps its sigil", async () => {
  const text = `${EX}SELECT * WHERE {\n  ?person ex:name ?name .\n  FILTER(?pe\n}\n`
  has(await q.complete(text, "FILTER(?pe"), "?person")
  const dollar = `${EX}SELECT * WHERE {\n  ?person ex:name ?name .\n  FILTER($\n}\n`
  has(await q.complete(dollar, "FILTER($"), "$person", "$name")
})

// --- prefixes and terms ----------------------------------------------------------

test("declared and well-known prefixes are offered", async () => {
  const text = `${EX}SELECT * WHERE { ?s  }\n`
  const res = await q.complete(text, "?s ")
  has(res, "ex:", "rdfs:", "foaf:")
})

test("workspace terms are completed after a declared prefix", async () => {
  const text = `${EX}SELECT * WHERE { ?s ex: }\n`
  const res = await q.complete(text, "?s ex:")
  has(res, "ex:name", "ex:worksFor", "ex:nickname")
})

test("classes come first after 'a', properties first in predicate position", async () => {
  const cls = await q.complete(`${EX}SELECT * WHERE { ?s a ex: }\n`, "a ex:")
  const firstTwo = labelsOf(cls).slice(0, 2).sort()
  assert.deepEqual(firstTwo, ["ex:Organisation", "ex:Person"], show(cls))
  const prop = await q.complete(`${EX}SELECT * WHERE { ?s ex: }\n`, "?s ex:")
  assert.ok(!["ex:Organisation", "ex:Person"].includes(labelsOf(prop)[0]), show(prop))
})

test("properties whose domain fits the subject's class come first", async () => {
  const person = await q.complete(`${EX}SELECT * WHERE {\n  ?p a ex:Person ;\n    ex:\n}\n`, "    ex:")
  assert.deepEqual(labelsOf(person).slice(0, 2).sort(), ["ex:name", "ex:worksFor"], show(person))
  const org = await q.complete(`${EX}SELECT * WHERE {\n  ?o a ex:Organisation ;\n    ex:\n}\n`, "    ex:")
  assert.equal(labelsOf(org)[0], "ex:founded", show(org))
})

test("the subject's class counts across triple patterns of the same group", async () => {
  const text = `${EX}SELECT * WHERE {\n  ?o a ex:Organisation .\n  ?o ex:\n}\n`
  const res = await q.complete(text, "?o ex:")
  assert.equal(labelsOf(res)[0], "ex:founded", show(res))
})

test("accepting a term whose prefix is undeclared adds a PREFIX line", async () => {
  const text = `${EX}SELECT * WHERE { ?s a foaf:Pers }\n`
  const res = await q.complete(text, "foaf:Pers")
  const person = itemsOf(res).find((i) => i.label === "foaf:Person")
  assert.ok(person, show(res))
  const edits = person.additionalTextEdits ?? []
  assert.ok(edits.some((e) => e.newText.includes("PREFIX foaf: <http://xmlns.com/foaf/0.1/>")), JSON.stringify(person))
  assert.ok(!edits.some((e) => e.newText.includes("@prefix")), JSON.stringify(person))
  // The new line joins the PREFIX block.
  const after = apply(text, edits)
  assert.match(after, /^PREFIX ex: <[^>]+>\nPREFIX foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/>\n/)
})

test("a term typed without a prefix is found by its local name", async () => {
  const res = await q.complete(`${EX}SELECT * WHERE { ?s a Pers }\n`, "a Pers")
  const labels = labelsOf(res)
  assert.ok(labels.includes("ex:Person") && labels.includes("foaf:Person"), show(res))
})

test("no term completion inside a string, an IRI or a comment", async () => {
  for (const [text, needle] of [
    [`${EX}SELECT * WHERE { ?s ex:name "ex:" }\n`, '"ex:'],
    [`${EX}SELECT * WHERE { ?s <https://query.example.org/def/ex: ?o }\n`, "def/ex:"],
    [`${EX}# ex:\nSELECT * WHERE { ?s ?p ?o }\n`, "# ex:"],
  ]) {
    const labels = labelsOf(await q.complete(text, needle))
    assert.ok(!labels.includes("ex:name"), `${needle}: ${labels.slice(0, 10).join(", ")}`)
  }
})

// --- functions -------------------------------------------------------------------

test("built-in functions and aggregates are offered in expressions", async () => {
  const filter = await q.complete(`${EX}SELECT * WHERE { ?s ex:name ?n FILTER(  ) }\n`, "FILTER( ")
  has(filter, "STR", "LANG", "REGEX", "CONTAINS", "BOUND", "COALESCE", "isIRI")
  const projection = await q.complete(`${EX}SELECT (  AS ?c) WHERE { ?s ex:name ?n }\n`, "SELECT ( ")
  has(projection, "COUNT", "SUM", "MIN", "MAX", "AVG", "GROUP_CONCAT", "SAMPLE")
})

// How long this takes: latency.test.mjs.
test("completion stays bounded in a long query", async () => {
  const body = Array.from({ length: 300 }, (_, i) => `  ?s${i} ex:name ?n${i} .`).join("\n")
  const text = `${EX}SELECT * WHERE {\n${body}\n  ?x ex:\n}\n`
  const res = await q.complete(text, "?x ex:")
  assert.ok(itemsOf(res).length <= 1000)
  has(res, "ex:name")
})
