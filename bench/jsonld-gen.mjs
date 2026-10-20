// A large synthetic JSON-LD document for the performance budgets: one
// @graph of people with an inline @context (no remote contexts, so it parses
// offline), using the features real files lean on: compact IRIs, @type,
// @id references, typed and language-tagged values, nested blank nodes and
// @list.
//
//   import { bigJsonLd } from "./jsonld-gen.mjs"
//   const text = bigJsonLd(30000)          // about 30,000 lines
//
//   node bench/jsonld-gen.mjs [lines=30000] [out.jsonld]   // stdout without out
import fs from "node:fs"
import { pathToFileURL } from "node:url"

const context = {
  ex: "http://example.org/",
  foaf: "http://xmlns.com/foaf/0.1/",
  rdfs: "http://www.w3.org/2000/01/rdf-schema#",
  schema: "https://schema.org/",
  xsd: "http://www.w3.org/2001/XMLSchema#",
  name: "foaf:name",
  knows: { "@id": "foaf:knows", "@type": "@id" },
  born: { "@id": "schema:birthDate", "@type": "xsd:date" },
  comment: { "@id": "rdfs:comment", "@language": "en" },
  address: "schema:address",
  skills: { "@id": "ex:skills", "@container": "@list" },
}

function person(i, n, tag) {
  return {
    "@id": `ex:p${tag}${i}`,
    "@type": "foaf:Person",
    name: `Person ${i}`,
    born: `19${String(50 + (i % 50)).padStart(2, "0")}-01-01`,
    comment: `A longer description of person ${i}`,
    knows: [`ex:p${tag}${(i + 1) % n}`, `ex:p${tag}${(i + 7) % n}`],
    address: { "@type": "schema:PostalAddress", "schema:addressLocality": `Town ${i % 100}` },
    skills: ["rdf", "sparql", `skill-${i % 13}`],
  }
}

/** A pretty-printed document of about `lines` lines; `tag` makes IRIs unique per file. */
export function bigJsonLd(lines = 30000, tag = "") {
  const perPerson = JSON.stringify(person(0, 1, tag), null, 2).split("\n").length
  const fixed = JSON.stringify({ "@context": context, "@graph": [] }, null, 2).split("\n").length
  const n = Math.max(1, Math.round((lines - fixed) / perPerson))
  const graph = Array.from({ length: n }, (_, i) => person(i, n, tag))
  return JSON.stringify({ "@context": context, "@graph": graph }, null, 2) + "\n"
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const text = bigJsonLd(Number(process.argv[2] ?? 30000))
  if (process.argv[3]) fs.writeFileSync(process.argv[3], text)
  else process.stdout.write(text)
}
