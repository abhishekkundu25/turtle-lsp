// Runs test/nvim/spec.lua in a headless, isolated Neovim and turns each check into a test.
import { test, after } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { serverPath, fixtures } from "./lsp-client.mjs"
import { processesOf, leftRunning } from "./procs.mjs"

const nvim = spawnSync("nvim", ["--version"])
const home = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-nvim-"))
// Neovim runs its own copy of the server, so the leak check below sees only
// servers Neovim started (other test files run servers in parallel).
const server = path.join(home, path.basename(serverPath))
fs.copyFileSync(serverPath, server)
fs.chmodSync(server, 0o755)
const run =
  nvim.status === 0
    ? spawnSync("nvim", ["--headless", "--clean", "-u", path.join(fixtures, "..", "nvim", "spec.lua")], {
        env: {
          ...process.env,
          TURTLE_LSP_SERVER: server,
          TURTLE_LSP_FIXTURES: fixtures,
          XDG_STATE_HOME: home,
          XDG_DATA_HOME: home,
          XDG_CONFIG_HOME: home,
          TURTLE_LSP_CACHE_DIR: path.join(home, "cache"),
        },
        encoding: "utf8",
        timeout: 120000,
      })
    : null
const output = run ? `${run.stdout}\n${run.stderr}` : ""
const lines = output.split(/\r?\n/).filter((l) => /^(not )?ok - /.test(l))
// The checks the run must report: the plan line (`1..N`) spec.lua prints, else
// the checks written in spec.lua (those indented sit under `if` and need the
// local-only `real` fixtures).
const spec = path.join(fixtures, "..", "nvim", "spec.lua")
const plan = /^1\.\.(\d+)$/m.exec(output)
const expected = plan
  ? Number(plan[1])
  : fs
      .readFileSync(spec, "utf8")
      .split("\n")
      .filter((l) => (fs.existsSync(path.join(fixtures, "real")) ? /^\s*check\(/ : /^check\(/).test(l)).length
const log = path.join(home, "nvim", "lsp.log")

after(() => fs.rmSync(home, { recursive: true, force: true }))

if (run) {
  test("nvim leaves no turtle-lsp running after it quits", async () => {
    const alive = await leftRunning(server, processesOf(server))
    assert.deepEqual(alive, [], `turtle-lsp still running after nvim quit: ${alive.join(", ")}`)
  })
}

if (!run) {
  test("nvim is installed", { skip: "nvim not found on PATH" }, () => {})
} else {
  test("nvim spec ran", () => {
    assert.ok(lines.length > 0, `no results from nvim:\n${output}`)
  })
  // A crash, a timeout or an early quit can still have printed "ok" lines.
  test("nvim exits 0, not by a signal", () => {
    assert.equal(run.error, undefined, `nvim did not run to its end: ${run.error}`)
    assert.equal(run.signal, null, `nvim was killed by ${run.signal}:\n${output}`)
    assert.equal(run.status, lines.some((l) => l.startsWith("not ok")) ? 1 : 0, `nvim exited ${run.status}:\n${output}`)
  })
  test("nvim reports every check", () => {
    assert.equal(lines.length, expected, `${lines.length} results for ${expected} checks:\n${output}`)
  })
  for (const line of lines) {
    const m = /^(not )?ok - (.+?) ::\s*(.*)$/.exec(line)
    test(`nvim: ${m[2]}`, () => {
      if (m[1]) {
        const tail = fs.existsSync(log) ? fs.readFileSync(log, "utf8").split("\n").slice(-15).join("\n") : ""
        assert.fail(`${m[3] ?? ""}\n--- lsp.log tail ---\n${tail}`)
      }
    })
  }
}
