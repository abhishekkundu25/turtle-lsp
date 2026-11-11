// Regenerates data/vocab.tsv from the sources data/vocab-sources.tsv names,
// through the library tools/vocab-lib.mjs, and the corrections of
// data/vocab-patches.tsv. Run by tools/check-vocab.sh, which downloads the
// fetched sources first; `npm run vocab` runs that script with --write.
//
//   node tools/build-vocab.mjs [--write] [--sources DIR]
//       Builds the list. Without --write, compares it with data/vocab.tsv and
//       fails on a difference; with --write, writes it. Either way prints the
//       terms each vocabulary gained and lost against data/vocab.tsv.
//   node tools/build-vocab.mjs --fetched [--sources DIR]
//       Prints one line per fetched row, tab-separated: the file it is read
//       from, its url, its sha256 and the manifest line, for the download.
//
// A fetched row is read from DIR/<prefix>.<n>.<format>, n counting the rows
// of its prefix from 1 (DIR is target/vocab-sources by default); a frozen
// row from data/vocab-frozen/<prefix>.tsv.
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { gzipSync } from "node:zlib"
import { buildVocab, parseManifest, SIZE_BUDGET } from "./vocab-lib.mjs"

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const MANIFEST = path.join(root, "data/vocab-sources.tsv")
const CORRECTIONS = path.join(root, "data/vocab-patches.tsv")
const FROZEN = path.join(root, "data/vocab-frozen")
const LIST = path.join(root, "data/vocab.tsv")

const args = process.argv.slice(2)
const flag = (name) => {
  const i = args.indexOf(name)
  if (i >= 0) args.splice(i, 1)
  return i >= 0
}
const option = (name, fallback) => {
  const i = args.indexOf(name)
  if (i < 0) return fallback
  const [, value] = args.splice(i, 2)
  if (value === undefined) usage(`${name} needs a value`)
  return value
}
function usage(message) {
  console.error(`${message}\nusage: node tools/build-vocab.mjs [--write | --fetched] [--sources DIR]`)
  process.exit(2)
}
const write = flag("--write")
const fetched = flag("--fetched")
const sourcesDir = path.resolve(option("--sources", path.join(root, "target/vocab-sources")))
if (args.length) usage(`unknown argument ${args[0]}`)
if (write && fetched) usage("--write and --fetched exclude each other")

const relative = (file) => path.relative(root, file)

// Where each row's bytes are, in manifest order.
function locate(rows) {
  const seen = new Map()
  return rows.map((row) => {
    if (row.format === "tsv") return path.join(FROZEN, `${row.prefix}.tsv`)
    const n = (seen.get(row.prefix) ?? 0) + 1
    seen.set(row.prefix, n)
    return path.join(sourcesDir, `${row.prefix}.${n}.${row.format}`)
  })
}

// The library's refusals name the row or line; a stack trace adds nothing.
const refused = (e) => {
  console.error(e.message)
  process.exit(1)
}

const manifest = fs.readFileSync(MANIFEST, "utf8")
let rows
try {
  rows = parseManifest(manifest)
} catch (e) {
  refused(e)
}
const files = locate(rows)

if (fetched) {
  rows.forEach((row, i) => {
    if (row.format !== "tsv") console.log([files[i], row.url, row.sha256, `manifest line ${row.line} (${row.prefix})`].join("\t"))
  })
  process.exit(0)
}

// Every frozen extract has its row: a file under data/vocab-frozen/ that no
// row reads would be a vocabulary nobody ships.
const frozen = new Set(rows.filter((r) => r.format === "tsv").map((r) => `${r.prefix}.tsv`))
for (const name of fs.readdirSync(FROZEN)) {
  if (!frozen.has(name)) {
    console.error(`${relative(path.join(FROZEN, name))}: no frozen row of the manifest reads it`)
    process.exit(1)
  }
}

const sources = rows.map((row, i) => {
  try {
    return fs.readFileSync(files[i])
  } catch {
    const hint = row.format === "tsv" ? "" : "; tools/check-vocab.sh downloads it"
    console.error(`manifest line ${row.line} (${row.prefix}): ${relative(files[i])} is missing${hint}`)
    process.exit(1)
  }
})
const previous = fs.existsSync(LIST) ? fs.readFileSync(LIST, "utf8") : ""

const { text, summary } = await buildVocab({ manifest, corrections: fs.readFileSync(CORRECTIONS, "utf8"), sources, previous }).catch(refused)

for (const { prefix, added, removed } of summary) {
  console.log(`${prefix}: ${added.length} added, ${removed.length} removed`)
  if (removed.length) console.log(`  removed: ${removed.join(" ")}`)
}
const terms = text.split("\n").filter((l) => l.startsWith("term\t")).length
const vocabularies = new Set(rows.map((r) => r.prefix)).size
const gzipped = gzipSync(Buffer.from(text), { level: 9 }).length
console.log(`${vocabularies} vocabularies, ${terms} terms, ${Buffer.byteLength(text)} bytes, ${gzipped} gzipped (budget ${SIZE_BUDGET})`)

if (text === previous) {
  console.log("data/vocab.tsv is what the sources give")
} else if (write) {
  fs.writeFileSync(LIST, text)
  console.log("wrote data/vocab.tsv")
} else {
  console.error("data/vocab.tsv differs from what the sources give; run npm run vocab")
  process.exit(1)
}
