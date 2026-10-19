// The editor's workspace, kept in this browser (IndexedDB) between visits.
// Storage can be missing or refused (private windows); then nothing is kept
// and the editor works the same.

const DB = "rdf-toolbox-editor"
const STORE = "workspace"

function db() {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(DB, 1)
    req.onupgradeneeded = () => req.result.createObjectStore(STORE)
    req.onsuccess = () => resolve(req.result)
    req.onerror = () => reject(req.error)
  })
}

export async function load() {
  try {
    const d = await db()
    return await new Promise((resolve, reject) => {
      const req = d.transaction(STORE).objectStore(STORE).get("current")
      req.onsuccess = () => resolve(req.result ?? null)
      req.onerror = () => reject(req.error)
    })
  } catch {
    return null
  }
}

export async function save(state) {
  try {
    const d = await db()
    await new Promise((resolve, reject) => {
      const tx = d.transaction(STORE, "readwrite")
      tx.objectStore(STORE).put(state, "current")
      tx.oncomplete = resolve
      tx.onerror = () => reject(tx.error)
    })
    return true
  } catch {
    return false
  }
}
