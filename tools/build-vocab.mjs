// Regenerates data/vocab.tsv from @zazuko/rdf-vocabularies (dev dependency).
// Format, UTF-8, one record per line, tab-separated:
//   @ns <TAB> prefix <TAB> namespace
//   term <TAB> local <TAB> kind(C|P|I) <TAB> flags(D=deprecated) <TAB> label <TAB> comment
// Newlines/tabs inside text are escaped as \n and \t.
import fs from "node:fs"
import path from "node:path"
import { createRequire } from "node:module"
const require = createRequire(import.meta.url)
const pkgDir = path.dirname(require.resolve("@zazuko/rdf-vocabularies"))
const prefixesMod = require("@zazuko/rdf-vocabularies/prefixes")
const prefixes = prefixesMod.default ?? prefixesMod.prefixes ?? prefixesMod

const RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
const RDFS = "http://www.w3.org/2000/01/rdf-schema#"
const OWL = "http://www.w3.org/2002/07/owl#"
const CLASS_TYPES = new Set([RDFS + "Class", OWL + "Class", RDFS + "Datatype", "http://www.w3.org/ns/shacl#NodeShape"])
const PROP_TYPES = new Set([RDF + "Property", OWL + "ObjectProperty", OWL + "DatatypeProperty", OWL + "AnnotationProperty",
  OWL + "FunctionalProperty", OWL + "InverseFunctionalProperty", OWL + "TransitiveProperty", OWL + "SymmetricProperty",
  OWL + "AsymmetricProperty", OWL + "ReflexiveProperty", OWL + "IrreflexiveProperty", OWL + "OntologyProperty"])
const LABELS = [RDFS + "label", "http://www.w3.org/2004/02/skos/core#prefLabel", "http://purl.org/dc/terms/title"]
const COMMENTS = [RDFS + "comment", "http://www.w3.org/2004/02/skos/core#definition", "http://purl.org/dc/terms/description", "http://purl.org/dc/elements/1.1/description"]

const unescape = (s) => s.replace(/\\(u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8}|.)/g, (_, e) =>
  e[0] === "u" || e[0] === "U" ? String.fromCodePoint(parseInt(e.slice(1), 16)) : ({ n: "\n", t: "\t", r: "\r", b: "\b", f: "\f" }[e] ?? e))
const clean = (s) => s.replace(/\s+/g, " ").trim()
const field = (s) => s.replace(/\\/g, "\\\\").replace(/\t/g, "\\t").replace(/\n/g, "\\n")
const LINE = /^<([^>]*)> <([^>]*)> (<([^>]*)>|_:\S+|"((?:[^"\\]|\\.)*)"(?:@([\w-]+)|\^\^<[^>]*>)?)(?: \S+)? \.$/

let out = ""
let total = 0
for (const [prefix, ns] of Object.entries(prefixes).sort()) {
  const file = path.join(pkgDir, "ontologies", `${prefix}.nq`)
  if (!fs.existsSync(file)) continue
  const terms = new Map()
  const get = (local) => terms.get(local) ?? terms.set(local, { types: new Set(), label: {}, comment: {}, dep: false, hint: "" }).get(local)
  for (const line of fs.readFileSync(file, "utf8").split("\n")) {
    const m = LINE.exec(line)
    if (!m || !m[1].startsWith(ns) || m[1].length === ns.length) continue
    const local = m[1].slice(ns.length)
    if (/[\s#/]/.test(local)) continue
    const t = get(local)
    const [, , p, , oIri, lit, lang] = m
    if (p === RDF + "type" && oIri) t.types.add(oIri)
    else if (p === RDFS + "subClassOf") t.hint ||= "C"
    else if (p === RDFS + "domain" || p === RDFS + "range" || p === RDFS + "subPropertyOf") t.hint ||= "P"
    else if (p === OWL + "deprecated" && lit === "true") t.dep = true
    else if (lit !== undefined) {
      const rank = !lang || lang === "en" || lang.startsWith("en-") ? 0 : 1
      for (const [list, key] of [[LABELS, "label"], [COMMENTS, "comment"]]) {
        if (list.includes(p) && (t[key].rank === undefined || rank < t[key].rank)) t[key] = { rank, text: clean(unescape(lit)) }
      }
    }
  }
  out += `@ns\t${prefix}\t${ns}\n`
  for (const [local, t] of [...terms].sort(([a], [b]) => a.localeCompare(b))) {
    const kind = [...t.types].some((x) => CLASS_TYPES.has(x)) ? "C" : [...t.types].some((x) => PROP_TYPES.has(x)) ? "P" : t.hint || (t.types.size ? "I" : /^[A-Z]/.test(local) ? "C" : "P")
    const comment = (t.comment.text ?? "").slice(0, 600)
    out += `term\t${field(local)}\t${kind}\t${t.dep ? "D" : ""}\t${field(t.label.text ?? "")}\t${field(comment)}\n`
    total++
  }
}
fs.writeFileSync(new URL("../data/vocab.tsv", import.meta.url), out)
console.log(`wrote ${total} terms, ${(out.length / 1e6).toFixed(1)} MB`)
