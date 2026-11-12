// The vocabulary list's generator as a pure function. It takes the text of
// the source manifest (data/vocab-sources.tsv), the text of the corrections
// (data/vocab-patches.tsv) and the bytes of each manifest row, and gives the
// text of data/vocab.tsv and the terms added and removed against a previous
// list. It reads no file and opens no connection: its caller does both.
//
// The list, UTF-8, one record per line, tab-separated:
//   @ns <TAB> prefix <TAB> namespace [<TAB> alias]...
//   term <TAB> local <TAB> kind(C|P|I) <TAB> flags(D=deprecated) <TAB> label <TAB> comment
//        <TAB> supers <TAB> domains <TAB> ranges
// Newlines, tabs and backslashes inside local, label and comment are escaped
// as \n, \t and \\. The last three columns are space-separated term lists
// (direct rdfs:subClassOf/subPropertyOf, rdfs:domain, rdfs:range;
// schema:domainIncludes/rangeIncludes count as domain/range), each term
// written compactly: a bare local name in the vocabulary's own namespace or
// one of its aliases, `prefix:local` in another vocabulary, else
// `<full IRI>`. Empty trailing columns are omitted.
//
// A frozen extract (format `tsv`) holds a vocabulary's `term` lines as the
// list writes them. A correction line is `add`, prefix, local, reason, then
// the columns of a `term` line from kind on.
import { createHash } from "node:crypto"
import { gzipSync } from "node:zlib"
import N3 from "n3"
import { RdfXmlParser } from "rdfxml-streaming-parser"

// The longest comment the list keeps, in UTF-16 code units. Comment text is
// what limits the list's size: this is the largest of 400, 300, 240, 200 and
// 160 that keeps it within SIZE_BUDGET. Measured on the pinned sources
// (2026-11-12), gzipped: 240 gives 790,582 bytes, 200 gives 759,927, 160
// gives 724,999.
export const COMMENT_CAP = 160
// The most data/vocab.tsv may weigh gzipped at level 9: its size before the
// sources were pinned.
export const SIZE_BUDGET = 733_090

const RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
const RDFS = "http://www.w3.org/2000/01/rdf-schema#"
const OWL = "http://www.w3.org/2002/07/owl#"
const SKOS = "http://www.w3.org/2004/02/skos/core#"
const SCHEMA_SPELLINGS = ["https://schema.org/", "http://schema.org/"]
const CLASS_TYPES = new Set([RDFS + "Class", OWL + "Class", OWL + "DeprecatedClass", RDFS + "Datatype", "http://www.w3.org/ns/shacl#NodeShape"])
const PROP_TYPES = new Set([RDF + "Property", OWL + "ObjectProperty", OWL + "DatatypeProperty", OWL + "AnnotationProperty",
  OWL + "FunctionalProperty", OWL + "InverseFunctionalProperty", OWL + "TransitiveProperty", OWL + "SymmetricProperty",
  OWL + "AsymmetricProperty", OWL + "ReflexiveProperty", OWL + "IrreflexiveProperty", OWL + "OntologyProperty", OWL + "DeprecatedProperty"])
const LABELS = new Set([RDFS + "label", SKOS + "prefLabel", "http://purl.org/dc/terms/title"])
const COMMENTS = new Set([RDFS + "comment", SKOS + "definition", "http://purl.org/dc/terms/description", "http://purl.org/dc/elements/1.1/description"])
const SUPER = new Set([RDFS + "subClassOf", RDFS + "subPropertyOf"])
const DOMAIN = new Set([RDFS + "domain", ...SCHEMA_SPELLINGS.map((ns) => ns + "domainIncludes")])
const RANGE = new Set([RDFS + "range", ...SCHEMA_SPELLINGS.map((ns) => ns + "rangeIncludes")])

// The deprecation markers: a term is deprecated when it is the subject of a
// triple whose predicate is a key here and whose object passes the test.
const isTrue = (o) => o.termType === "Literal" && o.value === "true"
const isIri = (iri) => (o) => o.termType === "NamedNode" && o.value === iri
const DEPRECATION = new Map([
  [OWL + "deprecated", isTrue],
  [RDF + "type", (o) => isIri(OWL + "DeprecatedClass")(o) || isIri(OWL + "DeprecatedProperty")(o)],
  ...SCHEMA_SPELLINGS.map((ns) => [ns + "supersededBy", () => true]),
  ["http://qudt.org/schema/qudt/deprecated", isTrue],
  ["http://metadataregistry.org/uri/profile/regap/status", isIri("http://metadataregistry.org/uri/RegStatus/1008")],
])

const FORMATS = new Set(["ttl", "nt", "rdfxml", "tsv"])
const MANIFEST_COLUMNS = ["prefix", "namespace", "aliases", "format", "url", "version", "licence", "sha256"]
const PREFIX = /^[A-Za-z][\w-]*$/
const SIMPLE = /^[A-Za-z_][\w.-]*$/
const isTermLocal = (local) => local.length > 0 && !/[\s#/]/.test(local)

const fail = (message) => {
  throw new Error(message)
}

// Code-point order: a UTF-16 surrogate stands for a code point above every
// other code unit, which plain `<` gets wrong for U+E000 to U+FFFF.
const byCodePoint = (a, b) => {
  const n = Math.min(a.length, b.length)
  for (let i = 0; i < n; i++) {
    const x = a.charCodeAt(i)
    const y = b.charCodeAt(i)
    if (x === y) continue
    const sx = x >= 0xd800 && x <= 0xdfff
    const sy = y >= 0xd800 && y <= 0xdfff
    return sx === sy ? x - y : sx ? 1 : -1
  }
  return a.length - b.length
}

// [line number, line] for each line that is neither blank nor a `#` comment.
const records = (text) =>
  text.split(/\r?\n/).flatMap((line, i) => (line.trim() === "" || line.startsWith("#") ? [] : [[i + 1, line]]))

const collapseSpaces = (s) => s.replace(/\s+/g, " ").trim()
const escapeColumn = (s) => s.replace(/\\/g, "\\\\").replace(/\t/g, "\\t").replace(/\n/g, "\\n")
const unescapeColumn = (s) => s.replace(/\\(.)/g, (_, e) => ({ t: "\t", n: "\n" })[e] ?? e)

// A sentence end: `.`, `!` or `?`, maybe closing quotes or brackets, then a
// space and a word that does not start in lower case (so "e.g. the" is not
// one). A `.` after an abbreviation is not one either: after a single letter
// ("e.g. Oil", "the U.S. Secretary", "William F. Smith", "p. 12") or after
// cf, vs, No or Inc. ("etc." ends a sentence as often as not, so it may.)
const SENTENCE_END = /(?:(?<!(?<![^ .(\[])(?:[Cc]f|[Vv]s|No|Inc|\p{L}))\.|[!?])["'’”)\]]*(?= +(?!\p{Ll}))/gu
// Whether the text stops inside a round or square bracket it opened. A
// closing bracket with nothing open (a smiley, a list item "a)") is ignored.
const insideBracket = (s) => {
  let depth = 0
  for (const c of s) {
    if (c === "(" || c === "[") depth++
    else if ((c === ")" || c === "]") && depth > 0) depth--
  }
  return depth > 0
}

// The first paragraph (schema.org writes its paragraph breaks as the
// characters `\n\n`, so those end it too), whitespace collapsed, at most COMMENT_CAP long: cut
// after the last sentence end inside the cap that is not inside a bracket,
// else after the last word with "…". The two refusals (abbreviations and
// brackets) can only miss a sentence end: the cut is then an earlier one or
// says "…".
const commentOf = (text) => {
  const comment = collapseSpaces(text.trim().split(/\n\s*\n|\\n\\n/)[0])
  if (comment.length <= COMMENT_CAP) return comment
  let end = 0
  for (const m of comment.matchAll(SENTENCE_END)) {
    const e = m.index + m[0].length
    if (e > COMMENT_CAP) break
    if (!insideBracket(comment.slice(0, e))) end = e
  }
  if (end > 0) return comment.slice(0, end)
  let cut = comment.lastIndexOf(" ", COMMENT_CAP - 1)
  if (cut <= 0) {
    cut = COMMENT_CAP - 1
    const c = comment.charCodeAt(cut - 1)
    if (c >= 0xd800 && c <= 0xdbff) cut--
  }
  return comment.slice(0, cut) + "…"
}

/**
 * Reads the source manifest. Returns one row per record, in file order:
 * `{ line, prefix, namespace, aliases, format, frozen, url, version, licence, sha256 }`,
 * `aliases` an array (empty for `-`), `frozen` true for a frozen extract
 * (format `tsv`). Throws, naming the line, on a row with the wrong number of
 * columns, an empty column, an unknown format, a fetched source whose url is
 * not https, or a malformed hash; and, naming both lines, on two prefixes
 * that share a namespace or an alias.
 */
export function parseManifest(text) {
  const rows = records(text).map(([line, record]) => {
    const where = `manifest line ${line}`
    const cols = record.split("\t")
    if (cols.length !== MANIFEST_COLUMNS.length) {
      fail(`${where}: ${cols.length} columns, expected ${MANIFEST_COLUMNS.length} (${MANIFEST_COLUMNS.join(", ")})`)
    }
    const empty = cols.findIndex((c) => c.trim() === "")
    if (empty >= 0) fail(`${where}: the ${MANIFEST_COLUMNS[empty]} column is empty`)
    const [prefix, namespace, aliases, format, url, version, licence, sha256] = cols
    if (!PREFIX.test(prefix)) fail(`${where}: "${prefix}" is not a prefix`)
    if (!FORMATS.has(format)) fail(`${where} (${prefix}): format "${format}" is not one of ${[...FORMATS].join(", ")}`)
    const frozen = format === "tsv"
    if (!frozen && !url.startsWith("https://")) fail(`${where} (${prefix}): a fetched source's url must be https: ${url}`)
    if (!/^[0-9a-f]{64}$/.test(sha256)) fail(`${where} (${prefix}): "${sha256}" is not a sha256 in lower-case hex`)
    return { line, prefix, namespace, aliases: aliases === "-" ? [] : aliases.split(" ").filter(Boolean), format, frozen, url, version, licence, sha256 }
  })
  // A namespace or alias belongs to one prefix: the list could not say whose
  // term an IRI under it is.
  const owners = new Map()
  for (const row of rows) {
    for (const ns of [row.namespace, ...row.aliases]) {
      const owner = owners.get(ns) ?? row
      if (owner.prefix !== row.prefix) fail(`manifest lines ${owner.line} and ${row.line}: ${owner.prefix} and ${row.prefix} share ${ns}`)
      owners.set(ns, owner)
    }
  }
  return rows
}

/**
 * Builds the text of data/vocab.tsv.
 *
 * - `manifest`: the manifest's text (see `parseManifest`).
 * - `corrections`: the corrections file's text.
 * - `sources`: the bytes of each row (a Uint8Array), in the order
 *   `parseManifest(manifest)` returns the rows: the downloaded file for a
 *   fetched row, the frozen extract for a `tsv` row.
 * - `previous`: the list to compare with, usually the committed one.
 * - `budget`: the most the output may weigh gzipped at level 9.
 *
 * Resolves to `{ text, summary, vocabularies, terms, gzipped }`: `summary`
 * holds one `{ prefix, added, removed }` (local names, code-point order) per
 * vocabulary whose terms differ from `previous`; the rest are the list's
 * counts of vocabularies and terms and its size gzipped at level 9. Rejects with a message naming the row
 * or line when a hash differs (checked for every row before any parsing), a
 * source does not parse or yields no term, a vocabulary's rows disagree, a
 * frozen extract or a correction is malformed or uses an unknown prefix, a
 * correction adds a term the source has, or the output is over the budget.
 * The same inputs give the same text.
 */
export async function buildVocab({ manifest, corrections = "", sources, previous = "", budget = SIZE_BUDGET }) {
  const rows = parseManifest(manifest)
  if (sources.length !== rows.length) fail(`${sources.length} sources for ${rows.length} manifest rows`)
  rows.forEach((row, i) => {
    const found = createHash("sha256").update(sources[i]).digest("hex")
    if (found !== row.sha256) fail(`manifest line ${row.line} (${row.prefix}): sha256 is ${found}, expected ${row.sha256}`)
  })

  const vocabs = vocabularies(rows)
  const namespaces = indexNamespaces(vocabs)
  for (const vocab of vocabs.values()) {
    const [first] = vocab.rows
    if (first.frozen) {
      vocab.terms = frozenTerms(decode(first, sources[rows.indexOf(first)]), vocab, namespaces, first)
      continue
    }
    const triples = new Map()
    for (const row of vocab.rows) {
      const quads = await parse(row, decode(row, sources[rows.indexOf(row)]))
      if (!quads.some((q) => q.subject.termType === "NamedNode" && localOf(vocab, q.subject.value) !== undefined)) {
        fail(`manifest line ${row.line} (${vocab.prefix}): the source has no term in ${[vocab.namespace, ...vocab.aliases].join(" or ")}`)
      }
      for (const q of quads) triples.set(`${ntriples(q.subject)} ${ntriples(q.predicate)} ${ntriples(q.object)}`, q)
    }
    // Triples in code-point order of their N-Triples form, so the first label
    // or hint seen does not depend on the source's order.
    const sorted = [...triples].sort(([a], [b]) => byCodePoint(a, b)).map(([, q]) => q)
    vocab.terms = fetchedTerms(sorted, vocab)
  }

  applyCorrections(corrections, vocabs, namespaces)

  let text = ""
  for (const vocab of [...vocabs.values()].sort((a, b) => byCodePoint(a.prefix, b.prefix))) {
    text += ["@ns", vocab.prefix, vocab.namespace, ...vocab.aliases].join("\t") + "\n"
    for (const term of [...vocab.terms.values()].sort((a, b) => byCodePoint(a.local, b.local))) {
      text += termLine(term, vocab, namespaces) + "\n"
    }
  }
  const gzipped = gzipSync(Buffer.from(text), { level: 9 }).length
  if (gzipped > budget) fail(`the output is ${gzipped} bytes gzipped, over its budget of ${budget}`)
  const terms = [...vocabs.values()].reduce((n, vocab) => n + vocab.terms.size, 0)
  return { text, summary: summarize(previous, text), vocabularies: vocabs.size, terms, gzipped }
}

// The vocabularies by prefix, each with its rows. Rows of one prefix must
// agree on namespace and aliases, and a frozen extract is a vocabulary's only row.
function vocabularies(rows) {
  const vocabs = new Map()
  for (const row of rows) {
    const vocab = vocabs.get(row.prefix)
    if (!vocab) {
      vocabs.set(row.prefix, { prefix: row.prefix, namespace: row.namespace, aliases: row.aliases, rows: [row], terms: new Map() })
      continue
    }
    const [first] = vocab.rows
    const disagree = (what) => fail(`manifest lines ${first.line} and ${row.line} (${row.prefix}) disagree: ${what}`)
    if (row.namespace !== first.namespace) disagree(`namespace ${first.namespace} or ${row.namespace}`)
    if (row.aliases.join(" ") !== first.aliases.join(" ")) disagree(`aliases ${first.aliases.join(" ") || "-"} or ${row.aliases.join(" ") || "-"}`)
    if (row.frozen || first.frozen) disagree("a frozen extract must be the vocabulary's only row")
    vocab.rows.push(row)
  }
  return vocabs
}

// Every spelling of every namespace, longest first, for writing and reading
// `prefix:local`.
function indexNamespaces(vocabs) {
  const spellings = [...vocabs.values()].flatMap((v) => [v.namespace, ...v.aliases].map((ns) => [v.prefix, ns]))
  spellings.sort(([pa, a], [pb, b]) => b.length - a.length || byCodePoint(pa, pb))
  return { spellings, byPrefix: new Map([...vocabs.values()].map((v) => [v.prefix, v.namespace])) }
}

function decode(row, bytes) {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    return fail(`manifest line ${row.line} (${row.prefix}): the source is not UTF-8`)
  }
}

async function parse(row, text) {
  try {
    if (row.format === "rdfxml") return await parseRdfXml(text, row.url)
    return new N3.Parser({ format: row.format === "nt" ? "N-Triples" : "text/turtle", baseIRI: row.url }).parse(text)
  } catch (e) {
    return fail(`manifest line ${row.line} (${row.prefix}): ${row.format} does not parse: ${e.message}`)
  }
}

function parseRdfXml(text, baseIRI) {
  return new Promise((resolve, reject) => {
    const quads = []
    new RdfXmlParser({ baseIRI })
      .on("data", (q) => quads.push(q))
      .on("error", reject)
      .on("end", () => resolve(quads))
      .end(text)
  })
}

// A term's N-Triples form, for ordering and deduplicating triples.
function ntriples(term) {
  switch (term.termType) {
    case "NamedNode":
      return `<${term.value}>`
    case "BlankNode":
      return `_:${term.value}`
    case "Literal": {
      const text = term.value.replace(/[\\"\n\r\t]/g, (c) => ({ "\\": "\\\\", '"': '\\"', "\n": "\\n", "\r": "\\r", "\t": "\\t" })[c])
      const tag = term.language ? "@" + term.language
        : term.datatype.value === "http://www.w3.org/2001/XMLSchema#string" ? "" : `^^<${term.datatype.value}>`
      return `"${text}"${tag}`
    }
    default:
      return `<<${ntriples(term.subject)} ${ntriples(term.predicate)} ${ntriples(term.object)}>>`
  }
}

// The local name of an IRI in the vocabulary's namespace or an alias, when it
// is a term's.
function localOf(vocab, iri) {
  const ns = [vocab.namespace, ...vocab.aliases].filter((n) => iri.length > n.length && iri.startsWith(n))
    .sort((a, b) => b.length - a.length)[0]
  if (ns === undefined) return undefined
  const local = iri.slice(ns.length)
  return isTermLocal(local) ? local : undefined
}

function fetchedTerms(triples, vocab) {
  const found = new Map()
  for (const { subject, predicate, object } of triples) {
    if (subject.termType !== "NamedNode") continue
    const local = localOf(vocab, subject.value)
    if (local === undefined) continue
    let t = found.get(local)
    if (!t) {
      t = { types: new Set(), label: {}, comment: {}, dep: false, hint: "", supers: new Set(), domains: new Set(), ranges: new Set() }
      found.set(local, t)
    }
    const p = predicate.value
    const iri = object.termType === "NamedNode" ? object.value : undefined
    if (DEPRECATION.get(p)?.(object)) t.dep = true
    if (iri && SUPER.has(p)) t.supers.add(iri)
    if (iri && DOMAIN.has(p)) t.domains.add(iri)
    if (iri && RANGE.has(p)) t.ranges.add(iri)
    if (p === RDF + "type" && iri) t.types.add(iri)
    else if (p === RDFS + "subClassOf") t.hint ||= "C"
    else if (p === RDFS + "domain" || p === RDFS + "range" || p === RDFS + "subPropertyOf") t.hint ||= "P"
    else if (object.termType === "Literal") {
      const lang = object.language
      const rank = !lang || lang === "en" || lang.startsWith("en-") ? 0 : 1
      for (const [set, k] of [[LABELS, "label"], [COMMENTS, "comment"]]) {
        if (set.has(p) && (t[k].rank === undefined || rank < t[k].rank)) t[k] = { rank, text: object.value }
      }
    }
  }
  const terms = new Map()
  for (const [local, t] of found) {
    const types = [...t.types]
    const kind = types.some((x) => CLASS_TYPES.has(x)) ? "C"
      : types.some((x) => PROP_TYPES.has(x)) ? "P"
      : t.hint || (types.length ? "I" : /^[A-Z]/.test(local) ? "C" : "P")
    terms.set(local, { local, kind, dep: t.dep, label: collapseSpaces(t.label.text ?? ""), comment: t.comment.text ?? "",
      supers: t.supers, domains: t.domains, ranges: t.ranges })
  }
  return terms
}

// The columns of a `term` line from local on, read back into a term.
function readTerm(cols, vocab, namespaces, where) {
  if (cols.length < 2 || cols.length > 8) fail(`${where}: ${cols.length} term columns, expected 2 to 8 (local, kind, flags, label, comment, supers, domains, ranges)`)
  const [local, kind, flags = "", label = "", comment = "", supers = "", domains = "", ranges = ""] = cols
  const name = unescapeColumn(local)
  if (!isTermLocal(name)) fail(`${where}: "${local}" is not a local name`)
  if (!["C", "P", "I"].includes(kind)) fail(`${where}: kind "${kind}" is not C, P or I`)
  if (flags !== "" && flags !== "D") fail(`${where}: flags "${flags}" are not empty or D`)
  const iris = (list) => new Set(list.split(" ").filter(Boolean).map((token) => expand(token, vocab, namespaces, where)))
  return { local: name, kind, dep: flags === "D", label: unescapeColumn(label), comment: unescapeColumn(comment),
    supers: iris(supers), domains: iris(domains), ranges: iris(ranges) }
}

function expand(token, vocab, namespaces, where) {
  if (token.startsWith("<") && token.endsWith(">")) return token.slice(1, -1)
  const colon = token.indexOf(":")
  if (colon < 0) return vocab.namespace + token
  const prefix = token.slice(0, colon)
  const ns = namespaces.byPrefix.get(prefix)
  if (ns === undefined) fail(`${where}: unknown prefix "${prefix}" in ${token}`)
  return ns + token.slice(colon + 1)
}

function frozenTerms(text, vocab, namespaces, row) {
  const terms = new Map()
  for (const [line, record] of records(text)) {
    const where = `frozen extract of ${vocab.prefix} (manifest line ${row.line}), line ${line}`
    const [tag, ...cols] = record.split("\t")
    if (tag !== "term") fail(`${where}: not a term line`)
    const term = readTerm(cols, vocab, namespaces, where)
    if (terms.has(term.local)) fail(`${where}: ${term.local} is there twice`)
    terms.set(term.local, term)
  }
  if (terms.size === 0) fail(`manifest line ${row.line} (${vocab.prefix}): the frozen extract has no term`)
  return terms
}

function applyCorrections(text, vocabs, namespaces) {
  const added = new Map()
  for (const [line, record] of records(text)) {
    const where = `corrections line ${line}`
    const [action, prefix, local, reason, ...cols] = record.split("\t")
    if (action !== "add") fail(`${where}: action "${action}" is not add`)
    const vocab = vocabs.get(prefix)
    if (!vocab) fail(`${where}: unknown prefix "${prefix}"`)
    if (!reason?.trim()) fail(`${where}: no reason`)
    const term = readTerm([local, ...cols], vocab, namespaces, where)
    const id = `${prefix}:${term.local}`
    if (added.has(id)) fail(`${where}: ${id} is already added on line ${added.get(id)}`)
    if (vocab.terms.has(term.local)) fail(`${where}: ${id} is already in the source (manifest line ${vocab.rows[0].line}); delete the correction`)
    added.set(id, line)
    vocab.terms.set(term.local, term)
  }
}

function compact(iri, vocab, namespaces) {
  for (const ns of [vocab.namespace, ...vocab.aliases]) {
    if (iri.startsWith(ns) && SIMPLE.test(iri.slice(ns.length))) return iri.slice(ns.length)
  }
  for (const [prefix, ns] of namespaces.spellings) {
    if (iri.startsWith(ns) && SIMPLE.test(iri.slice(ns.length))) return `${prefix}:${iri.slice(ns.length)}`
  }
  return `<${iri}>`
}

function termLine(term, vocab, namespaces) {
  const list = (set) => [...new Set([...set].map((iri) => compact(iri, vocab, namespaces)))].sort(byCodePoint).join(" ")
  const cols = [escapeColumn(term.local), term.kind, term.dep ? "D" : "", escapeColumn(term.label), escapeColumn(commentOf(term.comment)),
    list(term.supers), list(term.domains), list(term.ranges)]
  while (cols.length > 5 && cols[cols.length - 1] === "") cols.pop()
  return `term\t${cols.join("\t")}`
}

// The local names of each vocabulary of a list.
function termsByPrefix(text) {
  const vocabs = new Map()
  let current
  for (const line of text.split(/\r?\n/)) {
    const [tag, name] = line.split("\t")
    if (tag === "@ns") vocabs.set(name, (current = new Set()))
    else if (tag === "term" && current) current.add(name)
  }
  return vocabs
}

function summarize(previous, text) {
  const before = termsByPrefix(previous)
  const after = termsByPrefix(text)
  const prefixes = [...new Set([...before.keys(), ...after.keys()])].sort(byCodePoint)
  return prefixes.flatMap((prefix) => {
    const old = before.get(prefix) ?? new Set()
    const now = after.get(prefix) ?? new Set()
    const added = [...now].filter((l) => !old.has(l)).sort(byCodePoint)
    const removed = [...old].filter((l) => !now.has(l)).sort(byCodePoint)
    return added.length || removed.length ? [{ prefix, added, removed }] : []
  })
}
