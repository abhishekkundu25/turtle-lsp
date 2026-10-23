// Quick fixes and refactorings in SPARQL documents: add a missing PREFIX (the
// project's binding first), add all missing prefixes, remove an unused one,
// fix a misspelled term, organise prefixes, shorten and expand IRIs.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { queryServer, positionOf, apply, ofCode, fmt } from "./query-client.mjs"

const EX = "PREFIX ex: <https://query.example.org/def/>\n"

let q
before(async () => {
  q = await queryServer()
})
after(async () => {
  await q.stop()
})

const titles = (actions) => actions.map((a) => a.title)

// Code actions at `needle` with the document's diagnostics; `fn(actions, doc)`.
async function actionsAt(text, needle, fn, { offsetInNeedle = 0, only } = {}) {
  return q.withScratch(text, async (doc) => {
    const pos = needle ? positionOf(text, needle, offsetInNeedle) : { line: 0, character: 0 }
    const diagnostics = doc.diags.filter((d) => d.range.start.line <= pos.line && d.range.end.line >= pos.line)
    const actions = await q.s.request("textDocument/codeAction", {
      textDocument: { uri: doc.uri },
      range: { start: pos, end: pos },
      context: { diagnostics, ...(only ? { only } : {}) },
    })
    return fn(actions ?? [], doc)
  })
}

// The text after running the action whose title starts with `title`.
async function run(text, needle, title, opts) {
  return actionsAt(
    text,
    needle,
    async (actions, doc) => {
      let action = actions.find((a) => a.title.startsWith(title))
      assert.ok(action, `no "${title}" in: ${titles(actions).join(", ")}`)
      if (!action.edit) action = await q.s.request("codeAction/resolve", action)
      return { action, text: apply(text, action.edit.changes[doc.uri]) }
    },
    opts
  )
}

// --- prefixes ------------------------------------------------------------------

test("an undeclared well-known prefix gets an 'Add PREFIX' fix that joins the PREFIX block", async () => {
  const text = `${EX}SELECT * WHERE { ?s foaf:name ?n ; ex:name ?m }\n`
  const { action, text: after } = await run(text, "foaf:name", "Add PREFIX foaf:")
  assert.equal(action.kind, "quickfix")
  assert.equal(action.isPreferred, true)
  assert.equal(after, `${EX}PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT * WHERE { ?s foaf:name ?n ; ex:name ?m }\n`)
})

test("in a query without a prologue the PREFIX line goes first, then a blank line", async () => {
  const text = `SELECT * WHERE { ?s foaf:name ?n }\n`
  const { text: after } = await run(text, "foaf:name", "Add PREFIX foaf:")
  assert.equal(after, `PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n\nSELECT * WHERE { ?s foaf:name ?n }\n`)
})

test("a PREFIX added to a query with BASE goes after BASE", async () => {
  const text = `BASE <https://query.example.org/>\nSELECT * WHERE { ?s foaf:name ?n }\n`
  const { text: after } = await run(text, "foaf:name", "Add PREFIX foaf:")
  assert.equal(after, `BASE <https://query.example.org/>\nPREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT * WHERE { ?s foaf:name ?n }\n`)
})

test("the project's own binding of a prefix is offered first", async () => {
  const text = `SELECT * WHERE { ?s ex:name ?n }\n`
  await actionsAt(text, "ex:name", (actions) => {
    const add = actions.filter((a) => a.title.startsWith("Add PREFIX ex:"))
    assert.ok(add.length >= 1, titles(actions).join(", "))
    assert.match(add[0].title, /^Add PREFIX ex: <https:\/\/query\.example\.org\/def\/>/)
    assert.equal(add[0].isPreferred, true)
  })
})

test("'Add all missing prefixes' declares every undeclared well-known prefix at once", async () => {
  const text = `SELECT * WHERE { ?s foaf:name ?n ; dcterms:title ?t ; skos:prefLabel ?l }\n`
  const { text: after } = await run(text, "foaf:name", "Add all missing prefixes")
  for (const p of ["foaf", "dcterms", "skos"]) assert.match(after, new RegExp(`^PREFIX ${p}: <`, "m"), after)
  assert.ok(!after.includes("@prefix"), after)
  const diags = await q.withScratch(after, (d) => d.diags)
  assert.equal(ofCode(diags, "undeclared-prefix").length, 0, fmt(diags))
})

test("an unused PREFIX has a fix that removes its line", async () => {
  const text = `PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n${EX}SELECT * WHERE { ?s ex:name ?n }\n`
  const { text: after } = await run(text, "PREFIX foaf", "Remove unused prefix")
  assert.equal(after, `${EX}SELECT * WHERE { ?s ex:name ?n }\n`)
})

// --- terms -------------------------------------------------------------------------

test("a misspelled vocabulary term has a fix to the suggestion", async () => {
  const text = `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT * WHERE { ?s rdfs:labl ?l }\n`
  const { text: after } = await run(text, "rdfs:labl", "Change to 'rdfs:label'")
  assert.equal(after, text.replace("rdfs:labl", "rdfs:label"))
})

test("a misspelled own term has a fix to the described term", async () => {
  const text = `${EX}SELECT * WHERE { ?p ex:worksFr ?o }\n`
  const { text: after } = await run(text, "ex:worksFr", "Change to 'ex:worksFor'")
  assert.equal(after, text.replace("ex:worksFr", "ex:worksFor"))
})

// --- refactorings ------------------------------------------------------------------

test("organise prefixes sorts, dedupes and drops unused PREFIX lines", async () => {
  const text = `# Header
PREFIX z: <http://z.example/>
PREFIX unused: <http://unused.example/>
PREFIX b: <http://b.example/> # trailing
PREFIX z: <http://z.example/>
SELECT * WHERE { ?s z:p ?o ; b:q ?r }
`
  const { action, text: after } = await run(text, "z:p", "Organise prefixes")
  assert.equal(action.kind, "source.organizeImports")
  assert.equal(
    after,
    `# Header
PREFIX b: <http://b.example/> # trailing
PREFIX z: <http://z.example/>
SELECT * WHERE { ?s z:p ?o ; b:q ?r }
`
  )
})

test("organise prefixes is not offered when nothing changes", async () => {
  const text = `PREFIX a: <http://a/>\nPREFIX b: <http://b/>\nSELECT * WHERE { ?s a:p ?o ; b:q ?r }\n`
  await actionsAt(text, "a:p", (actions) => assert.ok(!titles(actions).includes("Organise prefixes"), titles(actions).join(", ")))
})

test("shorten IRI rewrites the IRI at the cursor with a declared prefix", async () => {
  const text = `${EX}SELECT * WHERE { ?s <https://query.example.org/def/name> ?n }\n`
  const { action, text: after } = await run(text, "<https://query.example.org/def/name>", "Shorten IRI", { offsetInNeedle: 3 })
  assert.equal(action.title, "Shorten IRI to ex:name")
  assert.equal(action.kind, "refactor.rewrite")
  assert.equal(after, text.replace("<https://query.example.org/def/name>", "ex:name"))
})

test("shorten all IRIs in a query", async () => {
  const text = `${EX}SELECT * WHERE { <https://query.example.org/def/a> <https://query.example.org/def/name> ?n }\n`
  const { text: after } = await run(text, null, "Shorten all IRIs in file")
  assert.equal(after, `${EX}SELECT * WHERE { ex:a ex:name ?n }\n`)
})

test("expand a prefixed name to its full IRI", async () => {
  const text = `${EX}SELECT * WHERE { ?s ex:name ?n }\n`
  const { text: after } = await run(text, "ex:name", "Expand", { offsetInNeedle: 4 })
  assert.equal(after, text.replace("?s ex:name", "?s <https://query.example.org/def/name>"))
})

test("a rewritten IRI never joins its neighbour, and the query still parses", async () => {
  const P = "PREFIX ex: <http://e/>\n"
  const cases = [
    ["SELECT * { <http://e/s><http://e/p><http://e/o> }", null, "Shorten all", "SELECT * { ex:s ex:p ex:o }"],
    ["SELECT * { ?s ex:p?<http://e/o> }", "<http://e/o>", "Shorten IRI", "SELECT * { ?s ex:p? ex:o }"],
    ["SELECT * { ?s ?p ?a FILTER(?a<ex:b) }", "ex:b", "Expand", "SELECT * { ?s ?p ?a FILTER(?a<<http://e/b>) }"],
  ]
  for (const [query, needle, title, want] of cases) {
    const { text: after } = await run(`${P}${query}\n`, needle, title, { offsetInNeedle: 1 })
    assert.equal(after, `${P}${want}\n`)
    const errors = await q.withScratch(after, (doc) => doc.diags.filter((d) => d.severity === 1))
    assert.deepEqual(errors, [], after)
  }
})

test("no Turtle-only actions are offered in a query", async () => {
  const text = `${EX}SELECT * WHERE { ?s ex:name ?n ; foaf:name ?m }\n`
  await actionsAt(text, "foaf:name", (actions) => {
    for (const a of actions) {
      assert.ok(!/@prefix|owl:imports/.test(a.title), a.title)
      for (const edits of Object.values(a.edit?.changes ?? {})) {
        for (const e of edits) assert.ok(!e.newText.includes("@prefix"), e.newText)
      }
    }
  })
})
