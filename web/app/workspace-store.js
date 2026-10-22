// Keeping the editor's workspace: which tab may write it, and what to write.
//
// Only one tab owns the workspace (`claim`); a second tab shows what was
// kept but never writes, so two tabs cannot overwrite each other. The owner
// writes only what changed: the files touched since the last write (with the
// revision it last saw of each) and the layout when that was touched.

import * as db from "./store.js"

export const LOCK = "rdf-editor-workspace"

/**
 * Take the workspace for this tab. `owner: false` means another tab has it;
 * `whenFree` then settles when that tab goes, and this tab holds the
 * workspace from then on. Where locks do not exist the tab owns the
 * workspace, as before.
 * @returns {Promise<{ owner: boolean, whenFree?: Promise<void> }>}
 */
export function claim({ locks = globalThis.navigator?.locks, name = LOCK } = {}) {
  if (!locks?.request) return Promise.resolve({ owner: true })
  const forever = new Promise(() => {})
  return new Promise((resolve) => {
    locks
      .request(name, { ifAvailable: true }, (lock) => {
        if (lock) {
          resolve({ owner: true })
          return forever
        }
        resolve({ owner: false, whenFree: new Promise((free) => locks.request(name, () => (free(), forever))) })
      })
      .catch(() => resolve({ owner: true }))
  })
}

/**
 * @typedef {object} Source what the store writes, asked for at write time
 * @property {(path: string) => ({ text: string, disk?: string | null } | null)} file a file now, or null if it is gone
 * @property {() => object} layout the layout record (folders, tabs, settings...)
 */

/**
 * @param {object} o
 * @param {Source} o.source
 * @param {string} [o.name] the database name
 * @param {number} [o.version] the database version
 * @param {number} [o.delay] ms after the last change before writing
 * @param {number} [o.maxWait] ms after the first change at the latest
 * @param {(state: "clean" | "pending" | "saving" | "error" | "readonly") => void} [o.onState]
 * @param {(paths: string[]) => void} [o.onConflict] files someone else changed and this write left alone
 */
export function createWorkspaceStore({ source, name = db.NAME, version = db.VERSION, delay = 300, maxWait = 2000, onState = () => {}, onConflict = () => {} }) {
  const revs = new Map() // path -> the revision last read or written
  const dirty = new Set()
  let layoutDirty = false
  let readOnly = false
  let state = "clean"
  let timer = null
  let first = 0 // when the oldest unwritten change was made
  let writing = null

  const set = (s) => {
    if (s === state) return
    state = s
    onState(s)
  }
  const arm = () => {
    clearTimeout(timer)
    const wait = Math.max(0, Math.min(delay, first + maxWait - Date.now()))
    timer = setTimeout(flush, wait)
  }
  const changed = () => {
    if (readOnly) return
    if (!first) first = Date.now()
    set("pending")
    arm()
  }

  /** The kept workspace as `{ files: [{ path, text, disk }], ...layout }`, or null. */
  async function load() {
    const kept = await db.readAll(name, version)
    if (!kept) return null
    revs.clear()
    for (const f of kept.files) revs.set(f.path, f.rev ?? 1)
    return { ...kept.layout, files: kept.files.map(({ path, text, disk }) => ({ path, text, disk })) }
  }

  /** Write what changed now; whether it was written. */
  async function flush() {
    clearTimeout(timer)
    if (readOnly) return false
    if (writing) {
      await writing
      return dirty.size || layoutDirty ? flush() : state === "clean"
    }
    if (!dirty.size && !layoutDirty) return state !== "error"
    const paths = [...dirty]
    const withLayout = layoutDirty
    dirty.clear()
    layoutDirty = false
    first = 0
    set("saving")
    const put = []
    const del = []
    for (const path of paths) {
      const f = source.file(path)
      if (f) put.push({ path, text: f.text, disk: f.disk, rev: revs.get(path) ?? 0 })
      else if (revs.has(path)) del.push(path)
    }
    writing = db.write({ put, del, layout: withLayout ? source.layout() : undefined }, name, version)
    const result = await writing
    writing = null
    if (result.ok) {
      for (const [path, rev] of result.revs) revs.set(path, rev)
      for (const path of del) revs.delete(path)
      if (result.conflicts.length) onConflict(result.conflicts)
    } else {
      for (const p of paths) dirty.add(p)
      layoutDirty ||= withLayout
    }
    if (!result.ok) set("error")
    else if (dirty.size || layoutDirty) {
      set("pending")
      first = Date.now()
      arm()
    } else set("clean")
    return result.ok
  }

  return {
    load,
    flush,
    /** The text or `disk` of `path` changed, or the file was made or removed. */
    touch(path) {
      if (readOnly) return
      dirty.add(path)
      changed()
    },
    /** Folders, tabs, settings... changed. */
    touchLayout() {
      if (readOnly) return
      layoutDirty = true
      changed()
    },
    /** A tab that does not own the workspace never writes. */
    setReadOnly(on) {
      readOnly = on
      if (on) {
        clearTimeout(timer)
        dirty.clear()
        layoutDirty = false
        set("readonly")
      } else if (state === "readonly") set("clean")
    },
    get state() {
      return state
    },
  }
}
