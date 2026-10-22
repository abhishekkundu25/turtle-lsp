// A large synthetic JSON-LD document for the performance budgets: one
// @graph of people with an inline @context (no remote contexts, so it parses
// offline), using the features real files lean on: compact IRIs, @type,
// @id references, typed and language-tagged values, nested blank nodes and
// @list.
//
// The document is described once, in bench/jsonld-people.tpl (a head, one
// person with %TAG%, %I%, %BORN%, %K1%, %K2%, %TOWN% and %SKILL% to fill,
// and a tail, split by `@@` lines). The Rust budget tests
// (tests/jsonld_budget.rs, src/jsonld/mod.rs) read the same file through
// tests/support/jsonld_people.rs, and write the same text.
//
//   import { bigJsonLd } from "./jsonld-gen.mjs"
//   const text = bigJsonLd(30000)          // about 30,000 lines
//
//   node bench/jsonld-gen.mjs [lines=30000] [out.jsonld]   // stdout without out
import fs from "node:fs"
import { fileURLToPath, pathToFileURL } from "node:url"

const template = fs.readFileSync(fileURLToPath(new URL("./jsonld-people.tpl", import.meta.url)), "utf8")
const [head, person, tail, ...extra] = template.split("@@\n")
if (tail === undefined || extra.length) throw new Error("jsonld-people.tpl needs a head, a person and a tail between @@ lines")

const lineCount = (s) => s.split("\n").length - 1

/** How many people make a document of about `lines` lines. */
function peopleCount(lines) {
  return Math.max(1, Math.round((lines - lineCount(head) - lineCount(tail)) / lineCount(person)))
}

/** What person `i` of `n` has that varies. */
function values(i, n) {
  return {
    "%BORN%": String(50 + (i % 50)).padStart(2, "0"),
    "%K1%": String((i + 1) % n),
    "%K2%": String((i + 7) % n),
    "%TOWN%": String(i % 100),
    "%SKILL%": String(i % 13),
  }
}

/** A pretty-printed document of about `lines` lines; `tag` makes IRIs unique per file. */
export function bigJsonLd(lines = 30000, tag = "") {
  const n = peopleCount(lines)
  const people = []
  for (let i = 0; i < n; i++) {
    const v = { "%TAG%": tag, "%I%": String(i), ...values(i, n) }
    people.push(person.replace(/%[A-Z0-9]+%/g, (k) => v[k]).replace(/\n$/, ""))
  }
  return head + people.join(",\n") + "\n" + tail
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const text = bigJsonLd(Number(process.argv[2] ?? 30000))
  if (process.argv[3]) fs.writeFileSync(process.argv[3], text)
  else process.stdout.write(text)
}
