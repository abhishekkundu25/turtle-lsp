// One version everywhere. A release is built from several manifests (the
// server's Cargo.toml, the extension's package.json, the CHANGELOG heading the
// release notes come from) and `build.lua` downloads the server by the version
// in Cargo.toml: a manifest left behind gives a Marketplace extension that does
// not match its server, or a 404 in Neovim. CI runs this on every push; the
// release workflow runs it again with RELEASE_TAG set to the pushed tag.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const read = (rel) => fs.readFileSync(path.join(root, rel), "utf8")
const json = (rel) => JSON.parse(read(rel))

// The `version = "..."` of the [package] table of a Cargo.toml.
function cargoVersion(rel) {
  const pkg = /^\[package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(read(rel))
  assert.ok(pkg, `${rel} has no [package] table`)
  const v = /^version\s*=\s*"([^"]+)"/m.exec(pkg[1])
  assert.ok(v, `${rel} has no package version`)
  return v[1]
}

// The version a Cargo.lock records for one of the workspace's own packages.
function lockVersion(rel, name) {
  const m = new RegExp(`^name = "${name}"\\nversion = "([^"]+)"`, "m").exec(read(rel))
  assert.ok(m, `${rel} has no package ${name}`)
  return m[1]
}

const version = cargoVersion("Cargo.toml")

test("Cargo.toml has a release version", () => {
  assert.match(version, /^\d+\.\d+\.\d+$/)
})

test("Cargo.lock records the same server version", () => {
  assert.equal(lockVersion("Cargo.lock", "turtle-lsp"), version)
})

test("the extension's package.json has the same version", () => {
  assert.equal(json("editors/vscode/package.json").version, version)
})

test("the extension's package-lock.json has the same version", () => {
  const lock = json("editors/vscode/package-lock.json")
  assert.equal(lock.version, version)
  assert.equal(lock.packages[""].version, version)
})

test("the root package.json, when it has a version, has the same one", () => {
  const { version: v } = json("package.json")
  if (v !== undefined) assert.equal(v, version)
})

// web/engine is the browser build, never published (`publish = false`): 0.0.0
// means "not versioned". Any other version has to follow the release.
test("web/engine/Cargo.toml is unversioned (0.0.0) or has the same version", () => {
  const v = cargoVersion("web/engine/Cargo.toml")
  if (v === "0.0.0") return
  assert.equal(v, version)
  assert.equal(lockVersion("web/engine/Cargo.lock", "turtle-lsp-web"), version)
})

// What is wrong with the headings of the CHANGELOG `text` for `version`, or
// null. Between releases the top heading may be "## Unreleased"; the first
// versioned heading below it must then be this version. A release
// (`releasing`) needs its notes under the version heading, so there an
// "## Unreleased" top heading is wrong. The release workflow cuts the notes of
// a tag from the heading that starts with "## <version> ": the space after
// the version is not optional.
export function changelogProblem(text, version, releasing) {
  const headings = text.split("\n").filter((l) => l.startsWith("## "))
  if (headings[0]?.trim() === "## Unreleased") {
    if (releasing) return `the top heading is "## Unreleased": move its notes under "## ${version} — <date>" before the release`
    headings.shift()
  }
  const heading = headings[0]
  if (!heading) return "CHANGELOG.md has no version heading"
  if (!heading.startsWith(`## ${version} `)) return `heading "${heading}" does not start with "## ${version} "`
  return null
}

const releasing = Boolean(process.env.RELEASE_TAG)

test("the top CHANGELOG.md version heading is this version, in the form the release notes are cut by", () => {
  assert.equal(changelogProblem(read("CHANGELOG.md"), version, releasing), null)
})

// Scratch copies of the CHANGELOG, with and without an "## Unreleased" section on top.
const versioned = read("CHANGELOG.md").replace(/^## Unreleased\n[\s\S]*?(?=^## )/m, "")
const unreleased = versioned.replace(/^## /m, "## Unreleased\n\n- A change.\n\n## ")

test("an '## Unreleased' top heading is skipped between releases", () => {
  assert.equal(changelogProblem(unreleased, version, false), null)
  assert.equal(changelogProblem(versioned, version, false), null)
  assert.match(changelogProblem(unreleased, "99.0.0", false), /does not start with "## 99\.0\.0 "/)
})

test("an '## Unreleased' top heading fails a release", () => {
  assert.match(changelogProblem(unreleased, version, true), /"## Unreleased"/)
  assert.equal(changelogProblem(versioned, version, true), null)
})

test("the pushed tag is v<version>", { skip: !process.env.RELEASE_TAG && "RELEASE_TAG is not set (release workflow only)" }, () => {
  assert.equal(process.env.RELEASE_TAG, `v${version}`)
})
