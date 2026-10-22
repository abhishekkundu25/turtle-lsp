// Reading files into the pages: the list picked in a dialog, the items
// dropped from the desktop and a folder opened in place all become
// `{ file, path }` entries (`path`: below the chosen folder, "/"-separated),
// under one set of limits so no folder can stall a tab.

/** The files a folder contributes, by extension; anything else is skipped. */
export const KNOWN = /\.(ttl|turtle|trig|nt|nq|rq|sparql|ru|jsonld)$/i

export const LIMITS = {
  files: 1000, // files taken in one import
  fileBytes: 10 * 1024 * 1024, // the largest file taken
  scan: 20_000, // entries looked at while walking, so a huge tree ends
  depth: 24, // folder levels walked
}

/** Folders never walked: hidden ones and dependencies. */
export const skipsFolder = (name) => name.startsWith(".") || name === "node_modules"

/** The files chosen in a dialog; a folder chosen keeps `webkitRelativePath`. */
export const picked = (list) => [...list].map((file) => ({ file, path: file.webkitRelativePath || file.name }))

const readEntries = (reader) => new Promise((res, rej) => reader.readEntries(res, rej))

/**
 * The files under the dropped `items` (DataTransferItems), folders walked.
 * Files inside folders are taken only when `KNOWN`; the others, and what the
 * walk gave up on (`limits.scan`, `limits.depth`), count in `skipped`.
 * @returns {Promise<{ entries: { file: File, path: string }[], skipped: number }>}
 */
export async function walkDropped(items, limits = LIMITS) {
  const entries = []
  let seen = 0
  let skipped = 0
  let truncated = false
  const walk = async (entry, depth, inFolder) => {
    if (seen++ >= limits.scan || depth > limits.depth) return void (truncated = true)
    if (entry.isFile) {
      if (inFolder && !KNOWN.test(entry.name)) return void skipped++
      const file = await new Promise((res, rej) => entry.file(res, rej))
      entries.push({ file, path: entry.fullPath.replace(/^\//, "") })
    } else if (entry.isDirectory) {
      if (depth > 0 && skipsFolder(entry.name)) return
      const reader = entry.createReader()
      for (;;) {
        const batch = await readEntries(reader)
        if (!batch.length) break
        for (const e of batch) await walk(e, depth + 1, true)
      }
    }
  }
  for (const entry of [...items].map((i) => i.webkitGetAsEntry?.()).filter(Boolean)) await walk(entry, 0, false)
  return { entries, skipped: skipped + (truncated ? 1 : 0) }
}

/**
 * The known files under a directory handle (File System Access API), as
 * paths below `prefix`. Hidden folders and `node_modules` are not walked.
 * `skipped` is 1 when the walk gave up on part of the tree.
 * @returns {Promise<{ entries: { file: File, path: string, onDisk: true }[], skipped: number }>}
 */
export async function walkHandle(dir, prefix = "", limits = LIMITS) {
  const entries = []
  let seen = 0
  let truncated = false
  const walk = async (handle, base, depth) => {
    for await (const [name, h] of handle.entries()) {
      if (seen++ >= limits.scan) return void (truncated = true)
      const path = base ? `${base}/${name}` : name
      if (h.kind === "directory") {
        if (skipsFolder(name)) continue
        if (depth >= limits.depth) {
          truncated = true
          continue
        }
        await walk(h, path, depth + 1)
        if (truncated) return
      } else if (KNOWN.test(name)) {
        entries.push({ file: await h.getFile(), path, onDisk: true })
      }
    }
  }
  await walk(dir, prefix, 0)
  return { entries, skipped: truncated ? 1 : 0 }
}

/**
 * Which `entries` to take: at most `limits.files` in all (`have` already
 * taken), none over `limits.fileBytes`, and with `onlyKnown` only those with a
 * known extension.
 * @returns {{ take: object[], skipped: number }}
 */
export function admit(entries, { limits = LIMITS, have = 0, onlyKnown = true } = {}) {
  const take = []
  let skipped = 0
  for (const e of entries) {
    if ((onlyKnown && !KNOWN.test(e.file.name)) || e.file.size > limits.fileBytes || have + take.length >= limits.files) skipped++
    else take.push(e)
  }
  return { take, skipped }
}

/** Why files are left out, for a note after `admit`. */
export const limitsNote = (limits = LIMITS) => `only Turtle, TriG, N-Triples, N-Quads, JSON-LD and SPARQL files up to ${limits.fileBytes / 1024 / 1024} MB are read, at most ${limits.files}`

/**
 * The text of a fetch `response`, read as it arrives; null (and the rest not
 * read) when it is longer than `max` bytes, whatever its `content-length` says.
 */
export async function readTextCapped(response, max = LIMITS.fileBytes) {
  const declared = Number(response.headers.get("content-length"))
  if (Number.isFinite(declared) && declared > max) {
    await response.body?.cancel().catch(() => {})
    return null
  }
  if (!response.body) {
    const text = await response.text()
    return new TextEncoder().encode(text).length > max ? null : text
  }
  const reader = response.body.getReader()
  const chunks = []
  let size = 0
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    size += value.byteLength
    if (size > max) {
      await reader.cancel().catch(() => {})
      return null
    }
    chunks.push(value)
  }
  const bytes = new Uint8Array(size)
  let at = 0
  for (const c of chunks) (bytes.set(c, at), (at += c.byteLength))
  return new TextDecoder().decode(bytes)
}
