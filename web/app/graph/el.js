// Building elements without markup strings: text always goes in as text, and
// nothing sets an inline handler or a style attribute (both hosts' policies
// refuse them).

/**
 * An element. `props`: `class`, `text`, and attributes (`false`/`null` leave
 * one out, `true` sets it empty). Children: nodes or strings, nested arrays allowed.
 * @returns {HTMLElement}
 */
export function h(doc, tag, props = {}, ...children) {
  const el = doc.createElement(tag)
  for (const [k, v] of Object.entries(props)) {
    if (v === false || v == null) continue
    if (k === "class") el.className = v
    else if (k === "text") el.textContent = v
    else el.setAttribute(k, v === true ? "" : String(v))
  }
  el.append(...kids(children))
  return el
}

/** `children` flattened, without null and false: for replaceChildren. */
export const kids = (...children) => {
  const out = []
  ;(function walk(list) {
    for (const c of list) if (Array.isArray(c)) walk(c)
    else if (c != null && c !== false) out.push(c)
  })(children)
  return out
}
