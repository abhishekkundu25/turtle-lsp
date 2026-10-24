// Renaming in SPARQL documents: variables (scoped, each occurrence keeps its
// sigil), prefixes (the PREFIX line and every prefixed name), and terms
// across Turtle, N-Quads and SPARQL files in both directions.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { queryServer, positionOf, apply, dir } from "./query-client.mjs"

const EX = "PREFIX ex: <https://query.example.org/def/>\n"

let q
before(async () => {
  q = await queryServer()
})
after(async () => {
  await q.stop()
})

const rename = (uri, position, newName) => q.s.request("textDocument/rename", { textDocument: { uri }, position, newName })
const nameOf = (u) => path.relative(dir, fileURLToPath(u)).split(path.sep).join("/")
// Edits per file as "line:char text", sorted. An edit changes only what
// differs: ex:worksFor to ex:employedBy writes employedBy after the ex:.
function edited(edit) {
  const out = {}
  for (const [u, edits] of Object.entries(edit.changes ?? {})) {
    out[nameOf(u)] = [...edits]
      .sort((a, b) => a.range.start.line - b.range.start.line || a.range.start.character - b.range.start.character)
      .map((e) => `${e.range.start.line}:${e.range.start.character} ${e.newText}`)
  }
  return out
}
const onlyIn = (edit, uri) => {
  const keys = Object.keys(edit.changes ?? {})
  assert.deepEqual(keys, [uri], keys.join(" "))
  return edit.changes[uri]
}

// --- variables -------------------------------------------------------------------

const scoped = `${EX}SELECT ?p ?n WHERE {
  ?p ex:name ?n .
  {
    SELECT ?p (COUNT(?o) AS ?count) WHERE {
      ?p ex:worksFor ?o .
      ?o ex:name ?n .
    }
    GROUP BY ?p
  }
  FILTER(?n != "x")
}
`

test("prepareRename on a variable answers with its range", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    const r = await q.s.request("textDocument/prepareRename", { textDocument: { uri }, position: positionOf(text, "?p ex:name", 1) })
    assert.ok(r, "renameable")
    const range = r.range ?? r
    assert.equal(range.start.line, 2)
  })
})

test("renaming a variable renames it in its scope only", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "?p ex:name ?n", 12), "label")
    const after = apply(text, onlyIn(edit, uri))
    // The subquery's own ?n is a different variable.
    assert.equal(
      after,
      scoped.replace("SELECT ?p ?n WHERE", "SELECT ?p ?label WHERE").replace("?p ex:name ?n .", "?p ex:name ?label .").replace('FILTER(?n != "x")', 'FILTER(?label != "x")')
    )
  })
})

test("a variable projected by a subquery is renamed inside it too", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "?p ex:name", 1), "person")
    const edits = onlyIn(edit, uri)
    assert.equal(edits.length, 5)
    assert.equal(apply(text, edits), text.replace(/\?p\b/g, "?person"))
  })
})

test("each occurrence keeps its own sigil, whatever sigil the new name has", async () => {
  const text = `${EX}SELECT ?x WHERE { $x ex:name ?y . FILTER(BOUND(?x)) }\n`
  for (const newName of ["z", "?z", "$z"]) {
    await q.withScratch(text, async ({ uri }) => {
      const edit = await rename(uri, positionOf(text, "$x", 1), newName)
      assert.equal(apply(text, onlyIn(edit, uri)), `${EX}SELECT ?z WHERE { $z ex:name ?y . FILTER(BOUND(?z)) }\n`, newName)
    })
  }
})

test("renaming a variable to a name used in the same scope is refused", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    await assert.rejects(rename(uri, positionOf(text, "?p ex:name ?n", 12), "p"), /\?p.*already/)
  })
})

test("renaming a variable to the name of a subquery's own variable is allowed", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    // ?o lives only in the subquery; the outer scope has no ?o.
    const edit = await rename(uri, positionOf(text, "?p ex:name ?n", 12), "o")
    assert.equal(onlyIn(edit, uri).length, 3)
  })
})

test("an invalid variable name is refused", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    for (const bad of ["two words", "1x-", "?", "a:b"]) {
      await assert.rejects(rename(uri, positionOf(text, "?p ex:name", 1), bad), /not a valid variable name/, bad)
    }
  })
})

// --- prefixes ------------------------------------------------------------------

test("renaming a prefix edits its PREFIX line and every prefixed name", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "ex:Person", 1), "def")
    const after = apply(text, onlyIn(edit, uri))
    assert.equal(after, text.replaceAll("ex:", "def:"))
  })
})

test("renaming a prefix from its PREFIX line works the same", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "PREFIX ex:", 8), "def:")
    assert.equal(apply(text, onlyIn(edit, uri)), text.replaceAll("ex:", "def:"))
  })
})

test("renaming a prefix to one the query already declares is refused", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    await assert.rejects(rename(uri, positionOf(text, "ex:Person", 1), "rdfs"), /rdfs:.*already declared/)
  })
})

test("a prefix from the folder's prefixes.ttl cannot be renamed in a query", async () => {
  await q.withFile("shared/shared.rq", async ({ uri, text }) => {
    await assert.rejects(rename(uri, positionOf(text, "ex:Person", 1), "def"), /prefixes\.ttl/)
  })
})

// --- terms across files -------------------------------------------------------------

test("renaming a term in a query renames it in the query, the ontology and the dataset", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "ex:worksFor", 5), "ex:employedBy")
    assert.deepEqual(edited(edit), {
      "people.rq": ["9:15 employedBy"],
      "ontology.ttl": ["18:3 employedBy"],
      "data.nq": ["3:68 employedBy"],
    })
  })
})

test("renaming a term in Turtle renames it in SPARQL files that are not open", async () => {
  await q.withFile("ontology.ttl", async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "ex:name a", 4), "fullName")
    const e = edited(edit)
    // ex:name to ex:fullName: the n becomes fullN.
    assert.deepEqual(e["people.rq"], ["7:7 fullN"])
    assert.deepEqual(e["update.ru"], ["4:7 fullN", "6:15 fullN", "7:15 fullN", "8:14 fullN"])
    assert.deepEqual(e["data.nq"], ["2:68 fullN", "7:36 fullN"])
  })
})

test("a term renamed into a namespace the query has no prefix for is written as a full IRI there", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const edit = await rename(uri, positionOf(text, "ex:worksFor", 5), "<https://elsewhere.example.org/employer>")
    assert.deepEqual(edited(edit)["people.rq"], ["9:12 <https://elsewhere.example.org/employer>"])
  })
})
