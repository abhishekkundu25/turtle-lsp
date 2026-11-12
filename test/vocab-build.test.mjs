// The vocabulary list's generator (tools/vocab-lib.mjs) on small sources
// under test/vocab/: formats, deprecation markers, comments, what a term is,
// order, aliases, corrections, failures, and a second run. Offline.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import { createHash } from "node:crypto"
import { buildVocab, COMMENT_CAP } from "../tools/vocab-lib.mjs"

const EX = "http://example.org/ns#"
const OTHER_NS = "http://example.org/other#"
const source = (name) => fs.readFileSync(new URL(`./vocab/${name}`, import.meta.url))
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex")

// A manifest row; `bytes` is what the caller hands the library for it.
const row = (prefix, format, bytes, { namespace = EX, aliases = "-" } = {}) => ({ prefix, format, bytes, namespace, aliases })
const OTHER = row("other", "tsv", source("other.tsv"), { namespace: OTHER_NS })

// Line 1 is a comment, so the first row is on manifest line 2.
const manifestOf = (rows) =>
  ["# prefix\tnamespace\taliases\tformat\turl\tversion\tlicence\tsha256",
    ...rows.map((r) => [r.prefix, r.namespace, r.aliases, r.format, `https://example.org/sources/${r.prefix}`, "1.0", "CC0-1.0", sha256(r.bytes)].join("\t"))]
    .join("\n") + "\n"
const build = (rows, options = {}) => buildVocab({ manifest: manifestOf(rows), sources: rows.map((r) => r.bytes), ...options })

// The `term` lines of one vocabulary of a list.
const termLines = (text, prefix) => {
  const lines = text.split("\n")
  const start = lines.findIndex((l) => l.split("\t")[0] === "@ns" && l.split("\t")[1] === prefix)
  assert.ok(start >= 0, `no @ns line for ${prefix}`)
  const end = lines.findIndex((l, i) => i > start && !l.startsWith("term\t"))
  return lines.slice(start + 1, end)
}
const termOf = (text, prefix, local) => termLines(text, prefix).find((l) => l.split("\t")[1] === local)
const locals = (text, prefix) => termLines(text, prefix).map((l) => l.split("\t")[1])
const flagged = (text, prefix) => termLines(text, prefix).filter((l) => l.split("\t")[3] === "D").map((l) => l.split("\t")[1])

// N-Triples for a vocabulary whose terms carry only an rdfs:comment.
const commented = (comments) =>
  Buffer.from(Object.entries(comments).map(([local, text]) =>
    `<${EX}${local}> <http://www.w3.org/2000/01/rdf-schema#comment> ${JSON.stringify(text)} .\n`).join(""))
const commentOf = (text, local) => termOf(text, "ex", local).split("\t")[5]

const ANIMALS = [
  "term\tAnimal\tC\t\tanimal\tA living thing that moves.\tother:Base",
  "term\tCat\tC\t\t\t",
  "term\tDog\tC\t\tdog\t\tAnimal",
  "term\tPuppy\tC\t\t\t\tDog",
  "term\tage\tP\t\t\t\t\tAnimal",
  "term\tlegs\tP\t\t\t",
  "term\towner\tP\t\towner\tThe one who owns it.\t\tDog\t<http://example.org/people/Person> other:Agent",
  "term\trex\tI\t\tRex\t",
]

test("one vocabulary as Turtle, N-Triples, RDF/XML and a frozen extract gives the same term lines", async () => {
  for (const [format, file] of [["ttl", "animals.ttl"], ["nt", "animals.nt"], ["rdfxml", "animals.rdf"], ["tsv", "animals.tsv"]]) {
    const { text } = await build([row("ex", format, source(file)), OTHER])
    assert.deepEqual(termLines(text, "ex"), ANIMALS, format)
  }
})

test("each deprecation marker flags its term, and a term with none stays unflagged", async () => {
  const { text } = await build([row("ex", "ttl", source("deprecated.ttl"))])
  assert.deepEqual(flagged(text, "ex"), ["OldClass", "OldHttp", "OldHttps", "OldUnit", "oldOwl", "oldProperty", "oldRda"])
  for (const local of ["Current", "published", "notDeprecated"]) {
    assert.equal(termOf(text, "ex", local).split("\t")[3], "", local)
  }
  // The deprecated OWL types still give a class and a property.
  assert.equal(termOf(text, "ex", "OldClass").split("\t")[2], "C")
  assert.equal(termOf(text, "ex", "oldProperty").split("\t")[2], "P")
})

test("a frozen extract keeps its flags", async () => {
  const { text } = await build([row("ex", "tsv", source("deprecated.tsv"))])
  assert.deepEqual(termLines(text, "ex"), ["term\tCurrent\tC\t\t\t", "term\tOld\tC\tD\t\t"])
})

test("a comment is its first paragraph, whitespace collapsed, cut at a sentence or a word inside the cap", async () => {
  const sentence = "This is a sentence."
  const sentences = Array(Math.ceil(COMMENT_CAP / sentence.length) + 2).fill(sentence).join(" ")
  const words = Array(COMMENT_CAP).fill("word").join(" ")
  const abbreviated = `Use one, e.g. the ${words}`
  const { text } = await build([row("ex", "nt", commented({
    short: "A short comment. It stays as it is!",
    paragraphs: "  One  line\n   continues.\n\n  A second paragraph is dropped.",
    sentences,
    words,
    abbreviated,
    longWord: "x".repeat(COMMENT_CAP * 2),
  }))])

  assert.equal(commentOf(text, "short"), "A short comment. It stays as it is!")
  assert.equal(commentOf(text, "paragraphs"), "One line continues.")

  // Whole sentences, as many as fit.
  const fit = Math.floor((COMMENT_CAP + 1) / (sentence.length + 1))
  assert.equal(commentOf(text, "sentences"), Array(fit).fill(sentence).join(" "))

  // No sentence end inside the cap: the last whole word, then "…".
  assert.equal(commentOf(text, "words"), Array(Math.floor(COMMENT_CAP / 5)).fill("word").join(" ") + "…")
  // "e.g. the" is not a sentence end.
  const cut = commentOf(text, "abbreviated")
  assert.ok(cut.endsWith("word…") && cut.length <= COMMENT_CAP, cut)
  assert.ok(abbreviated.startsWith(cut.slice(0, -1) + " "), cut)
  assert.equal(commentOf(text, "longWord"), "x".repeat(COMMENT_CAP - 1) + "…")
})

test("a comment is not cut after an abbreviation or inside a bracket", async () => {
  const words = Array(COMMENT_CAP).fill("word").join(" ")
  // Each is followed by a word that does not start in lower case, as a new
  // sentence would be.
  const abbreviations = ["e.g. Oil", "E.g. Oil", "i.e. SKUs", "Decree No. 12", "the U.S. Secretary", "one vs. Another",
    "Example Inc. Then", "cf. Section 2", "Jane Q. Example", "Example e. V. Then", "p. 12", "(e.g. Oil)", "(Ital. Ente)", "[Fagus spp. (Fagaceae)]"]
  const comments = {}
  abbreviations.forEach((a, i) => {
    comments[`after${i}`] = `A whole sentence. Then ${a} and ${words}`
    comments[`only${i}`] = `With ${a} and ${words}`
  })
  // A bracket that closed stops nothing, and "?" and "!" end a sentence after any word.
  comments.closed = `One (e.g. this) is whole. Then (as before) another. Next ${words}`
  comments.question = `Is it in the U.S? Is it at No! Next ${words}`
  comments.bracketed = `The material used. (E.g. Oil, ${words})`
  // "etc." ends a sentence when a capital follows; a stray ")" opens nothing.
  comments.etc = `Lumens, watts, etc. This is the rest. Then ${words}`
  comments.smiley = `Smile :) (note. Then) and ${words}`
  // schema.org writes its paragraph breaks as the characters \n\n.
  comments.paragraphs = "The first paragraph.\\n\\nRelated: " + words
  const { text } = await build([row("ex", "nt", commented(comments))])

  abbreviations.forEach((a, i) => {
    // The sentence before the abbreviation's, when there is one.
    assert.equal(commentOf(text, `after${i}`), "A whole sentence.", a)
    // Else the last whole word inside the cap, marked as a cut.
    const cut = commentOf(text, `only${i}`)
    assert.ok(cut.startsWith(`With ${a} and word`) && cut.endsWith("word…") && cut.length <= COMMENT_CAP, `${a}: ${cut}`)
  })
  assert.equal(commentOf(text, "closed"), "One (e.g. this) is whole. Then (as before) another.")
  assert.equal(commentOf(text, "question"), "Is it in the U.S? Is it at No!")
  assert.equal(commentOf(text, "bracketed"), "The material used.")
  assert.equal(commentOf(text, "etc"), "Lumens, watts, etc. This is the rest.")
  assert.ok(commentOf(text, "smiley").endsWith("word…"), commentOf(text, "smiley"))
  assert.equal(commentOf(text, "paragraphs"), "The first paragraph.")
})

test("a subject in an alias namespace is a term; a local name with whitespace, # or / is not", async () => {
  const { text } = await build([row("ex", "ttl", source("terms.ttl"), { aliases: "https://example.org/ns#" })])
  assert.deepEqual(locals(text, "ex"), ["plain", "viaAlias"])
  // A reference in the alias namespace is written as one in the vocabulary.
  assert.equal(termOf(text, "ex", "viaAlias"), "term\tviaAlias\tP\t\tvia the alias\t\t\t\tplain")
})

test("the order of a source's triples changes nothing", async () => {
  // Two labels of one rank, and both a subClassOf and a domain: the choice is
  // the first in the triples' code-point order, as in the old package's sorted files.
  const triples = [
    `<${EX}both> <http://www.w3.org/2000/01/rdf-schema#label> "beta" .`,
    `<${EX}both> <http://www.w3.org/2000/01/rdf-schema#label> "alpha" .`,
    `<${EX}both> <http://www.w3.org/2000/01/rdf-schema#subClassOf> <${EX}Base> .`,
    `<${EX}both> <http://www.w3.org/2000/01/rdf-schema#domain> <${EX}Base> .`,
  ]
  const lines = async (order) => termLines((await build([row("ex", "nt", Buffer.from(order.join("\n") + "\n"))])).text, "ex")
  const expected = ["term\tboth\tP\t\talpha\t\tBase\tBase"]
  assert.deepEqual(await lines(triples), expected)
  assert.deepEqual(await lines([...triples].reverse()), expected)
})

test("a vocabulary with two source rows is the union of both", async () => {
  const { text } = await build([row("ex", "ttl", source("part1.ttl")), row("ex", "ttl", source("part2.ttl"))])
  assert.deepEqual(termLines(text, "ex"), [
    "term\tFirst\tC\t\tfirst\t",
    "term\tSecond\tC\t\t\t",
    "term\tShared\tC\t\tshared\t",
  ])
})

test("terms and vocabularies come out in code-point order, not the locale's", async () => {
  const names = ["éclair", "apple", "Zebra"]
  const nt = Buffer.from(names.map((n) => `<${EX}${n}> <http://www.w3.org/2000/01/rdf-schema#label> "${n}" .\n`).join(""))
  const upper = row("Ex", "tsv", Buffer.from("term\tThing\tC\t\t\n"), { namespace: "http://example.org/upper#" })
  const { text } = await build([row("ex", "nt", nt), upper])
  assert.deepEqual(locals(text, "ex"), ["Zebra", "apple", "éclair"])
  assert.notDeepEqual([...names].sort((a, b) => a.localeCompare(b, "en")), ["Zebra", "apple", "éclair"])
  const prefixes = text.split("\n").filter((l) => l.startsWith("@ns\t")).map((l) => l.split("\t")[1])
  assert.deepEqual(prefixes, ["Ex", "ex"])
  assert.notDeepEqual(["ex", "Ex"].sort((a, b) => a.localeCompare(b, "en")), ["Ex", "ex"])
})

test("the @ns line carries the aliases as further fields, and has two fields when there are none", async () => {
  const aliased = row("ex", "ttl", source("terms.ttl"), { aliases: "https://example.org/ns# http://example.org/old#" })
  const { text } = await build([aliased, OTHER])
  const ns = text.split("\n").filter((l) => l.startsWith("@ns\t"))
  assert.deepEqual(ns, [`@ns\tex\t${EX}\thttps://example.org/ns#\thttp://example.org/old#`, `@ns\tother\t${OTHER_NS}`])
})

test("an add correction puts its term in place with every column", async () => {
  const corrections = [
    "# action\tprefix\tlocal\treason\tkind\tflags\tlabel\tcomment\tsupers\tdomains\tranges",
    "add\tex\tHorse\tThe example specification, section 1, defines it; the source lacks it.\tC\tD\thorse\tA large animal. It runs.\tother:Base Animal\t<http://example.org/people/Person>\tex:Dog",
  ].join("\n")
  const { text } = await build([row("ex", "ttl", source("animals.ttl")), OTHER], { corrections })
  assert.deepEqual(locals(text, "ex"), ["Animal", "Cat", "Dog", "Horse", "Puppy", "age", "legs", "owner", "rex"])
  assert.equal(termOf(text, "ex", "Horse"),
    "term\tHorse\tC\tD\thorse\tA large animal. It runs.\tAnimal other:Base\t<http://example.org/people/Person>\tDog")
})

test("each failure stops the build and names the row or line", async (t) => {
  const animals = row("ex", "ttl", source("animals.ttl"))
  const cases = [
    ["a hash that differs, refused before parsing",
      () => buildVocab({ manifest: manifestOf([animals]), sources: [Buffer.from("not Turtle at all {")] }),
      /^manifest line 2 \(ex\): sha256 is [0-9a-f]{64}, expected [0-9a-f]{64}$/],
    ["a source with no term",
      () => build([row("ex", "ttl", source("empty.ttl"))]),
      /^manifest line 2 \(ex\): the source has no term in http:\/\/example\.org\/ns#$/],
    ["rows that disagree",
      () => build([animals, row("ex", "ttl", source("part1.ttl"), { namespace: "http://example.org/ns/" })]),
      /^manifest lines 2 and 3 \(ex\) disagree: namespace/],
    ["two prefixes with one namespace",
      () => build([animals, row("again", "ttl", source("part1.ttl"))]),
      /^manifest lines 2 and 3: ex and again share http:\/\/example\.org\/ns#$/],
    ["an alias that is another vocabulary's namespace",
      () => build([animals, OTHER, row("third", "ttl", source("part1.ttl"), { namespace: "http://example.org/third#", aliases: `http://example.org/old# ${OTHER_NS}` })]),
      /^manifest lines 3 and 4: other and third share http:\/\/example\.org\/other#$/],
    ["an alias that is another vocabulary's alias",
      () => build([row("ex", "ttl", source("animals.ttl"), { aliases: "http://example.org/old#" }),
        row("other", "tsv", source("other.tsv"), { namespace: OTHER_NS, aliases: "http://example.org/old#" })]),
      /^manifest lines 2 and 3: ex and other share http:\/\/example\.org\/old#$/],
    ["a correction with an unknown prefix",
      () => build([animals, OTHER], { corrections: "# comment\nadd\tnope\tThing\tSome reason.\tC\n" }),
      /^corrections line 2: unknown prefix "nope"$/],
    ["a correction for a term the source has",
      () => build([animals, OTHER], { corrections: "add\tex\tDog\tSome reason.\tC\n" }),
      /^corrections line 1: ex:Dog is already in the source \(manifest line 2\)/],
    ["a frozen extract that uses an unknown prefix",
      () => build([row("ex", "tsv", source("unknown-prefix.tsv"))]),
      /^frozen extract of ex \(manifest line 2\), line 2: unknown prefix "nowhere" in nowhere:Base$/],
    ["an output over the budget",
      () => build([animals, OTHER], { budget: 100 }),
      /^the output is \d+ bytes gzipped, over its budget of 100$/],
  ]
  for (const [name, run, message] of cases) {
    await t.test(name, () => assert.rejects(run, { message }))
  }
})

test("the same inputs give the same bytes, and the summary lists added and removed terms per vocabulary", async () => {
  const rows = [row("ex", "rdfxml", source("animals.rdf")), OTHER]
  const first = await build(rows)
  const second = await build(rows, { previous: first.text })
  assert.equal(second.text, first.text)
  assert.deepEqual(second.summary, [])

  // Before: ex had Gone and not Cat, other was not there, vanished was.
  const previous = first.text.slice(0, first.text.indexOf("@ns\tother\t"))
    .replace("term\tCat\tC\t\t\t\n", "term\tGone\tC\t\t\t\n") +
    "@ns\tvanished\thttp://example.org/vanished#\nterm\tOld\tC\t\t\t\n"
  const { summary } = await build(rows, { previous })
  assert.deepEqual(summary, [
    { prefix: "ex", added: ["Cat"], removed: ["Gone"] },
    { prefix: "other", added: ["Agent", "Base"], removed: [] },
    { prefix: "vanished", added: [], removed: ["Old"] },
  ])
})
