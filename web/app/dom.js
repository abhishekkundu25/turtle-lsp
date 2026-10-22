// Page helpers every view shares.

const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }

/** Text safe to put in HTML, also inside a quoted attribute value. */
export const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ESCAPES[c])

/** The element with this id. */
export const $ = (id) => document.getElementById(id)
