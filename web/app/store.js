// The editor's workspace, kept in this browser (IndexedDB) between visits.
// Storage can be missing or refused (private windows); then nothing is kept
// and the editor works the same.
//
// Layout: one record per file in "files" ({ path, text, disk, rev }) and the
// rest of the workspace (folders, tabs, settings...) under "layout" in
// "meta". The connection is opened once and kept; when another tab needs to
// upgrade the database, this one lets go (`versionchange`) and opens again on
// its next use.

export const NAME = "rdf-toolbox-editor"
// Version 1 kept the whole workspace as one record, "current", in "workspace".
export const VERSION = 2
const FILES = "files"
const META = "meta"
const LEGACY = "workspace"

const connections = new Map() // name -> Promise<IDBDatabase>

function upgrade(db, tx, oldVersion) {
  if (oldVersion >= 2) return
  const files = db.createObjectStore(FILES, { keyPath: "path" })
  const meta = db.createObjectStore(META)
  if (oldVersion !== 1) return
  tx.objectStore(LEGACY).get("current").onsuccess = (e) => {
    const { files: old = [], ...layout } = e.target.result ?? {}
    for (const f of old) files.put({ path: f.path, text: f.text, disk: f.disk, rev: 1 })
    if (e.target.result) meta.put(layout, "layout")
    db.deleteObjectStore(LEGACY)
  }
}

/** The open database `name`: one connection per name, kept. */
export function open(name = NAME, version = VERSION) {
  let p = connections.get(name)
  if (!p) {
    p = new Promise((resolve, reject) => {
      const req = indexedDB.open(name, version)
      req.onupgradeneeded = (e) => upgrade(req.result, req.transaction, e.oldVersion)
      req.onsuccess = () => {
        const db = req.result
        const forget = () => connections.get(name) === p && connections.delete(name)
        db.onversionchange = () => (db.close(), forget())
        db.onclose = forget
        resolve(db)
      }
      req.onerror = () => reject(req.error)
    })
    p.catch(() => connections.get(name) === p && connections.delete(name))
    connections.set(name, p)
  }
  return p
}

/**
 * What is kept: `{ files: [{ path, text, disk, rev }], layout }`, or null when
 * nothing is (or storage is refused).
 */
export async function readAll(name = NAME, version = VERSION) {
  try {
    const db = await open(name, version)
    return await new Promise((resolve, reject) => {
      const tx = db.transaction([FILES, META])
      const out = { files: [], layout: null }
      tx.objectStore(FILES).getAll().onsuccess = (e) => (out.files = e.target.result)
      tx.objectStore(META).get("layout").onsuccess = (e) => (out.layout = e.target.result ?? null)
      tx.oncomplete = () => resolve(out.layout ? out : null)
      tx.onerror = tx.onabort = () => reject(tx.error)
    })
  } catch {
    return null
  }
}

/**
 * Write in one transaction. `put`: [{ path, text, disk, rev }] where `rev` is
 * the revision this tab last saw of the file (0 for a file it made): a file
 * whose stored revision differs was changed by someone else meanwhile, and is
 * left alone (reported in `conflicts`). `del`: paths to remove. `layout`: the
 * new layout record.
 * @returns {Promise<{ ok: boolean, revs: Map<string, number>, conflicts: string[] }>}
 */
export async function write({ put = [], del = [], layout } = {}, name = NAME, version = VERSION) {
  const revs = new Map()
  const conflicts = []
  try {
    const db = await open(name, version)
    return await new Promise((resolve) => {
      const tx = db.transaction([FILES, META], "readwrite")
      const files = tx.objectStore(FILES)
      for (const f of put) {
        files.get(f.path).onsuccess = (e) => {
          const was = e.target.result?.rev ?? 0
          if (was !== (f.rev ?? 0)) return void conflicts.push(f.path)
          revs.set(f.path, was + 1)
          files.put({ path: f.path, text: f.text, disk: f.disk, rev: was + 1 })
        }
      }
      for (const p of del) files.delete(p)
      if (layout) tx.objectStore(META).put(layout, "layout")
      tx.oncomplete = () => resolve({ ok: true, revs, conflicts })
      tx.onerror = tx.onabort = () => resolve({ ok: false, revs: new Map(), conflicts: [] })
    })
  } catch {
    return { ok: false, revs: new Map(), conflicts: [] }
  }
}
