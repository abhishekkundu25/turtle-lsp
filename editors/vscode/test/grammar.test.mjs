// Tokenizes Turtle, TriG and N-Triples with the extension's TextMate grammars
// using the same engine VS Code uses (vscode-textmate + oniguruma) and checks
// the scopes. The TriG and N-Triples grammars include rules from Turtle's.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import { createRequire } from "node:module"
import vsctm from "vscode-textmate"
import oniguruma from "vscode-oniguruma"

const require = createRequire(import.meta.url)
const wasm = fs.readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm"))
await oniguruma.loadWASM(wasm.buffer.slice(wasm.byteOffset, wasm.byteOffset + wasm.byteLength))
const registry = new vsctm.Registry({
  onigLib: Promise.resolve({ createOnigScanner: (p) => new oniguruma.OnigScanner(p), createOnigString: (s) => new oniguruma.OnigString(s) }),
  loadGrammar: async (scope) => {
    const file = { "source.turtle": "turtle", "source.trig": "trig", "source.ntriples": "ntriples" }[scope]
    if (!file) return null
    const name = `${file}.tmLanguage.json`
    return vsctm.parseRawGrammar(fs.readFileSync(new URL(`../syntaxes/${name}`, import.meta.url), "utf8"), name)
  },
})
const grammar = await registry.loadGrammar("source.turtle")
const trig = await registry.loadGrammar("source.trig")
const ntriples = await registry.loadGrammar("source.ntriples")

/** Map each token text on the given lines to its innermost scope. */
function scopes(src, g = grammar) {
  let state = vsctm.INITIAL
  const out = []
  for (const line of src.split("\n")) {
    const r = g.tokenizeLine(line, state)
    for (const t of r.tokens) {
      const text = line.slice(t.startIndex, t.endIndex)
      if (text.trim()) out.push([text, t.scopes.at(-1)])
    }
    state = r.ruleStack
  }
  return out
}
const scopeOf = (toks, text) => toks.find(([t]) => t === text)?.[1]

test("prefix directive", () => {
  const t = scopes("@prefix ex: <http://example.org/> .")
  assert.equal(scopeOf(t, "@prefix"), "keyword.other.directive.prefix.turtle")
  assert.equal(scopeOf(t, "ex"), "entity.name.namespace.turtle")
  assert.equal(scopeOf(t, "<http://example.org/>"), "markup.underline.link.iri.turtle")
})

test("triple with prefixed names, a, literal, langtag, datatype", () => {
  const t = scopes('ex:alice a foaf:Person ; foaf:name "Alice"@en ; ex:age "3"^^xsd:integer .')
  assert.equal(scopeOf(t, "a"), "keyword.other.rdf-type.turtle")
  assert.equal(scopeOf(t, "foaf"), "entity.name.namespace.turtle")
  assert.equal(scopeOf(t, "Person"), "entity.name.type.local-name.turtle")
  assert.equal(scopeOf(t, "@en"), "constant.language.language-tag.turtle")
  assert.equal(scopeOf(t, "^^"), "keyword.operator.datatype.turtle")
})

test("long strings span lines and hide what is inside", () => {
  const t = scopes('ex:s ex:p """first\nfoo:bar # not a comment\nend""" .')
  assert.equal(scopeOf(t, "foo:bar # not a comment"), "string.quoted.triple.double.turtle")
})

test("comments, numbers, booleans, blank nodes", () => {
  const t = scopes("_:b1 ex:n -1.5e3 , 42 , true . # done")
  assert.equal(scopeOf(t, "_:b1"), "variable.other.blank-node.turtle")
  assert.equal(scopeOf(t, "-1.5e3"), "constant.numeric.turtle")
  assert.equal(scopeOf(t, "42"), "constant.numeric.turtle")
  assert.equal(scopeOf(t, "true"), "constant.language.boolean.turtle")
  assert.equal(scopeOf(t, "# done"), "comment.line.number-sign.turtle")
})

test("RDF 1.2 tokens", () => {
  const t = scopes("ex:a ex:p ex:o ~ ex:r {| ex:src ex:s |} .\nex:x ex:y <<( ex:a ex:p ex:o )>> .")
  assert.equal(scopeOf(t, "~"), "keyword.operator.reifier.turtle")
  assert.equal(scopeOf(t, "{|"), "punctuation.section.annotation.turtle")
  assert.equal(scopeOf(t, "<<("), "punctuation.section.triple-term.turtle")
})

test("TriG: graph blocks, GRAPH, and Turtle inside", () => {
  const t = scopes('GRAPH ex:g {\n  ex:s a ex:B ; ex:p "x { y }"@en {| ex:src ex:s |} .\n}\nex:h { ex:a ex:p graph:x }', trig)
  assert.equal(scopeOf(t, "GRAPH"), "keyword.other.graph.trig")
  assert.equal(scopeOf(t, "{"), "punctuation.section.graph.trig")
  assert.equal(scopeOf(t, "}"), "punctuation.section.graph.trig")
  assert.equal(scopeOf(t, "a"), "keyword.other.rdf-type.turtle")
  assert.equal(scopeOf(t, "ex"), "entity.name.namespace.turtle")
  assert.equal(scopeOf(t, "{|"), "punctuation.section.annotation.turtle")
  assert.equal(scopeOf(t, "|}"), "punctuation.section.annotation.turtle")
  assert.equal(scopeOf(t, "@en"), "constant.language.language-tag.turtle")
  // Braces inside a string are string; `graph:` is a prefix, not the keyword.
  assert.ok(t.some(([text, scope]) => text.includes("x { y }") && scope.startsWith("string.")), JSON.stringify(t))
  assert.equal(scopeOf(t, "graph"), "entity.name.namespace.turtle")
})

test("TriG: directives and comments come from Turtle", () => {
  const t = scopes("@prefix ex: <http://example.org/> . # graphs follow\n{ ex:a ex:p 1 }", trig)
  assert.equal(scopeOf(t, "@prefix"), "keyword.other.directive.prefix.turtle")
  assert.equal(scopeOf(t, "# graphs follow"), "comment.line.number-sign.turtle")
  assert.equal(scopeOf(t, "1"), "constant.numeric.turtle")
})

test("N-Triples: IRIs, blank nodes, literals, triple terms", () => {
  const t = scopes('<http://e/s> <http://e/p> "v"@en-GB . # c\n_:b1 <http://e/p> "1"^^<http://www.w3.org/2001/XMLSchema#integer> .\n<http://e/s> <http://e/p> <<( _:b1 <http://e/p> "o" )>> .', ntriples)
  assert.equal(scopeOf(t, "<http://e/s>"), "markup.underline.link.iri.turtle")
  assert.equal(scopeOf(t, "_:b1"), "variable.other.blank-node.turtle")
  assert.equal(scopeOf(t, "@en-GB"), "constant.language.language-tag.turtle")
  assert.equal(scopeOf(t, "^^"), "keyword.operator.datatype.turtle")
  assert.equal(scopeOf(t, "# c"), "comment.line.number-sign.turtle")
  assert.equal(scopeOf(t, "<<("), "punctuation.section.triple-term.ntriples")
  assert.equal(scopeOf(t, ")>>"), "punctuation.section.triple-term.ntriples")
  assert.equal(scopeOf(t, "."), "punctuation.terminator.statement.ntriples")
})

test("N-Triples: Turtle-only syntax is not highlighted as such", () => {
  const t = scopes("ex:a a ex:B ;", ntriples)
  assert.equal(scopeOf(t, "a"), undefined)
  assert.equal(scopeOf(t, ";"), undefined)
})

test("every contributed language has a grammar, and the client serves it", () => {
  const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"))
  const extension = fs.readFileSync(new URL("../src/extension.ts", import.meta.url), "utf8")
  const ids = pkg.contributes.languages.map((l) => l.id)
  assert.deepEqual(ids, ["turtle", "trig", "ntriples"])
  for (const id of ids) {
    const g = pkg.contributes.grammars.find((x) => x.language === id)
    assert.ok(g && fs.existsSync(new URL(`../${g.path}`, import.meta.url)), `no grammar for ${id}`)
    assert.ok(extension.includes(`language: "${id}"`), `documentSelector misses ${id}`)
  }
  assert.ok(pkg.activationEvents.some((e) => e.includes("trig") && e.includes("nt")))
})
