// Tokenizes Turtle with the extension's TextMate grammar using the same engine
// VS Code uses (vscode-textmate + oniguruma) and checks the scopes.
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
  loadGrammar: async () => vsctm.parseRawGrammar(fs.readFileSync(new URL("../syntaxes/turtle.tmLanguage.json", import.meta.url), "utf8"), "turtle.tmLanguage.json"),
})
const grammar = await registry.loadGrammar("source.turtle")

/** Map each token text on the given lines to its innermost scope. */
function scopes(src) {
  let state = vsctm.INITIAL
  const out = []
  for (const line of src.split("\n")) {
    const r = grammar.tokenizeLine(line, state)
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
