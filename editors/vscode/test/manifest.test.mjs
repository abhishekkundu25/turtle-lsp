// Security-relevant parts of the extension manifest, and the settings the
// server reads.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import { documentSelector, inSchemes, JSONLD_PATTERN, OPT_IN, RDF_GLOB, RDFXML_PATTERN, registrationsInSchemes, settingsFrom } from "../src/client.ts"

const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"))
const settings = pkg.contributes.configuration.properties
const setting = settings["turtle.server.path"]
const restricted = pkg.capabilities.untrustedWorkspaces.restrictedConfigurations

// The keys src/settings.rs applies: the match arms of Settings::apply_at that
// name a whole setting (`["shacl", "maxTriples"] =>`), as dotted names.
const rustSource = fs.readFileSync(new URL("../../../src/settings.rs", import.meta.url), "utf8")
const serverKeys = [...rustSource.matchAll(/^\s*\[((?:"\w+"(?:,\s*)?)+)\]\s*=>/gm)]
  .map((m) => [...m[1].matchAll(/"(\w+)"/g)].map((k) => k[1]).join("."))
  .sort()

test("a workspace cannot choose which binary the extension runs", () => {
  // "machine" settings are ignored in workspace and folder settings files.
  assert.equal(setting.scope, "machine")
})

test("turtle.server.path is restricted in untrusted workspaces", () => {
  const cap = pkg.capabilities?.untrustedWorkspaces
  assert.equal(cap?.supported, "limited")
  assert.ok(cap.restrictedConfigurations.includes("turtle.server.path"))
})

test("network imports are off by default and restricted in untrusted workspaces", () => {
  const s = settings["turtle.imports.network"]
  assert.equal(s.type, "boolean")
  assert.equal(s.default, false)
  assert.equal(s.scope, "machine-overridable")
  assert.ok(pkg.capabilities.untrustedWorkspaces.restrictedConfigurations.includes("turtle.imports.network"))
})

test("server settings are declared with the server's defaults", () => {
  const expected = {
    "turtle.inlayHints.labels": ["string", "opaque"],
    "turtle.shacl.enabled": ["boolean", true],
    "turtle.shacl.maxTriples": ["integer", 2000000],
    "turtle.shacl.onType": ["boolean", true],
    "turtle.reasoning.rules": ["string", "default"],
    "turtle.reasoning.maxTriples": ["integer", 2000000],
    "turtle.reasoning.diagnostics": ["boolean", false],
    "turtle.lint.undescribedTerms": ["boolean", true],
    "turtle.imports.network": ["boolean", false],
    "turtle.imports.unresolved": ["string", "soften"],
    "turtle.format.lineWidth": ["integer", 100],
    "turtle.codeLens.references": ["boolean", true],
    "turtle.diagnostics.workspace": ["boolean", false],
    "turtle.parse.incremental": ["boolean", true],
    "turtle.rename.rdfXml": ["boolean", true],
    "turtle.query.timeoutSeconds": ["number", 30],
    "turtle.query.maxRows": ["integer", 1000],
    "turtle.query.maxBufferedRows": ["integer", 10000000],
    "turtle.query.reasoning": ["string", "off"],
  }
  for (const [key, [type, def]] of Object.entries(expected)) {
    assert.ok(settings[key], `${key} missing`)
    assert.equal(settings[key].type, type, key)
    assert.equal(settings[key].default, def, key)
    assert.ok(settings[key].markdownDescription || settings[key].description, `${key} has no description`)
  }
  assert.deepEqual(settings["turtle.inlayHints.labels"].enum, ["opaque", "all", "off"])
  assert.deepEqual(settings["turtle.imports.unresolved"].enum, ["soften", "ignore"])
  assert.deepEqual(settings["turtle.reasoning.rules"].enum, ["off", "default", "complete"])
  assert.match(settings["turtle.reasoning.rules"].enumDescriptions[2], /Slower/)
  assert.deepEqual(settings["turtle.query.reasoning"].enum, ["off", "default", "complete"])
  assert.match(settings["turtle.query.reasoning"].enumDescriptions[2], /Slower/)
})

test("lint.ignore and lint.enable are lists of codes, empty by default, sent as lists", () => {
  for (const key of ["turtle.lint.ignore", "turtle.lint.enable"]) {
    assert.equal(settings[key].type, "array", key)
    assert.equal(settings[key].items.type, "string", key)
    assert.deepEqual(settings[key].default, [], key)
    assert.equal(settings[key].scope, "resource", key)
  }
  // Every off-by-default code (features::curation::OPT_IN) is named in lint.enable's description.
  const curation = fs.readFileSync(new URL("../../../src/features/curation.rs", import.meta.url), "utf8")
  const names = Object.fromEntries([...curation.matchAll(/const (\w+): &str = "([\w-]+)"/g)].map((m) => [m[1], m[2]]))
  const optIn = curation.match(/OPT_IN: &\[&str\] = &\[([^\]]*)\]/)[1].match(/\w+/g).map((n) => names[n])
  assert.equal(optIn.length, 8, optIn.join(", "))
  for (const code of optIn) assert.match(settings["turtle.lint.enable"].markdownDescription, new RegExp(`\`${code}\``), code)
  // A list is sent as a list; like VS Code itself, the narrowest scope's list wins.
  const inspect = (key) => (key === "lint.ignore" ? { globalValue: ["unused-prefix"], workspaceValue: ["unknown-term"] } : {})
  assert.deepEqual(settingsFrom({ properties: settings, restricted, inspect, trusted: true }), { lint: { ignore: ["unknown-term"] } })
})

test("JSON-LD files are served without taking over VS Code's JSON language", () => {
  // The JSON language (and its highlighting) stays VS Code's own: no language
  // here claims .jsonld, and the client attaches to the files by name.
  const claimed = pkg.contributes.languages.flatMap((l) => l.extensions ?? [])
  assert.ok(!claimed.includes(".jsonld"), claimed.join(", "))
  assert.ok(!pkg.contributes.languages.some((l) => l.id === "json" || l.id === "jsonld"))
  assert.ok(pkg.activationEvents.some((e) => e.startsWith("workspaceContains:") && e.includes("jsonld")), pkg.activationEvents.join(", "))
  assert.ok(pkg.activationEvents.includes("onLanguage:json"))
  assert.equal(JSONLD_PATTERN, "**/*.jsonld")
  assert.ok(documentSelector(["file", "untitled"]).some((f) => f.scheme === "file" && f.pattern === JSONLD_PATTERN))
  const desktop = fs.readFileSync(new URL("../src/extension.ts", import.meta.url), "utf8")
  assert.match(desktop, /documentSelector\(\["file", "untitled"\]\)/)
})

test("one RDF glob: the workspaceContains activation, the start check, the watchers and the browser's files", () => {
  const contains = pkg.activationEvents.filter((e) => e.startsWith("workspaceContains:"))
  assert.deepEqual(contains, [`workspaceContains:${RDF_GLOB}`])
  // Each source file names the constant, not a copy of the glob.
  for (const file of ["activate.ts", "graph.ts", "web/files.ts"]) {
    const source = fs.readFileSync(new URL(`../src/${file}`, import.meta.url), "utf8")
    assert.ok(source.includes("RDF_GLOB"), file)
    assert.ok(!source.includes("{ttl,"), `${file} spells out the glob`)
  }
})

test(".owl and .rdf files are read and watched, and the client attaches to them while VS Code's XML language keeps them", () => {
  // Workspace files: the one glob (activation, watchers, the browser's files) names them.
  const extensions = RDF_GLOB.match(/\{([^}]*)\}/)[1].split(",")
  for (const ext of ["owl", "rdf"]) assert.ok(extensions.includes(ext), RDF_GLOB)
  assert.ok(!extensions.includes("xml") && !extensions.includes("owx"), RDF_GLOB)
  // No language here claims them or XML, and nothing starts on XML documents.
  const claimed = pkg.contributes.languages.flatMap((l) => l.extensions ?? [])
  for (const ext of [".owl", ".rdf", ".xml"]) assert.ok(!claimed.includes(ext), claimed.join(", "))
  assert.ok(!pkg.contributes.languages.some((l) => l.id === "xml"))
  assert.ok(!pkg.activationEvents.includes("onLanguage:xml"), pkg.activationEvents.join(", "))
  // The client attaches to them by name, as to JSON-LD files, in every scheme but untitled (which has no name).
  const selector = documentSelector(["file", "untitled", "vscode-vfs"])
  assert.equal(RDFXML_PATTERN, "**/*.{owl,rdf}")
  assert.deepEqual([...new Set(selector.filter((f) => "pattern" in f).map((f) => f.pattern))], [JSONLD_PATTERN, RDFXML_PATTERN])
  assert.deepEqual(selector.filter((f) => f.pattern === RDFXML_PATTERN).map((f) => f.scheme), ["file", "vscode-vfs"])
  assert.ok(!selector.some((f) => "language" in f && f.language === "xml"))
  // The graph reads the workspace: it is offered on these files from the Explorer and the editor title.
  const menus = pkg.contributes.menus
  const explorer = menus["explorer/context"].find((m) => m.command === "turtle.showGraph").when
  const regex = new RegExp(explorer.match(/resourceExtname =~ \/(.*)\/\)/)[1])
  for (const name of [".owl", ".rdf", ".ttl", ".jsonld"]) assert.ok(regex.test(name), `${name}: ${explorer}`)
  for (const name of [".xml", ".owx", ".owl.bak"]) assert.ok(!regex.test(name), `${name}: ${explorer}`)
  const title = menus["editor/title"].find((m) => m.command === "turtle.showGraph").when
  for (const ext of [".owl", ".rdf"]) assert.ok(title.includes(`resourceExtname == ${ext}`), title)
})

test("the server's registered selector, in the client's schemes, leaves .owl and .rdf out but for rename", () => {
  // The selector src/server/registrations.rs sends: its languages and the JSON-LD pattern, no scheme.
  const rust = fs.readFileSync(new URL("../../../src/server/registrations.rs", import.meta.url), "utf8")
  const languages = JSON.parse(rust.match(/LANGUAGES: \[&str; \d+\] =\s*(\[[^\]]*\])/)[1])
  assert.equal(rust.match(/JSONLD_PATTERN: &str = "([^"]*)"/)[1], JSONLD_PATTERN)
  const server = [...languages.map((language) => ({ language })), { pattern: JSONLD_PATTERN }]
  // Every language the client serves is one the server registers for.
  for (const f of documentSelector(["file"])) if ("language" in f) assert.ok(languages.includes(f.language), f.language)
  for (const schemes of [["file", "untitled"], ["vscode-vfs", "vscode-test-web"]]) {
    const selector = documentSelector(schemes)
    const narrowed = inSchemes(server, selector)
    assert.deepEqual(narrowed, selector.filter((f) => f.pattern !== RDFXML_PATTERN), schemes.join(", "))
    // Rename's selector has the RDF/XML pattern too: it keeps every filter.
    assert.equal(rust.match(/RDFXML_PATTERN: &str = "([^"]*)"/)[1], RDFXML_PATTERN)
    assert.deepEqual(inSchemes([...server, { pattern: RDFXML_PATTERN }], selector), selector, schemes.join(", "))
  }
  // A registration without a selector (the watchers') stays as it is; a filter with a scheme too.
  const params = {
    registrations: [
      { id: "w", method: "workspace/didChangeWatchedFiles", registerOptions: { watchers: [] } },
      { id: "f", method: "textDocument/formatting", registerOptions: { documentSelector: [...server, { scheme: "git", language: "turtle" }] } },
    ],
  }
  const out = registrationsInSchemes(params, documentSelector(["file"]))
  assert.deepEqual(out.registrations[0], params.registrations[0])
  assert.deepEqual(out.registrations[1].registerOptions.documentSelector, [
    ...documentSelector(["file"]).filter((f) => f.pattern !== RDFXML_PATTERN),
    { scheme: "git", language: "turtle" },
  ])
  // Both entries ask for the registrations, with the server's key, beside the
  // settings in initialize only, and pass the narrowing to the client.
  assert.equal(rust.match(/OPT_IN: &str = "([^"]*)"/)[1], OPT_IN)
  const activate = fs.readFileSync(new URL("../src/activate.ts", import.meta.url), "utf8")
  assert.match(activate, /initializationOptions: \{ \.\.\.settingsFor\(context, setup\), \[OPT_IN\]: true \}/)
  assert.equal(activate.match(/OPT_IN/g).length, 2, "OPT_IN is sent in initialize only (the import and initializationOptions)")
  assert.match(activate, /handleRegisterCapability: .*\n.*registrationsInSchemes\(params, setup\.documentSelector\)/)
})

test("the browser entry runs in a virtual workspace, with what is off described", () => {
  assert.equal(pkg.browser, "./out/web/extension.js")
  const cap = pkg.capabilities.virtualWorkspaces
  assert.equal(cap.supported, "limited")
  assert.match(cap.description, /Run query/)
  assert.match(cap.description, /2,000 files/)
  // Built by "npm run build" (npm test builds first).
  for (const out of [pkg.browser, "./out/web/worker.js"]) {
    assert.ok(fs.existsSync(new URL(`../${out}`, import.meta.url)), `${out} is not built`)
  }
})

test("suggestions pop up in Turtle and TriG strings, where SHACL queries are typed", () => {
  const defaults = pkg.contributes.configurationDefaults
  for (const lang of ["[turtle]", "[trig]"]) {
    assert.deepEqual(defaults?.[lang]?.["editor.quickSuggestions"], { other: "on", comments: "off", strings: "on" }, lang)
  }
  // Other languages keep VS Code's defaults.
  assert.ok(!("editor.quickSuggestions" in defaults))
  assert.deepEqual(
    Object.keys(defaults).filter((k) => defaults[k]["editor.quickSuggestions"]),
    ["[turtle]", "[trig]"]
  )
})

test("the manifest declares exactly the settings the server applies", () => {
  assert.ok(serverKeys.length >= 16, `found ${serverKeys.join(", ")} in src/settings.rs`)
  const declared = Object.keys(settings)
    .map((k) => k.replace(/^turtle\./, ""))
    // Editor-only settings, not sent to the server.
    .filter((k) => !k.startsWith("server.") && !k.startsWith("trace."))
    .sort()
  assert.deepEqual(declared, serverKeys)
})

test("settings the server limits or fetches with are restricted in untrusted workspaces", () => {
  for (const key of [
    "turtle.server.path",
    "turtle.imports.network",
    "turtle.query.timeoutSeconds",
    "turtle.query.maxRows",
    "turtle.query.maxBufferedRows",
    "turtle.shacl.maxTriples",
    "turtle.reasoning.rules",
    "turtle.reasoning.maxTriples",
    "turtle.diagnostics.workspace",
  ]) {
    assert.ok(restricted.includes(key), `${key} is not in restrictedConfigurations`)
    assert.ok(settings[key], `${key} is not a setting`)
  }
  assert.match(pkg.capabilities.untrustedWorkspaces.description, /turtle\.query/)
})

// Every setting set at the user level, as VS Code's inspect() reports it.
const allSet = (key) => ({ globalValue: settings[`turtle.${key}`].default })

test("every server setting reaches the server, and only the user's values do", () => {
  const flat = (o, prefix = "") =>
    Object.entries(o).flatMap(([k, v]) => (v && typeof v === "object" && !Array.isArray(v) && k !== "prefixes" ? flat(v, `${prefix}${k}.`) : [`${prefix}${k}`]))
  const sent = settingsFrom({ properties: settings, restricted, inspect: allSet, trusted: true })
  assert.deepEqual(flat(sent).sort(), serverKeys)
  // Unset settings: nothing is sent, so turtle-lsp.json decides.
  assert.deepEqual(settingsFrom({ properties: settings, restricted, inspect: () => ({}), trusted: true }), {})
})

test("an untrusted workspace turns network imports off and ignores workspace values of restricted settings", () => {
  const inspect = (key) => {
    if (key === "imports.network") return { globalValue: true, workspaceValue: true }
    if (key === "query.maxRows") return { globalValue: 50, workspaceValue: 99999999 }
    if (key === "format.lineWidth") return { workspaceValue: 60 }
    return {}
  }
  const untrusted = settingsFrom({ properties: settings, restricted, inspect, trusted: false })
  assert.deepEqual(untrusted, { imports: { network: false }, query: { maxRows: 50 }, format: { lineWidth: 60 }, reasoning: { rules: "default" } })
  const trusted = settingsFrom({ properties: settings, restricted, inspect, trusted: true })
  assert.deepEqual(trusted, { imports: { network: true }, query: { maxRows: 99999999 }, format: { lineWidth: 60 } })
})

test("in an untrusted workspace only the user's reasoning rules count, or the default, never turtle-lsp.json's", () => {
  // Sent even when unset, so the server's turtle-lsp.json (and, in the
  // browser, the file layered under these) cannot choose the rules.
  const sent = (inspect, trusted) => settingsFrom({ properties: settings, restricted, inspect, trusted }).reasoning
  assert.deepEqual(sent(() => ({}), false), { rules: "default" })
  assert.deepEqual(sent(() => undefined, false), { rules: "default" })
  assert.equal(settings["turtle.reasoning.rules"].default, "default")
  const workspace = (key) => (key === "reasoning.rules" ? { workspaceValue: "complete" } : {})
  assert.deepEqual(sent(workspace, false), { rules: "default" })
  const user = (key) => (key === "reasoning.rules" ? { globalValue: "off", workspaceValue: "complete" } : {})
  assert.deepEqual(sent(user, false), { rules: "off" })
  // Trusted, an unset setting is left to turtle-lsp.json.
  assert.equal(sent(() => ({}), true), undefined)
  assert.deepEqual(sent(workspace, true), { rules: "complete" })
  assert.match(pkg.capabilities.untrustedWorkspaces.description, /turtle\.reasoning\.rules/)
})

test("a setting left at its default is not sent, so turtle-lsp.json can decide (format.lineWidth)", () => {
  const sent = settingsFrom({ properties: settings, restricted, inspect: () => ({ defaultValue: 100 }), trusted: true })
  assert.equal(sent.format, undefined)
})
