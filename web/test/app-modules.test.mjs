// The page's shared modules and the web tooling, in Node (no browser): text
// and path helpers, the file walkers and their limits, the one static server,
// the build's Monaco filter and size budget, and the files the pages are made of.
import { test } from "node:test"
import assert from "node:assert/strict"
import crypto from "node:crypto"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import zlib from "node:zlib"
import { fileURLToPath } from "node:url"
import { esc } from "../app/dom.js"
import { lineStarts, offsetOf, positionAt, lineAt, applyEdits } from "../app/text.js"
import { dirOf, nameOf, join, under, uriOf, ROOT } from "../app/paths.js"
import { KNOWN, LIMITS, admit, picked, walkDropped, walkHandle, readTextCapped, limitsNote, readUtf8 } from "../app/files.js"
import { languageOf, READ_ONLY, COMPLETED } from "../app/editor-lsp.js"
import { resolveIn, listen } from "./static-server.mjs"
import { wanted, checkBudget, copyMonaco, WASM_GZIP_BUDGET } from "../build.mjs"

const web = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const app = path.join(web, "app")
const read = (...p) => fs.readFileSync(path.join(web, ...p), "utf8")
// Our own sources: the app's files, without the copied vendor directory.
const sources = () => fs.readdirSync(app).filter((f) => f.endsWith(".js") || f.endsWith(".html"))

test("there is one esc, in dom.js, and it escapes what ends an attribute or opens a tag", () => {
  const copies = sources().filter((f) => /const esc = /.test(read("app", f)))
  assert.deepEqual(copies, ["dom.js"])
  assert.equal(esc(`<a href="x" onclick='y'>&</a>`), "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;&lt;/a&gt;")
  assert.equal(esc(12), "12")
})

test("text helpers: line starts, offsets and positions agree, and edits apply against the original text", () => {
  const text = "ab\ncd\n\nefg"
  const starts = lineStarts(text)
  assert.deepEqual(starts, [0, 3, 6, 7])
  assert.equal(offsetOf(starts, text, { line: 1, character: 1 }), 4)
  assert.equal(offsetOf(starts, text, { line: 9, character: 0 }), text.length, "past the end is clamped")
  for (let o = 0; o <= text.length; o++) assert.equal(offsetOf(starts, text, positionAt(starts, o)), o)
  assert.equal(lineAt(text, starts, 1), "cd")
  assert.equal(lineAt(text, starts, 3), "efg")
  assert.equal(lineAt(text, starts, 4), "")
  assert.equal(lineAt("a\r\nb", lineStarts("a\r\nb"), 0), "a", "a CR before the break is not part of the line")
  const edits = [
    { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 1 } }, newText: "X" },
    { range: { start: { line: 3, character: 3 }, end: { line: 3, character: 3 } }, newText: "!" },
    { range: { start: { line: 1, character: 0 }, end: { line: 1, character: 2 } }, newText: "" },
  ]
  assert.equal(applyEdits(text, edits), "Xb\n\n\nefg!")
})

test("text helpers: edits at one position apply in the order of the array, as LSP says", () => {
  const at = (character, end = character) => ({ start: { line: 0, character }, end: { line: 0, character: end } })
  // Inserts at one offset: an import line, then a blank line, before the text.
  assert.equal(applyEdits("body", [{ range: at(0), newText: "a\n" }, { range: at(0), newText: "\n" }]), "a\n\nbody")
  assert.equal(applyEdits("ab", [{ range: at(1), newText: "1" }, { range: at(1), newText: "2" }, { range: at(1), newText: "3" }]), "a123b")
  // An insert and a replacement starting where it is, either way round.
  assert.equal(applyEdits("abcd", [{ range: at(1), newText: "X" }, { range: at(1, 3), newText: "Y" }]), "aXYd")
  assert.equal(applyEdits("abcd", [{ range: at(1, 3), newText: "Y" }, { range: at(1), newText: "X" }]), "aYXd")
  // Out of order in the array, apart in the text.
  assert.equal(applyEdits("abcd", [{ range: at(3, 4), newText: "D" }, { range: at(0, 1), newText: "A" }]), "AbcD")
})

test("paths: parents, names, joins, containment and file URIs", () => {
  assert.equal(dirOf("a/b/c.ttl"), "a/b")
  assert.equal(dirOf("c.ttl"), "")
  assert.equal(nameOf("a/b/c.ttl"), "c.ttl")
  assert.equal(join("", "c.ttl"), "c.ttl")
  assert.equal(join("a", "c.ttl"), "a/c.ttl")
  assert.ok(under("a/b", "a") && under("a", "a") && !under("ab", "a"))
  assert.equal(uriOf("my dir/é.ttl"), `${ROOT}my%20dir/%C3%A9.ttl`)
})

// ---- files.js ---------------------------------------------------------------------

const file = (name, size = 10) => ({ name, size })
// The FileSystemEntry API as the browser hands a dropped tree over.
const fileEntry = (name, fullPath) => ({ isFile: true, isDirectory: false, name, fullPath, file: (ok) => ok({ name, size: 5 }) })
const dirEntry = (name, fullPath, children) => ({
  isFile: false,
  isDirectory: true,
  name,
  fullPath,
  createReader() {
    let done = false
    return { readEntries: (ok) => (ok(done ? [] : children), (done = true)) }
  },
})
const item = (entry) => ({ webkitGetAsEntry: () => entry })

test("one set of limits: admit leaves out unknown extensions, big files and what is over the count", () => {
  const entries = [file("a.ttl"), file("b.md"), file("c.rq", LIMITS.fileBytes + 1), file("d.jsonld"), file("e.nt")].map((f) => ({ file: f, path: f.name }))
  const { take, skipped } = admit(entries)
  assert.deepEqual(take.map((e) => e.path), ["a.ttl", "d.jsonld", "e.nt"])
  assert.equal(skipped, 2)
  const few = admit(entries, { limits: { ...LIMITS, files: 2 } })
  assert.deepEqual(few.take.map((e) => e.path), ["a.ttl", "d.jsonld"])
  assert.equal(admit(entries, { have: LIMITS.files }).take.length, 0, "files already in the workspace count")
  assert.equal(admit(entries, { onlyKnown: false }).take.length, 4, "with any extension, only the big one is left out")
  assert.match(limitsNote(), /10 MB.*at most 1000/)
  assert.ok(KNOWN.test("x.TTL") && !KNOWN.test("x.md"))
  for (const name of ["onto.owl", "data.rdf", "ONTO.OWL", "onto.ofn", "ONTO.OFN", "onto.owx", "ONTO.OWX"]) assert.ok(KNOWN.test(name), name)
  for (const name of ["data.xml", "onto.omn"]) assert.ok(!KNOWN.test(name), name)
  assert.match(limitsNote(), /RDF\/XML \(\.owl, \.rdf\), OWL Functional \(\.ofn\), OWL\/XML \(\.owx\)/)
  assert.deepEqual(picked([{ name: "a.ttl" }, { name: "b.ttl", webkitRelativePath: "dir/b.ttl" }]).map((e) => e.path), ["a.ttl", "dir/b.ttl"])
})

test("the toolbox's file picker accepts exactly the extensions a folder contributes", () => {
  const html = fs.readFileSync(path.join(app, "toolbox.html"), "utf8")
  const accept = html.match(/id="picker"[^>]*accept="([^"]*)"/)[1].split(",")
  const listed = KNOWN.source.match(/\(([^)]*)\)/)[1].split("|").map((e) => `.${e}`)
  assert.deepEqual(accept, listed)
  assert.ok(accept.includes(".owl") && accept.includes(".rdf") && accept.includes(".ofn") && accept.includes(".owx"), accept.join(","))
})

test("the RDF Editor opens .owl, .rdf and .owx files as Monaco's XML and .ofn files as OWL Functional syntax, which the server's providers that read serve", () => {
  for (const name of ["onto.owl", "dir/data.RDF", "a.owx", "dir/O.OWX"]) assert.equal(languageOf(name), "xml", name)
  for (const name of ["onto.ofn", "dir/O.OFN"]) assert.equal(languageOf(name), "ofn", name)
  assert.deepEqual(READ_ONLY, ["xml", "ofn"])
  assert.deepEqual(COMPLETED, ["ofn", "xml"])
  assert.equal(languageOf("carol.jsonld"), "json")
  assert.equal(languageOf("a.ttl"), "turtle")
  // The file tree's badge and the status bar name the language.
  const editor = fs.readFileSync(path.join(app, "editor.js"), "utf8")
  assert.match(editor, /const ICON = \{[^}]*\bxml: "XML", ofn: "OFN"/)
  assert.match(editor, /xml: "RDF\/XML", ofn: "OWL Functional" \}\[languageOf\(file\)\]/)
  // An .owx file is XML to Monaco; the status bar names it OWL/XML.
  assert.match(editor, /\/\\\.owx\$\/i\.test\(file\) \? "OWL\/XML" : \{/)
  // connect() registers the server's features on its languages, JSON, XML
  // and OWL Functional syntax; on the last two, only those that read,
  // completion and rename.
  const lsp = fs.readFileSync(path.join(app, "editor-lsp.js"), "utf8")
  assert.match(lsp, /for \(const lang of \[\.\.\.LANGUAGES, "json", \.\.\.READ_ONLY\]\) \{\n\s*const writable = !READ_ONLY\.includes\(lang\)\n/)
  const registered = (guard) => [...lsp.matchAll(new RegExp(`\\n\\s*${guard}monaco\\.languages\\.(register\\w+)\\(lang,`, "g"))].map((m) => m[1]).sort()
  assert.deepEqual(registered("if \\(writable \\|\\| COMPLETED\\.includes\\(lang\\)\\) "), ["registerCompletionItemProvider"])
  assert.deepEqual(registered("if \\(writable\\) "), [
    "registerCodeActionProvider",
    "registerDocumentFormattingEditProvider",
    "registerDocumentSemanticTokensProvider",
    "registerInlayHintsProvider",
  ])
  assert.deepEqual(registered(""), [
    "registerCodeLensProvider",
    "registerDefinitionProvider",
    "registerDocumentHighlightProvider",
    "registerDocumentSymbolProvider",
    "registerFoldingRangeProvider",
    "registerHoverProvider",
    "registerReferenceProvider",
    "registerRenameProvider",
  ])
})

test("a local file is read as UTF-8, its byte order mark dropped, and refused with the offset of its first bad byte", async () => {
  const blob = (...parts) => new Blob(parts.map((p) => (typeof p === "string" ? Buffer.from(p) : Uint8Array.from(p))))
  assert.equal(await readUtf8(blob("ex:a ex:b \"é\" .\n")), 'ex:a ex:b "é" .\n')
  assert.equal(await readUtf8(blob([0xef, 0xbb, 0xbf], "ex:a .")), "ex:a .", "the BOM is dropped, as file.text() drops it")
  assert.equal(await readUtf8(blob("")), "")
  const refused = (bytes) => readUtf8(bytes).then(() => assert.fail("read"), (e) => e.message)
  assert.equal(await refused(blob("ex:a ", [0xe9], " .")), "not valid UTF-8 (first bad byte at offset 5)")
  // The offset counts the BOM and characters of several bytes; a U+FFFD
  // written in the file is not the bad byte.
  assert.equal(await refused(blob([0xef, 0xbb, 0xbf], "é\uFFFD😀", [0xff])), "not valid UTF-8 (first bad byte at offset 12)")
  // A sequence cut short, at the end and before more text.
  assert.equal(await refused(blob("ab", [0xe2, 0x82])), "not valid UTF-8 (first bad byte at offset 2)")
  assert.equal(await refused(blob("ab", [0xe2, 0x82], "c")), "not valid UTF-8 (first bad byte at offset 2)")
})

test("a dropped folder is walked with the limits: hidden folders and node_modules are not entered, unknown files count as skipped", async () => {
  const tree = dirEntry("model", "/model", [
    fileEntry("a.ttl", "/model/a.ttl"),
    fileEntry("readme.md", "/model/readme.md"),
    dirEntry(".git", "/model/.git", [fileEntry("config.ttl", "/model/.git/config.ttl")]),
    dirEntry("node_modules", "/model/node_modules", [fileEntry("x.ttl", "/model/node_modules/x.ttl")]),
    dirEntry("shapes", "/model/shapes", [fileEntry("b.ttl", "/model/shapes/b.ttl")]),
  ])
  const { entries, skipped } = await walkDropped([item(tree)])
  assert.deepEqual(entries.map((e) => e.path), ["model/a.ttl", "model/shapes/b.ttl"])
  assert.equal(skipped, 1)
  // A file dropped on its own is taken whatever it is called: the caller decides.
  const single = await walkDropped([item(fileEntry("notes.md", "/notes.md"))])
  assert.deepEqual(single.entries.map((e) => e.path), ["notes.md"])
})

test("walking a dropped tree ends at the scan and depth limits instead of reading everything", async () => {
  const many = dirEntry("big", "/big", Array.from({ length: 50 }, (_, i) => fileEntry(`f${i}.ttl`, `/big/f${i}.ttl`)))
  const scanned = await walkDropped([item(many)], { ...LIMITS, scan: 10 })
  assert.ok(scanned.entries.length < 10, `${scanned.entries.length} entries read`)
  assert.ok(scanned.skipped >= 1, "the cut is reported")
  let deep = fileEntry("deep.ttl", "/d/deep.ttl")
  for (let i = 0; i < 6; i++) deep = dirEntry(`d${i}`, `/d${i}`, [deep])
  const shallow = await walkDropped([item(deep)], { ...LIMITS, depth: 3 })
  assert.equal(shallow.entries.length, 0)
  assert.equal(shallow.skipped, 1)
})

test("walking a folder opened in place takes known files, skips hidden folders and node_modules, and stops at the limits", async () => {
  const dir = (children) => ({ kind: "directory", async *entries() { yield* Object.entries(children) } })
  const f = (name) => ({ kind: "file", getFile: async () => ({ name, size: 3 }) })
  const root = dir({ "a.ttl": f("a.ttl"), "readme.md": f("readme.md"), ".git": dir({ "c.ttl": f("c.ttl") }), node_modules: dir({ "d.ttl": f("d.ttl") }), sub: dir({ "b.rq": f("b.rq") }) })
  const { entries, skipped } = await walkHandle(root, "proj")
  assert.deepEqual(entries.map((e) => [e.path, e.onDisk]), [["proj/a.ttl", true], ["proj/sub/b.rq", true]])
  assert.equal(skipped, 0)
  const cut = await walkHandle(root, "proj", { ...LIMITS, scan: 2 })
  assert.equal(cut.skipped, 1)
})

test("a response is read only up to its cap: by content-length, and by the bytes that arrive when it does not say", async () => {
  const stream = (chunks) => new ReadableStream({ start(c) { for (const x of chunks) c.enqueue(new TextEncoder().encode(x)); c.close() } })
  assert.equal(await readTextCapped(new Response(stream(["abc", "def"])), 10), "abcdef")
  let cancelled = false
  // A body that never ends: only the cap stops the reading.
  const endless = new ReadableStream({ pull: (c) => c.enqueue(new TextEncoder().encode("x".repeat(8))), cancel: () => (cancelled = true) })
  const big = new Response(endless)
  assert.equal(big.headers.get("content-length"), null)
  assert.equal(await readTextCapped(big, 10), null)
  assert.ok(cancelled, "the rest is not downloaded")
  const declared = new Response("small body", { headers: { "content-length": "999999999" } })
  assert.equal(await readTextCapped(declared, 1000), null, "a declared length over the cap is refused before reading")
  assert.equal(await readTextCapped(new Response("é".repeat(6)), 10), null, "the cap counts bytes, not characters")
  assert.equal(await readTextCapped(new Response("é".repeat(5)), 10), "é".repeat(5))
})

// ---- the static server ---------------------------------------------------------------

test("the static server serves the app, refuses to leave its root and survives a file that cannot be read", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "static-"))
  try {
    fs.mkdirSync(path.join(root, "app"))
    fs.writeFileSync(path.join(root, "app", "index.html"), "<p>hi</p>")
    fs.writeFileSync(path.join(root, "app", "x.wasm"), "\0asm")
    fs.writeFileSync(path.join(root, "secret.txt"), "secret")
    fs.mkdirSync(path.join(root, "app-evil"))
    fs.writeFileSync(path.join(root, "app-evil", "x.txt"), "evil")
    const appRoot = path.join(root, "app")
    assert.equal(resolveIn(appRoot, "/"), path.join(appRoot, "index.html"))
    assert.equal(resolveIn(appRoot, "/../secret.txt"), null)
    assert.equal(resolveIn(appRoot, "/../app-evil/x.txt"), null, "a sibling that starts with the root's name is outside it")
    assert.equal(resolveIn(appRoot, "/missing"), null)
    const served = await listen({ root: appRoot, headers: { "cache-control": "no-cache" } })
    try {
      const get = async (p) => {
        const r = await fetch(served.origin + p)
        return [r.status, r.headers.get("content-type"), r.headers.get("cache-control"), await r.text()]
      }
      assert.deepEqual(await get("/"), [200, "text/html", "no-cache", "<p>hi</p>"])
      assert.equal((await get("/x.wasm"))[1], "application/wasm")
      assert.equal((await get("/../secret.txt"))[0], 404)
      assert.equal((await get("/%E0%A4%A"))[0], 400, "a bad escape is a bad request")
      // A directory and an unreadable file: errors, not a crashed server.
      fs.mkdirSync(path.join(appRoot, "dir"))
      assert.equal((await get("/dir"))[0], 404)
      fs.writeFileSync(path.join(appRoot, "locked.html"), "x")
      fs.chmodSync(path.join(appRoot, "locked.html"), 0)
      if (process.getuid?.() !== 0) assert.equal((await get("/locked.html"))[0], 500)
      assert.equal((await get("/"))[0], 200, "still serving")
    } finally {
      await served.close()
    }
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test("web/serve.mjs serves web/app through that server, with no-cache", async () => {
  const { spawn } = await import("node:child_process")
  const child = spawn(process.execPath, [path.join(web, "serve.mjs")], { env: { ...process.env, PORT: "0" }, stdio: ["ignore", "pipe", "inherit"] })
  try {
    const url = await new Promise((resolve, reject) => {
      child.stdout.once("data", (d) => resolve(String(d).trim()))
      child.once("exit", (c) => reject(new Error(`serve.mjs exited with ${c}`)))
    })
    const r = await fetch(`${url}toolbox.html`)
    assert.equal(r.status, 200)
    assert.equal(r.headers.get("cache-control"), "no-cache")
    assert.match(await r.text(), /RDF Toolbox/)
  } finally {
    child.kill()
    await new Promise((r) => (child.exitCode !== null ? r() : child.once("exit", r)))
  }
})

test("no test carries its own static server: they all use static-server.mjs", () => {
  const own = fs
    .readdirSync(path.join(web, "test"))
    .filter((f) => f.endsWith(".test.mjs") && /http\.createServer/.test(read("test", f)))
  // editor-query.test.mjs belongs to the engine seam work and still has a copy.
  assert.deepEqual(own.filter((f) => f !== "editor-query.test.mjs"), [])
})

// ---- the build -------------------------------------------------------------------------

test("the build skips the Monaco language services the editor never starts, and the message catalogues", () => {
  const sep = path.sep
  const p = (...x) => ["", "m", "vs", ...x].join(sep)
  for (const skipped of [p("language", "typescript"), p("language", "typescript", "ts.worker.js"), p("language", "css", "monaco.contribution.js"), p("language", "html"), p("tsMode-B0S_kp2q.js"), p("cssMode-CV6Ay48H.js"), p("htmlMode-27by_KM4.js"), p("assets", "ts.worker-BWKtMYOk.js"), p("assets", "css.worker-URu8fCFR.js"), p("assets", "html.worker-D1SL3iM8.js"), p("nls"), p("nls", "lang", "fr.js")]) {
    assert.equal(wanted(skipped), false, skipped)
  }
  // The services' small worker-URL modules stay: editor.main.js imports them.
  for (const kept of [p("language", "json", "monaco.contribution.js"), p("language", "json"), p("jsonMode-DLUiMUrI.js"), p("assets", "json.worker-CoJx_OPf.js"), p("assets", "editor.worker-lj3bdIIn.js"), p("ts.worker-2QLmBukE.js"), p("css.worker-CyhWkhHo.js"), p("typescript-_2t_511t.js"), p("basic-languages", "monaco.contribution.js"), p("editor", "editor.main.js"), p("nls.messages-loader.js"), p("loader.js")]) {
    assert.equal(wanted(kept), true, kept)
  }
})

test("the build fails when the engine is over its size budget", () => {
  assert.doesNotThrow(() => checkBudget(WASM_GZIP_BUDGET))
  assert.throws(() => checkBudget(WASM_GZIP_BUDGET + 1), /over the budget/)
  assert.throws(() => checkBudget(5_000_000, 4_000_000), /5\.00 MB gzipped, over the budget of 4\.00 MB/)
  // The engine in the tree is within it.
  const wasm = path.join(app, "engine.wasm")
  if (fs.existsSync(wasm)) assert.doesNotThrow(() => checkBudget(zlib.gzipSync(fs.readFileSync(wasm), { level: 9 }).length))
})

// The files Monaco's AMD module `id` (in the folder `vs` of a Monaco build)
// loads: itself and its static imports, through every module, with the loader
// plugins they name (`vs/nls.messages-loader!`). Paths relative to `vs`.
function monacoModules(vs, id) {
  const files = new Set()
  const walk = (id) => {
    const file = `${id}.js`
    if (files.has(file)) return
    files.add(file)
    const deps = fs.readFileSync(path.join(vs, file), "utf8").match(/^define\("[^"]*",\s*(\[[^\]]*\])/)
    assert.ok(deps, `${file} is an AMD module`)
    for (const dep of JSON.parse(deps[1])) {
      if (["require", "exports", "module"].includes(dep)) continue
      const name = dep.replace(/!.*$/, "")
      walk(name.startsWith("vs/") ? name.slice(3) : path.posix.normalize(path.posix.join(path.posix.dirname(id), name)))
    }
  }
  walk(id)
  return files
}

// The scripts the page's Monaco worker loads, relative to the page.
const workerScripts = () => [...read("app", "monaco-worker.js").matchAll(/"(vendor\/monaco\/[^"]+)"/g)].map((m) => m[1])

test("copyMonaco copies the editor without the unused services, with its licence and notices", () => {
  const from = path.join(web, "..", "node_modules", "monaco-editor")
  if (!fs.existsSync(from)) return
  const to = fs.mkdtempSync(path.join(os.tmpdir(), "monaco-"))
  try {
    const target = path.join(to, "monaco")
    assert.match(copyMonaco(from, target), /^\d+\.\d+\.\d+$/)
    assert.deepEqual(fs.readdirSync(path.join(target, "vs", "language")), ["json"])
    assert.ok(fs.statSync(path.join(target, "LICENSE")).size > 100)
    assert.ok(fs.statSync(path.join(target, "ThirdPartyNotices.txt")).size > 1000)
    // The editor, every file it loads, and the editor's and JSON's workers.
    for (const f of monacoModules(path.join(target, "vs"), "editor/editor.main")) assert.ok(fs.existsSync(path.join(target, "vs", f)), `${f} is copied`)
    const workers = workerScripts()
    assert.equal(workers.length, 2)
    for (const w of workers) assert.ok(fs.existsSync(path.join(to, w.slice("vendor/".length))), `${w} is copied`)
    // Not the other services' modes and workers, nor the catalogues.
    const left = fs.readdirSync(path.join(target, "vs"), { recursive: true }).map((f) => f.split(path.sep).join("/"))
    assert.deepEqual(left.filter((f) => /^(?:ts|css|html)Mode-|^assets\/(?:ts|css|html)\.worker-|^nls(?:\/|$)/.test(f)), [])
    assert.ok(fs.readdirSync(path.join(from, "min", "vs", "assets")).some((f) => /^ts\.worker-/.test(f)), "the package has a TypeScript worker for the copy to leave out")
  } finally {
    fs.rmSync(to, { recursive: true, force: true })
  }
})

// ---- the pages ---------------------------------------------------------------------------

const meta = (html) => html.match(/<meta http-equiv="Content-Security-Policy" content="([^"]*)">/)?.[1]
const directives = (csp) => Object.fromEntries(csp.split(";").map((d) => d.trim().split(/\s+/)).map(([name, ...values]) => [name, values]))

// The text of each inline script: a `<script` tag, in any letter case, with
// no `src` attribute, up to the next `</script>`. Found with indexOf, not a
// regular expression: this reads our own pages, it filters no HTML.
const inlineScripts = (html) => {
  const lower = html.toLowerCase()
  const out = []
  for (let at = lower.indexOf("<script"); at >= 0; at = lower.indexOf("<script", at + 1)) {
    const tagEnd = lower.indexOf(">", at)
    assert.ok(tagEnd >= 0, "every script tag is closed")
    const tag = lower.slice(at + "<script".length, tagEnd)
    if (tag && !/^[\s/]/.test(tag)) continue // `<scripts>`, another element
    const names = tag.split(/[\s/]+/).map((a) => a.split("=")[0])
    if (names.includes("src")) continue
    const end = lower.indexOf("</script", tagEnd)
    assert.ok(end >= 0, "every inline script is closed")
    out.push(html.slice(tagEnd + 1, end))
    at = end
  }
  return out
}

test("inlineScripts finds every script without a src, whatever its attributes", () => {
  assert.deepEqual(inlineScripts('<script type="module">a()</script>'), ["a()"])
  assert.deepEqual(inlineScripts("<SCRIPT>b()</SCRIPT>"), ["b()"])
  assert.deepEqual(inlineScripts('<script data-src="x.js">c()</script>'), ["c()"])
  assert.deepEqual(inlineScripts('<script\ntype="module">d()</script>'), ["d()"])
  assert.deepEqual(inlineScripts('<script type="module" src="app.js"></script><script src=x.js></script>'), [])
  assert.deepEqual(inlineScripts("<scripts>e</scripts>"), [])
})

test("both pages carry a Content-Security-Policy, with the editor page's inline scripts allowed by hash", () => {
  for (const page of ["index.html", "toolbox.html"]) {
    const html = read("app", page)
    const csp = meta(html)
    assert.ok(csp, `${page} has the meta`)
    assert.ok(html.indexOf("Content-Security-Policy") < html.indexOf("<script"), "before any script")
    const d = directives(csp)
    assert.deepEqual(d["object-src"], ["'none'"])
    assert.deepEqual(d["base-uri"], ["'none'"])
    assert.deepEqual(d["worker-src"], ["'self'"])
    assert.ok(d["script-src"].includes("'self'") && !d["script-src"].includes("'unsafe-inline'") && !d["script-src"].includes("'unsafe-eval'"))
    assert.ok(d["connect-src"].includes("'self'"))
  }
  assert.ok(directives(meta(read("app", "index.html")))["connect-src"].includes("https:"), "the fetch quick fix may read ontologies")
  const html = read("app", "index.html")
  const hashes = inlineScripts(html).map((text) => `'sha256-${crypto.createHash("sha256").update(text).digest("base64")}'`)
  assert.equal(hashes.length, 2)
  for (const h of hashes) assert.ok(directives(meta(html))["script-src"].includes(h), `${h} is allowed`)
  assert.deepEqual(inlineScripts(read("app", "toolbox.html")), [], "the toolbox has no inline script")
})

test("the editor page preloads every module editor.js imports, and nothing else, and the Monaco files it preloads are where the build puts them", () => {
  const html = read("app", "index.html")
  const preloaded = [...html.matchAll(/<link rel="modulepreload" href="([^"]+)">/g)].map((m) => m[1])
  // editor.js and what it imports, statically, through every module.
  const imported = new Set()
  const walk = (file) => {
    if (imported.has(file)) return
    imported.add(file)
    for (const m of read("app", file).matchAll(/^\s*(?:import|export)\s[^"']*?from\s*["'](\.[^"']+)["']|^\s*import\s*["'](\.[^"']+)["']/gm)) {
      walk(path.posix.normalize(path.posix.join(path.posix.dirname(file), m[1] ?? m[2])))
    }
  }
  walk("editor.js")
  assert.deepEqual([...preloaded].sort(), [...imported].sort())
  const monaco = path.join(web, "..", "node_modules", "monaco-editor", "min")
  const monacoPreloads = [...html.matchAll(/<link rel="preload" href="vendor\/monaco\/([^"]+)"/g)].map((m) => m[1])
  for (const href of monacoPreloads) {
    assert.ok(fs.existsSync(path.join(monaco, href)), `${href} is in Monaco's package`)
    assert.ok(wanted(path.join(monaco, href)), `${href} is copied by the build`)
  }
  // Monaco's code: editor.main.js and every file it loads, and nothing else.
  assert.deepEqual(monacoPreloads.sort(), [...monacoModules(path.join(monaco, "vs"), "editor/editor.main")].map((f) => `vs/${f}`).sort())
})

test("the dead prototype is gone, and so is the script that served it", () => {
  assert.equal(fs.existsSync(path.join(web, "prototype")), false)
  const scripts = JSON.parse(fs.readFileSync(path.join(web, "..", "package.json"), "utf8")).scripts
  assert.equal("web:prototype" in scripts, false)
  for (const spec of fs.readdirSync(path.join(web, "..", "docs", "specs")).filter((f) => f.startsWith("web-"))) {
    assert.doesNotMatch(fs.readFileSync(path.join(web, "..", "docs", "specs", spec), "utf8"), /web\/prototype/)
  }
})

test("the query pane's note names each kind of answer about inferred triples", async () => {
  const { inferredNote } = await import("../app/query-pane.js")
  assert.equal(inferredNote(undefined), "")
  assert.equal(inferredNote({ rules: "default", triples: 1, findings: 0 }), "Includes 1 inferred triple (TurbOWL, default rules)")
  assert.equal(inferredNote({ rules: "complete", triples: 12, findings: 2 }), "Includes 12 inferred triples (TurbOWL, complete rules); TurbOWL reported 2 findings")
  assert.equal(inferredNote({ skipped: { triples: 4000, limit: 1000 } }), "Inferred triples skipped: the workspace has 4,000 triples, over the reasoning limit of 1,000")
  assert.equal(inferredNote({ skipped: { inferred: true, limit: 1000 } }), "Inferred triples skipped: TurbOWL inferred more than 1,000 triples, over the reasoning limit")
})

// The query pane against a fake DOM and a fake worker: each test drives the
// worker's messages by hand.
class FakeEl {
  constructor() {
    this.parts = new Map()
    this.listeners = {}
    this.attrs = {}
    this.hidden = false
    this.textContent = ""
    this.title = ""
    this.className = ""
    this.innerHTML = ""
    this.classList = { toggle() {} }
  }
  querySelector(sel) {
    if (!this.parts.has(sel)) this.parts.set(sel, new FakeEl())
    return this.parts.get(sel)
  }
  setAttribute(k, v) { this.attrs[k] = v }
  getAttribute(k) { return this.attrs[k] ?? null }
  removeAttribute(k) { delete this.attrs[k] }
  addEventListener(type, f) { this.listeners[type] = f }
  click() { this.listeners.click?.() }
  append() {}
  after() {}
  focus() {}
}
class FakeWorker {
  static made = []
  static fail = null
  constructor() {
    if (FakeWorker.fail) throw FakeWorker.fail
    this.posted = []
    this.terminated = false
    FakeWorker.made.push(this)
  }
  postMessage(m) { this.posted.push(m) }
  terminate() { this.terminated = true }
  answer(data) { this.onmessage({ data }) }
}
async function queryPane({ settings = {}, reasoning = "default" } = {}) {
  globalThis.document = { createElement: () => new FakeEl(), querySelectorAll: () => [] }
  globalThis.Worker = FakeWorker
  FakeWorker.made = []
  FakeWorker.fail = null
  const { createQueryPane, watchdog } = await import("../app/query-pane.js")
  const qp = createQueryPane({ files: () => [], reveal() {}, settings: () => settings, inferred: { get: () => reasoning, set() {} } })
  const group = (name) => {
    const g = { el: new FakeEl(), editor: new FakeEl(), active: `${name}.rq` }
    qp.attach(g, () => qp.run(g))
    g.status = () => g.run.pane.querySelector(".qpane-status").textContent
    // The body's text: every character outside a tag.
    g.text = () => {
      let text = ""
      let inTag = false
      for (const ch of g.run.pane.querySelector(".qpane-body").innerHTML) {
        if (ch === "<") inTag = true
        else if (ch === ">") inTag = false
        else if (!inTag) text += ch
      }
      return text
    }
    return g
  }
  return { qp, group, watchdog }
}
const ask = (boolean) => ({ kind: "ask", boolean, elapsedMs: 1 })

test("cancelling or closing a query still queued fails only it: the running query keeps its worker and answers", async () => {
  const { qp, group } = await queryPane()
  const [a, b, c] = [group("a"), group("b"), group("c")]
  const runs = [qp.run(a), qp.run(b), qp.run(c)]
  const [w] = FakeWorker.made
  const id = w.posted[0].id
  w.answer({ id, started: true })
  b.run.button.click() // Cancel, before b started
  c.run.pane.querySelector(".ic").click() // Close Results, before c started
  w.answer({ id, ok: ask(true) })
  await Promise.all(runs)
  assert.equal(w.terminated, false)
  assert.equal(a.status(), "ask · 1 ms")
  assert.equal(b.status(), "cancelled")
  // Neither reached the worker, so it never spends time on them.
  assert.deepEqual(w.posted.map((m) => m.query), ["a.rq"])
  assert.equal(FakeWorker.made.length, 1)
  // The next query runs in the same worker.
  const next = qp.run(b)
  w.answer({ id: w.posted[1].id, started: true })
  w.answer({ id: w.posted[1].id, ok: ask(false) })
  await next
  assert.equal(b.text(), "false")
})

test("cancelling a started query ends its worker and fails what is queued behind it", async () => {
  const { qp, group } = await queryPane()
  const [a, b] = [group("a"), group("b")]
  const runs = [qp.run(a), qp.run(b)]
  const [w] = FakeWorker.made
  w.answer({ id: w.posted[0].id, started: true })
  a.run.button.click()
  await Promise.all(runs)
  assert.equal(w.terminated, true)
  assert.equal(a.status(), "cancelled")
  assert.equal(b.status(), "not run")
})

test("cancelling a query its worker has not started, while the engine loads, keeps the queue: the next query runs in a new worker", async () => {
  const { qp, group } = await queryPane()
  const [a, b] = [group("a"), group("b")]
  const runs = [qp.run(a), qp.run(b)]
  const [w] = FakeWorker.made
  a.run.button.click()
  assert.equal(w.terminated, true)
  const w2 = FakeWorker.made[1]
  assert.deepEqual(w2.posted.map((m) => m.query), ["b.rq"])
  w2.answer({ id: w2.posted[0].id, started: true })
  w2.answer({ id: w2.posted[0].id, ok: ask(true) })
  await Promise.all(runs)
  assert.equal(a.status(), "cancelled")
  assert.equal(b.text(), "true")
})

test("only the query the engine was running when it crashed is told to try Inferred off", async () => {
  const { qp, group } = await queryPane()
  const [a, b] = [group("a"), group("b")]
  const runs = [qp.run(a), qp.run(b)]
  const [w] = FakeWorker.made
  const id = w.posted[0].id
  w.answer({ id, started: true })
  w.answer({ id, error: "RuntimeError: unreachable", crashed: true })
  await Promise.all(runs)
  assert.match(a.text(), /RuntimeError: unreachable\. .*try Inferred off\./)
  assert.equal(b.status(), "error")
  assert.doesNotMatch(b.text(), /Inferred off/)
  // A worker that fails to load is not the inferred triples' doing.
  const c = group("c")
  const run = qp.run(c)
  FakeWorker.made.at(-1).onerror({ message: "Uncaught Error: boom", preventDefault() {} })
  await run
  assert.match(c.text(), /the query engine stopped: Uncaught Error: boom/)
  assert.doesNotMatch(c.text(), /Inferred off/)
})

test("a query worker that cannot be made shows an error in the pane", async () => {
  const { qp, group } = await queryPane()
  const a = group("a")
  FakeWorker.fail = new Error("Worker is not allowed here")
  await qp.run(a)
  assert.equal(a.status(), "error")
  assert.match(a.text(), /Worker is not allowed here/)
  assert.equal(a.run.button.querySelector(".qrun-label").textContent, "Run")
  FakeWorker.fail = null
  const run = qp.run(a)
  const [w] = FakeWorker.made
  w.answer({ id: w.posted[0].id, started: true })
  w.answer({ id: w.posted[0].id, ok: ask(true) })
  await run
  assert.equal(a.text(), "true")
})

test("the query carries its time limit, read as the engine reads the settings, and the watchdog uses that number", async () => {
  for (const [settings, timeout] of [
    [{}, 30],
    [{ turtle: { query: { timeoutSeconds: 2 } } }, 2],
    [{ query: { timeoutSeconds: 1 } }, 1],
    [{ "turtle.query.timeoutSeconds": 0 }, 0],
    [{ turtle: { "query.timeoutSeconds": 3 } }, 3],
    [{ query: { timeoutSeconds: -1 } }, 30],
  ]) {
    const { qp, group } = await queryPane({ settings })
    qp.run(group("a"))
    assert.equal(FakeWorker.made[0].posted[0].timeout, timeout, JSON.stringify(settings))
  }
  const { qp, group, watchdog } = await queryPane({ settings: { query: { timeoutSeconds: 1 } } })
  const a = group("a")
  const grace = watchdog.graceMs
  watchdog.graceMs = -950
  try {
    const run = qp.run(a)
    const [w] = FakeWorker.made
    w.answer({ id: w.posted[0].id, started: true })
    await run
    assert.equal(w.terminated, true)
    assert.match(a.text().trim(), /^Stopped: the query ran past its time limit of 1 second\./)
  } finally {
    watchdog.graceMs = grace
  }
})
