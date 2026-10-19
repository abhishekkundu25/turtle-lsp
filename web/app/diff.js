// The diff view, as VS Code's diff editor: Monaco's diff editor over the
// editor area, comparing a file with its text on disk (File: Compare with
// Saved) or with its formatted text (RDF: Preview Formatting, whose Apply
// takes the right side). Side by side, inline when the area is narrow.
// Temporary models live only while the view is open.

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c])
const base = (path) => path.split("/").pop()

/**
 * Build the diff view (hidden until a comparison opens).
 * @param {object} o
 * @param {object} o.monaco
 * @param {{ request(method: string, params: object): Promise<any> }} o.client the language server
 * @param {HTMLElement} o.mount the element the view covers (the editor area)
 * @param {(message: string) => void} o.toast
 * @param {() => object} o.options the editor options of the editor groups
 * @param {(editor: object) => void} [o.setup] called with each side's editor (to apply the settings)
 * @param {() => void} [o.onClose] called after the view closes (to give the focus back)
 * @returns {{ compareWithSaved(path: string, file: { model: object, disk?: string | null }): void,
 *   previewFormatting(path: string, file: { model: object }): Promise<void>, close(): void, isOpen(): boolean }}
 */
export function createDiff({ monaco, client, mount, toast, options, setup, onClose }) {
  const page = document.createElement("section")
  page.className = "dif"
  page.hidden = true
  page.setAttribute("aria-label", "Diff")
  page.innerHTML = `
    <div class="dif-hd">
      <span class="codicon codicon-diff"></span>
      <span class="dif-title"></span>
      <button class="dif-btn" data-act="apply" hidden>Apply</button>
      <button class="dif-x" data-act="close" aria-label="Close Diff" title="Close (Escape)"><span class="codicon codicon-close"></span></button>
    </div>
    <div class="dif-host"></div>`
  mount.append(page)
  const title = page.querySelector(".dif-title")
  const applyBtn = page.querySelector("[data-act=apply]")
  let view = null // { diff, temp: models to dispose, apply?: () => void }

  page.addEventListener("click", (e) => {
    const act = e.target.closest("[data-act]")?.dataset.act
    if (act === "close") close()
    else if (act === "apply") view?.apply?.()
  })
  page.addEventListener("keydown", (e) => {
    if (e.key !== "Escape" || e.defaultPrevented) return
    e.preventDefault()
    e.stopPropagation()
    close()
  })

  // Show `original` (left) against `modified` (right). `temp`: models the
  // view owns; `apply`: what Apply does (no Apply button without it).
  function show({ left, right, original, modified, temp, editable, apply }) {
    close({ quiet: true })
    title.innerHTML = `${esc(left)} <span class="dif-arrow">↔</span> ${esc(right)}`
    title.title = `${left} ↔ ${right}`
    applyBtn.hidden = !apply
    page.hidden = false
    const host = document.createElement("div")
    host.className = "dif-editor"
    page.querySelector(".dif-host").append(host)
    const diff = monaco.editor.createDiffEditor(host, {
      ...options(),
      model: undefined,
      inlayHints: { enabled: "off" },
      "semanticHighlighting.enabled": false,
      // Monaco reports the cancelled highlight request of a disposed editor as an error.
      occurrencesHighlight: "off",
      renderSideBySide: true,
      useInlineViewWhenSpaceIsLimited: true,
      renderSideBySideInlineBreakpoint: 900,
      originalEditable: false,
      readOnly: !editable,
      renderMarginRevertIcon: false,
    })
    diff.setModel({ original, modified })
    for (const ed of [diff.getOriginalEditor(), diff.getModifiedEditor()]) {
      setup?.(ed)
      // Escape closes the view unless an editor widget wants it.
      ed.addCommand(monaco.KeyCode.Escape, () => close(), "!findWidgetVisible && !suggestWidgetVisible && !renameInputVisible && !parameterHintsVisible && !referenceSearchVisible && !markersNavigationVisible")
    }
    view = { diff, temp, host, apply }
    diff.getModifiedEditor().focus()
  }

  function close({ quiet } = {}) {
    if (!view) return
    const { diff, temp, host } = view
    view = null
    page.hidden = true
    // Disposing the diff editor while it computes logs an error: let it finish.
    let done = false
    const dispose = () => {
      if (done) return
      done = true
      diff.dispose()
      host.remove()
      for (const m of temp) m.dispose()
    }
    if (diff.getLineChanges() !== null) dispose()
    else {
      const sub = diff.onDidUpdateDiff(() => (sub.dispose(), dispose()))
      setTimeout(dispose, 2000)
    }
    if (!quiet) onClose?.()
  }

  function compareWithSaved(path, file) {
    if (!file) return
    if (file.disk === undefined) return toast("Only files of a folder opened in place have a version on disk")
    if (file.disk === null) return toast(`${base(path)} is not on disk yet`)
    const original = monaco.editor.createModel(file.disk, file.model.getLanguageId())
    show({ left: `${base(path)} (on disk)`, right: base(path), original, modified: file.model, temp: [original], editable: true })
  }

  async function previewFormatting(path, file) {
    if (!file) return
    const { model } = file
    const { tabSize, insertSpaces } = model.getOptions()
    const version = model.getVersionId()
    const edits = await client.request("textDocument/formatting", { textDocument: { uri: model.uri.toString() }, options: { tabSize, insertSpaces } })
    if (model.isDisposed()) return
    if (model.getVersionId() !== version) return toast("The file changed while formatting; try again")
    if (!edits?.length) return toast("Already formatted, or the file has syntax errors")
    const formatted = monaco.editor.createModel(model.getValue(), model.getLanguageId())
    formatted.applyEdits(edits.map((e) => ({ range: new monaco.Range(e.range.start.line + 1, e.range.start.character + 1, e.range.end.line + 1, e.range.end.character + 1), text: e.newText })))
    if (formatted.getValue() === model.getValue()) {
      formatted.dispose()
      return toast("Already formatted, or the file has syntax errors")
    }
    show({
      left: base(path),
      right: `${base(path)} (formatted)`,
      original: model,
      modified: formatted,
      temp: [formatted],
      editable: false,
      apply() {
        // Never write the formatting of an older text over newer edits.
        if (model.getVersionId() !== version) {
          close()
          return toast("The file changed since the preview; run Preview Formatting again")
        }
        const text = formatted.getValue()
        close()
        model.pushStackElement()
        model.pushEditOperations([], [{ range: model.getFullModelRange(), text }], () => null)
        model.pushStackElement()
      },
    })
  }

  return { compareWithSaved, previewFormatting, close, isOpen: () => view !== null }
}
