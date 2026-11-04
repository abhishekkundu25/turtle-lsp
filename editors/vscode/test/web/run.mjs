// Runs test/web/suite.mjs in VS Code for the Web (@vscode/test-web) with the
// built browser entry (out/web, `npm run build`) and test/fixtures/web
// mounted as the workspace, in headless Google Chrome.
//
// test-web serves the workbench and downloads VS Code stable into
// .vscode-test/web (gitignored, and left out of the .vsix with the desktop
// test's .vscode-test). Its own browser launch only takes Playwright's
// bundled browsers, so it starts with browser "none" and this script opens
// Chrome itself (Playwright's "chrome" channel), with the two page functions
// the workbench calls in test mode: codeAutomationLog (the suite's console)
// and codeAutomationExit (its result). macOS and Linux (it lists processes
// with ps).
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import net from "node:net"
import os from "node:os"
import path from "node:path"
import { createRequire } from "node:module"
import { fileURLToPath } from "node:url"
import { build } from "esbuild"
import { open } from "@vscode/test-web"

const here = path.dirname(fileURLToPath(import.meta.url))
const extension = path.resolve(here, "../..")
const data = path.join(extension, ".vscode-test", "web")
const suite = path.join(data, "suite.js")
const fixtures = path.resolve(here, "../fixtures/web")
const TIMEOUT_MS = 100_000
// Chrome profiles of this runner: <tmp>/turtle-web-test-<runner pid>-<random>.
const PROFILE_PREFIX = "turtle-web-test-"

// The same Playwright test-web depends on.
const { chromium } = createRequire(createRequire(import.meta.url).resolve("@vscode/test-web"))("playwright")

if (!fs.existsSync(path.join(extension, "out/web/extension.js")) || !fs.existsSync(path.join(extension, "out/web/engine.wasm"))) {
  console.error("out/web is not built: run `node web/build.mjs` at the repository root (the engine), then `npm run build`")
  process.exit(1)
}

/** Running processes as [pid, ppid, command]. */
function processes() {
  return execFileSync("ps", ["-Ao", "pid=,ppid=,command="], { encoding: "utf8" })
    .split("\n")
    .map((l) => /^\s*(\d+)\s+(\d+)\s+(.*)$/.exec(l))
    .filter(Boolean)
    .map((m) => [Number(m[1]), Number(m[2]), m[3]])
}

/** The Chrome started with `profile` and every process under it. */
function chromeOf(profile) {
  const all = processes()
  const found = new Set(all.filter(([, , cmd]) => cmd.includes(profile)).map(([pid]) => pid))
  for (let grew = true; grew; ) {
    grew = false
    for (const [pid, ppid] of all) if (found.has(ppid) && !found.has(pid)) grew = found.add(pid)
  }
  return [...found]
}

const alive = (pid) => {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

function kill(pids) {
  for (const pid of pids) {
    try {
      process.kill(pid, "SIGKILL")
    } catch {}
  }
}

// A run that crashed or was killed (timeout 120) left its profile, maybe its
// Chrome: stop that Chrome and delete the profile, for every runner no longer
// running. A runner still running (another checkout) keeps its own.
for (const name of fs.readdirSync(os.tmpdir())) {
  const pid = Number(new RegExp(`^${PROFILE_PREFIX}(\\d+)-`).exec(name)?.[1])
  if (!pid || alive(pid)) continue
  const profile = path.join(os.tmpdir(), name)
  kill(chromeOf(profile))
  fs.rmSync(profile, { recursive: true, force: true })
}

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer().listen(0, "127.0.0.1", () => {
      const { port } = srv.address()
      srv.close(() => resolve(port))
    })
    srv.on("error", reject)
  })
}

const started = Date.now()
fs.mkdirSync(data, { recursive: true })
await build({
  entryPoints: [path.join(here, "suite.mjs")],
  outfile: suite,
  bundle: true,
  format: "cjs",
  platform: "browser",
  external: ["vscode"],
  logLevel: "warning",
})

const profile = fs.mkdtempSync(path.join(os.tmpdir(), `${PROFILE_PREFIX}${process.pid}-`))
const port = await freePort()
let server
let context
let chrome = []
let timer
try {
  server = await open({
    browserType: "none",
    quality: "stable",
    extensionDevelopmentPath: extension,
    extensionTestsPath: suite,
    folderPath: fixtures,
    // Not an IP address: the extension host's iframe is served from
    // <uuid>.<host>, which an IP address does not take.
    host: "localhost",
    port,
    testRunnerDataDir: data,
  })
  context = await chromium.launchPersistentContext(profile, {
    channel: "chrome",
    headless: true,
    viewport: { width: 1280, height: 800 },
  })
  chrome = chromeOf(profile)
  const page = context.pages()[0] ?? (await context.newPage())
  let exit
  const exited = new Promise((resolve) => (exit = resolve))
  const timedOut = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`the suite did not finish within ${TIMEOUT_MS / 1000} s`)), TIMEOUT_MS)
  })
  await page.exposeFunction("codeAutomationLog", (type, args) => (console[type] ?? console.log)(...args))
  await page.exposeFunction("codeAutomationExit", exit)
  await page.goto(`http://localhost:${port}`)
  const code = await Promise.race([exited, timedOut])
  if (code !== 0) throw new Error(`the suite exited with code ${code}`)
  console.log(`# ${Date.now() - started} ms in all`)
} catch (err) {
  console.error(`not ok - ${err?.message ?? err}`)
  process.exitCode = 1
} finally {
  clearTimeout(timer)
  await context?.close().catch(() => {})
  server?.dispose()
  // Closing the context ends Chrome; whatever is left of it is stopped here
  // and fails the run.
  const ours = [...new Set([...chrome, ...chromeOf(profile)])]
  for (const end = Date.now() + 5000; ours.some(alive) && Date.now() < end; ) await new Promise((r) => setTimeout(r, 100))
  const left = ours.filter(alive)
  if (left.length > 0) {
    console.error(`not ok - Chrome processes left running after the run: ${left.join(", ")}`)
    kill(left)
    process.exitCode = 1
  } else {
    console.log("ok - no Chrome process left running")
  }
  fs.rmSync(profile, { recursive: true, force: true })
}
