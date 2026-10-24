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
 * this tab waits, and `onOwn` runs when it gets the workspace: the other tab
 * went, or handed it over. `ask()` asks the owner to hand it over: the owner
 * runs `onLose({ asked: true })` (it writes what is waiting and stops
 * writing) and answers, and this tab takes the workspace, even from an owner
 * that does not answer in `wait` ms: that one gets `onLose({ asked: false })`
 * and must not write, since this tab may have read the workspace already.
 * Where locks do not exist the tab owns the workspace, as before.
 * @returns {Promise<{ owner: boolean, ask: () => Promise<void> }>}
 */
export function claim({ locks = globalThis.navigator?.locks, name = LOCK, onOwn = () => {}, onLose = async () => {}, wait = 3000 } = {}) {
  if (!locks?.request) return Promise.resolve({ owner: true, ask: async () => {} })
  const channel = new BroadcastChannel(name)
  let owner = false
  let queued = null // aborts this tab's place in the queue
  let answered = () => {}
  const hold = () => new Promise(() => {})
  // A held lock that ends is stolen (`ask` in another tab): this tab owns
  // nothing now. One that never held it was only taken out of the queue.
  function request(options) {
    let mine = false
    const take = () => ((mine = owner = true), (queued = null), onOwn(), hold())
    locks.request(name, options, take).catch(() => mine && lose({ asked: false }))
  }
  async function lose(how) {
    if (!owner) return
    owner = false
    await onLose(how)
    queue()
  }
  function queue() {
    queued = new AbortController()
    request({ signal: queued.signal })
  }
  channel.onmessage = async ({ data }) => {
    if (data === "hand-over" && owner) (await lose({ asked: true }), channel.postMessage("handed-over"))
    else if (data === "handed-over") answered()
  }
  async function ask() {
    if (owner) return
    const answer = new Promise((r) => (answered = r))
    channel.postMessage("hand-over")
    await Promise.race([answer, new Promise((r) => setTimeout(r, wait))])
    if (owner) return
    queued?.abort()
    request({ steal: true })
  }
  return new Promise((resolve) => {
    locks
      .request(name, { ifAvailable: true }, (lock) => {
        if (!lock) return (queue(), resolve({ owner: false, ask }))
        owner = true
        resolve({ owner: true, ask })
        return hold()
      })
      .catch(() => (owner ? lose({ asked: false }) : resolve({ owner: true, ask })))
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
