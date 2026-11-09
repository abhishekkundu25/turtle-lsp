// THIRD_PARTY_NOTICES.md ships in the release archives, the `.vsix` and the
// browser site. A crate built from a path (vendored, or a path dependency)
// comes from no registry that a licence tool reads, so it is easy to leave
// out: each one in a lock file has a row at its locked version, marked for
// where that lock file's build ships.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const read = (rel) => fs.readFileSync(path.join(root, rel), "utf8")

// The [[package]] entries of a Cargo.lock without a `source`: built from a
// path. The workspace's own packages are left out.
function pathCrates(rel) {
  const own = new Set(["turtle-lsp", "turtle-lsp-web"])
  return read(rel)
    .split("[[package]]")
    .slice(1)
    .filter((p) => !/^source = /m.test(p))
    .map((p) => ({
      name: /^name = "([^"]+)"/m.exec(p)[1],
      version: /^version = "([^"]+)"/m.exec(p)[1],
    }))
    .filter((p) => !own.has(p.name))
}

// Each row of the Rust crate table: name, version, where it ships.
const rows = read("THIRD_PARTY_NOTICES.md")
  .split("\n")
  .map((line) => /^\| \[([^\]]+)\]\([^)]*\) \| ([^ |]+) \|.*\| ([^|]+) \|$/.exec(line))
  .filter(Boolean)
  .map(([, name, version, where]) => ({ name, version, where: where.split(",").map((s) => s.trim()) }))

for (const [lock, where] of [
  ["Cargo.lock", "binary"],
  ["web/engine/Cargo.lock", "browser"],
]) {
  test(`every crate ${lock} builds from a path has a notice for the ${where}`, () => {
    const crates = pathCrates(lock)
    assert.ok(crates.length > 0, `${lock} has no path crate`)
    for (const c of crates) {
      const row = rows.find((r) => r.name === c.name && r.version === c.version)
      assert.ok(row, `THIRD_PARTY_NOTICES.md has no row for ${c.name} ${c.version}`)
      assert.ok(row.where.includes(where), `${c.name} ${c.version} is not marked "${where}"`)
    }
  })
}
