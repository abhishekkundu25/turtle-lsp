// Which nodes get a text label. Text is the expensive part of a frame, so:
// the selection, its neighbours and search matches always; the rest only
// when zoomed in, and then at most a few hundred, best connected first. A
// view with few nodes on screen labels them all. Pure: no DOM.

/** The zoom from which the other nodes get labels. */
export const LABEL_ZOOM = 1.2
/** At most this many labels in a frame. */
export const MAX_LABELS = 300
/** With this many nodes in view or fewer, every one is labelled. */
export const FEW = 60

/**
 * @param {object} o
 * @param {Iterable<number>} o.always the selection, its neighbours, matches
 * @param {number[]} o.inView the other nodes on screen
 * @param {number} o.zoom
 * @param {ArrayLike<number>} [o.deg] node degrees, to rank `inView`
 * @returns {number[]} node indices, `always` first
 */
export function pickLabels({ always = [], inView = [], zoom = 1, deg, max = MAX_LABELS, zoomMin = LABEL_ZOOM, few = FEW }) {
  const out = []
  const seen = new Set()
  for (const i of always) if (!seen.has(i)) seen.add(i), out.push(i)
  if (zoom < zoomMin && inView.length > few) return out
  const rest = inView.filter((i) => !seen.has(i))
  if (deg) rest.sort((a, b) => deg[b] - deg[a] || a - b)
  for (const i of rest) {
    if (out.length >= Math.max(max, seen.size)) break
    out.push(i)
  }
  return out
}

/** `s` cut to `n` characters with an ellipsis. */
export const truncate = (s, n = 32) => (s.length > n ? s.slice(0, n - 1) + "…" : s)
