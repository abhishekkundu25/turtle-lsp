// Hit testing on the page: the drawn nodes bucketed in a uniform grid over
// their bounding box (world coordinates), rebuilt whenever positions change.
// A lookup scans only the cells within the radius. Pure: no DOM.

/**
 * @param {ArrayLike<number>} x
 * @param {ArrayLike<number>} y
 * @param {ArrayLike<number>} ids the node indices to include (others are never hit)
 * @param {number} [cell] cell size; by default about one node per cell
 * @returns {{ nearest(px: number, py: number, radius: number): number, cell: number }}
 */
export function createHitGrid(x, y, ids, cell) {
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity
  const list = []
  for (const i of ids) {
    if (!Number.isFinite(x[i]) || !Number.isFinite(y[i])) continue
    list.push(i)
    if (x[i] < x0) x0 = x[i]
    if (x[i] > x1) x1 = x[i]
    if (y[i] < y0) y0 = y[i]
    if (y[i] > y1) y1 = y[i]
  }
  if (!list.length) return { nearest: () => -1, cell: 1 }
  const w = x1 - x0 || 1, h = y1 - y0 || 1
  let size = cell > 0 ? cell : Math.max(Math.sqrt((w * h) / list.length), 1e-6)
  // Never more cells than a few per node.
  while (Math.ceil(w / size + 1) * Math.ceil(h / size + 1) > 4 * list.length + 16) size *= 1.5
  const cols = Math.ceil(w / size) + 1, rows = Math.ceil(h / size) + 1
  const cellOf = (i) => Math.floor((y[i] - y0) / size) * cols + Math.floor((x[i] - x0) / size)
  const start = new Int32Array(cols * rows + 1)
  for (const i of list) start[cellOf(i) + 1]++
  for (let c = 0; c < cols * rows; c++) start[c + 1] += start[c]
  const items = new Int32Array(list.length)
  const fill = start.slice(0, cols * rows)
  for (const i of list) items[fill[cellOf(i)]++] = i
  return {
    cell: size,
    /** The node nearest (`px`, `py`) within `radius`, ties to the lower index; -1 when none. */
    nearest(px, py, radius) {
      const c0 = Math.max(0, Math.floor((px - radius - x0) / size)), c1 = Math.min(cols - 1, Math.floor((px + radius - x0) / size))
      const r0 = Math.max(0, Math.floor((py - radius - y0) / size)), r1 = Math.min(rows - 1, Math.floor((py + radius - y0) / size))
      let best = -1, bd = radius * radius
      for (let r = r0; r <= r1; r++)
        for (let c = c0; c <= c1; c++)
          for (let q = start[r * cols + c]; q < start[r * cols + c + 1]; q++) {
            const i = items[q]
            const d = (x[i] - px) ** 2 + (y[i] - py) ** 2
            if (d < bd || (d === bd && (best < 0 || i < best))) (bd = d), (best = i)
          }
      return best
    },
  }
}
