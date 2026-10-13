// Process inspection for resource tests: which server processes are running,
// and how much CPU a process has used. No dependencies; works on macOS, Linux
// and Windows.
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/** PIDs of running processes whose executable is `binary` (absolute path). */
export function processesOf(binary) {
  const want = path.resolve(binary)
  if (process.platform === "win32") {
    const name = path.basename(want).replace(/'/g, "")
    const out = execFileSync(
      "powershell",
      ["-NoProfile", "-Command", `Get-CimInstance Win32_Process -Filter "Name='${name}'" | ForEach-Object { "$($_.ProcessId)|$($_.ExecutablePath)" }`],
      { encoding: "utf8" },
    )
    return out
      .split(/\r?\n/)
      .map((l) => l.split("|"))
      .filter(([pid, exe]) => pid && exe && path.resolve(exe).toLowerCase() === want.toLowerCase())
      .map(([pid]) => Number(pid))
  }
  const out = execFileSync("ps", ["-Ao", "pid=,command="], { encoding: "utf8" })
  return out
    .split("\n")
    .map((l) => /^\s*(\d+)\s+(.*)$/.exec(l))
    .filter((m) => m && (m[2] === want || m[2].startsWith(`${want} `)))
    .map((m) => Number(m[1]))
}

/** CPU time (user + system) a process has used so far, in seconds. */
export function cpuSeconds(pid) {
  if (process.platform === "win32") {
    const out = execFileSync("powershell", ["-NoProfile", "-Command", `(Get-Process -Id ${pid}).CPU`], { encoding: "utf8" })
    return Number(out.trim().replace(",", "."))
  }
  if (process.platform === "linux") {
    // Fields 14 and 15 of /proc/<pid>/stat (after the parenthesised name), in clock ticks.
    const stat = fs.readFileSync(`/proc/${pid}/stat`, "utf8")
    const f = stat.slice(stat.lastIndexOf(")") + 2).split(" ")
    return (Number(f[11]) + Number(f[12])) / 100
  }
  // macOS: [[dd-]hh:]mm:ss.cc
  const t = execFileSync("ps", ["-o", "time=", "-p", String(pid)], { encoding: "utf8" }).trim()
  const [rest, days] = t.includes("-") ? t.split("-").reverse() : [t, "0"]
  return rest
    .split(":")
    .map(Number)
    .reduce((acc, x) => acc * 60 + x, 0) + Number(days) * 86400
}

/** Wait until none of `pids` is running any more (or the timeout passes). */
export async function leftRunning(binary, pids, timeoutMs = 5000) {
  const end = Date.now() + timeoutMs
  for (;;) {
    const alive = processesOf(binary).filter((p) => pids.includes(p))
    if (alive.length === 0 || Date.now() > end) return alive
    await sleep(100)
  }
}

/** PIDs of `binary` that started since `before` (a list from processesOf). */
export function startedSince(binary, before) {
  return processesOf(binary).filter((p) => !before.includes(p))
}
