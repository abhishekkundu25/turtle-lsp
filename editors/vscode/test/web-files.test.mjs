// The browser entry's read budget (src/web/budget.ts): which workspace files
// are read, decided from their sizes before any is read, and how many reads
// run at once.
import { test } from "node:test"
import assert from "node:assert/strict"
import { CHUNK, CONCURRENCY, choose, gather, mapBounded } from "../src/web/budget.ts"

const MB = 1024 * 1024
const ROOM = { files: 2000, bytes: 50 * MB }

/** A fake disk: `sizes` by name (a missing name fails its stat), with every stat and read recorded. */
function disk(sizes, { unreadable = [] } = {}) {
  const log = []
  let inFlight = 0
  let peak = 0
  const io = {
    async size(uri) {
      log.push(["size", uri])
      if (!(uri in sizes)) throw new Error("no such file")
      return sizes[uri]
    },
    async read(uri) {
      log.push(["read", uri])
      peak = Math.max(peak, ++inFlight)
      await new Promise((resolve) => setTimeout(resolve, 1))
      inFlight--
      return unreadable.includes(uri) ? undefined : { text: uri, bytes: sizes[uri] }
    },
  }
  return { io, log, reads: () => log.filter(([op]) => op === "read").map(([, uri]) => uri), peak: () => peak }
}

async function run(uris, d, room = ROOM) {
  const batches = []
  const tally = await gather(uris, d.io, room, async (batch) => {
    batches.push(batch.map((f) => f.uri))
  })
  return { tally, batches }
}

test("choose: files are taken in order while they fit, a too-large one is skipped and a later small one still fits", () => {
  assert.deepEqual(choose([10, 100, 20, undefined, 5], { files: 10, bytes: 40 }), { read: [0, 2, 4], tooBig: 1, tooMany: 0, unreadable: 1 })
})

test("choose: past the file count, the rest are counted too many", () => {
  assert.deepEqual(choose([1, 1, 1, 1], { files: 2, bytes: 100 }), { read: [0, 1], tooBig: 0, tooMany: 2, unreadable: 0 })
})

test("a file over the remaining budget is never read", async () => {
  const d = disk({ "a.ttl": 10 * MB, "dump.nt": 60 * MB, "b.ttl": 1 * MB })
  const { tally } = await run(["a.ttl", "dump.nt", "b.ttl"], d)
  assert.ok(!d.reads().includes("dump.nt"))
  assert.deepEqual(tally, { read: 2, tooBig: 1, tooMany: 0, unreadable: 0 })
})

test("every size is known before the first read, and no read is made past the budget", async () => {
  // 200 files of 1 MB: only 50 fit in 50 MB, so only 50 are read (the chunk of 200 is never read whole).
  const sizes = Object.fromEntries(Array.from({ length: 200 }, (_, i) => [`f${i}.ttl`, MB]))
  const d = disk(sizes)
  const { tally } = await run(Object.keys(sizes), d)
  const firstRead = d.log.findIndex(([op]) => op === "read")
  assert.equal(d.log.slice(firstRead).filter(([op]) => op === "size").length, 0)
  assert.equal(d.reads().length, 50)
  assert.deepEqual(tally, { read: 50, tooBig: 150, tooMany: 0, unreadable: 0 })
})

test("at most 2,000 files are read, the rest counted past the file limit", async () => {
  const sizes = Object.fromEntries(Array.from({ length: 2100 }, (_, i) => [`f${i}.ttl`, 10]))
  const d = disk(sizes)
  const { tally, batches } = await run(Object.keys(sizes), d)
  assert.equal(d.reads().length, 2000)
  assert.deepEqual(tally, { read: 2000, tooBig: 0, tooMany: 100, unreadable: 0 })
  assert.ok(batches.every((b) => b.length <= CHUNK))
  assert.equal(batches.flat().length, 2000)
})

test("reads never run more than CONCURRENCY at once", async () => {
  const sizes = Object.fromEntries(Array.from({ length: 300 }, (_, i) => [`f${i}.ttl`, 10]))
  const d = disk(sizes)
  await run(Object.keys(sizes), d)
  assert.equal(d.reads().length, 300)
  assert.ok(d.peak() <= CONCURRENCY, `peak ${d.peak()} > ${CONCURRENCY}`)
  assert.ok(d.peak() > 1, "reads run concurrently")
})

test("mapBounded keeps the order and the bound", async () => {
  let inFlight = 0
  let peak = 0
  const out = await mapBounded([5, 1, 3, 2, 4], 2, async (n) => {
    peak = Math.max(peak, ++inFlight)
    await new Promise((resolve) => setTimeout(resolve, n))
    inFlight--
    return n * 10
  })
  assert.deepEqual(out, [50, 10, 30, 20, 40])
  assert.equal(peak, 2)
})

test("a stat failure counts as unreadable and the file is not read", async () => {
  const d = disk({ "a.ttl": 10 })
  const { tally } = await run(["a.ttl", "gone.ttl"], d)
  assert.deepEqual(d.reads(), ["a.ttl"])
  assert.deepEqual(tally, { read: 1, tooBig: 0, tooMany: 0, unreadable: 1 })
})

test("a read failure (not UTF-8) counts as unreadable", async () => {
  const d = disk({ "a.ttl": 10, "latin1.ttl": 10 }, { unreadable: ["latin1.ttl"] })
  const { tally, batches } = await run(["a.ttl", "latin1.ttl"], d)
  assert.deepEqual(batches, [["a.ttl"]])
  assert.deepEqual(tally, { read: 1, tooBig: 0, tooMany: 0, unreadable: 1 })
})

test("a file larger when read than its stat said, past what is left, is dropped as too big", async () => {
  const d = disk({ "a.ttl": 10 })
  d.io.size = async () => 10
  d.io.read = async () => ({ text: "x", bytes: 1000 })
  const { tally, batches } = await run(["a.ttl"], d, { files: 10, bytes: 100 })
  assert.deepEqual(batches, [])
  assert.deepEqual(tally, { read: 0, tooBig: 1, tooMany: 0, unreadable: 0 })
})

test("a watched change uses the room left: a file that no longer fits is not read", async () => {
  const d = disk({ "a.ttl": 5 * MB })
  const { tally } = await run(["a.ttl"], d, { files: 1, bytes: 4 * MB })
  assert.deepEqual(d.reads(), [])
  assert.deepEqual(tally, { read: 0, tooBig: 1, tooMany: 0, unreadable: 0 })
})
